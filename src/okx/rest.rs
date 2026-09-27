use reqwest::Client;
use rust_decimal::Decimal;
use std::str::FromStr;
use std::time::Duration;

use crate::error::{Error, Result};

const BASE: &str = "https://www.okx.com";
const CANDLE_BATCH: u32 = 300;
const FUNDING_BATCH: u32 = 400;
const BATCH_SLEEP_MS: u64 = 120;

#[derive(serde::Deserialize)]
struct Resp<T> {
    code: String,
    msg: String,
    data: Vec<T>,
}

impl<T> Resp<T> {
    fn unwrap_ok(self) -> Result<Vec<T>> {
        if self.code != "0" {
            return Err(Error::Okx {
                code: self.code,
                msg: self.msg,
            });
        }
        Ok(self.data)
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawInstrument {
    inst_id: String,
    state: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTicker {
    inst_id: String,
    last: String,
    open24h: String,
    vol_ccy_24h: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawFundingNow {
    inst_id: String,
    funding_rate: Decimal,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FundingHistoryItem {
    pub funding_rate: Decimal,
    pub funding_time: String,
    #[serde(default)]
    pub realized_rate: Option<Decimal>,
}

#[derive(Debug, Clone)]
pub struct Candle1H {
    pub open_time: i64,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: Decimal,
}

#[derive(Debug, Clone)]
pub struct TickerRow {
    pub inst_id: String,
    pub last: Decimal,
    pub open_24h: Decimal,
    pub vol_ccy_24h: Decimal,
}

impl TickerRow {
    pub fn prior_24h_pct(&self) -> Decimal {
        if self.open_24h.is_zero() {
            Decimal::ZERO
        } else {
            (self.last - self.open_24h) / self.open_24h * Decimal::from(100)
        }
    }
}

pub struct RestClient {
    http: Client,
    base: String,
}

impl RestClient {
    pub fn new() -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(2)
            .pool_idle_timeout(Duration::from_secs(30))
            .tcp_nodelay(true)
            .build()
            .expect("reqwest client init");
        Self {
            http,
            base: BASE.into(),
        }
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T> {
        let resp = self
            .http
            .get(format!("{}{}", self.base, path))
            .query(params)
            .send()
            .await?
            .error_for_status()?
            .json::<T>()
            .await?;
        Ok(resp)
    }

    pub async fn all_swap_inst_ids(&self) -> Result<Vec<String>> {
        let resp: Resp<RawInstrument> = self
            .get("/api/v5/public/instruments", &[("instType", "SWAP")])
            .await?;

        Ok(resp
            .unwrap_ok()?
            .into_iter()
            .filter(|r| r.state == "live")
            .filter(|r| r.inst_id.ends_with("-USDT-SWAP"))
            .map(|r| r.inst_id)
            .collect())
    }

    pub async fn all_tickers_usdt_swap(&self) -> Result<Vec<TickerRow>> {
        let resp: Resp<RawTicker> = self
            .get("/api/v5/market/tickers", &[("instType", "SWAP")])
            .await?;

        Ok(resp
            .unwrap_ok()?
            .into_iter()
            .filter(|r| r.inst_id.ends_with("-USDT-SWAP"))
            .filter_map(|r| {
                Some(TickerRow {
                    inst_id: r.inst_id,
                    last: Decimal::from_str(&r.last).ok()?,
                    open_24h: Decimal::from_str(&r.open24h).ok()?,
                    vol_ccy_24h: Decimal::from_str(&r.vol_ccy_24h).ok()?,
                })
            })
            .collect())
    }

    pub async fn top_swap_by_volume(&self, top_n: usize) -> Result<Vec<String>> {
        let mut list = self.all_tickers_usdt_swap().await?;
        list.sort_by(|a, b| b.vol_ccy_24h.cmp(&a.vol_ccy_24h));
        list.truncate(top_n);
        Ok(list.into_iter().map(|r| r.inst_id).collect())
    }

    pub async fn funding_rate(&self, inst_id: &str) -> Result<Decimal> {
        let resp: Resp<RawFundingNow> = self
            .get("/api/v5/public/funding-rate", &[("instId", inst_id)])
            .await?;

        let rows = resp.unwrap_ok()?;
        rows.into_iter()
            .next()
            .map(|r| r.funding_rate)
            .ok_or_else(|| Error::Msg(format!("no funding rate for {}", inst_id)))
    }

    pub async fn candles_1h_history(
        &self,
        inst_id: &str,
        target_days: u32,
    ) -> Result<Vec<Candle1H>> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let target_start_ms = now_ms - (target_days as i64) * 24 * 3_600_000;

        let mut all: Vec<Candle1H> = Vec::new();
        let mut after: Option<i64> = None;
        let mut batch_count = 0u32;
        let max_batches = ((target_days as f64 / 12.5).ceil() as u32 + 1) * 2;

        loop {
            if batch_count >= max_batches {
                break;
            }

            let after_str = after.map(|v| v.to_string());
            let mut params: Vec<(&str, &str)> =
                vec![("instId", inst_id), ("bar", "1H"), ("limit", "300")];
            if let Some(ref a) = after_str {
                params.push(("after", a));
            }

            let resp: Resp<Vec<String>> =
                self.get("/api/v5/market/history-candles", &params).await?;

            let rows = match resp.unwrap_ok() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("  [{}] candles batch {}: {}", inst_id, batch_count, e);
                    break;
                }
            };

            if rows.is_empty() {
                break;
            }

            let mut batch: Vec<Candle1H> = rows
                .into_iter()
                .filter_map(|row| {
                    if row.len() < 6 {
                        return None;
                    }
                    Some(Candle1H {
                        open_time: row[0].parse().ok()?,
                        open: Decimal::from_str(&row[1]).ok()?,
                        high: Decimal::from_str(&row[2]).ok()?,
                        low: Decimal::from_str(&row[3]).ok()?,
                        close: Decimal::from_str(&row[4]).ok()?,
                        volume: Decimal::from_str(&row[5]).ok()?,
                    })
                })
                .collect();

            if batch.is_empty() {
                break;
            }

            let batch_earliest = batch.iter().map(|c| c.open_time).min().unwrap();
            all.append(&mut batch);
            batch_count += 1;

            if batch_earliest <= target_start_ms {
                break;
            }
            if let Some(prev_after) = after {
                if batch_earliest >= prev_after {
                    break;
                }
            }
            after = Some(batch_earliest);
            tokio::time::sleep(Duration::from_millis(BATCH_SLEEP_MS)).await;
        }

        all.sort_by_key(|c| c.open_time);
        all.dedup_by_key(|c| c.open_time);
        Ok(all)
    }

    pub async fn funding_rate_history_paged(
        &self,
        inst_id: &str,
    ) -> Result<Vec<FundingHistoryItem>> {
        let mut all: Vec<FundingHistoryItem> = Vec::new();
        let mut after: Option<i64> = None;
        let mut batch_count = 0u32;
        let max_batches = 5;

        loop {
            if batch_count >= max_batches {
                break;
            }

            let after_str = after.map(|v| v.to_string());
            let mut params: Vec<(&str, &str)> = vec![("instId", inst_id), ("limit", "400")];
            if let Some(ref a) = after_str {
                params.push(("after", a));
            }

            let resp: Resp<FundingHistoryItem> = self
                .get("/api/v5/public/funding-rate-history", &params)
                .await?;

            let rows = match resp.unwrap_ok() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("  [{}] funding batch {}: {}", inst_id, batch_count, e);
                    break;
                }
            };

            if rows.is_empty() {
                break;
            }

            let batch_len = rows.len();
            let batch_earliest = rows
                .iter()
                .filter_map(|r| r.funding_time.parse::<i64>().ok())
                .min();

            all.extend(rows);
            batch_count += 1;

            if batch_len < FUNDING_BATCH as usize {
                break;
            }

            match batch_earliest {
                Some(ts) => {
                    if let Some(prev) = after {
                        if ts >= prev {
                            break;
                        }
                    }
                    after = Some(ts);
                }
                None => break,
            }

            tokio::time::sleep(Duration::from_millis(BATCH_SLEEP_MS)).await;
        }

        all.sort_by_key(|r| r.funding_time.parse::<i64>().unwrap_or(0));
        all.dedup_by_key(|r| r.funding_time.clone());
        Ok(all)
    }

    /// 获取指定时间点的收盘价（精确到 1H K线）
    pub async fn price_at_time(&self, inst_id: &str, target_ts: i64) -> Result<Option<Decimal>> {
        let bar_time = (target_ts / 3_600_000) * 3_600_000;
        let params = [
            ("instId", inst_id),
            ("bar", "1H"),
            ("after", &(bar_time - 1).to_string()),
            ("limit", "1"),
        ];

        let resp: Resp<Vec<String>> = self.get("/api/v5/market/history-candles", &params).await?;

        let rows = resp.unwrap_ok()?;
        if let Some(row) = rows.first() {
            if row.len() >= 5 {
                return Ok(Decimal::from_str(&row[4]).ok());
            }
        }
        Ok(None)
    }
}

impl Default for RestClient {
    fn default() -> Self {
        Self::new()
    }
}

pub fn parse_dec(v: &serde_json::Value, key: &str) -> Option<Decimal> {
    let s = v.get(key)?.as_str()?;
    if s.is_empty() {
        return None;
    }
    Decimal::from_str(s).ok()
}
