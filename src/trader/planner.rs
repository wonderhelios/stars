//! Order planning, EMA smoothing, caps, and market-neutral trim logic.

use super::{Order, Plan, TradeConfig};
use crate::exchange::{round_size, Acct, MarketInfo};
use std::collections::{BTreeMap, HashMap};

/// 净敞口保护：只做减仓，不新增开仓。没有目标权重时按同侧仓位比例分摊，
/// 并始终按币名排序，避免 HashMap 随机顺序导致同一账户每次砍掉不同的币。
pub fn neutralize_orders(
    acct: &Acct,
    markets: &std::collections::HashMap<String, MarketInfo>,
    mids: &std::collections::HashMap<String, f64>,
    tolerance: f64,
    min_order_usd: f64,
) -> Vec<Order> {
    neutralize_orders_with_targets(acct, markets, mids, tolerance, min_order_usd, None)
}

/// 目标感知的中性化：先按各币「当前名义 − 同向目标名义」的超额比例减仓；
/// 若仍不够，再按剩余仓位比例分摊。目标名义为带符号的美元名义。
/// 这样恢复市场中性时，不会无差别地把刚调到目标附近的仓位整笔清零。
pub fn neutralize_orders_with_targets(
    acct: &Acct,
    markets: &std::collections::HashMap<String, MarketInfo>,
    mids: &std::collections::HashMap<String, f64>,
    tolerance: f64,
    min_order_usd: f64,
    target_notionals: Option<&std::collections::HashMap<String, f64>>,
) -> Vec<Order> {
    let mut long_n = 0.0;
    let mut short_n = 0.0;
    for (c, p) in acct.positions.iter() {
        let Some(&mid) = mids.get(c) else { continue };
        if !mid.is_finite() || mid <= 0.0 {
            continue;
        }
        let n = p.size.abs() * mid;
        if p.size > 0.0 {
            long_n += n;
        } else {
            short_n += n;
        }
    }
    let gross = long_n + short_n;
    let net = long_n - short_n;
    if gross <= 0.0 || net.abs() <= (tolerance * gross).max(min_order_usd) {
        return Vec::new();
    }
    let reduce_longs = net > 0.0;
    let side_sign = if reduce_longs { 1.0 } else { -1.0 };
    let mut coins: Vec<String> = acct
        .positions
        .iter()
        .filter(|(_, p)| (p.size > 0.0) == reduce_longs)
        .map(|(c, _)| c.clone())
        .collect();
    coins.sort();
    let mut remaining: std::collections::HashMap<String, f64> = coins
        .iter()
        .filter_map(|c| {
            let p = acct.positions.get(c)?;
            let mid = mids.get(c)?;
            Some((c.clone(), p.size.abs() * mid))
        })
        .collect();
    let mut budget = net.abs();
    let mut out: Vec<Order> = Vec::new();
    let mut reduce = |coin: &str,
                      want: f64,
                      remaining: &mut std::collections::HashMap<String, f64>,
                      budget: f64|
     -> f64 {
        if budget < min_order_usd || want <= 0.0 {
            return budget;
        }
        let Some(p) = acct.positions.get(coin) else { return budget };
        let (Some(&mid), Some(m)) = (mids.get(coin), markets.get(coin)) else { return budget };
        if !mid.is_finite() || mid <= 0.0 {
            return budget;
        }
        let available = remaining.get(coin).copied().unwrap_or(0.0);
        let take = available.min(want).min(budget);
        let size = round_size(take / mid, m.sz_decimals);
        if size * mid < min_order_usd {
            return budget;
        }
        let target = p.size - p.size.signum() * size;
        out.push(Order {
            coin: coin.to_string(),
            buy: p.size < 0.0,
            reduce_only: true,
            target,
            size,
            mid,
            sz_decimals: m.sz_decimals,
            notional: size * mid,
            reason: "净敞口中性化".into(),
        });
        remaining.insert(coin.to_string(), (available - size * mid).max(0.0));
        (budget - size * mid).max(0.0)
    };

    // 第一阶段：只减超出模型目标的部分，并按超额比例分配。
    if let Some(targets) = target_notionals {
        let mut excess: Vec<(String, f64)> = Vec::new();
        for c in &coins {
            let cur = remaining.get(c).copied().unwrap_or(0.0);
            let signed_target = targets.get(c).copied().unwrap_or(0.0);
            let same_side_target = if signed_target * side_sign > 0.0 {
                signed_target.abs().min(cur)
            } else {
                0.0
            };
            let x = (cur - same_side_target).max(0.0);
            if x > 1e-9 {
                excess.push((c.clone(), x));
            }
        }
        let total: f64 = excess.iter().map(|(_, x)| *x).sum();
        if total > 1e-9 {
            let pass_budget = budget;
            for (c, x) in excess {
                let allocation = if pass_budget < total { pass_budget * x / total } else { x };
                budget = reduce(&c, allocation, &mut remaining, budget);
            }
        }
    }

    // 第二阶段：超额部分仍不足时，按剩余名义比例分摊，不随机整币清仓。
    let leg_remaining: f64 = remaining.values().sum();
    if leg_remaining > 1e-9 {
        let pass_budget = budget;
        for c in &coins {
            let cur = remaining.get(c).copied().unwrap_or(0.0);
            budget = reduce(c, pass_budget * cur / leg_remaining, &mut remaining, budget);
        }
    }
    // 数量向下取整可能留下小额预算。这里保留这个误差，不再按币名把余量
    // 集中压给某一个仓，否则会破坏上面的比例分摊并可能整币清零。

    // 同一币可能先减超额、再参与比例分摊。聚合成每币一笔，避免重复手续费，
    // 并让 target 表示全部中性化订单执行后的最终仓位。
    let mut sizes: BTreeMap<String, f64> = BTreeMap::new();
    for order in out {
        *sizes.entry(order.coin).or_insert(0.0) += order.size;
    }
    sizes
        .into_iter()
        .filter_map(|(coin, size)| {
            let p = acct.positions.get(&coin)?;
            let (&mid, m) = (mids.get(&coin)?, markets.get(&coin)?);
            Some(Order {
                coin,
                buy: p.size < 0.0,
                reduce_only: true,
                target: p.size - p.size.signum() * size,
                size,
                mid,
                sz_decimals: m.sz_decimals,
                notional: size * mid,
                reason: "净敞口中性化".into(),
            })
        })
        .collect()
}


/// 某个币属于哪一档。用币名的字节和取模，保证跨进程、跨重启都一致。
pub fn slice_of(coin: &str, slices: u32) -> u32 {
    if slices <= 1 {
        return 0;
    }
    let sum: u32 = coin.bytes().map(|b| b as u32).sum();
    sum % slices
}

/// Build the order list. `mids` must contain every coin we intend to trade.
/// 将目标权重截断到单币上限，并把超限部分在同侧（多头或空头）其他未封顶币间等杠杆水满再分配。
///
/// `weights` 的口径：`|w_i| * margin_buffer * leverage = 单币目标名义 / 账户净值`。
/// 波动率系数已经乘在 `weights` 里，订单金额也只直接使用这份最终权重；
/// 换算上限和计算订单时都不能再额外乘一次波动率系数。
/// 因此 `max_single_weight`（占净值比例，如 0.20）折算到权重空间为
/// `max_w = max_single_weight / (margin_buffer * leverage)`。
/// 该函数是幂等的，返回被截断过的币种列表（供计划日志展示）。
pub fn cap_weights_same_side(
    weights: &mut [(String, f64)],
    max_single_weight: f64,
    margin_buffer: f64,
    leverage: f64,
) -> Vec<String> {
    let gross_mult = margin_buffer * leverage;
    if !max_single_weight.is_finite()
        || max_single_weight <= 0.0
        || !gross_mult.is_finite()
        || gross_mult <= 1e-12
    {
        return Vec::new();
    }
    let max_w = max_single_weight / gross_mult;
    let mut clipped: Vec<String> = Vec::new();

    for is_long in [true, false] {
        let sign = if is_long { 1.0 } else { -1.0 };
        let mut rem: Vec<usize> = weights
            .iter()
            .enumerate()
            .filter(|(_, (_, w))| if is_long { *w > 1e-12 } else { *w < -1e-12 })
            .map(|(i, _)| i)
            .collect();
        if rem.is_empty() {
            continue;
        }
        let mut rem_target: f64 = rem.iter().map(|&i| weights[i].1.abs()).sum();
        loop {
            let rem_sum: f64 = rem.iter().map(|&i| weights[i].1.abs()).sum();
            if rem.is_empty() || rem_sum <= 1e-12 || rem_target <= 1e-12 {
                break;
            }
            let scale = rem_target / rem_sum;
            let over: Vec<usize> = rem
                .iter()
                .copied()
                .filter(|&i| weights[i].1.abs() * scale > max_w + 1e-12)
                .collect();
            if over.is_empty() {
                for &i in &rem {
                    weights[i].1 = sign * (weights[i].1.abs() * scale);
                }
                break;
            }
            for &i in &over {
                weights[i].1 = sign * max_w;
                if !clipped.contains(&weights[i].0) {
                    clipped.push(weights[i].0.clone());
                }
                rem_target = (rem_target - max_w).max(0.0);
            }
            rem.retain(|i| !over.contains(i));
        }
    }

    // 若某条腿因币数偏少全部打满上限，导致多空总权重不再相等，
    // 将较重的一侧等比例缩放到与较轻一侧相等，严格保持目标组合美元中性（sum(w) ≈ 0）。
    let long_sum: f64 = weights.iter().filter(|(_, w)| *w > 1e-12).map(|(_, w)| *w).sum();
    let short_sum: f64 = weights
        .iter()
        .filter(|(_, w)| *w < -1e-12)
        .map(|(_, w)| w.abs())
        .sum();
    if long_sum > 1e-12 && short_sum > 1e-12 && (long_sum - short_sum).abs() > 1e-12 {
        let target_leg = long_sum.min(short_sum);
        if long_sum > short_sum {
            let scale = target_leg / long_sum;
            for (_, w) in weights.iter_mut() {
                if *w > 1e-12 {
                    *w *= scale;
                }
            }
        } else {
            let scale = target_leg / short_sum;
            for (_, w) in weights.iter_mut() {
                if *w < -1e-12 {
                    *w *= scale;
                }
            }
        }
    }
    clipped
}

/// 若权重组合的目标毛杠杆超过 `hard_gross_cap`（占账户净值倍数，如 2.8x），
/// 等比例缩小所有权重并返回 `(raw_gross_lev, scale)`，确保目标组合永远不超过硬杠杆上限。
pub fn apply_hard_gross_cap(
    weights: &mut [(String, f64)],
    margin_buffer: f64,
    leverage: f64,
    hard_gross_cap: f64,
) -> Option<(f64, f64)> {
    let gross_mult = margin_buffer * leverage;
    if !hard_gross_cap.is_finite()
        || hard_gross_cap <= 0.0
        || !gross_mult.is_finite()
        || gross_mult <= 1e-12
    {
        return None;
    }
    let raw_gross_lev = weights.iter().map(|(_, w)| w.abs()).sum::<f64>() * gross_mult;
    if raw_gross_lev > hard_gross_cap + 1e-9 {
        let scale = hard_gross_cap / raw_gross_lev;
        for (_, w) in weights.iter_mut() {
            *w *= scale;
        }
        Some((raw_gross_lev, scale))
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_plan(
    weights: &[(String, f64)],
    acct: &Acct,
    markets: &HashMap<String, MarketInfo>,
    mids: &HashMap<String, f64>,
    cfg: &TradeConfig,
    equity_override: Option<f64>,
) -> Plan {
    let equity = equity_override.unwrap_or(acct.equity);
    // Deploy only part of the equity so fees and adverse fills cannot push the
    // book past the margin limit mid-run.
    let deployable = equity * cfg.margin_buffer;
    let target_gross = deployable * cfg.leverage;
    let hard_cap = if cfg.hard_gross_cap.is_finite() && cfg.hard_gross_cap > 0.0 && equity > 0.0 {
        equity * cfg.hard_gross_cap
    } else {
        f64::INFINITY
    };

    let mut capped_weights = weights.to_vec();
    let hard_cap_note = apply_hard_gross_cap(
        &mut capped_weights,
        cfg.margin_buffer,
        cfg.leverage,
        cfg.hard_gross_cap,
    )
    .map(|(raw_lev, scale)| {
        format!(
            "目标总敞口 {:.2}x 触发硬上限 {:.2}x，等比例缩放 ×{:.3}",
            raw_lev, cfg.hard_gross_cap, scale
        )
    });
    if cfg.max_single_weight > 0.0 {
        cap_weights_same_side(
            &mut capped_weights,
            cfg.max_single_weight,
            cfg.margin_buffer,
            cfg.leverage,
        );
    }
    let weights = capped_weights.as_slice();
    // 权重已归一化到 Σ|w| = 1（或已乘入 vol_k），直接按权重缩放名义即可。
    let per_coin = if weights.is_empty() {
        0.0
    } else {
        target_gross / weights.len() as f64
    };
    let long_leg: Vec<String> = weights
        .iter()
        .filter(|(_, w)| *w > 0.0)
        .map(|(c, _)| c.clone())
        .collect();
    let short_leg: Vec<String> = weights
        .iter()
        .filter(|(_, w)| *w < 0.0)
        .map(|(c, _)| c.clone())
        .collect();
    let mut plan = Plan {
        equity,
        per_coin,
        long_leg,
        short_leg,
        held: acct
            .positions
            .iter()
            .filter(|(_, p)| p.size.abs() > 1e-12)
            .map(|(c, _)| c.clone())
            .collect(),
        ..Default::default()
    };
    if let Some(note) = hard_cap_note {
        plan.notes.push(note);
    }

    // 总敞口保护：把目标毛敞口按 Σ|w|（已包含 vol_k）和硬杠杆上限共同折算，
    // 超过 desired_gross 2% 就进入「强制减仓」模式：减仓不受错峰、EMA 和调仓带限制。
    let weights_gross: f64 = weights.iter().map(|(_, w)| w.abs()).sum();
    let desired_gross = (target_gross * weights_gross).min(hard_cap);
    let cur_gross: f64 = acct
        .positions
        .iter()
        .filter_map(|(c, p)| mids.get(c).map(|mid| p.size.abs() * mid))
        .sum();
    let over_cap = desired_gross > 0.0 && cur_gross > desired_gross * 1.02;
    if over_cap && equity > 0.0 {
        plan.notes.push(format!(
            "总敞口 {:.2}x 超过目标 {:.2}x，本次强制减仓（不受错峰限制）",
            cur_gross / equity,
            desired_gross / equity
        ));
    }

    // 错峰调仓与平滑 EMA 调仓：
    // - slices > 1 时按币名哈希分档，每天只调属于本档的币；
    // - slices == 1 且 ema_alpha < 1.0 时按 w_exec = w_cur + ema_alpha * (w_target - w_cur) 平滑收敛。
    let slices = cfg.rebalance_slices.max(1);
    let ema_alpha = if cfg.ema_alpha.is_finite() {
        cfg.ema_alpha.clamp(0.1, 1.0)
    } else {
        1.0
    };
    let ema_active = slices == 1 && ema_alpha < 1.0 - 1e-9;
    let rebal_band = if cfg.rebal_band.is_finite() {
        cfg.rebal_band.clamp(0.0, 0.50)
    } else {
        0.0
    };
    let day_no = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
        / 86_400_000) as u32;
    let in_slot = |coin: &str| slices <= 1 || (day_no + slice_of(coin, slices)) % slices == 0;

    // wanted: coin -> signed target size（按权重缩放名义）
    let mut wanted: HashMap<String, (f64, bool)> = HashMap::new();
    for (coin, w) in weights {
        if w.abs() < 1e-12 {
            continue;
        }
        let (Some(mid), Some(m)) = (mids.get(coin), markets.get(coin)) else {
            continue;
        };
        let notional = w.abs() * deployable * cfg.leverage;
        let size = round_size(notional / mid, m.sz_decimals);
        if size * mid >= cfg.effective_min_order_usd(acct.equity) {
            wanted.insert(coin.clone(), (if *w > 0.0 { size } else { -size }, *w > 0.0));
        } else {
            plan.notes.push(format!(
                "{coin}: 目标 ${:.0} 低于最小下单额，跳过",
                size * mid
            ));
        }
    }

    // 1) close positions that are gone, stopped, took profit, or must flip side
    let mut closes: Vec<Order> = Vec::new();
    let mut opens: Vec<Order> = Vec::new();
    let mut blocked_reopen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (coin, pos) in acct.positions.iter() {
        let cur = pos.size;
        if cur.abs() < 1e-12 {
            continue;
        }
        let in_today = in_slot(coin);
        let Some(mid) = mids.get(coin) else {
            plan.notes
                .push(format!("{coin}: 缺价格，无法处理现有仓位 {cur:.6}"));
            continue;
        };
        let Some(m) = markets.get(coin) else {
            plan.notes.push(format!("{coin}: 缺市场元数据"));
            continue;
        };
        let stop_hit = cfg.position_stop_pct > 0.0
            && pos.entry_px > 0.0
            && pos.entry_px.is_finite()
            && {
                let ret = mid / pos.entry_px - 1.0;
                let r = if cur > 0.0 { ret } else { -ret };
                r < -cfg.position_stop_pct
            };
        let tp_hit = !stop_hit
            && cfg.take_profit_pct > 0.0
            && pos.entry_px > 0.0
            && pos.entry_px.is_finite()
            && {
                let ret = mid / pos.entry_px - 1.0;
                let r = if cur > 0.0 { ret } else { -ret };
                r >= cfg.take_profit_pct
            };
        let raw_target = if stop_hit || tp_hit {
            blocked_reopen.insert(coin.clone());
            0.0 // 止损/止盈：本次直接平掉且当次禁止在开仓循环里反手重开
        } else {
            wanted.get(coin).map(|w| w.0).unwrap_or(0.0)
        };
        let single_over_cap = cfg.max_single_weight > 0.0
            && equity > 0.0
            && cur.abs() * mid > equity * cfg.max_single_weight * 1.02;
        let force_reduce_raw = (over_cap || single_over_cap)
            && (raw_target == 0.0
                || cur * raw_target < 0.0
                || (cur * raw_target > 0.0 && raw_target.abs() < cur.abs()));

        // 平滑 EMA 目标：止损/止盈/强制减仓直接走 raw_target；否则按 cur + alpha*(raw_target - cur) 收敛。
        // 掉出名单（raw_target == 0）若剩余名义过小（残尘）则直接全平，不留不可平残仓。
        let target = if stop_hit || tp_hit || !ema_active || force_reduce_raw {
            raw_target
        } else {
            let blended = cur + ema_alpha * (raw_target - cur);
            if raw_target == 0.0 {
                let dust_floor = cfg
                    .effective_min_order_usd(acct.equity)
                    .max(cfg.min_order_usd * 2.0);
                if blended.abs() * mid < dust_floor {
                    0.0
                } else {
                    blended
                }
            } else if cur * raw_target < 0.0 {
                if blended.abs() * mid < cfg.min_order_usd {
                    0.0
                } else {
                    blended
                }
            } else {
                blended
            }
        };

        let flip = cur * target < 0.0;
        if target == 0.0 || flip {
            // 单币止损必须无视错峰档位立即平仓，避免仓位被冻结在 !in_today 档位
            if !in_today && !stop_hit {
                // 不在今天档位：若单币本身已超净值上限，先越过错峰把它减回上限，避免高集中度拖延多日。
                if single_over_cap {
                    let cap_sz = round_size(equity * cfg.max_single_weight / mid, m.sz_decimals);
                    let cap_target = if cur > 0.0 { cap_sz } else { -cap_sz };
                    let trim = round_size((cur.abs() - cap_sz).max(0.0), m.sz_decimals);
                    let dnotional = trim * mid;
                    if dnotional >= cfg.min_order_usd {
                        opens.push(Order {
                            coin: coin.clone(),
                            buy: cur < 0.0,
                            reduce_only: true,
                            target: cap_target,
                            size: trim,
                            mid: *mid,
                            sz_decimals: m.sz_decimals,
                            notional: dnotional,
                            reason: "单币超限减仓".into(),
                        });
                    }
                }
                continue; // 完整平仓仍按档位错峰
            }
            let size = round_size(cur.abs(), m.sz_decimals);
            // **平仓只用绝对下限，不用按净值比例算的门槛。**
            if size * mid >= cfg.min_order_usd {
                closes.push(Order {
                    coin: coin.clone(),
                    buy: cur < 0.0, // buy to close a short
                    reduce_only: true,
                    target: 0.0,
                    size,
                    mid: *mid,
                    sz_decimals: m.sz_decimals,
                    notional: size * mid,
                    reason: if stop_hit {
                        "单币止损平仓".into()
                    } else if flip {
                        "反手平仓".into()
                    } else if tp_hit {
                        "止盈平仓".into()
                    } else {
                        "掉出名单平仓".into()
                    },
                });
            } else {
                plan.notes
                    .push(format!("{coin}: 残余仓位 ${:.1} 低于最小下单额，无法平", size * mid));
            }
        } else {
            let delta = target - cur;
            let reducing = cur * delta < 0.0;
            let force_reduce = (over_cap || single_over_cap) && reducing;
            let closing_decay = ema_active && raw_target == 0.0 && reducing;
            if !in_today && !force_reduce {
                continue; // 不在今天档位：只有「超限 + 减仓」才破例
            }
            let dsize = round_size(delta.abs(), m.sz_decimals);
            let dnotional = dsize * mid;
            let pos_target = raw_target.abs() * mid;
            let threshold = if force_reduce || closing_decay {
                // 超限减仓或掉出名单的平滑减仓：只保留交易所绝对下限，不受调仓带限制。
                cfg.min_order_usd
            } else {
                cfg.effective_min_order_usd(acct.equity)
                    .max(pos_target * rebal_band)
            };
            if dnotional >= threshold {
                opens.push(Order {
                    coin: coin.clone(),
                    buy: delta > 0.0,
                    reduce_only: cur * delta < 0.0,
                    target,
                    size: dsize,
                    mid: *mid,
                    sz_decimals: m.sz_decimals,
                    notional: dnotional,
                    reason: if cur * delta > 0.0 {
                        "加仓到目标".into()
                    } else if single_over_cap {
                        "单币超限减仓".into()
                    } else if closing_decay {
                        "掉出名单平滑减仓".into()
                    } else {
                        "减仓到目标".into()
                    },
                });
            }
        }
    }

    // 止盈摘要：止盈单不是"计划订单"，它在执行之后由 place_take_profits 单独挂 ✗
    // 但不写出来用户会以为"没有止盈"✗ 所以这里给一行摘要 ✗
    if cfg.take_profit_pct > 0.0 && !acct.positions.is_empty() {
        let mut lines = 0usize;
        let mut closest: Option<(String, f64)> = None;
        for (coin, pos) in acct.positions.iter() {
            let Some(&mid) = mids.get(coin) else { continue };
            if pos.entry_px <= 0.0 || mid <= 0.0 {
                continue;
            }
            let long = pos.size > 0.0;
            let line = if long { pos.entry_px * (1.0 + cfg.take_profit_pct) } else { pos.entry_px * (1.0 - cfg.take_profit_pct) };
            let need = (line / mid - 1.0) * if long { 1.0 } else { -1.0 } * 100.0;
            if need > 0.0 {
                lines += 1;
                if closest.as_ref().map(|(_, d)| need < *d).unwrap_or(true) {
                    closest = Some((coin.clone(), need));
                }
            }
        }
        let mut msg = format!(
            "止盈：执行后为 {} 个仓位挂成本 ±{:.0}% 的限价单（= {:.0}% ROE @ {:.0}x）",
            lines,
            cfg.take_profit_pct * 100.0,
            cfg.take_profit_pct * cfg.leverage * 100.0,
            cfg.leverage
        );
        if let Some((coin, need)) = closest {
            msg.push_str(&format!(
                "；最近的是 {coin}，还差 {need:.1}% 价格（{:.1}% ROE）",
                need * cfg.leverage
            ));
        }
        plan.notes.push(msg);
    }

    // 2) open positions that are missing entirely (or flipped across zero)
    for (coin, (raw_target, is_long)) in wanted.iter() {
        if blocked_reopen.contains(coin) {
            continue; // 本次刚触发止损/止盈平仓，绝不在同一计划里立即反手重开
        }
        if over_cap {
            continue; // 先减仓回到上限以内，再考虑新仓
        }
        if !in_slot(coin) {
            continue; // 新仓位也等轮到它那一档再开
        }
        let cur = acct.positions.get(coin).map(|p| p.size).unwrap_or(0.0);
        let exec_target = if ema_active {
            cur + ema_alpha * (*raw_target - cur)
        } else {
            *raw_target
        };
        let flip = cur * exec_target < 0.0;
        // A flip is handled by the close above; reopen here once it is flat.
        if cur.abs() < 1e-12 || flip {
            let Some(mid) = mids.get(coin) else { continue };
            let Some(m) = markets.get(coin) else { continue };
            let size = round_size(exec_target.abs(), m.sz_decimals);
            if size * mid >= cfg.effective_min_order_usd(acct.equity) {
                let open_long = exec_target > 0.0 || (exec_target == 0.0 && *is_long);
                opens.push(Order {
                    coin: coin.clone(),
                    buy: open_long,
                    reduce_only: false,
                    target: if open_long { size } else { -size },
                    size,
                    mid: *mid,
                    sz_decimals: m.sz_decimals,
                    notional: size * mid,
                    reason: if open_long { "开多".into() } else { "开空".into() },
                });
            }
        }
    }

    // 真实的目标总名义 = Σ|w| × 可部署 × 杠杆。之前用「开仓单求和」，那是本次
    // 的增量而不是目标敞口，用来展示会误导（平均每仓 × 2 × 腿数同样不准）。
    plan.gross_notional = weights.iter().map(|(_, w)| w.abs()).sum::<f64>()
        * deployable
        * cfg.leverage;
    plan.orders = closes;
    plan.orders.extend(opens);
    plan
}

