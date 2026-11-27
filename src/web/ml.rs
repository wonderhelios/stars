//! HTTP handlers for the Python-predicted / Rust-executed ML portfolio strategy.

use super::AppState;
use anyhow::Context;
use axum::{extract::State, http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;

/// 调 ML 服务里的 Python 脚本，返回 stdout。脚本用绝对路径（ml/ 里的 BASE），
/// 所以当前目录不重要，但 ml/ 目录必须在当前工作目录下（或脚本路径可解析）。
/// 解析 ML 脚本路径：绝对路径直接用；相对路径先试 cwd，再试可执行文件往上几级目录。
fn resolve_ml_script(cfg: &crate::live::LiveConfig, script: &str) -> String {
    let dir = cfg.ml_dir.trim_end_matches('/');
    if dir.is_empty() {
        return script.to_string();
    }
    if dir.starts_with('/') {
        return format!("{dir}/{script}");
    }
    let cwd_path = format!("{dir}/{script}");
    if std::path::Path::new(&cwd_path).exists() {
        return cwd_path;
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut cur = exe.parent().map(|p| p.to_path_buf());
        for _ in 0..4 {
            let Some(p) = cur else { break };
            let cand = p.join(dir).join(script);
            if cand.exists() {
                return cand.to_string_lossy().to_string();
            }
            cur = p.parent().map(|x| x.to_path_buf());
        }
    }
    cwd_path
}

async fn run_ml_python(
    cfg: &crate::live::LiveConfig,
    script: &str,
    extra: &[&str],
) -> anyhow::Result<String> {
    let path = resolve_ml_script(cfg, script);
    let cwd = std::env::current_dir().unwrap_or_default();
    let out = tokio::process::Command::new(&cfg.ml_python)
        .arg(&path)
        .args(extra)
        .current_dir(&cwd)
        .output()
        .await
        .context("启动 ML Python 脚本失败")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        return Err(anyhow::anyhow!("{stderr}"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// ML 策略的「计划调仓」：预测目标持仓 + 计算需要下的单（dry-run，不下单）。
pub(super) async fn ml_plan(State(state): State<AppState>) -> Response {
    let cfg = state.live.lock().await.config.clone();
    let ml_addr = match cfg.ml_vault_address.as_deref() {
        Some(a) if !a.is_empty() => a.to_string(),
        _ => {
            return (StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "未配置 ML 子账户地址（ml_vault_address）"})))
                .into_response();
        }
    };
    // 只调用一次 place.py：它先读取真实旧仓，再应用 buffer11。
    // predict 字段保留给前端兼容，但必须复用实际计划中的同一个 target，
    // 不能再单独调用无持仓 predict.py，否则目标文字与订单会互相矛盾。
    let plan_json = match run_ml_python(
        &cfg,
        "place.py",
        &["--vault", &ml_addr, "--dry-run", "--json"],
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "error": format!("计算调仓失败: {e}")})))
                .into_response();
        }
    };
    let plan: serde_json::Value = match serde_json::from_str(plan_json.trim()) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "error": format!("解析调仓计划失败: {e}")})))
                .into_response();
        }
    };
    let target = plan.get("target").cloned().unwrap_or(json!({}));
    if target.get("preview_without_current_positions").and_then(|v| v.as_bool()) != Some(false)
        || !target.get("buffer").and_then(|v| v.as_i64()).is_some_and(|b| b > 0)
    {
        return (StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok": false, "error": "调仓计划缺少真实持仓 buffer 配置，拒绝展示"})))
            .into_response();
    }
    Json(json!({
        "ok": true,
        "predict": target,
        "plan": plan,
    }))
    .into_response()
}

#[derive(serde::Deserialize, Default)]
struct MlExecuteBody {
    #[serde(default)]
    ioc_only: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct MlExecLeg {
    pub(super) coin: String,
    pub(super) buy: bool,
    pub(super) size: f64,
    pub(super) mid: f64,
    pub(super) sz_dec: i32,
    pub(super) reduce_only: bool,
    pub(super) action: String,
}

fn parse_json_f64(v: Option<&serde_json::Value>) -> f64 {
    match v {
        Some(x) => x
            .as_f64()
            .or_else(|| x.as_str().and_then(|s| s.parse::<f64>().ok()))
            .unwrap_or(0.0),
        None => 0.0,
    }
}

/// 将 `place.py` 输出的订单拆分为两阶段：
/// 1) `closes`（先执行，强制 `reduce_only=true`，含减仓与反手平旧仓部分）；
/// 2) `opens`（后执行，设全仓杠杆后再发，`reduce_only=false`，含新开/加仓/反手开新仓）。
pub(super) fn plan_ml_execution_phases(
    raw_orders: &[serde_json::Value],
    acct: &crate::exchange::Acct,
    markets: &std::collections::HashMap<String, crate::exchange::MarketInfo>,
    mids: &std::collections::HashMap<String, f64>,
) -> (Vec<MlExecLeg>, Vec<MlExecLeg>, Vec<serde_json::Value>) {
    let mut sim_pos: std::collections::HashMap<String, f64> = acct
        .positions
        .iter()
        .map(|(c, p)| (c.clone(), p.size))
        .collect();
    let mut closes = Vec::new();
    let mut opens = Vec::new();
    let mut invalid = Vec::new();

    for o in raw_orders {
        let coin = o.get("coin").and_then(|v| v.as_str()).unwrap_or("").trim();
        let buy = o.get("buy").and_then(|v| v.as_bool()).unwrap_or(false);
        let size = parse_json_f64(o.get("size"));
        let fallback_px = {
            let m = parse_json_f64(o.get("mid"));
            if m.is_finite() && m > 0.0 {
                m
            } else {
                parse_json_f64(o.get("price"))
            }
        };
        let live_mid = mids
            .get(coin)
            .copied()
            .filter(|p| p.is_finite() && *p > 0.0)
            .unwrap_or(fallback_px);
        if coin.is_empty() || !size.is_finite() || size <= 0.0 || !live_mid.is_finite() || live_mid <= 0.0 {
            invalid.push(json!({"coin": coin, "ok": false, "error": "参数无效"}));
            continue;
        }
        let sz_dec = markets.get(coin).map(|m| m.sz_decimals).unwrap_or(5);
        let explicit_ro = o.get("reduce_only").and_then(|v| v.as_bool());
        let action_hint = o
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let cur = sim_pos.get(coin).copied().unwrap_or(0.0);
        let reducing_dir = (cur > 1e-12 && !buy) || (cur < -1e-12 && buy);

        if explicit_ro == Some(true) {
            let sz = crate::exchange::round_size(size, sz_dec);
            if sz > 0.0 {
                closes.push(MlExecLeg {
                    coin: coin.to_string(),
                    buy,
                    size: sz,
                    mid: live_mid,
                    sz_dec,
                    reduce_only: true,
                    action: if action_hint.is_empty() { "reduce".into() } else { action_hint },
                });
                sim_pos.insert(coin.to_string(), cur + if buy { sz } else { -sz });
            }
            continue;
        }

        if reducing_dir {
            let close_sz = crate::exchange::round_size(size.min(cur.abs()), sz_dec);
            let open_sz = crate::exchange::round_size((size - cur.abs()).max(0.0), sz_dec);
            if close_sz > 0.0 {
                let is_full = close_sz + 1e-12 >= cur.abs();
                closes.push(MlExecLeg {
                    coin: coin.to_string(),
                    buy,
                    size: close_sz,
                    mid: live_mid,
                    sz_dec,
                    reduce_only: true,
                    action: if open_sz > 0.0 {
                        "flip_close".into()
                    } else if is_full {
                        "close".into()
                    } else {
                        "reduce".into()
                    },
                });
            }
            if open_sz > 0.0 {
                opens.push(MlExecLeg {
                    coin: coin.to_string(),
                    buy,
                    size: open_sz,
                    mid: live_mid,
                    sz_dec,
                    reduce_only: false,
                    action: "flip_open".into(),
                });
            }
            sim_pos.insert(coin.to_string(), cur + if buy { size } else { -size });
        } else {
            let sz = crate::exchange::round_size(size, sz_dec);
            if sz > 0.0 {
                opens.push(MlExecLeg {
                    coin: coin.to_string(),
                    buy,
                    size: sz,
                    mid: live_mid,
                    sz_dec,
                    reduce_only: false,
                    action: if !action_hint.is_empty() {
                        action_hint
                    } else if cur.abs() > 1e-12 {
                        "increase".into()
                    } else {
                        "open".into()
                    },
                });
                sim_pos.insert(coin.to_string(), cur + if buy { sz } else { -sz });
            }
        }
    }

    (closes, opens, invalid)
}

/// ML 策略的「执行调仓」：Python 只算 orders（不下单），Rust 用已验证的签名下单。
/// 顺序：先平仓/减仓（`reduce_only=true`）→ 显式设置全仓杠杆（min(max_lev, 5x)）→ 再开仓/加仓，
/// 默认走 `maker_then_ioc`（支持可选 `{"ioc_only": true}` 逃生舱）。
pub(super) async fn ml_execute(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let _gate = match state.exec_gate.try_lock() {
        Ok(g) => g,
        Err(_) => {
            return (StatusCode::CONFLICT,
                Json(json!({"ok": false, "error": "订单操作正在执行，请稍后"})))
                .into_response();
        }
    };
    let req_body: MlExecuteBody = if body.iter().all(|b| b.is_ascii_whitespace()) {
        MlExecuteBody::default()
    } else {
        serde_json::from_slice(&body).unwrap_or_default()
    };
    let ioc_only = req_body.ioc_only.unwrap_or(false);

    let cfg = state.live.lock().await.config.clone();
    let ml_addr = match cfg.ml_vault_address.as_deref() {
        Some(a) if !a.is_empty() => a.to_string(),
        _ => {
            return (StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "未配置 ML 子账户地址"})))
                .into_response();
        }
    };

    // 1. Python 只算 orders（dry-run ✗ 不需要私钥）
    let plan_json = match run_ml_python(&cfg, "place.py", &["--vault", &ml_addr, "--dry-run", "--json"]).await {
        Ok(s) => s,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "error": format!("计算 ML 调仓失败: {e}")}))).into_response();
        }
    };
    let plan: serde_json::Value = match serde_json::from_str(plan_json.trim()) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "error": format!("解析 ML 计划失败: {e}")}))).into_response();
        }
    };
    let orders = plan.get("orders").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    if orders.is_empty() {
        return Json(json!({"ok": true, "results": [], "note": "无需调仓"})).into_response();
    }

    // 2. Rust 用 ML 子账户的 Exec 下单（签名已验证 ✗ 和动量同一套）
    let exec = match crate::exchange::Exec::signer(
        &cfg.account, std::path::Path::new(&cfg.key_path), Some(&ml_addr),
    ).await {
        Ok(e) => e,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "error": format!("创建 ML 子账户签名客户端失败: {e}")}))).into_response();
        }
    };
    let markets = match exec.markets().await {
        Ok(m) => m,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "error": format!("读取市场信息失败: {e}")}))).into_response();
        }
    };
    let acct = match exec.account().await {
        Ok(a) => a,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "error": format!("读取 ML 账户持仓失败: {e}")}))).into_response();
        }
    };
    let mids = exec.all_mids().await.unwrap_or_default();
    let (closes, opens, invalid) = plan_ml_execution_phases(&orders, &acct, &markets, &mids);

    let mut results = invalid;
    let mut errors = results.len();
    let mut leverage_notes: Vec<String> = Vec::new();

    // 阶段 1：先执行全部平仓与减仓（reduce_only = true）
    for leg in &closes {
        let res = if ioc_only {
            exec.ioc(&leg.coin, leg.buy, true, leg.size, leg.mid, cfg.slippage, leg.sz_dec)
                .await
                .map(|st| crate::exchange::describe(&st))
        } else {
            exec.maker_then_ioc(&leg.coin, leg.buy, true, leg.size, leg.mid, cfg.slippage, leg.sz_dec, 12)
                .await
        };
        match res {
            Ok(st) => results.push(json!({
                "coin": leg.coin,
                "buy": leg.buy,
                "size": leg.size,
                "reduce_only": true,
                "action": leg.action,
                "status": st,
            })),
            Err(e) => {
                results.push(json!({
                    "coin": leg.coin,
                    "buy": leg.buy,
                    "size": leg.size,
                    "reduce_only": true,
                    "action": leg.action,
                    "ok": false,
                    "error": format!("{e}"),
                }));
                errors += 1;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }

    // 阶段 2：重读账户并为即将开仓/加仓的币显式设置全仓杠杆（min(max_leverage, 5x)）
    if !opens.is_empty() {
        let after = exec.account().await.unwrap_or(acct);
        let mut open_coins: Vec<String> = opens.iter().map(|o| o.coin.clone()).collect();
        open_coins.sort();
        open_coins.dedup();
        for coin in &open_coins {
            // 已有逐仓仓位时 HL 不允许原地切模式，仅对空仓或已是全仓的币设杠杆
            let can_set = after
                .positions
                .get(coin)
                .map(|p| p.size.abs() <= 1e-12 || p.is_cross)
                .unwrap_or(true);
            if !can_set {
                leverage_notes.push(format!("{coin}: 仍有逐仓持仓，跳过全仓杠杆设置"));
                continue;
            }
            let max_lev = markets.get(coin).map(|m| m.max_leverage).unwrap_or(5);
            let lev = 5u32.min(max_lev.max(1)).max(1);
            if let Err(e) = exec.set_leverage(coin, lev).await {
                leverage_notes.push(format!("{coin}: 设置 {lev}x 全仓杠杆失败: {e}"));
            }
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        }
    }

    // 阶段 3：刷新实时 mid 后执行开仓与加仓（reduce_only = false）
    let fresh_mids = if !opens.is_empty() {
        exec.all_mids().await.unwrap_or(mids)
    } else {
        mids
    };
    for leg in &opens {
        let live_mid = fresh_mids
            .get(&leg.coin)
            .copied()
            .filter(|p| p.is_finite() && *p > 0.0)
            .unwrap_or(leg.mid);
        let res = if ioc_only {
            exec.ioc(&leg.coin, leg.buy, false, leg.size, live_mid, cfg.slippage, leg.sz_dec)
                .await
                .map(|st| crate::exchange::describe(&st))
        } else {
            exec.maker_then_ioc(&leg.coin, leg.buy, false, leg.size, live_mid, cfg.slippage, leg.sz_dec, 12)
                .await
        };
        match res {
            Ok(st) => results.push(json!({
                "coin": leg.coin,
                "buy": leg.buy,
                "size": leg.size,
                "reduce_only": false,
                "action": leg.action,
                "status": st,
            })),
            Err(e) => {
                results.push(json!({
                    "coin": leg.coin,
                    "buy": leg.buy,
                    "size": leg.size,
                    "reduce_only": false,
                    "action": leg.action,
                    "ok": false,
                    "error": format!("{e}"),
                }));
                errors += 1;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }

    Json(json!({
        "ok": errors == 0,
        "errors": errors,
        "mode": if ioc_only { "ioc_only" } else { "maker_then_ioc" },
        "leverage_notes": leverage_notes,
        "results": results,
    }))
    .into_response()
}

/// 清空 ML 子账户 B 的净值历史 ✗ 不动动量的记录。
/// 用途：ML 平仓后调整初始资金（比如改成 $2500）✗ 清空后【立即用当前净值记一个点】当作新基准，
/// 这样「全部账户」的本金基准（动量 baseline + ML 首点）才不会少掉 ML 那一份。
pub(super) async fn ml_reset(State(state): State<AppState>) -> Response {
    let cfg = state.live.lock().await.config.clone();
    let ml_addr = match cfg.ml_vault_address.as_deref().filter(|a| !a.is_empty()) {
        Some(a) => a.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "未配置 ML 子账户地址"})),
            )
                .into_response();
        }
    };
    // 先查当前净值（清空后作为新基准 ✗ 让「全部账户」统计保持正确）
    let snap = crate::live::ml_account_snapshot(&ml_addr, None).await;
    if let Some(e) = snap.error {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok": false, "error": format!("读取 ML 净值失败: {e}")})),
        )
            .into_response();
    }
    let now = crate::live::now_ms_pub();
    let mut st = state.live.lock().await;
    st.ml_history.clear();
    st.ml_capital_flows.clear();
    if snap.equity.is_finite() && snap.equity > 0.0 {
        st.ml_history.push(crate::live::EquityPoint { ts: now, equity: snap.equity, pnl: 0.0 });
    }
    if let Err(e) = st.save(&state.live_path) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok": false, "error": format!("保存 ML 记录失败: {e}")})),
        )
            .into_response();
    }
    Json(json!({
        "ok": true,
        "baseline": snap.equity,
        "note": format!("ML 净值历史已清空，新基准 = 当前净值 ${:.2}", snap.equity),
    }))
    .into_response()
}

