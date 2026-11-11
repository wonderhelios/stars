"""日线请求边界和已污染数据清理测试。"""
from pathlib import Path
import datetime
import sys
import unittest

ML = Path(__file__).resolve().parent
if str(ML) not in sys.path:
    sys.path.insert(0, str(ML))

from candle_rules import DAY_MS, daily_request_end, merge_closed_daily_bars


def ms(value):
    return int(datetime.datetime.fromisoformat(value).replace(
        tzinfo=datetime.timezone.utc).timestamp() * 1000)


def bar(day, close_day):
    start = ms(day)
    return {"t": start, "T": ms(close_day) - 1, "o": "1", "c": "2"}


class CandleRuleTests(unittest.TestCase):
    def test_exact_day_boundary_excludes_today(self):
        now = ms("2026-11-11T00:00:00")
        self.assertEqual(daily_request_end(now), ms("2026-11-11T00:00:00") - 1)
        rows = merge_closed_daily_bars(
            [], [bar("2026-11-10T00:00:00", "2026-11-11T00:00:00"),
                 bar("2026-11-11T00:00:00", "2026-11-12T00:00:00")], now)
        self.assertEqual([x["t"] for x in rows], [ms("2026-11-10T00:00:00")])

    def test_ten_minutes_after_boundary_still_uses_yesterday(self):
        now = ms("2026-11-11T00:10:00")
        self.assertEqual(daily_request_end(now), ms("2026-11-11T00:00:00") - 1)
        rows = merge_closed_daily_bars(
            [bar("2026-11-11T00:00:00", "2026-11-12T00:00:00")],
            [bar("2026-11-10T00:00:00", "2026-11-11T00:00:00")], now)
        self.assertEqual([x["t"] for x in rows], [ms("2026-11-10T00:00:00")])

    def test_closed_history_is_retained_and_deduplicated(self):
        now = ms("2026-11-12T08:00:00")
        old = bar("2026-11-09T00:00:00", "2026-11-10T00:00:00")
        replacement = dict(old, c="3")
        rows = merge_closed_daily_bars(
            [old], [replacement,
                    bar("2026-11-10T00:00:00", "2026-11-11T00:00:00")], now)
        self.assertEqual(len(rows), 2)
        self.assertEqual(rows[0]["c"], "3")
        self.assertEqual(rows[-1]["T"] - rows[-1]["t"], DAY_MS - 1)


if __name__ == "__main__":
    unittest.main()
