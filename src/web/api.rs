use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
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
}

pub fn router(state: WebState) -> Router {
    Router::new()
        .route("/api/snapshot", get(get_snapshot))
        .route("/api/paper/signals", get(get_okx_signals))
        .route("/api/binance/signals", get(get_binance_signals))
        .route("/api/hyperliquid/signals", get(get_hl_signals))
        .route("/api/research/funding/latest", get(get_latest_funding))
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
