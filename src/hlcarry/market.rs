//! Hyperliquid spot + perp market data fetcher and parser for the carry shadow.

use anyhow::{Context, Result};
use serde_json::{json, Value};

const HL_INFO: &str = "https://api.hyperliquid.xyz/info";

// ---------------------------------------------------------------------------
// 行情
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Quote {
    pub spot: f64,
    pub perp: f64,
    pub funding_ann: f64,
}

/// 一个"两边都有盘、且能配对"的标的快照。
#[derive(Clone, Debug)]
pub struct Latch {
    pub coin: String,
    pub spot: String,
    pub funding_ann: f64,
    pub spot_volume: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Market {
    pub perp_mid: std::collections::HashMap<String, f64>,
    pub spot_mid: std::collections::HashMap<String, f64>,
    pub funding: std::collections::HashMap<String, f64>,
    /// 实际资金费结算记录：(结算时间 ms, 当期费率)，只用于持仓计提。
    pub funding_history: std::collections::HashMap<String, Vec<(i64, f64)>>,
    /// 现货 24h 成交额（按现货对名）。
    pub spot_volume: std::collections::HashMap<String, f64>,
    /// 已配对的标的（永续名 + 现货对），带成交额与资金费。
    pub latched: Vec<Latch>,
}

impl Market {
    pub fn quote(&self, coin: &str, spot: &str) -> Option<Quote> {
        let perp = *self.perp_mid.get(coin)?;
        let sp = *self.spot_mid.get(spot)?;
        let fh = *self.funding.get(coin).unwrap_or(&0.0);
        Some(Quote {
            spot: sp,
            perp,
            funding_ann: fh * 24.0 * 365.0 * 100.0,
        })
    }
}

/// 现货 token 名 → 永续币名。只接受同名或明确验证过的桥接资产。
/// 绝不能删除末尾数字：XMR1 与 XMR2 是不同发行方的不同资产。
pub fn perp_of_spot_token(token: &str, perps: &std::collections::HashSet<String>) -> Option<String> {
    if perps.contains(token) {
        return Some(token.to_string());
    }
    let mapped = match token {
        "UBTC" => "BTC",
        "UETH" => "ETH",
        "USOL" => "SOL",
        "XMR1" => "XMR",
        _ => return None,
    };
    perps.contains(mapped).then(|| mapped.to_string())
}

async fn post(http: &reqwest::Client, payload: Value) -> Result<Value> {
    let resp = http
        .post(HL_INFO)
        .json(&payload)
        .send()
        .await
        .context("HL info 请求失败")?;
    let v: Value = resp.json().await.context("HL info 解析失败")?;
    Ok(v)
}

/// 拉一次全市场快照：3 个公开请求。新建仓不需要历史资金费。
pub async fn fetch_market(http: &reqwest::Client) -> Result<Market> {
    let mids = post(http, json!({"type": "allMids"})).await?;
    let ctxs = post(http, json!({"type": "metaAndAssetCtxs"})).await?;
    let spot = post(http, json!({"type": "spotMetaAndAssetCtxs"})).await?;
    Ok(parse_market(&mids, &ctxs, &spot))
}

/// 持仓更新必须额外读取每个持仓币的实际资金费结算记录。
pub async fn fetch_market_with_funding(http: &reqwest::Client, coins: &[String]) -> Result<Market> {
    let mut market = fetch_market(http).await?;
    let now = crate::live::now_ms_pub();
    for coin in coins.iter().collect::<std::collections::HashSet<_>>() {
        let rows = post(http, json!({
            "type": "fundingHistory", "coin": coin,
            "startTime": now - 48 * 3_600_000, "endTime": now,
        })).await?;
        let arr = rows.as_array().context("fundingHistory 响应不是数组")?;
        let mut history = Vec::new();
        for row in arr {
            let Some(t) = row.get("time").and_then(|v| v.as_i64()) else { continue };
            let Some(rate) = row.get("fundingRate").and_then(|v| v.as_str())
                .and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite()) else { continue };
            history.push((t, rate));
        }
        history.sort_by_key(|x| x.0);
        market.funding_history.insert(coin.clone(), history);
    }
    Ok(market)
}

/// 纯解析：三个接口的原始 JSON → Market。抽出来便于用真实报文做单测。
pub fn parse_market(mids: &Value, ctxs: &Value, spot: &Value) -> Market {
    let mut m = Market::default();

    if let Some(obj) = mids.as_object() {
        for (k, v) in obj {
            let Some(px) = v.as_str().and_then(|s| s.parse::<f64>().ok()) else {
                continue;
            };
            if !(px > 0.0) {
                continue;
            }
            if k.starts_with('@') || k.contains('/') {
                m.spot_mid.insert(k.clone(), px);
            } else {
                m.perp_mid.insert(k.clone(), px);
            }
        }
    }

    let mut perp_set = std::collections::HashSet::new();
    if let Some(arr) = ctxs.as_array() {
        if arr.len() > 1 {
            if let (Some(meta), Some(list)) = (arr[0].get("universe"), arr[1].as_array()) {
                let empty = vec![];
                let uni = meta.as_array().unwrap_or(&empty);
                for (u, c) in uni.iter().zip(list.iter()) {
                    let Some(coin) = u.get("name").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    perp_set.insert(coin.to_string());
                    let f = c
                        .get("funding")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    m.funding.insert(coin.to_string(), f);
                }
            }
        }
    }

    // 现货：meta 给 token 名与 pair→token 映射，ctxs 给 pair 的成交额。
    let (smeta, sctxs) = match spot.as_array() {
        Some(a) if a.len() > 1 => (a[0].clone(), a[1].as_array().cloned().unwrap_or_default()),
        _ => (Value::Null, Vec::new()),
    };
    let tokens: Vec<String> = smeta
        .get("tokens")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .map(|t| {
                    t.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    let mut vol_by_pair: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for c in sctxs.iter() {
        let Some(pair) = c.get("coin").and_then(|v| v.as_str()) else {
            continue;
        };
        let v = c
            .get("dayNtlVlm")
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        vol_by_pair.insert(pair.to_string(), v);
    }

    if let Some(uni) = smeta.get("universe").and_then(|v| v.as_array()) {
        for u in uni {
            let Some(name) = u.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(idx) = u.get("tokens").and_then(|v| v.as_array()) else {
                continue;
            };
            let Some(base_i) = idx.first().and_then(|v| v.as_u64()) else {
                continue;
            };
            let Some(base) = tokens.get(base_i as usize) else {
                continue;
            };
            let Some(coin) = perp_of_spot_token(base, &perp_set) else {
                continue;
            };
            let vol = *vol_by_pair.get(name).unwrap_or(&0.0);
            m.spot_volume.insert(name.to_string(), vol);
            let fh = *m.funding.get(&coin).unwrap_or(&0.0);
            if m.spot_mid.contains_key(name) && m.perp_mid.contains_key(&coin) {
                m.latched.push(Latch {
                    coin,
                    spot: name.to_string(),
                    funding_ann: fh * 24.0 * 365.0 * 100.0,
                    spot_volume: vol,
                });
            }
        }
    }
    m
}

