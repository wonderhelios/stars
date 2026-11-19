import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import shadow
from shadow import (
    COOLDOWN_HOURS,
    FEE_MAKER,
    FEE_TAKER,
    HOUR_MS,
    MAX_HOLDING_HOURS,
    POSITION_FRACTION,
    STOP_LOSS_PCT,
    initial_state,
    process_entries,
    process_exits,
    scan_candidates,
    settle_hourly_funding,
    snapshot,
)


def cand(symbol, fr_ann=-60.0, ret24=0.15, entry=100.0, vol24=5_000_000.0):
    return {
        "symbol": symbol,
        "side": -1,
        "entry": entry,
        "ret24": ret24,
        "vol24": vol24,
        "fr_1h": fr_ann / (24.0 * 365.0 * 100.0),
        "fr_ann": fr_ann,
    }


class A1wShadowTests(unittest.TestCase):
    def test_entries_max_four_15pct_slot_and_idempotent(self):
        s = initial_state()
        s["equity"] = 2500.0
        s["balance"] = 2500.0
        t0 = 1_760_000_000_000 // HOUR_MS * HOUR_MS
        cs = [cand(f"C{i}", fr_ann=-100.0 + i * 5) for i in range(6)]
        process_entries(s, t0, cs)
        self.assertEqual(len(s["positions"]), 4)
        self.assertEqual(s["last_signal_time"], t0)
        self.assertTrue(all(p["side"] == -1 for p in s["positions"]))
        self.assertTrue(all(abs(p["notional"] - 2500.0 * POSITION_FRACTION) < 1e-6 for p in s["positions"]))
        self.assertAlmostEqual(s["total_cost"], 4 * 2500.0 * POSITION_FRACTION * FEE_MAKER)

        bal = s["balance"]
        process_entries(s, t0, cs)
        self.assertEqual(len(s["positions"]), 4)
        self.assertEqual(s["balance"], bal)

    def test_funding_threshold_filters_out_non_negative_funding(self):
        s = initial_state()
        t0 = 1_760_000_000_000 // HOUR_MS * HOUR_MS
        process_entries(s, t0, [cand("BTC", fr_ann=-10.0), cand("ETH", fr_ann=20.0)])
        self.assertEqual(len(s["positions"]), 0)
        self.assertTrue(all("费率未达" in sig["reason"] for sig in s["signals"]))

    def test_wide_15pct_stop_loss_triggers_taker_fee_and_24h_cooldown(self):
        s = initial_state()
        t0 = 1_760_000_000_000 // HOUR_MS * HOUR_MS
        process_entries(s, t0, [cand("INIT", fr_ann=-160.0, entry=100.0)])
        p = s["positions"][0]
        self.assertAlmostEqual(p["stop_price"], 100.0 * (1.0 + STOP_LOSS_PCT))

        # 价格上涨 10% 不触发止损
        process_exits(s, {"INIT": 110.0}, now=t0 + 2 * HOUR_MS)
        self.assertEqual(len(s["positions"]), 1)

        # 价格上涨 15% 触发宽止损
        sl_time = t0 + 3 * HOUR_MS
        process_exits(s, {"INIT": 115.0}, now=sl_time)
        snapshot(s, {})
        self.assertEqual(len(s["positions"]), 0)
        self.assertEqual(len(s["trades"]), 1)
        tr = s["trades"][0]
        self.assertEqual(tr["reason"], "15%宽止损")
        expected_pnl = p["notional"] * (-0.15) - p["notional"] * (FEE_MAKER + FEE_TAKER)
        self.assertAlmostEqual(tr["pnl_usd"], expected_pnl, places=6)
        self.assertAlmostEqual(s["realized_pnl"], expected_pnl, places=6)
        self.assertAlmostEqual(s["equity"], s["capital"] + expected_pnl, places=6)

        # 24h 冷却期内拒绝同币重入
        t_blocked = sl_time + 10 * HOUR_MS
        process_entries(s, t_blocked, [cand("INIT", fr_ann=-300.0, entry=120.0)])
        self.assertEqual(len(s["positions"]), 0)
        self.assertIn("冷却", s["signals"][-1]["reason"])

        # 满 24h 冷却后允许重入
        t_ok = sl_time + COOLDOWN_HOURS * HOUR_MS
        process_entries(s, t_ok, [cand("INIT", fr_ann=-300.0, entry=130.0)])
        self.assertEqual(len(s["positions"]), 1)

    def test_8h_time_exit_and_hourly_funding_accounting_exact_identity(self):
        s = initial_state()
        t0 = 1_760_000_000_000 // HOUR_MS * HOUR_MS
        process_entries(s, t0, [cand("BERA", fr_ann=-200.0, entry=100.0)])
        notional = s["positions"][0]["notional"]

        # 模拟 2 个整点结算，每小时资金费率 -0.001（空单付钱）
        def fake_fh(coin, start_ms, end_ms=None):
            return [
                {"coin": coin, "time": t0 + HOUR_MS + 50, "fundingRate": "-0.001"},
                {"coin": coin, "time": t0 + 2 * HOUR_MS + 50, "fundingRate": "-0.0015"},
            ]

        settle_hourly_funding(s, t0 + 2 * HOUR_MS, fetch_fn=fake_fh)
        # 重复调用同一小时必须幂等，不重复扣费
        settle_hourly_funding(s, t0 + 2 * HOUR_MS, fetch_fn=fake_fh)
        expected_funding = notional * (-0.0025)
        self.assertAlmostEqual(s["total_funding"], expected_funding, places=6)
        self.assertAlmostEqual(s["positions"][0]["funding_usd"], expected_funding, places=6)

        # 8h 到时平仓，价格跌至 90（做空毛赚 +10%）
        process_exits(s, {"BERA": 90.0}, now=t0 + MAX_HOLDING_HOURS * HOUR_MS)
        snapshot(s, {})
        self.assertEqual(len(s["positions"]), 0)
        tr = s["trades"][0]
        self.assertEqual(tr["reason"], "8h到时平仓")
        expected_total_net = notional * 0.10 + expected_funding - notional * (2 * FEE_MAKER)
        self.assertAlmostEqual(tr["pnl_usd"], expected_total_net, places=6)
        self.assertAlmostEqual(s["realized_pnl"], expected_total_net, places=6)
        self.assertAlmostEqual(s["equity"], s["capital"] + expected_total_net, places=6)
        self.assertIn("BERA", s["cooldowns"])

    def test_scan_candidates_filters_liquidity_ret24_and_sorts_by_funding(self):
        t0 = 1_760_000_000_000 // HOUR_MS * HOUR_MS
        ctxs = {f"FILL{i}": {"mark": 10.0, "prev_day": 10.0, "vol24": 5_000_000.0, "funding": 0.0001} for i in range(55)}
        ctxs.update(
            {
                "ILLIQ": {"mark": 12.0, "prev_day": 10.0, "vol24": 500_000.0, "funding": -0.001},
                "SMALL_UP": {"mark": 10.5, "prev_day": 10.0, "vol24": 5_000_000.0, "funding": -0.001},
                "COIN_A": {"mark": 11.2, "prev_day": 10.0, "vol24": 4_000_000.0, "funding": -0.0001},
                "COIN_B": {"mark": 12.0, "prev_day": 10.0, "vol24": 8_000_000.0, "funding": -0.0005},
            }
        )

        def fake_fh(coin, start_ms, end_ms=None):
            rates = {"COIN_A": "-0.0001", "COIN_B": "-0.0005"}
            return [{"coin": coin, "time": t0 - HOUR_MS + 10, "fundingRate": rates[coin]}]

        cands, warns = scan_candidates(t0, ctxs, fetch_fn=fake_fh)
        self.assertEqual(warns, [])
        self.assertEqual([c["symbol"] for c in cands], ["COIN_B", "COIN_A"])
        self.assertLess(cands[0]["fr_ann"], cands[1]["fr_ann"])

    @patch("shadow.scan_candidates")
    @patch("shadow.fetch_meta_and_ctxs")
    def test_late_restart_never_backfills_an_hour_old_entry(self, mock_ctxs, mock_scan):
        mock_ctxs.return_value = {"BTC": {"mark": 65000.0, "prev_day": 64000.0, "vol24": 1e9, "funding": 0.00001}}
        with tempfile.TemporaryDirectory() as tmp, patch.object(
            shadow, "STATE_PATH", Path(tmp) / "state.json"
        ), patch.object(shadow, "now_ms", return_value=4_200_000):
            shadow.run_once()
            state = shadow.load_state()
        mock_scan.assert_not_called()
        self.assertEqual(state["last_signal_time"], HOUR_MS)
        self.assertEqual(state["positions"], [])


if __name__ == "__main__":
    unittest.main()
