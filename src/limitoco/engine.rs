//! Offline-testable Limit-OCO shadow ledger & 60s intrabar execution state machine.

use super::signals::{compute_indicators, detect_sleeve_a, detect_sleeve_b, SeriesIndicators};
use super::{
    Bar1h, BtcRegime, ClosedTrade, LimitOcoState, OpenPosition, PendingOrder, SignalOrder,
    SleeveStat, UniversePreviewRow, COMMODITY_SYMBOLS, DAY_MS, HOUR_MS,
};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// 影子账本推演核心（完全离线可测试）
// ---------------------------------------------------------------------------

impl LimitOcoState {
    pub(super) fn record_closed_trade(
        &mut self,
        ts_ms: i64,
        pos: &OpenPosition,
        exit_px: f64,
        exit_fee: f64,
        pnl_price: f64,
        reason: &str,
    ) {
        self.cash += pnl_price - exit_fee;
        self.total_fees += exit_fee;
        let total_fee = pos.fee_usd + exit_fee;
        let net_pnl = pnl_price - total_fee + pos.funding_usd;
        let risk_usd = pos.risk_usd.max(1e-9);
        let r_mult = net_pnl / risk_usd;
        self.realized_pnl += net_pnl;

        self.trades.push(ClosedTrade {
            sleeve: pos.sleeve.clone(),
            symbol: pos.symbol.clone(),
            side: pos.side,
            entry_ts: pos.entry_ts,
            exit_ts: ts_ms,
            hold_hours: ((ts_ms - pos.entry_ts) / HOUR_MS).max(0),
            entry_px: pos.entry_px,
            exit_px,
            sl_px: pos.sl_px,
            tp_px: pos.tp_px,
            sl_pct: pos.sl_pct,
            tp_pct: pos.tp_pct,
            qty: pos.qty,
            notional: pos.qty * pos.entry_px,
            risk_usd,
            net_pnl,
            r_mult,
            fee_usd: total_fee,
            funding_usd: pos.funding_usd,
            reason: reason.into(),
        });
        if self.trades.len() > 500 {
            let drain = self.trades.len() - 500;
            self.trades.drain(0..drain);
        }

        if self.config.cooldown_hours > 0 {
            self.cooldowns.insert(
                format!("{}:{}", pos.sleeve, pos.symbol),
                ts_ms + self.config.cooldown_hours * HOUR_MS,
            );
        }

        self.roll_daily_breaker(ts_ms);
        self.daily_r_pnl += r_mult;
        if self.config.daily_loss_limit_r > 0.0 && self.daily_r_pnl <= -self.config.daily_loss_limit_r {
            if !self.daily_breaker_active {
                self.daily_breaker_active = true;
                self.log_event(
                    ts_ms,
                    "BREAKER",
                    "",
                    "",
                    format!(
                        "触发日内熔断：今日已实现 {:+.2}R <= -{:.1}R，暂停新开仓至次日 UTC 00:00",
                        self.daily_r_pnl, self.config.daily_loss_limit_r
                    ),
                );
            }
        }

        let side_cn = if pos.side == 1 { "做多" } else { "做空" };
        self.log_event(
            ts_ms,
            reason,
            &pos.symbol,
            &pos.sleeve,
            format!(
                "[{}] {} {} {} 平仓 @ {:.6} · {:+.2} USDT ({:+.2}R)",
                pos.sleeve, pos.symbol, side_cn, reason, exit_px, net_pnl, r_mult
            ),
        );
    }

    pub fn recompute_stats(&mut self, mark_prices: &HashMap<String, f64>, ts_ms: i64, record_history: bool) -> f64 {
        let mut unreal = 0.0;
        let mut gross_notional = 0.0;
        let mut net_notional = 0.0;

        for (sym, pos) in self.positions.iter_mut() {
            let px = mark_prices.get(sym).copied().filter(|&v| v > 0.0).unwrap_or(pos.mark_px);
            pos.mark_px = px;
            let u_pnl = pos.qty * if pos.side == 1 { px - pos.entry_px } else { pos.entry_px - px };
            let notional = pos.qty * px;
            pos.notional = notional;
            pos.unreal_pnl = u_pnl;
            pos.unreal_r = u_pnl / pos.risk_usd.max(1e-9);
            pos.hold_hours = ((ts_ms - pos.entry_ts) / HOUR_MS).max(0);
            pos.progress_pct = if pos.side == 1 {
                if px >= pos.entry_px {
                    ((px - pos.entry_px) / (pos.tp_px - pos.entry_px).max(1e-12) * 100.0).min(100.0)
                } else {
                    (-((pos.entry_px - px) / (pos.entry_px - pos.sl_px).max(1e-12) * 100.0)).max(-100.0)
                }
            } else if px <= pos.entry_px {
                ((pos.entry_px - px) / (pos.entry_px - pos.tp_px).max(1e-12) * 100.0).min(100.0)
            } else {
                (-((px - pos.entry_px) / (pos.sl_px - pos.entry_px).max(1e-12) * 100.0)).max(-100.0)
            };
            unreal += u_pnl;
            gross_notional += notional;
            net_notional += notional * (pos.side as f64);
        }

        for (sym, pend) in self.pending.iter_mut() {
            let px = mark_prices.get(sym).copied().filter(|&v| v > 0.0).unwrap_or(pend.mark_px);
            pend.mark_px = px;
            pend.dist_to_fill_pct = if pend.side == 1 {
                (px - pend.limit_px) / px.max(1e-12) * 100.0
            } else {
                (pend.limit_px - px) / px.max(1e-12) * 100.0
            };
        }

        let equity = self.cash + unreal;
        let cap = self.capital.max(1e-9);
        self.equity = equity;
        self.unrealized_pnl = unreal;
        self.pnl = equity - cap;
        self.pnl_pct = (equity / cap - 1.0) * 100.0;
        self.gross_notional = gross_notional;
        self.net_notional = net_notional;
        self.gross_leverage = if equity > 0.0 { gross_notional / equity } else { 0.0 };

        if equity > self.peak_equity {
            self.peak_equity = equity;
        }
        let dd = if self.peak_equity > 0.0 {
            (self.peak_equity - equity) / self.peak_equity * 100.0
        } else {
            0.0
        };
        if dd > self.max_dd_pct {
            self.max_dd_pct = dd;
        }
        self.days_elapsed = ((ts_ms - self.started_at) as f64 / DAY_MS as f64).max(0.0);
        self.last_update = ts_ms;
        self.next_bar_ms = (ts_ms.div_euclid(HOUR_MS) + 1) * HOUR_MS;

        // Prune expired cooldowns
        self.cooldowns.retain(|_, &mut until| until > ts_ms);

        // Aggregate trade metrics
        if !self.trades.is_empty() {
            let mut wins = 0usize;
            let mut w_sum = 0.0;
            let mut l_sum = 0.0;
            let mut tot_r = 0.0;
            for t in &self.trades {
                tot_r += t.r_mult;
                if t.net_pnl > 0.0 {
                    wins += 1;
                    w_sum += t.net_pnl;
                } else {
                    l_sum += t.net_pnl.abs();
                }
            }
            self.win_rate = (wins as f64) / (self.trades.len() as f64) * 100.0;
            self.profit_factor = if l_sum > 1e-9 { w_sum / l_sum } else if w_sum > 0.0 { 99.0 } else { 0.0 };
            self.total_r = tot_r;
            self.avg_r = tot_r / (self.trades.len() as f64);
        } else {
            self.win_rate = 0.0;
            self.profit_factor = 0.0;
            self.total_r = 0.0;
            self.avg_r = 0.0;
        }

        for (slv, max_p) in [("A", self.config.sleeve_a_max_pos), ("B", self.config.sleeve_b_max_pos)] {
            let mut trades_n = 0usize;
            let mut wins_n = 0usize;
            let mut ws = 0.0;
            let mut ls = 0.0;
            let mut sum_r = 0.0;
            let mut net_pnl = 0.0;
            for t in &self.trades {
                if t.sleeve == slv {
                    trades_n += 1;
                    sum_r += t.r_mult;
                    net_pnl += t.net_pnl;
                    if t.net_pnl > 0.0 {
                        wins_n += 1;
                        ws += t.net_pnl;
                    } else {
                        ls += t.net_pnl.abs();
                    }
                }
            }
            let open_n = self.positions.values().filter(|p| p.sleeve == slv).count();
            let pend_n = self.pending.values().filter(|p| p.sleeve == slv).count();
            self.sleeve_stats.insert(
                slv.into(),
                SleeveStat {
                    open: open_n,
                    max: max_p,
                    pending: pend_n,
                    trades: trades_n,
                    wins: wins_n,
                    win_rate: if trades_n > 0 { (wins_n as f64) / (trades_n as f64) * 100.0 } else { 0.0 },
                    sum_r,
                    net_pnl,
                    pf: if ls > 1e-9 { ws / ls } else if ws > 0.0 { 99.0 } else { 0.0 },
                },
            );
        }

        if record_history {
            let pt = (ts_ms, (equity * 100.0).round() / 100.0, (self.gross_leverage * 1000.0).round() / 1000.0);
            if let Some(last) = self.history.last_mut() {
                if (last.0 - ts_ms).abs() < 55 * 60_000 {
                    *last = pt;
                } else {
                    self.history.push(pt);
                }
            } else {
                self.history.push(pt);
            }
            if self.history.len() > 4000 {
                let drain = self.history.len() - 4000;
                self.history.drain(0..drain);
            }
        }
        equity
    }

    #[cfg(test)]
    pub fn process_closed_bar(
        &mut self,
        bar_ts: i64,
        bars_by_sym: &HashMap<String, Vec<Bar1h>>,
        funding_by_sym: &HashMap<String, Vec<(i64, f64)>>,
        asset_class_by_sym: &HashMap<String, String>,
        live_marks: Option<&HashMap<String, f64>>,
    ) {
        self.process_closed_bar_with_opts(bar_ts, bars_by_sym, funding_by_sym, asset_class_by_sym, live_marks, true);
    }

    pub fn process_closed_bar_with_opts(
        &mut self,
        bar_ts: i64,
        bars_by_sym: &HashMap<String, Vec<Bar1h>>,
        funding_by_sym: &HashMap<String, Vec<(i64, f64)>>,
        asset_class_by_sym: &HashMap<String, String>,
        live_marks: Option<&HashMap<String, f64>>,
        allow_new_signals: bool,
    ) {
        self.roll_daily_breaker(bar_ts);
        let bar_close_ms = bar_ts + HOUR_MS;

        // Strictly slice up to `bar_ts` so future bars can never enter indicators
        let mut sliced: HashMap<String, (&[Bar1h], SeriesIndicators, usize)> = HashMap::new();
        for (sym, bars) in bars_by_sym {
            if let Some(idx) = bars.iter().rposition(|b| b.open_time_ms == bar_ts) {
                let prefix = &bars[..=idx];
                let fev = funding_by_sym.get(sym).map(|v| v.as_slice()).unwrap_or(&[]);
                let ind = compute_indicators(
                    prefix,
                    fev,
                    self.config.split_jump_pct,
                    self.config.split_block_hours,
                );
                sliced.insert(sym.clone(), (prefix, ind, idx));
            }
        }

        let Some((btc_bars, btc_ind, btc_idx)) = sliced.get("BTCUSDT") else {
            self.log_event(bar_close_ms, "WARN", "", "", "缺少 BTCUSDT 已收盘 K 线，跳过本小时扫描".into());
            self.last_bar_ms = bar_ts;
            return;
        };
        let btc_p = btc_bars[*btc_idx].close;
        let btc_e168 = btc_ind.ema168[*btc_idx];
        let btc_e336 = btc_ind.ema336[*btc_idx];
        let bull_ok = btc_p > btc_e168 && btc_p > btc_e336;
        let short_ok = btc_p < btc_e168;
        let btc_ctx = BtcRegime {
            bar_ts,
            price: btc_p,
            ema168: btc_e168,
            ema336: btc_e336,
            bull_ok,
            short_ok,
            funding_rate: btc_ind.last_funding[*btc_idx],
            label: if bull_ok {
                "多头趋势 (A多允许)".into()
            } else if short_ok {
                "空头趋势 (A空+B空活跃)".into()
            } else {
                "震荡过渡 (仅B空/观望)".into()
            },
        };
        self.btc_regime = btc_ctx.clone();

        // 1. Manage existing positions: Funding -> SL -> TP -> Time-Stop
        let open_syms: Vec<String> = self.positions.keys().cloned().collect();
        for sym in open_syms {
            let Some((s_bars, _, idx)) = sliced.get(&sym) else {
                continue;
            };
            let b = &s_bars[*idx];
            let mut pos = match self.positions.remove(&sym) {
                Some(p) => p,
                None => continue,
            };
            pos.hold_bars += 1;

            if let Some(fev) = funding_by_sym.get(&sym) {
                let f_sum: f64 = fev
                    .iter()
                    .filter(|&&(fts, _)| fts > pos.last_fund_ts && fts <= bar_close_ms)
                    .map(|&(_, r)| r)
                    .sum();
                if f_sum != 0.0 {
                    let f_cf = -(pos.side as f64) * pos.qty * b.close * f_sum;
                    self.cash += f_cf;
                    self.total_funding += f_cf;
                    pos.funding_usd += f_cf;
                    pos.last_fund_ts = bar_close_ms;
                }
            }

            // 1a. Hard SL first
            if (pos.side == 1 && b.low <= pos.sl_px) || (pos.side == -1 && b.high >= pos.sl_px) {
                let fill_px = if (pos.side == 1 && b.open < pos.sl_px) || (pos.side == -1 && b.open > pos.sl_px) {
                    b.open
                } else {
                    pos.sl_px
                };
                let pnl_p = pos.qty * if pos.side == 1 { fill_px - pos.entry_px } else { pos.entry_px - fill_px };
                let fee = pos.qty * fill_px * self.config.sl_cost;
                self.record_closed_trade(bar_close_ms, &pos, fill_px, fee, pnl_p, "SL");
                continue;
            }

            // 1b. Maker TP
            if (pos.side == 1 && b.high >= pos.tp_px * (1.0 + self.config.tp_pen))
                || (pos.side == -1 && b.low <= pos.tp_px * (1.0 - self.config.tp_pen))
            {
                let pnl_p = pos.qty * if pos.side == 1 { pos.tp_px - pos.entry_px } else { pos.entry_px - pos.tp_px };
                let fee = pos.qty * pos.tp_px * self.config.maker_fee;
                self.record_closed_trade(bar_close_ms, &pos, pos.tp_px, fee, pnl_p, "TP");
                continue;
            }

            // 1c. Time-stop
            if pos.hold_bars >= pos.max_hold_bars {
                let pnl_p = pos.qty * if pos.side == 1 { b.close - pos.entry_px } else { pos.entry_px - b.close };
                let fee = pos.qty * b.close * self.config.time_exit_cost;
                self.record_closed_trade(bar_close_ms, &pos, b.close, fee, pnl_p, "TIME");
                continue;
            }

            self.positions.insert(sym, pos);
        }

        let bar_closes: HashMap<String, f64> = sliced
            .iter()
            .map(|(s, (bs, _, idx))| (s.clone(), bs[*idx].close))
            .collect();
        let equity = self.recompute_stats(&bar_closes, bar_close_ms, false);
        let mut gross_notional = self.gross_notional;

        // 2. Process resting GTX pending orders
        let mut pend_order: Vec<(String, f64)> = self.pending.iter().map(|(s, p)| (s.clone(), p.score)).collect();
        pend_order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));

        for (sym, _) in pend_order {
            let Some((s_bars, _, idx)) = sliced.get(&sym) else {
                continue;
            };
            let b = &s_bars[*idx];
            let mut pend = match self.pending.remove(&sym) {
                Some(p) => p,
                None => continue,
            };
            pend.age_hours += 1;

            let ran_away = (pend.side == 1 && b.high >= pend.tp_px) || (pend.side == -1 && b.low <= pend.tp_px);
            let lim = pend.limit_px;
            let mut filled = (pend.side == 1 && b.low <= lim * (1.0 - self.config.entry_pen))
                || (pend.side == -1 && b.high >= lim * (1.0 + self.config.entry_pen));
            if pend.age_hours == 1 && ((pend.side == 1 && b.open <= lim) || (pend.side == -1 && b.open >= lim)) {
                filled = false;
            }

            if filled {
                let slv_pos_n = self.positions.values().filter(|p| p.sleeve == pend.sleeve).count();
                let slv_same_n = self
                    .positions
                    .values()
                    .filter(|p| p.sleeve == pend.sleeve && p.side == pend.side)
                    .count();
                let (slv_max, slv_ss_max) = if pend.sleeve == "A" {
                    (self.config.sleeve_a_max_pos, self.config.sleeve_a_max_same_side)
                } else {
                    (self.config.sleeve_b_max_pos, self.config.sleeve_b_max_same_side)
                };
                if self.daily_breaker_active
                    || self.positions.len() >= self.config.max_total_positions
                    || slv_pos_n >= slv_max
                    || slv_same_n >= slv_ss_max
                    || equity <= 0.0
                    || self.positions.contains_key(&sym)
                {
                    continue;
                }
                let sl_dist = (lim - pend.sl_px).abs() / lim;
                if sl_dist < self.config.min_sl_pct || sl_dist > self.config.max_sl_pct {
                    continue;
                }
                let size_base = if self.config.compounding { equity } else { self.capital };
                let risk_usd = size_base * self.config.risk_per_trade;
                let notional = (risk_usd / sl_dist).min(size_base * self.config.max_pos_lev);
                if gross_notional + notional > equity * self.config.max_total_gross_lev {
                    continue;
                }

                let qty = notional / lim;
                let efee = notional * self.config.maker_fee;
                self.cash -= efee;
                self.total_fees += efee;
                self.funnel.filled += 1;

                let tp_pct = (pend.tp_px - lim).abs() / lim * 100.0;
                let pos_obj = OpenPosition {
                    sleeve: pend.sleeve.clone(),
                    symbol: sym.clone(),
                    asset_class: asset_class_by_sym.get(&sym).cloned().unwrap_or_else(|| "crypto".into()),
                    side: pend.side,
                    entry_ts: bar_close_ms,
                    last_fund_ts: bar_close_ms,
                    entry_px: lim,
                    mark_px: b.close,
                    qty,
                    notional,
                    risk_usd: notional * sl_dist,
                    sl_px: pend.sl_px,
                    tp_px: pend.tp_px,
                    sl_pct: sl_dist * 100.0,
                    tp_pct,
                    max_hold_bars: pend.max_hold_bars,
                    max_hold_hours: pend.max_hold_bars,
                    hold_bars: 0,
                    hold_hours: 0,
                    fee_usd: efee,
                    funding_usd: 0.0,
                    unreal_pnl: 0.0,
                    unreal_r: 0.0,
                    progress_pct: 0.0,
                    reason: pend.reason.clone(),
                };

                if (pend.side == 1 && b.low <= pend.sl_px) || (pend.side == -1 && b.high >= pend.sl_px) {
                    let pnl_p = qty * if pend.side == 1 { pend.sl_px - lim } else { lim - pend.sl_px };
                    let sfee = qty * pend.sl_px * self.config.sl_cost;
                    self.record_closed_trade(bar_close_ms, &pos_obj, pend.sl_px, sfee, pnl_p, "FILL_BAR_SL");
                    continue;
                }

                gross_notional += notional;
                let side_cn = if pend.side == 1 { "做多" } else { "做空" };
                self.log_event(
                    bar_close_ms,
                    "FILL",
                    &sym,
                    &pend.sleeve,
                    format!(
                        "[{}] {} {} Maker成交 @ {:.6} · 名义 ${:.0} · SL {:.6} (-{:.1}%) / TP {:.6} (+{:.1}%)",
                        pend.sleeve, sym, side_cn, lim, notional, pend.sl_px, sl_dist * 100.0, pend.tp_px, tp_pct
                    ),
                );
                self.positions.insert(sym, pos_obj);
                continue;
            }

            if ran_away {
                self.funnel.runaway += 1;
                continue;
            }
            if pend.age_hours >= pend.expire_hours {
                self.funnel.expired += 1;
                continue;
            }
            self.pending.insert(sym, pend);
        }

        // 3. Dynamic Top-80 ranking & new closed-bar signal generation
        let mut scored: Vec<(f64, String)> = Vec::new();
        for (sym, (_, ind, idx)) in &sliced {
            if *idx >= 340 && !ind.split_block[*idx] {
                let dqv7 = ind.avg_daily_qv7[*idx];
                if dqv7 >= self.config.min_daily_vol_usd {
                    scored.push((dqv7, sym.clone()));
                }
            }
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)));
        let top_liquid: Vec<String> = scored.iter().take(self.config.top_n_liquid).map(|x| x.1.clone()).collect();

        let mut preview = Vec::new();
        for (dqv7, sym) in scored.iter().take(24) {
            if let Some((s_bars, ind, idx)) = sliced.get(sym) {
                let cl = s_bars[*idx].close;
                let prev24 = s_bars[idx.saturating_sub(24)].close;
                let ret24 = if prev24 > 0.0 { (cl / prev24 - 1.0) * 100.0 } else { 0.0 };
                let is_comm = asset_class_by_sym.get(sym).map(|s| s == "commodity").unwrap_or(false)
                    || COMMODITY_SYMBOLS.contains(&sym.as_str());
                preview.push(UniversePreviewRow {
                    symbol: sym.clone(),
                    daily_qv7_m: (dqv7 / 1e6 * 10.0).round() / 10.0,
                    price: cl,
                    ret24h: (ret24 * 100.0).round() / 100.0,
                    funding_bp: (ind.last_funding[*idx] * 10_000.0 * 100.0).round() / 100.0,
                    is_commodity: is_comm,
                });
            }
        }
        self.universe.liquid_count = top_liquid.len();
        self.universe.top80_preview = preview;
        self.funnel.bars_scanned += top_liquid.len() as u64;

        if !allow_new_signals {
            // 冷启动距上根整点收盘已过久，或历史补扫阶段：仅更新 BTC 罗盘与 Top-80，不回溯挂陈旧单
        } else if self.daily_breaker_active {
            self.funnel.breaker_skipped += 1;
        } else if self.positions.len() < self.config.max_total_positions {
            // Sleeve A
            if self.config.enabled_a
                && self.positions.values().filter(|p| p.sleeve == "A").count() < self.config.sleeve_a_max_pos
            {
                let mut cands = Vec::new();
                for sym in &top_liquid {
                    if self.positions.contains_key(sym) || self.pending.contains_key(sym) {
                        continue;
                    }
                    if self.cooldowns.get(&format!("A:{sym}")).copied().unwrap_or(0) > bar_close_ms {
                        continue;
                    }
                    if let Some((s_bars, ind, idx)) = sliced.get(sym) {
                        let (sig, tag) = detect_sleeve_a(sym, s_bars, ind, *idx, &btc_ctx, &self.config);
                        match tag {
                            "a_vol_blocked" => { self.funnel.a_raw_cross += 1; self.funnel.a_vol_blocked += 1; }
                            "a_btc_blocked" => { self.funnel.a_raw_cross += 1; self.funnel.a_btc_blocked += 1; }
                            "a_fund_blocked" => { self.funnel.a_raw_cross += 1; self.funnel.a_fund_blocked += 1; }
                            "a_sl_rejected" => { self.funnel.a_raw_cross += 1; self.funnel.a_sl_rejected += 1; }
                            "ok" => { self.funnel.a_raw_cross += 1; }
                            _ => {}
                        }
                        if let Some(s) = sig {
                            cands.push(s);
                        }
                    }
                }
                cands.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.symbol.cmp(&b.symbol)));
                for sig in cands {
                    if self.pending.values().filter(|p| p.sleeve == "A").count() >= self.config.sleeve_a_max_pos * 2 {
                        break;
                    }
                    if !self.pending.contains_key(&sig.symbol) {
                        let lm = live_marks.and_then(|m| m.get(&sig.symbol).copied());
                        if self.place_pending(sig, bar_close_ms, lm) {
                            self.funnel.a_placed += 1;
                        }
                    }
                }
            }

            // Sleeve B
            if self.config.enabled_b
                && self.positions.values().filter(|p| p.sleeve == "B").count() < self.config.sleeve_b_max_pos
            {
                let mut cands = Vec::new();
                for sym in &top_liquid {
                    if self.positions.contains_key(sym) || self.pending.contains_key(sym) {
                        continue;
                    }
                    if self.cooldowns.get(&format!("B:{sym}")).copied().unwrap_or(0) > bar_close_ms {
                        continue;
                    }
                    if let Some((s_bars, ind, idx)) = sliced.get(sym) {
                        let (sig, tag) = detect_sleeve_b(sym, s_bars, ind, *idx, &self.config);
                        match tag {
                            "b_rsi_blocked" => { self.funnel.b_raw_rip += 1; self.funnel.b_rsi_blocked += 1; }
                            "b_fund_blocked" => { self.funnel.b_raw_rip += 1; self.funnel.b_fund_blocked += 1; }
                            "b_sl_rejected" => { self.funnel.b_raw_rip += 1; self.funnel.b_sl_rejected += 1; }
                            "ok" => { self.funnel.b_raw_rip += 1; }
                            _ => {}
                        }
                        if let Some(s) = sig {
                            cands.push(s);
                        }
                    }
                }
                cands.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.symbol.cmp(&b.symbol)));
                for sig in cands {
                    if self.pending.values().filter(|p| p.sleeve == "B").count() >= self.config.sleeve_b_max_pos * 2 {
                        break;
                    }
                    if !self.pending.contains_key(&sig.symbol) {
                        let lm = live_marks.and_then(|m| m.get(&sig.symbol).copied());
                        if self.place_pending(sig, bar_close_ms, lm) {
                            self.funnel.b_placed += 1;
                        }
                    }
                }
            }
        }

        self.last_bar_ms = bar_ts;
        self.recompute_stats(live_marks.unwrap_or(&bar_closes), bar_close_ms, true);
    }

    /// 挂出纯 Maker GTX 限价单；若传入实时标记价 `live_mark`，严格校验 Post-Only 被动性
    ///（做多挂单价必须严格低于现价，做空挂单价必须严格高于现价，否则按币安 GTX 规则拒单）。
    pub(super) fn place_pending(&mut self, sig: SignalOrder, ts_ms: i64, live_mark: Option<f64>) -> bool {
        if let Some(mp) = live_mark.filter(|&v| v > 0.0) {
            let violates_gtx = (sig.side == 1 && sig.limit_px >= mp * 0.9998)
                || (sig.side == -1 && sig.limit_px <= mp * 1.0002);
            if violates_gtx {
                let side_cn = if sig.side == 1 { "做多买单" } else { "做空卖单" };
                self.log_event(
                    ts_ms,
                    "GTX_REJECT",
                    &sig.symbol,
                    &sig.sleeve,
                    format!(
                        "[{}] {} {} GTX 拒单：挂单价 {:.6} 已不满足被动挂单条件（当前市价 {:.6}）",
                        sig.sleeve, sig.symbol, side_cn, sig.limit_px, mp
                    ),
                );
                return false;
            }
        }

        let mark_px = live_mark.filter(|&v| v > 0.0).unwrap_or(sig.signal_close);
        let dist_pct = if sig.side == 1 {
            (mark_px - sig.limit_px) / mark_px.max(1e-12) * 100.0
        } else {
            (sig.limit_px - mark_px) / mark_px.max(1e-12) * 100.0
        };
        let side_cn = if sig.side == 1 { "挂买单" } else { "挂卖单" };
        self.log_event(
            ts_ms,
            "SIGNAL",
            &sig.symbol,
            &sig.sleeve,
            format!(
                "[{}] {} {} GTX @ {:.6}（现价 {:.6}，距成交 +{:.2}%）· SL -{:.1}% / TP +{:.1}% ({})",
                sig.sleeve,
                sig.symbol,
                side_cn,
                sig.limit_px,
                mark_px,
                dist_pct,
                sig.sl_pct * 100.0,
                sig.tp_pct * 100.0,
                sig.reason
            ),
        );
        self.pending.insert(
            sig.symbol.clone(),
            PendingOrder {
                sleeve: sig.sleeve,
                symbol: sig.symbol,
                side: sig.side,
                limit_px: sig.limit_px,
                mark_px,
                dist_to_fill_pct: dist_pct,
                sl_px: sig.sl_px,
                tp_px: sig.tp_px,
                sl_pct: sig.sl_pct * 100.0,
                tp_pct: sig.tp_pct * 100.0,
                expire_hours: sig.expire_bars,
                max_hold_bars: sig.max_hold_bars,
                age_hours: 0,
                score: (sig.score * 10_000.0).round() / 10_000.0,
                funding_bp: (sig.funding_rate * 10_000.0 * 100.0).round() / 100.0,
                created_ts: ts_ms,
                reason: sig.reason,
            },
        );
        true
    }

    /// 60 秒实时盘中盯市推进：已挂在盘口的 GTX 限价单一旦被实时市价穿越即刻成交转持仓；
    /// 已持仓位一旦触及硬止损 SL 或 Maker 止盈 TP 即刻平仓落袋，无需干等到下一个整点。
    pub fn process_intrabar_marks(
        &mut self,
        mark_prices: &HashMap<String, f64>,
        asset_class_by_sym: &HashMap<String, String>,
        ts_ms: i64,
    ) {
        self.roll_daily_breaker(ts_ms);

        // 1. 持仓实时触发 SL / TP（硬止损优先）
        let open_syms: Vec<String> = self.positions.keys().cloned().collect();
        for sym in open_syms {
            let Some(&px) = mark_prices.get(&sym).filter(|&&v| v > 0.0) else {
                continue;
            };
            let mut pos = match self.positions.remove(&sym) {
                Some(p) => p,
                None => continue,
            };
            if (pos.side == 1 && px <= pos.sl_px) || (pos.side == -1 && px >= pos.sl_px) {
                let fill_px = if (pos.side == 1 && px < pos.sl_px) || (pos.side == -1 && px > pos.sl_px) {
                    px
                } else {
                    pos.sl_px
                };
                let pnl_p = pos.qty * if pos.side == 1 { fill_px - pos.entry_px } else { pos.entry_px - fill_px };
                let fee = pos.qty * fill_px * self.config.sl_cost;
                self.record_closed_trade(ts_ms, &pos, fill_px, fee, pnl_p, "SL");
                continue;
            }
            if (pos.side == 1 && px >= pos.tp_px * (1.0 + self.config.tp_pen))
                || (pos.side == -1 && px <= pos.tp_px * (1.0 - self.config.tp_pen))
            {
                let pnl_p = pos.qty * if pos.side == 1 { pos.tp_px - pos.entry_px } else { pos.entry_px - pos.tp_px };
                let fee = pos.qty * pos.tp_px * self.config.maker_fee;
                self.record_closed_trade(ts_ms, &pos, pos.tp_px, fee, pnl_p, "TP");
                continue;
            }
            pos.mark_px = px;
            self.positions.insert(sym, pos);
        }

        let equity = self.recompute_stats(mark_prices, ts_ms, false);
        let mut gross_notional = self.gross_notional;

        // 2. 已挂在盘口的 GTX 挂单：市价穿透挂单价即刻成交，或先触及 TP 则逃逸撤单
        let mut pend_order: Vec<(String, f64)> = self.pending.iter().map(|(s, p)| (s.clone(), p.score)).collect();
        pend_order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));

        for (sym, _) in pend_order {
            let Some(&px) = mark_prices.get(&sym).filter(|&&v| v > 0.0) else {
                continue;
            };
            let pend = match self.pending.remove(&sym) {
                Some(p) => p,
                None => continue,
            };
            let lim = pend.limit_px;
            let ran_away = (pend.side == 1 && px >= pend.tp_px) || (pend.side == -1 && px <= pend.tp_px);
            let filled = (pend.side == 1 && px <= lim * (1.0 - self.config.entry_pen))
                || (pend.side == -1 && px >= lim * (1.0 + self.config.entry_pen));

            if filled {
                let slv_pos_n = self.positions.values().filter(|p| p.sleeve == pend.sleeve).count();
                let slv_same_n = self
                    .positions
                    .values()
                    .filter(|p| p.sleeve == pend.sleeve && p.side == pend.side)
                    .count();
                let (slv_max, slv_ss_max) = if pend.sleeve == "A" {
                    (self.config.sleeve_a_max_pos, self.config.sleeve_a_max_same_side)
                } else {
                    (self.config.sleeve_b_max_pos, self.config.sleeve_b_max_same_side)
                };
                if self.daily_breaker_active
                    || self.positions.len() >= self.config.max_total_positions
                    || slv_pos_n >= slv_max
                    || slv_same_n >= slv_ss_max
                    || equity <= 0.0
                    || self.positions.contains_key(&sym)
                {
                    continue;
                }
                let sl_dist = (lim - pend.sl_px).abs() / lim;
                if sl_dist < self.config.min_sl_pct || sl_dist > self.config.max_sl_pct {
                    continue;
                }
                let size_base = if self.config.compounding { equity } else { self.capital };
                let risk_usd = size_base * self.config.risk_per_trade;
                let notional = (risk_usd / sl_dist).min(size_base * self.config.max_pos_lev);
                if gross_notional + notional > equity * self.config.max_total_gross_lev {
                    continue;
                }

                let qty = notional / lim;
                let efee = notional * self.config.maker_fee;
                self.cash -= efee;
                self.total_fees += efee;
                self.funnel.filled += 1;

                let tp_pct = (pend.tp_px - lim).abs() / lim * 100.0;
                let pos_obj = OpenPosition {
                    sleeve: pend.sleeve.clone(),
                    symbol: sym.clone(),
                    asset_class: asset_class_by_sym.get(&sym).cloned().unwrap_or_else(|| "crypto".into()),
                    side: pend.side,
                    entry_ts: ts_ms,
                    last_fund_ts: ts_ms,
                    entry_px: lim,
                    mark_px: px,
                    qty,
                    notional,
                    risk_usd: notional * sl_dist,
                    sl_px: pend.sl_px,
                    tp_px: pend.tp_px,
                    sl_pct: sl_dist * 100.0,
                    tp_pct,
                    max_hold_bars: pend.max_hold_bars,
                    max_hold_hours: pend.max_hold_bars,
                    hold_bars: 0,
                    hold_hours: 0,
                    fee_usd: efee,
                    funding_usd: 0.0,
                    unreal_pnl: 0.0,
                    unreal_r: 0.0,
                    progress_pct: 0.0,
                    reason: pend.reason.clone(),
                };

                if (pend.side == 1 && px <= pend.sl_px) || (pend.side == -1 && px >= pend.sl_px) {
                    let pnl_p = qty * if pend.side == 1 { pend.sl_px - lim } else { lim - pend.sl_px };
                    let sfee = qty * pend.sl_px * self.config.sl_cost;
                    self.record_closed_trade(ts_ms, &pos_obj, pend.sl_px, sfee, pnl_p, "FILL_BAR_SL");
                    continue;
                }

                gross_notional += notional;
                let side_cn = if pend.side == 1 { "做多" } else { "做空" };
                self.log_event(
                    ts_ms,
                    "FILL",
                    &sym,
                    &pend.sleeve,
                    format!(
                        "[{}] {} {} 盘中 Maker 成交 @ {:.6}（市价 {:.6}）· 名义 ${:.0} · SL {:.6} / TP {:.6}",
                        pend.sleeve, sym, side_cn, lim, px, notional, pend.sl_px, pend.tp_px
                    ),
                );
                self.positions.insert(sym, pos_obj);
                continue;
            }

            if ran_away {
                self.funnel.runaway += 1;
                self.log_event(
                    ts_ms,
                    "RUNAWAY",
                    &sym,
                    &pend.sleeve,
                    format!("[{}] {} 市价 {:.6} 已直接触及止盈位 {:.6}，撤销未成交挂单", pend.sleeve, sym, px, pend.tp_px),
                );
                continue;
            }

            self.pending.insert(sym, pend);
        }

        self.recompute_stats(mark_prices, ts_ms, false);
    }
}

