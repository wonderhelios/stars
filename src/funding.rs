//! 资金费套利面板数据（只读，不发单）：
//! ① HL↔币安 资金费差（同币种两边年化资金费之差，正 = HL 更贵 → 空 HL 多币安收钱）
//! ② 尖峰监控（HL 主盘 + 全部 HIP-3 所，标出极端年化资金费）
//!
//! 数据来源全部是公开接口：Hyperliquid /info 与币安 fapi。币安可能被地区屏蔽，
//! 拿不到时优雅降级（bn_error 标注），不影响 HL 侧数据。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

const HL_BASE: &str = "https://api.hyperliquid.xyz/info";
/// 尖峰阈值：年化 |资金费| 达到 40% 才算尖峰。
const SPIKE_ANN_MIN: f64 = 0.40;
/// 尖峰的最低 24h 成交额（美元），过滤僵尸市场。
const SPIKE_VOL_MIN: f64 = 10_000.0;
/// 资金费差面板的流动性门槛：两边 24h 成交额都超过才显示。
const PAIR_VOL_MIN: f64 = 1_000_000.0;
/// 面板最多保留的尖峰条数。
const SPIKE_MAX: usize = 50;

/// 从 JSON 里解析数字（HL 把金额/费率都返回成字符串）。
fn num(v: &Value) -> f64 {
    v.as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| v.as_f64())
        .unwrap_or(0.0)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FundingPair {
    pub coin: String,
    /// HL 年化资金费（小数，0.05 = 5%/年）。正 = 多头付空头。
    pub hl_ann: f64,
    /// 币安年化资金费（8h 结算 ×3 ×365）。
    pub bn_ann: f64,
    /// 差 = hl − bn。正 = HL 更贵：空 HL、多币安可以收这个差。
    pub diff_ann: f64,
    pub hl_vol: f64,
    pub bn_vol: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FundingSpike {
    /// "BTC"（主盘）或 "xyz:KORU"（HIP-3）。
    pub market: String,
    /// 年化资金费，可负（负 = 空头付多头，做多收钱）。
    pub ann: f64,
    /// 24h 成交额（美元）。
    pub vol: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct FundingState {
    pub updated: Option<i64>,
    pub pairs: Vec<FundingPair>,
    pub spikes: Vec<FundingSpike>,
    /// 币安数据拿不到时的说明。
    pub bn_error: Option<String>,
}

async fn post_hl(http: &reqwest::Client, payload: Value) -> Option<Value> {
    for _ in 0..2 {
        match http.post(HL_BASE).json(&payload).send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<Value>().await {
                Ok(v) => return Some(v),
                Err(_) => {}
            },
            _ => {}
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    None
}

/// 拉取币安公开接口的 JSON；失败返回 None（调用方负责记录降级说明）。
async fn fetch_bn_json(client: &reqwest::Client, url: &str) -> Option<Value> {
    match client.get(url).send().await {
        Ok(resp) if resp.status().is_success() => resp.json::<Value>().await.ok(),
        _ => None,
    }
}

/// 扫描一个所的 metaAndAssetCtxs，收集尖峰与（可选的）主盘资金费表。
fn scan_universe(
    v: &Value,
    spikes: &mut Vec<FundingSpike>,
    hl_fund: &mut HashMap<String, (f64, f64)>,
    is_main: bool,
) {
    let Some(uni) = v[0]["universe"].as_array() else {
        return;
    };
    let Some(ctxs) = v[1].as_array() else {
        return;
    };
    for (i, x) in uni.iter().enumerate() {
        let Some(name) = x["name"].as_str() else {
            continue;
        };
        let Some(ctx) = ctxs.get(i) else {
            continue;
        };
        let ann = num(&ctx["funding"]) * 24.0 * 365.0;
        let vol = num(&ctx["dayNtlVlm"]);
        if is_main && !name.contains(':') {
            hl_fund.insert(name.to_string(), (ann, vol));
        }
        if ann.abs() >= SPIKE_ANN_MIN && vol >= SPIKE_VOL_MIN {
            spikes.push(FundingSpike {
                market: name.to_string(),
                ann,
                vol,
            });
        }
    }
}

/// 拉取全部数据并计算（约 13 个公开接口请求，后台每 60 秒调一次）。
pub async fn refresh(http: &reqwest::Client, binance_http: &reqwest::Client) -> FundingState {
    let mut st = FundingState {
        updated: Some(crate::live::now_ms_pub()),
        ..Default::default()
    };

    // ---- ① HL 主盘 + ② HIP-3 各所 ----
    let mut hl_fund: HashMap<String, (f64, f64)> = HashMap::new();
    if let Some(v) = post_hl(http, json!({"type": "metaAndAssetCtxs"})).await {
        scan_universe(&v, &mut st.spikes, &mut hl_fund, true);
    }
    if let Some(v) = post_hl(http, json!({"type": "perpDexs"})).await {
        if let Some(arr) = v.as_array() {
            for dex_val in arr {
                let Some(dex) = dex_val["name"].as_str() else {
                    continue;
                };
                if dex.is_empty() {
                    continue;
                }
                let payload = json!({"type": "metaAndAssetCtxs", "dex": dex});
                if let Some(d) = post_hl(http, payload).await {
                    scan_universe(&d, &mut st.spikes, &mut hl_fund, false);
                }
            }
        }
    }
    st.spikes
        .sort_by(|a, b| b.ann.abs().partial_cmp(&a.ann.abs()).unwrap_or(std::cmp::Ordering::Equal));
    st.spikes.truncate(SPIKE_MAX);

    // ---- ③ 币安：资金费 + 24h 成交额 ----
    let mut bn_fund: HashMap<String, f64> = HashMap::new();
    let mut bn_vol: HashMap<String, f64> = HashMap::new();
    let mut bn_error: Option<String> = None;

    if let Some(v) = fetch_bn_json(binance_http, "https://fapi.binance.com/fapi/v1/premiumIndex").await {
        if let Some(arr) = v.as_array() {
            for x in arr {
                let Some(sym) = x["symbol"].as_str() else {
                    continue;
                };
                if !sym.ends_with("USDT") || sym.len() <= 4 {
                    continue;
                }
                let coin = &sym[..sym.len() - 4];
                let rate = num(&x["lastFundingRate"]);
                bn_fund.insert(coin.to_string(), rate * 3.0 * 365.0);
            }
        } else {
            bn_error = Some("币安 premiumIndex 响应异常".into());
        }
    } else {
        bn_error = Some("币安不可达".into());
    }
    if bn_error.is_none() {
        match fetch_bn_json(binance_http, "https://fapi.binance.com/fapi/v1/ticker/24hr").await {
            Some(v) => {
                if let Some(arr) = v.as_array() {
                    for x in arr {
                        let Some(sym) = x["symbol"].as_str() else {
                            continue;
                        };
                        if !sym.ends_with("USDT") || sym.len() <= 4 {
                            continue;
                        }
                        let coin = &sym[..sym.len() - 4];
                        let vol = num(&x["quoteVolume"]);
                        bn_vol.insert(coin.to_string(), vol);
                    }
                } else {
                    bn_error = Some("币安 ticker/24hr 响应异常".into());
                }
            }
            None => bn_error = Some("币安 ticker/24hr 不可达".into()),
        }
    }
    st.bn_error = bn_error;

    // ---- ④ 合并成资金费差对 ----
    for (coin, (hl_ann, hl_vol)) in hl_fund {
        let Some(&bn_ann) = bn_lookup(&bn_fund, &coin) else {
            continue;
        };
        let bv = bn_lookup(&bn_vol, &coin).copied().unwrap_or(0.0);
        if hl_vol >= PAIR_VOL_MIN && bv >= PAIR_VOL_MIN {
            st.pairs.push(FundingPair {
                coin,
                hl_ann,
                bn_ann,
                diff_ann: hl_ann - bn_ann,
                hl_vol,
                bn_vol: bv,
            });
        }
    }
    st.pairs
        .sort_by(|a, b| b.diff_ann.abs().partial_cmp(&a.diff_ann.abs()).unwrap_or(std::cmp::Ordering::Equal));
    st
}

/// 币安对高供应 meme 币只挂 `1000PEPEUSDT` / `1000SHIBUSDT` 等合约，
/// 而 HL 用 `PEPE` / `SHIB` / `kPEPE`。这里做兼容映射：
/// 依次尝试 `coin`、`1000{coin}`、以及 HL 的 k 前缀（`kPEPE` → `1000PEPE`）。
fn bn_lookup<'a>(m: &'a HashMap<String, f64>, coin: &str) -> Option<&'a f64> {
    if let Some(v) = m.get(coin) {
        return Some(v);
    }
    if let Some(v) = m.get(&format!("1000{coin}")) {
        return Some(v);
    }
    if let Some(stripped) = coin.strip_prefix('k') {
        if let Some(v) = m.get(&format!("1000{stripped}")) {
            return Some(v);
        }
    }
    None
}
