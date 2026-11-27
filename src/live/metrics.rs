//! Time-weighted return (TWR) curves, health scorecard, and daily execution statistics.

use super::{now_ms_pub, DailyExec, EquityPoint, LiveRecord, LiveState};

/// 从 `"成交 512.0@0.084119"` 里取出成交价。
fn fill_px(result: &str) -> Option<f64> {
    let at = result.split("成交 ").nth(1)?.split('@').nth(1)?;
    let num: String = at
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    num.parse().ok()
}

/// 把成交记录聚合成每日执行指标。
///
/// **换手的分母必须是【当天的净值】**，不是当前净值。
/// 真实 bug：用当前净值当所有天的分母，账户从 $550 充到 $3,100 之后，
/// 10-07 那天的换手被算成 0.44（真实 2.41），低估 5 倍 —— 正好把
/// "线上换手和回测差不多的 1.7/天"这个结论掩盖成"线上换手只有 0.3/天"。
pub fn daily_execution(
    records: &[LiveRecord],
    history: &[EquityPoint],
    fallback_equity: f64,
) -> Vec<DailyExec> {
    use std::collections::BTreeMap;
    // 每天用【当天最后一条】净值；没有当天记录就沿用最近一个更早的值。
    let mut eq_by_day: BTreeMap<i64, f64> = BTreeMap::new();
    for p in history.iter().filter(|p| p.equity.is_finite() && p.equity > 0.0) {
        eq_by_day.insert(p.ts / 86_400_000, p.equity);
    }
    let equity_on = |day: i64| -> f64 {
        eq_by_day
            .range(..=day)
            .next_back()
            .map(|(_, v)| *v)
            .unwrap_or(fallback_equity)
    };
    // day -> (笔数, 名义, 滑点合计, 有滑点的笔数)
    let mut acc: BTreeMap<i64, (usize, f64, f64, usize)> = BTreeMap::new();
    for r in records.iter().filter(|r| r.live) {
        let e = acc.entry(r.ts / 86_400_000).or_insert((0, 0.0, 0.0, 0));
        e.0 += 1;
        e.1 += r.notional.abs();
        if let Some(fill) = fill_px(&r.result) {
            if r.price > 0.0 && fill > 0.0 {
                // 买入成交价偏高 = 吃亏；卖出成交价偏低 = 吃亏。统一成"正数=不利"。
                let slip = if r.side.starts_with('买') {
                    (fill - r.price) / r.price
                } else {
                    (r.price - fill) / r.price
                };
                e.2 += slip;
                e.3 += 1;
            }
        }
    }
    acc.into_iter()
        .map(|(day, (orders, notional, ss, sn))| {
            let eq = equity_on(day);
            DailyExec {
                day,
                orders,
                notional,
                turnover: if eq > 0.0 { notional / eq } else { 0.0 },
                avg_slip_pct: (sn > 0).then(|| ss / sn as f64 * 100.0),
            }
        })
        .collect()
}


/// 把净值历史换算成【策略盈亏】（扣除基准本金与出入金 ✗）。
///
/// 服务端的按日/按月胜率原先直接比较原始净值 ✗ 一旦入金 ✗ 当天就会被算成
/// 大赚日（实测入金 991 出现 +22% 的假上涨）✓ 这里统一扣掉 ✓
/// 与前端 strategyHistory 的口径完全一致 ✓
pub fn adjusted_history(state: &LiveState) -> Vec<(i64, f64)> {
    let mut flows: Vec<(i64, f64)> = state
        .capital_flows
        .iter()
        .filter(|f| f.amount.is_finite())
        .map(|f| (f.effective_ts.unwrap_or(f.ts), f.amount))
        .collect();
    flows.sort_by_key(|(ts, _)| *ts);
    let baseline = state.capital_baseline.as_ref().map(|b| b.equity).unwrap_or(0.0);
    let mut out = Vec::with_capacity(state.history.len());
    let mut i = 0usize;
    let mut cash = 0.0f64;
    let mut pts: Vec<_> = state
        .history
        .iter()
        .filter(|p| p.equity.is_finite() && p.equity > 0.0)
        .collect();
    pts.sort_by_key(|p| p.ts);
    for p in pts {
        while i < flows.len() && flows[i].0 <= p.ts {
            cash += flows[i].1;
            i += 1;
        }
        out.push((p.ts, p.equity - baseline - cash));
    }
    out
}

/// 天序号（自 1970-01-01 起 ✗）→ 自然月序号（year*12 + month-1 ✓）。
/// 用于"按自然月"胜率分组 ✗ 不能拿 day/30 近似（月边界会错到 4 号 ✗）。
pub(super) fn month_ordinal(day: i64) -> i64 {
    // Howard Hinnant 的 civil_from_days ✗ 与 hlcarry::ymd_from_ms 同一套算法 ✓
    let z = day + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    y * 12 + (m - 1)
}

/// 净值指数序列（净值 ÷ 当时的净投入本金 ✗ 起点=1 ✓）。
///
/// 为什么不用"扣除出入金后的盈亏"直接算收益率：那个数可能是负的
/// （现在就是 −$900）✗ 拿负数当分母算涨幅会得到 −23% 这种假数字 ✓
/// 用指数就没有这个问题 ✓
pub fn index_history(state: &LiveState) -> Vec<(i64, f64)> {
    // 时间加权收益指数（TWR）✗ 起点 1.0 ✓
    //
    // 关键: 出入金那一步【跳过】✗ 不参与收益计算 ✓
    // 之前用"净值÷本金"✗ 在亏损状态下会被入金虚增
    //   （0.85 亏着的时候入 2,000 ✗ 比值会跳到 0.89 ✗ 假的 +4% ✓）
    let mut flows: Vec<i64> = state
        .capital_flows
        .iter()
        .filter(|f| f.amount.is_finite() && f.amount != 0.0)
        .map(|f| f.effective_ts.unwrap_or(f.ts))
        .collect();
    flows.sort_unstable();
    let mut pts: Vec<_> = state
        .history
        .iter()
        .filter(|p| p.equity.is_finite() && p.equity > 0.0)
        .collect();
    pts.sort_by_key(|p| p.ts);
    let mut out = Vec::with_capacity(pts.len());
    let mut idx = 1.0f64;
    let mut prev: Option<f64> = None;
    let mut fi = 0usize;
    for p in pts {
        let mut had_flow = false;
        while fi < flows.len() && flows[fi] <= p.ts {
            had_flow = true;
            fi += 1;
        }
        if let Some(pe) = prev {
            if !had_flow && pe > 0.0 {
                idx *= p.equity / pe;
            }
        }
        prev = Some(p.equity);
        out.push((p.ts, idx));
    }
    out
}


/// ML 子账户时间加权收益指数（TWR，起点 = 1.0）。
///
/// 跳过所有已登记的 `ml_capital_flows` 时间点；同时对历史未补录的瞬时大额划转
/// （间隔 ≤ 2 小时且单步净值突变超过 ±15%，如 11-16 划出 $630）自动视为资金划转步跳过，
/// 避免曲线上出现虚假的 -26% 暴跌。
pub fn ml_index_history(state: &LiveState) -> Vec<(i64, f64)> {
    let mut flows: Vec<i64> = state
        .ml_capital_flows
        .iter()
        .filter(|f| f.amount.is_finite() && f.amount != 0.0)
        .map(|f| f.effective_ts.unwrap_or(f.ts))
        .collect();
    flows.sort_unstable();
    let mut pts: Vec<_> = state
        .ml_history
        .iter()
        .filter(|p| p.equity.is_finite() && p.equity > 0.0)
        .collect();
    pts.sort_by_key(|p| p.ts);
    let mut out = Vec::with_capacity(pts.len());
    let mut idx = 1.0f64;
    let mut prev: Option<(i64, f64)> = None;
    let mut fi = 0usize;
    for p in pts {
        let mut had_flow = false;
        while fi < flows.len() && flows[fi] <= p.ts {
            had_flow = true;
            fi += 1;
        }
        if let Some((pts_prev, pe)) = prev {
            if !had_flow && pe > 0.0 {
                let ratio = p.equity / pe;
                let dt_ms = (p.ts - pts_prev).max(0);
                // 未补录的瞬时资金划转保护：<= 2 小时内单步变化超 ±15% 视作外部划转
                let unrecorded_transfer = dt_ms <= 2 * 3_600_000 && (ratio - 1.0).abs() > 0.15;
                if !unrecorded_transfer {
                    idx *= ratio;
                }
            }
        }
        prev = Some((p.ts, p.equity));
        out.push((p.ts, idx));
    }
    out
}

/// 五项健康指标（供前端红黄绿显示）。
///
/// 每项都带 status: green / yellow / red / unknown（样本不足）。
/// 阈值是在回测分布上定的 ✗ 不是拍脑袋 ✓
pub fn scorecard(state: &LiveState, health: &crate::health::Health) -> Vec<serde_json::Value> {
    let idx = index_history(state);
    let now = now_ms_pub();
    let thirty = 30 * 86_400_000i64;
    let cut = now - thirty;
    let mut window_days = 30i64;
    let mut start = idx.iter().filter(|(ts, _)| *ts <= cut).last().copied();
    if start.is_none() {
        start = idx.first().copied();
        if let (Some((a, _)), Some((b, _))) = (start, idx.last()) {
            window_days = ((b - a) / 86_400_000).max(1);
        }
    }
    let end = idx.last().copied();

    // ① 区间收益 = 指数比值（指数=净值/本金 ✗ 起点为 1 ✓）
    let enough = idx.len() >= 2;
    let (ret30, s_ret) = match (start, end) {
        (Some((_, a)), Some((_, b))) if enough && a > 1e-9 => {
            let r = (b / a - 1.0) * 100.0;
            let s = if r >= -9.0 { "green" } else if r >= -20.0 { "yellow" } else { "red" };
            (Some(r), s)
        }
        _ => (None, "unknown"),
    };

    // ② 毛收益 vs 手续费（同一区间 ✗ 毛 = 净 + 费）
    let fee30: f64 = state
        .records
        .iter()
        .filter(|r| r.live && r.ts >= (now - window_days * 86_400_000))
        .map(|r| r.notional.abs() * 0.00045)
        .sum();
    // 手续费换算成"占本金的比例"：分母用【窗口内的平均净值】✗ 不能用指数 ✓
    let avg_equity: f64 = {
        let lo = now - window_days * 86_400_000;
        let vals: Vec<f64> = state
            .history
            .iter()
            .filter(|p| p.ts >= lo && p.equity.is_finite() && p.equity > 0.0)
            .map(|p| p.equity)
            .collect();
        if vals.is_empty() { 0.0 } else { vals.iter().sum::<f64>() / vals.len() as f64 }
    };
    let (gross30, s_gross, fee_pct_out) = match (ret30, end) {
        (Some(net), Some(_)) if enough && avg_equity > 1.0 => {
            let fee_pct = fee30 / avg_equity * 100.0;
            let fee_pct = if fee_pct.is_finite() && fee_pct > 0.0 { fee_pct } else { 0.0 };
            let gross = net + fee_pct;
            let s = if gross >= fee_pct * 2.0 && gross > 0.0 {
                "green"
            } else if gross >= fee_pct || gross >= 0.0 {
                "yellow"
            } else {
                "red"
            };
            (Some(gross), s, Some(fee_pct))
        }
        _ => (None, "unknown", None),
    };

    // ③ 90 日 IC
    let (ic, s_ic) = match health.ic90 {
        Some(v) => {
            let s = if v >= 0.02 { "green" } else if v >= 0.009 { "yellow" } else { "red" };
            (Some(v), s)
        }
        None => (None, "unknown"),
    };

    // ④ 回撤（在 TWR 指数上分别算「当前回撤」与「历史最大回撤」，不再混淆！）
    let (cur_dd, max_dd, s_dd) = {
        let mut peak = f64::NEG_INFINITY;
        let mut worst = 0.0f64;
        for (_, v) in idx.iter() {
            if *v > peak { peak = *v; }
            if peak > 0.0 {
                let cur = (peak - *v) / peak * 100.0;
                if cur > worst { worst = cur; }
            }
        }
        let last_v = idx.last().map(|(_, v)| *v).unwrap_or(0.0);
        if idx.len() < 2 || peak <= 0.0 {
            (None, None, "unknown")
        } else {
            let cur_now = ((peak - last_v) / peak * 100.0).max(0.0);
            let s = if cur_now < 20.0 { "green" } else if cur_now < 34.0 { "yellow" } else { "red" };
            (Some(cur_now), Some(worst.max(0.0)), s)
        }
    };

    vec![
        serde_json::json!({"key":"ret30","name":"区间收益","value":ret30,"unit":"%","status":s_ret,
            "good":format!("近 {} 天 · ≥ −9%（回测最差月 −8.7%）", window_days),"bad":"< −20%"}),
        serde_json::json!({"key":"gross","name":"毛收益 / 手续费","value":gross30,"unit":"%","status":s_gross,
            "fee":fee30,"fee_pct":fee_pct_out,"net":ret30,
            "good":"毛收益 ≥ 手续费 2 倍","bad":"毛收益 < 手续费"}),
        serde_json::json!({"key":"ic90","name":format!("90 日 IC ({})", health.factor_label),"value":ic,"unit":"","status":s_ic,
            "good":"> 0.02","bad":"< 0.009"}),
        serde_json::json!({"key":"dd","name":"当前回撤","value":cur_dd,"max_dd":max_dd,"unit":"%","status":s_dd,
            "good":"越小越好 · < 20% 为健康","bad":"> 34%"}),
    ]
}

