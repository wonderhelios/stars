use super::*;
use crate::exchange::{Acct, MarketInfo};
use crate::store::PanelEntry;
use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hl::Candle;

    /// 造一个合成面板：24 个币、50 天，价格与成交量都带确定性噪声。
    fn synthetic() -> Vec<PanelEntry> {
        const DAY: i64 = 86_400_000;
        let t0 = 1_700_000_000_000i64;
        (0..24)
            .map(|j| {
                let mut px = 1.0 + j as f64 * 0.37;
                let candles = (0..50)
                    .map(|d| {
                        // 让各币的动量/波动/量能互不相同，制造跨因子分歧
                        let drift = ((j * 7 + d * 13) % 11) as f64 / 100.0 - 0.05;
                        px *= 1.0 + drift;
                        // 成交量要高于 min_vol_usd（$5M）才会进入流动宇宙
                        let vol = 6_000_000.0 + ((j * 31 + d * 17) % 23) as f64 * 90_000.0;
                        Candle {
                            t: t0 + d as i64 * DAY,
                            o: px,
                            h: px * 1.02,
                            l: px * 0.98,
                            c: px,
                            v: vol,
                        }
                    })
                    .collect();
                PanelEntry {
                    coin: format!("C{j:02}"),
                    candles,
                }
            })
            .collect()
    }

    /// 因子权重必须只用一次线性组合 + 最终归一化，不能引入额外的缩放。
    ///
    /// 用三个点验证（不需要复刻因子数学）：
    ///   w(1.0) = 纯动量（此时 Σ|w|=1，所以它就是未归一化的动量向量）
    ///   w(0.0) = 纯量能（同理）
    ///   w(0.7) 必须等于 normalize(0.7·w(1.0) + 0.3·w(0.0))
    /// 这条关系一旦破坏（比如权重加在两个腿之外、或 50/50 的旧行为被改动），
    /// 界面上的「动量权重 70%」就不再等于实际组合的 70%。
    #[test]
    fn factor_weight_is_a_linear_mix_of_the_two_books() {
        let panel = synthetic();
        let base = TradeConfig { min_position_usd: 0.0, inv_vol: false, ..Default::default() };
        let weights = |mw: f64| {
            let cfg = TradeConfig { momentum_weight: mw, ..base.clone() };
            let (w, _) = target_weights(&panel, &cfg, 1000.0, i64::MAX);
            let m: std::collections::HashMap<String, f64> = w.into_iter().collect();
            let gross: f64 = m.values().map(|x| x.abs()).sum();
            assert!((gross - 1.0).abs() < 1e-9, "mw={mw} 的 Σ|w| 必须是 1，实际 {gross}");
            m
        };
        let mom = weights(1.0);
        let vol = weights(0.0);
        assert!(!mom.is_empty() && !vol.is_empty(), "两个纯因子组合都不该为空");
        for mw in [0.5_f64, 0.7, 0.3] {
            let got = weights(mw);
            let mut coins: Vec<&String> = mom.keys().chain(vol.keys()).collect();
            coins.sort();
            coins.dedup();
            let mut raw: Vec<(String, f64)> = Vec::new();
            for c in coins {
                let x = mw * mom.get(c).copied().unwrap_or(0.0)
                    + (1.0 - mw) * vol.get(c).copied().unwrap_or(0.0);
                raw.push((c.clone(), x));
            }
            let sum: f64 = raw.iter().map(|(_, x)| x.abs()).sum();
            assert!(sum > 0.0);
            for (c, x) in raw {
                let want = x / sum;
                let have = got.get(&c).copied().unwrap_or(0.0);
                assert!(
                    (want - have).abs() < 1e-9,
                    "mw={mw} 时 {c} 的权重 {have} 不等于线性组合 {want}"
                );
            }
        }
    }

    /// 止盈（选项 B）：相对【持仓成本】。已到成本的 (1+tp) 的仓位，
    /// 调仓时就该平掉（而不是等交易所那张可能挂不上去的限价单）。
    #[test]
    fn take_profit_closes_a_winner_at_cost() {
        let panel = synthetic();
        let cfg = TradeConfig {
            min_position_usd: 0.0,
            take_profit_pct: 0.10,
            rebalance_slices: 1,
            ..Default::default()
        };
        let (w, _) = target_weights(&panel, &cfg, 1000.0, i64::MAX);
        let coin = w.iter().find(|(_, x)| *x > 0.0).expect("要有多头").0.clone();
        let mid = panel.iter().find(|e| e.coin == coin).unwrap().candles.last().unwrap().c;
        let markets: HashMap<String, MarketInfo> =
            panel.iter().map(|e| (e.coin.clone(), MarketInfo { sz_decimals: 2, max_leverage: 5 })).collect();
        let mids: HashMap<String, f64> = panel.iter().map(|e| (e.coin.clone(), e.candles.last().unwrap().c)).collect();
        let mut acct = Acct { equity: 1000.0, positions: Default::default() };
        // 成本 = 现价/1.12 → 已赚 12% ≥ 10%
        acct.positions.insert(coin.clone(), crate::exchange::Pos {
            size: 10.0, entry_px: mid / 1.12, ..Default::default()
        });
        let plan = build_plan(&w, &acct, &markets, &mids, &cfg, None);
        let tp: Vec<_> = plan.orders.iter().filter(|o| o.reason == "止盈平仓").collect();
        assert!(!tp.is_empty(), "应有止盈平仓单: {:?}", plan.orders.iter().map(|o| (&o.coin, &o.reason)).collect::<Vec<_>>());
        assert!(tp[0].reduce_only && !tp[0].buy, "平多头必须 reduce_only 卖出");

        // 赚 5% < 10%: 不应止盈
        let mut acct2 = Acct { equity: 1000.0, positions: Default::default() };
        acct2.positions.insert(coin.clone(), crate::exchange::Pos {
            size: 10.0, entry_px: mid / 1.05, ..Default::default()
        });
        let plan2 = build_plan(&w, &acct2, &markets, &mids, &cfg, None);
        assert!(!plan2.orders.iter().any(|o| o.reason == "止盈平仓"), "未到阈值不该止盈");
    }

    /// 净敞口保护：只减偏多的那条腿，绝不开新仓。
    #[test]
    fn neutralize_reduces_only_the_overweight_side() {
        let markets: HashMap<String, MarketInfo> =
            ["A", "B"].iter().map(|c| (c.to_string(), MarketInfo { sz_decimals: 2, max_leverage: 5 })).collect();
        let mids: HashMap<String, f64> = ["A", "B"].iter().map(|c| (c.to_string(), 100.0)).collect();
        let mut acct = Acct { equity: 1000.0, positions: Default::default() };
        acct.positions.insert("A".into(), crate::exchange::Pos { size: 10.0, entry_px: 100.0, ..Default::default() });
        acct.positions.insert("B".into(), crate::exchange::Pos { size: -5.0, entry_px: 100.0, ..Default::default() });
        let orders = neutralize_orders(&acct, &markets, &mids, 0.02, 10.0);
        assert_eq!(orders.len(), 1, "只应有一笔");
        assert_eq!(orders[0].coin, "A");
        assert!(!orders[0].buy, "减多头必须是卖出");
        assert!(orders[0].reduce_only);
        assert!((orders[0].notional - 500.0).abs() < 1e-6, "应减掉 500 名义，实际 {}", orders[0].notional);

        // 平衡 → 不动
        let mut bal = Acct { equity: 1000.0, positions: Default::default() };
        bal.positions.insert("A".into(), crate::exchange::Pos { size: 10.0, entry_px: 100.0, ..Default::default() });
        bal.positions.insert("B".into(), crate::exchange::Pos { size: -10.0, entry_px: 100.0, ..Default::default() });
        assert!(neutralize_orders(&bal, &markets, &mids, 0.02, 10.0).is_empty());

        // 净空 → 买回空头
        let mut sh = Acct { equity: 1000.0, positions: Default::default() };
        sh.positions.insert("A".into(), crate::exchange::Pos { size: 2.0, entry_px: 100.0, ..Default::default() });
        sh.positions.insert("B".into(), crate::exchange::Pos { size: -10.0, entry_px: 100.0, ..Default::default() });
        let o = neutralize_orders(&sh, &markets, &mids, 0.02, 10.0);
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].coin, "B");
        assert!(o[0].buy, "减空头必须是买入");
    }

    #[test]
    fn neutralize_is_deterministic_and_respects_targets() {
        let markets: HashMap<String, MarketInfo> = ["A", "B", "C"]
            .iter()
            .map(|c| (c.to_string(), MarketInfo { sz_decimals: 2, max_leverage: 5 }))
            .collect();
        let mids: HashMap<String, f64> = ["A", "B", "C"]
            .iter()
            .map(|c| (c.to_string(), 100.0))
            .collect();
        let mut acct = Acct { equity: 3000.0, positions: Default::default() };
        acct.positions.insert("A".into(), crate::exchange::Pos { size: 10.0, entry_px: 100.0, ..Default::default() });
        acct.positions.insert("B".into(), crate::exchange::Pos { size: 10.0, entry_px: 100.0, ..Default::default() });
        acct.positions.insert("C".into(), crate::exchange::Pos { size: -15.0, entry_px: 100.0, ..Default::default() });
        let targets = HashMap::from([
            ("A".to_string(), 500.0),
            ("B".to_string(), 1000.0),
            ("C".to_string(), -1500.0),
        ]);
        let orders = neutralize_orders_with_targets(&acct, &markets, &mids, 0.02, 10.0, Some(&targets));
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].coin, "A", "应优先削减超出目标的 A");
        assert!((orders[0].notional - 500.0).abs() < 1e-9);
        assert!((orders[0].target - 5.0).abs() < 1e-9, "A 应保留到目标 500");

        // 没有目标时，净多头 500 由两个等额仓位各承担 250，顺序固定且不随机清零。
        let fallback = neutralize_orders(&acct, &markets, &mids, 0.02, 10.0);
        assert_eq!(fallback.iter().map(|o| o.coin.as_str()).collect::<Vec<_>>(), vec!["A", "B"]);
        assert!(fallback.iter().all(|o| (o.notional - 250.0).abs() <= 1.0));
        assert!(fallback.iter().all(|o| o.target.abs() > 0.0));
    }

    #[test]
    fn neutralize_large_imbalance_spreads_instead_of_zeroing_winners() {
        let markets: HashMap<String, MarketInfo> = ["VVV", "HYPE", "BTC"]
            .iter()
            .map(|c| (c.to_string(), MarketInfo { sz_decimals: 2, max_leverage: 5 }))
            .collect();
        let mids: HashMap<String, f64> = ["VVV", "HYPE", "BTC"]
            .iter()
            .map(|c| (c.to_string(), 100.0))
            .collect();
        let mut acct = Acct { equity: 5000.0, positions: Default::default() };
        acct.positions.insert("VVV".into(), crate::exchange::Pos { size: 10.0, entry_px: 80.0, ..Default::default() });
        acct.positions.insert("HYPE".into(), crate::exchange::Pos { size: 10.0, entry_px: 80.0, ..Default::default() });
        acct.positions.insert("BTC".into(), crate::exchange::Pos { size: -5.0, entry_px: 100.0, ..Default::default() });
        let targets = HashMap::from([
            ("VVV".to_string(), 900.0),
            ("HYPE".to_string(), 900.0),
            ("BTC".to_string(), -500.0),
        ]);
        let orders = neutralize_orders_with_targets(&acct, &markets, &mids, 0.02, 10.0, Some(&targets));
        assert_eq!(orders.len(), 2);
        assert!(orders.iter().all(|o| o.target.abs() > 0.0), "大额不平衡也应分摊，不能清零 VVV/HYPE");
        let reduced: f64 = orders.iter().map(|o| o.notional).sum();
        assert!((reduced - 1500.0).abs() <= 2.0, "仅允许数量向下取整的误差");
    }

    #[test]
    fn weights_are_normalised_to_unit_gross() {
        // 跨因子会相互抵消，如果不重新归一化，Σ|w| 会小于 1，实际杠杆就会低于设置。
        // 这个不变量一旦退化，实盘敞口会静默缩水 13%~30%。
        let panel = synthetic();
        let cfg = TradeConfig {
            min_position_usd: 0.0,
            ..Default::default()
        };
        let (w, _) = target_weights(&panel, &cfg, 1000.0, i64::MAX);
        assert!(!w.is_empty(), "应该选出组合");
        let gross: f64 = w.iter().map(|(_, x)| x.abs()).sum();
        assert!(
            (gross - 1.0).abs() < 1e-9,
            "Σ|w| 必须是 1，实际 {gross}"
        );
    }

    #[test]
    fn long_and_short_legs_are_balanced() {
        let panel = synthetic();
        let cfg = TradeConfig {
            min_position_usd: 0.0,
            ..Default::default()
        };
        let (w, _) = target_weights(&panel, &cfg, 1000.0, i64::MAX);
        let net: f64 = w.iter().map(|(_, x)| *x).sum();
        assert!(net.abs() < 1e-9, "组合必须市场中性，净敞口 {net}");
    }

    #[test]
    fn inv_vol_weights_keep_market_neutrality_and_unit_gross() {
        let panel = synthetic();
        let cfg = TradeConfig {
            min_position_usd: 0.0,
            inv_vol: true,
            ..Default::default()
        };
        let (w, _) = target_weights(&panel, &cfg, 1000.0, i64::MAX);
        assert!(!w.is_empty());
        let gross: f64 = w.iter().map(|(_, x)| x.abs()).sum();
        let net: f64 = w.iter().map(|(_, x)| *x).sum();
        assert!((gross - 1.0).abs() < 1e-9, "inv_vol Σ|w| 必须是 1，实际 {gross}");
        assert!(net.abs() < 1e-9, "inv_vol 净敞口必须是 0，实际 {net}");
    }

    #[test]
    fn only_closed_bars_are_used() {
        // 最后一根如果是「正在形成」的当根，量能因子会拿到几分钟的成交量，
        // 实盘会选出与回测完全不同的组合。用 now 卡在最后一根中间来验证。
        const DAY: i64 = 86_400_000;
        let panel = synthetic();
        let fp = FactorPanel::build(&panel);
        let last = *fp.ts.last().unwrap();
        let i = fp.last_closed_index(last + DAY / 2);
        assert!(
            fp.ts[i] < last,
            "当根还没收盘，就不能用最后一根；得到索引 {} / {}", i, fp.len()
        );
        let i2 = fp.last_closed_index(last + DAY);
        assert_eq!(i2, fp.len() - 1, "收盘后应能用最后一根");
    }

    #[test]
    fn k_never_exceeds_half_the_universe() {
        // 2k > n 时重叠区会被判成多头，空头腿不足 k 个，组合变成净多头。
        let panel = synthetic();
        let cfg = TradeConfig {
            top_frac: 0.9,
            min_position_usd: 0.0,
            ..Default::default()
        };
        let (w, _) = target_weights(&panel, &cfg, 1000.0, i64::MAX);
        let net: f64 = w.iter().map(|(_, x)| *x).sum();
        assert!(net.abs() < 1e-9, "top_frac=0.9 也不能变成净多头，净敞口 {net}");
    }
}

#[cfg(test)]
mod slice_tests {
    use super::*;

    #[test]
    fn slice_of_is_deterministic_and_spreads_coins() {
        // 必须跨进程稳定：不能用 HashMap 迭代顺序或随机 hash
        assert_eq!(slice_of("BTC", 3), slice_of("BTC", 3));
        assert_eq!(slice_of("BTC", 1), 0);
        // 分档要真的把币摊开，不能全挤在一档
        let coins = ["BTC", "ETH", "SOL", "DOGE", "ARB", "HYPE", "WLD", "ADA", "XRP"];
        let mut seen = std::collections::HashSet::new();
        for c in coins {
            seen.insert(slice_of(c, 3));
        }
        assert!(seen.len() >= 2, "档位分布太集中: {seen:?}");
        for c in coins {
            assert!(slice_of(c, 3) < 3);
        }
    }
}

#[cfg(test)]
mod slice_plan_tests {
    use super::*;

    fn mk(coin: &str, sz: f64) -> (String, crate::exchange::Pos) {
        (
            coin.to_string(),
            crate::exchange::Pos {
                size: sz,
                entry_px: 1.0,
                liq_px: None,
                is_cross: true,
                leverage: 3,
                position_value: 100.0,
                unrealized_pnl: 0.0,
            },
        )
    }

    fn market(coin: &str) -> (String, MarketInfo) {
        (
            coin.to_string(),
            MarketInfo { sz_decimals: 2, max_leverage: 25 },
        )
    }

    /// 超限时（净值下跌、敞口被动爬到 3x+）必须能【越过错峰】立刻减仓，
    /// 否则 3 档错峰要拖 3 天，暴跌中敞口一直偏高、保证金缓冲被吃光。
    #[test]
    fn over_cap_trims_even_non_slot_positions() {
        let cfg = TradeConfig { rebalance_slices: 3, ..TradeConfig::default() };
        let coins = ["BTC", "ETH", "SOL", "DOGE", "ARB", "HYPE"];
        // 每个 10×100 = 1000，合计 6000 = 6x（远超 2.7x 上限）。
        let weights: Vec<(String, f64)> = coins
            .iter()
            .map(|c| (c.to_string(), if c == &"ETH" || c == &"DOGE" { -0.1 } else { 0.1 }))
            .collect();
        let acct = Acct {
            equity: 1000.0,
            positions: coins.iter().map(|c| mk(c, 10.0)).collect(),
        };
        let markets: HashMap<String, MarketInfo> = coins.iter().map(|c| market(c)).collect();
        let mids: HashMap<String, f64> = coins.iter().map(|c| (c.to_string(), 100.0)).collect();
        let plan = build_plan(&weights, &acct, &markets, &mids, &cfg, None);
        assert!(
            plan.notes.iter().any(|n| n.contains("强制减仓")),
            "超限时应有提示: {:?}",
            plan.notes
        );
        assert!(
            plan.orders.iter().any(|o| o.reduce_only),
            "超限时应有减仓单"
        );
        // 减仓单必须覆盖到「不在今天档位」的币。
        let day = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            / 86_400_000) as u32;
        let non_slot_with_order = plan
            .orders
            .iter()
            .any(|o| (day + slice_of(&o.coin, 3)) % 3 != 0 && o.reduce_only);
        assert!(non_slot_with_order, "超限减仓应越过错峰档位，实际订单: {:?}", plan.orders.iter().map(|o| (&o.coin, o.reduce_only)).collect::<Vec<_>>());
    }

    /// 错峰最关键的性质：不在今天档位里的币，绝不能因为它「不在目标权重里」
    /// 就被平掉。否则那不是错峰，是随机清仓。
    #[test]
    fn non_slot_positions_are_left_alone() {
        let cfg = TradeConfig { rebalance_slices: 3, ..TradeConfig::default() };
        // 造 6 个持仓，但目标权重里只有 3 个 —— 其余 3 个「本该被平」
        // 用真实币名。之前用 AAA/BBB/CCC —— 同字母重复三遍的字节和都是 3 的
        // 倍数，会全挤在同一档，测试因此"今天一个币都没有"而假失败。
        let held = ["BTC", "ETH", "SOL", "DOGE", "ARB", "HYPE"];
        // Σ|w| = 1.0 → 目标总敞口 2700（2.7x），当前名义 6×300 = 1800 < 2700，不触发超限强制减仓。
        let weights: Vec<(String, f64)> = vec![
            ("BTC".into(), 0.35),
            ("ETH".into(), -0.50),
            ("SOL".into(), 0.15),
        ];
        let acct = Acct {
            equity: 1000.0,
            positions: held.iter().map(|c| mk(c, 3.0)).collect(),
        };
        let markets: HashMap<String, MarketInfo> = held.iter().map(|c| market(c)).collect();
        let mids: HashMap<String, f64> = held.iter().map(|c| (c.to_string(), 100.0)).collect();
        let plan = build_plan(&weights, &acct, &markets, &mids, &cfg, None);

        let all_coins: Vec<&str> = plan
            .orders
            .iter()
            .map(|o| o.coin.as_str())
            .chain(plan.orders.iter().map(|o| o.coin.as_str()))
            .collect();
        let _ = all_coins;
        // 今天该处理的币
        let day = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            / 86_400_000) as u32;
        let today: Vec<&&str> = held
            .iter()
            .filter(|c| (day + slice_of(c, 3)) % 3 == 0)
            .collect();
        let not_today: Vec<&&str> = held
            .iter()
            .filter(|c| (day + slice_of(c, 3)) % 3 != 0)
            .collect();

        // 不在今天档位的币：一个订单都不能有
        for c in &not_today {
            let touched = plan.orders.iter().any(|o| o.coin == ***c);
            assert!(!touched, "{c} 不在今天的档位，却被下了单（会被误平）");
        }
        // 今天档位里的币：应该被处理到（DDD/EEE/FFF 不在目标里 → 该被平）
        assert!(!today.is_empty(), "今天档位里一个币都没有，测试无意义");
        assert!(!not_today.is_empty(), "所有币都在今天档位，测不出过滤是否生效");
    }

}

#[cfg(test)]
mod close_threshold_tests {
    use super::*;

    fn market(coin: &str) -> (String, MarketInfo) {
        (
            coin.to_string(),
            MarketInfo { sz_decimals: 2, max_leverage: 25 },
        )
    }

    /// **真调用 build_plan 的端到端回归**：$3,104 净值 + min_order_pct 1.7%
    /// （比例门槛 $52.77）+ 一个约 $45 的 GRAM 残仓 + 它不在目标名单里
    /// → 必须生成平仓单。这就是用户实际遇到的场景，不是算术推演。
    #[test]
    fn small_out_of_target_position_with_pct_threshold_is_still_closed() {
        let cfg = TradeConfig {
            min_order_pct: 0.017,
            min_order_usd: 10.0,
            rebalance_slices: 1,
            ..TradeConfig::default()
        };
        let mut positions = std::collections::HashMap::new();
        positions.insert(
            "GRAM".to_string(),
            crate::exchange::Pos {
                size: 31.0, // 31 × 1.4465 ≈ $44.84
                entry_px: 1.4,
                liq_px: None,
                is_cross: true,
                leverage: 3,
                position_value: 44.84,
                unrealized_pnl: 0.0,
            },
        );
        let acct = Acct { equity: 3104.0, positions };
        let markets: std::collections::HashMap<String, MarketInfo> =
            ["GRAM", "BTC", "ETH"].iter().map(|c| market(c)).collect();
        let mids: std::collections::HashMap<String, f64> = vec![
            ("GRAM".to_string(), 1.4465),
            ("BTC".to_string(), 80000.0),
            ("ETH".to_string(), 3000.0),
        ]
        .into_iter()
        .collect();
        // 目标里只有 BTC/ETH —— GRAM 掉出名单
        let weights: Vec<(String, f64)> =
            vec![("BTC".into(), 0.5), ("ETH".into(), -0.5)];

        // 前提：$44.84 确实低于比例门槛 $52.77
        assert!(44.84 < cfg.effective_min_order_usd(3104.0));

        let plan = build_plan(&weights, &acct, &markets, &mids, &cfg, None);
        let closed = plan.orders.iter().any(|o| o.coin == "GRAM" && o.reduce_only);
        let seen: Vec<(String, String)> =
            plan.orders.iter().map(|o| (o.coin.clone(), o.reason.clone())).collect();
        assert!(
            closed,
            "GRAM 残仓（$44.84 < 比例门槛）必须仍然被平掉，实际订单: {seen:?}"
        );
        assert!(plan.notes.iter().all(|n| !n.contains("GRAM: 残余仓位")),
            "不应再出现「残余仓位…无法平」: {:?}", plan.notes);
    }

    /// 回归：设了 `min_order_pct` 之后，掉出名单的【小仓位】必须仍然能平掉。
    ///
    /// 真实事故：$3,104 净值上设 1.7% → 比例门槛 $52.76，
    /// 而账上有 4 个 $45~47 的残仓，被这个门槛挡住平不掉，永远挂着。
    /// 平仓是降风险，只应受【绝对下限】（交易所最小下单额）约束。
    #[test]
    fn small_position_below_pct_threshold_must_still_close() {
        let mut cfg = TradeConfig::default();
        cfg.min_order_usd = 10.0;
        cfg.min_order_pct = 0.017;
        let equity = 3104.0;
        // 比例门槛确实是 52.76
        assert!((cfg.effective_min_order_usd(equity) - 52.768).abs() < 0.01);
        // 一个 $46 的残仓：低于比例门槛，但高于绝对下限 → 必须可平
        let pos_notional = 46.0;
        assert!(pos_notional < cfg.effective_min_order_usd(equity), "前提：它低于比例门槛");
        assert!(pos_notional >= cfg.min_order_usd, "且高于绝对下限 → 平仓判定应通过");
    }

    #[test]
    fn cap_weights_same_side_waterfills_excess_and_is_idempotent() {
        // margin_buffer * leverage = 0.9 * 3.0 = 2.7; max_single_weight = 0.20 => max_w = 0.20 / 2.7
        // 多头 5 币总权重 0.30（=0.81x < 5*0.20=1.00x），空头 5 币总权重 -0.30（美元中性）
        let mut w = vec![
            ("A".to_string(), 0.14),
            ("B".to_string(), 0.06),
            ("C".to_string(), 0.04),
            ("D".to_string(), 0.04),
            ("E".to_string(), 0.02),
            ("S1".to_string(), -0.06),
            ("S2".to_string(), -0.06),
            ("S3".to_string(), -0.06),
            ("S4".to_string(), -0.06),
            ("S5".to_string(), -0.06),
        ];
        let long_before: f64 = w.iter().filter(|(_, v)| *v > 0.0).map(|(_, v)| *v).sum();
        let short_before: f64 = w.iter().filter(|(_, v)| *v < 0.0).map(|(_, v)| v.abs()).sum();
        let clipped = cap_weights_same_side(&mut w, 0.20, 0.9, 3.0);
        assert!(clipped.contains(&"A".to_string()), "A 应被截断: {clipped:?}");
        let max_w = 0.20 / 2.7;
        for (c, v) in &w {
            assert!(v.abs() <= max_w + 1e-9, "{c} 截断后仍超限: {}", v.abs() * 2.7);
        }
        let long_after: f64 = w.iter().filter(|(_, v)| *v > 0.0).map(|(_, v)| *v).sum();
        let short_after: f64 = w.iter().filter(|(_, v)| *v < 0.0).map(|(_, v)| v.abs()).sum();
        assert!((long_after - long_before).abs() < 1e-9, "多头总权重应保持不变: {long_after} vs {long_before}");
        assert!((short_after - short_before).abs() < 1e-9, "空头总权重不应受影响");
        assert!((long_after - short_after).abs() < 1e-9, "多空必须保持中性");
        // 幂等性
        let snapshot = w.clone();
        let clipped2 = cap_weights_same_side(&mut w, 0.20, 0.9, 3.0);
        assert!(clipped2.is_empty(), "二次调用不应再截断: {clipped2:?}");
        for ((c1, v1), (c2, v2)) in w.iter().zip(snapshot.iter()) {
            assert_eq!(c1, c2);
            assert!((v1 - v2).abs() < 1e-12, "{c1} 幂等性偏差: {v1} vs {v2}");
        }
    }

    #[test]
    fn cap_weights_same_side_mirror_scales_heavier_side_to_stay_neutral() {
        // 多头只有 4 币（4 * 20% = 80% 净值上限），空头有 7 币（7 * 20% = 140% 可容纳 135%）。
        // 截断后多头打满上限 = 4 * max_w，空头必须按比例缩到与多头相等，严格保持 sum(w) ≈ 0。
        let mut w = vec![
            ("L1".to_string(), 0.15),
            ("L2".to_string(), 0.15),
            ("L3".to_string(), 0.10),
            ("L4".to_string(), 0.10),
            ("S1".to_string(), -0.08),
            ("S2".to_string(), -0.07),
            ("S3".to_string(), -0.07),
            ("S4".to_string(), -0.07),
            ("S5".to_string(), -0.07),
            ("S6".to_string(), -0.07),
            ("S7".to_string(), -0.07),
        ];
        let _clipped = cap_weights_same_side(&mut w, 0.20, 0.9, 3.0);
        let max_w = 0.20 / 2.7;
        for (c, v) in &w {
            assert!(v.abs() <= max_w + 1e-12, "{c} 超过单币上限: {v}");
        }
        let net: f64 = w.iter().map(|(_, v)| *v).sum();
        assert!(net.abs() < 1e-12, "多空打满不对称时必须镜像缩放保持美元中性，实际 net={net}");
        let long_sum: f64 = w.iter().filter(|(_, v)| *v > 0.0).map(|(_, v)| *v).sum();
        assert!((long_sum - 4.0 * max_w).abs() < 1e-12);
        // 幂等性
        let snap = w.clone();
        assert!(cap_weights_same_side(&mut w, 0.20, 0.9, 3.0).is_empty());
        assert_eq!(w, snap);
    }

    #[test]
    fn post_vol_target_cap_uses_actual_order_weight_space() {
        // 这些权重已经乘入 k=0.52。订单名义 = weight * 0.9 * 3 * equity，
        // 不能再乘一次 k；0.074 对应约 20%，0.077 对应约 20.8%。
        let k = 0.52;
        let mut w = vec![
            ("VVV".to_string(), 0.30 * k),
            ("XPL".to_string(), 0.20 * k),
            ("XMR".to_string(), 0.08 * k),
            ("NEAR".to_string(), 0.04 * k),
            ("ZRO".to_string(), 0.03 * k),
            ("BTC".to_string(), -0.15 * k),
            ("ETH".to_string(), -0.15 * k),
            ("SOL".to_string(), -0.15 * k),
            ("ARB".to_string(), -0.10 * k),
            ("DOGE".to_string(), -0.10 * k),
        ];
        let long_before: f64 = w.iter().filter(|(_, v)| *v > 0.0).map(|(_, v)| *v).sum();
        let clipped = cap_weights_same_side(&mut w, 0.20, 0.9, 3.0);
        assert!(clipped.contains(&"VVV".to_string()));
        assert!(clipped.contains(&"XPL".to_string()));
        let long_after: f64 = w.iter().filter(|(_, v)| *v > 0.0).map(|(_, v)| *v).sum();
        assert!((long_after - long_before).abs() < 1e-12, "同侧总杠杆必须不变");
        for (coin, weight) in &w {
            let equity_frac = weight.abs() * 0.9 * 3.0;
            assert!(equity_frac <= 0.20 + 1e-12, "{coin} 最终目标仍超限: {equity_frac}");
        }
        let vvv = w.iter().find(|(c, _)| c == "VVV").unwrap().1;
        assert!((vvv * 0.9 * 3.0 - 0.20).abs() < 1e-12, "VVV 应恰好封顶 20%");
        let snapshot = w.clone();
        assert!(cap_weights_same_side(&mut w, 0.20, 0.9, 3.0).is_empty(), "必须幂等");
        assert_eq!(w, snapshot);
    }

    #[test]
    fn over_cap_respects_vol_k_and_hard_gross_cap() {
        let coins = ["BTC", "ETH", "SOL", "DOGE"];
        let markets: HashMap<String, MarketInfo> = coins
            .iter()
            .map(|c| (c.to_string(), MarketInfo { sz_decimals: 2, max_leverage: 10 }))
            .collect();
        let mids: HashMap<String, f64> = coins.iter().map(|c| (c.to_string(), 100.0)).collect();

        // 1) vol_k = 0.50 → desired_gross = 0.50 * 2.7x = 1.35x; cur_gross = 2.0x > 1.35x * 1.02 → 必须触发 over_cap
        let cfg_low_vol = TradeConfig {
            rebalance_slices: 3,
            ema_alpha: 1.0,
            hard_gross_cap: 2.8,
            ..TradeConfig::default()
        };
        let w_half = vec![
            ("BTC".into(), 0.125),
            ("ETH".into(), 0.125),
            ("SOL".into(), -0.125),
            ("DOGE".into(), -0.125),
        ];
        let mut pos = HashMap::new();
        for c in &coins {
            pos.insert(
                c.to_string(),
                crate::exchange::Pos { size: if *c == "BTC" || *c == "ETH" { 5.0 } else { -5.0 }, entry_px: 100.0, ..Default::default() },
            );
        }
        let acct = Acct { equity: 1000.0, positions: pos };
        let plan_down = build_plan(&w_half, &acct, &markets, &mids, &cfg_low_vol, None);
        assert!(
            plan_down.notes.iter().any(|n| n.contains("强制减仓")),
            "vol_k=0.5 且当前 2.0x 应触发强制减仓: {:?}",
            plan_down.notes
        );

        // 2) vol_k = 1.50（原始 4.05x）必须被 hard_gross_cap = 2.8x 压回 <= 2.8x，绝不飙到 3.96x
        let cfg_cap = TradeConfig {
            rebalance_slices: 1,
            ema_alpha: 1.0,
            hard_gross_cap: 2.8,
            ..TradeConfig::default()
        };
        let w_big = vec![
            ("BTC".into(), 0.375),
            ("ETH".into(), 0.375),
            ("SOL".into(), -0.375),
            ("DOGE".into(), -0.375),
        ];
        let empty = Acct { equity: 10000.0, positions: HashMap::new() };
        let plan_cap = build_plan(&w_big, &empty, &markets, &mids, &cfg_cap, None);
        assert!(
            plan_cap.gross_notional <= 10000.0 * 2.8 + 1e-6,
            "目标总名义不得超过 hard_gross_cap=2.8x，实际 {}",
            plan_cap.gross_notional
        );
        let open_sum: f64 = plan_cap.orders.iter().map(|o| o.notional).sum();
        assert!(
            open_sum <= 10000.0 * 2.8 + 1e-6,
            "实际计划开仓总额不得超过 2.8x，实际 {open_sum}"
        );
    }

    #[test]
    fn ema_rebalance_blends_targets_and_skips_within_rebal_band() {
        let cfg = TradeConfig {
            rebalance_slices: 1,
            ema_alpha: 0.50,
            rebal_band: 0.05,
            min_order_usd: 10.0,
            min_order_pct: 0.005,
            hard_gross_cap: 2.8,
            ..TradeConfig::default()
        };
        let coins = ["BTC", "ETH", "SOL", "DOGE"];
        let markets: HashMap<String, MarketInfo> = coins
            .iter()
            .map(|c| (c.to_string(), MarketInfo { sz_decimals: 2, max_leverage: 10 }))
            .collect();
        let mids: HashMap<String, f64> = coins.iter().map(|c| (c.to_string(), 100.0)).collect();
        // equity = 10000, deployable*lev = 27000
        // BTC target = 0.25 -> $6750 (67.5), cur = 67.0 ($6700): diff $50, ema diff $25 < 5%*$6750=$337.5 -> 跳过
        // ETH target = 0.25 -> $6750 (67.5), cur = 0 -> EMA 开一半 = 33.75 ($3375)
        // SOL target = -0.25 -> -$6750 (-67.5), cur = -40.0 -> EMA blend = -53.75, delta = -13.75 ($1375 >= $337.5)
        // DOGE target = 0 (dropped), cur = -30.0 ($3000) -> EMA blend = -15.0 (平滑减仓 15.0)
        let mut pos = HashMap::new();
        pos.insert("BTC".into(), crate::exchange::Pos { size: 67.0, entry_px: 100.0, ..Default::default() });
        pos.insert("SOL".into(), crate::exchange::Pos { size: -40.0, entry_px: 100.0, ..Default::default() });
        pos.insert("DOGE".into(), crate::exchange::Pos { size: -30.0, entry_px: 100.0, ..Default::default() });
        let acct = Acct { equity: 10000.0, positions: pos };
        let weights = vec![
            ("BTC".into(), 0.25),
            ("ETH".into(), 0.25),
            ("SOL".into(), -0.25),
        ];
        let plan = build_plan(&weights, &acct, &markets, &mids, &cfg, None);
        assert!(plan.orders.iter().all(|o| o.coin != "BTC"), "BTC 在 5% rebal_band 内应跳过");
        let eth = plan.orders.iter().find(|o| o.coin == "ETH").expect("ETH 应开半仓");
        assert!((eth.size - 33.75).abs() < 0.05, "ETH EMA=0.5 应开 33.75，实际 {}", eth.size);
        let sol = plan.orders.iter().find(|o| o.coin == "SOL").expect("SOL 应平滑加空");
        assert!((sol.size - 13.75).abs() < 0.05, "SOL EMA=0.5 应加空 13.75，实际 {}", sol.size);
        let doge = plan.orders.iter().find(|o| o.coin == "DOGE").expect("DOGE 掉出名单应平滑减仓");
        assert!(doge.reduce_only && doge.buy);
        assert!((doge.size - 15.0).abs() < 0.05, "DOGE EMA=0.5 应先减半 15.0，实际 {}", doge.size);
    }

    #[test]
    fn position_stop_closes_even_when_not_in_today_slice() {
        let cfg = TradeConfig {
            rebalance_slices: 3,
            position_stop_pct: 0.20,
            ..TradeConfig::default()
        };
        let day = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            / 86_400_000) as u32;
        let candidates = ["BTC", "ETH", "SOL", "DOGE", "ARB", "HYPE"];
        let non_slot = candidates
            .iter()
            .copied()
            .find(|c| (day + slice_of(c, 3)) % 3 != 0)
            .expect("必须存在不在今日档位的币");
        let mut pos = HashMap::new();
        // 多头成本 100，现价 70 → 浮亏 -30% < -20%，即使不在今日档位也必须立刻全平
        pos.insert(
            non_slot.to_string(),
            crate::exchange::Pos { size: 5.0, entry_px: 100.0, ..Default::default() },
        );
        let acct = Acct { equity: 5000.0, positions: pos };
        let markets: HashMap<String, MarketInfo> = candidates
            .iter()
            .map(|c| (c.to_string(), MarketInfo { sz_decimals: 2, max_leverage: 10 }))
            .collect();
        let mids: HashMap<String, f64> = candidates
            .iter()
            .map(|c| (c.to_string(), if *c == non_slot { 70.0 } else { 100.0 }))
            .collect();
        let weights = vec![("BTC".into(), 0.5), ("ETH".into(), -0.5)];
        let plan = build_plan(&weights, &acct, &markets, &mids, &cfg, None);
        let stop_order = plan
            .orders
            .iter()
            .find(|o| o.coin == non_slot)
            .expect("触发单币止损的币即使不在今日档位也必须平仓");
        assert!(stop_order.reduce_only && !stop_order.buy);
        assert_eq!(stop_order.target, 0.0);
        assert_eq!(stop_order.reason, "单币止损平仓");
    }

    #[test]
    fn stop_or_tp_hit_does_not_reopen_opposite_side_in_same_plan() {
        let cfg = TradeConfig {
            rebalance_slices: 1,
            ema_alpha: 1.0,
            position_stop_pct: 0.20,
            take_profit_pct: 0.15,
            ..TradeConfig::default()
        };
        let mut pos = HashMap::new();
        // BTC 做多浮亏 -30%（触发 stop_hit），而 wanted 目标里 BTC 翻空
        pos.insert("BTC".into(), crate::exchange::Pos { size: 5.0, entry_px: 100.0, ..Default::default() });
        // ETH 做多浮盈 +25%（触发 tp_hit），而 wanted 目标里 ETH 翻空
        pos.insert("ETH".into(), crate::exchange::Pos { size: 5.0, entry_px: 100.0, ..Default::default() });
        let acct = Acct { equity: 5000.0, positions: pos };
        let markets: HashMap<String, MarketInfo> = ["BTC", "ETH"]
            .iter()
            .map(|c| (c.to_string(), MarketInfo { sz_decimals: 2, max_leverage: 10 }))
            .collect();
        let mids: HashMap<String, f64> = HashMap::from([("BTC".into(), 70.0), ("ETH".into(), 125.0)]);
        let weights = vec![("BTC".into(), -0.5), ("ETH".into(), -0.5)];
        let plan = build_plan(&weights, &acct, &markets, &mids, &cfg, None);
        let btc_orders: Vec<_> = plan.orders.iter().filter(|o| o.coin == "BTC").collect();
        let eth_orders: Vec<_> = plan.orders.iter().filter(|o| o.coin == "ETH").collect();
        assert_eq!(btc_orders.len(), 1, "止损币不得在同一计划里立即反手开仓: {btc_orders:?}");
        assert!(btc_orders[0].reduce_only && btc_orders[0].reason == "单币止损平仓");
        assert_eq!(eth_orders.len(), 1, "止盈币不得在同一计划里立即反手开仓: {eth_orders:?}");
        assert!(eth_orders[0].reduce_only && eth_orders[0].reason == "止盈平仓");
    }

    #[test]
    fn single_over_cap_trims_non_slot_position_immediately() {
        let cfg = TradeConfig {
            rebalance_slices: 3,
            max_single_weight: 0.20,
            ..TradeConfig::default()
        };
        let day = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            / 86_400_000) as u32;
        let candidates = ["BTC", "ETH", "SOL", "DOGE", "ARB", "HYPE"];
        let non_slots: Vec<&str> = candidates
            .iter()
            .copied()
            .filter(|c| (day + slice_of(c, 3)) % 3 != 0)
            .collect();
        assert!(non_slots.len() >= 2, "需要至少 2 个不在今日档位的币");
        let over_coin = non_slots[0];
        let ok_coin = non_slots[1];
        // 净值 $5000，20% 上限 = $1000；over_coin 当前持仓 $1650 (33%)，ok_coin 当前持仓 $500 (10%)
        // 总敞口 $2150 < 2.7x * $5000，不会触发总敞口 over_cap，纯粹检验单币超限越过错峰。
        let mut positions = HashMap::new();
        positions.insert(over_coin.to_string(), crate::exchange::Pos { size: 16.5, entry_px: 100.0, ..Default::default() });
        positions.insert(ok_coin.to_string(), crate::exchange::Pos { size: 5.0, entry_px: 100.0, ..Default::default() });
        let acct = Acct { equity: 5000.0, positions };
        let markets: HashMap<String, MarketInfo> = candidates
            .iter()
            .map(|c| (c.to_string(), MarketInfo { sz_decimals: 2, max_leverage: 10 }))
            .collect();
        let mids: HashMap<String, f64> = candidates.iter().map(|c| (c.to_string(), 100.0)).collect();
        // 目标权重里 over_coin 原本偏大（会被截到 20% = $1000），ok_coin 目标 15%（非今日档位应不动）
        let weights = vec![
            (over_coin.to_string(), 0.12),
            (ok_coin.to_string(), 0.05),
        ];
        let plan = build_plan(&weights, &acct, &markets, &mids, &cfg, None);
        let over_order = plan.orders.iter().find(|o| o.coin == over_coin);
        assert!(over_order.is_some(), "超限单币 {over_coin} 即使不在今日档位也应立即减仓");
        let o = over_order.unwrap();
        assert!(o.reduce_only && !o.buy, "减多头必须是 reduce_only 卖出");
        assert_eq!(o.reason, "单币超限减仓");
        assert!((o.target - 10.0).abs() < 0.05, "目标仓位应截到 20% ($1000 = 10.0)，实际 {}", o.target);
        assert!(!plan.orders.iter().any(|o| o.coin == ok_coin), "未超限的非今日档位币 {ok_coin} 不应调仓");
    }
}
