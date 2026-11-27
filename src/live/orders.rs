//! Cross-margin rebuild, resting take-profit management, and exchange fill reconciliation.

use super::runner::{ensure_candidates, signal_panel};
use super::{now_ms_pub, LiveRecord, LiveState};
use crate::exchange::{order_price, price_tick, round_size, Exec, MarketInfo};
use crate::trader;
use anyhow::{Context, Result};
use std::collections::HashMap;

///
/// Hyperliquid 不允许在持仓状态下切换保证金模式（"Cannot switch leverage
/// while a position is open"），所以必须平掉再开。逐币处理的好处是组合里
/// 其余仓位始终保持对冲，任一时刻只有 1 个仓位处于裸奔状态。
pub async fn rebuild_cross(
    store: &crate::store::Store,
    state: &LiveState,
    markets: &HashMap<String, MarketInfo>,
) -> Result<Vec<String>> {
    let cfg = state.config.clone();
    anyhow::ensure!(cfg.armed, "实盘未启用（需先打开「启用实盘」开关）");
    anyhow::ensure!(cfg.can_sign(), "未配置 API 钱包密钥");
    let exec = Exec::signer_config(&cfg).await?;
    let tc = cfg.trade_config();

    // 先读账户：仓位数上限（cap）依赖净值，必须在算权重之前拿到。
    let acct = exec.account().await?;
    if acct.positions.is_empty() {
        return Ok(vec!["账户没有持仓，无需转换".into()]);
    }
    let panel = signal_panel(store)?;
    let (weights, _) = trader::target_weights(
        &panel,
        &tc,
        acct.equity,
        crate::live::now_ms_pub() as i64,
    );
    ensure_candidates(&weights)?;
    let coins: Vec<String> = acct.positions.keys().cloned().collect();
    let mids = trader::fetch_mids(&exec, &coins).await;
    // 按各自权重还原仓位。之前用「净值×杠杆÷仓位数」等权重建，会把权重抹平，
    // 总敞口也从 Σ|w| 变成 1 —— 点一次「转为全仓」就悄悄改了权重和杠杆。
    let deployable = acct.equity * cfg.margin_buffer;
    let wmap: HashMap<String, f64> = weights.iter().cloned().collect();
    let per_coin = deployable * cfg.effective_leverage() / weights.len().max(1) as f64;

    let mut log = vec![format!(
        "逐币转全仓：{} 个持仓 · 目标每仓 ${:.2}",
        acct.positions.len(),
        per_coin
    )];
    for coin in &coins {
        let pos = &acct.positions[coin];
        // 0) 已经是全仓的不要动。审计指出：原来的代码会把已有全仓仓也平掉重建，
        //    白付两次成本，而且重建期间组合是裸的。
        if pos.is_cross {
            log.push(format!("{coin}: 已是全仓，跳过"));
            continue;
        }
        let Some(&mid) = mids.get(coin) else {
            log.push(format!("{coin}: 取价失败，跳过"));
            continue;
        };
        let Some(m) = markets.get(coin) else {
            log.push(format!("{coin}: 缺市场元数据，跳过"));
            continue;
        };
        let lev = (cfg.effective_leverage().round() as u32).clamp(1, m.max_leverage.max(1));

        // 1) 平仓
        let size = crate::exchange::round_size(pos.size.abs(), m.sz_decimals);
        if size * mid >= 10.0 {
            let buy = pos.size < 0.0;
            match exec
                .ioc(coin, buy, true, size, mid, cfg.slippage, m.sz_decimals)
                .await
            {
                Ok(s) => log.push(format!("平 {coin} {size} · {}", crate::exchange::describe(&s))),
                Err(e) => {
                    log.push(format!("平 {coin} 失败：{e}（保持原样）"));
                    continue;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // 1b) **必须确认真的平掉了**。IOC 可能只成交一部分，而返回仍然是 Ok；
        //     旧代码把任何 Ok 当"平完了"，然后按完整目标开仓 —— 审计给的反例是
        //     原多 10 只平掉 2 又买 10，最终多 18（超额 80%）；目标反向时甚至会
        //     做成和目标相反的净方向。用 200ms 睡眠代替确认是不成立的。
        let after = exec.account().await?;
        let residual = after.positions.get(coin).map(|p| p.size).unwrap_or(0.0);
        let mid_now = trader::fetch_mids(&exec, std::slice::from_ref(coin))
            .await
            .get(coin)
            .copied()
            .unwrap_or(mid);
        if (residual * mid_now).abs() >= 10.0 {
            log.push(format!(
                "{coin}: 平仓未完成（残留 {residual}），跳过该币 —— 继续开仓会变成反手加仓"
            ));
            continue;
        }

        // 2) 切全仓。**失败必须停手**：带着逐仓继续开，等于给单币强平敞口。
        if let Err(e) = exec.set_leverage(coin, lev).await {
            log.push(format!("{coin}: 切全仓失败 {e}，跳过该币（不开仓）"));
            continue;
        }
        log.push(format!("{coin}: 已切 {lev}x 全仓"));

        // 3) 按 target − actual 开仓，而不是按完整目标
        let w = weights.iter().find(|(c, _)| c == coin).map(|(_, w)| *w).unwrap_or(0.0);
        let want_long = w > 0.0;
        if want_long || w < 0.0 {
            let notional = wmap.get(coin).map(|x| x.abs()).unwrap_or(0.0)
                * deployable
                * cfg.effective_leverage();
            let tsize = crate::exchange::round_size(notional / mid_now, m.sz_decimals);
            let want = if want_long { tsize } else { -tsize };
            let delta = want - residual;
            let dsize = crate::exchange::round_size(delta.abs(), m.sz_decimals);
            if dsize * mid_now >= 10.0 {
                match exec
                    .ioc(coin, delta > 0.0, false, dsize, mid_now, cfg.slippage, m.sz_decimals)
                    .await
                {
                    Ok(s) => log.push(format!(
                        "重开 {coin} {dsize} · {}",
                        crate::exchange::describe(&s)
                    )),
                    Err(e) => log.push(format!("重开 {coin} 失败：{e}")),
                }
            }
        } else {
            log.push(format!("{coin}: 不在目标名单，保持平仓"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    Ok(log)
}


/// 撤掉本程序挂出的止盈单。
///
/// 只撤带 cloid 前缀的单，不会误撤用户手动挂的单。返回撤销数量。
/// 注意：`open_orders` 失败会直接冒泡 —— 撤不掉旧单就绝不能继续下单，
/// 否则同一个仓位上会同时存在两张只减仓单（交易所只按仓位校验，不会去重）。
pub async fn cancel_our_take_profits(exec: &Exec) -> Result<usize> {
    let orders = exec.open_order_details().await?;
    let ours: Vec<(String, u64)> = orders
        .iter()
        .filter(|o| crate::exchange::is_ours(o.cloid.as_deref()))
        .map(|o| (o.coin.clone(), o.oid))
        .collect();
    if ours.is_empty() {
        return Ok(0);
    }
    exec.cancel_orders(&ours).await
}

/// 为当前所有持仓挂止盈单。
///
/// `reference` 是**调仓时**记录的参考价：止盈幅度必须相对那个价格算，而不是
/// 相对当前中间价。否则手动「刷新止盈单」会把止盈线随行情一起搬走 —— 例如
/// 100 建仓、现价 80 的多头会被挂到 88，变成一笔自动的亏损出场。
/// 参考价缺失（或已越过止盈线）时退回当前中间价，避免挂出立即成交的单。
pub async fn place_take_profits(
    exec: &Exec,
    markets: &HashMap<String, MarketInfo>,
    tp_pct: f64,
    reference: &HashMap<String, f64>,
) -> Result<(Vec<String>, Vec<u64>)> {
    let mut log = Vec::new();
    let mut oids: Vec<u64> = Vec::new();
    if tp_pct <= 0.0 {
        log.push("未启用止盈".into());
        return Ok((log, oids));
    }
    anyhow::ensure!(!markets.is_empty(), "市场元数据为空，拒绝挂止盈单");
    let acct = exec.account().await?;
    if acct.positions.is_empty() {
        log.push("无持仓，不挂止盈".into());
        return Ok((log, oids));
    }
    let coins: Vec<String> = acct.positions.keys().cloned().collect();
    let mids = trader::fetch_mids(exec, &coins).await;
    anyhow::ensure!(!mids.is_empty(), "取不到中间价，拒绝挂止盈单");

    let mut placed = 0usize;
    let mut filled = 0usize;
    let mut failed = Vec::new();
    for (coin, pos) in acct.positions.iter() {
        let Some(m) = markets.get(coin.as_str()) else {
            failed.push(format!("{coin}: 缺市场元数据"));
            continue;
        };
        let Some(&mid) = mids.get(coin) else {
            failed.push(format!("{coin}: 缺中间价"));
            continue;
        };
        if mid <= 0.0 {
            continue;
        }
        let is_long = pos.size > 0.0;
        // 基准价 = 【持仓成本】（选项 B）。成本缺失才退回调仓参考价、再退回中间价。
        //
        // 为什么改成成本价：相对"上次调仓的中间价"挂止盈，止盈线会随行情每天搬家，
        // 变成"每天从开盘价再涨 10% 才走"，而不是"这笔仓位赚到 10% 就走"。
        // 实测（2026-10-17，234 币日线）：相对成本·日内触线 Sharpe 2.63、暴跌周 −11.2%，
        // 相对开盘·日内触线 Sharpe 3.18 但暴跌周 −16.8%。选成本版（用户选项 B）。
        let base = if pos.entry_px > 0.0 && pos.entry_px.is_finite() {
            pos.entry_px
        } else {
            reference.get(coin).copied().unwrap_or(mid)
        };
        // 现价已经越过止盈线：不挂"一挂就成交"的单子，交给调仓计划按「止盈」平掉。
        if (is_long && base * (1.0 + tp_pct) <= mid) || (!is_long && base * (1.0 - tp_pct) >= mid) {
            log.push(format!("{coin}: 已越过止盈线，由调仓计划平仓"));
            continue;
        }
        let (buy, px) = take_profit_order(is_long, base, tp_pct, m.sz_decimals);
        if px <= 0.0 {
            failed.push(format!("{coin}: 价位精度不足，无法挂出有效止盈价"));
            continue;
        }
        let size = round_size(pos.size.abs(), m.sz_decimals);
        if size <= 0.0 || size * mid < 10.0 {
            continue;
        }
        match exec.resting_reduce_order(coin, buy, size, px).await {
            // oid 为 0 表示挂出去的瞬间就成交了（价格已经越过止盈线）。
            // 这不是失败，但也不能报成「挂上」。
            Ok(0) => filled += 1,
            Ok(oid) => {
                oids.push(oid);
                placed += 1;
            }
            Err(e) => failed.push(format!("{coin}: {e}")),
        }
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }
    log.push(format!(
        "止盈单：挂上 {placed} 个{}（±{:.0}%）{}",
        if filled > 0 {
            format!("，{filled} 个挂出即成交")
        } else {
            String::new()
        },
        tp_pct * 100.0,
        if failed.is_empty() {
            String::new()
        } else {
            format!("，未挂 {}", failed.join("; "))
        }
    ));
    Ok((log, oids))
}

/// 供外部按钮调用：先撤自己的旧单，再按参考价重挂。
pub async fn refresh_take_profits(
    exec: &Exec,
    markets: &HashMap<String, MarketInfo>,
    tp_pct: f64,
    reference: &HashMap<String, f64>,
) -> Result<(Vec<String>, Vec<u64>)> {
    let mut log = Vec::new();
    let mut oids: Vec<u64> = Vec::new();
    // 先确认「一定挂得上」再去撤旧单：撤单成功而挂单失败（取不到价、缺元数据）
    // 会让账户在下次调仓前完全失去止盈保护，而且日志还会显示成功。
    if tp_pct > 0.0 {
        anyhow::ensure!(!markets.is_empty(), "市场元数据为空，拒绝撤销现有止盈单");
        let acct = exec.account().await?;
        if !acct.positions.is_empty() {
            let coins: Vec<String> = acct.positions.keys().cloned().collect();
            let mids = trader::fetch_mids(exec, &coins).await;
            anyhow::ensure!(!mids.is_empty(), "取不到中间价，拒绝撤销现有止盈单");
        }
    }
    match cancel_our_take_profits(exec).await {
        Ok(n) if n > 0 => log.push(format!("撤销旧止盈单 {n} 个")),
        Ok(_) => {}
        Err(e) => return Err(e.context("撤销旧止盈单失败")),
    }
    let (lines, placed_oids) = place_take_profits(exec, markets, tp_pct, reference).await?;
    oids.extend(placed_oids);
    log.extend(lines);
    Ok((log, oids))
}

/// 交易所允许的最小价位（和 `order_price` 的精度规则一致）。
/// 止盈单的方向与挂单价。多头要「卖出」且挂在市价上方，空头要「买入」且挂在
/// 市价下方 —— 方向写反会变成加仓，价格写反会立刻成交变成市价单。
///
/// 低价币的价位精度可能不够（一个 tick 就超过止盈幅度），取整会把价格推到
/// 市价的错误一侧。这时至少推离一个 tick；推不动就返回 0，让调用方跳过该币。
pub fn take_profit_order(is_long: bool, mid: f64, tp_pct: f64, sz_decimals: i32) -> (bool, f64) {
    if !mid.is_finite() || mid <= 0.0 {
        return (true, 0.0);
    }
    let (buy, raw) = if is_long {
        (false, mid * (1.0 + tp_pct))
    } else {
        (true, mid * (1.0 - tp_pct))
    };
    if !raw.is_finite() || raw <= 0.0 {
        return (buy, 0.0);
    }
    let tick = price_tick(raw, sz_decimals);
    // 挂单要和 IOC 相反：取整方向朝「远离市价」，避免变成立即成交的市价单。
    let mut px = order_price(raw, sz_decimals, !buy);
    let right_side = |p: f64| p > 0.0 && if is_long { p > mid } else { p < mid };
    for _ in 0..4 {
        if right_side(px) {
            return (buy, px);
        }
        px = if is_long { px + tick } else { px - tick };
    }
    if right_side(px) { (buy, px) } else { (buy, 0.0) }
}


impl LiveState {
    /// Current position unrealized PnL; separate from capital change and realized receipts.
    pub fn unrealized_pnl(&self, positions: &[crate::live::LivePosition]) -> f64 {
        positions.iter().map(|p| p.unrealized).sum()
    }
}


/// Import exchange close-fill receipts. An unmatched fill does not establish
/// order intent: it may be a split rebalance fill, a manual close or a trigger.
pub async fn reconcile_fills(exec: &Exec, state: &mut LiveState) -> Result<usize> {
    let v = exec.user_fills(500).await?;
    let arr = v.as_array().context("成交回执格式错误")?;
    reconcile_fill_rows(arr, state)
}

pub(super) fn reconcile_fill_rows(arr: &[serde_json::Value], state: &mut LiveState) -> Result<usize> {
    // 首次对账绝不能回溯：水位为 0 时直接对齐到「现在」，导入 0 笔。
    //
    // 否则 userFills 会把账户有史以来的成交全倒进来 —— 包括这个策略上线前的
    // 几个月、上一套策略的、甚至别的品种的，已实现盈亏会瞬间变成 −623 这种
    // 荒谬数字。这个错误真实发生过。
    if state.reconciled_to == 0 {
        state.reconciled_to = now_ms_pub();
        return Ok(0);
    }
    let mut known: std::collections::HashSet<u64> =
        state.records.iter().filter_map(|r| r.tid).collect();
    // 我们自己下的单在记录里没有 tid（下单回执里就没有），所以**只靠 tid 去重
    // 不够** —— 每次调仓的平仓都会被当成"被动成交"再导入一遍，盈亏双计，
    // 前端也会把换仓写成"止盈"。这里用 (币, 数量, 时间) 再兜一层。
    let ours: Vec<(String, f64, i64)> = state
        .records
        .iter()
        .filter(|r| r.tid.is_none())
        .map(|r| (r.coin.clone(), r.size.abs(), r.ts))
        .collect();
    let watermark = state.reconciled_to;
    let mut added = 0usize;
    let mut max_ts = watermark;

    // ★ 先按【交易所订单号 oid】把成交分组，再用【组总额】去比我们自己的汇总订单。
    //
    // 真实 bug：一笔 85 AERO 的调仓被交易所拆成 13/14/14/2/14… 共 7 笔成交。
    // 我们自己的记录是【汇总的 85】，而原来的逐笔去重拿 13 去比 85，差 72，
    // 远超 1% 容差，于是整组被判成"不是重复"全部导入 —— 实测名义被重复计算
    // 约 9.4%（$2,156 / $22,875），换手与成本指标跟着虚高。
    //
    // 同一笔订单的所有成交共享 oid，所以按 oid 汇总出来的总额才和我们的汇总记录
    // 可比。没有 oid 的成交用 tid 当键（每笔自成一组），行为与原来一致。
    let mut groups: std::collections::BTreeMap<(String, u64), Vec<&serde_json::Value>> =
        std::collections::BTreeMap::new();
    for f in arr {
        let ts = f["time"].as_i64().unwrap_or(0);
        if ts <= watermark {
            continue;
        }
        let Some(tid) = f["tid"].as_u64() else { continue };
        if known.contains(&tid) {
            continue;
        }
        let coin = f["coin"].as_str().unwrap_or("").to_string();
        // 只认主 DEX 的币：HIP-3 的名字带 ':'，我们从不交易它们。
        if coin.is_empty() || coin.contains(':') {
            continue;
        }
        let key = f["oid"].as_u64().unwrap_or(tid);
        groups.entry((coin, key)).or_default().push(f);
    }

    for ((coin, oid), fills) in groups {
        // 这个 oid 是我们自己挂的止盈单吗？是 → 成交标成「止盈成交」✗
        // 否则标「交易所成交补录」✗ 用户才分得清是止盈还是手动/被动平仓 ✗
        let is_tp = oid != 0 && state.tp_oids.contains(&oid);
        // 这一笔交易所订单【总共】成交了多少
        let total: f64 = fills
            .iter()
            .map(|f| {
                f["sz"]
                    .as_str()
                    .and_then(|x| x.parse::<f64>().ok())
                    .unwrap_or(0.0)
                    .abs()
            })
            .sum();
        let group_ts = fills
            .iter()
            .map(|f| f["time"].as_i64().unwrap_or(0))
            .max()
            .unwrap_or(0);
        // 我们自己发过的单（汇总一条）只要和这一【整组】对得上，整组跳过。
        let dupe = ours
            .iter()
            .any(|(c, s, t)| {
                c == &coin
                    && (s - total).abs() <= 1e-9_f64.max(total * 0.01)
                    && (t - group_ts).abs() <= 180_000
            });
        if dupe {
            continue;
        }
        // 【按交易订单合并】✗ 交易所一笔订单会被拆成很多条成交 ✗
        // 逐条记录会让页面出现 8 行 MON ✗ 和交易所开着 Aggregate 的视图对不上 ✗
        // 这里把同一 oid 的成交合并成一条 ✗ 数量相加 ✗ 成交价按名义加权 ✗
        let mut total_sz = 0.0f64;
        let mut total_notional = 0.0f64;
        let mut total_pnl = 0.0f64;
        let mut last_ts = 0i64;
        let mut first_tid: Option<u64> = None;
        let mut side_buy = false;
        let mut any = false;
        for f in fills {
            let ts = f["time"].as_i64().unwrap_or(0);
            let Some(tid) = f["tid"].as_u64() else { continue };
            // 组内也要查 known：没有 oid 的成交用 tid 当分组键，同一 tid 的
            // 重复行会落进同一组，必须在导入前再挡一次（旧代码是逐笔挡的）。
            if known.contains(&tid) {
                continue;
            }
            let sz: f64 = f["sz"].as_str().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0).abs();
            let px: f64 = f["px"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0.0);
            let dir = f["dir"].as_str().unwrap_or("");
            let pnl: f64 = f["closedPnl"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0.0);
            // 只补「不是我们主动发单」的成交；我们自己发的单在执行时已经记过了。
            if !dir.contains("Close") {
                continue;
            }
            total_sz += sz;
            total_notional += sz * px;
            total_pnl += pnl;
            last_ts = last_ts.max(ts);
            side_buy = f["side"].as_str() == Some("B");
            if first_tid.is_none() {
                first_tid = Some(tid);
            }
            known.insert(tid);
            any = true;
        }
        if any && total_sz > 0.0 {
            let avg_px = total_notional / total_sz;
            state.records.push(LiveRecord {
                ts: last_ts,
                coin: coin.clone(),
                side: if side_buy { "买".into() } else { "卖".into() },
                action: if is_tp { "止盈成交".into() } else { "交易所成交补录".into() },
                size: total_sz,
                price: avg_px,
                notional: total_notional,
                result: if is_tp {
                    format!("止盈单成交 · 已实现 {:+.2}", total_pnl)
                } else {
                    format!("交易所回执 · 已实现 {:+.2}", total_pnl)
                },
                live: true,
                pnl: Some(total_pnl),
                tid: first_tid,
                entry_px: None,
                reduce_only: true,
            });
            added += 1;
            if last_ts > max_ts {
                max_ts = last_ts;
            }
        }
    }
    if added > 0 || max_ts > watermark {
        state.reconciled_to = max_ts;
    }
    Ok(added)
}
