"""ml/place.py 的 Alo Maker 优先执行与防重查单单元测试。"""
from __future__ import annotations

import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import place


class FakeClock:
    def __init__(self):
        self.t = 0.0

    def monotonic(self):
        return self.t

    def sleep(self, dt):
        self.t += max(0.0, dt)


class TestPlaceMakerFirst(unittest.TestCase):
    def setUp(self):
        self.mids = {"BTC": 100000.0, "ETH": 2000.0}
        self.decimals = {"BTC": 5, "ETH": 4}
        self.coin_of = {0: "BTC", 1: "ETH"}
        self.orders = [
            {"a": 0, "b": True, "p": "100500", "s": "0.01000", "r": False, "t": {"limit": {"tif": "Ioc"}}},
            {"a": 1, "b": False, "p": "1990", "s": "0.5000", "r": False, "t": {"limit": {"tif": "Ioc"}}},
        ]

    def test_build_alo_order_direction_and_cloid(self):
        cloid = place.make_cloid()
        self.assertTrue(cloid.startswith("0x") and len(cloid) == 34)
        buy_o = place.build_alo_order(self.orders[0], 100000.0, 5, cloid)
        sell_o = place.build_alo_order(self.orders[1], 2000.0, 4, cloid)
        self.assertEqual(buy_o["t"], {"limit": {"tif": "Alo"}})
        self.assertEqual(buy_o["c"], cloid)
        self.assertLess(float(buy_o["p"]), 100000.0)
        self.assertGreater(float(sell_o["p"]), 2000.0)

    def test_all_maker_filled_immediately(self):
        calls = []

        def fake_exchange(wallet, action, vault):
            calls.append(action)
            return {"status": "ok", "response": {"data": {"statuses": [{"filled": {"oid": 1}}, {"filled": {"oid": 2}}]}}}

        with mock.patch.object(place, "exchange_post", side_effect=fake_exchange):
            res = place.execute_orders_maker_first(
                "0x" + "1" * 64, "0x" + "2" * 40, self.orders, self.mids, self.decimals, self.coin_of
            )
        self.assertEqual(res["maker_filled"], 2)
        self.assertEqual(res["ioc_fallback"], 0)
        self.assertEqual(res["errors"], [])
        self.assertEqual(len(calls), 1)
        self.assertEqual(calls[0]["orders"][0]["t"], {"limit": {"tif": "Alo"}})

    def test_post_only_rejected_falls_back_to_ioc(self):
        calls = []

        def fake_exchange(wallet, action, vault):
            calls.append(action)
            if len(calls) == 1:
                return {"status": "ok", "response": {"data": {"statuses": [
                    {"filled": {"oid": 1}},
                    {"error": "Post only order would have immediately matched"},
                ]}}}
            return {"status": "ok", "response": {"data": {"statuses": [{"filled": {"oid": 3}}]}}}

        with mock.patch.object(place, "exchange_post", side_effect=fake_exchange), \
             mock.patch.object(place, "info_post", return_value=self.mids):
            res = place.execute_orders_maker_first(
                "0x" + "1" * 64, "0x" + "2" * 40, self.orders, self.mids, self.decimals, self.coin_of
            )
        self.assertEqual(res["maker_filled"], 1)
        self.assertEqual(res["ioc_fallback"], 1)
        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[1]["orders"][0]["a"], 1)
        self.assertEqual(calls[1]["orders"][0]["t"], {"limit": {"tif": "Ioc"}})

    def test_resting_fills_during_poll_no_cancel_no_ioc(self):
        clock = FakeClock()
        calls = []

        def fake_exchange(wallet, action, vault):
            calls.append(action)
            return {"status": "ok", "response": {"data": {"statuses": [
                {"resting": {"oid": 101}},
                {"filled": {"oid": 102}},
            ]}}}

        with mock.patch.object(place, "exchange_post", side_effect=fake_exchange), \
             mock.patch.object(place, "query_order_status", return_value={"status": "order", "order": {"status": "filled", "order": {"oid": 101, "sz": "0"}}}):
            res = place.execute_orders_maker_first(
                "0x" + "1" * 64, "0x" + "2" * 40, self.orders, self.mids, self.decimals, self.coin_of,
                timeout_sec=12.0, poll_sec=1.5, sleep_fn=clock.sleep, monotonic_fn=clock.monotonic
            )
        self.assertEqual(res["maker_filled"], 2)
        self.assertEqual(res["ioc_fallback"], 0)
        self.assertEqual(len(calls), 1)  # 无撤单、无 IOC

    def test_resting_timeout_partial_fill_cancels_and_ioc_remainder_only(self):
        clock = FakeClock()
        calls = []

        def fake_exchange(wallet, action, vault):
            calls.append(action)
            if action["type"] == "order" and action["orders"][0]["t"]["limit"]["tif"] == "Alo":
                return {"status": "ok", "response": {"data": {"statuses": [
                    {"resting": {"oid": 201}},
                    {"filled": {"oid": 202}},
                ]}}}
            return {"status": "ok", "response": {"data": {"statuses": ["success"]}}}

        # 轮询时一直 open，撤单后再查返回 canceled 且剩余 sz=0.004（原 0.01，已成交 0.006）
        def fake_status(vault, cloid):
            if any(c["type"] == "cancel" for c in calls):
                return {"status": "order", "order": {"status": "canceled", "order": {"oid": 201, "sz": "0.00400"}}}
            return {"status": "order", "order": {"status": "open", "order": {"oid": 201, "sz": "0.00400"}}}

        with mock.patch.object(place, "exchange_post", side_effect=fake_exchange), \
             mock.patch.object(place, "query_order_status", side_effect=fake_status), \
             mock.patch.object(place, "info_post", return_value=self.mids):
            res = place.execute_orders_maker_first(
                "0x" + "1" * 64, "0x" + "2" * 40, self.orders, self.mids, self.decimals, self.coin_of,
                timeout_sec=3.0, poll_sec=1.5, sleep_fn=clock.sleep, monotonic_fn=clock.monotonic
            )
        self.assertEqual(res["maker_filled"], 1)
        self.assertEqual(res["ioc_fallback"], 1)
        self.assertEqual([c["type"] for c in calls], ["order", "cancel", "order"])
        self.assertEqual(calls[1]["cancels"], [{"a": 0, "o": 201}])
        self.assertEqual(calls[2]["orders"][0]["s"], "0.00400")
        self.assertEqual(calls[2]["orders"][0]["t"], {"limit": {"tif": "Ioc"}})

    def test_network_error_queries_cloid_prevents_double_order(self):
        calls = []

        def fake_exchange(wallet, action, vault):
            calls.append(action)
            if len(calls) == 1:
                raise TimeoutError("read timed out")
            return {"status": "ok", "response": {"data": {"statuses": [{"filled": {"oid": 302}}]}}}

        # 第一笔 BTC 实际已成交（绝不补单），第二笔 ETH unknownOid（可安全补 IOC）
        status_seq = iter([
            {"status": "order", "order": {"status": "filled", "order": {"oid": 301, "sz": "0"}}},
            {"status": "unknownOid"},
        ])

        with mock.patch.object(place, "exchange_post", side_effect=fake_exchange), \
             mock.patch.object(place, "query_order_status", side_effect=lambda v, c: next(status_seq)), \
             mock.patch.object(place, "info_post", return_value=self.mids):
            res = place.execute_orders_maker_first(
                "0x" + "1" * 64, "0x" + "2" * 40, self.orders, self.mids, self.decimals, self.coin_of
            )
        self.assertEqual(res["maker_filled"], 1)
        self.assertEqual(res["ioc_fallback"], 1)
        self.assertEqual(len(calls), 2)
        self.assertEqual(len(calls[1]["orders"]), 1)
        self.assertEqual(calls[1]["orders"][0]["a"], 1)


class TestDriftBandAndCloseFirst(unittest.TestCase):
    def setUp(self):
        self.idx = {"OPEN": 0, "CLOSE": 1, "FLIP_LS": 2, "FLIP_SL": 3, "KEEP_SMALL": 4, "KEEP_BIG": 5, "REDUCE": 6}
        self.dec = {k: 3 for k in self.idx}
        self.mids = {k: "100" for k in self.idx}

    def test_drift_band_skips_small_same_side_but_allows_large_same_side(self):
        # target notional = $200 (2.0 coins @ $100)
        # KEEP_SMALL: cur=1.60 -> delta=+0.40 ($40), ratio=40/200=20% < 25% -> 跳过
        # KEEP_BIG:   cur=1.40 -> delta=+0.60 ($60), ratio=60/200=30% >= 25% -> 执行
        # REDUCE:     cur=-2.60 -> target=-2.00, delta=+0.60 ($60), ratio=30% >= 25% -> 执行且 reduce_only=True
        target = {"KEEP_SMALL": 0.20, "KEEP_BIG": 0.20, "REDUCE": -0.20}
        current = {"KEEP_SMALL": 1.60, "KEEP_BIG": 1.40, "REDUCE": -2.60}
        orders, metas = place.build_orders_with_meta(
            target, current, self.idx, self.dec, equity=1000.0, mids=self.mids, drift_band=0.25,
        )
        coins = [m["coin"] for m in metas]
        self.assertNotIn("KEEP_SMALL", coins)
        self.assertEqual(coins, ["REDUCE", "KEEP_BIG"])  # reduce 先于 increase
        self.assertTrue(orders[0]["r"])
        self.assertEqual(metas[0]["action"], "reduce")
        self.assertFalse(orders[1]["r"])
        self.assertEqual(metas[1]["action"], "increase")

    def test_drift_band_never_blocks_opens_closes_or_flips_and_sorts_close_first(self):
        # OPEN: 0 -> +2.0 (open, r=False)
        # CLOSE: +0.5 -> 0 (close, r=True)
        # FLIP_LS: +0.2 -> -2.0 (flip, r=False)
        # FLIP_SL: -0.2 -> +2.0 (flip, r=False)
        # REDUCE: +3.0 -> +2.0 (reduce, delta/target=50% >= 25%, r=True)
        # KEEP_SMALL: +1.8 -> +2.0 (ratio=10% < 25%, skipped)
        target = {
            "OPEN": 0.20,
            "FLIP_LS": -0.20,
            "FLIP_SL": 0.20,
            "REDUCE": 0.20,
            "KEEP_SMALL": 0.20,
        }
        current = {
            "CLOSE": 0.50,
            "FLIP_LS": 0.20,
            "FLIP_SL": -0.20,
            "REDUCE": 3.00,
            "KEEP_SMALL": 1.80,
        }
        orders, metas = place.build_orders_with_meta(
            target, current, self.idx, self.dec, equity=1000.0, mids=self.mids, drift_band=0.25,
        )
        actions = [(m["coin"], m["action"], o["r"], o["b"], float(o["s"])) for o, m in zip(orders, metas)]
        self.assertEqual(actions, [
            ("CLOSE", "close", True, False, 0.5),
            ("REDUCE", "reduce", True, False, 1.0),
            ("FLIP_LS", "flip", False, False, 2.2),
            ("FLIP_SL", "flip", False, True, 2.2),
            ("OPEN", "open", False, True, 2.0),
        ])
        # 所有 reduce_only=True 订单严格排在 reduce_only=False 之前
        ro_flags = [o["r"] for o in orders]
        self.assertEqual(ro_flags, [True, True, False, False, False])


if __name__ == "__main__":
    unittest.main()
