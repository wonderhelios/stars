use reqwest::Client;
use rust_decimal::Decimal;
use std::str::FromStr;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::types::{Candle, FundingRate, Instrument, Interval, Symbol, Ticker};

const BASE: &str = "https://www.okx.com";

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
    tick_sz: Decimal,
    lot_sz: Decimal,
    min_sz: Decimal,
    ct_val: Option<Decimal>,
    lever: Option<Decimal>,
    state: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTicker {
    inst_id: String,
    last: Decimal,
    bid_px: Decimal,
    ask_px: Decimal,
    open24h: Decimal,
    high24h: Decimal,
    low24h: Decimal,
    vol24h: Decimal,
    vol_ccy_24h: Decimal,
    ts: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawFunding {
    inst_id: String,
    funding_rate: Decimal,
    next_funding_time: Option<String>,
    ts: String,
}

pub struct RestClient {
    http: Client,
    base: String,
}

impl RestClient {
    pub fn new() -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(15))
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

    /// 全部永续合约的元数据（含 ctVal、tickSize）
    pub async fn instruments(&self) -> Result<Vec<Instrument>> {
        let resp: Resp<RawInstrument> = self
            .get("/api/v5/public/instruments", &[("instType", "SWAP")])
            .await?;

        Ok(resp
            .unwrap_ok()?
            .into_iter()
            .filter(|r| r.state == "live")
            .filter_map(|r| {
                let symbol = Symbol::from_swap_inst_id(&r.inst_id)?;
                Some(Instrument {
                    symbol,
                    tick_size: r.tick_sz,
                    lot_size: r.lot_sz,
                    min_size: r.min_sz,
                    ct_val: r.ct_val.unwrap_or(Decimal::ONE),
                    max_leverage: r.lever.unwrap_or(Decimal::ONE),
                })
            })
            .collect())
    }

    /// 全市场 Ticker（一次拿全）
    pub async fn all_tickers(&self) -> Result<Vec<Ticker>> {
        let resp: Resp<RawTicker> = self
            .get("/api/v5/market/tickers", &[("instType", "SWAP")])
            .await?;

        Ok(resp
            .unwrap_ok()?
            .into_iter()
            .filter_map(|r| {
                let symbol = Symbol::from_swap_inst_id(&r.inst_id)?;
                Some(Ticker {
                    symbol,
                    last: r.last,
                    bid: r.bid_px,
                    ask: r.ask_px,
                    open_24h: r.open24h,
                    high_24h: r.high24h,
                    low_24h: r.low24h,
                    volume_24h: r.vol24h,
                    volume_quote_24h: r.vol_ccy_24h,
                    ts: r.ts.parse().unwrap_or(0),
                })
            })
            .collect())
    }

    /// K 线
    pub async fn candles(
        &self,
        symbol: &Symbol,
        interval: Interval,
        limit: u32,
    ) -> Result<Vec<Candle>> {
        let limit = limit.min(300).to_string();
        let resp: Resp<Vec<String>> = self
            .get(
                "/api/v5/market/candles",
                &[
                    ("instId", &symbol.inst_id),
                    ("bar", interval.as_okx()),
                    ("limit", &limit),
                ],
            )
            .await?;

        Ok(resp
            .unwrap_ok()?
            .into_iter()
            .filter_map(|row| parse_candle(&row))
            .collect())
    }

    /// 单个合约的资金费率
    pub async fn funding_rate(&self, inst_id: &str) -> Result<FundingRate> {
        let resp: Resp<RawFunding> = self
            .get("/api/v5/public/funding-rate", &[("instId", inst_id)])
            .await?;

        let r = resp
            .unwrap_ok()?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Msg(format!("no funding rate for {}", inst_id)))?;

        let symbol = Symbol::from_swap_inst_id(&r.inst_id)
            .ok_or_else(|| Error::Msg(format!("bad inst_id: {}", r.inst_id)))?;

        Ok(FundingRate {
            symbol,
            rate: r.funding_rate,
            next_time: r.next_funding_time.and_then(|s| s.parse().ok()),
            ts: r.ts.parse().unwrap_or(0),
        })
    }
}

impl Default for RestClient {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_candle(row: &[String]) -> Option<Candle> {
    if row.len() < 6 {
        return None;
    }
    Some(Candle {
        open_time: row[0].parse().ok()?,
        open: Decimal::from_str(&row[1]).ok()?,
        high: Decimal::from_str(&row[2]).ok()?,
        low: Decimal::from_str(&row[3]).ok()?,
        close: Decimal::from_str(&row[4]).ok()?,
        volume: Decimal::from_str(&row[5]).ok()?,
    })
}
