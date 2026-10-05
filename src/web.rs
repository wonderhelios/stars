//! HTTP API + shared state.

use crate::hl::{CoinMeta, MarketCtx};
use crate::momentum::PanelEntry;
use crate::paper::{self, PaperConfig, PaperState};
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
    pub paper: Arc<Mutex<PaperState>>,
    pub paper_path: Arc<std::path::PathBuf>,
    pub live: Arc<Mutex<crate::live::LiveState>>,
    pub live_path: Arc<std::path::PathBuf>,
    pub meta: Arc<Mutex<MetaCache>>,
    pub refresh: Arc<Mutex<RefreshStatus>>,
    /// Shared HTTP client so Hyperliquid connections are pooled.
    pub http: reqwest::Client,
    /// 调仓/止盈的执行闸门。互斥锁只保护状态、不覆盖下单过程，所以自动调仓
    /// 和手动点击可能同时进入 —— 两笔调仓交错会下重复单、把仓位搞乱。
    pub exec_gate: Arc<tokio::sync::Mutex<()>>,
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

#[derive(Clone, Default)]
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
        .route("/api/live", get(live_status))
        .route("/api/live/config", post(live_config))
        .route("/api/live/run", post(live_run))
        .route("/api/live/reset", post(live_reset))
        .route("/api/live/rebuild", post(live_rebuild))
        .route("/api/live/tp", post(live_tp))
        .route("/api/live/records/clear", post(live_records_clear))
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

const INDEX_HTML: &str = include_str!("../static/index.html");
const APP_JS: &str = include_str!("../static/app.js");

async fn index_html() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], INDEX_HTML)
}

async fn app_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
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
}

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
        lookback: req.lookback.max(1),
        top_frac: req.top_frac.clamp(0.05, 0.5),
        min_vol_usd: req.min_vol_usd.max(0.0),
        fee,
        capital: req.capital.max(1.0),
        leverage: req.leverage.clamp(1.0, 20.0),
        replay_days: req.replay_days.min(365),
        target_positions: req.target_positions.clamp(1, 30),
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
    let mut p = state.paper.lock().await;
    paper::step(&mut p, &panel, &max_lev);
    let _ = p.save(&state.paper_path);
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

// ==================== live trading ====================

async fn live_markets(state: &AppState) -> std::collections::HashMap<String, crate::exchange::MarketInfo> {
    let meta = state.meta.lock().await;
    crate::live::markets_from_meta(&meta.universe)
}

async fn live_status(State(state): State<AppState>) -> Response {
    let st = state.live.lock().await.clone();
    let markets = live_markets(&state).await;
    let snap = crate::live::snapshot(&st, &markets, Some(state.http.clone())).await;
    // Record at most one equity point per hour so the monitoring curve appears
    // the same day instead of after a couple of daily closes.
    if snap.equity > 0.0 {
        let now = crate::live::now_ms_pub();
        let hour = now / 3_600_000;
        let last = st.history.last().map(|p| p.ts / 3_600_000);
        if last != Some(hour) {
            let mut guard = state.live.lock().await;
            guard.history.push(crate::live::EquityPoint {
                ts: now,
                equity: snap.equity,
            });
            // Keep the file bounded (about 8 months of hourly points).
            let len = guard.history.len();
            if len > 6000 {
                guard.history.drain(0..len - 6000);
            }
            let _ = guard.save(&state.live_path);
        }
    }
    Json(json!(snap)).into_response()
}

#[derive(serde::Deserialize)]
struct LiveConfigBody {
    account: Option<String>,
    key_path: Option<String>,
    target_positions: Option<usize>,
    leverage: Option<f64>,
    margin_buffer: Option<f64>,
    slippage: Option<f64>,
    lookback: Option<usize>,
    top_frac: Option<f64>,
    min_vol_usd: Option<f64>,
    take_profit_pct: Option<f64>,
    rebalance_days: Option<u32>,
    armed: Option<bool>,
    auto_run: Option<bool>,
}

async fn live_config(
    State(state): State<AppState>,
    Json(body): Json<LiveConfigBody>,
) -> Response {
    let mut st = state.live.lock().await;
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
    if let Some(v) = body.take_profit_pct {
        c.take_profit_pct = v.clamp(0.0, 0.5);
    }
    if let Some(v) = body.rebalance_days {
        c.rebalance_days = v.clamp(1, 30);
    }
    if let Some(v) = body.armed {
        c.armed = v;
    }
    if let Some(v) = body.auto_run {
        c.auto_run = v;
    }
    let cfg = st.config.clone();
    let _ = st.save(&state.live_path);
    Json(json!({"ok": true, "config": cfg})).into_response()
}

#[derive(serde::Deserialize, Default)]
struct LiveRunBody {
    #[serde(default)]
    live: bool,
}

async fn live_run(State(state): State<AppState>, body: Option<Json<LiveRunBody>>) -> Response {
    let live = body.map(|b| b.live).unwrap_or(false);
    let _gate = match state.exec_gate.try_lock() {
        Ok(g) => g,
        Err(_) => {
            return Json(json!({
                "ok": false,
                "error": "上一次调仓还在执行中，请稍候再试"
            }))
            .into_response()
        }
    };
    let st = state.live.lock().await.clone();
    let markets = live_markets(&state).await;
    match crate::live::run(&state.store, &st, &markets, live).await {
        Ok((result, records)) => {
            let mut guard = state.live.lock().await;
            guard.last_run_at = Some(crate::live::now_ms_pub());
            guard.last_plan = result.plan_lines.clone();
            guard.last_live = live;
            if live {
                guard.records.extend(records);
                guard.history.push(crate::live::EquityPoint {
                    ts: crate::live::now_ms_pub(),
                    equity: result.equity,
                });
            }
            let _ = guard.save(&state.live_path);
            Json(json!({"ok": true, "result": result})).into_response()
        }
        Err(e) => Json(json!({"ok": false, "error": format!("{e}")})).into_response(),
    }
}

async fn live_reset(State(state): State<AppState>) -> Response {
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
    let _gate = match state.exec_gate.try_lock() {
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
    match crate::live::rebuild_cross(&state.store, &st, &markets).await {
        Ok(log) => Json(json!({"ok": true, "log": log})).into_response(),
        Err(e) => Json(json!({"ok": false, "error": format!("{e}")})).into_response(),
    }
}

/// 只清空下单记录（保留净值曲线和配置）。用于清掉旧版本写下的错位记录。
async fn live_records_clear(State(state): State<AppState>) -> Response {
    let mut st = state.live.lock().await;
    let n = st.records.len();
    st.records.clear();
    let _ = st.save(&state.live_path);
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
    let exec = match crate::exchange::Exec::signer(
        &st.config.account,
        std::path::Path::new(&st.config.key_path),
    )
    .await
    {
        Ok(e) => e,
        Err(e) => return Json(json!({"ok": false, "error": format!("{e}")})).into_response(),
    };
    let markets = live_markets(&state).await;
    match crate::live::refresh_take_profits(&exec, &markets, st.config.take_profit_pct, &st.tp_ref).await {
        Ok(log) => Json(json!({"ok": true, "log": log})).into_response(),
        Err(e) => Json(json!({"ok": false, "error": format!("{e}")})).into_response(),
    }
}
