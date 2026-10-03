use futures_util::{stream, StreamExt};
use reqwest::Client;
use rust_decimal::Decimal;
use serde_json::json;
use std::str::FromStr;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::error::{Error, Result};
use crate::signal::outcome_bar_open;

const BASE: &str = "https://api.hyperliquid.xyz/info";
const MAX_RETRY: u32 = 3;
const RETRY_SLEEP_MS: u64 = 400;

#[derive(serde::Deserialize)]
struct PerpDex {
    name: String,
}

#[derive(Debug, Clone)]
pub struct HlTicker {
    /// Conservative acquisition start: never label a slow response as a fresh mark.
    pub acquisition_started_at: i64,
    pub collateral_usdc: bool,
    pub coin: String,
    pub mark_px: Decimal,
    pub prev_day_px: Decimal,
    pub day_ntl_vlm: Decimal,
    pub funding: Decimal,
    pub max_leverage: Option<u32>,
    pub size_decimals: Option<u32>,
    #[allow(dead_code)]
    pub open_interest: Decimal,
    /// Exchange-provided impact bid/ask, not the best top-of-book quote.
    pub impact_bid: Option<Decimal>,
    pub impact_ask: Option<Decimal>,
}

impl HlTicker {
    pub fn prior_24h_pct(&self) -> Decimal {
        if self.prev_day_px.is_zero() {
            Decimal::ZERO
        } else {
            (self.mark_px - self.prev_day_px) / self.prev_day_px * Decimal::from(100)
        }
    }
}

pub struct HyperliquidRestClient {
    http: Client,
    base: String,
    dex_cache: Mutex<Option<(Instant, Vec<String>)>>,
}

impl HyperliquidRestClient {
    pub fn new() -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .http1_only()
            .no_gzip()
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
            dex_cache: Mutex::new(None),
        }
    }

    /// Independent pool: a stalled CDN connection must not stall the recovery read.
    fn fresh_connection(&self) -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(8))
                .connect_timeout(Duration::from_secs(3))
                .http1_only()
                .no_gzip()
                .pool_max_idle_per_host(0)
                .tcp_nodelay(true)
                .build()
                .expect("recovery client init"),
            base: self.base.clone(),
            dex_cache: Mutex::new(None),
        }
    }
    async fn execution_ctxs_by_dex(&self, dex: &str) -> Result<Vec<HlTicker>> {
        let recovery = self.fresh_connection();
        first_success(
            self.perp_ctxs_by_dex(dex),
            recovery.perp_ctxs_by_dex(dex),
            Duration::from_secs(2),
        )
        .await
    }
    pub async fn execution_l2_book(
        &self,
        coin: &str,
    ) -> anyhow::Result<trading_core::replay::Quote> {
        let recovery = self.fresh_connection();
        first_success(
            self.l2_book(coin),
            recovery.l2_book(coin),
            Duration::from_secs(1),
        )
        .await
    }

    async fn post_text(&self, body: &serde_json::Value) -> Result<String> {
        let mut last_err: Option<Error> = None;
        for attempt in 0..MAX_RETRY {
            let r = self
                .http
                .post(&self.base)
                .header("Accept-Encoding", "identity")
                .json(body)
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
        Err(last_err.unwrap_or_else(|| Error::Msg("HL post_text failed".into())))
    }

    async fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        body: serde_json::Value,
    ) -> Result<T> {
        let mut last_error = None;
        for attempt in 0..MAX_RETRY {
            let text = self.post_text(&body).await?;
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

    /// 从交易所发现主 DEX 和全部 HIP-3 DEX，避免新部署者被静态名单漏掉。
    pub async fn perp_dex_names(&self) -> Result<Vec<String>> {
        // Cache only market identities. Prices and funding are always fetched anew.
        // Failed refreshes do not extend the cache lifetime or claim full coverage.
        let mut cache = self.dex_cache.lock().await;
        if let Some((at, names)) = cache.as_ref() {
            if at.elapsed() < Duration::from_secs(600) {
                return Ok(names.clone());
            }
        }
        let recovery = self.fresh_connection();
        let dexes: Vec<Option<PerpDex>> = first_success(
            self.post_json(json!({"type":"perpDexs"})),
            recovery.post_json(json!({"type":"perpDexs"})),
            Duration::from_secs(1),
        )
        .await?;
        let names = dex_names(dexes);
        *cache = Some((Instant::now(), names.clone()));
        Ok(names)
    }

    /// 拉取指定 dex 的所有永续合约状态
    pub async fn perp_ctxs_by_dex(&self, dex: &str) -> Result<Vec<HlTicker>> {
        let body = if dex.is_empty() {
            json!({"type": "metaAndAssetCtxs"})
        } else {
            json!({"type": "metaAndAssetCtxs", "dex": dex})
        };

        let acquisition_started_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| Error::Msg(e.to_string()))?
            .as_millis() as i64;
        let resp: serde_json::Value = self.post_json(body).await?;

        let universe = resp
            .get(0)
            .and_then(|v| v.get("universe"))
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error::Msg(format!("bad universe for dex={}", dex)))?;
        let ctxs = resp
            .get(1)
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error::Msg(format!("missing ctxs for dex={}", dex)))?;

        if universe.len() != ctxs.len() {
            return Err(Error::Msg(format!("incomplete market response dex={dex}")));
        }
        let mut result = Vec::new();
        for (u, ctx) in universe.iter().zip(ctxs.iter()) {
            let coin = u
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::Msg(format!("missing market identity dex={dex}")))?;
            if u.get("isDelisted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                continue;
            }
            // Missing/invalid active-market fields make this DEX incomplete.
            // Silently skipping a row would incorrectly certify full coverage.
            let required = |key: &str| -> Result<Decimal> {
                let value = ctx
                    .get(key)
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::Msg(format!("missing {key} for {coin}")))?;
                Decimal::from_str(value)
                    .map_err(|e| Error::Msg(format!("invalid {key} for {coin}: {e}")))
            };
            let funding = required("funding")?;
            let mark_px = required("markPx")?;
            let prev_day_px = required("prevDayPx")?;
            let day_ntl_vlm = required("dayNtlVlm")?;
            let oi_str = ctx
                .get("openInterest")
                .and_then(|v| v.as_str())
                .unwrap_or("0");
            let impact = ctx.get("impactPxs").and_then(|v| v.as_array());
            let impact_bid = impact
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .and_then(|v| Decimal::from_str(v).ok());
            let impact_ask = impact
                .and_then(|v| v.get(1))
                .and_then(|v| v.as_str())
                .and_then(|v| Decimal::from_str(v).ok());

            result.push(HlTicker {
                acquisition_started_at,
                collateral_usdc: dex.is_empty() || resp[0]["collateralToken"].as_u64() == Some(0),
                coin: coin.to_string(),
                mark_px,
                prev_day_px,
                day_ntl_vlm,
                funding,
                max_leverage: u
                    .get("maxLeverage")
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u32::try_from(v).ok()),
                size_decimals: u
                    .get("szDecimals")
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u32::try_from(v).ok()),
                open_interest: Decimal::from_str(oi_str).unwrap_or(Decimal::ZERO),
                impact_bid,
                impact_ask,
            });
        }
        Ok(result)
    }

    /// 遍历所有 dex（主 + HIP-3），合并返回
    pub async fn all_perp_ctxs_all_dexes(&self) -> Result<(Vec<HlTicker>, bool)> {
        let (tickers, complete, _) = self.perp_ctxs_with_coverage().await?;
        Ok((tickers, complete))
    }
    pub async fn perp_ctxs_with_coverage(
        &self,
    ) -> Result<(Vec<HlTicker>, bool, std::collections::HashSet<String>)> {
        self.perp_ctxs_with_budget(Duration::from_secs(35), Duration::from_secs(8))
            .await
    }

    /// Execution frames must leave room for fresh books; slow scopes remain missing.
    pub async fn execution_perp_ctxs_with_coverage(
        &self,
    ) -> Result<(Vec<HlTicker>, bool, std::collections::HashSet<String>)> {
        self.perp_ctxs_with_budget(Duration::from_secs(8), Duration::from_secs(3))
            .await
    }

    async fn perp_ctxs_with_budget(
        &self,
        market_budget: Duration,
        discovery_budget: Duration,
    ) -> Result<(Vec<HlTicker>, bool, std::collections::HashSet<String>)> {
        // One budget includes discovery. Never wait for a stalled scope until
        // successful scopes have already aged beyond the execution freshness window.
        let deadline = tokio::time::Instant::now() + market_budget;
        let dexes = tokio::time::timeout(discovery_budget, self.perp_dex_names())
            .await
            .map_err(|_| Error::Msg("Hyperliquid perpDexs timeout".into()))??;
        let mut all = Vec::new();
        let mut loaded = 0usize;
        let mut scopes = std::collections::HashSet::new();
        let mut queries = stream::iter(dexes.iter().cloned())
            .map(|dex| async move {
                let request = async {
                    if market_budget <= Duration::from_secs(8) {
                        self.execution_ctxs_by_dex(&dex).await
                    } else {
                        self.perp_ctxs_by_dex(&dex).await
                    }
                };
                let result = tokio::time::timeout_at(deadline, request).await;
                (dex, result)
            })
            .buffer_unordered(12);
        loop {
            let (dex, result) = match tokio::time::timeout_at(deadline, queries.next()).await {
                Ok(Some(next)) => next,
                Ok(None) => break,
                Err(_) => {
                    tracing::warn!(
                        "HL market fetch budget exhausted; only fresh successful DEXes retained"
                    );
                    break;
                }
            };
            match result {
                Ok(Ok(mut tickers)) => {
                    all.append(&mut tickers);
                    loaded += 1;
                    scopes.insert(if dex.is_empty() { "main".into() } else { dex });
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        "HL dex={} failed: {}",
                        if dex.is_empty() { "main" } else { &dex },
                        e
                    );
                }
                Err(_) => tracing::warn!(
                    "HL dex={} timeout",
                    if dex.is_empty() { "main" } else { &dex }
                ),
            }
        }
        if all.is_empty() {
            return Err(Error::Msg("no Hyperliquid perp contexts loaded".into()));
        }
        if loaded < dexes.len() {
            tracing::warn!("HL loaded {}/{} perp DEXes", loaded, dexes.len());
        }
        tracing::info!(
            "HL DEX coverage: {}/{} DEXes, {} perps",
            loaded,
            dexes.len(),
            all.len()
        );
        Ok((all, loaded == dexes.len(), scopes))
    }

    pub async fn l2_book(&self, coin: &str) -> anyhow::Result<trading_core::replay::Quote> {
        let raw = self.post_json(json!({"type":"l2Book","coin":coin})).await?;
        crate::execution_research::parse_book(raw)
    }

    /// 获取指定时间点的收盘价。coin 用全名，例如 "BTC" 或 "para:TREAD"
    pub async fn price_at_time(&self, coin: &str, target_ts: i64) -> Result<Option<Decimal>> {
        let bar_time = outcome_bar_open(target_ts);
        let start = bar_time;
        let end = bar_time + 60_000;

        let body = json!({
            "type": "candleSnapshot",
            "req": {
                "coin": coin,
                "interval": "1m",
                "startTime": start,
                "endTime": end
            }
        });

        let resp: Vec<serde_json::Value> = self.post_json(body).await?;

        for row in resp {
            let Some(t) = row.get("t").and_then(|v| v.as_i64()) else {
                continue;
            };
            if t == bar_time {
                let close_str = row.get("c").and_then(|v| v.as_str()).unwrap_or("");
                return Ok(Decimal::from_str(close_str).ok());
            }
        }
        Ok(None)
    }
}

fn dex_names(dexes: Vec<Option<PerpDex>>) -> Vec<String> {
    let mut names = vec![String::new()];
    for dex in dexes.into_iter().flatten() {
        if !dex.name.is_empty() && !names.contains(&dex.name) {
            names.push(dex.name);
        }
    }
    names
}

/// Delayed duplicate reads only: first valid result wins; dropping the other
/// future cancels it. No retries of orders and no extension of the caller's budget.
async fn first_success<T, E>(
    primary: impl std::future::Future<Output = std::result::Result<T, E>>,
    recovery: impl std::future::Future<Output = std::result::Result<T, E>>,
    delay: Duration,
) -> std::result::Result<T, E> {
    tokio::pin!(primary, recovery);
    tokio::select! {
        result=&mut primary => return match result {Ok(v)=>Ok(v),Err(_)=>recovery.await},
        _=tokio::time::sleep(delay)=>{}
    }
    tokio::select! {
        result=&mut primary => match result {Ok(v)=>Ok(v),Err(_)=>recovery.await},
        result=&mut recovery => match result {Ok(v)=>Ok(v),Err(_)=>primary.await},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn recovery_read_wins_over_stalled_primary_and_survives_bad_reply() {
        let stalled = std::future::pending::<std::result::Result<u32, &str>>();
        let result = tokio::time::timeout(
            Duration::from_millis(100),
            first_success(stalled, async { Ok(7) }, Duration::from_millis(5)),
        )
        .await
        .unwrap();
        assert_eq!(result, Ok(7));
        assert_eq!(
            first_success(
                async { Err("truncated") },
                async { Ok::<_, &str>(8) },
                Duration::from_secs(2)
            )
            .await,
            Ok(8)
        );
        let result = first_success(
            async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(9)
            },
            async { Err("bad recovery") },
            Duration::from_millis(1),
        )
        .await;
        assert_eq!(result, Ok(9));
    }

    #[tokio::test]
    async fn dex_discovery_cache_expires_and_never_masks_failed_refresh() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let app = axum::Router::new().route(
            "/info",
            axum::routing::post(move || {
                let observed = observed.clone();
                async move {
                    let n = observed.fetch_add(1, Ordering::SeqCst);
                    if n == 0 {
                        axum::Json(json!([null, {"name":"xyz"}]))
                    } else {
                        axum::Json(json!({"unexpected":"response"}))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut client = HyperliquidRestClient::new();
        client.base = format!("http://{addr}/info");
        assert_eq!(client.perp_dex_names().await.unwrap(), vec!["", "xyz"]);
        assert_eq!(client.perp_dex_names().await.unwrap(), vec!["", "xyz"]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        client.dex_cache.lock().await.as_mut().unwrap().0 =
            Instant::now() - Duration::from_secs(601);
        assert!(client.perp_dex_names().await.is_err());
        // Failed refresh exhausts both independent decode paths, never returns stale names.
        assert_eq!(calls.load(Ordering::SeqCst), 7);
        server.abort();
    }

    #[tokio::test]
    async fn fast_scope_is_retained_without_waiting_for_stalled_scope() {
        let app = axum::Router::new().route(
            "/info",
            axum::routing::post(
                |axum::Json(body): axum::Json<serde_json::Value>| async move {
                    if body["dex"] == "slow" {
                        std::future::pending::<()>().await;
                    }
                    axum::Json(json!([
                        {"universe":[{"name":"AAA","maxLeverage":10,"szDecimals":2}]},
                        [{"funding":"0.0001","markPx":"11","prevDayPx":"10","dayNtlVlm":"1000000"}]
                    ]))
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut client = HyperliquidRestClient::new();
        client.base = format!("http://{addr}/info");
        *client.dex_cache.lock().await = Some((Instant::now(), vec!["".into(), "slow".into()]));
        let started = Instant::now();
        let (tickers, complete, scopes) = client
            .perp_ctxs_with_budget(Duration::from_millis(150), Duration::from_millis(50))
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(tickers.len(), 1);
        assert!(!complete);
        assert!(scopes.contains("main"));
        assert!(!scopes.contains("slow"));
        assert!(crate::paper::now_ms() - tickers[0].acquisition_started_at < 1_000);
        server.abort();
    }

    #[tokio::test]
    async fn missing_active_market_fields_cannot_claim_full_dex_coverage() {
        let app = axum::Router::new().route(
            "/info",
            axum::routing::post(|| async {
                axum::Json(json!([
                    {"universe":[{"name":"AAA","maxLeverage":10,"szDecimals":2}]},
                    [{"funding":"0.0001","prevDayPx":"10","dayNtlVlm":"1000000"}]
                ]))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut client = HyperliquidRestClient::new();
        client.base = format!("http://{addr}/info");
        let error = client.perp_ctxs_by_dex("").await.unwrap_err();
        assert!(error.to_string().contains("missing markPx"));
        server.abort();
    }

    #[test]
    fn discovers_new_hip3_dexes_after_main_market() {
        let response: Vec<Option<PerpDex>> =
            serde_json::from_str(r#"[null,{"name":"xyz"},{"name":"cash"}]"#).unwrap();
        assert_eq!(dex_names(response), vec!["", "xyz", "cash"]);
    }
}

impl Default for HyperliquidRestClient {
    fn default() -> Self {
        Self::new()
    }
}
