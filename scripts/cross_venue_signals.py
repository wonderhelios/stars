#!/usr/bin/env python3
"""列出最近一段时间在至少两个交易所出现的同名信号。"""

import argparse
from collections import defaultdict
from contextlib import closing
from dataclasses import dataclass
from datetime import datetime, timezone
from decimal import Decimal, InvalidOperation
import math
from pathlib import Path
import sqlite3
import sys
import time
from urllib.parse import quote


DB_FILES = {
    "OKX": "paper.sqlite",
    "Binance": "binance.sqlite",
    "HL": "hyperliquid.sqlite",
}


@dataclass(frozen=True)
class Signal:
    venue: str
    inst_id: str
    funding_rate: Decimal
    triggered_at: int


def asset_name(venue: str, inst_id: str) -> str:
    """只移除已知的合约后缀；HIP-3 保留原始名称供结果核对。"""
    if venue == "OKX" and inst_id.endswith("-USDT-SWAP"):
        return inst_id[: -len("-USDT-SWAP")]
    if venue == "Binance" and inst_id.endswith("USDT"):
        return inst_id[: -len("USDT")]
    if venue == "HL":
        return inst_id.split(":", 1)[-1]
    return inst_id


def read_signals(path: Path, venue: str, start_ms: int, end_ms: int) -> list[Signal]:
    if not path.is_file():
        raise RuntimeError(f"{venue} 数据库不存在：{path}")
    uri = f"file:{quote(str(path.resolve()), safe='/')}?mode=ro"
    try:
        with closing(sqlite3.connect(uri, uri=True, timeout=5)) as conn:
            rows = conn.execute(
                "SELECT inst_id, funding_rate, triggered_at FROM signals_v3 "
                "WHERE triggered_at >= ? AND triggered_at <= ? "
                "ORDER BY triggered_at DESC",
                (start_ms, end_ms),
            ).fetchall()
    except sqlite3.Error as exc:
        raise RuntimeError(f"读取 {venue} 的 signals_v3 失败：{exc}") from exc

    signals = []
    for inst_id, raw_rate, triggered_at in rows:
        try:
            rate = Decimal(raw_rate)
        except (InvalidOperation, TypeError) as exc:
            raise RuntimeError(f"{venue} {inst_id} 的资金费率无效：{raw_rate}") from exc
        signals.append(Signal(venue, inst_id, rate, triggered_at))
    return signals


def overlapping_assets(signals: list[Signal]) -> dict[str, dict[str, list[Signal]]]:
    grouped: dict[str, dict[str, list[Signal]]] = defaultdict(lambda: defaultdict(list))
    for signal in signals:
        grouped[asset_name(signal.venue, signal.inst_id)][signal.venue].append(signal)
    return {asset: venues for asset, venues in grouped.items() if len(venues) >= 2}


def rate_pct(rate: Decimal) -> str:
    return f"{rate * 100:+.4f}%"


def signal_time(timestamp_ms: int) -> str:
    return datetime.fromtimestamp(timestamp_ms / 1000, timezone.utc).strftime("%m-%d %H:%M UTC")


def format_cell(signals: list[Signal], venue: str) -> str:
    if not signals:
        return "—"
    if venue == "HL":
        return "; ".join(
            f"{signal.inst_id} {signal_time(signal.triggered_at)} {rate_pct(signal.funding_rate)}/h "
            f"(8h≈{rate_pct(signal.funding_rate * 8)})"
            for signal in signals
        )
    return "; ".join(
        f"{signal.inst_id} {signal_time(signal.triggered_at)} {rate_pct(signal.funding_rate)}"
        for signal in signals
    )


def print_report(signals: list[Signal], hours: float) -> None:
    counts = {venue: sum(s.venue == venue for s in signals) for venue in DB_FILES}
    print(
        f"最近 {hours:g} 小时信号："
        + "，".join(f"{venue} {count}" for venue, count in counts.items())
    )
    matches = overlapping_assets(signals)
    if not matches:
        print("没有在至少两个交易所出现的同名信号。")
        return

    headers = ("Asset", "OKX (per period)", "Binance (per period)", "HL (hourly / 8h equiv)")
    rows = [
        (
            asset,
            format_cell(venues.get("OKX", []), "OKX"),
            format_cell(venues.get("Binance", []), "Binance"),
            format_cell(venues.get("HL", []), "HL"),
        )
        for asset, venues in sorted(matches.items())
    ]
    widths = [max(len(row[i]) for row in [headers, *rows]) for i in range(4)]
    print(" | ".join(headers[i].ljust(widths[i]) for i in range(4)))
    print("-+-".join("-" * width for width in widths))
    for row in rows:
        print(" | ".join(row[i].ljust(widths[i]) for i in range(4)))
    print(f"\n总计 {len(matches)} 个同名币种在至少两个交易所出现。")
    print("同名只表示过去窗口内分别触发，时间戳为 UTC，不代表同时触发。")
    print("HL 的 8h 数字仅为小时费率 ×8；其余费率按各自结算周期记录，不能直接比较。")
    print("HIP-3 同名合约可能不是同一底层资产，需核对原始合约名。")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--db-dir", type=Path, default=Path("/var/lib/okx-quant"), help="数据库目录"
    )
    parser.add_argument("--hours", type=float, default=24, help="回看小时数，默认 24")
    args = parser.parse_args()
    if not math.isfinite(args.hours) or args.hours <= 0:
        parser.error("--hours 必须为大于 0 的有限数字")

    end_ms = int(time.time() * 1000)
    start_ms = end_ms - int(args.hours * 3_600_000)
    try:
        signals = [
            signal
            for venue, filename in DB_FILES.items()
            for signal in read_signals(args.db_dir / filename, venue, start_ms, end_ms)
        ]
    except RuntimeError as exc:
        print(exc, file=sys.stderr)
        return 1
    print_report(signals, args.hours)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
