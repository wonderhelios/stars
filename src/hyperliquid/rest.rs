use reqwest::Client;
use rust_decimal::Decimal;
use serde_json::json;
use std::str::FromStr;
use std::time::Duration;

use crate::error::{Error, Result};

const BASE: &str = "https://api.hyperliquid.xyz/info";
const MAX_RETRY: u32 = 3;
const RETRY_SLEEP_MS: u64 = 400;

/// HIP-3 的第三方部署者命名空间。空字符串 = 主 DEX
pub const HIP3_DEXES: &[&str] = &["", "para", "xyz", "mkts", "io"];

#[derive(Debug, Clone)]
pub struct HlTicker {
    pub coin: String,
    pub mark_px: Decimal,
    pub prev_day_px: Decimal,
    pub day_ntl_vlm: Decimal,
    pub funding: Decimal,
    #[allow(dead_code)]
    pub open_interest: Decimal,
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
        let text = self.post_text(&body).await?;
        serde_json::from_str(&text).map_err(|e| Error::Json(e))
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

            result.push(HlTicker {
                coin: coin.to_string(),
                mark_px: Decimal::from_str(mark_str).unwrap_or(Decimal::ZERO),
                prev_day_px: Decimal::from_str(prev_str).unwrap_or(Decimal::ZERO),
                day_ntl_vlm: Decimal::from_str(vol_str).unwrap_or(Decimal::ZERO),
                funding: Decimal::from_str(funding_str).unwrap_or(Decimal::ZERO),
                open_interest: Decimal::from_str(oi_str).unwrap_or(Decimal::ZERO),
            });
        }
        Ok(result)
    }

    /// 遍历所有 dex（主 + HIP-3），合并返回
    pub async fn all_perp_ctxs_all_dexes(&self) -> Result<Vec<HlTicker>> {
        let mut all = Vec::new();
        for dex in HIP3_DEXES {
            match self.perp_ctxs_by_dex(dex).await {
                Ok(mut tickers) => all.append(&mut tickers),
                Err(e) => {
                    tracing::warn!("HL dex={} failed: {}", dex, e);
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(all)
    }

    /// 获取指定时间点的收盘价。coin 用全名，例如 "BTC" 或 "para:TREAD"
    pub async fn price_at_time(&self, coin: &str, target_ts: i64) -> Result<Option<Decimal>> {
        let bar_time = (target_ts / 3_600_000) * 3_600_000;
        let start = bar_time - 3_600_000;
        let end = bar_time + 3_600_000;

        let body = json!({
            "type": "candleSnapshot",
            "req": {
                "coin": coin,
                "interval": "1h",
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

    #[allow(dead_code)]
    pub async fn funding_history(
        &self,
        coin: &str,
        start_ts: i64,
        end_ts: i64,
    ) -> Result<Vec<(i64, Decimal)>> {
        let body = json!({
            "type": "fundingHistory",
            "coin": coin,
            "startTime": start_ts,
            "endTime": end_ts
        });

        let resp: Vec<serde_json::Value> = self.post_json(body).await?;
        let mut result = Vec::new();
        for row in resp {
            let Some(t) = row.get("time").and_then(|v| v.as_i64()) else {
                continue;
            };
            let Some(rate_str) = row.get("fundingRate").and_then(|v| v.as_str()) else {
                continue;
            };
            if let Ok(rate) = Decimal::from_str(rate_str) {
                result.push((t, rate));
            }
        }
        result.sort_by_key(|(t, _)| *t);
        Ok(result)
    }
}

impl Default for HyperliquidRestClient {
    fn default() -> Self {
        Self::new()
    }
}
