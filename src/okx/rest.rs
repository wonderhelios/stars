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
    #[allow(dead_code)]
    #[serde(default)]
    ct_val: Option<Decimal>,
}

/// REST 客户端：启动时拉一次全市场 SWAP 的 instId 列表
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
            // 只保留 U 本位（quote 为 USDT），避免币本位结算逻辑复杂
            .filter(|r| r.inst_id.ends_with("-USDT-SWAP"))
            .map(|r| r.inst_id)
            .collect())
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
