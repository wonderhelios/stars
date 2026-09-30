use futures_util::{stream, StreamExt};
use reqwest::Client;
use rust_decimal::Decimal;
use serde_json::json;
use std::str::FromStr;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::signal::outcome_bar_open;

const BASE: &str = "https://api.hyperliquid.xyz/info";
const MAX_RETRY: u32 = 3;
const RETRY_SLEEP_MS: u64 = 400;

#[derive(serde::Deserialize)]
struct PerpDex {
    name: String,
}

#[derive(Debug, Clone)]
pub struct HlTicker {
    pub coin: String,
    pub mark_px: Decimal,
    pub prev_day_px: Decimal,
    pub day_ntl_vlm: Decimal,
    pub funding: Decimal,
    pub max_leverage: Option<u32>,
    pub size_decimals: Option<u32>,
    #[allow(dead_code)]
    pub open_interest: Decimal,
    /// Exchange-provided impact bid/ask, not the best top-of-book quote.
    pub impact_bid: Option<Decimal>,
    pub impact_ask: Option<Decimal>,
}

impl HlTicker {
    pub fn prior_24h_pct(&self) -> Decimal {
        if self.prev_day_px.is_zero() {
            Decimal::ZERO
        } else {
            (self.mark_px - self.prev_day_px) / self.prev_day_px * Decimal::from(100)
        }
    }
}

pub struct HyperliquidRestClient {
    http: Client,
    base: String,
}

impl HyperliquidRestClient {
    pub fn new() -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(30))
            .tcp_nodelay(true)
            .build()
            .expect("reqwest client init");
        Self {
            http,
            base: BASE.into(),
        }
    }

    async fn post_text(&self, body: &serde_json::Value) -> Result<String> {
        let mut last_err: Option<Error> = None;
        for attempt in 0..MAX_RETRY {
            let r = self
                .http
                .post(&self.base)
                .json(body)
                .send()
                .await
                .and_then(|resp| resp.error_for_status());

            match r {
                Ok(resp) => match resp.text().await {
                    Ok(t) => return Ok(t),
                    Err(e) => last_err = Some(Error::Http(e)),
                },
                Err(e) => last_err = Some(Error::Http(e)),
            }

            if attempt < MAX_RETRY - 1 {
                tokio::time::sleep(Duration::from_millis(RETRY_SLEEP_MS)).await;
            }
        }
        Err(last_err.unwrap_or_else(|| Error::Msg("HL post_text failed".into())))
    }

    async fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        body: serde_json::Value,
    ) -> Result<T> {
        let mut last_error = None;
        for attempt in 0..MAX_RETRY {
            let text = self.post_text(&body).await?;
            match serde_json::from_str(&text) {
                Ok(value) => return Ok(value),
                Err(error) => last_error = Some(error),
            }
            if attempt + 1 < MAX_RETRY {
                tokio::time::sleep(Duration::from_millis(RETRY_SLEEP_MS)).await;
            }
        }
        Err(Error::Json(
            last_error.expect("at least one decode attempt"),
        ))
    }

    /// 从交易所发现主 DEX 和全部 HIP-3 DEX，避免新部署者被静态名单漏掉。
    pub async fn perp_dex_names(&self) -> Result<Vec<String>> {
        let dexes: Vec<Option<PerpDex>> = self.post_json(json!({"type": "perpDexs"})).await?;
        Ok(dex_names(dexes))
    }

    /// 拉取指定 dex 的所有永续合约状态
    pub async fn perp_ctxs_by_dex(&self, dex: &str) -> Result<Vec<HlTicker>> {
        let body = if dex.is_empty() {
            json!({"type": "metaAndAssetCtxs"})
        } else {
            json!({"type": "metaAndAssetCtxs", "dex": dex})
        };

        let resp: serde_json::Value = self.post_json(body).await?;

        let universe = resp
            .get(0)
            .and_then(|v| v.get("universe"))
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error::Msg(format!("bad universe for dex={}", dex)))?;
        let ctxs = resp
            .get(1)
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error::Msg(format!("missing ctxs for dex={}", dex)))?;

        let mut result = Vec::new();
        for (u, ctx) in universe.iter().zip(ctxs.iter()) {
            let Some(coin) = u.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            if u.get("isDelisted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                continue;
            }
            let Some(funding_str) = ctx.get("funding").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(mark_str) = ctx.get("markPx").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(prev_str) = ctx.get("prevDayPx").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(vol_str) = ctx.get("dayNtlVlm").and_then(|v| v.as_str()) else {
                continue;
            };
            let oi_str = ctx
                .get("openInterest")
                .and_then(|v| v.as_str())
                .unwrap_or("0");
            let impact = ctx.get("impactPxs").and_then(|v| v.as_array());
            let impact_bid = impact
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .and_then(|v| Decimal::from_str(v).ok());
            let impact_ask = impact
                .and_then(|v| v.get(1))
                .and_then(|v| v.as_str())
                .and_then(|v| Decimal::from_str(v).ok());

            result.push(HlTicker {
                coin: coin.to_string(),
                mark_px: Decimal::from_str(mark_str).unwrap_or(Decimal::ZERO),
                prev_day_px: Decimal::from_str(prev_str).unwrap_or(Decimal::ZERO),
                day_ntl_vlm: Decimal::from_str(vol_str).unwrap_or(Decimal::ZERO),
                funding: Decimal::from_str(funding_str).unwrap_or(Decimal::ZERO),
                max_leverage: u
                    .get("maxLeverage")
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u32::try_from(v).ok()),
                size_decimals: u
                    .get("szDecimals")
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u32::try_from(v).ok()),
                open_interest: Decimal::from_str(oi_str).unwrap_or(Decimal::ZERO),
                impact_bid,
                impact_ask,
            });
        }
        Ok(result)
    }

    /// 遍历所有 dex（主 + HIP-3），合并返回
    pub async fn all_perp_ctxs_all_dexes(&self) -> Result<(Vec<HlTicker>, bool)> {
        let dexes = tokio::time::timeout(Duration::from_secs(15), self.perp_dex_names())
            .await
            .map_err(|_| Error::Msg("Hyperliquid perpDexs timeout".into()))??;
        let mut all = Vec::new();
        let mut loaded = 0usize;
        let mut queries = stream::iter(dexes.iter().cloned())
            .map(|dex| async move {
                let result =
                    tokio::time::timeout(Duration::from_secs(20), self.perp_ctxs_by_dex(&dex))
                        .await;
                (dex, result)
            })
            .buffer_unordered(5);
        while let Some((dex, result)) = queries.next().await {
            match result {
                Ok(Ok(mut tickers)) => {
                    all.append(&mut tickers);
                    loaded += 1;
                }
                Ok(Err(e)) => {
                    tracing::warn!("HL dex={} failed: {}", dex, e);
                }
                Err(_) => tracing::warn!("HL dex={} timeout", dex),
            }
        }
        if all.is_empty() {
            return Err(Error::Msg("no Hyperliquid perp contexts loaded".into()));
        }
        if loaded < dexes.len() {
            tracing::warn!("HL loaded {}/{} perp DEXes", loaded, dexes.len());
        }
        tracing::info!(
            "HL DEX coverage: {}/{} DEXes, {} perps",
            loaded,
            dexes.len(),
            all.len()
        );
        Ok((all, loaded == dexes.len()))
    }

    /// 获取指定时间点的收盘价。coin 用全名，例如 "BTC" 或 "para:TREAD"
    pub async fn price_at_time(&self, coin: &str, target_ts: i64) -> Result<Option<Decimal>> {
        let bar_time = outcome_bar_open(target_ts);
        let start = bar_time;
        let end = bar_time + 60_000;

        let body = json!({
            "type": "candleSnapshot",
            "req": {
                "coin": coin,
                "interval": "1m",
                "startTime": start,
                "endTime": end
            }
        });

        let resp: Vec<serde_json::Value> = self.post_json(body).await?;

        for row in resp {
            let Some(t) = row.get("t").and_then(|v| v.as_i64()) else {
                continue;
            };
            if t == bar_time {
                let close_str = row.get("c").and_then(|v| v.as_str()).unwrap_or("");
                return Ok(Decimal::from_str(close_str).ok());
            }
        }
        Ok(None)
    }
}

fn dex_names(dexes: Vec<Option<PerpDex>>) -> Vec<String> {
    let mut names = vec![String::new()];
    for dex in dexes.into_iter().flatten() {
        if !dex.name.is_empty() && !names.contains(&dex.name) {
            names.push(dex.name);
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_new_hip3_dexes_after_main_market() {
        let response: Vec<Option<PerpDex>> =
            serde_json::from_str(r#"[null,{"name":"xyz"},{"name":"cash"}]"#).unwrap();
        assert_eq!(dex_names(response), vec!["", "xyz", "cash"]);
    }
}

impl Default for HyperliquidRestClient {
    fn default() -> Self {
        Self::new()
    }
}
