use reqwest::Client;
use rust_decimal::Decimal;
use std::str::FromStr;
use std::time::Duration;

use crate::error::{Error, Result};

const BASE: &str = "https://fapi.binance.com";

#[derive(serde::Deserialize)]
struct FundingHistoryItem {
    #[serde(rename = "fundingTime")]
    #[allow(dead_code)]
    funding_time: i64,
    #[serde(rename = "fundingRate")]
    #[allow(dead_code)]
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
#[allow(dead_code)]
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
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(15))
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

    /// 获取原始文本响应，便于手动解析容忍个别字段异常
    async fn get_text(&self, path: &str, params: &[(&str, &str)]) -> Result<String> {
        let resp = self
            .http
            .get(format!("{}{}", self.base, path))
            .query(params)
            .send()
            .await?
            .error_for_status()?
            .text()
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

    /// 全市场 24h ticker（用 Value 手动解析，容忍个别字段缺失）
    pub async fn all_tickers(&self) -> Result<Vec<BinanceTicker>> {
        let text = self.get_text("/fapi/v1/ticker/24hr", &[]).await?;

        let arr: Vec<serde_json::Value> = serde_json::from_str(&text)
            .map_err(|e| Error::Msg(format!("BN ticker json: {} (len={})", e, text.len())))?;

        let mut result = Vec::with_capacity(arr.len());
        for item in &arr {
            let Some(symbol) = item.get("symbol").and_then(|v| v.as_str()) else {
                continue;
            };
            let last_str = item.get("lastPrice").and_then(|v| v.as_str()).unwrap_or("");
            let open_str = item.get("openPrice").and_then(|v| v.as_str()).unwrap_or("");
            let vol_str = item
                .get("quoteVolume")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let last = Decimal::from_str(last_str).unwrap_or(Decimal::ZERO);
            let open_24h = Decimal::from_str(open_str).unwrap_or(Decimal::ZERO);
            let vol_quote = Decimal::from_str(vol_str).unwrap_or(Decimal::ZERO);

            // 跳过明显无效的
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

    #[allow(dead_code)]
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
        let limit = limit.min(1500).to_string();
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
