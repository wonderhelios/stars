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
    /// HL 标记价（纸面 carry 的标记用）。
    pub hl_px: f64,
    /// 币安标记价。
    pub bn_px: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FundingSpike {
    /// "BTC"（主盘）或 "xyz:KORU"（HIP-3）。
    pub market: String,
    /// 年化资金费，可负（负 = 空头付多头，做多收钱）。
    pub ann: f64,
    /// 24h 成交额（美元）。
    pub vol: f64,
    /// 币安是否有同名永续（有 = 可以对冲，才是可执行的尖峰）。
    #[serde(default)]
    pub hedgeable: bool,
}

/// 跨所原始行情（所有两边达标的币 ✗ 不按价差过滤 ✗ 供纸面引擎盯市 ✗）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct XPrice {
    pub coin: String,
    pub okx: f64,
    pub bn: f64,
    /// OKX 24h 成交额（美元 ✗ 用于流动性过滤 ✗）
    pub okx_vol: f64,
    /// 币安 24h 成交额（美元 ✗）
    pub bn_vol: f64,
}

/// 跨所现货价差（OKX vs 币安）。用户当年 G/USDT 那种机会。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct XVenue {
    pub coin: String,
    pub okx_px: f64,
    pub bn_px: f64,
    /// (OKX / 币安 − 1) × 100，正 = OKX 更贵。
    pub diff_pct: f64,
    pub okx_vol: f64,
    pub bn_vol: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct FundingState {
    pub updated: Option<i64>,
    pub pairs: Vec<FundingPair>,
    pub spikes: Vec<FundingSpike>,
    /// 币安数据拿不到时的说明。
    pub bn_error: Option<String>,
    /// 跨所现货价差（只保留两边成交额都达标、且 |差| 超过阈值的）。
    #[serde(default)]
    pub xvenue: Vec<XVenue>,
    /// 未过滤的原始价差行情（供纸面引擎盯市）。
    #[serde(default)]
    pub xvenue_px: Vec<XPrice>,
    #[serde(default)]
    pub xvenue_error: Option<String>,
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
    hl_fund: &mut HashMap<String, (f64, f64, f64)>, // coin -> (ann, vol, markPx)
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
        let px = num(&ctx["markPx"]);
        if is_main && !name.contains(':') {
            hl_fund.insert(name.to_string(), (ann, vol, px));
        }
        if ann.abs() >= SPIKE_ANN_MIN && vol >= SPIKE_VOL_MIN {
            spikes.push(FundingSpike {
                market: name.to_string(),
                ann,
                vol,
                hedgeable: false,
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
    let mut hl_fund: HashMap<String, (f64, f64, f64)> = HashMap::new();
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

    // ---- ③ 币安：资金费 + 标记价 + 24h 成交额 ----
    let mut bn_fund: HashMap<String, f64> = HashMap::new();
    let mut bn_px: HashMap<String, f64> = HashMap::new();
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
                bn_px.insert(coin.to_string(), num(&x["markPrice"]));
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
    for (coin, (hl_ann, hl_vol, hl_px)) in hl_fund {
        let Some(&bn_ann) = bn_lookup(&bn_fund, &coin) else {
            continue;
        };
        let bv = bn_lookup(&bn_vol, &coin).copied().unwrap_or(0.0);
        let bpx = bn_lookup(&bn_px, &coin).copied().unwrap_or(0.0);
        if hl_vol >= PAIR_VOL_MIN && bv >= PAIR_VOL_MIN {
            st.pairs.push(FundingPair {
                coin,
                hl_ann,
                bn_ann,
                diff_ann: hl_ann - bn_ann,
                hl_vol,
                bn_vol: bv,
                hl_px,
                bn_px: bpx,
            });
        }
    }
    st.pairs
        .sort_by(|a, b| b.diff_ann.abs().partial_cmp(&a.diff_ann.abs()).unwrap_or(std::cmp::Ordering::Equal));

    // ---- ⑤ 尖峰是否可对冲（币安有同名永续才可执行）----
    for sp in st.spikes.iter_mut() {
        let base = sp.market.rsplit(':').next().unwrap_or(&sp.market);
        sp.hedgeable = bn_lookup(&bn_fund, base).is_some();
    }
    st.spikes.sort_by(|a, b| {
        b.hedgeable
            .cmp(&a.hedgeable)
            .then_with(|| b.ann.abs().partial_cmp(&a.ann.abs()).unwrap_or(std::cmp::Ordering::Equal))
    });

    // ---- ⑥ 跨所现货价差（OKX vs 币安）----
    // 用户当年 G/USDT 那种"同一币在两个所价格不一样"的机会。只保留两边
    // 成交额都 ≥ $1M 的币（否则扫出来全是同名不同币或僵尸价），并且只在
    // |差| ≥ 0.3% 时才记录。
    let mut okx_px: HashMap<String, (f64, f64)> = HashMap::new();
    match fetch_okx_spot(binance_http).await {
        Ok(list) => {
            for (coin, px, vol) in list {
                okx_px.insert(coin, (px, vol));
            }
        }
        Err(e) => st.xvenue_error = Some(e),
    }
    if st.xvenue_error.is_none() {
        let mut bn_spot: HashMap<String, (f64, f64)> = HashMap::new();
        if let Some(v) = fetch_bn_json(binance_http, "https://api.binance.com/api/v3/ticker/24hr").await {
            if let Some(arr) = v.as_array() {
                for x in arr {
                    let Some(sym) = x["symbol"].as_str() else { continue };
                    if !sym.ends_with("USDT") || sym.len() <= 4 {
                        continue;
                    }
                    let coin = &sym[..sym.len() - 4];
                    let px = num(&x["lastPrice"]);
                    let vol = num(&x["quoteVolume"]);
                    if px > 0.0 {
                        bn_spot.insert(coin.to_string(), (px, vol));
                    }
                }
            }
        }
        for (coin, (opx, ovol)) in okx_px {
            let Some(&(bpx, bvol)) = bn_spot.get(&coin) else { continue };
            if opx <= 0.0 || bpx <= 0.0 || ovol < 1_000_000.0 || bvol < 1_000_000.0 {
                continue;
            }
            st.xvenue_px.push(XPrice {
                coin: coin.clone(),
                okx: opx,
                bn: bpx,
                okx_vol: ovol,
                bn_vol: bvol,
            });
            let diff = (opx / bpx - 1.0) * 100.0;
            if diff.abs() >= 0.3 {
                st.xvenue.push(XVenue {
                    coin,
                    okx_px: opx,
                    bn_px: bpx,
                    diff_pct: diff,
                    okx_vol: ovol,
                    bn_vol: bvol,
                });
            }
        }
        st.xvenue
            .sort_by(|a, b| b.diff_pct.abs().partial_cmp(&a.diff_pct.abs()).unwrap_or(std::cmp::Ordering::Equal));
        st.xvenue.truncate(30);
    }

    st
}

/// 拉 OKX 现货 ticker，返回 (币, 最新价, 24h 成交额)。
async fn fetch_okx_spot(client: &reqwest::Client) -> Result<Vec<(String, f64, f64)>, String> {
    let resp = client
        .get("https://www.okx.com/api/v5/market/tickers?instType=SPOT")
        .send()
        .await
        .map_err(|e| format!("OKX 不可达: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("OKX HTTP {}", resp.status()));
    }
    let v: Value = resp.json().await.map_err(|e| format!("OKX 响应异常: {e}"))?;
    if v["code"].as_str() != Some("0") {
        return Err(format!("OKX code={}", v["code"]));
    }
    let Some(arr) = v["data"].as_array() else {
        return Err("OKX 数据为空".into());
    };
    let mut out = Vec::new();
    for t in arr {
        let Some(id) = t["instId"].as_str() else { continue };
        let Some(coin) = id.strip_suffix("-USDT") else { continue };
        let px = num(&t["last"]);
        let vol = num(&t["volCcy24h"]);
        if px > 0.0 {
            out.push((coin.to_string(), px, vol));
        }
    }
    Ok(out)
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
