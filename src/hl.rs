//! Minimal Hyperliquid public-data client: universe, market contexts, and
//! historical candles. Read-only, no keys required.

use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::{json, Value};

const BASE: &str = "https://api.hyperliquid.xyz/info";

#[derive(Debug, Clone, Deserialize)]
pub struct CoinMeta {
    pub name: String,
    #[serde(default = "default_sz_decimals", rename = "szDecimals")]
    pub sz_decimals: i32,
    #[serde(default = "default_max_lev", rename = "maxLeverage")]
    pub max_leverage: u32,
    #[serde(default, rename = "isDelisted")]
    pub is_delisted: bool,
}

fn default_max_lev() -> u32 {
    10
}

/// Fallback lot precision when the API omits it.
fn default_sz_decimals() -> i32 {
    4
}

/// Hyperliquid maintenance margin rate for a coin: half the initial margin at
/// the coin's max leverage.
pub fn maintenance_margin_rate(max_leverage: u32) -> f64 {
    let lev = max_leverage.max(1) as f64;
    0.5 / lev
}

#[derive(Debug, Clone)]
pub struct MarketCtx {
    pub coin: String,
    pub funding: f64,       // hourly funding rate (fraction)
    pub oracle_px: f64,
    pub prev_day_px: f64,
    pub day_ntl_vlm: f64,   // 24h notional volume, USD
    pub is_delisted: bool,
}

impl MarketCtx {
    pub fn prior_24h_pct(&self) -> f64 {
        if self.prev_day_px <= 0.0 {
            0.0
        } else {
            (self.oracle_px - self.prev_day_px) / self.prev_day_px * 100.0
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Candle {
    pub t: i64, // open time ms
    #[serde(deserialize_with = "de_f64")]
    pub o: f64,
    #[serde(deserialize_with = "de_f64")]
    pub h: f64,
    #[serde(deserialize_with = "de_f64")]
    pub l: f64,
    #[serde(deserialize_with = "de_f64")]
    pub c: f64,
    #[serde(deserialize_with = "de_f64")]
    pub v: f64, // base volume
}

fn de_f64<'de, D>(d: D) -> std::result::Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::String(s) => s.parse::<f64>().map_err(serde::de::Error::custom),
        serde_json::Value::Number(n) => Ok(n.as_f64().unwrap_or(0.0)),
        _ => Err(serde::de::Error::custom("expected number or numeric string")),
    }
}

#[derive(Clone)]
pub struct HlClient {
    http: reqwest::Client,
}

impl HlClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client");
        Self { http }
    }

    async fn post(&self, payload: Value) -> Result<Value> {
        let mut last: Option<anyhow::Error> = None;
        for attempt in 0..6u32 {
            match self
                .http
                .post(BASE)
                .json(&payload)
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.status().is_success() {
                        return Ok(resp.json().await?);
                    }
                    if resp.status().as_u16() == 429 {
                        tokio::time::sleep(std::time::Duration::from_millis(
                            400 * 2u64.pow(attempt),
                        ))
                        .await;
                        last = Some(anyhow!("rate limited"));
                        continue;
                    }
                    last = Some(anyhow!("HTTP {}", resp.status()));
                }
                Err(e) => last = Some(anyhow!("request failed: {e}")),
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
        Err(last.unwrap_or_else(|| anyhow!("post failed")))
    }

    /// Full universe (including delisted coins).
    pub async fn universe(&self) -> Result<Vec<CoinMeta>> {
        let v = self.post(json!({"type": "meta"})).await?;
        let metas: Vec<CoinMeta> = serde_json::from_value(v["universe"].clone())
            .map_err(|e| anyhow!("parse universe: {e}"))?;
        Ok(metas)
    }

    /// Current market contexts (funding / price / 24h volume) for all coins.
    pub async fn market_ctxs(&self) -> Result<Vec<MarketCtx>> {
        let v = self.post(json!({"type": "metaAndAssetCtxs"})).await?;
        let metas: Vec<CoinMeta> = serde_json::from_value(v[0]["universe"].clone())?;
        let ctxs: Vec<Value> = serde_json::from_value(v[1].clone())?;
        let mut out = Vec::new();
        for (m, c) in metas.into_iter().zip(ctxs.into_iter()) {
            out.push(MarketCtx {
                coin: m.name,
                funding: c["funding"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0),
                oracle_px: c["oraclePx"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0),
                prev_day_px: c["prevDayPx"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0),
                day_ntl_vlm: c["dayNtlVlm"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0),
                is_delisted: m.is_delisted,
            });
        }
        Ok(out)
    }

    /// Daily candles for one coin over [start_ms, end_ms).
    pub async fn daily_candles(&self, coin: &str, start_ms: i64, end_ms: i64) -> Result<Vec<Candle>> {
        self.candles(coin, "1d", start_ms, end_ms, 400 * 86_400_000).await
    }

    /// Candles at an arbitrary interval ("1h", "4h", "15m", ...).
    /// `window_ms` caps how much history one request covers.
    pub async fn candles(
        &self,
        coin: &str,
        interval: &str,
        start_ms: i64,
        end_ms: i64,
        window_ms: i64,
    ) -> Result<Vec<Candle>> {
        let mut out = Vec::new();
        let mut t = start_ms;
        while t < end_ms {
            let e = (t + window_ms).min(end_ms);
            let v = self
                .post(json!({"type": "candleSnapshot", "req": {
                    "coin": coin, "interval": interval, "startTime": t, "endTime": e
                }}))
                .await?;
            let batch: Vec<Candle> = serde_json::from_value(v).unwrap_or_default();
            out.extend(batch);
            t = e;
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
        Ok(out)
    }

    /// Hourly candles: one request covers ~400 hours.

    /// Exchange fee schedule (base tier). `cross` = taker, `add` = maker.
    /// The zero address returns the standard tier every new account starts on.
    pub async fn fee_schedule(&self) -> Result<(f64, f64)> {
        let v = self
            .post(json!({"type": "userFees",
                         "user": "0x0000000000000000000000000000000000000000"}))
            .await?;
        let cross = v["feeSchedule"]["cross"]
            .as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.00045);
        let add = v["feeSchedule"]["add"]
            .as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.00015);
        Ok((cross, add))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: the API spells it `szDecimals`; without the rename every coin
    /// silently fell back to 4 decimals and every live order was rejected.
    #[test]
    fn coin_meta_reads_sz_decimals() {
        let raw = r#"{"name":"PUMP","szDecimals":0,"maxLeverage":10,"isDelisted":false}"#;
        let c: CoinMeta = serde_json::from_str(raw).unwrap();
        assert_eq!(c.sz_decimals, 0);
        assert_eq!(c.max_leverage, 10);

        let raw2 = r#"{"name":"BCH","szDecimals":3}"#;
        let c2: CoinMeta = serde_json::from_str(raw2).unwrap();
        assert_eq!(c2.sz_decimals, 3);
        assert_eq!(c2.max_leverage, 10); // default applies
        assert!(!c2.is_delisted);
    }

    /// Regression: the API spells it `isDelisted`; without the rename delisted
    /// coins looked active and were re-downloaded on every restart.
    #[test]
    fn coin_meta_reads_is_delisted() {
        let raw = r#"{"name":"MATIC","szDecimals":1,"isDelisted":true}"#;
        let c: CoinMeta = serde_json::from_str(raw).unwrap();
        assert!(c.is_delisted);
    }
}
