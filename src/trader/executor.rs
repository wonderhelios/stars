//! Plan execution (close-first -> cross leverage -> open), mid fetching, and panel loading.

use super::{Plan, TradeConfig};
use crate::exchange::{round_size, Exec, MarketInfo};
use crate::store::PanelEntry;
use anyhow::{Context, Result};
use std::collections::HashMap;

/// 执行结果：前置日志（设杠杆等）与**逐条对应 plan.orders 的结果**分开返回，
/// 避免用下标对应时错位（曾经导致失败信息挂到别的币上）。
pub struct Outcome {
    pub prelim: Vec<String>,
    pub orders: Vec<String>,
    pub aborted: bool,
}

/// Send the plan. `orders[i]` always corresponds to `plan.orders[i]`.
///
/// 顺序很重要：先平仓 → 重新读账户 → 给已经空仓的币设杠杆（Hyperliquid
/// 不允许持仓时切换保证金模式）→ 再开仓。这样反手的币也能切成全仓。
pub async fn execute(
    exec: &Exec, plan: &Plan, cfg: &TradeConfig,
    markets: &HashMap<String, MarketInfo>, live: bool,
) -> Result<Outcome> {
    execute_inner(exec, plan, cfg, markets, live).await
}


async fn execute_inner(
    exec: &Exec,
    plan: &Plan,
    cfg: &TradeConfig,
    markets: &HashMap<String, MarketInfo>,
    live: bool,
) -> Result<Outcome> {
    let mut prelim: Vec<String> = Vec::new();
    let mut orders: Vec<String> = vec![String::new(); plan.orders.len()];

    if !live {
        for (i, o) in plan.orders.iter().enumerate() {
            orders[i] = format!(
                "[DRY-RUN] {} {} {} {:.6} @≈{:.6} (${:.2}) · {}",
                if o.buy { "买" } else { "卖" },
                o.coin,
                if o.reduce_only { "只减仓" } else { "开仓" },
                o.size,
                o.mid,
                o.notional,
                o.reason
            );
        }
        return Ok(Outcome { prelim, orders, aborted: false });
    }

    // ---- 1) 先平仓 ----
    for (i, o) in plan.orders.iter().enumerate() {
        if !o.reduce_only {
            continue;
        }
        orders[i] = match exec
            .maker_then_ioc(&o.coin, o.buy, true, o.size, o.mid, cfg.slippage, o.sz_decimals, 12)
            .await
        {
            Ok(st) => format!(
                "{} {} 只减仓 {:.6} · {} · {}",
                if o.buy { "买" } else { "卖" },
                o.coin,
                o.size,
                st,
                o.reason
            ),
            Err(e) => format!("{} {} 平仓失败: {e}", if o.buy { "买" } else { "卖" }, o.coin),
        };
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }

    // ---- 2) 重新读账户：既用于设杠杆，也用于把开仓量算成「目标 − 实际」----
    //
    // 之前的做法是直接按计划里的数量开仓。如果某笔平仓只成交了一部分（或被拒），
    // 仓位就与计划不符，而开仓单仍按旧计划发出 —— 结果可能把仓位做成反方向，
    // 而且止盈单还会挂到错误的一侧，把错误固定一整天。
    let after = match exec.account().await {
        Ok(a) => Some(a),
        Err(e) => {
            prelim.push(format!("开仓前重读账户失败，出于安全跳过全部开仓: {e}"));
            None
        }
    };
    let mut all_coins: Vec<String> = Vec::new();
    all_coins.extend(plan.long_leg.iter().cloned());
    all_coins.extend(plan.short_leg.iter().cloned());
    all_coins.extend(plan.orders.iter().map(|o| o.coin.clone()));
    all_coins.sort();
    all_coins.dedup();
    if let Some(after) = after.as_ref() {
        for coin in &all_coins {
            if after
                .positions
                .get(coin)
                .map(|p| p.size.abs() > 1e-12)
                .unwrap_or(false)
            {
                continue; // 仍有仓位，切不了模式，跳过
            }
            let max_lev = markets.get(coin).map(|m| m.max_leverage).unwrap_or(10);
            let want = cfg.leverage.round() as u32;
            let lev = want.clamp(1, max_lev.max(1));
            match exec.set_leverage(coin, lev).await {
                Ok(_) => {}
                Err(e) => prelim.push(format!("{coin}: 设置杠杆失败 {e}")),
            }
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        }
    }

    // ---- 3) 再开仓 ----
    if let Some(after) = after {
        for (i, o) in plan.orders.iter().enumerate() {
            if o.reduce_only {
                continue;
            }
            let actual = after.positions.get(&o.coin).map(|p| p.size).unwrap_or(0.0);
            // 平仓后方向仍然相反 —— 残余仓位没平掉，只能用只减仓把它清掉，
            // 绝不能按计划继续加另一个方向。
            if actual * o.target < 0.0 && actual.abs() > 1e-12 {
                orders[i] = match exec
                    .maker_then_ioc(
                        &o.coin,
                        actual < 0.0,
                        true,
                        actual.abs(),
                        o.mid,
                        cfg.slippage,
                        o.sz_decimals,
                        12,
                    )
                    .await
                {
                    Ok(st) => format!(
                        "{} {} 只减仓平残余 {:.6} · {} · 平仓未完成，纠正方向",
                        if actual < 0.0 { "买" } else { "卖" },
                        o.coin,
                        actual.abs(),
                        st
                    ),
                    Err(e) => format!("{} 纠正残余仓位失败: {e}", o.coin),
                };
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                continue;
            }
            let delta = o.target - actual;
            let dsize = round_size(delta.abs(), o.sz_decimals);
            let dnotional = dsize * o.mid;
            if dnotional < cfg.effective_min_order_usd(plan.equity) {
                orders[i] = format!(
                    "{} 已到位（差 ${:.2}），跳过",
                    o.coin, dnotional
                );
                continue;
            }
            let reduce_only = actual * delta < 0.0;
            orders[i] = match exec
                .maker_then_ioc(
                    &o.coin,
                    delta > 0.0,
                    reduce_only,
                    dsize,
                    o.mid,
                    cfg.slippage,
                    o.sz_decimals,
                    12,
                )
                .await
            {
                Ok(st) => format!(
                    "{} {} {} {:.6} · {} · {}",
                    if delta > 0.0 { "买" } else { "卖" },
                    o.coin,
                    if reduce_only { "只减仓" } else { "开仓" },
                    dsize,
                    st,
                    if (dsize - o.size).abs() > 1e-12 {
                        format!("{}（按实际仓位修正）", o.reason)
                    } else {
                        o.reason.clone()
                    }
                ),
                Err(e) => format!("{} 开仓失败: {e}", o.coin),
            };
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
    }


    Ok(Outcome { prelim, orders, aborted: false })
}

/// Mids for the coins we care about, from one bulk request.
pub async fn fetch_mids(exec: &Exec, coins: &[String]) -> HashMap<String, f64> {
    let all = match exec.all_mids().await {
        Ok(m) => m,
        Err(e) => {
            eprintln!("取价失败: {e}");
            return HashMap::new();
        }
    };
    coins
        .iter()
        .filter_map(|c| all.get(c).map(|p| (c.clone(), *p)))
        .collect()
}

pub fn load_panel(store: &crate::store::Store) -> Result<Vec<PanelEntry>> {
    Ok(store
        .all_panels()
        .context("read candle cache")?
        .into_iter()
        .map(|(coin, candles)| PanelEntry { coin, candles })
        .collect())
}


