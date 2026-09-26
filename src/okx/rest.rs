use reqwest::Client;
use rust_decimal::Decimal;
use std::str::FromStr;
use std::time::Duration;

use crate::error::{Error, Result};

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
    state: String,
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

/// REST 客户端：启动时拉全市场 instId；研究模式下拉历史费率与 K线
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

    /// 拉取全部 live 状态的 USDT 本位 SWAP instId
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

    /// 历史资金费率（最多 100 条）
    pub async fn funding_rate_history(
        &self,
        inst_id: &str,
        limit: u32,
    ) -> Result<Vec<FundingHistoryItem>> {
        let limit = limit.min(100).to_string();
        let resp: Resp<FundingHistoryItem> = self
            .get(
                "/api/v5/public/funding-rate-history",
                &[("instId", inst_id), ("limit", &limit)],
            )
            .await?;
        resp.unwrap_ok()
    }

    /// 最近 1H K线（时间正序返回，最早在前）
    pub async fn candles_1h(&self, inst_id: &str, limit: u32) -> Result<Vec<Candle1H>> {
        let limit = limit.min(300).to_string();
        let resp: Resp<Vec<String>> = self
            .get(
                "/api/v5/market/candles",
                &[("instId", inst_id), ("bar", "1H"), ("limit", &limit)],
            )
            .await?;

        let mut candles: Vec<Candle1H> = resp
            .unwrap_ok()?
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

        // OKX 返回：新 → 旧。反转成正序。
        candles.reverse();
        Ok(candles)
    }
}

impl Default for RestClient {
    fn default() -> Self {
        Self::new()
    }
}

/// 辅助：把 JSON 里的字符串字段解析为 Decimal
pub fn parse_dec(v: &serde_json::Value, key: &str) -> Option<Decimal> {
    let s = v.get(key)?.as_str()?;
    if s.is_empty() {
        return None;
    }
    Decimal::from_str(s).ok()
}
