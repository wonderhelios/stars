use reqwest::Client;
use rust_decimal::Decimal;
use std::str::FromStr;
use std::time::Duration;

use crate::error::{Error, Result};

const BASE: &str = "https://fapi.binance.com";
const BATCH_SLEEP_MS: u64 = 120;
const KLINE_BATCH: u32 = 1500;

#[derive(serde::Deserialize)]
struct FundingHistoryItem {
    #[serde(rename = "fundingTime")]
    funding_time: i64,
    #[serde(rename = "fundingRate")]
    funding_rate: Decimal,
}

#[derive(serde::Deserialize)]
struct ExchangeInfo {
    symbols: Vec<SymbolInfo>,
}

#[derive(serde::Deserialize)]
struct SymbolInfo {
    symbol: String,
    #[serde(rename = "contractType")]
    contract_type: String,
    status: String,
}

#[derive(serde::Deserialize)]
struct Ticker24h {
    symbol: String,
    #[serde(rename = "lastPrice")]
    last_price: String,
    #[serde(rename = "openPrice")]
    open_price: String,
    #[serde(rename = "quoteVolume")]
    quote_volume: String,
}

#[derive(serde::Deserialize)]
struct PremiumIndex {
    #[allow(dead_code)]
    symbol: String,
    #[serde(rename = "lastFundingRate")]
    last_funding_rate: Decimal,
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

#[derive(Debug, Clone)]
pub struct BinanceCandle {
    pub open_time: i64,
    #[allow(dead_code)]
    pub close: Decimal,
}

pub struct BinanceRestClient {
    http: Client,
    base: String,
}

impl BinanceRestClient {
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

    pub async fn all_perp_symbols(&self) -> Result<Vec<String>> {
        let resp: ExchangeInfo = self.get("/fapi/v1/exchangeInfo", &[]).await?;
        Ok(resp
            .symbols
            .into_iter()
            .filter(|s| s.contract_type == "PERPETUAL" && s.status == "TRADING")
            .map(|s| s.symbol)
            .collect())
    }

    pub async fn all_tickers(&self) -> Result<Vec<BinanceTicker>> {
        let resp: Vec<Ticker24h> = self.get("/fapi/v1/ticker/24hr", &[]).await?;
        Ok(resp
            .into_iter()
            .filter_map(|r| {
                Some(BinanceTicker {
                    symbol: r.symbol,
                    last: Decimal::from_str(&r.last_price).ok()?,
                    open_24h: Decimal::from_str(&r.open_price).ok()?,
                    vol_quote: Decimal::from_str(&r.quote_volume).ok()?,
                })
            })
            .collect())
    }

    pub async fn top_by_volume(&self, top_n: usize) -> Result<Vec<String>> {
        let mut list = self.all_tickers().await?;
        list.sort_by(|a, b| b.vol_quote.cmp(&a.vol_quote));
        list.truncate(top_n);
        Ok(list.into_iter().map(|r| r.symbol).collect())
    }

    pub async fn funding_rate(&self, symbol: &str) -> Result<Decimal> {
        let resp: PremiumIndex = self
            .get("/fapi/v1/premiumIndex", &[("symbol", symbol)])
            .await?;
        Ok(resp.last_funding_rate)
    }

    #[allow(dead_code)]
    pub async fn funding_rate_history(
        &self,
        symbol: &str,
        limit: u32,
    ) -> Result<Vec<FundingHistoryItem>> {
        let limit = limit.min(1000).to_string();
        let resp: Vec<FundingHistoryItem> = self
            .get(
                "/fapi/v1/fundingRate",
                &[("symbol", symbol), ("limit", &limit)],
            )
            .await?;
        Ok(resp)
    }

    #[allow(dead_code)]
    pub async fn candles_1h(&self, symbol: &str, limit: u32) -> Result<Vec<BinanceCandle>> {
        let limit = limit.min(KLINE_BATCH).to_string();
        let resp: Vec<Vec<serde_json::Value>> = self
            .get(
                "/fapi/v1/klines",
                &[("symbol", symbol), ("interval", "1h"), ("limit", &limit)],
            )
            .await?;

        let mut candles: Vec<BinanceCandle> = resp
            .into_iter()
            .filter_map(|row| {
                if row.len() < 5 {
                    return None;
                }
                Some(BinanceCandle {
                    open_time: row[0].as_i64()?,
                    close: Decimal::from_str(row[4].as_str()?).ok()?,
                })
            })
            .collect();

        candles.sort_by_key(|c| c.open_time);
        Ok(candles)
    }

    /// 获取指定时间点的收盘价（精确到 1H K线，向下取整到整点）
    pub async fn price_at_time(&self, symbol: &str, target_ts: i64) -> Result<Option<Decimal>> {
        let bar_time = (target_ts / 3_600_000) * 3_600_000;
        let start = bar_time - 3_600_000;
        let end = bar_time + 3_600_000;

        let resp: Vec<Vec<serde_json::Value>> = self
            .get(
                "/fapi/v1/klines",
                &[
                    ("symbol", symbol),
                    ("interval", "1h"),
                    ("startTime", &start.to_string()),
                    ("endTime", &end.to_string()),
                    ("limit", "3"),
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

const _: () = {
    // 抑制 unused 警告
    let _ = BATCH_SLEEP_MS;
};
