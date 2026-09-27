use reqwest::Client;
use rust_decimal::Decimal;
use std::str::FromStr;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::signal::outcome_bar_open;

const BASE: &str = "https://www.okx.com";
const FUNDING_BATCH: u32 = 400;
const BATCH_SLEEP_MS: u64 = 120;
const MAX_RETRY: u32 = 3;
const RETRY_SLEEP_MS: u64 = 400;

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
    #[serde(default)]
    bid_px: String,
    #[serde(default)]
    ask_px: String,
    #[serde(default)]
    high24h: String,
    #[serde(default)]
    low24h: String,
    #[serde(default)]
    ts: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawFundingNow {
    #[allow(dead_code)]
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
    pub close: Decimal,
}

#[derive(Debug, Clone)]
pub struct TickerRow {
    pub inst_id: String,
    pub last: Decimal,
    pub open_24h: Decimal,
    pub vol_ccy_24h: Decimal,
}

#[derive(Debug, Clone)]
pub struct LiveTickerRow {
    pub inst_id: String,
    pub last: Decimal,
    pub bid: Decimal,
    pub ask: Decimal,
    pub open_24h: Decimal,
    pub high_24h: Decimal,
    pub low_24h: Decimal,
    pub volume_quote_24h: Decimal,
    pub ts: i64,
}

impl RawTicker {
    fn into_live(self) -> Option<LiveTickerRow> {
        let last = Decimal::from_str(&self.last).ok()?;
        let open_24h = Decimal::from_str(&self.open24h).ok()?;
        let volume_base = Decimal::from_str(&self.vol_ccy_24h).ok()?;
        let ts = self.ts.parse::<i64>().ok()?;
        if last <= Decimal::ZERO || ts <= 0 {
            return None;
        }
        Some(LiveTickerRow {
            inst_id: self.inst_id,
            last,
            bid: Decimal::from_str(&self.bid_px).unwrap_or(last),
            ask: Decimal::from_str(&self.ask_px).unwrap_or(last),
            open_24h,
            high_24h: Decimal::from_str(&self.high24h).unwrap_or(Decimal::ZERO),
            low_24h: Decimal::from_str(&self.low24h).unwrap_or(Decimal::ZERO),
            volume_quote_24h: volume_base * last,
            ts,
        })
    }
}

impl TickerRow {
    pub fn volume_quote_24h(&self) -> Decimal {
        self.vol_ccy_24h * self.last
    }

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
            .timeout(Duration::from_secs(30))
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
        Err(last_err.unwrap_or_else(|| Error::Msg("OKX get_text failed".into())))
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

    pub async fn all_swap_inst_ids(&self) -> Result<Vec<String>> {
        let resp: Resp<RawInstrument> = self
            .get_json("/api/v5/public/instruments", &[("instType", "SWAP")])
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
            .get_json("/api/v5/market/tickers", &[("instType", "SWAP")])
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

    /// WebSocket 停止推送时，一次 REST 请求回补全部合约的最新价格。
    pub async fn all_live_tickers_usdt_swap(&self) -> Result<Vec<LiveTickerRow>> {
        let resp: Resp<RawTicker> = self
            .get_json("/api/v5/market/tickers", &[("instType", "SWAP")])
            .await?;

        Ok(resp
            .unwrap_ok()?
            .into_iter()
            .filter(|r| r.inst_id.ends_with("-USDT-SWAP"))
            .filter_map(RawTicker::into_live)
            .collect())
    }

    pub async fn top_swap_by_volume(&self, top_n: usize) -> Result<Vec<String>> {
        let mut list = self.all_tickers_usdt_swap().await?;
        list.sort_by_key(|b| std::cmp::Reverse(b.volume_quote_24h()));
        list.truncate(top_n);
        Ok(list.into_iter().map(|r| r.inst_id).collect())
    }

    pub async fn funding_rate(&self, inst_id: &str) -> Result<Decimal> {
        let resp: Resp<RawFundingNow> = self
            .get_json("/api/v5/public/funding-rate", &[("instId", inst_id)])
            .await?;

        let rows = resp.unwrap_ok()?;
        rows.into_iter()
            .next()
            .map(|r| r.funding_rate)
            .ok_or_else(|| Error::Msg(format!("no funding rate for {}", inst_id)))
    }

    pub async fn ticker(&self, inst_id: &str) -> Result<TickerRow> {
        let resp: Resp<RawTicker> = self
            .get_json("/api/v5/market/ticker", &[("instId", inst_id)])
            .await?;
        let row = resp
            .unwrap_ok()?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Msg(format!("no ticker for {}", inst_id)))?;
        Ok(TickerRow {
            inst_id: row.inst_id,
            last: Decimal::from_str(&row.last).map_err(|e| Error::Msg(e.to_string()))?,
            open_24h: Decimal::from_str(&row.open24h).map_err(|e| Error::Msg(e.to_string()))?,
            vol_ccy_24h: Decimal::from_str(&row.vol_ccy_24h)
                .map_err(|e| Error::Msg(e.to_string()))?,
        })
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

            let resp: Resp<Vec<String>> = self
                .get_json("/api/v5/market/history-candles", &params)
                .await?;
            let rows = resp.unwrap_ok()?;

            if rows.is_empty() {
                break;
            }

            let mut batch: Vec<Candle1H> = rows
                .into_iter()
                .filter_map(|row| {
                    if row.len() < 9 || row[8] != "1" {
                        return None;
                    }
                    Some(Candle1H {
                        open_time: row[0].parse().ok()?,
                        open: Decimal::from_str(&row[1]).ok()?,
                        close: Decimal::from_str(&row[4]).ok()?,
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
                .get_json("/api/v5/public/funding-rate-history", &params)
                .await?;
            let rows = resp.unwrap_ok()?;

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

    pub async fn price_at_time(&self, inst_id: &str, target_ts: i64) -> Result<Option<Decimal>> {
        let bar_time = outcome_bar_open(target_ts);
        let params = [
            ("instId", inst_id),
            ("bar", "1m"),
            ("after", &(bar_time + 60_000).to_string()),
            ("limit", "2"),
        ];

        let resp: Resp<Vec<String>> = self
            .get_json("/api/v5/market/history-candles", &params)
            .await?;

        let rows = resp.unwrap_ok()?;
        for row in rows {
            if row.len() >= 9 && row[0].parse::<i64>().ok() == Some(bar_time) && row[8] == "1" {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_volume_is_compared_in_quote_currency() {
        let ticker = TickerRow {
            inst_id: "BTC-USDT-SWAP".to_string(),
            last: Decimal::from(50_000),
            open_24h: Decimal::from(49_000),
            vol_ccy_24h: Decimal::from(20),
        };
        assert_eq!(ticker.volume_quote_24h(), Decimal::from(1_000_000));
    }

    #[test]
    fn bulk_rest_ticker_can_refresh_dashboard_snapshot() {
        let raw: RawTicker = serde_json::from_str(
            r#"{"instId":"BTC-USDT-SWAP","last":"50000","open24h":"49000","volCcy24h":"20","bidPx":"49999","askPx":"50001","high24h":"51000","low24h":"48000","ts":"1790527000000"}"#,
        )
        .unwrap();
        let row = raw.into_live().unwrap();
        assert_eq!(row.bid, Decimal::from(49_999));
        assert_eq!(row.ask, Decimal::from(50_001));
        assert_eq!(row.volume_quote_24h, Decimal::from(1_000_000));
        assert_eq!(row.ts, 1_790_527_000_000);
    }
}
