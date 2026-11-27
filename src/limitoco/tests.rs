use super::*;
use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_bars(closes: &[f64], spread: f64, qv: f64, start_ms: i64) -> Vec<Bar1h> {
        closes
            .iter()
            .enumerate()
            .map(|(i, &c)| {
                let o = if i > 0 { closes[i - 1] } else { c };
                Bar1h {
                    open_time_ms: start_ms + (i as i64) * HOUR_MS,
                    open: o,
                    high: o.max(c) * (1.0 + spread),
                    low: o.min(c) * (1.0 - spread),
                    close: c,
                    volume: qv / c.max(1e-9),
                    quote_volume: qv,
                }
            })
            .collect()
    }

    #[test]
    fn contract_classification_excludes_us_stocks_and_keeps_commodities() {
        assert_eq!(
            classify_binance_contract(&json!({"symbol":"BTCUSDT","status":"TRADING","contractType":"PERPETUAL","quoteAsset":"USDT"})),
            "crypto"
        );
        assert_eq!(
            classify_binance_contract(&json!({"symbol":"XAUUSDT","status":"TRADING","contractType":"PERPETUAL","quoteAsset":"USDT","underlyingSubType":["TRADFI"]})),
            "commodity"
        );
        assert_eq!(
            classify_binance_contract(&json!({"symbol":"CLUSDT","status":"TRADING","contractType":"PERPETUAL","quoteAsset":"USDT"})),
            "commodity"
        );
        assert_eq!(
            classify_binance_contract(&json!({"symbol":"NVDAUSDT","status":"TRADING","contractType":"PERPETUAL","quoteAsset":"USDT"})),
            "stock"
        );
        assert_eq!(
            classify_binance_contract(&json!({"symbol":"FOOEQUSDT","status":"TRADING","contractType":"PERPETUAL","quoteAsset":"USDT","underlyingSubType":["EQUITY"]})),
            "stock"
        );
        assert_eq!(
            classify_binance_contract(&json!({"symbol":"USDCUSDT","status":"TRADING","contractType":"PERPETUAL","quoteAsset":"USDT"})),
            "excluded"
        );
    }

    #[test]
    fn truncation_invariance_proves_zero_lookahead() {
        let closes: Vec<f64> = (0..520)
            .map(|i| 100.0 + 15.0 * ((i as f64) / 25.0).sin() + 0.03 * (i as f64))
            .collect();
        let bars_full = make_bars(&closes, 0.005, 2_000_000.0, 1_700_000_000_000);
        let funding_full: Vec<(i64, f64)> = (0..520)
            .step_by(8)
            .map(|i| (bars_full[i].open_time_ms, 0.0001 * (i as f64).cos()))
            .collect();

        let cut = 381usize;
        let idx = cut - 1;
        let cut_close = bars_full[idx].open_time_ms + HOUR_MS;
        let bars_trunc = &bars_full[..cut];
        let funding_trunc: Vec<(i64, f64)> = funding_full.iter().copied().filter(|&(t, _)| t < cut_close).collect();

        let full = compute_indicators(&bars_full, &funding_full, 0.35, 72);
        let trunc = compute_indicators(bars_trunc, &funding_trunc, 0.35, 72);

        assert!((full.ema168[idx] - trunc.ema168[idx]).abs() < 1e-12);
        assert!((full.ema336[idx] - trunc.ema336[idx]).abs() < 1e-12);
        assert!((full.atr24[idx] - trunc.atr24[idx]).abs() < 1e-12);
        assert!((full.rsi2_d[idx] - trunc.rsi2_d[idx]).abs() < 1e-12);
        assert!((full.avg_qv24[idx] - trunc.avg_qv24[idx]).abs() < 1e-12);
        assert!((full.avg_daily_qv7[idx] - trunc.avg_daily_qv7[idx]).abs() < 1e-12);
        assert_eq!(full.split_block[idx], trunc.split_block[idx]);
        assert!((full.last_funding[idx] - trunc.last_funding[idx]).abs() < 1e-12);
    }

    #[test]
    fn funding_alignment_is_strictly_before_bar_close() {
        let bars = make_bars(&[100.0; 10], 0.002, 1_000_000.0, 0);
        let ind = compute_indicators(&bars, &[(0, 0.0001), (HOUR_MS, 0.0009)], 0.35, 72);
        assert!((ind.last_funding[0] - 0.0001).abs() < 1e-12);
        assert!((ind.last_funding[1] - 0.0009).abs() < 1e-12);
    }

    #[test]
    fn sleeve_a_btc_ema336_guard_and_sleeve_b_short_rip() {
        let cfg = LimitOcoConfig::default();
        let mut closes = vec![100.0; 350];
        for i in 0..10 {
            closes.push(100.0 + ((i + 1) as f64) * 0.2);
        }
        let mut bars = make_bars(&closes, 0.006, 1_500_000.0, 1_700_000_000_000);
        let last_idx = bars.len() - 1;
        bars[last_idx] = Bar1h {
            open_time_ms: bars[last_idx].open_time_ms,
            open: 102.0,
            high: 106.0,
            low: 101.8,
            close: 105.5,
            volume: 30_000.0,
            quote_volume: 3_500_000.0,
        };
        let ind = compute_indicators(&bars, &[(bars[0].open_time_ms, 0.00002)], 0.35, 72);

        // Fix 1: BTC < EMA336 blocks A_Long
        let btc_chop = BtcRegime {
            bar_ts: bars[last_idx].open_time_ms,
            price: 95_000.0,
            ema168: 94_000.0,
            ema336: 98_000.0,
            bull_ok: false,
            short_ok: false,
            funding_rate: 0.0,
            label: String::new(),
        };
        let (sig_blk, tag_blk) = detect_sleeve_a("ETHUSDT", &bars, &ind, last_idx, &btc_chop, &cfg);
        assert!(sig_blk.is_none());
        assert_eq!(tag_blk, "a_btc_blocked");

        // BTC > EMA168 & EMA336 allows A_Long
        let btc_bull = BtcRegime { bull_ok: true, ..btc_chop };
        let (sig_ok, tag_ok) = detect_sleeve_a("ETHUSDT", &bars, &ind, last_idx, &btc_bull, &cfg);
        assert_eq!(tag_ok, "ok");
        let s = sig_ok.unwrap();
        assert_eq!(s.side, 1);
        assert!((s.tp_pct / s.sl_pct - 1.2).abs() < 1e-6);

        // Sleeve B ShortRip: downtrend (< EMA336) + 72h +10% rip + RSI2=100
        let mut b_closes: Vec<f64> = (0..300).map(|i| 200.0 - 0.25 * (i as f64)).collect();
        let rip_base = *b_closes.last().unwrap();
        for k in 1..=72 {
            b_closes.push(rip_base * (1.0 + 0.10 * (k as f64) / 72.0));
        }
        let b_bars = make_bars(&b_closes, 0.005, 2_000_000.0, 1_700_000_000_000);
        let b_ind = compute_indicators(&b_bars, &[(b_bars[0].open_time_ms, 0.0001)], 0.35, 72);
        let (sig_b, tag_b) = detect_sleeve_b("SOLUSDT", &b_bars, &b_ind, b_bars.len() - 1, &cfg);
        assert_eq!(tag_b, "ok");
        let sb = sig_b.unwrap();
        assert_eq!(sb.side, -1);
        assert!((sb.tp_pct / sb.sl_pct - 0.9).abs() < 1e-6);
    }

    #[test]
    fn shadow_lifecycle_fill_tp_cooldown_and_daily_breaker() {
        let cfg = LimitOcoConfig {
            running: true,
            capital: 10_000.0,
            risk_tier: "1.5x".into(),
            risk_per_trade: 0.012,
            max_pos_lev: 2.2,
            max_total_gross_lev: 6.0,
            cooldown_hours: 72,
            daily_loss_limit_r: 2.0,
            ..Default::default()
        };
        let t0 = 1_735_689_600_000i64; // 2025-01-01 00:00 UTC
        let mut st = LimitOcoState::new_with_config(cfg, t0);

        let btc_bars = make_bars(&[100_000.0; 365], 0.002, 50_000_000.0, t0);
        let mut eth_bars = make_bars(&[100.0; 365], 0.004, 5_000_000.0, t0);

        st.pending.insert(
            "ETHUSDT".into(),
            PendingOrder {
                sleeve: "A".into(),
                symbol: "ETHUSDT".into(),
                side: 1,
                limit_px: 99.0,
                mark_px: 100.0,
                dist_to_fill_pct: 1.0,
                sl_px: 95.0,
                tp_px: 107.8,
                sl_pct: 4.04,
                tp_pct: 8.89,
                expire_hours: 12,
                max_hold_bars: 168,
                age_hours: 0,
                score: 2.0,
                funding_bp: 0.0,
                created_ts: btc_bars[350].open_time_ms,
                reason: "test".into(),
            },
        );

        let b351_ts = eth_bars[351].open_time_ms;
        eth_bars[351] = Bar1h {
            open_time_ms: b351_ts,
            open: 100.0,
            high: 101.0,
            low: 98.5,
            close: 100.2,
            volume: 50_000.0,
            quote_volume: 5_000_000.0,
        };
        let mut bm = HashMap::new();
        bm.insert("BTCUSDT".into(), btc_bars.clone());
        bm.insert("ETHUSDT".into(), eth_bars.clone());
        let mut ac = HashMap::new();
        ac.insert("BTCUSDT".into(), "crypto".into());
        ac.insert("ETHUSDT".into(), "crypto".into());

        st.process_closed_bar(b351_ts, &bm, &HashMap::new(), &ac, None);
        assert!(st.positions.contains_key("ETHUSDT"));
        assert!((st.positions["ETHUSDT"].risk_usd - 120.0).abs() < 0.05);

        let b352_ts = eth_bars[352].open_time_ms;
        eth_bars[352] = Bar1h {
            open_time_ms: b352_ts,
            open: 100.2,
            high: 108.5,
            low: 100.0,
            close: 108.0,
            volume: 50_000.0,
            quote_volume: 5_000_000.0,
        };
        bm.insert("ETHUSDT".into(), eth_bars);
        st.process_closed_bar(b352_ts, &bm, &HashMap::new(), &ac, None);
        assert!(!st.positions.contains_key("ETHUSDT"));
        assert_eq!(st.trades.len(), 1);
        assert_eq!(st.trades[0].reason, "TP");
        assert!(st.trades[0].r_mult > 2.1);
        assert_eq!(st.cooldowns["A:ETHUSDT"], b352_ts + HOUR_MS + 72 * HOUR_MS);

        // Trigger daily breaker via simulated SLs on same UTC day
        let dummy = OpenPosition {
            sleeve: "B".into(),
            symbol: "SOLUSDT".into(),
            asset_class: "crypto".into(),
            side: -1,
            entry_ts: b352_ts,
            last_fund_ts: b352_ts,
            entry_px: 100.0,
            mark_px: 100.0,
            qty: 20.0,
            notional: 2000.0,
            risk_usd: 100.0,
            sl_px: 105.0,
            tp_px: 92.5,
            sl_pct: 5.0,
            tp_pct: 7.5,
            max_hold_bars: 96,
            max_hold_hours: 96,
            hold_bars: 1,
            hold_hours: 1,
            fee_usd: 0.4,
            funding_usd: 0.0,
            unreal_pnl: 0.0,
            unreal_r: 0.0,
            progress_pct: 0.0,
            reason: "test".into(),
        };
        for k in 1..=5 {
            st.record_closed_trade(b352_ts + k * HOUR_MS, &dummy, 105.0, 2.1, -100.0, "SL");
        }
        assert!(st.daily_breaker_active);
        st.roll_daily_breaker(b352_ts + 2 * DAY_MS);
        assert!(!st.daily_breaker_active);
    }

    #[test]
    fn gtx_passivity_rejects_impossible_post_only_orders() {
        let mut st = LimitOcoState::new_with_config(LimitOcoConfig::default(), 1_735_689_600_000);
        // Short signal @ 0.045752 when live market is already 0.046100 -> MUST be rejected (GTX Sell must be > mark!)
        let short_sig = SignalOrder {
            sleeve: "B".into(),
            symbol: "SKYUSDT".into(),
            side: -1,
            limit_px: 0.045752,
            sl_px: 0.048928,
            tp_px: 0.040988,
            expire_bars: 12,
            max_hold_bars: 96,
            score: 0.126,
            sl_pct: 0.069,
            tp_pct: 0.104,
            signal_close: 0.04554,
            atr24: 0.00106,
            funding_rate: 0.00005,
            reason: "test".into(),
        };
        assert!(!st.place_pending(short_sig.clone(), 1_735_689_600_000, Some(0.0461)));
        assert!(st.pending.is_empty());
        assert_eq!(st.events.last().unwrap().kind, "GTX_REJECT");

        // Same Short signal @ 0.045752 when live market is 0.04550 (below ask) -> ACCEPTED!
        assert!(st.place_pending(short_sig, 1_735_689_600_000, Some(0.04550)));
        assert!(st.pending.contains_key("SKYUSDT"));
        assert!(st.pending["SKYUSDT"].dist_to_fill_pct > 0.0);

        // Long signal @ 100.0 when live market is already 99.5 -> MUST be rejected (GTX Buy must be < mark!)
        let long_sig = SignalOrder {
            sleeve: "A".into(),
            symbol: "ETHUSDT".into(),
            side: 1,
            limit_px: 100.0,
            sl_px: 95.0,
            tp_px: 111.0,
            expire_bars: 12,
            max_hold_bars: 168,
            score: 2.0,
            sl_pct: 0.05,
            tp_pct: 0.11,
            signal_close: 100.5,
            atr24: 1.4,
            funding_rate: 0.0,
            reason: "test".into(),
        };
        assert!(!st.place_pending(long_sig, 1_735_689_600_000, Some(99.5)));
        assert!(!st.pending.contains_key("ETHUSDT"));
    }

    #[test]
    fn intrabar_marks_fill_resting_gtx_and_close_oco_immediately() {
        let mut st = LimitOcoState::new_with_config(LimitOcoConfig::default(), 1_735_689_600_000);
        let sig = SignalOrder {
            sleeve: "B".into(),
            symbol: "SKYUSDT".into(),
            side: -1,
            limit_px: 0.045752,
            sl_px: 0.048928,
            tp_px: 0.040988,
            expire_bars: 12,
            max_hold_bars: 96,
            score: 0.126,
            sl_pct: 0.0694,
            tp_pct: 0.1041,
            signal_close: 0.04554,
            atr24: 0.00106,
            funding_rate: 0.00005,
            reason: "test".into(),
        };
        // Legitimately placed at 01:00 while mark was 0.04554 (below sell limit)
        assert!(st.place_pending(sig, 1_735_689_600_000, Some(0.04554)));
        assert!(st.pending.contains_key("SKYUSDT"));

        // 15 minutes later mark rises to 0.0461 -> fills immediately into active short position!
        let mut marks = HashMap::new();
        marks.insert("SKYUSDT".into(), 0.0461);
        let mut ac = HashMap::new();
        ac.insert("SKYUSDT".into(), "crypto".into());
        st.process_intrabar_marks(&marks, &ac, 1_735_689_600_000 + 15 * 60_000);
        assert!(!st.pending.contains_key("SKYUSDT"));
        assert!(st.positions.contains_key("SKYUSDT"));
        assert!((st.positions["SKYUSDT"].entry_px - 0.045752).abs() < 1e-9);

        // Later mark drops to 0.04090 (< tp_px * 0.9998) -> TP triggers immediately!
        marks.insert("SKYUSDT".into(), 0.04090);
        st.process_intrabar_marks(&marks, &ac, 1_735_689_600_000 + 45 * 60_000);
        assert!(!st.positions.contains_key("SKYUSDT"));
        assert_eq!(st.trades.len(), 1);
        assert_eq!(st.trades[0].reason, "TP");
        assert!(st.trades[0].r_mult > 1.4);
    }
}
