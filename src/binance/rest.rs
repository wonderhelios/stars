use reqwest::Client;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::signal::outcome_bar_open;

const BASE: &str = "https://fapi.binance.com";
const MAX_RETRY: u32 = 3;
const RETRY_SLEEP_MS: u64 = 400;

#[derive(serde::Deserialize)]
struct PremiumIndex {
    #[allow(dead_code)]
    symbol: String,
    #[serde(rename = "lastFundingRate")]
    last_funding_rate: Decimal,
    #[serde(rename = "nextFundingTime")]
    next_funding_time: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct FundingNow {
    pub rate: Decimal,
    pub next_funding_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct BinanceTicker {
    pub symbol: String,
    pub last: Decimal,
    pub open_24h: Decimal,
    pub vol_quote: Decimal,
}

impl BinanceTicker {
    pub fn prior_24h_pct(&self) -> Decimal {
        if self.open_24h.is_zero() {
            Decimal::ZERO
        } else {
            (self.last - self.open_24h) / self.open_24h * Decimal::from(100)
        }
    }
}

pub struct BinanceRestClient {
    http: Client,
    base: String,
}

impl BinanceRestClient {
    pub fn new() -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            // 恢复一个健康的连接池大小，不再激进限制
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

    /// GET 文本，带 3 次自动重试（处理连接被服务端提前关闭的情况）
    async fn get_text(&self, path: &str, params: &[(&str, &str)]) -> Result<String> {
        let url = format!("{}{}", self.base, path);
        let mut last_err: Option<Error> = None;

        for attempt in 0..MAX_RETRY {
            let r = self
                .http
                .get(&url)
                .query(params)
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
        Err(last_err.unwrap_or_else(|| Error::Msg("BN get_text failed".into())))
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T> {
        let mut last_error = None;
        for attempt in 0..MAX_RETRY {
            let text = self.get_text(path, params).await?;
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

    /// 全市场 24h ticker：手动解析，容忍个别字段异常
    pub async fn all_tickers(&self) -> Result<Vec<BinanceTicker>> {
        let arr: Vec<serde_json::Value> = self.get_json("/fapi/v1/ticker/24hr", &[]).await?;

        let mut result = Vec::with_capacity(arr.len());
        for item in &arr {
            let Some(symbol) = item.get("symbol").and_then(|v| v.as_str()) else {
                continue;
            };
            if !symbol.ends_with("USDT") {
                continue;
            }
            let last_str = item.get("lastPrice").and_then(|v| v.as_str()).unwrap_or("");
            let open_str = item.get("openPrice").and_then(|v| v.as_str()).unwrap_or("");
            let vol_str = item
                .get("quoteVolume")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let last = Decimal::from_str(last_str).unwrap_or(Decimal::ZERO);
            let open_24h = Decimal::from_str(open_str).unwrap_or(Decimal::ZERO);
            let vol_quote = Decimal::from_str(vol_str).unwrap_or(Decimal::ZERO);

            if last.is_zero() || open_24h.is_zero() {
                continue;
            }

            result.push(BinanceTicker {
                symbol: symbol.to_string(),
                last,
                open_24h,
                vol_quote,
            });
        }
        Ok(result)
    }

    pub async fn funding_now(&self, symbol: &str) -> Result<FundingNow> {
        let resp: PremiumIndex = self
            .get_json("/fapi/v1/premiumIndex", &[("symbol", symbol)])
            .await?;
        Ok(FundingNow {
            rate: resp.last_funding_rate,
            next_funding_at: resp.next_funding_time,
        })
    }

    /// Symbols absent from fundingInfo use the exchange's normal eight-hour interval.
    pub async fn funding_intervals(&self) -> Result<HashMap<String, i64>> {
        let rows: Vec<serde_json::Value> = self.get_json("/fapi/v1/fundingInfo", &[]).await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                Some((
                    row.get("symbol")?.as_str()?.to_string(),
                    row.get("fundingIntervalHours")?.as_i64()?,
                ))
            })
            .collect())
    }

    pub async fn best_quotes(&self) -> Result<HashMap<String, (Decimal, Decimal)>> {
        let rows: Vec<serde_json::Value> = self.get_json("/fapi/v1/ticker/bookTicker", &[]).await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let symbol = row.get("symbol")?.as_str()?.to_string();
                let bid = Decimal::from_str(row.get("bidPrice")?.as_str()?).ok()?;
                let ask = Decimal::from_str(row.get("askPrice")?.as_str()?).ok()?;
                (bid > Decimal::ZERO && ask >= bid).then_some((symbol, (bid, ask)))
            })
            .collect())
    }

    pub async fn ticker(&self, symbol: &str) -> Result<BinanceTicker> {
        let row: serde_json::Value = self
            .get_json("/fapi/v1/ticker/24hr", &[("symbol", symbol)])
            .await?;
        let parse = |key: &str| -> Result<Decimal> {
            let raw = row.get(key).and_then(|v| v.as_str()).unwrap_or("");
            Decimal::from_str(raw).map_err(|e| Error::Msg(format!("{} {}: {}", symbol, key, e)))
        };
        Ok(BinanceTicker {
            symbol: symbol.to_string(),
            last: parse("lastPrice")?,
            open_24h: parse("openPrice")?,
            vol_quote: parse("quoteVolume")?,
        })
    }

    /// 获取指定时间点的收盘价
    pub async fn price_at_time(&self, symbol: &str, target_ts: i64) -> Result<Option<Decimal>> {
        let bar_time = outcome_bar_open(target_ts);
        let start = bar_time;
        let end = bar_time + 60_000;

        let resp: Vec<Vec<serde_json::Value>> = self
            .get_json(
                "/fapi/v1/klines",
                &[
                    ("symbol", symbol),
                    ("interval", "1m"),
                    ("startTime", &start.to_string()),
                    ("endTime", &end.to_string()),
                    ("limit", "1"),
                ],
            )
            .await?;

        for row in resp {
            if row.len() < 5 {
                continue;
            }
            if let Some(ts) = row[0].as_i64() {
                if ts == bar_time {
                    return Ok(Decimal::from_str(row[4].as_str().unwrap_or("")).ok());
                }
            }
        }
        Ok(None)
    }
}

impl Default for BinanceRestClient {
    fn default() -> Self {
        Self::new()
    }
}
