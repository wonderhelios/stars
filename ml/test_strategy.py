"""ML 冻结三周期集成、buffer11 和订单生产的离线测试，不访问外部接口。"""
from pathlib import Path
import copy
import sys
import unittest
from unittest import mock

import numpy as np

ML = Path(__file__).resolve().parent
if str(ML) not in sys.path:
    sys.path.insert(0, str(ML))

from strategy import fixed_weight_selection, load_config, model_scores, predict_component
from place import account_equity, build_orders


class StrategyTests(unittest.TestCase):
    def setUp(self):
        self.config = load_config()
        self.coins = np.array(["C%02d" % i for i in range(24)])
        self.scores = np.arange(24.0)

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

    def test_equal_weight_ensemble_scores(self):
        model = {"components": [
            {"name": "close1", "weight": 1 / 3, "trees": [[{"leaf": 1.0}]]},
            {"name": "close3", "weight": 1 / 3, "trees": [[{"leaf": 2.0}]]},
            {"name": "close5", "weight": 1 / 3, "trees": [[{"leaf": 3.0}]]},
        ]}
        np.testing.assert_allclose(model_scores(model, np.zeros((4, 1))), np.full(4, 2.0))

    def test_fixed_two_times_weights(self):
        selected = fixed_weight_selection(self.scores, self.coins, self.config)
        self.assertEqual(selected["long"], ["C23", "C22", "C21", "C20", "C19", "C18", "C17", "C16"])
        self.assertEqual(selected["short"], ["C00", "C01", "C02", "C03", "C04", "C05", "C06", "C07"])
        self.assertAlmostEqual(sum(selected["weights"].values()), 0.0)
        self.assertAlmostEqual(sum(abs(w) for w in selected["weights"].values()), 2.0)
        self.assertTrue(all(abs(w) == 1 / 8 for w in selected["weights"].values()))

    def test_buffer11_retains_old_positions(self):
        # 24 币中 C13 多头排第 11，C12 排第 12：前者保留，后者替换。
        current = {"C13": 1.0, "C12": 1.0}
        selected = fixed_weight_selection(self.scores, self.coins, self.config, current)
        self.assertIn("C13", selected["long"])
        self.assertNotIn("C12", selected["long"])
        self.assertNotIn("C16", selected["long"])  # 缓冲保留后只补前七名
        self.assertEqual(len(selected["long"]), 8)
        self.assertEqual(len(selected["short"]), 8)
        self.assertFalse(set(selected["long"]) & set(selected["short"]))

    def test_buffer11_short_boundary(self):
        # 排名从高到低：C10 排第 14，位于倒数第 11；C11 排第 13，在缓冲外。
        current = {"C10": -1.0, "C11": -1.0,
                   "C00": -1.0, "C01": -1.0, "C02": -1.0,
                   "C03": -1.0, "C04": -1.0, "C05": -1.0}
        selected = fixed_weight_selection(self.scores, self.coins, self.config, current)
        self.assertIn("C10", selected["short"])
        self.assertNotIn("C11", selected["short"])
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
            equity=1734.0, mids={"ZEC": "840"},
        )
        self.assertEqual(orders, [])


if __name__ == "__main__":
    unittest.main()
