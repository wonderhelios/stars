//! Minimal Hyperliquid public-data client: universe, market contexts, and
//! historical candles. Read-only, no keys required.

use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::{json, Value};

const BASE: &str = "https://api.hyperliquid.xyz/info";

#[derive(Debug, Clone, Deserialize)]
pub struct CoinMeta {
    pub name: String,
    #[allow(dead_code)]
    pub sz_decimals: Option<u32>,
    #[serde(default)]
    pub is_delisted: bool,
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
        let mut out = Vec::new();
        let mut t = start_ms;
        while t < end_ms {
            let e = (t + 400 * 86_400_000).min(end_ms);
            let v = self
                .post(json!({"type": "candleSnapshot", "req": {
                    "coin": coin, "interval": "1d", "startTime": t, "endTime": e
                }}))
                .await?;
            let batch: Vec<Candle> = serde_json::from_value(v).unwrap_or_default();
            out.extend(batch);
            t = e;
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
        Ok(out)
    }
}
