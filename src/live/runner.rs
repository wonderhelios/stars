//! Live account snapshots, daily momentum rebalance runner, volatility sizing, and equity recorder.

use super::{
    adjusted_history, cancel_our_take_profits, daily_execution, index_history, ml_index_history,
    now_ms_pub, place_take_profits, CapitalBaseline, EquityPoint, LivePosition,
    LiveRecord, LiveSnapshot, LiveState, MlAccountSnapshot,
};
use crate::exchange::{Exec, MarketInfo};
use crate::hl::maintenance_margin_rate;
use crate::trader;
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;

/// Read-only view of the live account: positions, exposure and liquidation risk.
pub async fn snapshot(
    state: &LiveState,
    markets: &HashMap<String, MarketInfo>,
    http: Option<reqwest::Client>,
) -> LiveSnapshot {
    let cfg = state.config.clone();
    // ---- 胜率（三个维度 ✗ 口径写清楚 ✗）----
    //
    // ① win_rate_days  : 按【日】统计 ✗ 当日收盘净值 > 前一日收盘 ✓
    // ② win_rate_months: 按【月】统计 ✗ 当月净值 > 上月末 ✓
    // ③ win_rate_trades: 按【平仓记录】统计 ✗ pnl > 0 ✓
    //    （pnl 来自交易所回执 ✗ 已含手续费 ✓ 但同一仓位可能分多次平 ✓ 是按笔不算按仓 ✗）
    let (wr_days, wr_months, wr_trades, n_days, n_months, n_trades) = {
        // 按日取最后一点
        let adjusted = adjusted_history(state);
        let mut by_day: std::collections::BTreeMap<i64, f64> = std::collections::BTreeMap::new();
        for (ts, eq) in adjusted.iter() {
            by_day.insert(ts / 86_400_000, *eq);
        }
        let days: Vec<f64> = by_day.values().copied().collect();
        let mut up_days = 0usize;
        for i in 1..days.len() {
            if days[i] > days[i - 1] {
                up_days += 1;
            }
        }
        let n_days = days.len().saturating_sub(1);
        let wr_days = (n_days > 0).then(|| up_days as f64 / n_days as f64 * 100.0);

        // 按月：每个自然月最后一个点（真实年月分组 ✗ 不用 30 天近似 ✓）
        let mut by_month: std::collections::BTreeMap<i64, f64> = std::collections::BTreeMap::new();
        for (&day, &eq) in by_day.iter() {
            by_month.insert(super::metrics::month_ordinal(day), eq);
        }
        let months: Vec<f64> = by_month.values().copied().collect();
        let mut up_months = 0usize;
        for i in 1..months.len() {
            if months[i] > months[i - 1] {
                up_months += 1;
            }
        }
        let n_months = months.len().saturating_sub(1);
        let wr_months = (n_months > 0).then(|| up_months as f64 / n_months as f64 * 100.0);

        // 按平仓记录
        let closes: Vec<f64> = state
            .records
            .iter()
            .filter(|r| r.reduce_only)
            .filter_map(|r| r.pnl)
            .collect();
        let n_trades = closes.len();
        let wr_trades = (n_trades > 0).then(|| {
            closes.iter().filter(|p| **p > 0.0).count() as f64 / n_trades as f64 * 100.0
        });
        (wr_days, wr_months, wr_trades, n_days, n_months, n_trades)
    };

    let mut snap = LiveSnapshot {
        win_rate_days: wr_days,
        win_rate_months: wr_months,
        win_rate_trades: wr_trades,
        win_samples_days: n_days,
        win_samples_months: n_months,
        win_samples_trades: n_trades,
        capital_baseline: state.baseline(),
        configured: cfg.can_sign(),
        error: None,
        account: cfg.account.clone(),
        equity: 0.0,
        positions: Vec::new(),
        gross_notional: 0.0,
        net_notional: 0.0,
        margin_used: 0.0,
        maintenance_margin: 0.0,
        liq_buffer_pct: 0.0,
        nearest_liq_pct: None,
        isolated_count: 0,
        tp_orders: Vec::new(),
        tp_oids: state.tp_oids.clone(),
        start_equity: 0.0,
        cumulative_pnl: 0.0,
        shadow: Vec::new(),
        unpriced: Vec::new(),
        net_deposit: 0.0,
        capital_flows: Vec::new(),
        capital_alloc: state.capital_alloc.clone(),
        exec_daily: Vec::new(),
        tp_ref: HashMap::new(),
        config: cfg.clone(),
        history: state.history.clone(),
        ml_history: state.ml_history.clone(),
        ml_capital_flows: state.ml_capital_flows.clone(),
        ml_net_deposit: state.ml_capital_flows.iter().filter(|f| f.amount.is_finite()).map(|f| f.amount).sum(),
        mom_index_history: index_history(state),
        ml_index_history: ml_index_history(state),
        records: state.records.clone(),
        last_run_at: state.last_run_at,
        last_plan: state.last_plan.clone(),
        last_live: state.last_live,
        ml_account: None,
    };
    if cfg.account.is_empty() {
        snap.error = Some("未配置账户地址".into());
        return snap;
    }
    // 有子账户（vault）就查子账户，否则查主账户。
    let query_addr = cfg.vault_address.as_deref().unwrap_or(&cfg.account);
    let exec_res = match http {
        Some(h) => Exec::reader_shared(h, Some(query_addr)).await,
        None => Exec::reader_for(Some(query_addr)).await,
    };
    let exec = match exec_res {
        Ok(e) => e,
        Err(e) => {
            snap.error = Some(format!("{e}"));
            return snap;
        }
    };
    let acct = match exec.account().await {
        Ok(a) => a,
        Err(e) => {
            snap.error = Some(format!("读取账户失败: {e}"));
            return snap;
        }
    };
    snap.equity = acct.equity;
    let coins: Vec<String> = acct.positions.keys().cloned().collect();
    let mids = trader::fetch_mids(&exec, &coins).await;
    let mut unpriced: Vec<String> = Vec::new();

    for (coin, pos) in acct.positions.iter() {
        // 拿不到中间价时**绝不能退回开仓价** —— 那会把未实现盈亏算成 0，
        // 一个正在亏损的退市仓位会显示成持平，虚增净值和盈亏。改用交易所
        // 报的市值反推标记价（positionValue = |size| × mark），它永远存在。
        let (mark, price_known) = match mids.get(coin).copied() {
            Some(m) if m > 0.0 => (m, true),
            _ => {
                let known = pos.size.abs() > 1e-12 && pos.position_value > 0.0;
                let m = if known {
                    pos.position_value / pos.size.abs()
                } else {
                    pos.entry_px
                };
                // 只有**连交易所市值都没有**才算真的定价不了。
                if !known {
                    unpriced.push(coin.clone());
                }
                (m, known)
            }
        };
        let notional = (pos.size * mark).abs();
        // 有中间价就自己算（口径统一），否则直接用交易所的权威值。
        let unrealized = if price_known {
            pos.size * (mark - pos.entry_px)
        } else {
            pos.unrealized_pnl
        };
        let dist = pos.liq_px.and_then(|liq| {
            if mark > 0.0 && liq > 0.0 {
                Some(if pos.size > 0.0 {
                    (mark - liq) / mark * 100.0
                } else {
                    (liq - mark) / mark * 100.0
                })
            } else {
                None
            }
        });
        snap.gross_notional += notional;
        snap.net_notional += pos.size * mark;
        if let Some(m) = markets.get(coin.as_str()) {
            snap.maintenance_margin += notional * maintenance_margin_rate(m.max_leverage);
        }
        if let Some(d) = dist {
            snap.nearest_liq_pct = Some(snap.nearest_liq_pct.map_or(d, |x: f64| x.min(d)));
        }
        snap.positions.push(LivePosition {
            coin: coin.clone(),
            side: if pos.size > 0.0 { "long".into() } else { "short".into() },
            size: pos.size,
            entry_px: pos.entry_px,
            mark_px: mark,
            notional,
            unrealized,
            liq_px: pos.liq_px,
            dist_pct: dist,
            is_cross: pos.is_cross,
            leverage: pos.leverage,
        });
    }
    snap.positions.sort_by(|a, b| a.coin.cmp(&b.coin));
    snap.isolated_count = snap.positions.iter().filter(|p| !p.is_cross).count();
    snap.cumulative_pnl = state.unrealized_pnl(&snap.positions);
    // Missing historical equity remains unknown until an observation is persisted.
    snap.start_equity = state.baseline().map(|b| b.equity).unwrap_or(0.);
    snap.net_deposit = state.net_deposit;
    snap.capital_flows = state.capital_flows.clone();
    snap.exec_daily = daily_execution(&state.records, &state.history, snap.equity);
    // 不按 reduceOnly 过滤：该字段在 openOrders 里不一定存在，过滤会导致
    // 表格永远是空的。程序只挂只减仓单，所以全部展示即可。
    if let Ok(orders) = exec.open_order_details().await {
        snap.tp_orders = orders;
    }
    snap.margin_used = if cfg.effective_leverage() > 0.0 {
        snap.gross_notional / cfg.effective_leverage()
    } else {
        0.0
    };
    snap.liq_buffer_pct = if snap.equity > 0.0 {
        ((snap.equity - snap.maintenance_margin) / snap.equity * 100.0).max(0.0)
    } else {
        0.0
    };
    snap.unpriced = unpriced;
    snap
}

/// 查询 ML 策略子账户（第二个子账户）的净值/仓位，只读。
/// 与动量子账户并排展示 + 合计总资金。
pub async fn ml_account_snapshot(
    ml_vault_address: &str,
    http: Option<reqwest::Client>,
) -> MlAccountSnapshot {
    let mut out = MlAccountSnapshot {
        account: ml_vault_address.to_string(),
        equity: 0.0,
        positions: Vec::new(),
        error: None,
    };
    let exec = match http {
        Some(h) => Exec::reader_shared(h, Some(ml_vault_address)).await,
        None => Exec::reader_for(Some(ml_vault_address)).await,
    };
    let exec = match exec {
        Ok(e) => e,
        Err(e) => {
            out.error = Some(format!("{e}"));
            return out;
        }
    };
    let acct = match exec.account().await {
        Ok(a) => a,
        Err(e) => {
            out.error = Some(format!("读取 ML 子账户失败: {e}"));
            return out;
        }
    };
    out.equity = acct.equity;
    let coins: Vec<String> = acct.positions.keys().cloned().collect();
    let mids = trader::fetch_mids(&exec, &coins).await;
    for (coin, pos) in acct.positions.iter() {
        let mark = mids
            .get(coin)
            .copied()
            .filter(|m| *m > 0.0)
            .unwrap_or_else(|| {
                if pos.size.abs() > 1e-12 && pos.position_value > 0.0 {
                    pos.position_value / pos.size.abs()
                } else {
                    pos.entry_px
                }
            });
        let notional = (pos.size * mark).abs();
        let unrealized = if mark > 0.0 {
            pos.size * (mark - pos.entry_px)
        } else {
            pos.unrealized_pnl
        };
        out.positions.push(LivePosition {
            coin: coin.clone(),
            side: if pos.size > 0.0 { "long".into() } else { "short".into() },
            size: pos.size,
            entry_px: pos.entry_px,
            mark_px: mark,
            notional,
            unrealized,
            liq_px: pos.liq_px,
            dist_pct: pos.liq_px.and_then(|liq| {
                if mark > 0.0 && liq > 0.0 {
                    Some(if pos.size > 0.0 {
                        (mark - liq) / mark * 100.0
                    } else {
                        (liq - mark) / mark * 100.0
                    })
                } else {
                    None
                }
            }),
            is_cross: pos.is_cross,
            leverage: pos.leverage,
        });
    }
    out
}

#[derive(Serialize)]
pub struct RunResult {
    pub aborted: bool,
    pub equity: f64,
    pub long_leg: Vec<String>,
    pub short_leg: Vec<String>,
    pub per_coin: f64,
    pub plan_lines: Vec<String>,
    /// 本次调仓使用的参考价，供上层持久化（止盈幅度相对它计算）。
    pub tp_ref: Option<HashMap<String, f64>>,
    /// 本次实际使用的波动目标缩放系数（供上层持久化 ✗ 下次死区要用 ✓）
    pub vol_k: Option<f64>,
    pub executed: Vec<String>,
    pub live: bool,
    /// 本次挂出的止盈单 oid，供上层写入状态（对账时标记「止盈成交」）。
    pub tp_oids: Vec<u64>,
    /// 本次触发止损的币，以及解禁时间（供上层写进冷却表 ✗）
    pub stopped: Vec<(String, i64)>,
}

pub(super) fn ensure_candidates(weights: &[(String, f64)]) -> Result<()> {
    if !weights.is_empty() {
        return Ok(());
    }
    anyhow::bail!("流动性过滤后没有候选")
}

pub(super) fn signal_panel(store: &crate::store::Store) -> Result<Vec<crate::store::PanelEntry>> {
    trader::load_panel(store)
}

/// Compute the target book and, when `live` is true and the config is armed,
/// send the orders. Ranking is identical to the paper engine.
pub async fn run(
    store: &crate::store::Store,
    state: &LiveState,
    markets: &HashMap<String, MarketInfo>,
    live: bool,
) -> Result<(RunResult, Vec<LiveRecord>)> {
    let cfg = state.config.clone();
    anyhow::ensure!(!cfg.account.is_empty(), "未配置账户地址");
    if live {
        // A dry run is read-only, so only real trading needs the signing key.
        anyhow::ensure!(!cfg.key_path.is_empty(), "未配置 API 钱包密钥路径");
        anyhow::ensure!(cfg.armed, "实盘未启用（需先打开「启用实盘」开关）");
    }

    let exec = if live {
        Exec::signer_config(&cfg).await?
    } else {
        Exec::reader_config(&cfg).await?
    };
    let tc = cfg.trade_config();

    let panel = signal_panel(store)?;
    anyhow::ensure!(
        panel.len() >= 20,
        "K 线缓存不足（{} 币）—— 请确认对应信号源的日线缓存已就绪",
        panel.len()
    );
    let acct0 = exec.account().await?;
    let (weights, liquid) = trader::target_weights(&panel, &tc, acct0.equity, crate::live::now_ms_pub() as i64);
    ensure_candidates(&weights)?;
    // ---- 波动率目标：把组合日波动控制在 vol_target 附近 ----
    //
    // 回测（2024-07 起 843 天 ✗ 纯动量）: 加 2%/天的波动目标后
    //   Sharpe 2.50 → 2.68 ✗ 月胜率 64% → 82% ✗ 3 个月亏损率 19% → 8%
    //   平均有效杠杆约 1.8x（波动大时自动减到 0.2x ✗ 小时加到 3x ✗）
    let mut weights = weights;
    let mut vol_lines: Vec<String> = Vec::new();
    let mut applied_k: Option<f64> = None;
    if cfg.vol_target > 0.0 {
        let effective_gross_lev = cfg.effective_leverage() * cfg.margin_buffer;
        if let Some(vol) = book_vol_estimate(&panel, &weights, effective_gross_lev) {
            let mut k = (cfg.vol_target / vol).clamp(0.2, 3.0);
            let prev_k = state.vol_k;
            // 死区：跟【上次实际用的缩放系数】比 ✗ 不跟"当前杠杆"比 ✓
            // 因为入金会让当前杠杆被动变小 ✗ 拿它做基准会一直挡住新资金 ✓
            if prev_k > 0.0 && cfg.vol_band > 0.0 && (k / prev_k - 1.0).abs() < cfg.vol_band {
                vol_lines.push(format!(
                    "波动目标：新系数 {:.2} 与上次 {:.2} 相差 <{:.0}% → 沿用上次系数（死区 ✗ 省手续费）",
                    k, prev_k, cfg.vol_band * 100.0
                ));
                k = prev_k;
            }
            applied_k = Some(k);
            for (_, w) in weights.iter_mut() {
                *w *= k;
            }
            vol_lines.push(format!(
                "波动目标 {:.1}%/天：预估组合日波动 {:.2}% → 仓位 ×{:.2}（实际杠杆 {:.2}x = {}x × 缓冲 {} × {:.2}）",
                cfg.vol_target * 100.0,
                vol * 100.0,
                k,
                cfg.effective_leverage() * cfg.margin_buffer * k,
                cfg.effective_leverage(),
                cfg.margin_buffer,
                k
            ));
        } else {
            vol_lines.push("波动目标：样本不足，按原杠杆执行".into());
        }
    }

    let acct = exec.account().await?;
    let mut coins: Vec<String> = weights.iter().map(|(c, _)| c.clone()).collect();
    coins.extend(acct.positions.keys().cloned());
    coins.sort();
    coins.dedup();
    let mids = trader::fetch_mids(&exec, &coins).await;

    // ---- 冷却过滤（先做 ✗ 后面的止损要能覆盖它）+ 单币止损 ----
    //
    // 回测（833 天 ✗ 日频中性 ✗ 14 日动量）: -25% 单币止损 + 5 天冷却
    //   Sharpe 1.32 → 1.51 ✗ 最大回撤 60.6% → 42.4%
    // 分区间看它是"分 regime"的：趋势期会砍掉会回来的票（2.51 → 2.06）✗
    // 但单边下跌期救命（0.00 → 0.82）✓ 用一点上行换生存 ✓
    let mut weights = weights;
    let mut stop_lines: Vec<String> = Vec::new();
    let now_ms = crate::live::now_ms_pub();

    // ① 冷却中的币：不许重新开仓 ✗ 已持有的维持原仓（不强制平 ✗）
    // 止损关闭时冷却一并失效 ✗ 否则关掉止损后旧冷却还会冻住仓位 ✗
    if cfg.position_stop_pct > 0.0 {
        // 权重口径: w=1 ⇔ 名义 = equity × margin_buffer × leverage
        // （build_plan 里 per_coin = deployable × leverage / n ✗ Σ|w|=1 ✗）
        let gross_unit = (acct.equity * cfg.margin_buffer * cfg.effective_leverage()).max(1.0);
        let mut cooled: Vec<(String, f64)> = Vec::new();
        for (coin, until) in state.stop_cooldowns.iter() {
            if *until <= now_ms {
                continue;
            }
            weights.retain(|(c, _)| c != coin);
            if let Some(pos) = acct.positions.get(coin) {
                if let Some(&mark) = mids.get(coin) {
                    if mark > 0.0 {
                        let cur = pos.size.abs() * mark / gross_unit * pos.size.signum();
                        cooled.push((coin.clone(), cur));
                    }
                }
            }
        }
        for (c, w) in cooled {
            weights.push((c, w));
        }
    }

    // ② 单币止损：逆向浮亏超过阈值 → 目标清零（必须放在冷却之后 ✗ 让它覆盖 ✗）
    let gross_before: f64 = weights.iter().map(|(_, w)| w.abs()).sum();
    if cfg.position_stop_pct > 0.0 {
        for (coin, pos) in acct.positions.iter() {
            let Some(&mark) = mids.get(coin) else { continue };
            if mark <= 0.0 || pos.entry_px <= 0.0 {
                continue;
            }
            let mv = (mark / pos.entry_px - 1.0) * pos.size.signum();
            if mv < -cfg.position_stop_pct {
                weights.retain(|(c, _)| c != coin);
                weights.push((coin.clone(), 0.0));
                stop_lines.push(format!(
                    "{coin} {} 浮亏 {:.1}% < -{:.0}% → 平仓并冷却 {} 天",
                    if pos.size > 0.0 { "多" } else { "空" },
                    mv * 100.0,
                    cfg.position_stop_pct * 100.0,
                    cfg.stop_cooldown_days
                ));
            }
        }
    }

    // ②b 止损后重新归一化 ✗ 两个目标:
    //   ① 总敞口回到止损前（否则少了一个仓位 ✗ 会整体缩水 ✓）
    //   ② 多腿和空腿的名义【各自相等】（否则 6 多 vs 7 空 ✗ 变成净空头 ✗）
    if !stop_lines.is_empty() {
        let long_gross: f64 = weights.iter().filter(|(_, w)| *w > 0.0).map(|(_, w)| w.abs()).sum();
        let short_gross: f64 = weights.iter().filter(|(_, w)| *w < 0.0).map(|(_, w)| w.abs()).sum();
        if long_gross > 1e-12 && short_gross > 1e-12 && gross_before > 1e-12 {
            // 每条腿的目标毛额 = 总目标的一半
            let leg_target = gross_before / 2.0;
            let kl = leg_target / long_gross;
            let ks = leg_target / short_gross;
            for (_, w) in weights.iter_mut() {
                if *w > 0.0 {
                    *w *= kl;
                } else if *w < 0.0 {
                    *w *= ks;
                }
            }
            stop_lines.push(format!(
                "止损后重新归一化：多腿 ×{kl:.3} ✗ 空腿 ×{ks:.3}（两侧名义拉平 ✗ 保持中性）"
            ));
        }
    }

    // ③ 触发止损的币 → 写入冷却（解禁时间 = 现在 + N 天）
    let mut stopped_now: Vec<(String, i64)> = Vec::new();
    if cfg.position_stop_pct > 0.0 && cfg.stop_cooldown_days > 0 {
        let until = now_ms + cfg.stop_cooldown_days * 86_400_000;
        for coin in acct.positions.keys() {
            let Some(&mark) = mids.get(coin) else { continue };
            let pos = &acct.positions[coin];
            if pos.entry_px <= 0.0 || mark <= 0.0 {
                continue;
            }
            if (mark / pos.entry_px - 1.0) * pos.size.signum() < -cfg.position_stop_pct {
                stopped_now.push((coin.clone(), until));
            }
        }
    }

    // ④a 总敞口硬上限（在单币上限、计划展示与调仓后中性化之前统一作用到 weights）
    let mut cap_lines: Vec<String> = Vec::new();
    let hard_cap_lev = cfg.hard_gross_cap.clamp(0.5, 5.0);
    if let Some((raw_lev, scale)) = trader::apply_hard_gross_cap(
        &mut weights,
        cfg.margin_buffer,
        cfg.effective_leverage(),
        hard_cap_lev,
    ) {
        cap_lines.push(format!(
            "硬杠杆上限 {:.2}x：目标总敞口 {:.2}x 触发封顶，等比例缩放 ×{:.3}",
            hard_cap_lev, raw_lev, scale
        ));
    }

    // ④b 单币目标权重上限（作用在 vol_target 缩放、止损归一化与硬杠杆上限之后的最终目标权重上）
    if cfg.max_single_weight > 0.0 {
        let clipped = trader::cap_weights_same_side(
            &mut weights,
            cfg.max_single_weight,
            cfg.margin_buffer,
            cfg.effective_leverage(),
        );
        if !clipped.is_empty() {
            cap_lines.push(format!(
                "单币上限 {:.0}%：截断 {} 并在同侧等杠杆再分配",
                cfg.max_single_weight * 100.0,
                clipped.join(" ")
            ));
        }
    }

    let plan = trader::build_plan(&weights, &acct, markets, &mids, &tc, None);

    let mut plan_lines = vec![format!(
        "流动宇宙 {} 币 · 多头腿 {} · 空头腿 {} · 账户净值 ${:.2} · 每仓 ${:.2} · 杠杆 {:.0}x · 缓冲 {:.0}%",
        liquid.len(),
        weights.iter().filter(|(_, w)| *w > 0.0).count(),
        weights.iter().filter(|(_, w)| *w < 0.0).count(),
        plan.equity,
        plan.per_coin,
        cfg.effective_leverage(),
        cfg.margin_buffer * 100.0
    )];
    plan_lines.extend(vol_lines.clone());
    plan_lines.extend(stop_lines.clone());
    plan_lines.extend(cap_lines.clone());
    // 腿的名单**按权重强弱排序**并显示权重值。
    //
    // 原来直接用 Vec 的插入顺序 —— 那是三因子合并后的任意顺序，看不出谁强谁弱。
    // 用户想知道"为什么某个币被做空、它排第几"，光有名字是答不了的。
    {
        let wmap: std::collections::HashMap<&str, f64> =
            weights.iter().map(|(c, w)| (c.as_str(), *w)).collect();
        let legs = |names: &[String]| -> String {
            let mut v: Vec<(&String, f64)> = names
                .iter()
                .map(|c| (c, wmap.get(c.as_str()).copied().unwrap_or(0.0)))
                .collect();
            // 做多按权重从大到小，做空按 |权重| 从大到小 —— 最强的排最前
            v.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap_or(std::cmp::Ordering::Equal));
            v.iter()
                .map(|(c, w)| format!("{c}({:.3})", w.abs()))
                .collect::<Vec<_>>()
                .join(" ")
        };
    plan_lines.push("信号源: Hyperliquid".into());
    plan_lines.push(format!("多头腿（按权重降序）: {}", legs(&plan.long_leg)));
        plan_lines.push(format!("空头腿（按权重降序）: {}", legs(&plan.short_leg)));
    }
    for o in &plan.orders {
        plan_lines.push(format!(
            "{} {} {} {:.6} @≈{:.6} (${:.2}) · {}",
            if o.buy { "买入" } else { "卖出" },
            o.coin,
            if o.reduce_only { "只减仓" } else { "开仓" },
            o.size,
            o.mid,
            o.notional,
            o.reason
        ));
    }
    for n in &plan.notes {
        plan_lines.push(format!("注意: {n}"));
    }
    if !plan.held.is_empty() {
        plan_lines.push(format!(
            "注意: {} 个已有仓位无法切换保证金模式（Hyperliquid 不允许持仓时切换），\
             如需全部转为全仓请用「转为全仓」逐币处理",
            plan.held.len()
        ));
    }
    if plan.orders.is_empty() {
        plan_lines.push("无需调仓（已在目标状态）".into());
    }

    let now = now_ms_pub();
    let mut records = Vec::new();
    for o in &plan.orders {
        // 减仓/平仓的单子带上原持仓的入场价，前端据此算这笔的已实现盈亏
        let entry_px = if o.reduce_only {
            acct.positions.get(&o.coin).map(|p| p.entry_px)
        } else {
            None
        };
        records.push(LiveRecord {
            pnl: None,
            tid: None,
            ts: now,
            coin: o.coin.clone(),
            side: if o.buy { "买".into() } else { "卖".into() },
            action: o.reason.clone(),
            size: o.size,
            price: o.mid,
            notional: o.notional,
            result: if live { "已发送".into() } else { "计划".into() },
            live,
            entry_px,
            reduce_only: o.reduce_only,
        });
    }

    // 调仓前先撤掉自己的旧止盈单。必须在 execute 之前：否则昨天的止盈单会在
    // 调仓过程中成交，把计划要减的仓位提前平掉，之后的减仓单被拒、开仓单照发。
    //
    // 失败必须中止，不能只记录：撤不掉的旧单会在新单之外继续存在，同一个仓位上
    // 出现两张只减仓单（交易所只按仓位校验，不会去重）。
    if live {
        match cancel_our_take_profits(&exec).await {
            Ok(n) if n > 0 => plan_lines.push(format!("撤销旧止盈单 {n} 个")),
            Ok(_) => {}
            Err(e) => {
                return Err(e.context("撤销旧止盈单失败，已在调仓前中止"));
            }
        }
    }

    let outcome = trader::execute(&exec, &plan, &tc, markets, live).await?;
    // 前置日志（设杠杆失败等）单独放在最前面，绝不与订单结果混用下标
    for line in &outcome.prelim {
        plan_lines.push(format!("注意: {line}"));
    }
    // 逐条按同一下标对应，保证失败信息挂在正确的币上
    for (i, line) in outcome.orders.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        if let Some(r) = records.get_mut(i) {
            r.result = line.clone();
        }
    }
    let aborted = outcome.aborted;
    let executed = outcome.orders;

    // 调仓后拉取最新实时 mids，并用 maker_then_ioc（reduce_only=true）做目标感知中性化，
    // 避免使用调仓前陈旧价格和 100% taker IOC。
    if live && !aborted {
        match exec.account().await {
            Ok(after) => {
                let after_coins: Vec<String> = after.positions.keys().cloned().collect();
                let mut neut_mids = mids.clone();
                for (c, p) in trader::fetch_mids(&exec, &after_coins).await {
                    if p.is_finite() && p > 0.0 {
                        neut_mids.insert(c, p);
                    }
                }
                let neut_equity = if after.equity.is_finite() && after.equity > 0.0 {
                    after.equity
                } else {
                    plan.equity
                };
                let target_notionals: HashMap<String, f64> = weights
                    .iter()
                    .map(|(c, w)| {
                        (c.clone(), w * neut_equity * cfg.margin_buffer * cfg.effective_leverage())
                    })
                    .collect();
                let neutral = trader::neutralize_orders_with_targets(
                    &after,
                    markets,
                    &neut_mids,
                    0.02,
                    10.0,
                    Some(&target_notionals),
                );
                for o in neutral {
                    let entry_px = after.positions.get(&o.coin).map(|p| p.entry_px);
                    let result = match exec
                        .maker_then_ioc(&o.coin, o.buy, true, o.size, o.mid, cfg.slippage, o.sz_decimals, 12)
                        .await
                    {
                        Ok(st) => st,
                        Err(e) => format!("中性化下单失败: {e}"),
                    };
                    plan_lines.push(format!(
                        "{} {} 只减仓 {:.6} @≈{:.6} (${:.2}) · 目标感知中性化",
                        if o.buy { "买入" } else { "卖出" },
                        o.coin,
                        o.size,
                        o.mid,
                        o.notional,
                    ));
                    records.push(LiveRecord {
                        pnl: None,
                        tid: None,
                        ts: now_ms_pub(),
                        coin: o.coin.clone(),
                        side: if o.buy { "买".into() } else { "卖".into() },
                        action: "目标感知中性化".into(),
                        size: o.size,
                        price: o.mid,
                        notional: o.notional,
                        result,
                        live: true,
                        entry_px,
                        reduce_only: true,
                    });
                }
            }
            Err(e) => plan_lines.push(format!("注意: 调仓后重读账户失败，跳过中性化: {e}")),
        }
    }

    let mut tp_ref_out: HashMap<String, f64> = HashMap::new();
    let mut tp_oids_out: Vec<u64> = Vec::new();
    if live && !aborted {
        // 基准价用本次调仓的目标币中间价，而不是挂单时的实时价 ——
        // 后者会让手动「刷新止盈单」把止盈线随行情一起搬走。
        let tp_ref: HashMap<String, f64> = plan
            .long_leg
            .iter()
            .chain(plan.short_leg.iter())
            .filter_map(|c| mids.get(c).map(|m| (c.clone(), *m)))
            .collect();
        tp_ref_out = tp_ref.clone();
        match place_take_profits(&exec, markets, cfg.take_profit_pct, &tp_ref).await {
            Ok((lines, oids)) => {
                plan_lines.extend(lines);
                tp_oids_out = oids;
            }
            Err(e) => plan_lines.push(format!("止盈单挂单失败: {e}")),
        }
    }

    Ok((
        RunResult {
            aborted,
            vol_k: applied_k,
            equity: plan.equity,
            long_leg: plan.long_leg.clone(),
            short_leg: plan.short_leg.clone(),
            per_coin: plan.per_coin,
            tp_ref: (!aborted).then_some(tp_ref_out),
            tp_oids: tp_oids_out,
            plan_lines,
            executed,
            live,
            stopped: stopped_now,
        },
        records,
    ))
}


/// 用【当前持仓】算组合预估日波动（已含杠杆）✗ 供前端实时显示 ✓
pub fn current_book_vol(
    panel: &[crate::store::PanelEntry],
    positions: &[(String, f64)], // (coin, 带符号名义占净值比例)
) -> Option<f64> {
    if positions.len() < 4 {
        return None;
    }
    let total: f64 = positions.iter().map(|(_, w)| w.abs()).sum();
    if total <= 1e-9 {
        return None;
    }
    let scaled: Vec<(String, f64)> = positions
        .iter()
        .map(|(c, w)| (c.clone(), w / total))
        .collect();
    // Σ|w|=1 的组合波动 ✗ 再乘回总名义（因为 total 就是实际杠杆 ✓）
    book_vol_estimate(panel, &scaled, total)
}

/// 用最近 20 个已收盘交易日的日收益估算「当前权重下」组合的日波动（已乘杠杆）。
///
/// - 剔除尚未收盘的当根日线（`c.t + 86_400_000 <= now_ms`）；
/// - 严格按蜡烛时间戳 `c.t` 对齐各币日收益（绝不按数组倒序下标错位相加）；
/// - `leverage` 应传入有效名义杠杆（即 `leverage * margin_buffer`）。
fn book_vol_estimate(
    panel: &[crate::store::PanelEntry],
    weights: &[(String, f64)],
    leverage: f64,
) -> Option<f64> {
    book_vol_estimate_at(panel, weights, leverage, now_ms_pub())
}

pub(super) fn book_vol_estimate_at(
    panel: &[crate::store::PanelEntry],
    weights: &[(String, f64)],
    leverage: f64,
    now_ms: i64,
) -> Option<f64> {
    use std::collections::{BTreeMap, BTreeSet};
    const WINDOW: usize = 20;
    const DAY: i64 = 86_400_000;

    let mut coin_rets: Vec<(f64, HashMap<i64, f64>)> = Vec::new();
    let mut all_ts: BTreeSet<i64> = BTreeSet::new();

    for (coin, w) in weights {
        if !w.is_finite() || w.abs() <= 1e-12 {
            continue;
        }
        let Some(entry) = panel.iter().find(|e| &e.coin == coin) else {
            continue;
        };
        let mut closed: BTreeMap<i64, f64> = BTreeMap::new();
        for c in &entry.candles {
            if c.c.is_finite() && c.c > 0.0 && c.t.saturating_add(DAY) <= now_ms {
                closed.insert(c.t, c.c);
            }
        }
        if closed.len() < 11 {
            continue;
        }
        let mut rets: HashMap<i64, f64> = HashMap::new();
        let mut prev: Option<(i64, f64)> = None;
        for (t, c) in closed {
            if let Some((pt, pc)) = prev {
                if t - pt == DAY && pc > 0.0 {
                    rets.insert(t, c / pc - 1.0);
                    all_ts.insert(t);
                }
            }
            prev = Some((t, c));
        }
        if rets.len() < 10 {
            continue;
        }
        coin_rets.push((*w, rets));
    }
    if coin_rets.len() < 4 {
        return None;
    }

    let total_gross: f64 = coin_rets.iter().map(|(w, _)| w.abs()).sum();
    if total_gross <= 1e-12 {
        return None;
    }

    // 优先取所有参与币种都有收益的严格时间戳交集；若不足 10 天（如个别新币），
    // 放宽到当日覆盖 >= 80% 权重并按覆盖比例归一化。
    let mut aligned: Vec<i64> = all_ts
        .iter()
        .copied()
        .filter(|t| coin_rets.iter().all(|(_, rm)| rm.contains_key(t)))
        .collect();
    if aligned.len() < 10 {
        aligned = all_ts
            .iter()
            .copied()
            .filter(|t| {
                let cov: f64 = coin_rets
                    .iter()
                    .filter(|(_, rm)| rm.contains_key(t))
                    .map(|(w, _)| w.abs())
                    .sum();
                cov >= 0.8 * total_gross
            })
            .collect();
    }
    if aligned.len() < 10 {
        return None;
    }
    let take = aligned.len().min(WINDOW);
    let recent = &aligned[aligned.len() - take..];

    let mut daily: Vec<f64> = Vec::with_capacity(recent.len());
    for &t in recent {
        let mut r = 0.0;
        let mut cov = 0.0;
        for (w, rm) in &coin_rets {
            if let Some(&ret) = rm.get(&t) {
                r += w * ret;
                cov += w.abs();
            }
        }
        if cov > 1e-12 {
            daily.push(r * (total_gross / cov));
        }
    }
    if daily.len() < 10 {
        return None;
    }
    let mean = daily.iter().sum::<f64>() / daily.len() as f64;
    let var = daily.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (daily.len() - 1) as f64;
    let vol = var.sqrt();
    (vol.is_finite() && vol > 1e-9).then(|| vol * leverage)
}


/// 现在统一走这里：锁内判重、写入、裁剪、落盘。
pub async fn record_equity(
    live: &std::sync::Arc<tokio::sync::Mutex<LiveState>>,
    path: &std::path::Path,
    account: &str,
    equity: f64,
    pnl: f64,
) -> Result<()> {
    if !equity.is_finite() || equity < 0.0 || !pnl.is_finite() {
        anyhow::bail!("账户净值或盈亏不是有效数值");
    }
    let now = now_ms_pub();
    let hour = now / 3_600_000;
    let mut g = live.lock().await;
    anyhow::ensure!(g.config.account.eq_ignore_ascii_case(account), "账户已改变，丢弃旧账户净值");
    let previous = g.clone();
    let mut changed = false;
    if g.capital_baseline.is_none() {
        g.capital_baseline = g.baseline().or_else(|| (equity > 0.).then(|| CapitalBaseline {
            account: account.into(), equity, ts: Some(now), source: "首次记录净值".into(),
        }));
        if let Some(b) = &g.capital_baseline { g.start_equity = Some(b.equity); changed = true; }
    }
    // 锁内判重：同一小时内已有记录就不再写。
    if g.history.last().map(|p| p.ts / 3_600_000) == Some(hour) {
        if changed { if let Err(e) = g.save(path) { *g = previous; return Err(e.context("起始资金保存失败")); } }
        return Ok(());
    }
    g.history.push(EquityPoint { ts: now, equity, pnl });
    let len = g.history.len();
    if len > 6000 {
        g.history.drain(0..len - 6000);
    }
    if let Err(e) = g.save(path) { *g = previous; return Err(e.context("净值记录保存失败")); }
    Ok(())
}

/// 记录 ML 子账户 B 的净值点（每小时保留最新值）。用于 ML 和全部账户曲线。
pub async fn record_ml_equity(
    live: &std::sync::Arc<tokio::sync::Mutex<LiveState>>,
    path: &std::path::Path,
    equity: f64,
) -> Result<()> {
    if !equity.is_finite() || equity < 0.0 {
        anyhow::bail!("ML 净值不是有效数值");
    }
    let now = now_ms_pub();
    let hour = now / 3_600_000;
    let mut g = live.lock().await;
    let previous = g.clone();
    // 同小时不能简单跳过：早先的异常或过时快照必须能被后续有效快照纠正。
    if g.ml_history.last().map(|p| p.ts / 3_600_000) == Some(hour) {
        if let Some(last) = g.ml_history.last_mut() {
            *last = EquityPoint { ts: now, equity, pnl: 0.0 };
        }
    } else {
        g.ml_history.push(EquityPoint { ts: now, equity, pnl: 0.0 });
    }
    let len = g.ml_history.len();
    if len > 6000 {
        g.ml_history.drain(0..len - 6000);
    }
    if let Err(e) = g.save(path) {
        *g = previous;
        return Err(e.context("ML 净值记录保存失败"));
    }
    Ok(())
}


/// 从交易所读一次账户并记一个净值点 —— 给后台定时器用，与页面是否打开无关。
///
/// 这是净值曲线**唯一可靠的来源**：只靠 handler 记录的话，不看页面就不记，
/// 曲线会缺掉整段时间，而它恰恰是判断策略赚亏的唯一视图。
pub async fn record_equity_now(
    live: &std::sync::Arc<tokio::sync::Mutex<LiveState>>,
    path: &std::path::Path,
) -> Result<()> {
    let st = live.lock().await.clone();
    if st.config.account.is_empty() {
        return Ok(());
    }
    let exec = Exec::reader_config(&st.config).await?;
    let acct = exec.account().await?;
    let unreal: f64 = acct.positions.values().map(|p| p.unrealized_pnl).sum();
    record_equity(live, path, &st.config.account, acct.equity, unreal).await?;
    if let Some(ml_addr) = st.config.ml_vault_address.as_deref().filter(|s| !s.is_empty()) {
        let ml_snap = ml_account_snapshot(ml_addr, None).await;
        if ml_snap.error.is_none() && ml_snap.equity.is_finite() && ml_snap.equity >= 0.0 {
            let _ = record_ml_equity(live, path, ml_snap.equity).await;
        }
    }
    Ok(())
}

