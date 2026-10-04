//! HTTP API + shared state.

use crate::hl::{CoinMeta, MarketCtx};
use crate::momentum::{self, BacktestParams, HedgeMode, PanelEntry};
use crate::paper::{self, PaperConfig, PaperState};
use crate::store::Store;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, services::ServeDir};

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub paper: Arc<Mutex<PaperState>>,
    pub paper_path: Arc<std::path::PathBuf>,
    pub meta: Arc<Mutex<MetaCache>>,
    pub refresh: Arc<Mutex<RefreshStatus>>,
}

#[derive(Clone, Default)]
pub struct MetaCache {
    pub universe: Vec<CoinMeta>,
    pub ctxs: Vec<MarketCtx>,
    pub liquid: Vec<String>,
    pub refreshed_at: i64,
}

#[derive(Clone, Default)]
pub struct RefreshStatus {
    pub phase: String,
    pub coins_done: usize,
    pub coins_total: usize,
    pub current: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/status", get(status))
        .route("/api/backtest", post(backtest))
        .route("/api/paper", get(paper_status))
        .route("/api/paper/start", post(paper_start))
        .route("/api/paper/stop", post(paper_stop))
        .route("/api/paper/reset", post(paper_reset))
        .route("/api/paper/step", post(paper_step))
        .fallback_service(ServeDir::new("static"))
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

async fn status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let meta = state.meta.lock().await;
    let cached = state.store.cached_coins().unwrap_or_default();
    let latest_ts = state.store.latest_ts().ok().flatten();
    let paper = paper::snapshot(&*state.paper.lock().await);
    let refresh = state.refresh.lock().await.clone();

    Json(json!({
        "universe_total": meta.universe.len(),
        "universe_liquid": meta.liquid.len(),
        "liquid_coins": meta.liquid,
        "ctxs": meta.ctxs.iter().map(|c| json!({
            "coin": c.coin,
            "funding_8h_pct": c.funding * 8.0 * 100.0,
            "price": c.oracle_px,
            "prior_24h_pct": c.prior_24h_pct(),
            "day_vol_usd": c.day_ntl_vlm,
        })).collect::<Vec<_>>(),
        "cached_coins": cached.len(),
        "latest_ts": latest_ts,
        "refreshed_at": meta.refreshed_at,
        "refresh": json!({
            "phase": refresh.phase,
            "coins_done": refresh.coins_done,
            "coins_total": refresh.coins_total,
            "current": refresh.current,
        }),
        "paper": paper,
    }))
}

#[derive(serde::Deserialize)]
struct BacktestReq {
    #[serde(default = "d14")]
    lookback: usize,
    #[serde(default = "d020")]
    top_frac: f64,
    #[serde(default = "d5m")]
    min_vol_usd: f64,
    #[serde(default = "d_ew")]
    hedge: String,
}

fn d14() -> usize { 14 }
fn d020() -> f64 { 0.2 }
fn d5m() -> f64 { 5_000_000.0 }
fn d_ew() -> String { "equal_weight".into() }

async fn backtest(
    State(state): State<AppState>,
    Json(req): Json<BacktestReq>,
) -> Response {
    let hedge = match req.hedge.as_str() {
        "long_short" => HedgeMode::LongShort,
        _ => HedgeMode::EqualWeight,
    };
    let params = BacktestParams {
        lookback: req.lookback.max(1),
        top_frac: req.top_frac.clamp(0.05, 0.5),
        min_vol_usd: req.min_vol_usd.max(0.0),
        hedge,
    };

    let panel: Vec<PanelEntry> = state
        .store
        .all_panels()
        .unwrap_or_default()
        .into_iter()
        .map(|(coin, candles)| PanelEntry { coin, candles })
        .collect();
    if panel.len() < 10 {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": "缓存数据不足，请等待数据回填完成"})),
        )
            .into_response();
    }

    let result = momentum::run(&panel, &params);

    Json(json!(result)).into_response()
}

async fn paper_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!(paper::snapshot(&*state.paper.lock().await)))
}

#[derive(serde::Deserialize)]
struct PaperStartReq {
    #[serde(default = "d14")]
    lookback: usize,
    #[serde(default = "d020")]
    top_frac: f64,
    #[serde(default = "d5m")]
    min_vol_usd: f64,
    #[serde(default = "d_fee")]
    fee: f64,
    #[serde(default = "d_cap")]
    capital: f64,
}

fn d_fee() -> f64 { 0.00045 }
fn d_cap() -> f64 { 10_000.0 }

async fn paper_start(
    State(state): State<AppState>,
    Json(req): Json<PaperStartReq>,
) -> Response {
    let mut p = state.paper.lock().await;
    if p.running {
        return (StatusCode::CONFLICT, Json(json!({"error": "纸交易已在运行"}))).into_response();
    }
    p.config = Some(PaperConfig {
        lookback: req.lookback.max(1),
        top_frac: req.top_frac.clamp(0.05, 0.5),
        min_vol_usd: req.min_vol_usd.max(0.0),
        fee: req.fee,
        capital: req.capital.max(1.0),
    });
    p.running = true;
    p.equity = p.config.as_ref().unwrap().capital;
    p.benchmark = p.config.as_ref().unwrap().capital;
    p.positions.clear();
    p.history.clear();
    p.last_ts = None;
    p.days_elapsed = 0;
    p.total_cost = 0.0;
    p.started_at = None;
    let _ = p.save(&state.paper_path);
    Json(json!({"ok": true, "config": p.config})).into_response()
}

async fn paper_stop(State(state): State<AppState>) -> Response {
    let mut p = state.paper.lock().await;
    p.running = false;
    let _ = p.save(&state.paper_path);
    Json(json!({"ok": true})).into_response()
}

async fn paper_reset(State(state): State<AppState>) -> Response {
    let mut p = state.paper.lock().await;
    *p = PaperState::default();
    let _ = p.save(&state.paper_path);
    Json(json!({"ok": true})).into_response()
}

async fn paper_step(State(state): State<AppState>) -> Response {
    let mut p = state.paper.lock().await;
    if !p.running {
        return (StatusCode::CONFLICT, Json(json!({"error": "纸交易未启动"}))).into_response();
    }
    let panel: Vec<PanelEntry> = state
        .store
        .all_panels()
        .unwrap_or_default()
        .into_iter()
        .map(|(coin, candles)| PanelEntry { coin, candles })
        .collect();
    paper::step(&mut p, &panel);
    let _ = p.save(&state.paper_path);
    Json(json!(paper::snapshot(&*p))).into_response()
}
