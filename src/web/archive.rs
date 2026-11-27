//! HTTP handlers for archived shadow modules (HL Carry, Intraday, FastSwing, TradFi).

use super::AppState;
use anyhow::Context;
use axum::{extract::State, http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;

pub(super) async fn intraday_status(State(state): State<AppState>) -> Response {
    match tokio::fs::read(&*state.intraday_path).await {
        Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(value) => Json(value).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":format!("日内影子账本损坏: {e}")}))).into_response(),
        },
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":format!("日内影子尚未启动: {e}")}))).into_response(),
    }
}

pub(super) async fn tradfi_status(State(state): State<AppState>) -> Response {
    match tokio::fs::read(&*state.tradfi_path).await {
        Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(value) => Json(value).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":format!("TradFi 影子账本损坏: {e}")}))).into_response(),
        },
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":format!("TradFi 影子尚未启动: {e}")}))).into_response(),
    }
}

pub(super) async fn fastswing_status(State(state): State<AppState>) -> Response {
    match tokio::fs::read(&*state.fastswing_path).await {
        Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(value) => Json(value).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":format!("快速波段影子账本损坏: {e}")}))).into_response(),
        },
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":format!("快速波段影子尚未启动: {e}")}))).into_response(),
    }
}


fn tradfi_default_capital() -> f64 { 2000.0 }
fn tradfi_default_lev() -> f64 { 3.0 }
fn tradfi_default_stock_alloc() -> f64 { 0.40 }
fn tradfi_default_vbounce_alloc() -> f64 { 0.20 }
fn tradfi_default_vol_target() -> f64 { 0.40 }
fn tradfi_default_min_bars() -> usize { 7 }
fn tradfi_default_max_dd() -> f64 { 20.0 }
fn tradfi_default_maker_wait() -> f64 { 8.0 }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct TradFiConfig {
    #[serde(default = "tradfi_default_capital")]
    capital: f64,
    #[serde(default = "tradfi_default_lev")]
    gross_leverage: f64,
    #[serde(default = "tradfi_default_stock_alloc")]
    stock_alloc: f64,
    #[serde(default = "tradfi_default_vbounce_alloc")]
    vbounce_alloc: f64,
    #[serde(default = "tradfi_default_vol_target")]
    vol_target_ann: f64,
    #[serde(default = "tradfi_default_min_bars")]
    stock_min_bars: usize,
    #[serde(default = "tradfi_default_max_dd")]
    max_dd_pct: f64,
    #[serde(default = "tradfi_default_maker_wait")]
    maker_wait_sec: f64,
    #[serde(default)]
    taker_only: bool,
    #[serde(default)]
    armed: bool,
    #[serde(default)]
    auto_run: bool,
    #[serde(default)]
    key_file: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    api_secret: String,
    #[serde(default)]
    proxy_url: String,
}

impl Default for TradFiConfig {
    fn default() -> Self {
        Self {
            capital: tradfi_default_capital(),
            gross_leverage: tradfi_default_lev(),
            stock_alloc: tradfi_default_stock_alloc(),
            vbounce_alloc: tradfi_default_vbounce_alloc(),
            vol_target_ann: tradfi_default_vol_target(),
            stock_min_bars: tradfi_default_min_bars(),
            max_dd_pct: tradfi_default_max_dd(),
            maker_wait_sec: tradfi_default_maker_wait(),
            taker_only: false,
            armed: false,
            auto_run: false,
            key_file: String::new(),
            api_key: String::new(),
            api_secret: String::new(),
            proxy_url: String::new(),
        }
    }
}

fn tradfi_config_path(state: &AppState) -> std::path::PathBuf {
    state.tradfi_path.with_file_name("tradfi_config.json")
}

fn load_tradfi_config(path: &std::path::Path) -> TradFiConfig {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<TradFiConfig>(&s).ok())
        .unwrap_or_default()
}

fn save_tradfi_config(path: &std::path::Path, cfg: &TradFiConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(cfg)?;
    std::fs::write(&tmp, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn mask_api_key(k: &str) -> String {
    let t = k.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.len() <= 8 {
        return "****".to_string();
    }
    format!("{}...{}", &t[..4], &t[t.len() - 4..])
}

fn resolve_tradfi_creds(cfg: &TradFiConfig) -> (String, String) {
    let mut k = cfg.api_key.trim().to_string();
    let mut s = cfg.api_secret.trim().to_string();
    let kf = cfg.key_file.trim();
    if (k.is_empty() || s.is_empty()) && !kf.is_empty() {
        if let Ok(raw) = std::fs::read_to_string(kf) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if k.is_empty() {
                    if let Some(x) = v.get("api_key").and_then(|x| x.as_str()) {
                        k = x.trim().to_string();
                    }
                }
                if s.is_empty() {
                    if let Some(x) = v.get("api_secret").and_then(|x| x.as_str()) {
                        s = x.trim().to_string();
                    }
                }
            }
        }
    }
    (k, s)
}

fn tradfi_config_public_json(cfg: &TradFiConfig) -> serde_json::Value {
    let (k, s) = resolve_tradfi_creds(cfg);
    json!({
        "capital": cfg.capital,
        "gross_leverage": cfg.gross_leverage,
        "stock_alloc": cfg.stock_alloc,
        "vbounce_alloc": cfg.vbounce_alloc,
        "vol_target_ann": cfg.vol_target_ann,
        "stock_min_bars": cfg.stock_min_bars,
        "max_dd_pct": cfg.max_dd_pct,
        "maker_wait_sec": cfg.maker_wait_sec,
        "taker_only": cfg.taker_only,
        "armed": cfg.armed,
        "auto_run": cfg.auto_run,
        "key_file": cfg.key_file,
        "proxy_url": cfg.proxy_url,
        "has_api_key": !k.is_empty(),
        "has_api_secret": !s.is_empty(),
        "api_key_masked": mask_api_key(&k),
    })
}

fn resolve_tradfi_repo_dir() -> std::path::PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    if cwd.join("tradfi").join("binance_exec.py").exists() {
        return cwd;
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut cur = exe.parent().map(|p| p.to_path_buf());
        for _ in 0..4 {
            let Some(p) = cur else { break };
            if p.join("tradfi").join("binance_exec.py").exists() {
                return p;
            }
            cur = p.parent().map(|x| x.to_path_buf());
        }
    }
    cwd
}

async fn run_tradfi_python(
    state: &AppState,
    cfg: &TradFiConfig,
    extra: &[&str],
) -> anyhow::Result<serde_json::Value> {
    let py = {
        let live_cfg = state.live.lock().await.config.clone();
        if !live_cfg.ml_python.trim().is_empty() && std::path::Path::new(&live_cfg.ml_python).exists() {
            live_cfg.ml_python
        } else {
            "python3".to_string()
        }
    };
    let repo_dir = resolve_tradfi_repo_dir();
    let cfg_path = tradfi_config_path(state);
    let state_path = state.tradfi_path.with_file_name("tradfi_live_state.json");
    let mut cmd = tokio::process::Command::new(&py);
    cmd.arg("-m")
        .arg("tradfi.binance_exec")
        .arg("--config")
        .arg(&cfg_path)
        .arg("--state-file")
        .arg(&state_path)
        .arg("--json")
        .args(extra)
        .current_dir(&repo_dir);
    let (k, s) = resolve_tradfi_creds(cfg);
    if !k.is_empty() {
        cmd.env("BINANCE_API_KEY", &k);
    }
    if !s.is_empty() {
        cmd.env("BINANCE_API_SECRET", &s);
    }
    if !cfg.proxy_url.trim().is_empty() {
        cmd.env("HTTPS_PROXY", cfg.proxy_url.trim());
        cmd.env("HTTP_PROXY", cfg.proxy_url.trim());
    }
    let out = cmd.output().await.context("启动 tradfi.binance_exec 失败")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        return Err(anyhow::anyhow!("{detail}"));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).context("解析 TradFi 执行结果失败")?;
    Ok(val)
}

pub(super) async fn tradfi_config_get(State(state): State<AppState>) -> Response {
    let cfg = load_tradfi_config(&tradfi_config_path(&state));
    Json(json!({"ok": true, "config": tradfi_config_public_json(&cfg)})).into_response()
}

#[derive(serde::Deserialize)]
pub(super) struct TradFiConfigBody {
    capital: Option<f64>,
    gross_leverage: Option<f64>,
    stock_alloc: Option<f64>,
    vbounce_alloc: Option<f64>,
    vol_target_ann: Option<f64>,
    stock_min_bars: Option<usize>,
    max_dd_pct: Option<f64>,
    maker_wait_sec: Option<f64>,
    taker_only: Option<bool>,
    armed: Option<bool>,
    auto_run: Option<bool>,
    key_file: Option<String>,
    api_key: Option<String>,
    api_secret: Option<String>,
    clear_keys: Option<bool>,
    proxy_url: Option<String>,
}

pub(super) async fn tradfi_config_save(
    State(state): State<AppState>,
    Json(body): Json<TradFiConfigBody>,
) -> Response {
    let path = tradfi_config_path(&state);
    let mut cfg = load_tradfi_config(&path);
    if let Some(v) = body.capital {
        if !v.is_finite() || v <= 0.0 {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "资金必须大于 0"}))).into_response();
        }
        cfg.capital = v.clamp(50.0, 10_000_000.0);
    }
    if let Some(v) = body.gross_leverage {
        if !v.is_finite() {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "杠杆参数无效"}))).into_response();
        }
        cfg.gross_leverage = v.clamp(1.0, 5.0);
    }
    if let Some(v) = body.stock_alloc {
        if !v.is_finite() {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "美股占比无效"}))).into_response();
        }
        cfg.stock_alloc = v.clamp(0.0, 1.0);
    }
    if let Some(v) = body.vbounce_alloc {
        if !v.is_finite() {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "超跌抄底占比无效"}))).into_response();
        }
        cfg.vbounce_alloc = v.clamp(0.0, 0.50);
    }
    if cfg.stock_alloc + cfg.vbounce_alloc > 1.0 {
        cfg.vbounce_alloc = (1.0 - cfg.stock_alloc).max(0.0);
    }
    if let Some(v) = body.vol_target_ann {
        if !v.is_finite() {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "目标年化波动无效"}))).into_response();
        }
        cfg.vol_target_ann = v.clamp(0.0, 2.0);
    }
    if let Some(v) = body.stock_min_bars {
        cfg.stock_min_bars = v.clamp(6, 30);
    }
    if let Some(v) = body.max_dd_pct {
        if !v.is_finite() {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "回撤熔断参数无效"}))).into_response();
        }
        cfg.max_dd_pct = v.clamp(0.0, 80.0);
    }
    if let Some(v) = body.maker_wait_sec {
        if !v.is_finite() {
            return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "Maker 等待秒数无效"}))).into_response();
        }
        cfg.maker_wait_sec = v.clamp(1.0, 60.0);
    }
    if let Some(v) = body.taker_only {
        cfg.taker_only = v;
    }
    if let Some(v) = body.armed {
        cfg.armed = v;
    }
    if let Some(v) = body.auto_run {
        cfg.auto_run = v;
    }
    if let Some(v) = body.key_file {
        cfg.key_file = v.trim().to_string();
    }
    if let Some(v) = body.proxy_url {
        cfg.proxy_url = v.trim().to_string();
    }
    if body.clear_keys == Some(true) {
        cfg.api_key.clear();
        cfg.api_secret.clear();
        cfg.armed = false;
        cfg.auto_run = false;
    } else {
        if let Some(v) = body.api_key {
            let t = v.trim();
            if !t.is_empty() {
                cfg.api_key = t.to_string();
            }
        }
        if let Some(v) = body.api_secret {
            let t = v.trim();
            if !t.is_empty() {
                cfg.api_secret = t.to_string();
            }
        }
    }
    if let Err(e) = save_tradfi_config(&path, &cfg) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": format!("保存 TradFi 配置失败: {e}")}))).into_response();
    }
    Json(json!({"ok": true, "config": tradfi_config_public_json(&cfg)})).into_response()
}

async fn run_tradfi_shadow_python(
    state: &AppState,
    cfg: &TradFiConfig,
    extra: &[&str],
) -> anyhow::Result<serde_json::Value> {
    let py = {
        let live_cfg = state.live.lock().await.config.clone();
        if !live_cfg.ml_python.trim().is_empty() && std::path::Path::new(&live_cfg.ml_python).exists() {
            live_cfg.ml_python
        } else {
            "python3".to_string()
        }
    };
    let repo_dir = resolve_tradfi_repo_dir();
    let cfg_path = tradfi_config_path(state);
    let mut cmd = tokio::process::Command::new(&py);
    cmd.arg("-m")
        .arg("tradfi.shadow")
        .arg("--config")
        .arg(&cfg_path)
        .arg("--state-file")
        .arg(&*state.tradfi_path)
        .args(extra)
        .current_dir(&repo_dir);
    if !cfg.proxy_url.trim().is_empty() {
        cmd.env("HTTPS_PROXY", cfg.proxy_url.trim());
        cmd.env("HTTP_PROXY", cfg.proxy_url.trim());
    }
    let out = cmd.output().await.context("启动 tradfi.shadow 失败")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        return Err(anyhow::anyhow!("{detail}"));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).context("解析 TradFi 影子结果失败")?;
    Ok(val)
}

pub(super) async fn tradfi_shadow_rebalance(State(state): State<AppState>) -> Response {
    let cfg = load_tradfi_config(&tradfi_config_path(&state));
    match run_tradfi_shadow_python(&state, &cfg, &["--rebalance-now"]).await {
        Ok(val) => Json(json!({"ok": true, "shadow": val})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": format!("同步并调仓 TradFi 影子失败: {e}")}))).into_response(),
    }
}

pub(super) async fn tradfi_shadow_reset(State(state): State<AppState>) -> Response {
    let cfg = load_tradfi_config(&tradfi_config_path(&state));
    match run_tradfi_shadow_python(&state, &cfg, &["--reset"]).await {
        Ok(val) => Json(json!({"ok": true, "shadow": val})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": format!("重置 TradFi 影子失败: {e}")}))).into_response(),
    }
}

pub(super) async fn tradfi_plan(State(state): State<AppState>) -> Response {
    let cfg = load_tradfi_config(&tradfi_config_path(&state));
    match run_tradfi_python(&state, &cfg, &["--dry-run"]).await {
        Ok(val) => Json(json!({"ok": true, "plan": val})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": format!("计算 TradFi 调仓计划失败: {e}")}))).into_response(),
    }
}

pub(super) async fn tradfi_execute(State(state): State<AppState>) -> Response {
    let _gate = match state.exec_gate.try_lock() {
        Ok(g) => g,
        Err(_) => return (StatusCode::CONFLICT, Json(json!({"ok": false, "error": "有订单操作正在执行，请稍后再试"}))).into_response(),
    };
    let cfg = load_tradfi_config(&tradfi_config_path(&state));
    if !cfg.armed {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "请先将「启用币安实盘」设为开启并保存配置"}))).into_response();
    }
    let (k, s) = resolve_tradfi_creds(&cfg);
    if k.is_empty() || s.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "未读取到有效的币安 API 密钥（请检查本地密钥文件路径）"}))).into_response();
    }
    match run_tradfi_python(&state, &cfg, &["--live"]).await {
        Ok(val) => Json(json!({"ok": true, "result": val})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": format!("执行币安实盘调仓失败: {e}")}))).into_response(),
    }
}

pub(super) async fn tradfi_flatten(State(state): State<AppState>) -> Response {
    let _gate = match state.exec_gate.try_lock() {
        Ok(g) => g,
        Err(_) => return (StatusCode::CONFLICT, Json(json!({"ok": false, "error": "有订单操作正在执行，请稍后再试"}))).into_response(),
    };
    let cfg = load_tradfi_config(&tradfi_config_path(&state));
    if !cfg.armed {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "请先开启「启用实盘」并保存配置后再执行平仓"}))).into_response();
    }
    let (k, s) = resolve_tradfi_creds(&cfg);
    if k.is_empty() || s.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": "未读取到有效的币安 API 密钥"}))).into_response();
    }
    match run_tradfi_python(&state, &cfg, &["--flatten", "--live"]).await {
        Ok(val) => Json(json!({"ok": true, "result": val})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok": false, "error": format!("币安平仓失败: {e}")}))).into_response(),
    }
}

pub async fn run_tradfi_auto_rebalance(state: &AppState) {
    let cfg = load_tradfi_config(&tradfi_config_path(state));
    let (k, s) = resolve_tradfi_creds(&cfg);
    if !(cfg.auto_run && cfg.armed && !k.is_empty() && !s.is_empty()) {
        return;
    }
    let _gate = match state.exec_gate.try_lock() {
        Ok(g) => g,
        Err(_) => {
            tracing::info!("TradFi 自动调仓跳过：已有订单操作在执行中");
            return;
        }
    };
    tracing::info!("TradFi 币安实盘自动调仓开始（杠杆 {:.1}x）", cfg.gross_leverage);
    match run_tradfi_python(state, &cfg, &["--live"]).await {
        Ok(val) => {
            let n = val.get("orders").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            tracing::info!("TradFi 币安实盘自动调仓完成：{n} 笔订单");
        }
        Err(e) => tracing::error!("TradFi 币安实盘自动调仓失败：{e}"),
    }
}


// ---- HL 单账户 carry 影子 ----

pub(super) async fn hlcarry_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let st = state.hlcarry.lock().await.clone();
    Json(json!({
        "state": st,
        "equity": st.equity(),
        "annualized_pct": st.annualized_pct(),
        "funding_total": st.funding_total,
        "basis_total": st.basis_total(),
        "warnings": st.warnings(),
    }))
}


#[derive(serde::Deserialize)]
pub(super) struct HlCarryParamsBody {
    top_k: Option<usize>,
    min_funding_ann_pct: Option<f64>,
    min_spot_volume_usd: Option<f64>,
    max_leg_weight: Option<f64>,
    payback_days: Option<f64>,
    min_hold_hours: Option<f64>,
    basis_take_profit_bp: Option<f64>,
    min_entry_basis_bp: Option<f64>,
}

/// 改选币/换仓参数。只改规则，不动当前持仓——下一次评估才生效。
pub(super) async fn hlcarry_params(
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
    if let Some(v) = body.basis_take_profit_bp {
        st.params.basis_take_profit_bp = v;
    }
    if let Some(v) = body.min_entry_basis_bp {
        st.params.min_entry_basis_bp = v;
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
pub(super) struct HlCarryOpenBody {
    budget_usd: Option<f64>,
}

pub(super) async fn hlcarry_open(State(state): State<AppState>, Json(body): Json<HlCarryOpenBody>) -> Response {
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

pub(super) async fn hlcarry_step(State(state): State<AppState>) -> Response {
    if state.hlcarry.lock().await.start_equity <= 0.0 {
        return (StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "还没有影子账户，请先用「按预算重置模拟」设置预算"})))
            .into_response();
    }
    let coins = state.hlcarry.lock().await.legs.iter().map(|l| l.coin.clone()).collect::<Vec<_>>();
    let market = match crate::hlcarry::fetch_market_with_funding(&state.http, &coins).await {
        Ok(m) => m,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY,
                    Json(json!({"ok": false, "error": format!("行情或资金费历史失败: {e:#}")}))).into_response()
        }
    };
    let now = crate::live::now_ms_pub();
    let mut st = state.hlcarry.lock().await;
    let previous = st.clone();
    st.step(&market, now);
    st.maybe_rebalance(&market, now);
    if let Err(e) = st.save(&state.hlcarry_path) {
        *st = previous;
        return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "error": format!("保存失败: {e:#}")}))).into_response();
    }
    Json(json!({"ok": true, "legs": st.legs.len()})).into_response()
}

// ---- 纸面资金费 carry 策略 ----

// ---- 跨所价差纸面引擎 ----

