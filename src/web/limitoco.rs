//! HTTP handlers for the pure-Rust Binance A+B Limit-OCO V2 shadow engine.

use super::AppState;
use axum::{extract::State, http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;

// ---------------------------------------------------------------------------
// 币安 A+B 纯限价 OCO 影子接口（纯 Rust 引擎）
// ---------------------------------------------------------------------------

pub(super) async fn limitoco_status(State(state): State<AppState>) -> Response {
    let snap = state.limitoco.lock().await.clone();
    Json(snap).into_response()
}

pub(super) async fn limitoco_config_get(State(state): State<AppState>) -> Response {
    let cfg = state.limitoco.lock().await.config.clone();
    Json(json!({
        "ok": true,
        "config": cfg,
        "tier_presets": crate::limitoco::tier_presets(),
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
pub(super) struct LimitOcoConfigBody {
    running: Option<bool>,
    capital: Option<f64>,
    risk_tier: Option<String>,
    risk_per_trade: Option<f64>,
    max_pos_lev: Option<f64>,
    max_total_gross_lev: Option<f64>,
    compounding: Option<bool>,
    max_total_positions: Option<usize>,
    sleeve_a_max_pos: Option<usize>,
    sleeve_a_max_same_side: Option<usize>,
    sleeve_b_max_pos: Option<usize>,
    sleeve_b_max_same_side: Option<usize>,
    max_sl_pct: Option<f64>,
    cooldown_hours: Option<i64>,
    daily_loss_limit_r: Option<f64>,
    top_n_liquid: Option<usize>,
    min_daily_vol_usd: Option<f64>,
    enabled_a: Option<bool>,
    enabled_b: Option<bool>,
    proxy_url: Option<String>,
}

pub(super) async fn limitoco_config_save(
    State(state): State<AppState>,
    Json(body): Json<LimitOcoConfigBody>,
) -> Response {
    let mut guard = state.limitoco.lock().await;
    let previous = guard.clone();
    let mut cfg = guard.config.clone();

    if let Some(ref tier) = body.risk_tier {
        cfg.apply_tier_preset(tier);
    }
    if let Some(v) = body.capital {
        if !v.is_finite() || v < 100.0 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "初始本金须 >= 100 USDT"})),
            )
                .into_response();
        }
        cfg.capital = v;
    }
    if let Some(v) = body.risk_per_trade {
        if !v.is_finite() || v <= 0.0 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "单笔风险比例无效"})),
            )
                .into_response();
        }
        cfg.risk_per_trade = v;
    }
    if let Some(v) = body.max_pos_lev {
        if !v.is_finite() || v <= 0.0 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "单仓杠杆上限无效"})),
            )
                .into_response();
        }
        cfg.max_pos_lev = v;
    }
    if let Some(v) = body.max_total_gross_lev {
        if !v.is_finite() || v <= 0.0 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "组合总杠杆上限无效"})),
            )
                .into_response();
        }
        cfg.max_total_gross_lev = v;
    }
    if let Some(v) = body.compounding { cfg.compounding = v; }
    if let Some(v) = body.max_total_positions { cfg.max_total_positions = v; }
    if let Some(v) = body.sleeve_a_max_pos { cfg.sleeve_a_max_pos = v; }
    if let Some(v) = body.sleeve_a_max_same_side { cfg.sleeve_a_max_same_side = v; }
    if let Some(v) = body.sleeve_b_max_pos { cfg.sleeve_b_max_pos = v; }
    if let Some(v) = body.sleeve_b_max_same_side { cfg.sleeve_b_max_same_side = v; }
    if let Some(v) = body.max_sl_pct { cfg.max_sl_pct = v; }
    if let Some(v) = body.cooldown_hours { cfg.cooldown_hours = v; }
    if let Some(v) = body.daily_loss_limit_r { cfg.daily_loss_limit_r = v; }
    if let Some(v) = body.top_n_liquid { cfg.top_n_liquid = v; }
    if let Some(v) = body.min_daily_vol_usd { cfg.min_daily_vol_usd = v; }
    if let Some(v) = body.enabled_a { cfg.enabled_a = v; }
    if let Some(v) = body.enabled_b { cfg.enabled_b = v; }
    if let Some(v) = body.running { cfg.running = v; }
    if let Some(v) = body.proxy_url { cfg.proxy_url = v; }

    cfg.sanitize();
    let now = crate::limitoco::now_ms();
    // 尚未开单且未跑过历史时，改本金直接同步账本初始权益，体验更自然
    let untouched = guard.trades.is_empty() && guard.positions.is_empty() && guard.pending.is_empty() && guard.last_bar_ms == 0;
    if untouched && (guard.capital - cfg.capital).abs() > 1e-6 {
        *guard = crate::limitoco::LimitOcoState::new_with_config(cfg.clone(), now);
    } else {
        guard.sync_config(cfg.clone(), now);
        guard.log_event(
            now,
            "CONFIG",
            "",
            "",
            format!(
                "更新策略参数 · 档位 {} ({:.1}%R) · 单仓上限 {:.1}x / 总杠杆 {:.1}x · 复利{} · SL<={:.0}%",
                cfg.risk_tier,
                cfg.risk_per_trade * 100.0,
                cfg.max_pos_lev,
                cfg.max_total_gross_lev,
                if cfg.compounding { "开" } else { "关" },
                cfg.max_sl_pct * 100.0
            ),
        );
    }
    if let Err(e) = guard.save(&state.limitoco_path) {
        *guard = previous;
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok": false, "error": format!("保存 Limit-OCO 配置失败: {e}")})),
        )
            .into_response();
    }
    Json(json!({
        "ok": true,
        "config": guard.config,
        "state": &*guard,
    }))
    .into_response()
}

pub(super) async fn limitoco_start(State(state): State<AppState>) -> Response {
    let _gate = match state.limitoco_gate.try_lock() {
        Ok(g) => g,
        Err(_) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({"ok": false, "error": "影子引擎正在扫描行情，请稍后再试"})),
            )
                .into_response()
        }
    };
    let mut snapshot = {
        let mut g = state.limitoco.lock().await;
        let now = crate::limitoco::now_ms();
        g.config.running = true;
        g.running = true;
        g.status_text = "启动并首次扫描中…".into();
        g.log_event(now, "START", "", "", "启动币安 A+B Limit-OCO 影子引擎".into());
        let _ = g.save(&state.limitoco_path);
        g.clone()
    };
    crate::limitoco::step_once(&mut snapshot, &state.limitoco_path, true).await;
    let mut g = state.limitoco.lock().await;
    let latest_cfg = g.config.clone();
    *g = snapshot;
    g.sync_config(latest_cfg, crate::limitoco::now_ms());
    let _ = g.save(&state.limitoco_path);
    Json(json!({"ok": true, "state": &*g})).into_response()
}

pub(super) async fn limitoco_stop(State(state): State<AppState>) -> Response {
    let mut g = state.limitoco.lock().await;
    let now = crate::limitoco::now_ms();
    g.config.running = false;
    g.running = false;
    g.status_text = "已暂停".into();
    g.log_event(now, "STOP", "", "", "暂停币安 A+B Limit-OCO 影子引擎".into());
    if let Err(e) = g.save(&state.limitoco_path) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok": false, "error": format!("保存状态失败: {e}")})),
        )
            .into_response();
    }
    Json(json!({"ok": true, "state": &*g})).into_response()
}

pub(super) async fn limitoco_step(State(state): State<AppState>) -> Response {
    let _gate = match state.limitoco_gate.try_lock() {
        Ok(g) => g,
        Err(_) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({"ok": false, "error": "影子引擎正在扫描中，请稍候"})),
            )
                .into_response()
        }
    };
    let mut snapshot = state.limitoco.lock().await.clone();
    crate::limitoco::step_once(&mut snapshot, &state.limitoco_path, true).await;
    let mut g = state.limitoco.lock().await;
    if g.started_at == snapshot.started_at {
        let latest_cfg = g.config.clone();
        *g = snapshot;
        g.sync_config(latest_cfg, crate::limitoco::now_ms());
        let _ = g.save(&state.limitoco_path);
    }
    Json(json!({"ok": true, "state": &*g})).into_response()
}

pub(super) async fn limitoco_reset(State(state): State<AppState>) -> Response {
    let _gate = match state.limitoco_gate.try_lock() {
        Ok(g) => g,
        Err(_) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({"ok": false, "error": "影子引擎正在扫描中，请稍后再重置"})),
            )
                .into_response()
        }
    };
    let mut g = state.limitoco.lock().await;
    let cfg = g.config.clone();
    let now = crate::limitoco::now_ms();
    *g = crate::limitoco::LimitOcoState::new_with_config(cfg, now);
    if let Err(e) = g.save(&state.limitoco_path) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok": false, "error": format!("重置保存失败: {e}")})),
        )
            .into_response();
    }
    Json(json!({"ok": true, "state": &*g})).into_response()
}

