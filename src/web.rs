//! HTTP API + shared state.

use crate::hl::{CoinMeta, MarketCtx};
use crate::momentum::PanelEntry;
use crate::paper::{self, PaperConfig, PaperState};
use crate::store::Store;
use axum::{
    extract::{Query, State},
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
    pub paper: Arc<Mutex<PaperState>>,
    pub paper_path: Arc<std::path::PathBuf>,
    /// 影子组合（动量 + 距高点 + 上涨占比 三因子）纸面引擎。
    pub paper2: Arc<Mutex<PaperState>>,
    pub paper2_path: Arc<std::path::PathBuf>,
    /// 第三个纸面组合：纯动量 + 波动目标（用来单独验证波动目标）
    pub paper3: Arc<Mutex<PaperState>>,
    pub paper3_path: Arc<std::path::PathBuf>,
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
    /// 组合策略纸面（日线动量 + 12h 区间位置）。
    pub combine: Arc<Mutex<crate::combine::CombineState>>,
    pub combine_path: Arc<std::path::PathBuf>,
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
        .route("/", get(index_html))
        .route("/app.js", get(app_js))
        .route("/api/health", get(health))
        .route("/api/status", get(status))
        .route("/api/paper", get(paper_status))
        .route("/api/paper/start", post(paper_start))
        .route("/api/paper/stop", post(paper_stop))
        .route("/api/paper/reset", post(paper_reset))
        .route("/api/paper/step", post(paper_step))
        .route("/api/paper2", get(paper2_status))
        .route("/api/paper2/start", post(paper2_start))
        .route("/api/paper2/stop", post(paper2_stop))
        .route("/api/paper2/reset", post(paper2_reset))
        .route("/api/paper2/step", post(paper2_step))
        .route("/api/paper3", get(paper3_status))
        .route("/api/paper3/start", post(paper3_start))
        .route("/api/paper3/stop", post(paper3_stop))
        .route("/api/paper3/reset", post(paper3_reset))
        .route("/api/paper3/step", post(paper3_step))
        .route("/api/live", get(live_status))
        .route("/api/strategy-health", get(strategy_health))
        .route("/api/paper4", get(paper4_status))
        .route("/api/paper4/start", post(paper4_start))
        .route("/api/paper4/stop", post(paper4_stop))
        .route("/api/paper4/reset", post(paper4_reset))
        .route("/api/snapshots", get(snapshots_view))
        .route("/api/live/config", post(live_config))
        .route("/api/live/run", post(live_run))
        .route("/api/live/reset", post(live_reset))
        .route("/api/live/rebuild", post(live_rebuild))
        .route("/api/live/tp", post(live_tp))
        .route("/api/live/records/clear", post(live_records_clear))
        .route("/api/live/capital", post(live_capital))
        .route("/api/live/baseline", post(live_baseline))
        .route("/api/live/reset-start", post(live_reset_start))
        .route("/api/live/capital/time", post(live_capital_time))
        .route("/api/live/capital/delete", post(live_capital_delete))
        .route("/api/capital", post(capital_alloc))
        .route("/api/hlcarry", get(hlcarry_status))
        .route("/api/hlcarry/open", post(hlcarry_open))
        .route("/api/hlcarry/step", post(hlcarry_step))
        .route("/api/hlcarry/params", post(hlcarry_params))
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

const INDEX_HTML: &str = include_str!("../static/index.html");
const APP_JS: &str = include_str!("../static/app.js");

async fn index_html() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache, no-store, must-revalidate"),
        ],
        INDEX_HTML,
    )
}

async fn app_js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache, no-store, must-revalidate"),
        ],
        APP_JS,
    )
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
        "paper": paper,
        "oi": oi_cov,
    }))
}

fn d14() -> usize { 14 }
fn d020() -> f64 { 0.2 }
fn d5m() -> f64 { 5_000_000.0 }

async fn paper_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!(paper::snapshot(&*state.paper.lock().await)))
}

#[derive(serde::Deserialize)]
struct PaperStartReq {
    #[serde(default)]
    vol_target: Option<f64>,
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
    #[serde(default = "d_lev")]
    leverage: f64,
    #[serde(default = "d_replay")]
    replay_days: usize,
    #[serde(default = "d_target_pos")]
    target_positions: usize,
    #[serde(default = "d_mw")]
    momentum_weight: f64,
    #[serde(default)]
    dd_weight: f64,
    #[serde(default)]
    upfrac_weight: f64,
    #[serde(default)]
    slippage: Option<f64>,
}

fn d_mw() -> f64 { 0.5 }
fn d_fee() -> f64 { 0.00045 }
fn d_cap() -> f64 { 2_000.0 }
fn d_lev() -> f64 { 3.0 }
fn d_replay() -> usize { 90 }
fn d_target_pos() -> usize { 8 }

async fn paper_start(
    State(state): State<AppState>,
    Json(req): Json<PaperStartReq>,
) -> Response {
    let mut p = state.paper.lock().await;
    if p.running {
        return (StatusCode::CONFLICT, Json(json!({"error": "纸交易已在运行"}))).into_response();
    }
    // Default the fee to the live Hyperliquid taker rate when the caller does
    // not override it.
    let fee = if req.fee > 0.0 {
        req.fee
    } else {
        let cached = state.meta.lock().await.fee_taker;
        if cached > 0.0 { cached } else { d_fee() }
    };
    p.config = Some(PaperConfig {
        vol_target: req.vol_target.unwrap_or(0.0).clamp(0.0, 0.10),
        lookback: req.lookback.max(1),
        top_frac: req.top_frac.clamp(0.05, 0.5),
        min_vol_usd: req.min_vol_usd.max(0.0),
        fee,
        slippage: req.slippage.unwrap_or(0.0003).clamp(0.0, 0.01),
        capital: req.capital.max(1.0),
        leverage: req.leverage.clamp(1.0, 20.0),
        replay_days: req.replay_days.min(365),
        target_positions: req.target_positions.clamp(1, 30),
        momentum_weight: req.momentum_weight.clamp(0.0, 1.0),
        dd_weight: req.dd_weight.max(0.0),
        upfrac_weight: req.upfrac_weight.max(0.0),
    });
    p.running = true;
    let capital = p.config.as_ref().unwrap().capital;
    p.equity = capital;
    p.market = 1.0;
    p.positions.clear();
    p.trades.clear();
    p.history.clear();
    p.last_ts = None;
    p.days_elapsed = 0;
    p.total_cost = 0.0;
    p.started_at = None;
    let _ = p.save(&state.paper_path);
    drop(p);

    // Run the initial replay immediately so the book, trade log and equity
    // curve are populated the moment the user presses start.
    let snapshot = run_paper_step(&state).await;
    match snapshot {
        Some(v) => Json(json!({"ok": true, "config": v["config"], "paper": v})).into_response(),
        None => Json(json!({"ok": true})).into_response(),
    }
}

/// Load the candle panel + per-coin max leverage and advance the paper trade.
async fn run_paper_step(state: &AppState) -> Option<serde_json::Value> {
    run_paper_step_which(state, 0).await
}

/// which: 0 = paper（对照）✗ 1 = paper2（影子）✗ 2 = paper3（波动目标）
async fn run_paper_step_which(state: &AppState, which: u8) -> Option<serde_json::Value> {
    let panel: Vec<PanelEntry> = state
        .store
        .all_panels()
        .unwrap_or_default()
        .into_iter()
        .map(|(coin, candles)| PanelEntry { coin, candles })
        .collect();
    let max_lev: std::collections::HashMap<String, u32> = {
        let meta = state.meta.lock().await;
        meta.universe
            .iter()
            .map(|c| (c.name.clone(), c.max_leverage))
            .collect()
    };
    let (p, path) = match which {
        1 => (state.paper2.clone(), state.paper2_path.clone()),
        2 => (state.paper3.clone(), state.paper3_path.clone()),
        _ => (state.paper.clone(), state.paper_path.clone()),
    };
    let mut p = p.lock().await;
    paper::step(&mut p, &panel, &max_lev);
    let _ = p.save(&path);
    Some(json!(paper::snapshot(&*p)))
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
    if !state.paper.lock().await.running {
        return (StatusCode::CONFLICT, Json(json!({"error": "纸交易未启动"}))).into_response();
    }
    match run_paper_step(&state).await {
        Some(v) => Json(v).into_response(),
        None => (StatusCode::CONFLICT, Json(json!({"error": "数据不足"}))).into_response(),
    }
}

// ==================== paper2 (影子组合) ====================

async fn paper2_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!(paper::snapshot(&*state.paper2.lock().await)))
}

async fn paper2_start(State(state): State<AppState>, Json(req): Json<PaperStartReq>) -> Response {
    let mut p = state.paper2.lock().await;
    // 「应用并重启」：正在运行时直接重置重来，而不是返回 409 把用户挡在外面。
    if p.running {
        p.running = false;
    }
    let fee = if req.fee > 0.0 {
        req.fee
    } else {
        let cached = state.meta.lock().await.fee_taker;
        if cached > 0.0 { cached } else { d_fee() }
    };
    p.config = Some(PaperConfig {
        vol_target: req.vol_target.unwrap_or(0.0).clamp(0.0, 0.10),
        lookback: req.lookback.max(1),
        top_frac: req.top_frac.clamp(0.05, 0.5),
        min_vol_usd: req.min_vol_usd.max(0.0),
        fee,
        slippage: req.slippage.unwrap_or(0.0003).clamp(0.0, 0.01),
        capital: req.capital.max(1.0),
        leverage: req.leverage.clamp(1.0, 20.0),
        replay_days: req.replay_days.min(365),
        target_positions: req.target_positions.clamp(1, 30),
        momentum_weight: req.momentum_weight.clamp(0.0, 1.0),
        dd_weight: req.dd_weight.max(0.0),
        upfrac_weight: req.upfrac_weight.max(0.0),
    });
    p.running = true;
    let capital = p.config.as_ref().unwrap().capital;
    p.equity = capital;
    p.market = 1.0;
    p.positions.clear();
    p.trades.clear();
    p.history.clear();
    p.last_ts = None;
    p.days_elapsed = 0;
    p.total_cost = 0.0;
    p.started_at = None;
    let _ = p.save(&state.paper2_path);
    drop(p);
    let snapshot = run_paper_step_which(&state, 1).await;
    match snapshot {
        Some(v) => Json(json!({"ok": true, "config": v["config"], "paper": v})).into_response(),
        None => Json(json!({"ok": true})).into_response(),
    }
}

async fn paper2_stop(State(state): State<AppState>) -> Response {
    let mut p = state.paper2.lock().await;
    p.running = false;
    let _ = p.save(&state.paper2_path);
    Json(json!({"ok": true})).into_response()
}

async fn paper2_reset(State(state): State<AppState>) -> Response {
    let mut p = state.paper2.lock().await;
    *p = PaperState::default();
    let _ = p.save(&state.paper2_path);
    Json(json!({"ok": true})).into_response()
}

async fn paper2_step(State(state): State<AppState>) -> Response {
    if !state.paper2.lock().await.running {
        return (StatusCode::CONFLICT, Json(json!({"error": "影子组合未启动"}))).into_response();
    }
    match run_paper_step_which(&state, 1).await {
        Some(v) => Json(v).into_response(),
        None => (StatusCode::CONFLICT, Json(json!({"error": "数据不足"}))).into_response(),
    }
}
// ==================== paper3 (波动目标对照) ====================

async fn paper3_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!(paper::snapshot(&*state.paper3.lock().await)))
}

async fn paper3_start(State(state): State<AppState>, Json(req): Json<PaperStartReq>) -> Response {
    let mut p = state.paper3.lock().await;
    // 「应用并重启」：正在运行时直接重置重来，而不是返回 409 把用户挡在外面。
    if p.running {
        p.running = false;
    }
    let fee = if req.fee > 0.0 {
        req.fee
    } else {
        let cached = state.meta.lock().await.fee_taker;
        if cached > 0.0 { cached } else { d_fee() }
    };
    p.config = Some(PaperConfig {
        vol_target: req.vol_target.unwrap_or(0.0).clamp(0.0, 0.10),
        lookback: req.lookback.max(1),
        top_frac: req.top_frac.clamp(0.05, 0.5),
        min_vol_usd: req.min_vol_usd.max(0.0),
        fee,
        slippage: req.slippage.unwrap_or(0.0003).clamp(0.0, 0.01),
        capital: req.capital.max(1.0),
        leverage: req.leverage.clamp(1.0, 20.0),
        replay_days: req.replay_days.min(365),
        target_positions: req.target_positions.clamp(1, 30),
        momentum_weight: req.momentum_weight.clamp(0.0, 1.0),
        dd_weight: req.dd_weight.max(0.0),
        upfrac_weight: req.upfrac_weight.max(0.0),
    });
    p.running = true;
    let capital = p.config.as_ref().unwrap().capital;
    p.equity = capital;
    p.market = 1.0;
    p.positions.clear();
    p.trades.clear();
    p.history.clear();
    p.last_ts = None;
    p.days_elapsed = 0;
    p.total_cost = 0.0;
    p.started_at = None;
    let _ = p.save(&state.paper3_path);
    drop(p);
    let snapshot = run_paper_step_which(&state, 2).await;
    match snapshot {
        Some(v) => Json(json!({"ok": true, "config": v["config"], "paper": v})).into_response(),
        None => Json(json!({"ok": true})).into_response(),
    }
}

async fn paper3_stop(State(state): State<AppState>) -> Response {
    let mut p = state.paper3.lock().await;
    p.running = false;
    let _ = p.save(&state.paper3_path);
    Json(json!({"ok": true})).into_response()
}

async fn paper3_reset(State(state): State<AppState>) -> Response {
    let mut p = state.paper3.lock().await;
    *p = PaperState::default();
    let _ = p.save(&state.paper3_path);
    Json(json!({"ok": true})).into_response()
}

async fn paper3_step(State(state): State<AppState>) -> Response {
    if !state.paper3.lock().await.running {
        return (StatusCode::CONFLICT, Json(json!({"error": "波动目标对照未启动"}))).into_response();
    }
    match run_paper_step_which(&state, 2).await {
        Some(v) => Json(v).into_response(),
        None => (StatusCode::CONFLICT, Json(json!({"error": "数据不足"}))).into_response(),
    }
}

// ==================== live trading ====================

async fn live_markets(state: &AppState) -> std::collections::HashMap<String, crate::exchange::MarketInfo> {
    let meta = state.meta.lock().await;
    crate::live::markets_from_meta(&meta.universe)
}

async fn snapshots_view(State(state): State<AppState>) -> Json<serde_json::Value> {
    let store = &state.store;
    let oi = store.oi_coverage().unwrap_or((0, 0, None, None));
    let dep = store.depth_coverage().unwrap_or((0, 0, None, None));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let today = now / 86_400_000 * 86_400_000;
    let today_rows = store.oi_day(today).unwrap_or_default();
    Json(json!({
        "oi": {"days": oi.0, "coins": oi.1, "first": oi.2, "last": oi.3},
        "depth": {"days": dep.0, "coins": dep.1, "first": dep.2, "last": dep.3},
        "today": today_rows.iter().take(12).map(|(c, oi, px, f)| json!({"coin": c, "oi_usd": oi, "mark_px": px, "funding": f})).collect::<Vec<_>>(),
        "today_count": today_rows.len(),
    }))
}

async fn paper4_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    {
        // 首次打开（还没历史 ✗ 也没仓位）✗ 现场算一次当前仓位给出页面看 ✓
        let mut c = state.combine.lock().await;
        if c.history.len() < 2 && c.a_pos.is_empty() && c.b_pos.is_empty() {
            let d: Vec<PanelEntry> = state
                .store
                .all_panels()
                .unwrap_or_default()
                .into_iter()
                .map(|(coin, candles)| PanelEntry { coin, candles })
                .collect();
            let h: Vec<PanelEntry> = state
                .store
                .all_panels_1h()
                .unwrap_or_default()
                .into_iter()
                .map(|(coin, candles)| PanelEntry { coin, candles })
                .collect();
            if !d.is_empty() {
                let mut marks: std::collections::HashMap<String, f64> = {
                    let m = state.meta.lock().await;
                    m.ctxs
                        .iter()
                        .filter(|x| x.mark_px > 0.0)
                        .map(|x| (x.coin.clone(), x.mark_px))
                        .collect()
                };
                // meta 还没刷新（启动时 backfill 会拖住它 ✗）✗ 自己拉一次现价 ✓
                if marks.is_empty() {
                    if let Ok(ctxs) = crate::hl::HlClient::new().market_ctxs().await {
                        marks = ctxs
                            .into_iter()
                            .filter(|x| x.mark_px > 0.0)
                            .map(|x| (x.coin, x.mark_px))
                            .collect();
                    }
                }
                // 拿不到实时价就【先不建仓】✗ 免得用过期价建出一个假浮盈 ✓
                if !marks.is_empty() {
                    crate::combine::preview(&mut c, &d, &h, &marks);
                    c.save(state.combine_path.as_ref());
                }
            }
        } else {
            // 已有仓位：优先用【实时标记价】刷浮盈（meta 每 ~60 秒刷新 ✓）
            let marks: std::collections::HashMap<String, f64> = {
                let m = state.meta.lock().await;
                m.ctxs
                    .iter()
                    .filter(|x| x.mark_px > 0.0)
                    .map(|x| (x.coin.clone(), x.mark_px))
                    .collect()
            };
            crate::combine::mark_with_prices(&mut c, &marks);
        }
    }
    let snap = state.combine.lock().await.snapshot();
    Json(json!({
        "equity": snap.equity,
        "pnl_pct": snap.pnl_pct,
        "days_elapsed": snap.days_elapsed,
        "history": snap.history,
        "positions": snap.positions,
        "running": snap.running,
        "last_a": snap.last_a,
        "last_b": snap.last_b,
        "unrealized": snap.unrealized,
        "a_pos": snap.a_pos.iter().map(|p| json!({
            "coin": p.coin, "side": p.side, "entry_px": p.entry_px,
            "notional": p.notional, "mark_px": p.mark_px, "unrealized": p.unrealized(),
            "entry_ts": p.entry_ts,
        })).collect::<Vec<_>>(),
        "b_pos": snap.b_pos.iter().map(|p| json!({
            "coin": p.coin, "side": p.side, "entry_px": p.entry_px,
            "notional": p.notional, "mark_px": p.mark_px, "unrealized": p.unrealized(),
            "entry_ts": p.entry_ts,
        })).collect::<Vec<_>>(),
        "config": snap.config,
    }))
}

async fn paper4_start(State(state): State<AppState>) -> Response {
    let mut c = state.combine.lock().await;
    c.running = true;
    let _ = std::fs::write(state.combine_path.as_ref(), serde_json::to_vec(&*c).unwrap_or_default());
    Json(json!({"ok": true})).into_response()
}

async fn paper4_stop(State(state): State<AppState>) -> Response {
    let mut c = state.combine.lock().await;
    c.running = false;
    let _ = std::fs::write(state.combine_path.as_ref(), serde_json::to_vec(&*c).unwrap_or_default());
    Json(json!({"ok": true})).into_response()
}

async fn paper4_reset(State(state): State<AppState>) -> Response {
    let cap = state.combine.lock().await.config.as_ref().map(|c| c.capital).unwrap_or(5000.0);
    let mut c = state.combine.lock().await;
    // 重置：先删文件 ✗ 再用固定起点重建 ✓
    let _ = std::fs::remove_file(state.combine_path.as_ref());
    *c = crate::combine::CombineState::load_or_init(state.combine_path.as_ref(), cap);
    let _ = std::fs::write(state.combine_path.as_ref(), serde_json::to_vec(&*c).unwrap_or_default());
    Json(json!({"ok": true})).into_response()
}

async fn strategy_health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let panel: Vec<PanelEntry> = state
        .store
        .all_panels()
        .unwrap_or_default()
        .into_iter()
        .map(|(coin, candles)| PanelEntry { coin, candles })
        .collect();
    let hist: Vec<(i64, f64)> = {
        let st = state.live.lock().await;
        // 用扣除出入金后的序列 ✗ 否则入金会被当成一段上涨 ✓
        crate::live::adjusted_history(&st)
    };
    let h = crate::health::compute(&panel, &hist);
    Json(serde_json::json!(h))
}

async fn live_status(State(state): State<AppState>) -> Response {
    let mut st = state.live.lock().await.clone();
    let markets = live_markets(&state).await;
    // Read-only receipts: an exchange fill does not establish take-profit intent.
    if !st.config.account.is_empty() {
        let now = crate::live::now_ms_pub();
        if now - st.last_reconcile_ms > 300_000 {
            st.last_reconcile_ms = now;
            if let Ok(exec) = crate::exchange::Exec::reader_config(&st.config).await {
                match crate::live::reconcile_fills(&exec, &mut st).await {
                    Ok(n) => {
                        if n > 0 { tracing::info!("对账：补入 {n} 笔交易所成交回执"); }
                        let mut current = state.live.lock().await;
                        let previous = current.clone();
                        crate::live::merge_reconciliation(&mut current, &st);
                        if let Err(e) = current.save(&state.live_path) {
                            *current = previous;
                            tracing::warn!("成交对账保存失败: {e}");
                        }
                    }
                    Err(e) => tracing::warn!("对账失败: {e}"),
                }
            }
        }
    }
    let mut snap = crate::live::snapshot(&st, &markets, Some(state.http.clone())).await;
    // 影子回测：从实盘第一天起，用同一套信号和成本假设重算一遍，供页面并排对照。
    // 只处理实盘开始之后的日期，所以很快。
    let shadow = st
        .history
        .first()
        .map(|p| crate::live::shadow_curve(&state.store, p.ts, &st.config))
        .unwrap_or_default();
    // 页面访问时也补一个点（方便刚打开就能看到），但判重和写入都在锁内完成 ——
    // 原来的写法用进入 handler 时的克隆判断，并发请求会写出重复时间戳。
    // 后台任务才是主要的记录者，见 main.rs 的 record_equity 定时器。
    if snap.error.is_none() {
        if let Err(e) = crate::live::record_equity(&state.live, &state.live_path, &snap.account, snap.equity, snap.cumulative_pnl).await {
            tracing::warn!("账户净值保存失败: {e:#}");
        }
        let current = state.live.lock().await;
        if current.config.account.eq_ignore_ascii_case(&snap.account) {
            snap.capital_baseline = current.baseline();
            snap.start_equity = snap.capital_baseline.as_ref().map(|b| b.equity).unwrap_or(0.);
            snap.history = current.history.clone();
        }
    }
    snap.shadow = shadow;
    // 五项健康指标（红黄绿）✗ 需要 IC/回撤 ✗ 从同一份面板和调整后净值算 ✓
    let scorecard = {
        let panel: Vec<PanelEntry> = state
            .store
            .all_panels()
            .unwrap_or_default()
            .into_iter()
            .map(|(coin, candles)| PanelEntry { coin, candles })
            .collect();
        if panel.is_empty() {
            Vec::new()
        } else {
            let st2 = state.live.lock().await;
            let hist = crate::live::adjusted_history(&st2);
            let h = crate::health::compute(&panel, &hist);
            crate::live::scorecard(&st2, &h)
        }
    };
    // 当前组合预估日波动（已含杠杆）✗ 就是波动目标看的那个数 ✓
    let vol_now = {
        let panel: Vec<PanelEntry> = state
            .store
            .all_panels()
            .unwrap_or_default()
            .into_iter()
            .map(|(coin, candles)| PanelEntry { coin, candles })
            .collect();
        let positions: Vec<(String, f64)> = snap
            .positions
            .iter()
            .filter(|p| p.notional.abs() > 0.0)
            .map(|p| {
                let w = if p.side == "long" { p.notional } else { -p.notional };
                (p.coin.clone(), w / snap.equity.max(1e-9))
            })
            .collect();
        crate::live::current_book_vol(&panel, &positions)
    };
    let mut v = json!(snap);
    if let Some(obj) = v.as_object_mut() {
        obj.insert("scorecard".into(), serde_json::Value::Array(scorecard));
        obj.insert(
            "vol_now".into(),
            serde_json::json!({
                "pct": vol_now.map(|v| v * 100.0),
                "target_pct": snap.config.vol_target * 100.0,
            }),
        );
    }
    Json(v).into_response()
}

#[derive(serde::Deserialize)]
struct CapitalBody {
    amount: f64,
    effective_ts: Option<i64>,
    note: Option<String>,
}

#[derive(serde::Deserialize)]
struct LiveConfigBody {
    vol_target: Option<f64>,
    vol_band: Option<f64>,
    position_stop_pct: Option<f64>,
    stop_cooldown_days: Option<i64>,
    account: Option<String>,
    key_path: Option<String>,
    target_positions: Option<usize>,
    leverage: Option<f64>,
    margin_buffer: Option<f64>,
    slippage: Option<f64>,
    lookback: Option<usize>,
    top_frac: Option<f64>,
    min_vol_usd: Option<f64>,
    min_order_pct: Option<f64>,
    take_profit_pct: Option<f64>,
    rebalance_slices: Option<u32>,
    momentum_weight: Option<f64>,

    armed: Option<bool>,
    auto_run: Option<bool>,
}

async fn live_config(
    State(state): State<AppState>,
    Json(body): Json<LiveConfigBody>,
) -> Response {
    let _gate = match state.exec_gate.try_lock() {
        Ok(g) => g,
        Err(_) => return (StatusCode::CONFLICT, Json(json!({"ok":false,"error":"订单操作正在执行，请稍后保存配置"}))).into_response(),
    };
    let mut st = state.live.lock().await;
    let previous = st.clone();
    let c = &mut st.config;
    if let Some(v) = body.account {
        c.account = v.trim().to_string();
    }
    if let Some(v) = body.key_path {
        c.key_path = v.trim().to_string();
    }
    if let Some(v) = body.target_positions {
        c.target_positions = v.clamp(1, 30);
    }
    if let Some(v) = body.leverage {
        c.leverage = v.clamp(1.0, 20.0);
    }
    if let Some(v) = body.margin_buffer {
        c.margin_buffer = v.clamp(0.3, 1.0);
    }
    if let Some(v) = body.slippage {
        c.slippage = v.clamp(0.0005, 0.05);
    }
    if let Some(v) = body.lookback {
        c.lookback = v.clamp(2, 120);
    }
    if let Some(v) = body.top_frac {
        // 与纸交易用同一区间。之前实盘允许到 0.9、纸交易只到 0.5，
        // 同一个参数在两个引擎里含义不同，无法互相验证。
        c.top_frac = v.clamp(0.02, 0.5);
    }
    if let Some(v) = body.min_vol_usd {
        c.min_vol_usd = v.max(0.0);
    }
    if let Some(v) = body.min_order_pct {
        // 上限 5%：再高就等于"一个仓位动 5% 才调"，那已经不是这个策略了。
        c.min_order_pct = v.clamp(0.0, 0.05);
    }
    if let Some(v) = body.rebalance_slices {
        c.rebalance_slices = v.clamp(1, 10);
    }
    if let Some(v) = body.take_profit_pct {
        c.take_profit_pct = v.clamp(0.0, 0.5);
    }
    if let Some(v) = body.momentum_weight {
        // 动量占比 0~1（量能占其余）。0.7 = 动量七成。
        c.momentum_weight = v.clamp(0.0, 1.0);
    }
    if let Some(v) = body.vol_band {
        c.vol_band = v.clamp(0.0, 0.6);
    }
    if let Some(v) = body.vol_target {
        if !(0.0..=0.10).contains(&v) {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "目标日波动必须在 0~10%（0 = 关闭）"}))).into_response();
        }
        c.vol_target = v;
    }
    if let Some(v) = body.position_stop_pct {
        if !(0.0..=0.9).contains(&v) {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "单币止损必须在 0~90%（0 = 关闭）"}))).into_response();
        }
        c.position_stop_pct = v;
    }
    if let Some(v) = body.stop_cooldown_days {
        if !(0..=60).contains(&v) {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "冷却天数必须在 0~60"}))).into_response();
        }
        c.stop_cooldown_days = v;
    }
    if let Some(v) = body.armed {
        c.armed = v;
    }
    if let Some(v) = body.auto_run {
        c.auto_run = v;
    }
    st.reset_capital_for_account_change(&previous.config.account);
    let cfg = st.config.clone();
    if let Err(e) = st.save(&state.live_path) {
        *st = previous;
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok":false,"error":format!("保存配置失败: {e}")}))).into_response();
    }
    Json(json!({"ok": true, "config": cfg})).into_response()
}

#[derive(serde::Deserialize, Default)]
struct LiveRunBody {
    #[serde(default)]
    live: bool,
}

async fn live_run(State(state): State<AppState>, body: Option<Json<LiveRunBody>>) -> Response {
    let live = body.map(|b| b.live).unwrap_or(false);
    // 交易执行必须在「脱离 HTTP 请求」的后台任务里跑。
    //
    // 之前是在 handler 里同步执行：29 笔 IOC 下单要十几到几十秒，一旦浏览器
    // 或 nginx 断开连接，axum 会直接丢弃这个 future —— 执行到一半就没了，
    // 仓位建了一半、组合失去对冲，而且前端只看到一个空响应。这个坑真实发生过。
    //
    // 现在立刻返回「已开始」，执行结果写进 LiveState，页面轮询取回。
    let job = match state.run_gate.clone().try_lock_owned() {
        Ok(g) => g,
        Err(_) => {
            return Json(json!({
                "ok": false,
                "error": "上一次调仓还在执行中，请稍候再试"
            }))
            .into_response()
        }
    };

    let execution = match state.exec_gate.clone().try_lock_owned() {
        Ok(g) => g,
        Err(_) => return (StatusCode::CONFLICT, Json(json!({"ok":false,"error":"有订单操作正在执行，请稍后重试"}))).into_response(),
    };
    let store = state.store.clone();
    let live_state = state.live.clone();
    let live_path = state.live_path.clone();
    let markets = live_markets(&state).await;
    let st = state.live.lock().await.clone();
    if live && (!st.config.armed || !st.config.can_sign()) {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok":false,"error":"请先完成钱包授权并保存实盘配置，再执行调仓"}))).into_response();
    }
    let mut guard = state.live.lock().await;
    guard.last_plan = vec!["调仓执行中…（页面会自动刷新结果）".into()];
    let _ = guard.save(&live_path);
    drop(guard);

    tokio::spawn(async move {
        // gate 的所有权跟着任务走，任务结束才释放
        let _job = job;
        let _held = execution;
        let outcome = async {
            let needs_baseline = live && live_state.lock().await.capital_baseline.is_none();
            if needs_baseline {
                crate::live::record_equity_now(&live_state, &live_path).await?;
            }
            let st = live_state.lock().await.clone();
            crate::live::run(&store, &st, &markets, live).await
        }.await;
        let mut g = live_state.lock().await;
        match outcome {
            Ok((result, records)) => {
                g.last_run_at = Some(crate::live::now_ms_pub());
                if result.live {
                    for (coin, until) in result.stopped.iter() {
                        g.stop_cooldowns.insert(coin.clone(), *until);
                    }
                }
                g.last_plan = result.plan_lines.clone();
                if let Some(r) = result.tp_ref.clone() {
                    g.tp_ref = r;
                }
                // 只有真实调仓才记录这次用的波动缩放系数（干跑不写 ✓）
                if live {
                    if let Some(k) = result.vol_k {
                        g.vol_k = k;
                    }
                }
                g.last_live = live;
                if live {
                    g.records.extend(records);
                    g.history.push(crate::live::EquityPoint {
                        ts: crate::live::now_ms_pub(),
                        equity: result.equity,
                        pnl: 0.0,
                    });
                }
                let _ = g.save(&live_path);
            }
            Err(e) => {
                g.last_plan = vec![format!("调仓失败: {e:#}")];
                let _ = g.save(&live_path);
            }
        }
    });

    Json(json!({"ok": true, "started": true,
        "message": "已开始执行，结果会在页面自动刷新"})).into_response()
}

/// 记录一笔入金（正数）或出金（负数）。
///
/// 入金/出金会直接改变账户净值，但不是策略赚的 —— 必须记进 net_deposit，
/// 净收益扣除净入金；前端简单收益率以起点资金加净入金为分母。
async fn live_capital(State(state): State<AppState>, Json(body): Json<CapitalBody>) -> Response {
    let amount = body.amount;
    if !amount.is_finite() || amount == 0.0 {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "金额必须是有限且非零的数"}))).into_response();
    }
    let mut st = state.live.lock().await;
    let previous = st.clone();
    if !(st.net_deposit + amount).is_finite() {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok":false,"error":"累计金额超出范围"}))).into_response();
    }
    if let Some(ts) = body.effective_ts {
        if !valid_flow_time(&st, ts) {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok":false,"error":"到账时间必须在记录起点之后且不晚于现在"}))).into_response();
        }
    }
    // 防重复提交：金额与到账时间完全相同的流水已存在时拒绝，避免把同一笔入金记两遍。
    if st.capital_flows.iter().any(|f| {
        (f.amount - amount).abs() < 0.01
            && f.effective_ts.or(Some(f.ts)) == body.effective_ts.or(Some(f.ts))
    }) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"ok": false, "error": "已存在金额与到账时间相同的流水，疑似重复提交"})),
        )
            .into_response();
    }
    st.net_deposit += amount;
    st.capital_flows.push(crate::live::CapitalFlow {
        ts: crate::live::now_ms_pub(),
        effective_ts: body.effective_ts,
        amount,
        note: body.note.unwrap_or_default(),
    });
    if let Err(e) = st.save(&state.live_path) {
        *st = previous;
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": format!("保存失败: {e:#}")}))).into_response();
    }
    Json(json!({"ok": true, "net_deposit": st.net_deposit})).into_response()
}

#[derive(serde::Deserialize)]
struct FlowIndexBody {
    index: usize,
}

/// 删除一条资金流水（用于清理重复提交）。会同步修正 net_deposit。
async fn live_capital_delete(State(state): State<AppState>, Json(body): Json<FlowIndexBody>) -> Response {
    let mut st = state.live.lock().await;
    let previous = st.clone();
    if body.index >= st.capital_flows.len() {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "流水序号越界"}))).into_response();
    }
    let removed = st.capital_flows.remove(body.index);
    st.net_deposit -= removed.amount;
    if let Err(e) = st.save(&state.live_path) {
        *st = previous;
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": format!("保存失败: {e:#}")}))).into_response();
    }
    Json(json!({"ok": true, "removed": removed.amount, "net_deposit": st.net_deposit})).into_response()
}

#[derive(serde::Deserialize)]
struct BaselineBody {
    equity: f64,
    /// 记录起点的毫秒时间戳（必须早于所有资金流水）
    ts: i64,
    #[serde(default)]
    note: Option<String>,
}

/// 恢复/修正「记录起点」。只改基准，不动持仓、记录和配置。
/// 把记录起点重置到「现在」：归档旧状态 ✗ 清空历史/流水/成交回执 ✗
/// 本金改为当前净值（所以战绩从 0 开始 ✓）
///
/// 注意: 这只是【记账口径】的重置 ✗ 账户里真实的钱一分没变 ✓
/// 旧状态会先备份到 live-backup-<时间戳>.json ✓ 可恢复 ✓
async fn live_reset_start(State(state): State<AppState>) -> Response {
    let _gate = match state.exec_gate.try_lock() {
        Ok(g) => g,
        Err(_) => return (StatusCode::CONFLICT, Json(json!({"ok":false,"error":"订单操作正在执行，请稍后重试"}))).into_response(),
    };
    let mut st = state.live.lock().await;
    let previous = st.clone();
    // 当前净值：优先用最新记录点 ✗ 没有就用基准 ✓
    let equity = st
        .history
        .last()
        .map(|p| p.equity)
        .or_else(|| st.capital_baseline.as_ref().map(|b| b.equity))
        .unwrap_or(0.0);
    if !(equity > 0.0) {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok":false,"error":"取不到当前净值，无法重置"}))).into_response();
    }
    // 归档
    let stamp = crate::live::now_ms_pub();
    let live_path: &std::path::Path = state.live_path.as_ref();
    if let Ok(bytes) = std::fs::read(live_path) {
        let dir = live_path.parent().unwrap_or(std::path::Path::new("."));
        let _ = std::fs::write(dir.join(format!("live-backup-{stamp}.json")), bytes);
    }
    let now = crate::live::now_ms_pub();
    st.capital_baseline = Some(crate::live::CapitalBaseline {
        account: st.config.account.clone(),
        equity,
        ts: Some(now),
        source: "重置记录起点（旧记录已归档）".into(),
    });
    st.history = vec![crate::live::EquityPoint { ts: now, equity, pnl: 0.0 }];
    st.capital_flows.clear();
    st.records.clear();
    st.tp_oids.clear();
    st.tp_ref.clear();
    st.last_plan.clear();
    st.vol_k = 0.0;
    st.net_deposit = 0.0;
    if let Err(e) = st.save(live_path) {
        *st = previous;
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok":false,"error":format!("保存失败: {e:#}")}))).into_response();
    }
    Json(json!({"ok": true, "baseline_equity": equity, "ts": now})).into_response()
}

async fn live_baseline(State(state): State<AppState>, Json(body): Json<BaselineBody>) -> Response {
    if !body.equity.is_finite() || body.equity <= 0.0 || body.ts <= 0 {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "起点净值必须 > 0 且时间戳合法"}))).into_response();
    }
    let mut st = state.live.lock().await;
    let previous = st.clone();
    st.capital_baseline = Some(crate::live::CapitalBaseline {
        account: st.config.account.clone(),
        equity: body.equity,
        ts: Some(body.ts),
        source: body.note.unwrap_or_else(|| "手动恢复记录起点".into()),
    });
    if let Err(e) = st.save(&state.live_path) {
        *st = previous;
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": format!("保存失败: {e:#}")}))).into_response();
    }
    Json(json!({"ok": true, "baseline": st.capital_baseline})).into_response()
}

fn valid_flow_time(st: &crate::live::LiveState, ts: i64) -> bool {
    ts > 0 && ts <= crate::live::now_ms_pub() && st.baseline().and_then(|b| b.ts).map_or(false, |start| ts > start)
}

#[derive(serde::Deserialize)]
struct CapitalTimeBody {
    account: String,
    index: usize,
    recorded_ts: i64,
    effective_ts: i64,
}

async fn live_capital_time(State(state): State<AppState>, Json(body): Json<CapitalTimeBody>) -> Response {
    let mut st = state.live.lock().await;
    if !st.config.account.eq_ignore_ascii_case(&body.account) ||
        st.capital_flows.get(body.index).map(|f| f.ts) != Some(body.recorded_ts) {
        return (StatusCode::CONFLICT, Json(json!({"ok":false,"error":"账户或流水已变化，请刷新后重试"}))).into_response();
    }
    if !valid_flow_time(&st, body.effective_ts) {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok":false,"error":"到账时间必须在记录起点之后且不晚于现在"}))).into_response();
    }
    let previous = st.clone();
    st.capital_flows[body.index].effective_ts = Some(body.effective_ts);
    if let Err(e) = st.save(&state.live_path) {
        *st = previous;
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok":false,"error":format!("保存失败: {e}")}))).into_response();
    }
    Json(json!({"ok":true})).into_response()
}

// ---- 资金费套利面板 ----

/// 资金矩阵配置：三种策略的资金分配（仅记录与展示，不自动执行）。
#[derive(serde::Deserialize)]
struct CapitalAllocBody {
    momentum_usd: f64,
    carry_usd: f64,
    hunting_usd: f64,
}

async fn capital_alloc(State(state): State<AppState>, Json(body): Json<CapitalAllocBody>) -> Response {
    let vals = [body.momentum_usd, body.carry_usd, body.hunting_usd];
    if !vals.iter().all(|v| v.is_finite() && *v >= 0.0) {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "金额必须是非负有限数"}))).into_response();
    }
    let mut st = state.live.lock().await;
    let prev = st.clone();
    st.capital_alloc = crate::live::CapitalAlloc {
        momentum_usd: body.momentum_usd,
        carry_usd: body.carry_usd,
        hunting_usd: body.hunting_usd,
    };
    if let Err(e) = st.save(&state.live_path) {
        *st = prev;
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": format!("保存失败: {e:#}")}))).into_response();
    }
    Json(json!({"ok": true})).into_response()
}


// ---- HL 单账户 carry 影子 ----

async fn hlcarry_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let st = state.hlcarry.lock().await.clone();
    Json(json!({
        "state": st,
        "equity": st.equity(),
        "annualized_pct": st.annualized_pct(),
        "warnings": st.warnings(),
    }))
}


#[derive(serde::Deserialize)]
struct HlCarryParamsBody {
    top_k: Option<usize>,
    min_funding_ann_pct: Option<f64>,
    min_spot_volume_usd: Option<f64>,
    max_leg_weight: Option<f64>,
    payback_days: Option<f64>,
    min_hold_hours: Option<f64>,
}

/// 改选币/换仓参数。只改规则，不动当前持仓——下一次评估才生效。
async fn hlcarry_params(
    State(state): State<AppState>,
    Json(body): Json<HlCarryParamsBody>,
) -> Response {
    let mut st = state.hlcarry.lock().await;
    let previous = st.clone();
    if let Some(v) = body.top_k {
        st.params.top_k = v;
    }
    if let Some(v) = body.min_funding_ann_pct {
        st.params.min_funding_ann_pct = v;
    }
    if let Some(v) = body.min_spot_volume_usd {
        st.params.min_spot_volume_usd = v;
    }
    if let Some(v) = body.max_leg_weight {
        st.params.max_leg_weight = v;
    }
    if let Some(v) = body.min_hold_hours {
        st.params.min_hold_hours = v;
    }
    if let Some(v) = body.payback_days {
        st.params.payback_days = v;
    }
    st.params = st.params.clone().sanitized();
    if let Err(e) = st.save(&state.hlcarry_path) {
        *st = previous;
        return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "error": format!("保存失败: {e:#}")}))).into_response();
    }
    Json(json!({"ok": true, "params": st.params})).into_response()
}

#[derive(serde::Deserialize)]
struct HlCarryOpenBody {
    budget_usd: Option<f64>,
}

async fn hlcarry_open(State(state): State<AppState>, Json(body): Json<HlCarryOpenBody>) -> Response {
    let Some(budget) = body.budget_usd.filter(|b| b.is_finite() && *b > 0.0) else {
        return (StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "budget_usd 必须是正数"}))).into_response();
    };
    // 影子预算只影响模拟，但仍然拒绝明显手滑的量级。
    if budget > 10_000_000.0 {
        return (StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "budget_usd 上限 10,000,000"}))).into_response();
    }
    let market = match crate::hlcarry::fetch_market(&state.http).await {
        Ok(m) => m,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY,
                    Json(json!({"ok": false, "error": format!("行情失败: {e:#}")}))).into_response()
        }
    };
    let now = crate::live::now_ms_pub();
    let mut st = state.hlcarry.lock().await;
    let previous = st.clone();
    st.open(budget, &market, now);
    if let Err(e) = st.save(&state.hlcarry_path) {
        *st = previous;
        return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "error": format!("保存失败: {e:#}")}))).into_response();
    }
    Json(json!({"ok": true, "legs": st.legs.len()})).into_response()
}

async fn hlcarry_step(State(state): State<AppState>) -> Response {
    // 先看有没有持仓，再决定要不要打网络：空仓时没必要拉行情。
    if state.hlcarry.lock().await.legs.is_empty() {
        return (StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "还没有影子持仓，请先用「按预算重置模拟」设置预算"})))
            .into_response();
    }
    let market = match crate::hlcarry::fetch_market(&state.http).await {
        Ok(m) => m,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY,
                    Json(json!({"ok": false, "error": format!("行情失败: {e:#}")}))).into_response()
        }
    };
    let now = crate::live::now_ms_pub();
    let mut st = state.hlcarry.lock().await;
    if st.legs.is_empty() {
        return (StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "影子持仓已清空，请先用「按预算重置模拟」设置预算"})))
            .into_response();
    }
    let previous = st.clone();
    st.step(&market, now);
    if let Err(e) = st.save(&state.hlcarry_path) {
        *st = previous;
        return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "error": format!("保存失败: {e:#}")}))).into_response();
    }
    Json(json!({"ok": true})).into_response()
}

// ---- 纸面资金费 carry 策略 ----

// ---- 跨所价差纸面引擎 ----

async fn live_reset(State(state): State<AppState>, Query(q): Query<std::collections::HashMap<String, String>>) -> Response {
    // 破坏性操作：必须显式带 confirm=reset，避免误触/脚本误调清空全部历史。
    if q.get("confirm").map(|v| v.as_str()) != Some("reset") {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "重置实盘记录必须显式确认：/api/live/reset?confirm=reset"}))).into_response();
    }
    let mut st = state.live.lock().await;
    let cfg = st.config.clone();
    *st = crate::live::LiveState {
        config: cfg,
        ..Default::default()
    };
    let _ = st.save(&state.live_path);
    Json(json!({"ok": true})).into_response()
}

/// 逐币把现有仓位转成全仓（平→切→重开）。
async fn live_rebuild(State(state): State<AppState>) -> Response {
    // 和调仓一样必须跑在脱离请求的后台任务里。转全仓是「逐币平仓 → 切模式 →
    // 重新开仓」的多步流程，中途如果 axum 因为客户端断开而丢弃 future，
    // 就会停在只剩半边的状态：一部分币平了、一部分还开着，组合裸着没人管。
    let gate = match state.exec_gate.clone().try_lock_owned() {
        Ok(g) => g,
        Err(_) => {
            return Json(json!({"ok": false, "error": "有订单操作正在执行中，请稍候"})).into_response()
        }
    };
    let st = state.live.lock().await.clone();
    if !st.config.armed {
        return Json(json!({"ok": false, "error": "实盘未启用，请先在「实盘设置」打开开关"})).into_response();
    }
    let markets = live_markets(&state).await;
    let store = state.store.clone();
    let live = state.live.clone();
    let path = state.live_path.clone();
    {
        let mut g = state.live.lock().await;
        g.last_plan = vec!["转全仓执行中…（页面会自动刷新结果）".into()];
        let _ = g.save(&path);
    }
    tokio::spawn(async move {
        let _held = gate;
        let outcome = crate::live::rebuild_cross(&store, &st, &markets).await;
        let mut g = live.lock().await;
        g.last_plan = match outcome {
            Ok(log) => log,
            Err(e) => vec![format!("转全仓失败：{e}")],
        };
        let _ = g.save(&path);
    });
    Json(json!({"ok": true, "started": true})).into_response()
}

/// 只清空下单记录（保留净值曲线和配置）。用于清掉旧版本写下的错位记录。
async fn live_records_clear(State(state): State<AppState>) -> Response {
    let mut st = state.live.lock().await;
    let previous = st.clone();
    st.capital_baseline = st.baseline();
    let n = st.records.len();
    st.records.clear();
    // Clearing the execution log must not erase account performance history.
    if let Err(e) = st.save(&state.live_path) {
        *st = previous;
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok":false,"error":format!("清除记录保存失败: {e}")}))).into_response();
    }
    Json(json!({"ok": true, "cleared": n})).into_response()
}

/// 重新挂止盈单（不调仓，只刷新止盈）。
async fn live_tp(State(state): State<AppState>) -> Response {
    let _gate = match state.exec_gate.try_lock() {
        Ok(g) => g,
        Err(_) => {
            return Json(json!({"ok": false, "error": "有订单操作正在执行中，请稍候"})).into_response()
        }
    };
    let st = state.live.lock().await.clone();
    if !st.config.armed {
        return Json(json!({"ok": false, "error": "实盘未启用"})).into_response();
    }
    if !st.config.can_sign() {
        return Json(json!({"ok": false, "error": "未配置 API 钱包密钥"})).into_response();
    }
    let exec = match crate::exchange::Exec::signer_config(&st.config)
    .await
    {
        Ok(e) => e,
        Err(e) => return Json(json!({"ok": false, "error": format!("{e}")})).into_response(),
    };
    let markets = live_markets(&state).await;
    match crate::live::refresh_take_profits(&exec, &markets, st.config.take_profit_pct, &st.tp_ref).await {
        Ok((log, oids)) => {
            // 写进真正的状态（不是上面那个 clone ✗）✗ 否则内存里没有 oid ✗
            // 对账时就无法把成交标成「止盈成交」✗
            let mut guard = state.live.lock().await;
            guard.tp_oids.extend(oids);
            if guard.tp_oids.len() > 4000 {
                let cut = guard.tp_oids.len() - 4000;
                guard.tp_oids.drain(0..cut);
            }
            let _ = guard.save(&state.live_path);
            drop(guard);
            Json(json!({"ok": true, "log": log})).into_response()
        }
        Err(e) => Json(json!({"ok": false, "error": format!("{e}")})).into_response(),
    }
}

#[cfg(test)]
mod capital_time_tests {
    use super::*;
    #[test]
    fn capital_time_requires_a_known_baseline_and_past_time() {
        let mut st = crate::live::LiveState::default();
        assert!(!valid_flow_time(&st, 1000));
        st.config.account = "a".into();
        st.capital_baseline = Some(crate::live::CapitalBaseline { account:"a".into(), equity:500.0, ts:Some(1000), source:"test".into() });
        assert!(!valid_flow_time(&st, 1000));
        assert!(valid_flow_time(&st, 1001));
        assert!(!valid_flow_time(&st, crate::live::now_ms_pub()+60_000));
    }
    #[test]
    fn legacy_capital_flow_does_not_invent_an_effective_time() {
        let f: crate::live::CapitalFlow = serde_json::from_value(json!({"ts":2000,"amount":500,"note":"legacy"})).unwrap();
        assert!(f.effective_ts.is_none());
        let mut updated = f; updated.effective_ts=Some(1500);
        let restored: crate::live::CapitalFlow = serde_json::from_value(serde_json::to_value(updated).unwrap()).unwrap();
        assert_eq!(restored.ts,2000);
        assert_eq!(restored.effective_ts,Some(1500));
    }
}

#[cfg(test)]
mod live_routes_tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn live_page_and_configuration_preserve_save_and_execution_gates() {
        let root = std::env::temp_dir().join(format!("stars-routes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let base = AppState {
            store: Arc::new(Store::open(&root.join("hl.sqlite")).unwrap()),
            paper: Arc::new(Mutex::new(PaperState::default())),
            paper_path: Arc::new(root.join("paper.json")),
            live: Arc::new(Mutex::new(crate::live::LiveState::default())),
            live_path: Arc::new(root.join("hl.json")),
            meta: Arc::new(Mutex::new(MetaCache::default())),
            refresh: Arc::new(Mutex::new(RefreshStatus::default())),
            http: reqwest::Client::new(), exec_gate: Arc::new(Mutex::new(())),
            run_gate: Arc::new(Mutex::new(())),
            hlcarry: Arc::new(Mutex::new(crate::hlcarry::HlCarryState::default())),
            hlcarry_path: Arc::new(root.join("hlcarry.json")),
            combine: Arc::new(Mutex::new(crate::combine::CombineState::load_or_init(
                &root.join("combine.json"),
                5000.0,
            ))),
            combine_path: Arc::new(root.join("combine.json")),
            auth_token: None,
            paper2: Arc::new(Mutex::new(crate::paper::PaperState::default())),
            paper2_path: Arc::new(root.join("paper2.json")),
            paper3: Arc::new(Mutex::new(crate::paper::PaperState::default())),
            paper3_path: Arc::new(root.join("paper3.json")),
        };
        let app = router(base.clone());
        let response = app.clone().oneshot(Request::builder().uri("/").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(app.clone().oneshot(Request::builder().uri("/api/points").body(Body::empty()).unwrap()).await.unwrap().status(), StatusCode::NOT_FOUND);
        let request = Request::builder().method("POST").uri("/api/live/config")
            .header("content-type", "application/json").body(Body::from(r#"{"lookback":21,"target_positions":4}"#)).unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(base.live.lock().await.config.lookback, 21);
        assert!(!base.live.lock().await.config.armed);
        assert!(base.live_path.exists());
        let failing = AppState { live_path: Arc::new(root.clone()), ..base.clone() };
        let response = live_config(State(failing), Json(serde_json::from_value(json!({"armed":true})).unwrap())).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!base.live.lock().await.config.armed);
        let planning = base.run_gate.lock().await;
        let request = Request::builder().method("POST").uri("/api/live/config")
            .header("content-type", "application/json").body(Body::from(r#"{"armed":false,"auto_run":false,"min_vol_usd":1000000}"#)).unwrap();
        assert_eq!(app.clone().oneshot(request).await.unwrap().status(), StatusCode::OK);
        drop(planning);
        let executing = base.exec_gate.lock().await;
        let request = Request::builder().method("POST").uri("/api/live/config")
            .header("content-type", "application/json").body(Body::from(r#"{"armed":true}"#)).unwrap();
        assert_eq!(app.clone().oneshot(request).await.unwrap().status(), StatusCode::CONFLICT);
        assert!(!base.live.lock().await.config.armed);
        drop(executing);
        drop(app); drop(base);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn capital_routes_round_trip() {
        let root = std::env::temp_dir().join(format!("stars-funding-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let base = AppState {
            store: Arc::new(Store::open(&root.join("hl.sqlite")).unwrap()),
            paper: Arc::new(Mutex::new(PaperState::default())),
            paper_path: Arc::new(root.join("paper.json")),
            live: Arc::new(Mutex::new(crate::live::LiveState::default())),
            live_path: Arc::new(root.join("hl.json")),
            meta: Arc::new(Mutex::new(MetaCache::default())),
            refresh: Arc::new(Mutex::new(RefreshStatus::default())),
            http: reqwest::Client::new(), exec_gate: Arc::new(Mutex::new(())),
            run_gate: Arc::new(Mutex::new(())),
            hlcarry: Arc::new(Mutex::new(crate::hlcarry::HlCarryState::default())),
            hlcarry_path: Arc::new(root.join("hlcarry.json")),
            combine: Arc::new(Mutex::new(crate::combine::CombineState::load_or_init(
                &root.join("combine.json"),
                5000.0,
            ))),
            combine_path: Arc::new(root.join("combine.json")),
            auth_token: None,
            paper2: Arc::new(Mutex::new(crate::paper::PaperState::default())),
            paper2_path: Arc::new(root.join("paper2.json")),
            paper3: Arc::new(Mutex::new(crate::paper::PaperState::default())),
            paper3_path: Arc::new(root.join("paper3.json")),
        };
        let app = router(base.clone());
        // 资金矩阵：保存 → 持久化 → 拒绝对负数。
        let req = Request::builder().method("POST").uri("/api/capital")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"momentum_usd":3500,"carry_usd":1000,"hunting_usd":500}"#)).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        {
            let st = base.live.lock().await;
            assert_eq!(st.capital_alloc.momentum_usd, 3500.0);
            assert_eq!(st.capital_alloc.carry_usd, 1000.0);
            assert_eq!(st.capital_alloc.hunting_usd, 500.0);
        }
        assert!(base.live_path.exists());
        // 负数被拒。
        let req = Request::builder().method("POST").uri("/api/capital")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"momentum_usd":-1,"carry_usd":0,"hunting_usd":0}"#)).unwrap();
        assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::BAD_REQUEST);
        // 落盘后重新加载：字段必须保留（serde 持久化回归）。
        let reloaded = crate::live::LiveState::load(&root.join("hl.json"));
        assert_eq!(reloaded.capital_alloc.momentum_usd, 3500.0);
        // 经 /api/live 读回：快照必须包含 capital_alloc（漏掉会静默丢配置）。
        let resp = app.clone().oneshot(Request::builder().uri("/api/live").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["capital_alloc"]["momentum_usd"], 3500.0);
        assert_eq!(v["capital_alloc"]["carry_usd"], 1000.0);
        assert_eq!(v["capital_alloc"]["hunting_usd"], 500.0);
        drop(app); drop(base);
        std::fs::remove_dir_all(root).unwrap();
    }
}
