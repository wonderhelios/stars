"""ML 四模型生产链路的离线测试，不访问行情或交易接口。"""
from pathlib import Path
import copy
import sys
import unittest

import numpy as np

ML = Path(__file__).resolve().parent
if str(ML) not in sys.path:
    sys.path.insert(0, str(ML))

from ensemble import (
    component_labels, component_maturity, ensemble_scores, fixed_weight_selection,
    load_config, predict_component, validate_execution_quarter,
)
from place import build_orders


class EnsembleTests(unittest.TestCase):
    def setUp(self):
        self.config = load_config()

    def test_label_formula_and_maturity(self):
        n = 9
        close = np.column_stack([100 + np.arange(n), 200 + 3 * np.arange(n)]).astype(float)
        open_prices = np.column_stack([50 + np.arange(n), 400 + 4 * np.arange(n)]).astype(float)
        members = np.ones((n, 2), dtype=bool)
        close3 = component_labels("close3", open_prices, close, members)
        close5 = component_labels("close5", open_prices, close, members)
        open3 = component_labels("open3", open_prices, close, members)
        open5 = component_labels("open5", open_prices, close, members)
        self.assertEqual(component_maturity("close3"), 3)
        self.assertEqual(component_maturity("close5"), 5)
        self.assertEqual(component_maturity("open3"), 4)
        self.assertEqual(component_maturity("open5"), 6)
        # 较小收益排第一分位 0.5，较大收益排第二分位 1.0。
        np.testing.assert_allclose(close3[0], [0.5, 1.0])
        self.assertAlmostEqual(close[3, 0] / close[0, 0] - 1, 0.03)
        np.testing.assert_allclose(close5[0], [0.5, 1.0])
        # open3 是 O[4]/O[1]-1，open5 是 O[6]/O[1]-1。
        self.assertAlmostEqual(open_prices[4, 0] / open_prices[1, 0] - 1, 3 / 51)
        self.assertAlmostEqual(open_prices[6, 0] / open_prices[1, 0] - 1, 5 / 51)
        np.testing.assert_allclose(open3[0], [1.0, 0.5])
        np.testing.assert_allclose(open5[0], [1.0, 0.5])
        self.assertTrue(np.isnan(close3[-3:]).all())
        self.assertTrue(np.isnan(close5[-5:]).all())
        self.assertTrue(np.isnan(open3[-4:]).all())
        self.assertTrue(np.isnan(open5[-6:]).all())

    def test_json_missing_direction(self):
        component = {"trees": [[
            {"feat": 0, "thr": 0.0, "left": 1, "right": 2, "miss": 1},
            {"leaf": 1.25}, {"leaf": -0.5},
        ]]}
        self.assertEqual(predict_component(component, np.array([0.0])), 1.25)
        self.assertEqual(predict_component(component, np.array([1.0])), -0.5)
        self.assertEqual(predict_component(component, np.array([np.nan])), 1.25)
        # missing_type=None 时 NaN 先变成 0，再与正负阈值比较。
        for threshold, expected in [(0.5, 1.25), (-0.5, -0.5)]:
            none_component = copy.deepcopy(component)
            none_component["trees"][0][0].update({"thr": threshold, "missing_type": "None"})
            self.assertEqual(predict_component(none_component, np.array([np.nan])), expected)
        zero_component = copy.deepcopy(component)
        zero_component["trees"][0][0].update({"thr": -0.5, "missing_type": "Zero"})
        self.assertEqual(predict_component(zero_component, np.array([0.0])), 1.25)

    def test_daily_rank_ensemble(self):
        up = {"name": "up", "trees": [[
            {"feat": 0, "thr": 0.0, "left": 1, "right": 2, "miss": 1},
            {"leaf": 0.0}, {"leaf": 1.0},
        ]]}
        down = copy.deepcopy(up)
        down["name"] = "down"
        down["trees"][0][1]["leaf"] = 1.0
        down["trees"][0][2]["leaf"] = 0.0
        scores = ensemble_scores({"components": [up, down]}, np.array([[-1.0], [0.0], [1.0]]))
        np.testing.assert_allclose(scores, [2 / 3, 2 / 3, 2 / 3])

    def test_fixed_two_times_weights(self):
        selected = fixed_weight_selection(np.arange(20.0), np.array(["C%02d" % i for i in range(20)]), self.config)
        self.assertEqual(selected["long"], ["C19", "C18", "C17", "C16", "C15", "C14"])
        self.assertEqual(selected["short"], ["C00", "C01", "C02", "C03", "C04", "C05"])
        self.assertAlmostEqual(sum(selected["weights"].values()), 0.0)
        self.assertAlmostEqual(sum(abs(w) for w in selected["weights"].values()), 2.0)
        self.assertAlmostEqual(sum(w for w in selected["weights"].values() if w > 0), 1.0)
        self.assertTrue(all(abs(w) == 1 / 6 for w in selected["weights"].values()))

    def test_execution_quarter_and_fail_closed(self):
        model = {"quarter": "2026-Q1"}
        self.assertEqual(validate_execution_quarter(model, "2025-12-31"), "2026-Q1")
        self.assertEqual(validate_execution_quarter({"quarter": "2026-Q4"}, "2026-11-09"), "2026-Q4")
        with self.assertRaises(ValueError):
            validate_execution_quarter({"quarter": "2026-Q3"}, "2026-11-09")


class OrderTests(unittest.TestCase):
    def test_weight_sizing_and_legacy_position_close(self):
        target = {"A": 1 / 6, "B": -1 / 6}
        current = {"A": 0.0, "C": 1.0}
        orders = build_orders(
            target, current, {"A": 0, "B": 1, "C": 2}, {"A": 3, "B": 3, "C": 3},
            equity=1200.0, mids={"A": "100", "B": "50", "C": "20"},
        )
        by_asset = {order["a"]: order for order in orders}
        self.assertEqual(float(by_asset[0]["s"]), 2.0)   # +200 / 100
        self.assertEqual(float(by_asset[1]["s"]), 4.0)   # -200 / 50
        self.assertEqual(float(by_asset[2]["s"]), 1.0)   # 目标外旧仓归零
        self.assertTrue(by_asset[0]["b"])
        self.assertFalse(by_asset[1]["b"])
        self.assertFalse(by_asset[2]["b"])

    def test_empty_or_invalid_target_fails_closed(self):
        with self.assertRaises(ValueError):
            build_orders({}, {}, {}, {}, 1000.0, mids={})
        with self.assertRaises(ValueError):
            build_orders({"A": np.nan}, {}, {"A": 0}, {}, 1000.0, mids={"A": "1"})


if __name__ == "__main__":
    unittest.main()
