"""ML 冻结单模型、buffer9 和订单生产的离线测试，不访问外部接口。"""
from pathlib import Path
import copy
import sys
import unittest

import numpy as np

ML = Path(__file__).resolve().parent
if str(ML) not in sys.path:
    sys.path.insert(0, str(ML))

from strategy import fixed_weight_selection, load_config, predict_component
from place import build_orders


class StrategyTests(unittest.TestCase):
    def setUp(self):
        self.config = load_config()
        self.coins = np.array(["C%02d" % i for i in range(20)])
        self.scores = np.arange(20.0)

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

    def test_fixed_two_times_weights(self):
        selected = fixed_weight_selection(self.scores, self.coins, self.config)
        self.assertEqual(selected["long"], ["C19", "C18", "C17", "C16", "C15", "C14"])
        self.assertEqual(selected["short"], ["C00", "C01", "C02", "C03", "C04", "C05"])
        self.assertAlmostEqual(sum(selected["weights"].values()), 0.0)
        self.assertAlmostEqual(sum(abs(w) for w in selected["weights"].values()), 2.0)
        self.assertTrue(all(abs(w) == 1 / 6 for w in selected["weights"].values()))

    def test_buffer9_retains_old_positions(self):
        # C11 多头排第 9，C10 空头排倒数第 10：前者保留，后者替换。
        current = {"C11": 1.0, "C10": -1.0}
        selected = fixed_weight_selection(self.scores, self.coins, self.config, current)
        self.assertIn("C11", selected["long"])
        self.assertNotIn("C14", selected["long"])  # 缓冲保留后只补前五名
        self.assertNotIn("C10", selected["short"])
        self.assertEqual(len(selected["long"]), 6)
        self.assertEqual(len(selected["short"]), 6)
        self.assertFalse(set(selected["long"]) & set(selected["short"]))

    def test_buffer9_short_boundary(self):
        current = {"C09": -1.0, "C08": -1.0,
                   "C00": -1.0, "C01": -1.0, "C02": -1.0, "C03": -1.0}
        selected = fixed_weight_selection(self.scores, self.coins, self.config, current)
        self.assertNotIn("C09", selected["short"])  # 倒数第 10 被移出
        self.assertIn("C08", selected["short"])  # 倒数第 9 保留
        self.assertIn("C04", selected["short"])  # 按分数补入下一空仓


class OrderTests(unittest.TestCase):
    def test_weight_sizing_and_legacy_position_close(self):
        target = {"A": 1 / 6, "B": -1 / 6}
        current = {"A": 0.0, "C": 1.0}
        orders = build_orders(
            target, current, {"A": 0, "B": 1, "C": 2}, {"A": 3, "B": 3, "C": 3},
            equity=1200.0, mids={"A": "100", "B": "50", "C": "20"},
        )
        by_asset = {order["a"]: order for order in orders}
        self.assertEqual(float(by_asset[0]["s"]), 2.0)
        self.assertEqual(float(by_asset[1]["s"]), 4.0)
        self.assertEqual(float(by_asset[2]["s"]), 1.0)
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
