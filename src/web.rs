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
    /// Prevent duplicate manual/automatic jobs while their public-data preparation runs.
    pub run_gate: Arc<tokio::sync::Mutex<()>>,
    /// 资金费套利面板数据（每 60 秒后台刷新）。
    pub funding: Arc<Mutex<crate::funding::FundingState>>,
    /// 币安公开接口客户端（可选 BINANCE_PROXY 代理，地区屏蔽时优雅降级）。
    pub binance_http: reqwest::Client,
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
        .route("/api/live", get(live_status))
        .route("/api/live/config", post(live_config))
        .route("/api/live/run", post(live_run))
        .route("/api/live/reset", post(live_reset))
        .route("/api/live/rebuild", post(live_rebuild))
        .route("/api/live/tp", post(live_tp))
        .route("/api/live/records/clear", post(live_records_clear))
        .route("/api/live/capital", post(live_capital))
        .route("/api/live/capital/time", post(live_capital_time))
        .route("/api/funding", get(funding_status))
        .route("/api/funding/refresh", post(funding_refresh))
        .route("/api/capital", post(capital_alloc))
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
    Json(json!(snap)).into_response()
}

#[derive(serde::Deserialize)]
struct CapitalBody {
    amount: f64,
    effective_ts: Option<i64>,
    note: Option<String>,
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
    min_order_pct: Option<f64>,
    take_profit_pct: Option<f64>,
    rebalance_slices: Option<u32>,

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
                g.last_plan = result.plan_lines.clone();
                if let Some(r) = result.tp_ref.clone() {
                    g.tp_ref = r;
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

async fn funding_status(State(state): State<AppState>) -> Json<crate::funding::FundingState> {
    let st = state.funding.lock().await.clone();
    Json(st)
}

async fn funding_refresh(State(state): State<AppState>) -> Response {
    let fresh = crate::funding::refresh(&state.http, &state.binance_http).await;
    let mut st = state.funding.lock().await;
    *st = fresh;
    Json(st.clone()).into_response()
}

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
        Ok(log) => Json(json!({"ok": true, "log": log})).into_response(),
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
            funding: Arc::new(Mutex::new(crate::funding::FundingState::default())),
            binance_http: reqwest::Client::new(),
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
    async fn funding_and_capital_routes_round_trip() {
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
            funding: Arc::new(Mutex::new(crate::funding::FundingState::default())),
            binance_http: reqwest::Client::new(),
        };
        let app = router(base.clone());
        // 资金费面板（初始为空状态, 仍应 200）。
        let resp = app.clone().oneshot(Request::builder().uri("/api/funding").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
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
