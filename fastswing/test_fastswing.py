"""Unit tests for fastswing.strategy and fastswing.shadow."""
from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from fastswing.shadow import FastSwingShadowLedger, HOUR_MS
from fastswing.strategy import (
    Bar1h,
    FastSwingConfig,
    compute_indicators,
    detect_sfp_signal,
)


def _make_uptrend_bars(n: int = 200, start_px: float = 100.0, step: float = 0.2) -> list[Bar1h]:
    bars: list[Bar1h] = []
    t0 = 1_700_000_000_000
    for i in range(n):
        c = start_px + i * step
        bars.append(Bar1h(t0 + i * HOUR_MS, c - 0.1, c + 0.5, c - 0.5, c, 1000.0))
    return bars


class TestFastSwingStrategy(unittest.TestCase):
    def test_long_sfp_detected_in_uptrend_and_blocked_in_downtrend(self) -> None:
        cfg = FastSwingConfig(lookback_hours=24, atr_period=24, ema_period=168, wick_atr_min=0.5)
        bars = _make_uptrend_bars(200, 100.0, 0.2)
        prev_low = min(b.low for b in bars[-24:])
        # Add a textbook bullish SFP sweeping below prev_low and closing back above
        last = bars[-1]
        sfp_bar = Bar1h(
            open_time_ms=last.open_time_ms + HOUR_MS,
            open=last.close,
            high=last.close + 0.3,
            low=prev_low - 3.0,
            close=prev_low + 0.6,
            volume=5000.0,
        )
        bars.append(sfp_bar)
        sig, reason = detect_sfp_signal("SOXLUSDT", bars, cfg)
        self.assertIsNotNone(sig, f"Expected long SFP, got None: {reason}")
        assert sig is not None
        self.assertEqual(sig.side, 1)
        self.assertEqual(len(sig.levels), 3)
        self.assertGreater(sig.levels[0], sig.levels[1])
        self.assertGreater(sig.levels[1], sig.levels[2])
        self.assertLess(sig.sl_price, sig.levels[2])

        # Now test same bullish SFP in a downtrend (close < EMA168) -> must be rejected
        down_bars = _make_uptrend_bars(200, 200.0, -0.3)
        d_prev_low = min(b.low for b in down_bars[-24:])
        d_last = down_bars[-1]
        down_bars.append(
            Bar1h(
                open_time_ms=d_last.open_time_ms + HOUR_MS,
                open=d_last.close,
                high=d_last.close + 0.2,
                low=d_prev_low - 3.0,
                close=d_prev_low + 0.5,
                volume=5000.0,
            )
        )
        sig2, reason2 = detect_sfp_signal("SOXLUSDT", down_bars, cfg)
        self.assertIsNone(sig2)
        self.assertIn("逆EMA", reason2)

    def test_split_guard_blocks_signal(self) -> None:
        cfg = FastSwingConfig(lookback_hours=24, atr_period=24, ema_period=168, split_ret_threshold=0.40)
        bars = _make_uptrend_bars(200, 100.0, 0.2)
        # Inject an 8:1 split bar 5 hours ago
        sp = bars[-5]
        bars[-5] = Bar1h(sp.open_time_ms, sp.open, sp.high, sp.low * 0.125, sp.close * 0.125, sp.volume)
        sig, reason = detect_sfp_signal("SNDKUSDT", bars, cfg)
        self.assertIsNone(sig)
        self.assertIn("拆股", reason)


class TestFastSwingShadowLedger(unittest.TestCase):
    def test_scaled_limit_fill_75_25_tp_and_breakeven_trail(self) -> None:
        cfg = FastSwingConfig(slot_leverage=2.0, max_gross_leverage=6.0)
        ledger = FastSwingShadowLedger(cfg=cfg, capital=5000.0)
        bars = _make_uptrend_bars(200, 100.0, 0.2)
        prev_low = min(b.low for b in bars[-24:])
        last = bars[-1]
        sfp_bar = Bar1h(last.open_time_ms + HOUR_MS, last.close, last.close + 0.3, prev_low - 3.0, prev_low + 0.6, 5000.0)
        bars.append(sfp_bar)

        sig, _ = detect_sfp_signal("SOXLUSDT", bars, cfg)
        assert sig is not None
        now = sfp_bar.open_time_ms + HOUR_MS
        ok = ledger.enqueue_signal(sig, now, {"SOXLUSDT": sig.close})
        self.assertTrue(ok)
        self.assertIn("SOXLUSDT", ledger.state["pending"])
        self.assertAlmostEqual(ledger.state["pending"]["SOXLUSDT"]["max_notional"], 10000.0, places=2)

        # Pullback fills tier 1 & tier 2 (1/6 + 2/6 = 50% -> $5,000 filled)
        t1_px = sig.levels[1] - 0.01
        ledger.process_pending_and_positions({"SOXLUSDT": t1_px}, {}, {"SOXLUSDT": sig.atr}, now + 60_000)
        self.assertIn("SOXLUSDT", ledger.state["pending"])
        self.assertAlmostEqual(ledger.state["pending"]["SOXLUSDT"]["filled_notional"], 5000.0, places=1)

        # Price bounces to TP1 -> promotes to active position and immediately takes 75% profit
        avg_e = ledger.state["pending"]["SOXLUSDT"]["filled_cost"] / ledger.state["pending"]["SOXLUSDT"]["filled_qty"]
        tp1_target = avg_e + cfg.tp1_atr * sig.atr + 0.1
        ledger.process_pending_and_positions({"SOXLUSDT": tp1_target}, {}, {"SOXLUSDT": sig.atr}, now + 120_000)
        self.assertNotIn("SOXLUSDT", ledger.state["pending"])
        self.assertIn("SOXLUSDT", ledger.state["positions"])
        pos = ledger.state["positions"]["SOXLUSDT"]
        self.assertTrue(pos["tp1_hit"])
        self.assertGreater(pos["realized_usd"], 0.0)
        # Runner SL moved above breakeven
        self.assertGreater(pos["cur_sl"], avg_e)
        # Remaining qty is 25% of initial
        self.assertAlmostEqual(pos["qty"] / pos["init_qty"], 0.25, places=4)

        # Price falls back to breakeven SL -> closes remaining 25% cleanly
        ledger.process_pending_and_positions({"SOXLUSDT": avg_e - 0.05}, {}, {"SOXLUSDT": sig.atr}, now + 180_000)
        self.assertNotIn("SOXLUSDT", ledger.state["positions"])
        self.assertEqual(len(ledger.state["trades"]), 1)
        self.assertGreater(ledger.state["trades"][0]["net_pnl"], 0.0)
        self.assertEqual(ledger.state["trades"][0]["reason"], "保本/移动止盈")

    def test_hard_stop_and_atomic_save_load(self) -> None:
        cfg = FastSwingConfig(slot_leverage=2.0, max_gross_leverage=6.0)
        ledger = FastSwingShadowLedger(cfg=cfg, capital=5000.0)
        bars = _make_uptrend_bars(200, 100.0, 0.2)
        prev_low = min(b.low for b in bars[-24:])
        last = bars[-1]
        sfp_bar = Bar1h(last.open_time_ms + HOUR_MS, last.close, last.close + 0.3, prev_low - 3.0, prev_low + 0.6, 5000.0)
        bars.append(sfp_bar)
        sig, _ = detect_sfp_signal("NVDAUSDT", bars, cfg)
        assert sig is not None
        now = sfp_bar.open_time_ms + HOUR_MS
        ledger.enqueue_signal(sig, now, {"NVDAUSDT": sig.close})

        # Fill all 3 tiers cleanly without hitting SL
        ledger.process_pending_and_positions({"NVDAUSDT": sig.levels[2] - 0.01}, {}, {"NVDAUSDT": sig.atr}, now + 60_000)
        self.assertIn("NVDAUSDT", ledger.state["positions"])
        self.assertEqual(ledger.state["positions"]["NVDAUSDT"]["filled_tiers"], 3)

        # Funding settlement (long pays positive rate)
        ledger.apply_funding_events({"NVDAUSDT": [(now + 120_000, 0.0005)]}, {"NVDAUSDT": sig.levels[2]})
        self.assertLess(ledger.state["total_funding"], 0.0)

        # Crash past SL -> hard stop triggers
        ledger.process_pending_and_positions({"NVDAUSDT": sig.sl_price - 1.0}, {}, {"NVDAUSDT": sig.atr}, now + 180_000)
        self.assertNotIn("NVDAUSDT", ledger.state["positions"])
        self.assertEqual(len(ledger.state["trades"]), 1)
        self.assertLess(ledger.state["trades"][0]["net_pnl"], 0.0)
        self.assertEqual(ledger.state["trades"][0]["reason"], "硬止损")

        # Save & reload round-trip
        with tempfile.TemporaryDirectory() as td:
            p = Path(td) / "fastswing.json"
            ledger.save(p)
            reloaded = FastSwingShadowLedger.load(p, cfg=cfg)
            self.assertEqual(len(reloaded.state["trades"]), 1)
            self.assertAlmostEqual(reloaded.state["equity"], ledger.state["equity"], places=4)


if __name__ == "__main__":
    unittest.main()
