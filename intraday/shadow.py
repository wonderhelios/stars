#!/usr/bin/env python3
"""Hyperliquid A1w 逼空衰竭做空前向影子：只读公开行情，独立模拟账本，绝不下单。

策略规则（严格无前视 OOS 验证版）：
  - 标的池：Hyperliquid 全市场永续合约，24h 成交额 >= $2M
  - 开仓信号（整点扫描）：24h 涨幅 >= +8% 且上一整点已结算资金费率年化 <= -25%，按负费率深度降序做空
  - 仓位与风控：单仓 15% 净值、最多 4 并发仓（最高毛敞口 0.60x）、15% 宽灾备止损、8h 到时平仓、平仓/止损后单币 24h 冷却
  - 成本与资金费：开仓及到时平仓按 Maker 2.5bp/边，触发止损按 Taker 7.0bp/边；每整点按 Hyperliquid fundingHistory 实付结算小时资金费率
"""
from __future__ import annotations

from datetime import datetime, timezone
from pathlib import Path
import fcntl
import json
import math
import os
import time
import traceback
import urllib.request

STATE_PATH = Path(os.environ.get("STARS_INTRADAY", "/var/lib/stars/intraday_shadow.json"))
CAPITAL = float(os.environ.get("STARS_INTRADAY_CAPITAL", "2500"))
HL_INFO_URL = os.environ.get("STARS_HL_INFO_URL", "https://api.hyperliquid.xyz/info")

RET24_MIN = 0.08
FR_ANN_MAX = -25.0
MIN_VOL_USD = 2_000_000.0
POSITION_FRACTION = 0.15
MAX_POSITIONS = 4
STOP_LOSS_PCT = 0.15
MAX_HOLDING_HOURS = 8
COOLDOWN_HOURS = 24
FEE_MAKER = 0.00025
FEE_TAKER = 0.00070

HOUR_MS = 3_600_000


def now_ms() -> int:
    return int(datetime.now(timezone.utc).timestamp() * 1000)


def finite(v) -> bool:
    try:
        return v is not None and math.isfinite(float(v))
    except (TypeError, ValueError):
        return False


def post_info(payload: dict, timeout: int = 15):
    body = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        HL_INFO_URL,
        data=body,
        headers={
            "Content-Type": "application/json",
            "User-Agent": "stars-hl-a1w-shadow/1.0",
        },
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def fetch_meta_and_ctxs() -> dict[str, dict]:
    raw = post_info({"type": "metaAndAssetCtxs"})
    if not isinstance(raw, list) or len(raw) < 2:
        raise RuntimeError("Hyperliquid metaAndAssetCtxs 返回格式异常")
    universe = raw[0].get("universe", [])
    ctxs = raw[1]
    out: dict[str, dict] = {}
    for meta, ctx in zip(universe, ctxs):
        coin = meta.get("name")
        if not coin or meta.get("isDelisted"):
            continue
        mark = ctx.get("markPx")
        prev_day = ctx.get("prevDayPx")
        vol24 = ctx.get("dayNtlVlm")
        funding = ctx.get("funding")
        if not finite(mark) or float(mark) <= 0:
            continue
        out[coin] = {
            "mark": float(mark),
            "prev_day": float(prev_day) if finite(prev_day) else 0.0,
            "vol24": float(vol24) if finite(vol24) else 0.0,
            "funding": float(funding) if finite(funding) else 0.0,
        }
    return out


def fetch_funding_history(coin: str, start_ms: int, end_ms: int | None = None) -> list[dict]:
    payload: dict = {"type": "fundingHistory", "coin": coin, "startTime": int(start_ms)}
    if end_ms is not None:
        payload["endTime"] = int(end_ms)
    rows = post_info(payload)
    return rows if isinstance(rows, list) else []


def initial_state() -> dict:
    ts = now_ms()
    return {
        "running": True,
        "capital": CAPITAL,
        "balance": CAPITAL,
        "equity": CAPITAL,
        "started_at": ts,
        "positions": [],
        "trades": [],
        "signals": [],
        "history": [[ts, CAPITAL]],
        "cooldowns": {},
        "realized_pnl": 0.0,
        "total_cost": 0.0,
        "total_funding": 0.0,
        "last_update": ts,
        "last_signal_time": None,
        "last_error": None,
    }


def load_state() -> dict:
    try:
        state = json.loads(STATE_PATH.read_text())
        state.setdefault("cooldowns", {})
        state.setdefault("total_funding", 0.0)
        return state
    except FileNotFoundError:
        return initial_state()


def save_state(state: dict) -> None:
    STATE_PATH.parent.mkdir(parents=True, exist_ok=True)
    tmp = STATE_PATH.with_suffix(".tmp")
    tmp.write_text(json.dumps(state, ensure_ascii=False, separators=(",", ":")))
    tmp.replace(STATE_PATH)


def settle_hourly_funding(state: dict, current_hour: int, fetch_fn=fetch_funding_history) -> list[str]:
    """按 Hyperliquid 实际小时资金费率结算所有在仓空单，严格按整点幂等。"""
    warnings: list[str] = []
    state.setdefault("total_funding", 0.0)
    for p in state["positions"]:
        entry_hour = (int(p["entry_ts"]) // HOUR_MS) * HOUR_MS
        last_fh = int(p.get("last_funding_hour", entry_hour))
        if current_hour <= last_fh:
            continue
        p.setdefault("funding_usd", 0.0)
        try:
            rows = fetch_fn(p["coin"], last_fh + 1, current_hour + 60_000)
            by_hour: dict[int, float] = {}
            for r in rows:
                ts = int(r.get("time", 0))
                h_ts = (ts // HOUR_MS) * HOUR_MS
                if last_fh < h_ts <= current_hour and finite(r.get("fundingRate")):
                    by_hour[h_ts] = float(r["fundingRate"])
            for h_ts in sorted(by_hour):
                fr = by_hour[h_ts]
                # 空单(side=-1)：正费率收钱(+notional*fr)，负费率付钱(notional*fr < 0)
                notional = float(p.get("remaining_notional", p["notional"]))
                fpnl = notional * (-int(p["side"])) * fr
                state["balance"] += fpnl
                state["realized_pnl"] += fpnl
                state["total_funding"] += fpnl
                p["funding_usd"] += fpnl
                p["last_funding_hour"] = h_ts
        except Exception as e:
            warnings.append(f"{p['coin']} 费率结算失败: {e}")
    return warnings


def scan_candidates(signal_time: int, ctxs: dict[str, dict], fetch_fn=fetch_funding_history) -> tuple[list[dict], list[str]]:
    """严格无前视整点扫描：筛选 24h 成交额 >= $2M 且 24h 涨幅 >= +8% 的币种，读取上一整点已结算资金费率。"""
    if len(ctxs) < 50:
        raise RuntimeError(f"Hyperliquid 合约行情覆盖不足: {len(ctxs)}")
    warnings: list[str] = []
    out: list[dict] = []
    for coin, c in ctxs.items():
        if c["vol24"] < MIN_VOL_USD or c["prev_day"] <= 0 or c["mark"] <= 0:
            continue
        ret24 = c["mark"] / c["prev_day"] - 1.0
        if ret24 < RET24_MIN:
            continue
        fr = c["funding"]
        try:
            rows = fetch_fn(coin, signal_time - 2 * HOUR_MS, signal_time + 60_000)
            settled = [
                ((int(r["time"]) // HOUR_MS) * HOUR_MS, float(r["fundingRate"]))
                for r in rows
                if finite(r.get("fundingRate")) and ((int(r["time"]) // HOUR_MS) * HOUR_MS) <= signal_time
            ]
            if settled:
                settled.sort(key=lambda x: x[0])
                fr = settled[-1][1]
        except Exception as e:
            warnings.append(f"{coin} fundingHistory: {e}")
        fr_ann = fr * 24.0 * 365.0 * 100.0
        out.append(
            {
                "symbol": coin,
                "side": -1,
                "entry": c["mark"],
                "ret24": ret24,
                "vol24": c["vol24"],
                "fr_1h": fr,
                "fr_ann": fr_ann,
            }
        )
    # 负费率越深排序越靠前
    out.sort(key=lambda x: x["fr_ann"])
    return out, warnings


def snapshot(state: dict, marks: dict[str, float] | None = None) -> dict:
    marks = marks or {}
    unreal = 0.0
    gross = 0.0
    net = 0.0
    for p in state["positions"]:
        mark = marks.get(p["coin"], p.get("mark_price", p["entry_price"]))
        p["mark_price"] = mark
        remaining = float(p.get("remaining_fraction", 1.0))
        p["remaining_notional"] = p["notional"] * remaining
        p["unrealized_pnl"] = p["remaining_notional"] * p["side"] * (mark / p["entry_price"] - 1.0)
        unreal += p["unrealized_pnl"]
        gross += p["remaining_notional"]
        net += p["remaining_notional"] * p["side"]
    state["equity"] = state["balance"] + unreal
    state["unrealized_pnl"] = unreal
    state["pnl"] = state["equity"] - state["capital"]
    state["pnl_pct"] = 100.0 * state["pnl"] / state["capital"] if state["capital"] > 0 else 0.0
    state["gross_notional"] = gross
    state["net_notional"] = net
    state["days_elapsed"] = (now_ms() - state["started_at"]) / 86_400_000.0
    wins = sum(1 for t in state["trades"] if t["pnl_usd"] > 0)
    state["win_rate"] = 100.0 * wins / len(state["trades"]) if state["trades"] else 0.0
    state["last_update"] = now_ms()
    state["config"] = {
        "strategy": "hl_a1w_short",
        "ret24_min": RET24_MIN,
        "fr_ann_max": FR_ANN_MAX,
        "min_vol_usd": MIN_VOL_USD,
        "position_fraction": POSITION_FRACTION,
        "max_positions": MAX_POSITIONS,
        "gross_leverage": round(POSITION_FRACTION * MAX_POSITIONS, 2),
        "stop_loss_pct": STOP_LOSS_PCT,
        "max_holding_hours": MAX_HOLDING_HOURS,
        "cooldown_hours": COOLDOWN_HOURS,
        "fee_per_side": FEE_MAKER,
        "fee_taker": FEE_TAKER,
    }
    return state


def process_exits(state: dict, marks: dict[str, float], now: int | None = None) -> None:
    now = now_ms() if now is None else int(now)
    state.setdefault("cooldowns", {})
    for p in list(state["positions"]):
        mark = marks.get(p["coin"])
        if not finite(mark):
            continue
        mark = float(mark)
        hit_sl = mark >= p["stop_price"] if p["side"] == -1 else mark <= p["stop_price"]
        hit_expiry = now >= p["expiry_ts"]
        if hit_sl:
            reason = "15%宽止损"
            exit_fee_rate = FEE_TAKER
        elif hit_expiry:
            reason = "8h到时平仓"
            exit_fee_rate = FEE_MAKER
        else:
            continue

        notional = float(p.get("remaining_notional", p["notional"]))
        exit_fee = notional * exit_fee_rate
        gross = notional * p["side"] * (mark / p["entry_price"] - 1.0)
        funding_usd = float(p.get("funding_usd", 0.0))
        entry_fee = float(p.get("entry_fee", 0.0))
        # funding_usd 已在整点逐小时计入 balance 与 realized_pnl，这里不再重复叠加到 balance/realized_pnl
        price_net = gross - exit_fee - entry_fee
        total_trade_net = price_net + funding_usd

        state["balance"] += gross - exit_fee
        state["total_cost"] += exit_fee
        state["realized_pnl"] += price_net
        state["trades"].append(
            {
                "exit_ts": now,
                "entry_ts": p["entry_ts"],
                "coin": p["coin"],
                "side": "long" if p["side"] == 1 else "short",
                "exit_price": mark,
                "entry_price": p["entry_price"],
                "notional": notional,
                "entry_fr_ann": p.get("entry_fr_ann", 0.0),
                "funding_usd": funding_usd,
                "pnl_usd": total_trade_net,
                "reason": reason,
            }
        )
        state["cooldowns"][p["coin"]] = now + COOLDOWN_HOURS * HOUR_MS
        state["positions"].remove(p)


def process_entries(state: dict, signal_time: int, candidates: list[dict]) -> None:
    if state.get("last_signal_time") == signal_time:
        return
    cooldowns = state.setdefault("cooldowns", {})
    # 清理已过期冷却记录
    for coin in [k for k, v in cooldowns.items() if v <= signal_time]:
        del cooldowns[coin]

    accepted: set[str] = set()
    for c in candidates:
        coin = c["symbol"]
        fr_ann = float(c["fr_ann"])
        if fr_ann > FR_ANN_MAX:
            reason = f"费率未达 {FR_ANN_MAX:.0f}%"
        elif any(p["coin"] == coin for p in state["positions"]):
            reason = "已有同币仓位"
        elif cooldowns.get(coin, 0) > signal_time:
            rem_h = (cooldowns[coin] - signal_time) / HOUR_MS
            reason = f"24h冷却中({rem_h:.1f}h)"
        elif len(state["positions"]) >= MAX_POSITIONS:
            reason = f"已达{MAX_POSITIONS}仓上限"
        else:
            notional = POSITION_FRACTION * state["equity"]
            entry = float(c["entry"])
            fee = notional * FEE_MAKER
            state["balance"] -= fee
            state["total_cost"] += fee
            entry_hour = (signal_time // HOUR_MS) * HOUR_MS
            state["positions"].append(
                {
                    "coin": coin,
                    "side": -1,
                    "entry_ts": signal_time,
                    "entry_price": entry,
                    "mark_price": entry,
                    "notional": notional,
                    "remaining_fraction": 1.0,
                    "remaining_notional": notional,
                    "entry_fee": fee,
                    "entry_fr_ann": fr_ann,
                    "ret24": float(c["ret24"]),
                    "vol24": float(c["vol24"]),
                    "funding_usd": 0.0,
                    "last_funding_hour": entry_hour,
                    "stop_price": entry * (1.0 + STOP_LOSS_PCT),
                    "expiry_ts": signal_time + MAX_HOLDING_HOURS * HOUR_MS,
                    "unrealized_pnl": 0.0,
                }
            )
            accepted.add(coin)
            reason = "模拟开空"
        state["signals"].append(
            {
                "ts": signal_time,
                "coin": coin,
                "side": "short",
                "ret24": float(c["ret24"]),
                "fr_ann": fr_ann,
                "vol24": float(c["vol24"]),
                "accepted": coin in accepted,
                "reason": reason,
            }
        )
    state["last_signal_time"] = signal_time
    state["signals"] = state["signals"][-120:]


def run_once() -> None:
    state = load_state()
    marks: dict[str, float] = {}
    warnings: list[str] = []
    try:
        ctxs = fetch_meta_and_ctxs()
        marks = {coin: c["mark"] for coin, c in ctxs.items()}
        now = now_ms()
        current_hour = (now // HOUR_MS) * HOUR_MS

        # 1. 先结算跨整点小时资金费率，再处理止损与 8h 到时平仓
        if now >= current_hour + 30_000:
            warnings.extend(settle_hourly_funding(state, current_hour))
        process_exits(state, marks, now=now)
        snapshot(state, marks)

        # 2. 整点入场窗口 [XX:00:30, XX:05:00) 执行 A1w 扫描与开空
        if state.get("last_signal_time") != current_hour:
            if current_hour + 30_000 <= now < current_hour + 5 * 60_000:
                candidates, scan_warns = scan_candidates(current_hour, ctxs)
                warnings.extend(scan_warns)
                process_entries(state, current_hour, candidates)
                snapshot(state, marks)
                state["last_error"] = "；".join(warnings[:3]) if warnings else None
            elif now >= current_hour + 5 * 60_000:
                # 重启或停机错过整点窗口，绝不补造历史入场
                state["last_signal_time"] = current_hour
                state["last_error"] = "错过整点入场窗口，本轮空仓，等待下一小时"
        elif warnings:
            state["last_error"] = "；".join(warnings[:3])
        else:
            state["last_error"] = None

        hist = state["history"]
        bucket = (now // 900_000) * 900_000
        if not hist or hist[-1][0] != bucket:
            hist.append([bucket, state["equity"]])
        else:
            hist[-1] = [bucket, state["equity"]]
        state["history"] = hist[-2880:]
        state["trades"] = state["trades"][-500:]
        state["running"] = True
    except Exception as e:
        state["last_error"] = f"{type(e).__name__}: {e}"
        state["running"] = True
        traceback.print_exc()
    save_state(snapshot(state, marks))


def main() -> None:
    lock = open(STATE_PATH.with_suffix(".lock"), "w")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print("日内影子已有进程运行", flush=True)
        return
    print(f"Hyperliquid A1w 逼空衰竭影子启动，状态={STATE_PATH}，只读行情，不下单", flush=True)
    while True:
        run_once()
        time.sleep(15)


if __name__ == "__main__":
    main()
