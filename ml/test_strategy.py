"""ML 季度扩展 4 周期集成、buffer13、因果持仓波动率目标与零泄漏训练的离线测试。"""
from pathlib import Path
import copy
import datetime
import json
import math
import sys
import tempfile
import unittest
from unittest import mock

import numpy as np

ML = Path(__file__).resolve().parent
if str(ML) not in sys.path:
    sys.path.insert(0, str(ML))

from strategy import (
    apply_causal_vol_targeting,
    compute_causal_holdings_vol_scale,
    fixed_weight_selection,
    latest_quarter_end,
    load_config,
    model_scores,
    ordered_feature_names,
    predict_component,
    validate_model,
)
from train_model import horizon_eligible_split
from place import account_equity, build_orders
import predict as predict_mod


class StrategyTests(unittest.TestCase):
    def setUp(self):
        self.config = load_config()
        self.coins = np.array(["C%02d" % i for i in range(24)])
        self.scores = np.arange(24.0)

    def test_config_phase2_defaults(self):
        self.assertEqual(self.config["format_version"], 5)
        self.assertEqual(self.config["retrain_frequency"], "quarterly")
        self.assertEqual(self.config["horizons"], [1, 3, 5, 7])
        self.assertEqual([c["horizon"] for c in self.config["ensemble_components"]], [1, 3, 5, 7])
        self.assertTrue(all(abs(c["weight"] - 0.25) < 1e-12 for c in self.config["ensemble_components"]))
        self.assertEqual(self.config["exit_rank"], 13)
        self.assertEqual(self.config["drift_band"], 0.25)
        self.assertEqual(self.config["vol_target"], 0.30)
        self.assertEqual(self.config["vol_lookback_days"], 20)
        self.assertEqual(self.config["max_leverage"], 2.0)
        self.assertEqual(self.config["min_lev_scale"], 0.35)

    def test_latest_quarter_end(self):
        self.assertEqual(latest_quarter_end(datetime.date(2026, 1, 1)), datetime.date(2025, 12, 31))
        self.assertEqual(latest_quarter_end(datetime.date(2026, 3, 31)), datetime.date(2025, 12, 31))
        self.assertEqual(latest_quarter_end(datetime.date(2026, 4, 1)), datetime.date(2026, 3, 31))
        self.assertEqual(latest_quarter_end(datetime.date(2026, 7, 1)), datetime.date(2026, 6, 30))
        self.assertEqual(latest_quarter_end(datetime.date(2026, 10, 1)), datetime.date(2026, 9, 30))
        self.assertEqual(latest_quarter_end(datetime.date(2026, 11, 27)), datetime.date(2026, 9, 30))
        self.assertEqual(latest_quarter_end(datetime.date(2027, 1, 1)), datetime.date(2026, 12, 31))

    def test_json_missing_direction(self):
        component = {"trees": [[
            {"feat": 0, "thr": 0.0, "left": 1, "right": 2, "miss": 1},
            {"leaf": 1.25}, {"leaf": -0.5},
        ]]}
        self.assertEqual(predict_component(component, np.array([0.0])), 1.25)
        self.assertEqual(predict_component(component, np.array([1.0])), -0.5)
        self.assertEqual(predict_component(component, np.array([np.nan])), 1.25)
        for threshold, expected in [(0.5, 1.25), (-0.5, -0.5)]:
            none_component = copy.deepcopy(component)
            none_component["trees"][0][0].update({"thr": threshold, "missing_type": "None"})
            self.assertEqual(predict_component(none_component, np.array([np.nan])), expected)
        zero_component = copy.deepcopy(component)
        zero_component["trees"][0][0].update({"thr": -0.5, "missing_type": "Zero"})
        self.assertEqual(predict_component(zero_component, np.array([0.0])), 1.25)

    def test_equal_weight_four_horizon_ensemble_scores(self):
        model = {"components": [
            {"name": "close1", "horizon": 1, "weight": 0.25, "trees": [[{"leaf": 1.0}]]},
            {"name": "close3", "horizon": 3, "weight": 0.25, "trees": [[{"leaf": 2.0}]]},
            {"name": "close5", "horizon": 5, "weight": 0.25, "trees": [[{"leaf": 3.0}]]},
            {"name": "close7", "horizon": 7, "weight": 0.25, "trees": [[{"leaf": 4.0}]]},
        ]}
        np.testing.assert_allclose(model_scores(model, np.zeros((4, 1))), np.full(4, 2.5))

    def test_fixed_two_times_weights(self):
        selected = fixed_weight_selection(self.scores, self.coins, self.config)
        self.assertEqual(selected["long"], ["C23", "C22", "C21", "C20", "C19", "C18", "C17", "C16"])
        self.assertEqual(selected["short"], ["C00", "C01", "C02", "C03", "C04", "C05", "C06", "C07"])
        self.assertAlmostEqual(sum(selected["weights"].values()), 0.0)
        self.assertAlmostEqual(sum(abs(w) for w in selected["weights"].values()), 2.0)
        self.assertTrue(all(abs(w) == 1 / 8 for w in selected["weights"].values()))

    def test_buffer13_retains_old_long_positions(self):
        # 24 币（C23 第 1 名 .. C00 第 24 名）：C11 排第 13 名（保留），C10 排第 14 名（移出）。
        current = {"C11": 1.0, "C10": 1.0}
        selected = fixed_weight_selection(self.scores, self.coins, self.config, current)
        self.assertIn("C11", selected["long"])
        self.assertNotIn("C10", selected["long"])
        self.assertNotIn("C16", selected["long"])  # 保留 C11 后只补前 7 名
        self.assertEqual(len(selected["long"]), 8)
        self.assertEqual(len(selected["short"]), 8)
        self.assertFalse(set(selected["long"]) & set(selected["short"]))

    def test_buffer13_short_boundary(self):
        # 24 币中倒数第 13 名是 rank = 24 - 13 + 1 = 12（C12）；rank 11（C13）在缓冲外。
        current = {
            "C12": -1.0, "C13": -1.0,
            "C00": -1.0, "C01": -1.0, "C02": -1.0,
            "C03": -1.0, "C04": -1.0, "C05": -1.0,
        }
        selected = fixed_weight_selection(self.scores, self.coins, self.config, current)
        self.assertIn("C12", selected["short"])
        self.assertNotIn("C13", selected["short"])
        self.assertIn("C06", selected["short"])

    def test_excess_buffer_positions_are_trimmed_and_dust_does_not_block(self):
        # 多空各有 9 个缓冲内旧仓：保留各自排名最强的 8 个，不能再拒绝调仓。
        current = {f"C{i:02d}": 0.001 for i in range(15, 24)}
        current.update({f"C{i:02d}": -0.001 for i in range(0, 9)})
        selected = fixed_weight_selection(self.scores, self.coins, self.config, current)
        self.assertEqual(selected["long"], [f"C{i:02d}" for i in range(23, 15, -1)])
        self.assertEqual(selected["short"], [f"C{i:02d}" for i in range(0, 8)])
        self.assertNotIn("C15", selected["weights"])
        self.assertNotIn("C08", selected["weights"])

        # 被裁掉的旧仓低于 $10，订单层仍可跳过；不得反过来卡死目标生成。
        orders = build_orders(
            selected["weights"], current,
            {coin: i for i, coin in enumerate(self.coins)},
            {coin: 0 for coin in self.coins}, equity=800.0,
            mids={coin: "1" for coin in self.coins},
        )
        self.assertEqual(len(orders), 16)
        self.assertNotIn(15, {order["a"] for order in orders})
        self.assertNotIn(8, {order["a"] for order in orders})


class CausalVolTargetingTests(unittest.TestCase):
    def setUp(self):
        self.config = load_config()
        self.coins = np.array([f"C{i:02d}" for i in range(16)])
        self.weights = {f"C{i:02d}": 0.125 for i in range(8)}
        self.weights.update({f"C{i:02d}": -0.125 for i in range(8, 16)})

    def _make_price_panel(self, n_days, long_daily_rets, short_daily_rets=None):
        """构造给定多/空日收益序列的闭合价格矩阵。"""
        start = np.datetime64("2026-10-01", "D")
        dates = start + np.arange(n_days)
        closes = np.ones((n_days, 16), dtype=float) * 100.0
        short_daily_rets = np.zeros(n_days - 1) if short_daily_rets is None else short_daily_rets
        for d in range(1, n_days):
            closes[d, :8] = closes[d - 1, :8] * (1.0 + long_daily_rets[d - 1])
            closes[d, 8:] = closes[d - 1, 8:] * (1.0 + short_daily_rets[d - 1])
        return dates, closes

    def test_scales_down_high_vol_and_respects_floor(self):
        # 多腿日收益在 ±3% 交替（总权重 1.0），空腿 0 → 组合日收益 ±3%，std(ddof=1) ≈ 0.03078
        n_days = 30
        alt = np.array([0.03 if i % 2 == 0 else -0.03 for i in range(n_days - 1)])
        dates, closes = self._make_price_panel(n_days, alt)
        sig_date = str(dates[-1])

        info = compute_causal_holdings_vol_scale(
            self.weights, closes, self.coins, dates, sig_date, self.config,
        )
        expected_rv = float(np.std(alt[-20:], ddof=1)) * math.sqrt(365.0)
        self.assertAlmostEqual(info["rv20"], expected_rv, places=10)
        expected_scale = max(0.35, min(1.0, 0.30 / expected_rv))
        self.assertAlmostEqual(info["lev_scale"], expected_scale, places=10)
        self.assertLess(info["lev_scale"], 1.0)
        self.assertGreaterEqual(info["lev_scale"], 0.35)
        self.assertTrue(info["vol_target_active"])
        self.assertAlmostEqual(info["effective_gross"], 2.0 * expected_scale, places=10)

        # 极端高波动：触及 min_lev_scale=0.35 下限
        extreme = np.array([0.10 if i % 2 == 0 else -0.10 for i in range(n_days - 1)])
        dates2, closes2 = self._make_price_panel(n_days, extreme)
        info2 = compute_causal_holdings_vol_scale(
            self.weights, closes2, self.coins, dates2, str(dates2[-1]), self.config,
        )
        self.assertAlmostEqual(info2["lev_scale"], 0.35, places=10)
        self.assertAlmostEqual(info2["effective_gross"], 0.70, places=10)

    def test_caps_at_max_leverage_in_low_vol_regime(self):
        n_days = 30
        tiny = np.array([0.002 if i % 2 == 0 else -0.002 for i in range(n_days - 1)])
        dates, closes = self._make_price_panel(n_days, tiny)
        cfg = copy.deepcopy(self.config)
        cfg["max_lev_scale"] = 1.5  # 即使 max_lev_scale>1，也必须被 max_leverage=2.0 硬上限封顶
        info = compute_causal_holdings_vol_scale(
            self.weights, closes, self.coins, dates, str(dates[-1]), cfg,
        )
        self.assertAlmostEqual(info["lev_scale"], 1.0, places=10)
        self.assertLessEqual(info["effective_gross"], cfg["max_leverage"] + 1e-12)

    def test_strictly_causal_ignores_future_bars(self):
        n_days = 35
        rng = np.random.default_rng(7)
        rets = rng.normal(0.0, 0.025, size=n_days - 1)
        dates, closes = self._make_price_panel(n_days, rets)
        sig_date = str(dates[25])  # 留出 9 天“未来”数据

        info_clean = compute_causal_holdings_vol_scale(
            self.weights, closes[:26], self.coins, dates[:26], sig_date, self.config,
        )
        # 在 signal_date 之后注入 10x 极端暴跌，绝不能影响 rv20 或 lev_scale
        closes_future_shock = closes.copy()
        closes_future_shock[26:, :8] *= 0.05
        closes_future_shock[26:, 8:] *= 5.0
        info_with_future = compute_causal_holdings_vol_scale(
            self.weights, closes_future_shock, self.coins, dates, sig_date, self.config,
        )
        self.assertEqual(info_clean["rv20"], info_with_future["rv20"])
        self.assertEqual(info_clean["lev_scale"], info_with_future["lev_scale"])

    def test_disabled_when_vol_target_zero(self):
        n_days = 30
        alt = np.array([0.08 if i % 2 == 0 else -0.08 for i in range(n_days - 1)])
        dates, closes = self._make_price_panel(n_days, alt)
        cfg = copy.deepcopy(self.config)
        cfg["vol_target"] = 0.0
        selected = {"long": list(self.coins[:8]), "short": list(self.coins[8:]), "weights": dict(self.weights)}
        scaled, info = apply_causal_vol_targeting(selected, closes, self.coins, dates, str(dates[-1]), cfg)
        self.assertFalse(info["vol_target_active"])
        self.assertAlmostEqual(info["lev_scale"], 1.0, places=12)
        self.assertAlmostEqual(sum(abs(w) for w in scaled.values()), 2.0, places=12)

    def test_insufficient_history_falls_back_gracefully(self):
        # 仅 12 天价格（11 个日收益 < 20）→ 回退 lev_scale=1.0
        n_days = 12
        alt = np.array([0.08 if i % 2 == 0 else -0.08 for i in range(n_days - 1)])
        dates, closes = self._make_price_panel(n_days, alt)
        info = compute_causal_holdings_vol_scale(
            self.weights, closes, self.coins, dates, str(dates[-1]), self.config,
        )
        self.assertIsNone(info["rv20"])
        self.assertAlmostEqual(info["lev_scale"], 1.0, places=12)
        self.assertEqual(info["fallback_reason"], "insufficient_history")


class TrainCutoffAndPredictTests(unittest.TestCase):
    def test_strict_label_end_cutoff_per_horizon(self):
        # 构造 400 天连续日历，train_end 设在倒数第 25 天，验证每个 h in [1,3,5,7] 绝不越界泄漏
        start = np.datetime64("2025-01-01", "D")
        ts = start + np.arange(400)
        k = 4
        n = len(ts)
        row_days = np.repeat(np.arange(n), k)
        row_labels = np.ones(n * k, dtype=float)
        train_start = "2025-01-01"
        train_end = str(ts[374])  # 后面还有 25 天“未来”数据
        train_end64 = np.datetime64(train_end, "D")

        for h in (1, 3, 5, 7):
            eligible, train_mask, valid_mask, split, label_end = horizon_eligible_split(
                ts, row_days, row_labels, h, train_start, train_end,
            )
            elig_dates = ts[row_days[eligible]]
            elig_label_ends = label_end[eligible]
            # 硬约束 1：所有合格样本的标签到期日 <= train_end
            self.assertLessEqual(elig_label_ends.max(), train_end64)
            # 硬约束 2：最大样本日严格等于 train_end - h 天
            self.assertEqual(elig_dates.max(), train_end64 - np.timedelta64(h, "D"))
            # 硬约束 3：训练集与验证集零时间重叠（train label_end < split <= valid row_date）
            self.assertLess(label_end[train_mask].max(), split)
            self.assertGreaterEqual(ts[row_days[valid_mask]].min(), split)
            self.assertFalse((train_mask & valid_mask).any())

    def test_predict_loads_v5_model_and_applies_causal_vt(self):
        cfg = load_config()
        names = ordered_feature_names()
        n_days, k = 30, 24
        start_ms = int(datetime.datetime(2026, 10, 28, tzinfo=datetime.timezone.utc).timestamp() * 1000)
        ts_ms = np.array([start_ms + i * 86400000 for i in range(n_days)], dtype=np.int64)
        coins = np.array([f"C{j:02d}" for j in range(k)])
        members = np.ones((n_days, k), dtype=bool)
        # 构造高波价格让 VT30 真正生效（lev_scale < 1.0）
        closes = np.ones((n_days, k), dtype=float) * 100.0
        for d in range(1, n_days):
            r_hi = 0.04 if d % 2 == 0 else -0.04
            closes[d, 12:] = closes[d - 1, 12:] * (1.0 + r_hi)
            closes[d, :12] = closes[d - 1, :12] * (1.0 - r_hi)
        feats = {f"F_{nm}": np.zeros((n_days, k), dtype=float) for nm in names}
        # 用 F_kmid 编码截面分数，F_log_price 编码闭合收盘价
        feats["F_kmid"][-1, :] = np.arange(k, dtype=float)
        feats["F_log_price"] = np.log(closes)

        # 每个组件仅 1 棵基于 feat=0 (kmid) 的简单树
        model = {
            "format_version": 5,
            "strategy_id": cfg["strategy_id"],
            "model": cfg["label"],
            "horizons": [1, 3, 5, 7],
            "train_start": cfg["train_start"],
            "train_end": "2026-11-26",
            "retrain_frequency": "quarterly",
            "feature_names": names,
            "components": [
                {
                    "name": spec["name"],
                    "horizon": spec["horizon"],
                    "weight": spec["weight"],
                    "best_iteration": 1,
                    "trees": [[{"feat": 0, "thr": 11.5, "left": 1, "right": 2, "miss": 1},
                               {"leaf": -1.0}, {"leaf": 1.0}]],
                }
                for spec in cfg["ensemble_components"]
            ],
        }
        validate_model(model, cfg)

        with tempfile.TemporaryDirectory() as td:
            td = Path(td)
            feat_path = td / "features.npz"
            model_path = td / "model.json"
            np.savez(feat_path, ts=ts_ms, coins=coins, members=members, **feats)
            model_path.write_text(json.dumps(model), encoding="utf-8")

            res = predict_mod.latest_positions(
                features_path=feat_path, model_path=model_path,
                current_positions={"C23": 1.0},
            )
            self.assertEqual(res["horizons"], [1, 3, 5, 7])
            self.assertEqual(res["buffer"], 13)
            self.assertFalse(res["preview_without_current_positions"])
            self.assertIsNotNone(res["rv20"])
            self.assertLess(res["lev_scale"], 1.0)
            self.assertGreaterEqual(res["lev_scale"], 0.35)
            self.assertLessEqual(res["gross_leverage"], 2.0 + 1e-9)
            self.assertAlmostEqual(
                sum(abs(w) for w in res["weights"].values()),
                res["gross_leverage"], places=9,
            )

    def test_production_model_json_matches_v5_and_zero_leakage(self):
        from strategy import load_model
        cfg = load_config()
        model = load_model()
        validate_model(model, cfg)
        self.assertEqual(model["train_start"], "2023-06-01")
        self.assertEqual(model["train_end"], "2026-11-26")
        self.assertEqual(model["retrain_frequency"], "quarterly")
        self.assertEqual([c["horizon"] for c in model["components"]], [1, 3, 5, 7])
        self.assertEqual(len(model["feature_names"]), 151)
        for name, rep in model["training"].items():
            self.assertLessEqual(rep["max_label_end_date"], model["train_end"], msg=name)
            self.assertLessEqual(rep["json_max_error"], 1e-10, msg=name)


class UnifiedEquityTests(unittest.TestCase):
    @unittest.mock.patch("place.info_post")
    def test_uses_unified_spot_total(self, info_post):
        info_post.side_effect = [
            {"marginSummary": {"accountValue": "568.136963"}},
            {"balances": [{"coin": "USDC", "total": "2345.230768"}]},
        ]
        self.assertAlmostEqual(account_equity("0xvault"), 2345.230768)

    @unittest.mock.patch("place.info_post")
    def test_spot_failure_never_falls_back_to_perp_subaccount(self, info_post):
        info_post.side_effect = [
            {"marginSummary": {"accountValue": "568.136963"}},
            RuntimeError("temporary spot failure"),
        ]
        with self.assertRaisesRegex(RuntimeError, "temporary spot failure"):
            account_equity("0xvault")


class OrderTests(unittest.TestCase):
    def test_weight_sizing_and_legacy_position_close(self):
        target = {"A": 1 / 8, "B": -1 / 8}
        current = {"A": 0.0, "C": 1.0}
        orders = build_orders(
            target, current, {"A": 0, "B": 1, "C": 2}, {"A": 3, "B": 3, "C": 3},
            equity=1200.0, mids={"A": "100", "B": "50", "C": "20"},
        )
        # C 全平应排在最前面且 reduce_only=True
        self.assertEqual(orders[0]["a"], 2)
        self.assertTrue(orders[0]["r"])
        by_asset = {order["a"]: order for order in orders}
        self.assertEqual(float(by_asset[0]["s"]), 1.5)
        self.assertEqual(float(by_asset[1]["s"]), 3.0)
        self.assertEqual(float(by_asset[2]["s"]), 1.0)
        self.assertTrue(by_asset[0]["b"])
        self.assertFalse(by_asset[1]["b"])
        self.assertFalse(by_asset[2]["b"])

    def test_empty_or_invalid_target_fails_closed(self):
        with self.assertRaises(ValueError):
            build_orders({}, {}, {}, {}, 1000.0, mids={})
        with self.assertRaises(ValueError):
            build_orders({"A": np.nan}, {}, {"A": 0}, {}, 1000.0, mids={"A": "1"})

    def test_rounded_order_below_minimum_is_skipped(self):
        # 复现 ZEC 边界：取整前差额约 $10.05，但 0.01 ZEC 的最终订单仅约 $8.36。
        orders = build_orders(
            {"ZEC": 1 / 8}, {"ZEC": 0.27}, {"ZEC": 0}, {"ZEC": 2},
            equity=1734.0, mids={"ZEC": "840"}, drift_band=0.0,
        )
        self.assertEqual(orders, [])


if __name__ == "__main__":
    unittest.main()
