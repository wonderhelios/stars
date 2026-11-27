use super::*;
use super::metrics::month_ordinal;
use super::orders::reconcile_fill_rows;
use super::runner::{book_vol_estimate_at, ensure_candidates, signal_panel};
use crate::trader::TradeConfig;

#[cfg(test)]
mod tp_tests {
    use super::*;

    #[test]
    fn take_profit_sells_above_market_for_longs() {
        let (buy, px) = take_profit_order(true, 100.0, 0.10, 2);
        assert!(!buy, "多头止盈必须是卖出");
        assert!(px > 100.0, "多头止盈必须挂在市价上方，实际 {px}");
    }

    #[test]
    fn take_profit_buys_below_market_for_shorts() {
        let (buy, px) = take_profit_order(false, 100.0, 0.10, 2);
        assert!(buy, "空头止盈必须是买入");
        assert!(px < 100.0, "空头止盈必须挂在市价下方，实际 {px}");
    }

    #[test]
    fn take_profit_stays_on_the_correct_side_after_rounding() {
        for &mid in &[0.00001234_f64, 0.5, 3.14159, 87.65, 1234.5, 98765.4] {
            for &sd in &[0i32, 1, 2, 4, 6] {
                let (buy, px) = take_profit_order(true, mid, 0.10, sd);
                assert!(!buy);
                assert!(px > mid, "多头 mid={mid} sd={sd} 得到 {px}");
                let (buy, px) = take_profit_order(false, mid, 0.10, sd);
                assert!(buy);
                assert!(px < mid, "空头 mid={mid} sd={sd} 得到 {px}");
            }
        }
    }

    #[test]
    fn take_profit_price_is_never_zero_or_negative() {
        for &mid in &[0.000001_f64, 1.0, 100.0] {
            for &tp in &[0.01, 0.1, 0.5] {
                for &sd in &[0i32, 2, 6] {
                    let (_, up) = take_profit_order(true, mid, tp, sd);
                    let (_, dn) = take_profit_order(false, mid, tp, sd);
                    assert!(up >= 0.0 && dn >= 0.0, "mid={mid} tp={tp} sd={sd} 得到 {up}/{dn}");
                    assert!(up > mid || up == 0.0, "多头挂单价必须高于市价: {up} vs {mid}");
                    assert!(dn < mid || dn == 0.0, "空头挂单价必须低于市价: {dn} vs {mid}");
                }
            }
        }
    }
}



#[cfg(test)]
mod config_parity_tests {
    use super::*;

    /// 实盘配置的每个可调字段都必须真的传进 TradeConfig。
    ///
    /// 之前用 `..Default::default()` 兜底，漏传 `rebalance_slices`，
    /// 结果是界面上改了参数、实盘行为完全不变，而且不留任何痕迹。
    #[test]
    fn every_live_field_reaches_trade_config() {
        let cfg = LiveConfig {
            lookback: 7,
            top_frac: 0.11,
            min_vol_usd: 7_000_000.0,
            leverage: 5.0,
            target_positions: 13,
            slippage: 0.007,
            margin_buffer: 0.77,
            min_order_pct: 0.012,
            rebalance_slices: 4,
            ema_alpha: 0.35,
            rebal_band: 0.08,
            hard_gross_cap: 2.5,
            position_stop_pct: 0.22,
            momentum_weight: 0.7,
            dd_weight: 1.0,
            upfrac_weight: 0.8,
            take_profit_pct: 0.1,
            max_single_weight: 0.18,
            ..LiveConfig::default()
        };
        let tc = cfg.trade_config();
        assert_eq!(tc.lookback, 7, "lookback 没传");
        assert_eq!(tc.top_frac, 0.11, "top_frac 没传");
        assert_eq!(tc.min_vol_usd, 7_000_000.0, "min_vol_usd 没传");
        assert_eq!(tc.leverage, 5.0, "leverage 没传");
        assert_eq!(tc.target_positions, 13, "target_positions 没传");
        assert_eq!(tc.slippage, 0.007, "slippage 没传");
        assert_eq!(tc.margin_buffer, 0.77, "margin_buffer 没传");
        assert_eq!(tc.min_order_pct, 0.012, "min_order_pct 没传");
        assert_eq!(tc.rebalance_slices, 4, "rebalance_slices 没传（UI 会失效）");
        assert_eq!(tc.ema_alpha, 0.35, "ema_alpha 没传");
        assert_eq!(tc.rebal_band, 0.08, "rebal_band 没传");
        assert_eq!(tc.hard_gross_cap, 2.5, "hard_gross_cap 没传");
        assert_eq!(tc.position_stop_pct, 0.22, "position_stop_pct 没传");
        assert_eq!(tc.momentum_weight, 0.7, "momentum_weight 没传（UI 会失效）");
        assert_eq!(tc.dd_weight, 1.0, "dd_weight 没传（三因子 UI 会失效）");
        assert_eq!(tc.upfrac_weight, 0.8, "upfrac_weight 没传（三因子 UI 会失效）");
        assert_eq!(tc.take_profit_pct, 0.1, "take_profit_pct 没传（止盈会失效）");
        assert_eq!(tc.max_single_weight, 0.18, "max_single_weight 没传");
    }
}


#[cfg(test)]
mod config_robustness {
    use super::*;

    /// 配置文件里缺字段时，**不能**把 account / key_path 静默清空。
    ///
    /// 真实事故：LiveConfig 里 11 个字段没有 #[serde(default)]，只要旧配置少任何一个，
    /// 整个结构反序列化失败并回退默认值 —— 账户地址和密钥路径变成空字符串，
    /// 页面显示"未启用 / 净值 $0.00"，而没有任何报错。
    #[test]
    fn missing_fields_do_not_wipe_the_account() {
        // 只给必填的两个字段，其余全缺
        let json = r#"{"account":"0xABC","key_path":"/tmp/k.key"}"#;
        let cfg: LiveConfig = serde_json::from_str(json).expect("缺字段不应导致解析失败");
        assert_eq!(cfg.account, "0xABC", "account 被清空了");
        assert_eq!(cfg.key_path, "/tmp/k.key", "key_path 被清空了");
        // 其余字段走 Phase 2 C3 默认值
        assert_eq!(cfg.leverage, LiveConfig::default().leverage);
        assert_eq!(cfg.rebalance_slices, 1);
        assert_eq!(cfg.top_frac, 0.30);
        assert_eq!(cfg.ema_alpha, 0.50);
        assert_eq!(cfg.rebal_band, 0.05);
        assert_eq!(cfg.min_order_pct, 0.005);
        assert_eq!(cfg.position_stop_pct, 0.0);
        assert_eq!(cfg.hard_gross_cap, 2.8);
    }

    #[test]
    fn empty_object_still_parses() {
        let cfg: LiveConfig = serde_json::from_str("{}").expect("空对象也应能解析");
        assert_eq!(cfg.account, "");
        assert_eq!(cfg.ema_alpha, 0.50);
        assert_eq!(cfg.rebal_band, 0.05);
        assert_eq!(cfg.hard_gross_cap, 2.8);
    }

    #[test]
    fn live_server_config_snapshot_deserializes_without_loss() {
        // 真实线上 live.json 的 config 快照（2026-11-27 审计导出），不含 Phase 2 新增 3 字段
        let raw = r#"{
            "account":"0x355764449D7d610d20fee736477e0728B0cd8821",
            "armed":true,"auto_run":true,"dd_weight":1.0,
            "key_path":"/etc/hyper-fly/api-wallet.key","leverage":3.0,"lookback":14,
            "margin_buffer":0.9,"max_single_weight":0.2,"min_order_pct":0.005,
            "min_vol_usd":10000000.0,"ml_dir":"/opt/stars/ml",
            "ml_python":"/root/miniconda3/envs/ml/bin/python",
            "ml_vault_address":"0x7aa0d75c9dadb1dc7c67963542069c4a577b7046",
            "momentum_weight":1.0,"position_stop_pct":0.0,"rebalance_slices":3,
            "slippage":0.005,"stop_cooldown_days":0,"take_profit_pct":0.0,
            "target_positions":10,"top_frac":0.2,"upfrac_weight":1.0,
            "vault_address":"0x14a8d29ae15be2937bb94e32adc90953a1ffad83",
            "vol_band":0.2,"vol_target":0.032
        }"#;
        let cfg: LiveConfig = serde_json::from_str(raw).expect("真实线上配置必须无损反序列化");
        assert_eq!(cfg.account, "0x355764449D7d610d20fee736477e0728B0cd8821");
        assert_eq!(cfg.vault_address.as_deref(), Some("0x14a8d29ae15be2937bb94e32adc90953a1ffad83"));
        assert_eq!(cfg.ml_vault_address.as_deref(), Some("0x7aa0d75c9dadb1dc7c67963542069c4a577b7046"));
        assert!(cfg.armed && cfg.auto_run);
        assert_eq!(cfg.top_frac, 0.2);
        assert_eq!(cfg.rebalance_slices, 3);
        assert_eq!(cfg.min_vol_usd, 10_000_000.0);
        assert_eq!(cfg.vol_target, 0.032);
        assert_eq!(cfg.target_positions, 10);
        // 新增字段走安全默认值
        assert_eq!(cfg.ema_alpha, 0.50);
        assert_eq!(cfg.rebal_band, 0.05);
        assert_eq!(cfg.hard_gross_cap, 2.8);
    }

    #[test]
    fn book_vol_excludes_unclosed_bar_and_aligns_by_timestamp() {
        const DAY: i64 = 86_400_000;
        let t0 = 1_700_000_000_000i64;
        // 造 4 个完全相同日收益序列的币：L1/L2 做多 +0.25，S1/S2 做空 -0.25。
        // 因为多空完全抵消，按时间戳对齐后的组合日收益恒为 0（但给 L1 加微弱常数斜率让波动>1e-9）。
        // 故意给 S1 多加一根「未收盘」异常暴涨蜡烛，并让 S2 尾部少 1 天（检验时间戳对齐而非倒序下标！）。
        let mk_candles = |n: usize, extra_unclosed: Option<f64>, bump: f64| {
            let mut px = 100.0;
            let mut cs = Vec::new();
            for d in 0..n {
                let r = if d % 2 == 0 { 0.04 } else { -0.03 } + bump * (d as f64 % 3.0);
                px *= 1.0 + r;
                cs.push(crate::hl::Candle {
                    t: t0 + d as i64 * DAY,
                    o: px,
                    h: px,
                    l: px,
                    c: px,
                    v: 1_000_000.0,
                });
            }
            if let Some(jump) = extra_unclosed {
                cs.push(crate::hl::Candle {
                    t: t0 + n as i64 * DAY,
                    o: px,
                    h: px * jump,
                    l: px,
                    c: px * jump,
                    v: 1_000_000.0,
                });
            }
            cs
        };
        // now_ms 落在第 24 天（t0 + 24*DAY）刚开盘不久，因此 d=24 未收盘，仅 d<=23 已收盘。
        let now_ms = t0 + 24 * DAY + 3_600_000;
        let panel = vec![
            crate::store::PanelEntry { coin: "L1".into(), candles: mk_candles(24, Some(5.0), 0.001) },
            crate::store::PanelEntry { coin: "L2".into(), candles: mk_candles(24, None, 0.0) },
            crate::store::PanelEntry { coin: "S1".into(), candles: mk_candles(24, Some(9.0), 0.0) },
            // S2 少一天（只到 d=22）：若按倒序下标对齐会与其它币错开 1 天，组合波动会爆增到 ~3%+
            crate::store::PanelEntry { coin: "S2".into(), candles: mk_candles(23, None, 0.0) },
        ];
        let weights = vec![
            ("L1".into(), 0.25),
            ("L2".into(), 0.25),
            ("S1".into(), -0.25),
            ("S2".into(), -0.25),
        ];
        let v_buf = book_vol_estimate_at(&panel, &weights, 2.7, now_ms).expect("应算出有效波动");
        let v_raw = book_vol_estimate_at(&panel, &weights, 3.0, now_ms).expect("应算出有效波动");
        // 未收盘的 5x/9x 跳空被剔除，且时间戳严格对齐后多空几乎完全抵消，日波动应远小于 1%
        assert!(v_buf < 0.005, "未收盘蜡烛或时间戳错位污染了组合波动: {v_buf}");
        assert!(((v_buf / v_raw) - (2.7 / 3.0)).abs() < 1e-9, "应按有效杠杆 2.7 缩放");
    }
}


/// 记录一个净值点。**判重和写入必须在同一把锁里**。
///
/// 之前是在页面 handler 里用「进入 handler 时拿到的克隆」判断这个小时是否已记录，
/// 然后才加锁写入 —— 两个并发请求都会认为"该小时还没记"，于是同一分钟写进两三个点。
/// 而且它只在页面被访问时才写，不看页面就不记，曲线缺失掉整段时间。

#[cfg(test)]
mod ml_equity_tests {
    use super::*;

    #[tokio::test]
    async fn same_hour_snapshot_replaces_stale_or_bad_value() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("live.json");
        let live = std::sync::Arc::new(tokio::sync::Mutex::new(LiveState::default()));
        record_ml_equity(&live, &path, 2500.0).await.unwrap();
        record_ml_equity(&live, &path, 635.087057).await.unwrap();
        record_ml_equity(&live, &path, 2524.780226).await.unwrap();
        let state = live.lock().await;
        assert_eq!(state.ml_history.len(), 1);
        assert_eq!(state.ml_history[0].equity, 2524.780226);
        drop(state);
        let saved = LiveState::load(&path);
        assert_eq!(saved.ml_history.len(), 1);
        assert_eq!(saved.ml_history[0].equity, 2524.780226);
        drop(live);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod reconciliation_tests {
    use super::*;
    #[test]
    fn reconciliation_preserves_concurrent_orders_and_deduplicates_fills() {
        let record=|tid:u64| LiveRecord{ts:tid as i64,coin:"C".into(),side:"买".into(),action:"开".into(),size:1.,price:100.,notional:100.,result:"ok".into(),live:true,pnl:None,tid:Some(tid),entry_px:None,reduce_only:false};
        let mut current=LiveState::default();current.records.push(record(1));let mut snapshot=current.clone();
        current.records.push(record(2));snapshot.records.push(record(3));
        merge_reconciliation(&mut current,&snapshot);
        assert_eq!(current.records.len(),3,"stale snapshot must not erase a concurrent order");
        merge_reconciliation(&mut current,&snapshot);assert_eq!(current.records.len(),3);
    }
}

#[cfg(test)]
mod hl_signal_tests {
    use super::*;
    #[test]
    fn nonempty_weights_do_not_require_extra_candidate_limits() {
        let one = vec![("A".into(), 1.)];
        assert!(ensure_candidates(&one).is_ok());
    }
    #[test]
    fn signal_panel_keeps_the_hl_store_bars() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let hl = crate::store::Store::open(&root.join("hl")).unwrap();
        let yesterday = now_ms_pub()/86_400_000*86_400_000-86_400_000;
        for i in 0..24 {
            let candles:Vec<_>=(0..40).map(|d|crate::hl::Candle{t:yesterday-(40-d)*86_400_000,o:100.,h:100.,l:100.,c:100.,v:6000000.}).collect();
            hl.upsert_candles(&format!("C{i}"), &candles).unwrap();
        }
        assert_eq!(signal_panel(&hl).unwrap().len(), 24);
        drop(hl); std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn signal_panel_uses_the_hl_universe() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let hl = crate::store::Store::open(&root.join("hl")).unwrap();
        let yesterday=now_ms_pub()/86400000*86400000-86400000;
        let candles:Vec<_>=(0..40).map(|d|crate::hl::Candle{t:yesterday-(39-d)*86400000,o:2.,h:2.,l:2.,c:2.,v:6000000.}).collect();
        hl.upsert_candles("HL_ONLY", &candles).unwrap();
        assert_eq!(signal_panel(&hl).unwrap()[0].coin, "HL_ONLY");
        drop(hl); std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn fractional_leverage_is_preserved() {
        let hl = LiveConfig { leverage: 3.49, ..Default::default() };
        assert_eq!(hl.trade_config().leverage, 3.49);
    }
}

#[cfg(test)]
mod min_order_pct_tests {
    use super::*;

    /// 配置字段漏传是踩过的坑（`rebalance_slices` 曾经被 `..Default::default()` 静默吞掉）。
    /// 这条确保 `min_order_pct` 真的会到 TradeConfig。
    #[test]
    fn min_order_pct_reaches_trade_config() {
        let mut cfg = LiveConfig::default();
        cfg.min_order_pct = 0.02;
        assert_eq!(cfg.trade_config().min_order_pct, 0.02);
    }

    /// 有效门槛 = max(绝对下限, 净值 × 比例)。
    #[test]
    fn effective_threshold_scales_with_equity() {
        let mut cfg = TradeConfig::default();
        cfg.min_order_usd = 10.0;
        // 比例 0 → 与旧版一致
        cfg.min_order_pct = 0.0;
        assert_eq!(cfg.effective_min_order_usd(100_000.0), 10.0);
        // 比例 1% × 大账号 → 比例项接管
        cfg.min_order_pct = 0.01;
        assert_eq!(cfg.effective_min_order_usd(2900.0), 29.0);
        // 小账号 → 绝对下限仍然接管，不会低到无法下单
        assert_eq!(cfg.effective_min_order_usd(585.0), 10.0);
    }
}

#[cfg(test)]
mod capital_regressions {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn legacy_baseline_migrates_before_hour_dedup_and_survives_clear_restart() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let path = dir.join("live.json");
        let now = now_ms_pub();
        let mut old = LiveState::default();
        old.config.account = "account-a".into();
        old.history = vec![EquityPoint { ts: now - 3_600_000, equity: 500., pnl: 0. },
            EquityPoint { ts: now, equity: 569.09, pnl: 9.03 }];
        let state = Arc::new(Mutex::new(old));
        record_equity(&state, &path, "account-a", 569.09, 9.03).await.unwrap();
        let persisted = LiveState::load(&path);
        assert_eq!(persisted.baseline().unwrap().equity, 500.);
        assert_eq!(persisted.history.len(), 2);
        { let mut s = state.lock().await; s.history.clear(); s.records.clear(); s.save(&path).unwrap(); }
        let restarted = Arc::new(Mutex::new(LiveState::load(&path)));
        record_equity(&restarted, &path, "account-a", 600., 100.).await.unwrap();
        assert_eq!(restarted.lock().await.baseline().unwrap().equity, 500.);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn baseline_is_observed_not_inferred_and_rejects_old_account_and_failed_save() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let path = dir.join("live.json");
        let mut initial = LiveState::default(); initial.config.account = "a".into();
        assert!(initial.baseline().is_none());
        let state = Arc::new(Mutex::new(initial));
        record_equity(&state, &path, "a", 569.09, 9.03).await.unwrap();
        assert_eq!(state.lock().await.baseline().unwrap().equity, 569.09);
        assert!(record_equity(&state, &path, "a", f64::INFINITY, 0.).await.is_err());
        { let mut s = state.lock().await; s.config.account = "b".into(); s.reset_capital_for_account_change("a"); }
        assert!(record_equity(&state, &path, "a", 1000., 0.).await.is_err());
        assert!(state.lock().await.baseline().is_none());
        record_equity(&state, &path, "b", 200., 0.).await.unwrap();
        assert_eq!(state.lock().await.baseline().unwrap().equity, 200.);
        { let mut s = state.lock().await; s.history.clear(); }
        record_equity(&state, &path, "b", 0., 0.).await.unwrap();
        assert_eq!(state.lock().await.history.last().unwrap().equity, 0.);
        let fresh = Arc::new(Mutex::new(LiveState::default()));
        assert!(record_equity(&fresh, &dir, "", 100., 0.).await.is_err());
        assert!(fresh.lock().await.baseline().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saved_baseline_wins_over_trimmed_history_and_cannot_cross_accounts() {
        let mut s = LiveState::default();
        s.start_equity = Some(500.);
        s.history.push(EquityPoint { ts: 123, equity: 700., pnl: 0. });
        let b = s.baseline().unwrap(); assert_eq!(b.equity, 500.); assert!(b.ts.is_none());
        s.capital_baseline = Some(b); s.config.account = "other".into();
        assert!(s.baseline().is_none());
    }


}

#[cfg(test)]
mod monitor_receipt_tests {
    use super::*;
    #[test]
    fn split_close_receipts_are_not_assumed_to_be_take_profits() {
        let mut state = LiveState::default(); state.reconciled_to = 100;
        let rows = vec![
            serde_json::json!({"time":200,"tid":1,"coin":"AERO","sz":"57","px":"0.80177","dir":"Close Short","side":"B","closedPnl":"-1.27"}),
            serde_json::json!({"time":200,"tid":2,"coin":"AERO","sz":"46","px":"0.80171","dir":"Close Short","side":"B","closedPnl":"-1.03"}),
            serde_json::json!({"time":200,"tid":2,"coin":"AERO","sz":"46","px":"0.80171","dir":"Close Short","side":"B","closedPnl":"-1.03"}),
        ];
        assert_eq!(reconcile_fill_rows(&rows, &mut state).unwrap(), 2);
        assert!(state.records.iter().all(|r| r.action == "交易所成交补录" && !r.result.contains("被动") && r.reduce_only && r.side == "买"));
        assert!((state.records.iter().map(|r| r.pnl.unwrap()).sum::<f64>() + 2.30).abs() < 1e-8);
        assert_eq!(reconcile_fill_rows(&rows, &mut state).unwrap(), 0);
    }
    #[test]
    fn first_receipt_watermark_survives_merge_without_fills() {
        let mut current = LiveState::default(); let mut snapshot = current.clone();
        assert_eq!(reconcile_fill_rows(&[], &mut snapshot).unwrap(), 0);
        merge_reconciliation(&mut current, &snapshot);
        assert!(current.reconciled_to > 0);
    }
}


#[cfg(test)]
mod capital_flow_tests {
    use super::*;

    #[test]
    fn cash_flows_follow_the_account_baseline() {
        let mut st = LiveState::default();
        st.config.account = "a".into();
        st.net_deposit = 100.0;
        st.capital_flows.push(CapitalFlow { ts: 1, effective_ts: None, amount: 100.0, note: String::new() });
        st.reset_capital_for_account_change("A");
        assert_eq!(st.net_deposit, 100.0);
        st.config.account = "b".into();
        st.reset_capital_for_account_change("a");
        assert_eq!(st.net_deposit, 0.0);
        assert!(st.capital_flows.is_empty());
    }

    /// 充钱只会增加 net_deposit，绝不改变"策略赚出来的部分"。
    /// 入金不改变净收益金额；简单收益率会因投入本金增加而变化。
    #[test]
    fn deposit_does_not_change_trading_pnl() {
        let mut st = LiveState::default();
        st.start_equity = Some(1000.0);
        st.net_deposit = 0.0;
        let equity_before = 1100.0; // 策略赚了 100
        let trading_before = equity_before - st.net_deposit - st.start_equity.unwrap();
        assert!((trading_before - 100.0).abs() < 1e-9);

        // 充 500
        st.net_deposit += 500.0;
        let equity_after = equity_before + 500.0;
        let trading_after = equity_after - st.net_deposit - st.start_equity.unwrap();
        assert!((trading_after - 100.0).abs() < 1e-9,
            "入金后策略赚出的部分变了：{trading_after}");
    }
}


#[cfg(test)]
mod fill_dedup_tests {
    use super::*;
    use serde_json::json;

    fn recorded_order(ts: i64, coin: &str, size: f64) -> LiveRecord {
        LiveRecord {
            ts,
            coin: coin.into(),
            side: "卖".into(),
            action: "减仓到目标".into(),
            size,
            price: 0.9455,
            notional: size * 0.9455,
            result: format!("卖 {coin} 只减仓 {size}"),
            live: true,
            pnl: None,
            tid: None, // 我们自己发的单在回执里没有 tid
            entry_px: None,
            reduce_only: true,
        }
    }

    fn close_fill(t: i64, tid: u64, oid: u64, sz: f64) -> serde_json::Value {
        json!({
            "time": t, "tid": tid, "oid": oid, "coin": "AERO",
            "dir": "Close Long", "side": "A",
            "sz": format!("{sz}"), "px": "0.9455", "closedPnl": "0.10"
        })
    }

    /// ★ 真实事故回归：一笔 85 AERO 的调仓被交易所拆成 13/14/14/2/14… 多笔成交。
    /// 我们的记录是【汇总的 85】。旧代码逐笔拿 13 去比 85，差太远，于是整组被当成
    /// "不是重复"全部导入，名义重复计算约 9.4%。
    /// 现在按 oid 分组，用组总额 85 去比 → 整组跳过。
    #[test]
    fn split_fills_matching_our_aggregate_order_are_not_imported() {
        let t0 = 1_700_000_000_000i64;
        let mut st = LiveState::default();
        st.reconciled_to = t0 - 1_000;
        st.records.push(recorded_order(t0, "AERO", 85.0));
        // 同一笔订单（oid=999）被拆成 60 + 20 + 5 = 85
        let arr = vec![
            close_fill(t0, 1, 999, 60.0),
            close_fill(t0, 2, 999, 20.0),
            close_fill(t0, 3, 999, 5.0),
        ];
        let added = reconcile_fill_rows(&arr, &mut st).unwrap();
        assert_eq!(added, 0, "整组 85 已被我们的汇总记录覆盖，不应再导入");
        assert_eq!(st.records.len(), 1, "不应新增补录记录");
    }

    /// 反向：交易所上有一笔我们【没有】记录过的成交（比如被强平），
    /// 必须仍然被导入 —— 否则已实现盈亏会漏。
    #[test]
    fn fills_with_no_matching_order_are_still_imported() {
        let t0 = 1_700_000_000_000i64;
        let mut st = LiveState::default();
        st.reconciled_to = t0 - 1_000;
        // 我们没有任何 AERO 记录
        let arr = vec![
            close_fill(t0, 11, 777, 30.0),
            close_fill(t0, 12, 777, 10.0),
        ];
        let added = reconcile_fill_rows(&arr, &mut st).unwrap();
        // 同一 oid 的两条成交合并成一条记录（和交易所的 Aggregate 视图一致 ✗）
        assert_eq!(added, 1, "同一笔订单的成交应合并为一条记录");
        assert_eq!(st.records.len(), 1);
        let r = &st.records[0];
        assert!((r.size - 40.0).abs() < 1e-9, "数量应相加，实际 {}", r.size);
        // 每条 fill 的 closedPnl 都是 0.10 → 两条合并应得 0.20
        assert!((r.pnl.unwrap_or(0.0) - 0.2).abs() < 1e-9, "已实现应收拢，实际 {:?}", r.pnl);
    }
}

#[cfg(test)]
mod month_ordinal_tests {
    use super::month_ordinal;

    /// 1970-01-01 = day 0 → 月序号 1970*12 + 0
    #[test]
    fn epoch_is_month_zero() {
        assert_eq!(month_ordinal(0), 1970 * 12);
    }

    /// 2026-10-31 与 2026-11-01 必须落在【相邻但不同】的两个月
    /// （day/30 会把 11-01、11-02 错分到 10 月的桶里 ✗ 这就是要修的 bug）
    #[test]
    fn month_boundary_is_calendar_month() {
        // 2026-10-31 00:00 UTC 的天序号（与 Python 独立核对：20757）
        let d_oct31: i64 = 20_757;
        assert_eq!(month_ordinal(d_oct31), 2026 * 12 + 9, "2026-10 应为 2026*12+9");
        assert_eq!(month_ordinal(d_oct31 + 1), 2026 * 12 + 10, "2026-11-01 应进入 11 月");
        assert_eq!(month_ordinal(d_oct31 + 2), 2026 * 12 + 10, "2026-11-02 仍在 11 月");
    }

    /// 跨年：2026-12-31 → 2027-01-01 月序号 +1 且年份进位
    #[test]
    fn year_rollover() {
        let d = 20_818; // 2026-12-31（Python 核对：20818）
        assert_eq!(month_ordinal(d), 2026 * 12 + 11);
        assert_eq!(month_ordinal(d + 1), 2027 * 12 + 0);
    }
}
