use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    /// (coin, spot, funding_hourly, spot_vol, px)
    fn market(rows: &[(&str, &str, f64, f64, f64)]) -> Market {
        let mut m = Market::default();
        for (coin, spot, fh, vol, px) in rows {
            // 测试默认给 100bp 升水，聚焦原有资金费逻辑；基差测试再覆盖价格。
            let perp = *px * 1.01;
            m.perp_mid.insert(coin.to_string(), perp);
            m.spot_mid.insert(spot.to_string(), *px);
            m.funding.insert(coin.to_string(), *fh);
            m.spot_volume.insert(spot.to_string(), *vol);
            m.latched.push(Latch {
                coin: coin.to_string(),
                spot: spot.to_string(),
                funding_ann: fh * 24.0 * 365.0 * 100.0,
                spot_volume: *vol,
            });
        }
        m
    }

    #[test]
    fn entry_basis_filter_rejects_discount_even_with_high_funding() {
        let p = Params::default();
        let mut m = market(&[
            ("AAA", "@1", 0.001_0, 5e6, 10.0),
            ("BBB", "@2", 0.000_2, 5e6, 10.0),
            ("CCC", "@3", 0.000_1, 5e6, 10.0),
        ]);
        m.perp_mid.insert("AAA".into(), 9.99);  // -10bp：资金费最高也拒绝
        m.perp_mid.insert("BBB".into(), 10.05); // +50bp：允许
        m.perp_mid.insert("CCC".into(), 10.0);  // 0bp：低于默认20bp
        let selected = select_candidates(&m, &p);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].coin, "BBB");
        assert!((selected[0].basis_bp - 50.0).abs() < 1e-8);

        let mut st = HlCarryState::default();
        st.open(1000.0, &m, 1);
        assert_eq!(st.legs.len(), 1);
        assert_eq!(st.legs[0].coin, "BBB");
        let legacy: Params = serde_json::from_str(r#"{
            "top_k":5,"min_funding_ann_pct":3.0,"min_spot_volume_usd":1000000.0,
            "max_leg_weight":0.4,"payback_days":90.0,"min_hold_hours":48.0,
            "basis_take_profit_bp":50.0
        }"#).unwrap();
        assert_eq!(legacy.min_entry_basis_bp, 20.0, "旧状态缺字段时应使用新默认值");
    }

    #[test]
    fn selects_by_funding_and_filters() {
        let p = Params::default();
        let m = market(&[
            ("AAA", "@1", 0.000_05, 5e6, 10.0),   // 43.8%/年
            ("BBB", "@2", 0.000_0125, 5e6, 10.0), // 10.95%/年
            ("CCC", "@3", 0.000_0001, 5e6, 10.0), // 0.09%/年 -> 低于门槛
            ("DDD", "@4", 0.000_05, 1e4, 10.0),   // 现货太薄
        ]);
        let c = select_candidates(&m, &p);
        let names: Vec<&str> = c.iter().map(|x| x.coin.as_str()).collect();
        assert_eq!(names, vec!["AAA", "BBB"], "只留过门槛且够厚的，按资金费降序");

        let t = target_weights(&c, &p);
        assert_eq!(t.len(), 2);
        // AAA 原始权重 0.837 被 0.40 上限压住；余量留现金，不回补给 BBB
        assert!((t[0].weight - 0.40).abs() < 1e-9, "高资金费腿应被压到上限");
        assert!(t[1].weight > 0.0 && t[1].weight < 0.40);
        let sum: f64 = t.iter().map(|x| x.weight).sum();
        assert!(sum <= 1.0 + 1e-9, "权重和不得超过 1（余额是现金）");
        assert!(sum < 1.0 - 1e-6, "上限生效时应留出现金");
    }

    #[test]
    fn weights_respect_cap() {
        let p = Params {
            top_k: 3,
            max_leg_weight: 0.5,
            ..Params::default()
        };
        let m = market(&[
            ("AAA", "@1", 0.000_2, 5e6, 10.0),
            ("BBB", "@2", 0.000_01, 5e6, 10.0),
        ]);
        let t = target_weights(&select_candidates(&m, &p), &p);
        for x in &t {
            assert!(x.weight <= 0.5 + 1e-9, "单腿不得超过上限");
        }
        let sum: f64 = t.iter().map(|x| x.weight).sum();
        assert!(sum <= 1.0 + 1e-9, "权重和不得超过 1");
        assert!(sum > 0.0);
    }

    #[test]
    fn rebalance_gate_blocks_small_improvements() {
        let p = Params::default();
        let mut st = HlCarryState {
            params: p.clone(),
            ..HlCarryState::default()
        };
        let m1 = market(&[
            ("AAA", "@1", 0.000_0125, 5e6, 10.0),
            ("BBB", "@2", 0.000_0120, 5e6, 10.0),
        ]);
        st.open(1000.0, &m1, 1);
        assert_eq!(st.legs.len(), 2);
        let m2 = market(&[
            ("AAA", "@1", 0.000_0128, 5e6, 10.0),
            ("BBB", "@2", 0.000_0122, 5e6, 10.0),
        ]);
        assert!(!st.maybe_rebalance(&m2, 2), "改进低于门槛时不应换仓");
    }

    #[test]
    fn rebalance_switches_when_clearly_better() {
        let p = Params::default();
        let mut st = HlCarryState {
            params: p.clone(),
            ..HlCarryState::default()
        };
        let m1 = market(&[
            ("AAA", "@1", 0.000_0125, 5e6, 10.0),
            ("BBB", "@2", 0.000_0125, 5e6, 10.0),
        ]);
        st.open(1000.0, &m1, 1);
        let fee_before = st.fees_paid_total;
        let m2 = market(&[
            ("AAA", "@1", 0.000_0125, 5e6, 10.0),
            ("BBB", "@2", 0.000_0125, 5e6, 10.0),
            ("ZZZ", "@9", 0.000_4, 5e6, 10.0),
        ]);
        assert!(st.maybe_rebalance(&m2, 2), "改进远超门槛时应换仓");
        assert!(
            st.legs.iter().any(|l| l.coin == "ZZZ"),
            "应纳入高资金费的新币"
        );
        assert!(st.fees_paid_total > fee_before, "换仓必须记手续费");
    }

    #[test]
    fn turnover_and_threshold_math() {
        let p = Params {
            payback_days: 30.0,
            ..Params::default()
        };
        let th = rebalance_threshold_ann_pct(1.0, &p);
        assert!((th - 10.88 / 100.0 * 365.0 / 30.0).abs() < 1e-9);
        assert_eq!(rebalance_threshold_ann_pct(0.0, &p), 0.0);
    }

    #[test]
    fn open_then_accrue_funding() {
        let mut st = HlCarryState {
            params: Params {
                max_leg_weight: 1.0,
                ..Params::default()
            },
            ..HlCarryState::default()
        };
        let mut m = market(&[("BTC", "@142", 0.000_0125, 5e6, 80_000.0)]);
        st.open(1000.0, &m, 1_000_000);
        assert_eq!(st.legs.len(), 1);
        let fee = st.legs[0].fee_paid;
        assert!((fee - 1000.0 * ROUND_TRIP_FEE_BP / 10_000.0).abs() < 1e-9);
        let end = 1_000_000 + 10 * 3_600_000;
        m.funding_history.insert("BTC".into(), vec![
            (1_000_000 + 3_600_000, 0.000_01),
            (end, -0.000_02),
        ]);
        st.step(&m, end);
        st.step(&m, end); // 同一结算事件不得重复计提。
        let expect = 1000.0 * (0.000_01 - 0.000_02);
        assert!((st.legs[0].funding_accrued - expect).abs() < 1e-9);
        assert!(st.legs[0].basis_pnl.abs() < 1e-6);
    }

    #[test]
    fn basis_pnl_offsets_price_move() {
        let mut st = HlCarryState::default();
        let m0 = market(&[("BTC", "@142", 0.000_0125, 5e6, 80_000.0)]);
        st.open(1000.0, &m0, 1_000_000);
        assert_eq!(st.legs.len(), 1, "正资金费标的应能建仓");
        let m1 = market(&[("BTC", "@142", 0.000_0125, 5e6, 88_000.0)]);
        st.step(&m1, 1_000_000 + 3_600_000);
        assert!(st.legs[0].basis_pnl.abs() < 1e-6, "delta 中性后基差盈亏应≈0");
    }

    #[test]
    fn ymd_formats_utc() {
        assert_eq!(ymd_from_ms(0), "1970-01-01");
        // 2026-10-27T00:00:00Z
        assert_eq!(ymd_from_ms(1_793_059_200_000), "2026-10-27");
        assert_eq!(ymd_from_ms(1_793_059_200_000 + 86_400_000 - 1), "2026-10-27");
        assert_eq!(ymd_from_ms(1_793_059_200_000 + 86_400_000), "2026-10-28");
    }

    #[test]
    fn daily_aggregates_across_days() {
        let mut st = HlCarryState {
            params: Params {
                max_leg_weight: 1.0,
                ..Params::default()
            },
            ..HlCarryState::default()
        };
        let mut m = market(&[("BTC", "@142", 0.000_0125, 5e6, 80_000.0)]);
        let t0 = 1_793_059_200_000i64; // 2026-10-27T00:00Z
        m.funding_history.insert("BTC".into(), vec![
            (t0 + 10 * 3_600_000, 0.000_01),
            (t0 + 30 * 3_600_000, 0.000_02),
        ]);
        st.open(1000.0, &m, t0);
        let fee0 = st.fees_paid_total;
        assert!(fee0 > 0.0);

        // 同一天内计提 10 小时
        st.step(&m, t0 + 10 * 3_600_000);
        let (d0_date, d0_fees, d0_funding) = {
            let d0 = st.daily.last().unwrap();
            (d0.date.clone(), d0.fees, d0.funding)
        };
        assert_eq!(d0_date, "2026-10-27");
        assert!((d0_fees - fee0).abs() < 1e-9, "手续费应计入当日");
        assert!(d0_funding > 0.0, "资金费应计入当日");

        // 跨到第二天
        st.step(&m, t0 + 30 * 3_600_000);
        assert_eq!(st.daily.len(), 2, "跨日应新开一条");
        let d1 = st.daily.last().unwrap();
        assert_eq!(d1.date, "2026-10-28");
        // 新一天的增量从零开始
        assert!(d1.funding < d0_funding, "次日资金费只算次日增量");
    }

    #[test]
    fn equity_is_continuous_across_rebalance() {
        let mut st = HlCarryState::default();
        let m1 = market(&[
            ("AAA", "@1", 0.000_2, 5e6, 100.0),
            ("BBB", "@2", 0.000_2, 5e6, 100.0),
        ]);
        st.open(1000.0, &m1, 1);
        // 价格变动制造基差盈亏
        let m2 = market(&[
            ("AAA", "@1", 0.000_2, 5e6, 110.0),
            ("BBB", "@2", 0.000_2, 5e6, 110.0),
        ]);
        st.step(&m2, 2);
        let eq_before = st.equity();
        // 出现强得多的币，触发换仓（会 resize 旧腿）
        let m3 = market(&[
            ("AAA", "@1", 0.000_2, 5e6, 110.0),
            ("BBB", "@2", 0.000_2, 5e6, 110.0),
            ("ZZZ", "@9", 0.002, 5e6, 100.0),
        ]);
        let moved = st.maybe_rebalance(&m3, 3);
        assert!(moved);
        // 换仓只应扣手续费，不应让权益凭空跳变
        let eq_after = st.equity();
        assert!(
            (eq_after - eq_before).abs() < eq_before * 0.02,
            "换仓后权益跳变过大：{eq_before} -> {eq_after}"
        );
    }

    #[test]
    fn rebalance_record_explains_the_move() {
        let mut st = HlCarryState::default();
        let m1 = market(&[
            ("AAA", "@1", 0.000_0125, 5e6, 100.0),
            ("BBB", "@2", 0.000_0125, 5e6, 100.0),
        ]);
        st.open(1000.0, &m1, 1);
        let m2 = market(&[
            ("AAA", "@1", 0.000_0125, 5e6, 100.0),
            ("BBB", "@2", 0.000_0125, 5e6, 100.0),
            ("ZZZ", "@9", 0.000_4, 5e6, 100.0),
        ]);
        assert!(st.maybe_rebalance(&m2, 2));
        let r = st.rebalances.last().expect("应记录一次换仓");
        assert!(r.turnover > 0.0);
        assert!(r.funding_after > r.funding_before, "换仓后资金费必须更高");
        assert!(r.fee_usd > 0.0, "换仓必须记手续费");
        assert!(r.added.contains(&"ZZZ".to_string()), "新币应出现在 added");
        assert!(
            r.target.iter().any(|(c, _, _)| c == "ZZZ"),
            "目标篮子应包含新币"
        );
        assert_eq!(st.daily.last().unwrap().rebalances, 1, "当日换仓次数应 +1");
    }

    #[test]
    fn removed_leg_pnl_is_carried_into_closed_pnl() {
        let mut st = HlCarryState {
            params: Params {
                top_k: 1,
                max_leg_weight: 1.0,
                ..Params::default()
            },
            ..HlCarryState::default()
        };
        let t0 = 1_793_059_200_000i64;
        let m1 = market(&[("AAA", "@1", 0.000_0125, 5e6, 100.0)]);
        st.open(1000.0, &m1, t0);
        st.step(&m1, t0 + 3_600_000);
        let equity_before = st.equity();
        let fees_before = st.fees_paid_total;

        let m2 = market(&[
            ("AAA", "@1", 0.000_0125, 5e6, 100.0),
            ("ZZZ", "@9", 0.000_4, 5e6, 100.0),
        ]);
        assert!(st.maybe_rebalance(&m2, t0 + 3_600_000));
        let fee_delta = st.fees_paid_total - fees_before;
        assert_eq!(st.legs.len(), 1);
        assert_eq!(st.legs[0].coin, "ZZZ");
        assert!(st.closed_pnl < 0.0, "旧腿的净损益（含已付手续费）要结转");
        assert!(
            (st.equity() - (equity_before - fee_delta)).abs() < 1e-7,
            "换仓权益应只减少新产生的手续费：{} -> {}，新费 {}",
            equity_before,
            st.equity(),
            fee_delta
        );
    }

    #[test]
    fn save_is_atomic_and_load_roundtrips() {
        let root = std::env::temp_dir().join(format!(
            "stars-hlcarry-{}-{}",
            std::process::id(),
            crate::live::now_ms_pub()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("state.json");
        let blocked = root.join("blocked.json");
        let state = HlCarryState {
            budget_usd: 1234.5,
            start_equity: 1234.5,
            ..HlCarryState::default()
        };

        state.save(&path).unwrap();
        let loaded = HlCarryState::load(&path);
        assert_eq!(loaded.budget_usd, 1234.5);
        assert_eq!(loaded.start_equity, 1234.5);

        std::fs::create_dir(&blocked).unwrap();
        assert!(state.save(&blocked).is_err(), "写入失败必须返回错误");
        assert!(blocked.is_dir(), "原目标应保持完整，不能被部分写入覆盖");
        assert!(!root.join(".blocked.json.tmp").exists(), "失败的临时文件应清理");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_real_api_payload() {
        let mids = serde_json::json!({
            "BTC": "79948.5", "ETH": "2313.45",
            "@142": "79871.5", "@151": "2311.25"
        });
        let ctxs = serde_json::json!([
            {"universe": [{"name": "BTC"}, {"name": "ETH"}]},
            [{"funding": "0.0000125"}, {"funding": "0.0000125"}]
        ]);
        let spot = serde_json::json!([
            {"tokens": [{"name": "USDC"}, {"name": "UBTC"}, {"name": "UETH"}],
             "universe": [
                {"name": "@142", "tokens": [1, 0]},
                {"name": "@151", "tokens": [2, 0]}
             ]},
            [{"coin": "@142", "dayNtlVlm": "19357783"},
             {"coin": "@151", "dayNtlVlm": "9321402"}]
        ]);
        let m = parse_market(&mids, &ctxs, &spot);
        assert_eq!(m.perp_mid.get("BTC"), Some(&79948.5));
        assert_eq!(m.spot_mid.get("@142"), Some(&79871.5));
        assert_eq!(m.spot_volume.get("@142"), Some(&19357783.0));
        let latched: Vec<&str> = m.latched.iter().map(|l| l.coin.as_str()).collect();
        assert!(latched.contains(&"BTC"), "UBTC 应映射到永续 BTC");
        assert!(latched.contains(&"ETH"), "UETH 应映射到永续 ETH");
        let q = m.quote("BTC", "@142").unwrap();
        assert!((q.funding_ann - 10.95).abs() < 0.01);
    }

    #[test]
    fn xmr2_is_not_xmr_and_candidates_are_unique() {
        let perps = std::collections::HashSet::from(["XMR".to_string()]);
        assert_eq!(perp_of_spot_token("XMR1", &perps).as_deref(), Some("XMR"));
        assert_eq!(perp_of_spot_token("XMR2", &perps), None);
        assert_eq!(perp_of_spot_token("XMRN", &perps), None);
        let m = market(&[
            ("XMR", "@260", 0.000_1, 8e6, 500.0),
            ("XMR", "@260", 0.000_1, 8e6, 500.0),
        ]);
        let selected = select_candidates(&m, &Params::default());
        assert_eq!(selected.len(), 1);
    }

    #[test]
    fn daily_components_reconcile_with_total_equity() {
        let mut st = HlCarryState { params: Params { max_leg_weight: 1.0, ..Params::default() },
            ..HlCarryState::default() };
        let mut m = market(&[("BTC", "@142", 0.000_0125, 5e6, 80_000.0)]);
        let t0 = 1_793_059_200_000i64;
        m.funding_history.insert("BTC".into(), vec![(t0 + 3_600_000, 0.000_01)]);
        st.open(1000.0, &m, t0);
        st.step(&m, t0 + 3_600_000);
        let funding = st.daily.iter().map(|d| d.funding).sum::<f64>();
        let basis = st.daily.iter().map(|d| d.basis).sum::<f64>();
        let fees = st.daily.iter().map(|d| d.fees).sum::<f64>();
        assert!((funding - st.funding_total).abs() < 1e-8);
        assert!((fees - st.fees_paid_total).abs() < 1e-8);
        assert!((basis - st.basis_total()).abs() < 1e-8);
        assert!((st.equity() - st.start_equity - funding - basis + fees).abs() < 1e-8);
    }

    #[test]
    fn basis_take_profit_closes_only_triggered_leg_during_lock() {
        let mut st = HlCarryState {
            params: Params { min_entry_basis_bp: 0.0, ..Params::default() },
            ..HlCarryState::default()
        };
        let mut m0 = market(&[
            ("AAA", "@1", 0.000_2, 5e6, 100.0),
            ("BBB", "@2", 0.000_2, 5e6, 100.0),
        ]);
        m0.perp_mid.insert("AAA".into(), 100.0);
        m0.perp_mid.insert("BBB".into(), 100.0);
        st.open(1000.0, &m0, 1_000_000);
        let mut m1 = market(&[
            ("AAA", "@1", 0.000_2, 5e6, 100.0),
            ("BBB", "@2", 0.000_2, 5e6, 100.0),
        ]);
        m1.perp_mid.insert("AAA".into(), 99.0);
        st.step(&m1, 1_000_000 + 3_600_000);
        assert!((st.legs[0].basis_move_bp + 100.0).abs() < 1e-8);
        let equity_before = st.equity();
        let fees_before = st.fees_paid_total;

        assert!(st.maybe_rebalance(&m1, 1_000_000 + 3_600_000));
        assert_eq!(st.legs.len(), 1);
        assert_eq!(st.legs[0].coin, "BBB");
        assert!((st.equity() - equity_before).abs() < 1e-8, "止盈本身不得改变权益");
        assert_eq!(st.fees_paid_total, fees_before, "往返费已预扣，不得重复收费");
        let record = st.rebalances.last().unwrap();
        assert_eq!(record.reason, "basis_take_profit");
        assert_eq!(record.removed, vec!["AAA"]);
        assert_eq!(record.basis_moves.len(), 1);
    }

    #[test]
    fn basis_move_is_relative_to_entry_ratio() {
        let mut st = HlCarryState {
            params: Params { top_k: 1, max_leg_weight: 1.0, ..Params::default() },
            ..HlCarryState::default()
        };
        let mut m0 = market(&[("AAA", "@1", 0.000_2, 5e6, 100.0)]);
        m0.perp_mid.insert("AAA".into(), 102.0);
        st.open(1000.0, &m0, 1_000_000);
        let mut m1 = market(&[("AAA", "@1", 0.000_2, 5e6, 100.0)]);
        m1.perp_mid.insert("AAA".into(), 100.0);
        st.step(&m1, 1_000_000 + 3_600_000);
        let expected = ((100.0 / 100.0) / (102.0 / 100.0) - 1.0) * 10_000.0;
        assert!((st.legs[0].basis_move_bp - expected).abs() < 1e-8);
        assert!(st.legs[0].basis_bp.abs() < 1e-8, "当前绝对基差与相对入场变化不是一回事");
    }

    #[test]
    fn reopen_after_full_take_profit_preserves_ledger() {
        let mut st = HlCarryState {
            params: Params { top_k: 1, max_leg_weight: 1.0, ..Params::default() },
            ..HlCarryState::default()
        };
        let m0 = market(&[("AAA", "@1", 0.000_2, 5e6, 100.0)]);
        st.open(1000.0, &m0, 1_000_000);
        st.step(&m0, 1_000_000 + 3_600_000);
        let mut m1 = m0.clone();
        m1.perp_mid.insert("AAA".into(), 99.0);
        st.step(&m1, 1_000_000 + 2 * 3_600_000);
        assert!(st.maybe_rebalance(&m1, 1_000_000 + 2 * 3_600_000));
        assert!(st.legs.is_empty());
        let closed = st.closed_pnl;
        let history_len = st.history.len();
        let fees_before = st.fees_paid_total;

        assert!(st.maybe_rebalance(&m0, 1_000_000 + 3 * 3_600_000));
        assert_eq!(st.legs.len(), 1);
        assert_eq!(st.start_equity, 1000.0);
        assert_eq!(st.closed_pnl, closed);
        assert_eq!(st.history.len(), history_len);
        assert!(st.fees_paid_total > fees_before);
    }

    #[test]
    fn basis_take_profit_refills_and_does_not_reset_min_hold_lock() {
        let mut st = HlCarryState {
            params: Params {
                top_k: 1,
                max_leg_weight: 1.0,
                min_hold_hours: 48.0,
                payback_days: 90.0,
                ..Params::default()
            },
            ..HlCarryState::default()
        };
        // 两个合规候选：AAA 资金费略高，开仓选中 AAA；BBB 作为备选
        let m0 = market(&[
            ("AAA", "@1", 0.000_20, 5e6, 100.0),
            ("BBB", "@2", 0.000_15, 5e6, 100.0),
        ]);
        st.open(1000.0, &m0, 1_000_000);
        assert_eq!(st.legs.len(), 1);
        assert_eq!(st.legs[0].coin, "AAA");

        // 1 小时后 AAA 基差收敛 100bp 触发止盈：应立即补仓 BBB，且不买回 AAA
        let mut m1 = m0.clone();
        m1.perp_mid.insert("AAA".into(), 99.0);
        st.step(&m1, 1_000_000 + 3_600_000);
        assert!(st.maybe_rebalance(&m1, 1_000_000 + 3_600_000));
        assert_eq!(st.legs.len(), 1);
        assert_eq!(st.legs[0].coin, "BBB");
        assert!(st.tp_cooldown_coins.contains(&"AAA".to_string()));
        assert_eq!(st.rebalances.last().unwrap().reason, "basis_take_profit");
        assert_eq!(st.rebalances.last().unwrap().added, vec!["BBB".to_string()]);

        // 再过 1 小时（远小于 48h），若出现高资金费 CCC，不应被刚才的基差止盈锁住 48h
        let m2 = market(&[
            ("AAA", "@1", 0.000_20, 5e6, 100.0),
            ("BBB", "@2", 0.000_05, 5e6, 100.0),
            ("CCC", "@3", 0.001_00, 5e6, 100.0),
        ]);
        assert!(st.maybe_rebalance(&m2, 1_000_000 + 2 * 3_600_000));
        assert_eq!(st.legs[0].coin, "CCC");
        assert!(st.tp_cooldown_coins.is_empty());
    }
}
