use axum::{extract::State, routing::get, Json, Router};
use std::sync::Arc;
use tower_http::{cors::CorsLayer, services::ServeDir};

use crate::paper::{PaperDb, SignalRow};
use crate::state::SharedState;
use crate::types::MarketSnapshot;

#[derive(Clone)]
pub struct WebState {
    pub app: SharedState,
    pub paper: Arc<PaperDb>,
}

pub fn router(state: WebState) -> Router {
    Router::new()
        .route("/api/snapshot", get(get_snapshot))
        .route("/api/paper/signals", get(get_paper_signals))
        .route("/api/health", get(health))
        .fallback_service(ServeDir::new("static"))
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

async fn get_paper_signals(State(state): State<WebState>) -> Json<Vec<SignalRow>> {
    match state.paper.all_signals().await {
        Ok(rows) => Json(rows),
        Err(_) => Json(vec![]),
    }
}
