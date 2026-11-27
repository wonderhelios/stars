"""ml/daily.py 的 UTC 日级幂等锁与季度模型更新检查单元测试（全 mock，无网络调用）。"""
from __future__ import annotations

import argparse
import datetime
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ML = Path(__file__).resolve().parent
if str(ML) not in sys.path:
    sys.path.insert(0, str(ML))

import daily
from strategy import load_config, ordered_feature_names


def _make_args(lock_path, dry_run=False, force=False, vault="0x" + "a" * 40):
    return argparse.Namespace(
        dry_run=dry_run,
        force=force,
        vault=vault,
        lock_file=str(lock_path),
    )


class DailyIdempotencyLockTests(unittest.TestCase):
    def setUp(self):
        # 2026-11-27 距离 REBALANCE_START (2026-05-04) 为 207 天，207 % 3 == 0（调仓日）
        self.rebal_day = datetime.date(2026, 11, 27)
        self.next_day = datetime.date(2026, 11, 28)  # 非调仓日
        self.next_rebal_day = datetime.date(2026, 11, 30)  # 下一个调仓日
        self.assertTrue(daily.is_rebalance_day(self.rebal_day))
        self.assertFalse(daily.is_rebalance_day(self.next_day))
        self.assertTrue(daily.is_rebalance_day(self.next_rebal_day))

    def test_blocks_second_same_utc_day_run_and_allows_next_day_and_force(self):
        with tempfile.TemporaryDirectory() as td:
            lock_path = Path(td) / "last_daily_run.json"
            with mock.patch.object(daily, "run_script", return_value=True) as m_run, \
                 mock.patch.object(daily, "ensure_current_model") as m_model, \
                 mock.patch.object(daily, "_read_api_token", return_value="tok"), \
                 mock.patch.object(daily, "execute_ml_rebalance", return_value={"ok": True, "results": [1]}) as m_exec:
                # 1. 同日第一次触发（如 00:15 UTC）：正常拉数据、下单并写入锁
                code1 = daily.run_daily(_make_args(lock_path), today=self.rebal_day)
                self.assertEqual(code1, 0)
                self.assertEqual(m_exec.call_count, 1)
                self.assertEqual(m_model.call_count, 1)
                self.assertTrue(lock_path.exists())
                lock_data = json.loads(lock_path.read_text(encoding="utf-8"))
                self.assertEqual(lock_data["last_utc_date"], "2026-11-27")
                self.assertEqual(lock_data["status"], "ok")
                self.assertTrue(lock_data["ran_at_utc"].endswith("Z"))

                # 2. 同日第二次触发（如 16:15 UTC 重复 cron）：被锁拦截，0 退出且不下单、不拉数据
                m_run.reset_mock()
                m_exec.reset_mock()
                m_model.reset_mock()
                code2 = daily.run_daily(_make_args(lock_path), today=self.rebal_day)
                self.assertEqual(code2, 0)
                self.assertEqual(m_run.call_count, 0)
                self.assertEqual(m_exec.call_count, 0)
                self.assertEqual(m_model.call_count, 0)

                # 3. 同日显式 --force：覆盖幂等锁重新执行
                code3 = daily.run_daily(_make_args(lock_path, force=True), today=self.rebal_day)
                self.assertEqual(code3, 0)
                self.assertEqual(m_exec.call_count, 1)
                self.assertEqual(m_model.call_count, 1)

                # 4. 下一个 UTC 日期（2026-11-30 调仓日）：正常放行并更新锁日期
                m_exec.reset_mock()
                code4 = daily.run_daily(_make_args(lock_path), today=self.next_rebal_day)
                self.assertEqual(code4, 0)
                self.assertEqual(m_exec.call_count, 1)
                lock_data2 = json.loads(lock_path.read_text(encoding="utf-8"))
                self.assertEqual(lock_data2["last_utc_date"], "2026-11-30")

    def test_dry_run_neither_blocked_nor_writes_lock(self):
        with tempfile.TemporaryDirectory() as td:
            lock_path = Path(td) / "last_daily_run.json"
            daily.write_daily_lock(self.rebal_day, status="ok", note="prev", lock_path=lock_path)
            with mock.patch.object(daily, "run_script", return_value=True) as m_run, \
                 mock.patch.object(daily, "ensure_current_model"), \
                 mock.patch.object(daily, "execute_ml_rebalance") as m_exec:
                code = daily.run_daily(_make_args(lock_path, dry_run=True), today=self.rebal_day)
                self.assertEqual(code, 0)
                self.assertEqual(m_exec.call_count, 0)
                # 最后一次 run_script 应为 place.py --dry-run
                self.assertEqual(m_run.call_args_list[-1].args[0], "place.py")
                self.assertEqual(json.loads(lock_path.read_text())["note"], "prev")

    def test_failed_execution_does_not_write_lock_allowing_retry(self):
        with tempfile.TemporaryDirectory() as td:
            lock_path = Path(td) / "last_daily_run.json"
            with mock.patch.object(daily, "run_script", return_value=True), \
                 mock.patch.object(daily, "ensure_current_model"), \
                 mock.patch.object(daily, "_read_api_token", return_value="tok"), \
                 mock.patch.object(daily, "execute_ml_rebalance", return_value={"ok": False, "error": "boom"}):
                code = daily.run_daily(_make_args(lock_path), today=self.rebal_day)
                self.assertEqual(code, 1)
                self.assertFalse(lock_path.exists())


class QuarterlyModelFreshnessTests(unittest.TestCase):
    def _fake_model(self, train_end):
        cfg = load_config()
        return {
            "format_version": 5,
            "strategy_id": cfg["strategy_id"],
            "model": cfg["label"],
            "horizons": [1, 3, 5, 7],
            "train_start": cfg["train_start"],
            "train_end": train_end,
            "retrain_frequency": "quarterly",
            "feature_names": ordered_feature_names(),
            "components": [
                {"name": c["name"], "horizon": c["horizon"], "weight": c["weight"],
                 "best_iteration": 1, "trees": [[{"leaf": 0.0}]]}
                for c in cfg["ensemble_components"]
            ],
        }

    def test_quarterly_freshness_across_quarter_boundary(self):
        model = self._fake_model("2026-11-26")
        with mock.patch("strategy.load_model", return_value=model):
            # 2026-11-27（Q4，最近季度末 2026-09-30）：2026-11-26 >= 2026-09-30 -> 有效
            self.assertTrue(daily.model_is_current(datetime.date(2026, 11, 27)))
            # 2027-01-01（Q1，最近季度末 2026-12-31）：2026-11-26 < 2026-12-31 -> 过期触发重训
            self.assertFalse(daily.model_is_current(datetime.date(2027, 1, 1)))
        self.assertEqual(daily.target_train_end_for(datetime.date(2027, 1, 1)), datetime.date(2026, 12, 31))


if __name__ == "__main__":
    unittest.main()
