use axum::{extract::State, routing::get, Json, Router};
use tower_http::{cors::CorsLayer, services::ServeDir};

use crate::state::SharedState;
use crate::types::MarketSnapshot;

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/snapshot", get(get_snapshot))
        .route("/api/health", get(health))
        .fallback_service(ServeDir::new("static"))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

async fn get_snapshot(State(state): State<SharedState>) -> Json<Vec<MarketSnapshot>> {
    let map = state.snapshots.read().await;
    let mut list: Vec<MarketSnapshot> = map.values().cloned().collect();

    // 默认按成交额降序，前端可再排序
    list.sort_by(|a, b| b.volume_quote_24h.cmp(&a.volume_quote_24h));

    Json(list)
}
