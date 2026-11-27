//! HTTP handlers for live Momentum + ML monitoring, config, capital flows, and execution.

use super::AppState;
use crate::store::PanelEntry;
use axum::{extract::{Query, State}, http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;

// ==================== live trading ====================

pub(super) async fn live_markets(state: &AppState) -> std::collections::HashMap<String, crate::exchange::MarketInfo> {
    let meta = state.meta.lock().await;
    crate::live::markets_from_meta(&meta.universe)
}

pub(super) async fn snapshots_view(State(state): State<AppState>) -> Json<serde_json::Value> {
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


pub(super) async fn strategy_health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let panel: Vec<PanelEntry> = state
        .store
        .all_panels()
        .unwrap_or_default()
        .into_iter()
        .map(|(coin, candles)| PanelEntry { coin, candles })
        .collect();
    let (hist, params) = {
        let st = state.live.lock().await;
        (
            crate::live::index_history(&st),
            crate::health::HealthParams::from_live_config(&st.config),
        )
    };
    let h = crate::health::compute_with_params(&panel, &hist, &params);
    Json(serde_json::json!(h))
}


pub(super) async fn live_status(State(state): State<AppState>) -> Response {
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
    // ML 策略子账户（第二个子账户）：只读查询净值/仓位，供页面并排展示 + 合计总资金。
    if let Some(ml_addr) = snap.config.ml_vault_address.clone() {
        if !ml_addr.is_empty() {
            snap.ml_account = Some(
                crate::live::ml_account_snapshot(&ml_addr, Some(state.http.clone())).await,
            );
        }
    }
    // 记录 ML 子账户净值历史（用于「全部账户」= 动量 + ML 合计曲线）
    if let Some(acct) = &snap.ml_account {
        if acct.error.is_none() && acct.equity.is_finite() && acct.equity >= 0.0 {
            if let Err(e) = crate::live::record_ml_equity(&state.live, &state.live_path, acct.equity).await {
                tracing::warn!("ML 净值记录失败: {e:#}");
            }
        }
    }
    {
        let current = state.live.lock().await;
        snap.ml_history = current.ml_history.clone();
        snap.ml_capital_flows = current.ml_capital_flows.clone();
        snap.ml_net_deposit = current.ml_capital_flows.iter().filter(|f| f.amount.is_finite()).map(|f| f.amount).sum();
        snap.mom_index_history = crate::live::index_history(&current);
        snap.ml_index_history = crate::live::ml_index_history(&current);
    }
    // 健康指标（红黄绿）：按实盘 3 因子 + TWR 净值指数计算
    let (scorecard, health_summary) = {
        let panel: Vec<PanelEntry> = state
            .store
            .all_panels()
            .unwrap_or_default()
            .into_iter()
            .map(|(coin, candles)| PanelEntry { coin, candles })
            .collect();
        if panel.is_empty() {
            (Vec::new(), None)
        } else {
            let st2 = state.live.lock().await;
            let hist = crate::live::index_history(&st2);
            let params = crate::health::HealthParams::from_live_config(&st2.config);
            let h = crate::health::compute_with_params(&panel, &hist, &params);
            let sc = crate::live::scorecard(&st2, &h);
            (sc, Some(h))
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
        if let Some(h) = health_summary {
            obj.insert("health".into(), serde_json::json!(h));
        }
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
pub(super) struct CapitalBody {
    amount: f64,
    effective_ts: Option<i64>,
    note: Option<String>,
    /// "momentum"（默认）或 "ml"
    #[serde(default)]
    account_type: Option<String>,
}

#[derive(serde::Deserialize)]
pub(super) struct LiveConfigBody {
    vol_target: Option<f64>,
    vol_band: Option<f64>,
    position_stop_pct: Option<f64>,
    stop_cooldown_days: Option<i64>,
    max_single_weight: Option<f64>,
    account: Option<String>,
    vault_address: Option<String>,
    ml_vault_address: Option<String>,
    ml_python: Option<String>,
    ml_dir: Option<String>,
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
    ema_alpha: Option<f64>,
    rebal_band: Option<f64>,
    hard_gross_cap: Option<f64>,
    momentum_weight: Option<f64>,
    dd_weight: Option<f64>,
    upfrac_weight: Option<f64>,

    armed: Option<bool>,
    auto_run: Option<bool>,
}

pub(super) async fn live_config(
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
    if let Some(v) = body.vault_address {
        // 空串 = 清空子账户（回到主账户）。自动去 HL: 前缀（HL 网页复制常带）
        let t = v.trim();
        let t = t.strip_prefix("HL:").or_else(|| t.strip_prefix("hl:")).unwrap_or(t);
        c.vault_address = if t.is_empty() { None } else { Some(t.to_string()) };
    }
    if let Some(v) = body.ml_vault_address {
        let t = v.trim();
        let t = t.strip_prefix("HL:").or_else(|| t.strip_prefix("hl:")).unwrap_or(t);
        c.ml_vault_address = if t.is_empty() { None } else { Some(t.to_string()) };
    }
    if let Some(v) = body.ml_python {
        if !v.trim().is_empty() {
            c.ml_python = v.trim().to_string();
        }
    }
    if let Some(v) = body.ml_dir {
        if !v.trim().is_empty() {
            c.ml_dir = v.trim().to_string();
        }
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
    if let Some(v) = body.ema_alpha {
        if !v.is_finite() {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "EMA 系数无效"}))).into_response();
        }
        c.ema_alpha = v.clamp(0.1, 1.0);
    }
    if let Some(v) = body.rebal_band {
        if !v.is_finite() {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "调仓死区无效"}))).into_response();
        }
        c.rebal_band = v.clamp(0.0, 0.50);
    }
    if let Some(v) = body.hard_gross_cap {
        if !v.is_finite() || v <= 0.0 {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "硬杠杆上限必须为正数"}))).into_response();
        }
        c.hard_gross_cap = v.clamp(0.5, 5.0);
    }
    if let Some(v) = body.take_profit_pct {
        c.take_profit_pct = v.clamp(0.0, 0.5);
    }
    if let Some(v) = body.momentum_weight {
        // 动量占比 0~1（量能占其余）。0.7 = 动量七成。
        c.momentum_weight = v.clamp(0.0, 1.0);
    }
    if let Some(v) = body.dd_weight {
        c.dd_weight = v.clamp(0.0, 2.0);
    }
    if let Some(v) = body.upfrac_weight {
        c.upfrac_weight = v.clamp(0.0, 2.0);
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
    if let Some(v) = body.max_single_weight {
        if !(0.0..=1.0).contains(&v) {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "单币上限必须在 0~100%（0 = 关闭）"}))).into_response();
        }
        c.max_single_weight = v;
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
pub(super) struct LiveRunBody {
    #[serde(default)]
    live: bool,
}

pub(super) async fn live_run(State(state): State<AppState>, body: Option<Json<LiveRunBody>>) -> Response {
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
                    g.push_history_dedup(crate::live::now_ms_pub(), result.equity, 0.0);
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
pub(super) async fn live_capital(State(state): State<AppState>, Json(body): Json<CapitalBody>) -> Response {
    let amount = body.amount;
    if !amount.is_finite() || amount == 0.0 {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "金额必须是有限且非零的数"}))).into_response();
    }
    let is_ml = body.account_type.as_deref() == Some("ml");
    let mut st = state.live.lock().await;
    let previous = st.clone();
    if is_ml {
        if let Some(ts) = body.effective_ts {
            if ts <= 0 || ts > crate::live::now_ms_pub() {
                return (StatusCode::BAD_REQUEST, Json(json!({"ok":false,"error":"ML 流水时间不合法"}))).into_response();
            }
        }
        if st.ml_capital_flows.iter().any(|f| {
            (f.amount - amount).abs() < 0.01
                && f.effective_ts.or(Some(f.ts)) == body.effective_ts.or(Some(f.ts))
        }) {
            return (
                StatusCode::CONFLICT,
                Json(json!({"ok": false, "error": "ML 已存在相同金额与时间的流水"})),
            )
                .into_response();
        }
        st.ml_capital_flows.push(crate::live::CapitalFlow {
            ts: crate::live::now_ms_pub(),
            effective_ts: body.effective_ts,
            amount,
            note: body.note.unwrap_or_default(),
        });
        if let Err(e) = st.save(&state.live_path) {
            *st = previous;
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": format!("保存失败: {e:#}")}))).into_response();
        }
        let ml_net: f64 = st.ml_capital_flows.iter().filter(|f| f.amount.is_finite()).map(|f| f.amount).sum();
        return Json(json!({"ok": true, "account_type": "ml", "ml_net_deposit": ml_net})).into_response();
    }
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
pub(super) struct FlowIndexBody {
    index: usize,
    #[serde(default)]
    account_type: Option<String>,
}

/// 删除一条资金流水（用于清理重复提交）。会同步修正 net_deposit。
pub(super) async fn live_capital_delete(State(state): State<AppState>, Json(body): Json<FlowIndexBody>) -> Response {
    let mut st = state.live.lock().await;
    let previous = st.clone();
    if body.account_type.as_deref() == Some("ml") {
        if body.index >= st.ml_capital_flows.len() {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "ML 流水序号越界"}))).into_response();
        }
        let removed = st.ml_capital_flows.remove(body.index);
        if let Err(e) = st.save(&state.live_path) {
            *st = previous;
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": format!("保存失败: {e:#}")}))).into_response();
        }
        let ml_net: f64 = st.ml_capital_flows.iter().filter(|f| f.amount.is_finite()).map(|f| f.amount).sum();
        return Json(json!({"ok": true, "removed": removed.amount, "ml_net_deposit": ml_net})).into_response();
    }
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
pub(super) struct BaselineBody {
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
pub(super) async fn live_reset_start(State(state): State<AppState>) -> Response {
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

pub(super) async fn live_baseline(State(state): State<AppState>, Json(body): Json<BaselineBody>) -> Response {
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

pub(super) fn valid_flow_time(st: &crate::live::LiveState, ts: i64) -> bool {
    ts > 0 && ts <= crate::live::now_ms_pub() && st.baseline().and_then(|b| b.ts).map_or(false, |start| ts > start)
}

#[derive(serde::Deserialize)]
pub(super) struct CapitalTimeBody {
    account: String,
    index: usize,
    recorded_ts: i64,
    effective_ts: i64,
}

pub(super) async fn live_capital_time(State(state): State<AppState>, Json(body): Json<CapitalTimeBody>) -> Response {
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
pub(super) struct CapitalAllocBody {
    momentum_usd: f64,
    carry_usd: f64,
    hunting_usd: f64,
}

pub(super) async fn capital_alloc(State(state): State<AppState>, Json(body): Json<CapitalAllocBody>) -> Response {
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



pub(super) async fn live_reset(State(state): State<AppState>, Query(q): Query<std::collections::HashMap<String, String>>) -> Response {
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
pub(super) async fn live_rebuild(State(state): State<AppState>) -> Response {
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
pub(super) async fn live_records_clear(State(state): State<AppState>) -> Response {
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
pub(super) async fn live_tp(State(state): State<AppState>) -> Response {
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

