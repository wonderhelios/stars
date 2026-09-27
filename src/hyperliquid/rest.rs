use reqwest::Client;
use rust_decimal::Decimal;
use serde_json::json;
use std::str::FromStr;
use std::time::Duration;

use crate::error::{Error, Result};

const BASE: &str = "https://api.hyperliquid.xyz/info";

#[derive(Debug, Clone)]
pub struct HlTicker {
    pub coin: String,
    pub mark_px: Decimal,
    pub prev_day_px: Decimal,
    pub day_ntl_vlm: Decimal,
    pub funding: Decimal,
    pub open_interest: Decimal,
}

impl HlTicker {
    /// 前 24h 涨跌幅（%）。用 markPx vs prevDayPx 计算
    pub fn prior_24h_pct(&self) -> Decimal {
        if self.prev_day_px.is_zero() {
            Decimal::ZERO
        } else {
            (self.mark_px - self.prev_day_px) / self.prev_day_px * Decimal::from(100)
        }
    }
}

#[derive(Debug, Clone)]
pub struct HlCandle {
    pub open_time: i64,
    pub close: Decimal,
}

pub struct HyperliquidRestClient {
    http: Client,
    base: String,
}

impl HyperliquidRestClient {
    pub fn new() -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client init"),
            base: BASE.into(),
        }
    }

    /// Hyperliquid 所有请求都是 POST，body 里带 type 字段
    async fn post<T: serde::de::DeserializeOwned>(&self, body: serde_json::Value) -> Result<T> {
        let resp = self
            .http
            .post(&self.base)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json::<T>()
            .await?;
        Ok(resp)
    }

    /// 获取所有永续合约的实时状态（含资金费率、标记价、持仓量）
    /// 接口类型：metaAndAssetCtxs
    pub async fn all_perp_ctxs(&self) -> Result<Vec<HlTicker>> {
        let body = json!({"type": "metaAndAssetCtxs"});
        let resp: serde_json::Value = self.post(body).await?;

        // 返回格式：[universe数组, assetCtxs数组]
        let universe = resp
            .get(0)
            .and_then(|v| v.get("universe"))
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error::Msg("bad metaAndAssetCtxs response".into()))?;
        let ctxs = resp
            .get(1)
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error::Msg("missing assetCtxs".into()))?;

        let mut result = Vec::new();
        for (u, ctx) in universe.iter().zip(ctxs.iter()) {
            let Some(coin) = u.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            // 跳过已下架的
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
            let Some(oi_str) = ctx.get("openInterest").and_then(|v| v.as_str()) else {
                continue;
            };

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

    /// 按 24h 成交额排序取 Top N
    pub async fn top_by_volume(&self, top_n: usize) -> Result<Vec<String>> {
        let mut list = self.all_perp_ctxs().await?;
        list.sort_by(|a, b| b.day_ntl_vlm.cmp(&a.day_ntl_vlm));
        list.truncate(top_n);
        Ok(list.into_iter().map(|r| r.coin).collect())
    }

    /// 获取指定时间点的收盘价（精确到 1H K线）
    /// Hyperliquid 的 K线时间戳是 1H 整点，直接用 target_ts 向下取整
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

        let resp: Vec<serde_json::Value> = self.post(body).await?;

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

    /// 获取历史资金费率（1H 粒度）
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

        let resp: Vec<serde_json::Value> = self.post(body).await?;
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
