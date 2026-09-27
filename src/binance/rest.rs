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
    pub close: Decimal,
}

pub struct BinanceRestClient {
    http: Client,
    base: String,
}

impl BinanceRestClient {
    pub fn new() -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client init"),
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

    /// 获取全部 live 状态的 U 本位永续合约 symbol（如 BTCUSDT）
    pub async fn all_perp_symbols(&self) -> Result<Vec<String>> {
        let resp: ExchangeInfo = self.get("/fapi/v1/exchangeInfo", &[]).await?;
        Ok(resp
            .symbols
            .into_iter()
            .filter(|s| s.contract_type == "PERPETUAL" && s.status == "TRADING")
            .map(|s| s.symbol)
            .collect())
    }

    /// 全市场 24h ticker
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

    /// 按成交额排序取 Top N
    pub async fn top_by_volume(&self, top_n: usize) -> Result<Vec<String>> {
        let mut list = self.all_tickers().await?;
        list.sort_by(|a, b| b.vol_quote.cmp(&a.vol_quote));
        list.truncate(top_n);
        Ok(list.into_iter().map(|r| r.symbol).collect())
    }

    /// 单个 symbol 当前资金费率
    pub async fn funding_rate(&self, symbol: &str) -> Result<Decimal> {
        let resp: PremiumIndex = self
            .get("/fapi/v1/premiumIndex", &[("symbol", symbol)])
            .await?;
        Ok(resp.last_funding_rate)
    }

    /// 历史资金费率（最多 1000 条）
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

    /// 拉取 1H K线历史（最多 1500 根，覆盖约 62 天）
    pub async fn candles_1h(&self, symbol: &str, limit: u32) -> Result<Vec<BinanceCandle>> {
        let limit = limit.min(KLINE_BATCH).to_string();
        // Binance klines 返回格式: [open_time, open, high, low, close, volume, ...]
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

    /// 获取指定时间点的收盘价（精确到 1H K线）
    /// 获取指定时间点的收盘价（精确到 1H K线）
    pub async fn price_at_time(&self, symbol: &str, target_ts: i64) -> Result<Option<Decimal>> {
        // 【关键修复】将目标时间戳向下取整到 1H 整点
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
                // 【关键修复】和整点比较，而不是毫秒时间戳
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
