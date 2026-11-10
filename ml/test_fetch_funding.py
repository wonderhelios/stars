"""funding 下载重试策略测试，全部 mock，不访问网络。"""
from pathlib import Path
import io
import sys
import unittest
import urllib.error
from unittest import mock

ML = Path(__file__).resolve().parent
if str(ML) not in sys.path:
    sys.path.insert(0, str(ML))

from fetch_funding import hl_info


class FundingRetryTests(unittest.TestCase):
    @mock.patch("fetch_funding.time.sleep")
    @mock.patch("fetch_funding.urllib.request.urlopen")
    def test_retry_then_success(self, urlopen, sleep):
        urlopen.side_effect = [urllib.error.URLError("timeout"), io.BytesIO(b"[]")]
        self.assertEqual(hl_info({"coin": "BTC"}), [])
        self.assertEqual(urlopen.call_count, 2)
        self.assertEqual(sleep.call_count, 1)

    @mock.patch("fetch_funding.time.sleep")
    @mock.patch("fetch_funding.urllib.request.urlopen")
    def test_stops_after_three_failures(self, urlopen, sleep):
        urlopen.side_effect = urllib.error.URLError("connection reset")
        with self.assertRaisesRegex(RuntimeError, "连续失败 3 次"):
            hl_info({"coin": "ETH"})
        self.assertEqual(urlopen.call_count, 3)
        self.assertEqual(sleep.call_count, 2)

    @mock.patch("fetch_funding.time.sleep")
    @mock.patch("fetch_funding.urllib.request.urlopen")
    def test_http_429_uses_retry_after(self, urlopen, sleep):
        error = urllib.error.HTTPError(
            "https://api.hyperliquid.xyz/info", 429, "rate limit",
            {"Retry-After": "7"}, io.BytesIO(b"limited"),
        )
        urlopen.side_effect = [error, io.BytesIO(b"[]")]
        self.assertEqual(hl_info({"coin": "SOL"}), [])
        sleep.assert_called_once_with(7.0)


if __name__ == "__main__":
    unittest.main()
