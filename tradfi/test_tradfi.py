"""Unit tests for tradfi (M3b 4-Core Dual-Leg strategy, shadow ledger, Binance executor)."""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from tradfi.binance_exec import (
    SymbolFilters,
    build_rebalance_orders,
    check_dd_breaker,
    format_decimal,
    round_step,
    round_tick,
)
from tradfi.shadow import TradFiShadowLedger
from tradfi.strategy import (
    DailyBar,
    StrategyConfig,
    compute_m3b_target,
    compute_stock_a4_slice,
)


def _make_bars(prices: list[float], vol: float = 20_000_000.0, start_ms: int = 1_760_000_000_000) -> list[DailyBar]:
    return [DailyBar(open_time_ms=start_ms + i * 86_400_000, close=p, quote_vol=vol) for i, p in enumerate(prices)]


class TestTradFiStrategy(unittest.TestCase):
    def _sample_panel(self) -> dict[str, list[DailyBar]]:
        # 10 synthetic 1x stocks with distinct trends + 4 commodities
        panel: dict[str, list[DailyBar]] = {}
        trends = {
            "AAPLUSDT": 0.012,
            "AVGOUSDT": 0.015,
            "AMZNUSDT": 0.008,
            "TSLAUSDT": 0.005,
            "MSFTUSDT": 0.001,
            "GOOGLUSDT": -0.002,
            "METAUSDT": -0.006,
            "NVDAUSDT": -0.012,
            "COINUSDT": -0.018,
            "MSTRUSDT": -0.022,
        }
        for s, drift in trends.items():
            px = 100.0
            arr = []
            for i in range(30):
                # slight alternating wobble so vol > 0
                wob = 0.004 if i % 2 == 0 else -0.003
                px *= (1.0 + drift + wob)
                arr.append(px)
            panel[s] = _make_bars(arr)

        # Commodities: NATGAS uptrend, XAU/XAG/CL downtrend
        for s, drift in [("NATGASUSDT", 0.015), ("XAUUSDT", -0.008), ("XAGUSDT", -0.014), ("CLUSDT", -0.010)]:
            px = 100.0
            arr = []
            for i in range(30):
                wob = 0.003 if i % 2 == 0 else -0.002
                px *= (1.0 + drift + wob)
                arr.append(px)
            panel[s] = _make_bars(arr)
        return panel

    def test_corporate_action_guard_blocks_split_stock(self) -> None:
        panel = self._sample_panel()
        # Inject an 8-for-1 split (-88% in one day) on SNDKUSDT
        pxs = [800.0 * (0.99 ** i) for i in range(29)] + [89.0]
        panel["SNDKUSDT"] = _make_bars(pxs)
        cfg = StrategyConfig(
            gross_leverage=2.0,
            stock_universe=list(panel.keys()),
        )
        w, scores, blocked = compute_stock_a4_slice(panel, cfg)
        self.assertIn("SNDKUSDT", blocked)
        self.assertNotIn("SNDKUSDT", w)
        self.assertNotIn("SNDKUSDT", scores)

    def test_stock_leg_market_neutral_and_capped(self) -> None:
        panel = self._sample_panel()
        cfg = StrategyConfig(gross_leverage=2.0, stock_universe=[k for k in panel if k not in ("XAUUSDT", "XAGUSDT", "CLUSDT", "NATGASUSDT")])
        w, _, blocked = compute_stock_a4_slice(panel, cfg)
        self.assertEqual(blocked, [])
        pos_sum = sum(v for v in w.values() if v > 0)
        neg_sum = sum(-v for v in w.values() if v < 0)
        self.assertAlmostEqual(pos_sum, 1.0, places=3)
        self.assertAlmostEqual(neg_sum, 1.0, places=3)
        for v in w.values():
            self.assertLessEqual(abs(v), 0.30 * 2.0 + 1e-6)

    def test_m3b_dual_leg_and_two_phase_slicing(self) -> None:
        panel = self._sample_panel()
        cfg = StrategyConfig(
            gross_leverage=2.0,
            stock_universe=[k for k in panel if k not in ("XAUUSDT", "XAGUSDT", "CLUSDT", "NATGASUSDT")],
            comm_universe=["XAUUSDT", "XAGUSDT", "CLUSDT", "NATGASUSDT"],
        )
        t0 = compute_m3b_target(panel, cfg, prev_stock_phases=None, day_index=10)
        # NATGAS long, XAU/XAG/CL short
        self.assertGreater(t0.weights["NATGASUSDT"], 0.0)
        self.assertLess(t0.weights["XAUUSDT"], 0.0)
        self.assertLess(t0.weights["CLUSDT"], 0.0)
        self.assertEqual(t0.legs["NATGASUSDT"], "comm_ema")
        # Both phases initialized on cold start
        self.assertEqual(t0.stock_phase_weights["p0"], t0.stock_phase_weights["p1"])

        # On next day (odd index 11), only p1 should update while p0 stays frozen
        t1 = compute_m3b_target(panel, cfg, prev_stock_phases=t0.stock_phase_weights, day_index=11)
        self.assertEqual(t1.stock_phase_weights["p0"], t0.stock_phase_weights["p0"])

    def test_shadow_ledger_rebalance_funding_and_persistence(self) -> None:
        panel = self._sample_panel()
        cfg = StrategyConfig(
            gross_leverage=2.0,
            stock_universe=[k for k in panel if k not in ("XAUUSDT", "XAGUSDT", "CLUSDT", "NATGASUSDT")],
            comm_universe=["XAUUSDT", "XAGUSDT", "CLUSDT", "NATGASUSDT"],
        )
        target = compute_m3b_target(panel, cfg, day_index=10)
        prices = {s: bars[-1].close for s, bars in panel.items()}
        ledger = TradFiShadowLedger(cfg=cfg)
        now_ms = 1_763_600_000_000
        rec = ledger.rebalance(target, prices, now_ms, fee_rate=0.0002)
        self.assertGreater(rec["turnover_usd"], 0.0)
        self.assertGreater(rec["fees_usd"], 0.0)
        self.assertEqual(len(ledger.state["positions"]), len(target.weights))

        # Apply positive funding rate on short XAUUSDT -> short receives funding
        before_cash = ledger.state["cash"]
        delta = ledger.apply_funding_events({"XAUUSDT": [(now_ms + 8 * 3600 * 1000, 0.0005)]}, prices)
        self.assertGreater(delta, 0.0)
        self.assertAlmostEqual(ledger.state["cash"], before_cash + delta, places=6)

        with tempfile.TemporaryDirectory() as td:
            p = Path(td) / "tradfi_shadow.json"
            ledger.save(p)
            reloaded = TradFiShadowLedger.load(p, cfg=cfg)
            self.assertEqual(len(reloaded.state["positions"]), len(ledger.state["positions"]))
            self.assertAlmostEqual(reloaded.state["equity"], ledger.state["equity"], places=4)

    def test_binance_exec_rounding_orders_and_dd_breaker(self) -> None:
        self.assertEqual(format_decimal(round_step(1.2349, 0.01), 0.01), "1.23")
        self.assertEqual(round_tick(100.123, 0.05, "BUY"), 100.10)
        self.assertEqual(round_tick(100.123, 0.05, "SELL"), 100.15)

        panel = self._sample_panel()
        cfg = StrategyConfig(
            gross_leverage=2.0,
            stock_universe=[k for k in panel if k not in ("XAUUSDT", "XAGUSDT", "CLUSDT", "NATGASUSDT")],
            comm_universe=["XAUUSDT", "XAGUSDT", "CLUSDT", "NATGASUSDT"],
        )
        target = compute_m3b_target(panel, cfg, day_index=10)
        prices = {s: bars[-1].close for s, bars in panel.items()}
        filters = {s: SymbolFilters(s, 0.001, 0.001, 0.01, 5.0) for s in prices}
        # Existing position in an out-of-target symbol should be closed first (reduce_only=True)
        orders = build_rebalance_orders(2000.0, {"GOOGLUSDT": 5.0}, target, prices, filters)
        self.assertTrue(orders[0].reduce_only)
        self.assertEqual(orders[0].symbol, "GOOGLUSDT")

        # Hard drawdown breaker test
        with tempfile.TemporaryDirectory() as td:
            sp = Path(td) / "live_state.json"
            tripped, peak, dd = check_dd_breaker(2000.0, sp, 15.0)
            self.assertFalse(tripped)
            self.assertEqual(peak, 2000.0)
            tripped2, peak2, dd2 = check_dd_breaker(1680.0, sp, 15.0)
            self.assertTrue(tripped2)
            self.assertEqual(peak2, 2000.0)
            self.assertAlmostEqual(dd2, 16.0, places=2)


if __name__ == "__main__":
    unittest.main()
