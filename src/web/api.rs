use axum::{
    extract::State,
    http::{header, StatusCode},
    routing::get,
    Json, Router,
};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, services::ServeDir};

use crate::paper_store::{PaperDb, SignalRow};
use crate::state::SharedState;
use crate::types::MarketSnapshot;

#[derive(Clone)]
pub struct WebState {
    pub app: SharedState,
    pub okx_paper: Arc<PaperDb>,
    pub binance_paper: Arc<PaperDb>,
    pub hl_paper: Arc<PaperDb>,
    /// Loopback-only Hyper Fly paper dashboard port. No live account is read.
    pub hyper_fly_research_port: Option<u16>,
}

#[derive(Serialize)]
struct ExecutionSummary {
    configured: bool,
    available: bool,
    as_of_ms: Option<i64>,
    last_scan_ms: Option<i64>,
    scan_filters: Option<ExecutionFilters>,
    paused: bool,
    halted: bool,
    risk_limit: Option<String>,
    active_positions: Option<usize>,
    max_positions: Option<u64>,
    realized_profit: Option<f64>,
    realized_loss: Option<f64>,
    unknown_realized_count: Option<u64>,
    exchange_stale: bool,
    mark_gaps: Option<u64>,
    equity: Option<f64>,
    curve: Vec<ResearchEquityPoint>,
    trades: Vec<ResearchTrade>,
    error: Option<&'static str>,
}

#[derive(Serialize)]
struct ResearchEquityPoint {
    ts: i64,
    equity: f64,
}

#[derive(Serialize)]
struct ResearchTrade {
    coin: String,
    entry_at: Option<i64>,
    exit_at: Option<i64>,
    pnl: Option<f64>,
    note: Option<String>,
    entry_leverage: u64,
}

#[derive(Serialize)]
struct ExecutionFilters {
    total: Option<u64>,
    invalid_market: Option<u64>,
    leverage: Option<u64>,
    rise: Option<u64>,
    funding: Option<u64>,
    volume: Option<u64>,
    eligible: Option<u64>,
}

impl ExecutionSummary {
    fn unavailable(configured: bool, error: Option<&'static str>) -> Self {
        Self {
            configured,
            available: false,
            as_of_ms: None,
            last_scan_ms: None,
            scan_filters: None,
            paused: false,
            halted: false,
            risk_limit: None,
            active_positions: None,
            max_positions: None,
            realized_profit: None,
            realized_loss: None,
            unknown_realized_count: None,
            exchange_stale: false,
            mark_gaps: None,
            equity: None,
            curve: Vec::new(),
            trades: Vec::new(),
            error,
        }
    }
}

pub fn router(state: WebState) -> Router {
    Router::new()
        .route("/api/snapshot", get(get_snapshot))
        .route("/api/paper/signals", get(get_okx_signals))
        .route("/api/binance/signals", get(get_binance_signals))
        .route("/api/hyperliquid/signals", get(get_hl_signals))
        .route("/api/hyperliquid/simulation", get(get_hl_execution))
        .route("/api/health", get(health))
        .fallback_service(ServeDir::new("static"))
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .with_state(state)
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

async fn get_hl_execution(State(state): State<WebState>) -> impl axum::response::IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(read_hl_execution(&state).await),
    )
}

async fn read_hl_execution(state: &WebState) -> ExecutionSummary {
    let Some(port) = state.hyper_fly_research_port else {
        return ExecutionSummary::unavailable(false, None);
    };
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .no_proxy()
        .build()
    {
        Ok(client) => client,
        Err(_) => return ExecutionSummary::unavailable(true, Some("读取服务初始化失败")),
    };
    let url = format!("http://127.0.0.1:{port}/api/research");
    let result = client.get(url).send().await;
    let data: Value = match result {
        Ok(response) if response.status().is_success() => match response.json().await {
            Ok(value) => value,
            Err(_) => return ExecutionSummary::unavailable(true, Some("模拟数据解析失败")),
        },
        _ => return ExecutionSummary::unavailable(true, Some("本机模拟服务暂时不可用")),
    };
    execution_summary(&data)
        .unwrap_or_else(|| ExecutionSummary::unavailable(true, Some("模拟服务返回的数据不完整")))
}

fn execution_summary(data: &Value) -> Option<ExecutionSummary> {
    if data.get("mode")?.as_str()? != "paper" {
        return None;
    }
    let profit = data.get("realized_profit")?.as_f64()?;
    let loss = data.get("realized_loss")?.as_f64()?;
    if !profit.is_finite() || !loss.is_finite() {
        return None;
    }
    Some(ExecutionSummary {
        configured: true,
        available: true,
        as_of_ms: data.get("as_of_ms").and_then(Value::as_i64),
        last_scan_ms: data.get("last_scan_ms").and_then(Value::as_i64),
        scan_filters: data.get("scan_filters").filter(|v| v.is_object()).map(|v| {
            ExecutionFilters {
                total: v.get("total").and_then(Value::as_u64),
                invalid_market: v.get("invalid_market").and_then(Value::as_u64),
                leverage: v.get("leverage").and_then(Value::as_u64),
                rise: v.get("rise").and_then(Value::as_u64),
                funding: v.get("funding").and_then(Value::as_u64),
                volume: v.get("volume").and_then(Value::as_u64),
                eligible: v.get("eligible").and_then(Value::as_u64),
            }
        }),
        paused: data.get("paused").and_then(Value::as_bool).unwrap_or(false),
        halted: data.get("halted").is_some_and(|v| !v.is_null()),
        risk_limit: data
            .get("risk_limit")
            .and_then(|v| v.get("kind"))
            .and_then(Value::as_str)
            .filter(|v| *v == "daily" || *v == "total")
            .map(str::to_owned),
        active_positions: data.get("active_positions").and_then(Value::as_u64).map(|n| n as usize),
        max_positions: data.get("max_positions").and_then(Value::as_u64),
        realized_profit: Some(profit),
        realized_loss: Some(loss),
        unknown_realized_count: data.get("unknown_realized_count").and_then(Value::as_u64),
        exchange_stale: data.get("exchange_stale").and_then(Value::as_bool).unwrap_or(true),
        mark_gaps: data.get("paper_mark_gaps").and_then(Value::as_u64),
        equity: data.get("equity").and_then(Value::as_f64).filter(|v| v.is_finite()),
        curve: data.get("curve").and_then(Value::as_array).map(|rows| {
            rows.iter().filter_map(|row| Some(ResearchEquityPoint {
                ts: row.get("ts")?.as_i64()?,
                equity: row.get("equity")?.as_f64().filter(|v| v.is_finite())?,
            })).collect()
        }).unwrap_or_default(),
        trades: data.get("trades").and_then(Value::as_array).map(|rows| {
            rows.iter().filter_map(|row| Some(ResearchTrade {
                coin: row.get("coin")?.as_str()?.to_owned(),
                entry_at: row.get("entry_at").and_then(Value::as_i64),
                exit_at: row.get("exit_at").and_then(Value::as_i64),
                pnl: row.get("pnl").and_then(Value::as_f64).filter(|v| v.is_finite()),
                note: row.get("note").and_then(Value::as_str).map(str::to_owned),
                entry_leverage: row.get("entry_leverage")?.as_u64()?,
            })).take(100).collect()
        }).unwrap_or_default(),
        error: None,
    })
}

async fn read_signals(db: &PaperDb) -> Result<Json<Vec<SignalRow>>, StatusCode> {
    db.all_signals().await.map(Json).map_err(|error| {
        tracing::error!("paper signals read failed: {}", error);
        StatusCode::SERVICE_UNAVAILABLE
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn execution_summary_exposes_aggregates_without_account_or_trades() {
        let raw = json!({
            "mode":"paper", "account":"private-address", "as_of_ms":123,
            "last_scan_ms":100, "scan_filters":{"total":234,"eligible":2},
            "paused":false, "halted":null, "risk_limit":{"kind":"daily"},
            "active_positions":1,
            "trades":[{"coin":"TEST","entry_at":100,"exit_at":200,"pnl":2.5,"note":"24h","entry_leverage":3}],
            "curve":[{"ts":100,"equity":500.0}], "equity":500.0,
            "max_positions":5,
            "realized_profit":12.0, "realized_loss":-3.0,
            "unknown_realized_count":1
        });
        let output = serde_json::to_value(execution_summary(&raw).unwrap()).unwrap();
        assert_eq!(output["realized_profit"], 12.0);
        assert_eq!(output["risk_limit"], "daily");
        assert_eq!(output["active_positions"], 1);
        assert_eq!(output["max_positions"], 5);
        assert_eq!(output["trades"][0]["coin"], "TEST");
        assert_eq!(output["curve"][0]["equity"], 500.0);
        assert!(output.get("account").is_none());
        assert!(!output.to_string().contains("PRIVATE"));
        assert!(execution_summary(&json!({"mode":"live"})).is_none());
    }
}
