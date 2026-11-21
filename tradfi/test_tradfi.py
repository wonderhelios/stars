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
    exchange_leverage_for_gross,
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

    def test_s4_commodity_rsi_half_size_and_5d_trailing_exit(self) -> None:
        from tradfi.strategy import compute_comm_ema_weights

        # 1. Monotonic downtrend -> RSI14 == 0 <= 25 -> short should be halved (-0.5)
        down_px = [100.0 * (0.985 ** i) for i in range(30)]
        # 2. Downtrend that suddenly rebounds above prior 5d high -> 5d trailing exit -> flat (0.0)
        rebound_px = [100.0 * (0.985 ** i) for i in range(29)] + [100.0 * (0.985 ** 23) * 1.03]
        # 3. Healthy moderate uptrend -> RSI between 25 and 75 -> full long (+1.0)
        up_px = [100.0]
        for i in range(29):
            up_px.append(up_px[-1] * (1.015 if i % 2 == 0 else 0.992))

        bars_map = {
            "XAUUSDT": _make_bars(down_px),
            "CLUSDT": _make_bars(rebound_px),
            "NATGASUSDT": _make_bars(up_px),
        }
        cfg = StrategyConfig(gross_leverage=2.0, comm_universe=["XAUUSDT", "CLUSDT", "NATGASUSDT"])
        w, sigs = compute_comm_ema_weights(bars_map, cfg)
        self.assertEqual(sigs["XAUUSDT"], -0.5)
        self.assertEqual(sigs["CLUSDT"], 0.0)
        self.assertNotIn("CLUSDT", w)
        self.assertGreater(sigs["NATGASUSDT"], 0.0)
        self.assertLess(w["XAUUSDT"], 0.0)

    def test_fast_7d_onboarding_and_exchange_leverage_scaling(self) -> None:
        panel = self._sample_panel()
        # New listing with 7 bars (6 daily returns) -> should be scored via 7d fallback
        panel["ANTUSDT"] = _make_bars([100.0 * (1.02 ** i) for i in range(7)])
        # Very fresh listing with only 4 bars -> still skipped
        panel["IGVUSDT"] = _make_bars([100.0 * (1.03 ** i) for i in range(4)])
        cfg = StrategyConfig(
            stock_universe=[k for k in panel if k not in ("XAUUSDT", "XAGUSDT", "CLUSDT", "NATGASUSDT")],
        )
        self.assertEqual(cfg.gross_leverage, 3.0)
        self.assertEqual(cfg.stock_min_bars, 7)
        w, scores, _ = compute_stock_a4_slice(panel, cfg)
        self.assertIn("ANTUSDT", scores)
        self.assertNotIn("IGVUSDT", scores)
        self.assertGreater(w.get("ANTUSDT", 0.0), 0.0)

        # Exchange leverage ceiling scales with target gross leverage
        self.assertEqual(exchange_leverage_for_gross(2.0), 5)
        self.assertEqual(exchange_leverage_for_gross(3.0), 6)
        self.assertEqual(exchange_leverage_for_gross(5.0), 10)

    def test_tradfi_config_loading_and_shadow_sync(self) -> None:
        from tradfi.binance_exec import load_tradfi_config
        from tradfi.shadow import sync_shadow_config_from_file

        with tempfile.TemporaryDirectory() as td:
            cp = Path(td) / "tradfi_config.json"
            cp.write_text(json.dumps({
                "capital": 5000.0,
                "gross_leverage": 4.0,
                "stock_alloc": 0.45,
                "vbounce_alloc": 0.25,
                "stock_min_bars": 8,
                "api_key": "demo_key",
            }), encoding="utf-8")
            loaded = load_tradfi_config(cp)
            self.assertEqual(loaded["capital"], 5000.0)
            self.assertEqual(loaded["gross_leverage"], 4.0)

            ledger = TradFiShadowLedger(cfg=StrategyConfig(gross_leverage=3.0), capital=5000.0)
            self.assertEqual(ledger.state["capital"], 5000.0)
            self.assertEqual(ledger.state["equity"], 5000.0)
            changed = sync_shadow_config_from_file(ledger, config_path=cp)
            self.assertTrue(changed)
            self.assertEqual(ledger.cfg.gross_leverage, 4.0)
            self.assertAlmostEqual(ledger.cfg.stock_alloc, 0.45, places=4)
            self.assertAlmostEqual(ledger.cfg.vbounce_alloc, 0.25, places=4)
            self.assertAlmostEqual(ledger.cfg.comm_alloc, 0.30, places=4)
            self.assertEqual(ledger.cfg.stock_min_bars, 8)
            # Second sync with identical config should report no change
            self.assertFalse(sync_shadow_config_from_file(ledger, config_path=cp))

    def test_intraday_single_position_stop_and_dd_breaker(self) -> None:
        panel = self._sample_panel()
        cfg = StrategyConfig(
            gross_leverage=3.0,
            stock_universe=[k for k in panel if k not in ("XAUUSDT", "XAGUSDT", "CLUSDT", "NATGASUSDT")],
            comm_universe=["XAUUSDT", "XAGUSDT", "CLUSDT", "NATGASUSDT"],
        )
        target = compute_m3b_target(panel, cfg, day_index=10)
        prices = {s: bars[-1].close for s, bars in panel.items()}
        ledger = TradFiShadowLedger(cfg=cfg, capital=5000.0)
        now_ms = 1_763_600_000_000
        ledger.rebalance(target, prices, now_ms, fee_rate=0.0002)

        # Shock one long position by -16% -> single-pos intraday stop triggers
        long_sym = next(s for s, p in ledger.state["positions"].items() if p["side"] == "long")
        shocked = dict(prices)
        shocked[long_sym] = prices[long_sym] * 0.84
        risk1 = ledger.check_intraday_risk(shocked, now_ms + 300_000, fee_rate=0.0002)
        self.assertFalse(risk1["breaker_tripped"])
        self.assertIn(long_sym, risk1["stopped"])
        self.assertNotIn(long_sym, ledger.state["positions"])
        self.assertIn(long_sym, ledger.state["stop_cooldowns"])

        # Shock one short position in our favor (+16%) then pull back 6% -> trailing TP triggers
        short_sym = next(s for s, p in ledger.state["positions"].items() if p["side"] == "short")
        fav_px = dict(shocked)
        fav_px[short_sym] = prices[short_sym] * 0.84  # +16% favorable for short
        risk_peak = ledger.check_intraday_risk(fav_px, now_ms + 400_000, fee_rate=0.0002)
        self.assertEqual(risk_peak["taken"], [])
        self.assertAlmostEqual(ledger.state["positions"][short_sym]["peak_fav_pct"], 16.0, places=2)

        pullback_px = dict(fav_px)
        pullback_px[short_sym] = prices[short_sym] * 0.90  # +10% remaining -> 6% callback >= 5%
        risk_tp = ledger.check_intraday_risk(pullback_px, now_ms + 500_000, fee_rate=0.0002)
        self.assertIn(short_sym, risk_tp["taken"])
        self.assertNotIn(short_sym, ledger.state["positions"])

        # Shock all remaining positions adversarially to trigger 20% account DD breaker
        crash = dict(pullback_px)
        for sym, pos in ledger.state["positions"].items():
            crash[sym] = pos["entry_price"] * (0.70 if pos["side"] == "long" else 1.30)
        risk2 = ledger.check_intraday_risk(crash, now_ms + 600_000, fee_rate=0.0002)
        self.assertTrue(risk2["breaker_tripped"])
        self.assertTrue(ledger.state["breaker_tripped"])
        self.assertEqual(len(ledger.state["positions"]), 0)
        actions = [t["action"] for t in ledger.state["trades"]]
        self.assertIn("盘中止损", actions)
        self.assertIn("盘中移动止盈", actions)
        self.assertIn("熔断平仓", actions)

    def test_vbounce_oversold_entry_exit_and_three_leg_blend(self) -> None:
        from tradfi.strategy import compute_vbounce_weights

        # Day 0: NVDA rises from 70 -> 100 over 57 days (MA60 ~85), then crashes 3 days (97, 94, 91 >= MA60)
        uptrend_prefix = [70.0 + i * 0.55 for i in range(57)]
        crash_px = uptrend_prefix + [97.0, 94.0, 91.0]
        bars_d0 = {"NVDAUSDT": _make_bars(crash_px)}
        cfg = StrategyConfig(gross_leverage=2.0, stock_universe=["NVDAUSDT"], comm_universe=[])
        self.assertAlmostEqual(cfg.stock_alloc, 0.40, places=4)
        self.assertAlmostEqual(cfg.comm_alloc, 0.40, places=4)
        self.assertAlmostEqual(cfg.vbounce_alloc, 0.20, places=4)
        self.assertEqual(cfg.vbounce_trend_ma, 60)

        w0, pos0 = compute_vbounce_weights(bars_d0, cfg, prev_vbounce_positions=None)
        self.assertIn("NVDAUSDT", w0)
        self.assertAlmostEqual(w0["NVDAUSDT"], 2.0 / 4.0, places=4)
        self.assertEqual(pos0["NVDAUSDT"]["hold_days"], 0)
        self.assertAlmostEqual(pos0["NVDAUSDT"]["entry_price"], 91.0, places=4)

        # Bear-market falling knife (below MA60) must be rejected by MA60 filter
        bear_px = [140.0 - i * 0.6 for i in range(57)] + [97.0, 94.0, 91.0]
        w_bear, pos_bear = compute_vbounce_weights({"NVDAUSDT": _make_bars(bear_px)}, cfg, prev_vbounce_positions=None)
        self.assertEqual(w_bear, {})
        self.assertEqual(pos_bear, {})

        # Day 1: small bounce to 92.0 (still below MA5 ~94.8, hold_days=1 < 3) -> keep holding
        bars_d1 = {"NVDAUSDT": _make_bars(crash_px + [92.0])}
        w1, pos1 = compute_vbounce_weights(bars_d1, cfg, prev_vbounce_positions=pos0)
        self.assertIn("NVDAUSDT", w1)
        self.assertEqual(pos1["NVDAUSDT"]["hold_days"], 1)

        # Day 2: sharp rebound above MA5 (99.0 >= MA5) -> exit cleanly
        bars_d2 = {"NVDAUSDT": _make_bars(crash_px + [92.0, 99.0])}
        w2, pos2 = compute_vbounce_weights(bars_d2, cfg, prev_vbounce_positions=pos1)
        self.assertNotIn("NVDAUSDT", w2)
        self.assertEqual(pos2, {})

        # Stop-loss exit: -9% drop from entry (91 -> 82.5)
        bars_stop = {"NVDAUSDT": _make_bars(crash_px + [82.5])}
        w_stop, pos_stop = compute_vbounce_weights(bars_stop, cfg, prev_vbounce_positions=pos0)
        self.assertNotIn("NVDAUSDT", w_stop)
        self.assertEqual(pos_stop, {})

        # Max hold days (3) exit
        pos_old = {"NVDAUSDT": {"entry_price": 91.0, "hold_days": 2}}
        w_exp, pos_exp = compute_vbounce_weights(bars_d1, cfg, prev_vbounce_positions=pos_old)
        self.assertNotIn("NVDAUSDT", w_exp)
        self.assertEqual(pos_exp, {})

    def test_comm_4h_dual_timeframe_and_freeze_daily_state(self) -> None:
        from tradfi.strategy import compute_comm_4h_weights

        panel = self._sample_panel()
        # Construct 60 4h bars where XAUUSDT flips strongly UP on 4h while daily is still DOWN
        up_4h = [100.0]
        for i in range(59):
            up_4h.append(up_4h[-1] * (1.006 if i % 2 == 0 else 0.997))
        dn_4h = [100.0]
        for i in range(59):
            dn_4h.append(dn_4h[-1] * (0.994 if i % 2 == 0 else 1.002))
        bars_4h = {
            "XAUUSDT": _make_bars(up_4h, vol=10_000_000.0),
            "CLUSDT": _make_bars(dn_4h, vol=10_000_000.0),
            "NATGASUSDT": _make_bars(up_4h, vol=10_000_000.0),
            "XAGUSDT": _make_bars(dn_4h, vol=10_000_000.0),
        }
        cfg = StrategyConfig(
            gross_leverage=2.0,
            stock_universe=[k for k in panel if k not in ("XAUUSDT", "XAGUSDT", "CLUSDT", "NATGASUSDT")],
            comm_universe=["XAUUSDT", "XAGUSDT", "CLUSDT", "NATGASUSDT"],
            comm_4h_frac=0.50,
        )
        w4, sig4 = compute_comm_4h_weights(bars_4h, cfg)
        self.assertGreater(sig4["XAUUSDT"], 0.0)
        self.assertLess(sig4["CLUSDT"], 0.0)
        self.assertGreater(w4["XAUUSDT"], 0.0)

        t_daily = compute_m3b_target(panel, cfg, day_index=10, bars_4h_map=None)
        t_dual = compute_m3b_target(panel, cfg, day_index=10, bars_4h_map=bars_4h)
        # XAU daily is short, 4h is long -> blended weight should be less short (higher) than pure daily
        self.assertGreater(t_dual.weights.get("XAUUSDT", 0.0), t_daily.weights.get("XAUUSDT", 0.0))

        # Intraday 4h refresh with freeze_daily_state=True must not mutate stock_phases or vbounce hold_days
        t_4h_only = compute_m3b_target(
            panel,
            cfg,
            prev_stock_phases=t_dual.stock_phase_weights,
            day_index=11,
            prev_vbounce_positions=t_dual.vbounce_positions,
            bars_4h_map=bars_4h,
            freeze_daily_state=True,
        )
        self.assertEqual(t_4h_only.stock_phase_weights, t_dual.stock_phase_weights)
        self.assertEqual(t_4h_only.vbounce_positions, t_dual.vbounce_positions)

    def test_cash_sweep_and_portfolio_vol_target(self) -> None:
        panel = self._sample_panel()
        stocks = [k for k in panel if k not in ("XAUUSDT", "XAGUSDT", "CLUSDT", "NATGASUSDT")]
        comms = ["XAUUSDT", "XAGUSDT", "CLUSDT", "NATGASUSDT"]

        # Without cash sweep vs with cash sweep (no vbounce slots active -> 40/40 sweeps to 50/50)
        cfg_no_sweep = StrategyConfig(
            gross_leverage=2.0,
            stock_universe=stocks,
            comm_universe=comms,
            vbounce_cash_sweep=False,
            vol_target_ann=0.0,
        )
        cfg_sweep = StrategyConfig(
            gross_leverage=2.0,
            stock_universe=stocks,
            comm_universe=comms,
            vbounce_cash_sweep=True,
            vol_target_ann=0.0,
        )
        t_no = compute_m3b_target(panel, cfg_no_sweep, day_index=10)
        t_sw = compute_m3b_target(panel, cfg_sweep, day_index=10)
        gross_no = sum(abs(v) for v in t_no.weights.values())
        gross_sw = sum(abs(v) for v in t_sw.weights.values())
        self.assertGreater(gross_sw, gross_no * 1.15)

        # Strict vol target (e.g. 5% ann vol) must scale down gross weights on a volatile panel
        vol_panel = dict(panel)
        for s in comms:
            px = 100.0
            arr = []
            for i in range(30):
                px *= (1.035 if i % 2 == 0 else 0.968)
                arr.append(px)
            vol_panel[s] = _make_bars(arr)
        t_sw_v = compute_m3b_target(vol_panel, cfg_sweep, day_index=10)
        cfg_vt = StrategyConfig(
            gross_leverage=2.0,
            stock_universe=stocks,
            comm_universe=comms,
            vbounce_cash_sweep=True,
            vol_target_ann=0.05,
        )
        t_vt = compute_m3b_target(vol_panel, cfg_vt, day_index=10)
        gross_sw_v = sum(abs(v) for v in t_sw_v.weights.values())
        gross_vt = sum(abs(v) for v in t_vt.weights.values())
        self.assertLess(gross_vt, gross_sw_v * 0.90)


if __name__ == "__main__":
    unittest.main()
