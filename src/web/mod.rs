//! HTTP API + shared state.

mod archive;
mod assets;
mod limitoco;
mod live;
mod ml;

pub use archive::run_tradfi_auto_rebalance;


use crate::hl::{CoinMeta, MarketCtx};
use crate::store::Store;
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::{compression::CompressionLayer, cors::CorsLayer};

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub live: Arc<Mutex<crate::live::LiveState>>,
    pub live_path: Arc<std::path::PathBuf>,
    pub meta: Arc<Mutex<MetaCache>>,
    pub refresh: Arc<Mutex<RefreshStatus>>,
    /// Shared HTTP client so Hyperliquid connections are pooled.
    pub http: reqwest::Client,
    /// 调仓/止盈的执行闸门。互斥锁只保护状态、不覆盖下单过程，所以自动调仓
    /// 和手动点击可能同时进入 —— 两笔调仓交错会下重复单、把仓位搞乱。
    pub exec_gate: Arc<tokio::sync::Mutex<()>>,
    /// Prevent duplicate manual/automatic jobs while their public-data preparation runs.
    pub run_gate: Arc<tokio::sync::Mutex<()>>,
    /// 资金费套利面板数据（每 60 秒后台刷新）。
    /// 币安公开接口客户端（可选 BINANCE_PROXY 代理，地区屏蔽时优雅降级）。
    /// 纸面资金费 carry 策略状态。
    /// 访问令牌（STARS_TOKEN ✗）。配置后所有 /api/* 需要 Bearer 鉴权。
    pub auth_token: Option<String>,
    /// 跨所价差纸面引擎（OKX vs 币安现货）。
    /// HL 单账户 carry 影子（现货 + 空永续）。
    pub hlcarry: Arc<Mutex<crate::hlcarry::HlCarryState>>,
    pub hlcarry_path: Arc<std::path::PathBuf>,
    /// 币安日内前向影子账本，由独立只读采集进程写入。
    pub intraday_path: Arc<std::path::PathBuf>,
    /// 币安 TradFi 双腿（美股 A4 + 金银油气 EMA5/20）前向影子账本。
    pub tradfi_path: Arc<std::path::PathBuf>,
    /// 币安核心 10 标的 1h 顺势 SFP 快速波段前向影子账本。
    pub fastswing_path: Arc<std::path::PathBuf>,
    /// 币安 USD-M A+B 纯限价 OCO 影子状态（纯 Rust 引擎）。
    pub limitoco: Arc<Mutex<crate::limitoco::LimitOcoState>>,
    pub limitoco_path: Arc<std::path::PathBuf>,
    pub limitoco_gate: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Clone, Default)]
pub struct MetaCache {
    pub universe: Vec<CoinMeta>,
    pub ctxs: Vec<MarketCtx>,
    pub liquid: Vec<String>,
    pub refreshed_at: i64,
    /// Hyperliquid base-tier fee schedule (cross = taker, add = maker).
    pub fee_taker: f64,
    pub fee_maker: f64,
}

#[derive(Clone, Default, serde::Serialize)]
pub struct RefreshStatus {
    pub phase: String,
    pub coins_done: usize,
    pub coins_total: usize,
    pub current: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(assets::index_html))
        .route("/css/tokens.css", get(assets::css_tokens))
        .route("/css/base.css", get(assets::css_base))
        .route("/css/components.css", get(assets::css_components))
        .route("/js/main.js", get(assets::js_main))
        .route("/js/core/api.js", get(assets::js_core_api))
        .route("/js/core/fmt.js", get(assets::js_core_fmt))
        .route("/js/core/chart.js", get(assets::js_core_chart))
        .route("/js/core/router.js", get(assets::js_core_router))
        .route("/js/views/overview.js", get(assets::js_view_overview))
        .route("/js/views/live.js", get(assets::js_view_live))
        .route("/js/views/limitoco.js", get(assets::js_view_limitoco))
        .route("/js/views/archive.js", get(assets::js_view_archive))
        .route("/api/health", get(health))
        .route("/api/status", get(status))
        .route("/api/live", get(live::live_status))
        .route("/api/strategy-health", get(live::strategy_health))
        .route("/api/ml/plan", get(ml::ml_plan))
        .route("/api/ml/execute", post(ml::ml_execute))
        .route("/api/ml/reset", post(ml::ml_reset))
        .route("/api/snapshots", get(live::snapshots_view))
        .route("/api/live/config", post(live::live_config))
        .route("/api/live/run", post(live::live_run))
        .route("/api/live/reset", post(live::live_reset))
        .route("/api/live/rebuild", post(live::live_rebuild))
        .route("/api/live/tp", post(live::live_tp))
        .route("/api/live/records/clear", post(live::live_records_clear))
        .route("/api/live/capital", post(live::live_capital))
        .route("/api/live/baseline", post(live::live_baseline))
        .route("/api/live/reset-start", post(live::live_reset_start))
        .route("/api/live/capital/time", post(live::live_capital_time))
        .route("/api/live/capital/delete", post(live::live_capital_delete))
        .route("/api/capital", post(live::capital_alloc))
        .route("/api/hlcarry", get(archive::hlcarry_status))
        .route("/api/hlcarry/open", post(archive::hlcarry_open))
        .route("/api/hlcarry/step", post(archive::hlcarry_step))
        .route("/api/hlcarry/params", post(archive::hlcarry_params))
        .route("/api/intraday", get(archive::intraday_status))
        .route("/api/tradfi", get(archive::tradfi_status))
        .route("/api/fastswing", get(archive::fastswing_status))
        .route("/api/tradfi/config", get(archive::tradfi_config_get).post(archive::tradfi_config_save))
        .route("/api/tradfi/shadow/rebalance", post(archive::tradfi_shadow_rebalance))
        .route("/api/tradfi/shadow/reset", post(archive::tradfi_shadow_reset))
        .route("/api/tradfi/plan", post(archive::tradfi_plan))
        .route("/api/tradfi/execute", post(archive::tradfi_execute))
        .route("/api/tradfi/flatten", post(archive::tradfi_flatten))
        .route("/api/limitoco", get(limitoco::limitoco_status))
        .route("/api/limitoco/config", get(limitoco::limitoco_config_get).post(limitoco::limitoco_config_save))
        .route("/api/limitoco/start", post(limitoco::limitoco_start))
        .route("/api/limitoco/stop", post(limitoco::limitoco_stop))
        .route("/api/limitoco/step", post(limitoco::limitoco_step))
        .route("/api/limitoco/reset", post(limitoco::limitoco_reset))
        .layer(axum::middleware::from_fn({
            let token = state.auth_token.clone();
            move |req: axum::http::Request<axum::body::Body>, next: axum::middleware::Next| {
                let token = token.clone();
                async move { require_auth(token, req, next).await }
            }
        }))
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// 鉴权中间件：配置了 STARS_TOKEN 时，所有 /api/*（除 health）都要 Bearer 令牌。
async fn require_auth(
    token: Option<String>,
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    let path = req.uri().path().to_string();
    let protected = path.starts_with("/api/") && path != "/api/health";
    let Some(expected) = token else {
        return next.run(req).await;
    };
    if !protected || expected.is_empty() {
        return next.run(req).await;
    }
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim() == expected.trim())
        .unwrap_or(false);
    if ok {
        next.run(req).await
    } else {
        (StatusCode::UNAUTHORIZED, Json(json!({"ok": false, "error": "未授权：请先登录（服务端 STARS_TOKEN）"}))).into_response()
    }
}


async fn health() -> &'static str {
    "ok"
}


async fn status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let meta = state.meta.lock().await;
    let cached = state.store.cached_coins().unwrap_or_default();
    let latest_ts = state.store.latest_ts().ok().flatten();
    let refresh = state.refresh.lock().await.clone();
    let oi_cov = state
        .store
        .oi_coverage()
        .map(|(d, c, f, l)| serde_json::json!({"days": d, "coins": c, "first": f, "last": l}))
        .unwrap_or(serde_json::Value::Null);

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
        "fee_taker": meta.fee_taker,
        "fee_maker": meta.fee_maker,
        "refresh": json!({
            "phase": refresh.phase,
            "coins_done": refresh.coins_done,
            "coins_total": refresh.coins_total,
            "current": refresh.current,
        }),
        "oi": oi_cov,
    }))
}


#[cfg(test)]
mod tests;
