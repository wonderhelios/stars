"""OHLCV 下载必须失败关闭，不能用旧缓存继续调仓。"""
from pathlib import Path
import datetime
import json
import sys
import tempfile
import unittest
from unittest.mock import patch

ML = Path(__file__).resolve().parent
if str(ML) not in sys.path:
    sys.path.insert(0, str(ML))

from candle_rules import DAY_MS, require_latest_closed_daily_bar
from fetch_ohlcv import fetch_one, post


def ms(value):
    return int(datetime.datetime.fromisoformat(value).replace(
        tzinfo=datetime.timezone.utc).timestamp() * 1000)


def bar(day):
    start = ms(day)
    return {"t": start, "T": start + DAY_MS - 1, "o": "1", "c": "2"}


class OhlcvFailClosedTests(unittest.TestCase):
    def test_latest_closed_day_is_required(self):
        now = ms("2026-11-15T00:10:00")
        fresh = [bar("2026-11-13T00:00:00"), bar("2026-11-14T00:00:00")]
        self.assertIs(require_latest_closed_daily_bar(fresh, now, "BTC"), fresh)
        with self.assertRaisesRegex(RuntimeError, "拒绝沿用旧数据"):
            require_latest_closed_daily_bar(fresh[:1], now, "BTC")
        with self.assertRaisesRegex(RuntimeError, "无数据"):
            require_latest_closed_daily_bar([], now, "BTC")

    @patch("fetch_ohlcv.time.sleep")
    @patch("fetch_ohlcv.urllib.request.urlopen", side_effect=TimeoutError("timeout"))
    def test_request_exhaustion_raises(self, _urlopen, _sleep):
        with self.assertRaisesRegex(RuntimeError, "连续失败 5 次"):
            post("https://example.invalid", {"type": "candleSnapshot",
                                            "req": {"coin": "BTC"}})

    @patch("fetch_ohlcv.post", return_value=[])
    def test_empty_response_does_not_overwrite_stale_cache(self, _post):
        now = ms("2026-11-15T00:10:00")
        stale = [bar("2026-11-09T00:00:00")]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "BTC.json"
            path.write_text(json.dumps(stale))
            with self.assertRaisesRegex(RuntimeError, "拒绝沿用旧缓存"):
                fetch_one("BTC", directory, now, ms("2023-02-26T00:00:00"))
            self.assertEqual(json.loads(path.read_text()), stale)

    @patch("fetch_ohlcv.post")
    def test_stale_cache_is_repaired_before_success(self, mocked_post):
        now = ms("2026-11-15T00:10:00")
        mocked_post.return_value = [bar("2026-11-10T00:00:00"),
                                    bar("2026-11-14T00:00:00")]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "BTC.json"
            path.write_text(json.dumps([bar("2026-11-09T00:00:00")]))
            fetch_one("BTC", directory, now, ms("2023-02-26T00:00:00"))
            result = json.loads(path.read_text())
            self.assertEqual(result[-1]["t"], ms("2026-11-14T00:00:00"))

    @patch("fetch_ohlcv.post", return_value=[])
    def test_delisted_history_does_not_block_active_universe(self, _post):
        now = ms("2026-11-15T00:10:00")
        stale = [bar("2024-01-01T00:00:00")]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "OLD.json"
            path.write_text(json.dumps(stale))
            fetch_one("OLD", directory, now, ms("2023-02-26T00:00:00"),
                      require_fresh=False)
            self.assertEqual(json.loads(path.read_text()), stale)


if __name__ == "__main__":
    unittest.main()
