use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use std::sync::Arc;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, services::ServeDir};

use crate::paper_store::{FundingSnapshotRow, PaperDb, SignalRow};
use crate::state::SharedState;
use crate::types::MarketSnapshot;

#[derive(Clone)]
pub struct WebState {
    pub app: SharedState,
    pub okx_paper: Arc<PaperDb>,
    pub binance_paper: Arc<PaperDb>,
    pub hl_paper: Arc<PaperDb>,
    pub http: reqwest::Client,
    pub strategy_library_url: String,
}

pub fn router(state: WebState) -> Router {
    Router::new()
        .route("/api/snapshot", get(get_snapshot))
        .route("/api/paper/signals", get(get_okx_signals))
        .route("/api/binance/signals", get(get_binance_signals))
        .route("/api/hyperliquid/signals", get(get_hl_signals))
        .route("/api/research/funding/latest", get(get_latest_funding))
        .route("/api/research/evidence", get(get_research_evidence))
        .route("/api/research/handoff", get(get_handoff_status))
        .route("/api/research/strategies", post(publish_strategy))
        .route("/api/research/execution", post(execution_replay))
        .route("/api/research/cycles", get(get_execution_cycles))
        .route("/api/health", get(health))
        .fallback_service(ServeDir::new("static"))
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

#[derive(serde::Deserialize)]
struct ReplayRequest {
    strategy: trading_core::strategy::StrategyConfig,
    #[serde(default = "default_capital")]
    capital: f64,
    #[serde(default)]
    cycle: Option<String>,
}
fn default_capital() -> f64 {
    500.0
}

async fn get_execution_cycles(State(state): State<WebState>) -> Response {
    match state.hl_paper.execution_cycles().await {
        Ok(value) => Json(value).into_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error":error.to_string()})),
        )
            .into_response(),
    }
}

async fn execution_replay(
    State(state): State<WebState>,
    Json(input): Json<ReplayRequest>,
) -> Response {
    match state
        .hl_paper
        .execution_candidate_for_cycle(input.strategy, input.capital, input.cycle)
        .await
    {
        Ok(result) => Json(result).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":error.to_string()})),
        )
            .into_response(),
    }
}

async fn publish_strategy(
    State(state): State<WebState>,
    Json(strategy): Json<serde_json::Value>,
) -> Response {
    let input = match serde_json::from_value::<trading_core::strategy::StrategyConfig>(strategy) {
        Ok(input) => input,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error":e.to_string()})),
            )
                .into_response()
        }
    };
    // Fail clearly before posting newly introduced fields to an outdated receiver.
    let compatibility=async {
        let response=state.http.get(&state.strategy_library_url).timeout(std::time::Duration::from_secs(8)).send().await?;
        anyhow::ensure!(response.status().is_success(),"Hyper Fly策略库状态接口失败：{}",response.status());
        let status=response.json::<serde_json::Value>().await?;
        anyhow::ensure!(status["rule_hash"].as_str()==Some(trading_core::rule_hash().as_str()),
            "Hyper Fly看板未升级或共享规则不一致。请更新Hyper Fly，重启看板和机器人，再重新回放并保存；不能丢弃止盈字段来兼容旧版。");
        Ok::<_,anyhow::Error>(())
    }.await;
    if let Err(error) = compatibility {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error":error.to_string()})),
        )
            .into_response();
    }
    // Recompute evidence on the server. The browser cannot promote a sampled result.
    let candidate = match state.hl_paper.execution_candidate(input, 500.0).await {
        Ok(result) => result,
        Err(error) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error":error.to_string()})),
            )
                .into_response()
        }
    };
    let strategy = &candidate["strategy"];
    let response = match state
        .http
        .post(&state.strategy_library_url)
        .json(&strategy)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::error!("strategy library write failed: {}", error);
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error":"无法连接 Hyper Fly 策略库，请确认看板服务正在运行"
                })),
            )
                .into_response();
        }
    };
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let raw = match response.text().await {
        Ok(raw) => raw,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error":format!("读取Hyper Fly策略库响应失败：{e}")})),
            )
                .into_response()
        }
    };
    let body = strategy_library_reply(status, &raw);
    (status, Json(body)).into_response()
}

fn strategy_library_reply(status: StatusCode, raw: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(&raw).unwrap_or_else(|_| {
        let detail: String = raw.chars().take(1500).collect();
        serde_json::json!({"error":format!("Hyper Fly拒绝策略（HTTP {}）：{}。若提示未知止盈字段，请更新Hyper Fly并重启看板服务。",status.as_u16(),detail)})
    })
}

#[cfg(test)]
mod publish_reply_tests {
    use super::*;
    #[test]
    fn plain_422_keeps_field_error_and_upgrade_hint() {
        let raw = "Failed to deserialize: unknown field `take_profit_price_pct`";
        let reply = strategy_library_reply(StatusCode::UNPROCESSABLE_ENTITY, raw);
        let error = reply["error"].as_str().unwrap();
        assert!(error.contains(raw));
        assert!(error.contains("422"));
        assert!(error.contains("更新Hyper Fly"));
        assert_eq!(
            strategy_library_reply(StatusCode::OK, r#"{"saved":true}"#)["saved"],
            true
        );
    }
}

async fn health() -> &'static str {
    "ok"
}

async fn get_snapshot(State(state): State<WebState>) -> Json<Vec<MarketSnapshot>> {
    let map = state.app.snapshots.read().await;
    let mut list: Vec<MarketSnapshot> = map.values().cloned().collect();
    list.sort_by(|a, b| b.volume_quote_24h.cmp(&a.volume_quote_24h));
    Json(list)
}

async fn get_okx_signals(
    State(state): State<WebState>,
) -> Result<Json<Vec<SignalRow>>, StatusCode> {
    read_signals(&state.okx_paper).await
}

async fn get_binance_signals(
    State(state): State<WebState>,
) -> Result<Json<Vec<SignalRow>>, StatusCode> {
    read_signals(&state.binance_paper).await
}

async fn get_hl_signals(State(state): State<WebState>) -> Result<Json<Vec<SignalRow>>, StatusCode> {
    read_signals(&state.hl_paper).await
}

async fn read_signals(db: &PaperDb) -> Result<Json<Vec<SignalRow>>, StatusCode> {
    db.all_signals().await.map(Json).map_err(|error| {
        tracing::error!("paper signals read failed: {}", error);
        StatusCode::SERVICE_UNAVAILABLE
    })
}

#[derive(serde::Serialize)]
struct VenueFundingSnapshot {
    venue: &'static str,
    #[serde(flatten)]
    snapshot: FundingSnapshotRow,
}

async fn get_latest_funding(
    State(state): State<WebState>,
) -> Result<Json<Vec<VenueFundingSnapshot>>, StatusCode> {
    let (okx, binance, hyperliquid) = tokio::join!(
        state.okx_paper.latest_funding_snapshots(),
        state.binance_paper.latest_funding_snapshots(),
        state.hl_paper.latest_funding_snapshots(),
    );
    let mut result = Vec::new();
    let mut succeeded = 0usize;
    for (venue, rows) in [
        ("OKX", okx),
        ("Binance", binance),
        ("Hyperliquid", hyperliquid),
    ] {
        match rows {
            Ok(rows) => {
                succeeded += 1;
                result.extend(
                    rows.into_iter()
                        .map(|snapshot| VenueFundingSnapshot { venue, snapshot }),
                );
            }
            Err(error) => tracing::error!("{} funding research read failed: {}", venue, error),
        }
    }
    if succeeded == 0 {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    } else {
        Ok(Json(result))
    }
}

// One cached analysis per process: simultaneous browser polls never duplicate a large historical read.
#[derive(serde::Deserialize)]
struct ResearchRange {
    cycle: Option<String>,
}
async fn get_research_evidence(
    State(state): State<WebState>,
    Query(range): Query<ResearchRange>,
) -> Response {
    let periods = match state.hl_paper.execution_cycles().await {
        Ok(value) => value,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error":e.to_string()})),
            )
                .into_response()
        }
    };
    let cycle = range
        .cycle
        .unwrap_or_else(|| periods["active_cycle"].as_str().unwrap_or("").to_owned());
    let since = if cycle == "all" {
        0
    } else {
        match periods["cycles"]
            .as_array()
            .and_then(|rows| rows.iter().find(|c| c["id"].as_str() == Some(&cycle)))
            .and_then(|c| c["started_at"].as_i64())
        {
            Some(at) => at,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error":"unknown research cycle"})),
                )
                    .into_response()
            }
        }
    };
    static CACHE: std::sync::OnceLock<
        tokio::sync::Mutex<Option<(std::time::Instant, String, serde_json::Value)>>,
    > = std::sync::OnceLock::new();
    let mut cached = CACHE
        .get_or_init(|| tokio::sync::Mutex::new(None))
        .lock()
        .await;
    if let Some((at, cached_cycle, value)) = cached.as_ref() {
        if cached_cycle == &cycle && at.elapsed() < std::time::Duration::from_secs(120) {
            return Json(value.clone()).into_response();
        }
    }
    let (okx, binance, hl) = tokio::join!(
        state.okx_paper.research_history(),
        state.binance_paper.research_history(),
        state.hl_paper.research_history()
    );
    let mut inputs = Vec::new();
    let mut errors = Vec::new();
    for (venue, result) in [("OKX", okx), ("Binance", binance), ("Hyperliquid", hl)] {
        match result {
            Ok((at, rows)) => inputs.push((
                venue,
                at.max(since),
                crate::research_lab::cycle_points(rows, since),
            )),
            Err(e) => errors.push(format!("{venue}: {e}")),
        }
    }
    let execution = state.hl_paper.execution_coverage().await;
    match tokio::task::spawn_blocking(move || crate::research_lab::analyze(inputs, errors)).await {
        Ok(report) => {
            let mut value = serde_json::to_value(report).expect("serializable research report");
            value["collection_cycle"] = serde_json::json!(cycle);
            value["cycle_started_at"] = serde_json::json!(since);
            value["execution_coverage"] = match execution {
                Ok(v) => v,
                Err(e) => serde_json::json!({"error":e.to_string()}),
            };
            *cached = Some((std::time::Instant::now(), cycle.clone(), value.clone()));
            Json(value).into_response()
        }
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error":e.to_string()})),
        )
            .into_response(),
    }
}

async fn get_handoff_status(State(state): State<WebState>) -> Response {
    let result = async {
        let response = state
            .http
            .get(&state.strategy_library_url)
            .timeout(std::time::Duration::from_secs(8))
            .send()
            .await?;
        anyhow::ensure!(
            response.status().is_success(),
            "Hyper Fly 策略状态读取失败：{}",
            response.status()
        );
        Ok::<_, anyhow::Error>(response.json::<serde_json::Value>().await?)
    }
    .await;
    match result {
        Ok(mut value) => {
            let compatible =
                value["rule_hash"].as_str() == Some(trading_core::rule_hash().as_str());
            value["compatible_rules"] = serde_json::json!(compatible);
            if !compatible {
                value["compatibility_error"]=serde_json::json!("研究服务与Hyper Fly规则版本不同或看板未升级，请同时更新两项目并重启看板和机器人；然后重新回放。");
            }
            Json(value).into_response()
        }
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error":error.to_string()})),
        )
            .into_response(),
    }
}
