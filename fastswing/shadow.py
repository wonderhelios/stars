"""Pure-stdlib forward shadow daemon for Core-10 Trend-Filtered 1h SFP Fast-Swing.

Reads ONLY public Binance USD-M Futures market data (`fapi.binance.com`):
  - Closed 1h klines (`/fapi/v1/klines?interval=1h`)
  - Real-time mark prices (`/fapi/v1/premiumIndex`)
  - Realized 8h funding rates (`/fapi/v1/fundingRate`)

No API keys, never places real orders. Writes atomic JSON snapshot to
`STARS_FASTSWING` (default `/var/lib/stars/fastswing_shadow.json`) for the
`stars` web console (`/api/fastswing`).
"""
from __future__ import annotations

import argparse
import json
import os
import time
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

from fastswing.strategy import (
    Bar1h,
    FastSwingConfig,
    SfpSignal,
    compute_indicators,
    detect_sfp_signal,
)

FAPI_BASE = os.environ.get("BINANCE_FAPI_BASE", "https://fapi.binance.com").rstrip("/")
STATE_PATH = Path(os.environ.get("STARS_FASTSWING", "/var/lib/stars/fastswing_shadow.json"))
CAPITAL_DEFAULT = float(os.environ.get("STARS_FASTSWING_CAPITAL", "5000"))
SLOT_LEV_DEFAULT = float(os.environ.get("STARS_FASTSWING_SLOT_LEV", "2.0"))
MAX_GROSS_LEV_DEFAULT = float(os.environ.get("STARS_FASTSWING_MAX_GROSS", "6.0"))
POLL_SEC = int(os.environ.get("STARS_FASTSWING_POLL_SEC", "60"))

TIER_WEIGHTS: Tuple[float, float, float] = (1.0 / 6.0, 2.0 / 6.0, 3.0 / 6.0)
HOUR_MS = 3_600_000


def _new_diagnostics() -> Dict[str, Any]:
    """Counters start at deployment; old ledgers load without resetting equity."""
    return {
        "scanned_bars": 0,
        "raw_sweeps": 0,
        "wick_rejected": 0,
        "ema_rejected": 0,
        "split_rejected": 0,
        "complete_signals": 0,
        "accepted_signals": 0,
        "rejected_signals": 0,
        "unfilled_expired": 0,
        "unfilled_runaway": 0,
        "last_scan": {},
        "recent_events": [],
    }


def _http_get_json(path: str, params: Optional[Dict[str, Any]] = None, timeout: int = 20) -> Any:
    url = f"{FAPI_BASE}{path}"
    if params:
        url += "?" + urllib.parse.urlencode(params)
    req = urllib.request.Request(url, headers={"User-Agent": "stars-fastswing-shadow/1.0"})
    last_err: Optional[Exception] = None
    for attempt in range(4):
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return json.loads(resp.read().decode("utf-8"))
        except Exception as exc:
            last_err = exc
            if attempt < 3:
                time.sleep(0.8 * (attempt + 1))
    raise RuntimeError(f"GET {url} failed: {last_err}")


def fetch_closed_1h_bars(
    symbols: List[str],
    limit: int = 240,
    now_ms: Optional[int] = None,
) -> Dict[str, List[Bar1h]]:
    """Fetch strictly closed 1h klines before the current UTC hour."""
    if now_ms is None:
        now_ms = int(time.time() * 1000)
    cur_hour_open = (now_ms // HOUR_MS) * HOUR_MS
    out: Dict[str, List[Bar1h]] = {}
    for sym in symbols:
        try:
            rows = _http_get_json("/fapi/v1/klines", {"symbol": sym, "interval": "1h", "limit": limit})
        except Exception:
            continue
        bars: List[Bar1h] = []
        for r in rows:
            try:
                ot = int(r[0])
                if ot >= cur_hour_open:
                    continue
                bars.append(
                    Bar1h(
                        open_time_ms=ot,
                        open=float(r[1]),
                        high=float(r[2]),
                        low=float(r[3]),
                        close=float(r[4]),
                        volume=float(r[5]),
                    )
                )
            except (IndexError, TypeError, ValueError):
                continue
        bars.sort(key=lambda b: b.open_time_ms)
        if bars:
            out[sym] = bars
        time.sleep(0.04)
    return out


def fetch_mark_prices(symbols: List[str]) -> Dict[str, float]:
    rows = _http_get_json("/fapi/v1/premiumIndex")
    want = set(symbols)
    out: Dict[str, float] = {}
    if isinstance(rows, list):
        for r in rows:
            s = r.get("symbol")
            if s in want:
                try:
                    px = float(r.get("markPrice") or 0.0)
                    if px > 0.0:
                        out[s] = px
                except (TypeError, ValueError):
                    pass
    return out


def fetch_funding_since(symbol: str, start_ms: int, end_ms: int) -> List[Tuple[int, float]]:
    if end_ms <= start_ms:
        return []
    rows = _http_get_json(
        "/fapi/v1/fundingRate",
        {"symbol": symbol, "startTime": start_ms + 1, "endTime": end_ms, "limit": 100},
    )
    out: List[Tuple[int, float]] = []
    if isinstance(rows, list):
        for r in rows:
            try:
                ts = int(r["fundingTime"])
                rate = float(r["fundingRate"])
                if start_ms < ts <= end_ms:
                    out.append((ts, rate))
            except (KeyError, TypeError, ValueError):
                continue
    out.sort(key=lambda x: x[0])
    return out


class FastSwingShadowLedger:
    """In-memory + atomic-JSON shadow ledger for Core-10 Trend-Filtered 1h SFP."""

    STRATEGY_ID = "binance_fastswing_sfp_v1"

    def __init__(
        self,
        state: Optional[Dict[str, Any]] = None,
        cfg: Optional[FastSwingConfig] = None,
        capital: Optional[float] = None,
    ):
        self.cfg = cfg or FastSwingConfig(
            slot_leverage=SLOT_LEV_DEFAULT,
            max_gross_leverage=MAX_GROSS_LEV_DEFAULT,
        )
        if state:
            self.state = state
        else:
            cap = float(capital) if (capital is not None and float(capital) > 0.0) else CAPITAL_DEFAULT
            now_ms = int(time.time() * 1000)
            self.state = {
                "strategy": self.STRATEGY_ID,
                "running": True,
                "capital": cap,
                "cash": cap,
                "equity": cap,
                "pnl": 0.0,
                "pnl_pct": 0.0,
                "realized_pnl": 0.0,
                "unrealized_pnl": 0.0,
                "total_funding": 0.0,
                "total_fees": 0.0,
                "gross_notional": 0.0,
                "net_notional": 0.0,
                "peak_equity": cap,
                "max_dd_pct": 0.0,
                "win_rate": 0.0,
                "profit_factor": 0.0,
                "started_at": now_ms,
                "last_update": now_ms,
                "last_scanned_bar_ms": 0,
                "last_error": None,
                "days_elapsed": 0.0,
                "positions": {},  # symbol -> active position dict
                "pending": {},    # symbol -> resting 1:2:3 Maker order dict
                "history": [],    # [[ts_ms, equity], ...]
                "signals": [],    # recent SFP signal log
                "trades": [],     # closed trade log
                "config": {
                    "strategy": self.STRATEGY_ID,
                    "symbols": list(self.cfg.symbols),
                    "lookback_hours": self.cfg.lookback_hours,
                    "atr_period": self.cfg.atr_period,
                    "ema_period": self.cfg.ema_period,
                    "wick_atr_min": self.cfg.wick_atr_min,
                    "tp1_atr": self.cfg.tp1_atr,
                    "tp1_frac": self.cfg.tp1_frac,
                    "sl_buf_atr": self.cfg.sl_buf_atr,
                    "trail_atr": self.cfg.trail_atr,
                    "max_hold_hours": self.cfg.max_hold_hours,
                    "pending_expire_hours": self.cfg.pending_expire_hours,
                    "slot_leverage": self.cfg.slot_leverage,
                    "max_gross_leverage": self.cfg.max_gross_leverage,
                    "maker_fee_bp": round(self.cfg.maker_fee * 10_000.0, 2),
                    "taker_fee_bp": round(self.cfg.taker_fee * 10_000.0, 2),
                },
            }
        for key, value in _new_diagnostics().items():
            self.state.setdefault(key, value)

    def _diag_event(self, event: Dict[str, Any]) -> None:
        events = self.state["recent_events"]
        events.append(event)
        if len(events) > 200:
            del events[:-200]

    def _scan_geometry(self, bars: List[Bar1h], atr: float) -> Tuple[int, float]:
        """Return the same long-first raw sweep side and wick/ATR as the signal."""
        b = bars[-1]
        window = bars[-self.cfg.lookback_hours - 1 : -1]
        prev_low = min(x.low for x in window)
        prev_high = max(x.high for x in window)
        lower = min(b.open, b.close) - b.low
        upper = b.high - max(b.open, b.close)
        if b.low < prev_low and b.close > prev_low:
            return 1, lower / atr if atr > 0 else 0.0
        if b.high > prev_high and b.close < prev_high:
            return -1, upper / atr if atr > 0 else 0.0
        return 0, 0.0

    @classmethod
    def load(
        cls,
        path: Path,
        cfg: Optional[FastSwingConfig] = None,
        capital: Optional[float] = None,
    ) -> "FastSwingShadowLedger":
        if path.exists():
            try:
                data = json.loads(path.read_text(encoding="utf-8"))
                if isinstance(data, dict) and data.get("strategy") == cls.STRATEGY_ID:
                    return cls(state=data, cfg=cfg)
            except Exception:
                pass
        return cls(state=None, cfg=cfg, capital=capital)

    def save(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(".json.tmp")
        snap = self.snapshot()
        tmp.write_text(json.dumps(snap, ensure_ascii=False, indent=2), encoding="utf-8")
        os.replace(tmp, path)

    def _gross_notional(self, prices: Dict[str, float], include_pending: bool = False) -> float:
        g = 0.0
        for sym, pos in self.state["positions"].items():
            px = prices.get(sym) or float(pos.get("mark_price") or pos["avg_entry"])
            g += abs(float(pos["qty"]) * px)
        if include_pending:
            for pend in self.state["pending"].values():
                g += float(pend.get("rem_notional") or 0.0)
        return g

    def mark_to_market(self, prices: Dict[str, float], now_ms: int) -> None:
        unrl = 0.0
        gross = 0.0
        net = 0.0
        for sym, pos in self.state["positions"].items():
            px = prices.get(sym)
            if px and px > 0.0:
                pos["mark_price"] = px
            else:
                px = float(pos.get("mark_price") or pos["avg_entry"])
            side = int(pos["side"])
            qty = float(pos["qty"])
            entry = float(pos["avg_entry"])
            u = qty * ((px - entry) if side == 1 else (entry - px))
            pos["unrealized_pnl"] = u
            pos["notional"] = abs(qty * px)
            unrl += u
            gross += abs(qty * px)
            net += side * abs(qty * px)

        equity = float(self.state["cash"]) + unrl
        cap = float(self.state["capital"])
        self.state["unrealized_pnl"] = unrl
        self.state["equity"] = equity
        self.state["pnl"] = equity - cap
        self.state["pnl_pct"] = ((equity / cap - 1.0) * 100.0) if cap > 0.0 else 0.0
        self.state["gross_notional"] = gross
        self.state["net_notional"] = net
        peak = max(float(self.state.get("peak_equity") or cap), equity)
        self.state["peak_equity"] = peak
        dd = ((peak - equity) / peak * 100.0) if peak > 0.0 else 0.0
        self.state["max_dd_pct"] = max(float(self.state.get("max_dd_pct") or 0.0), dd)
        self.state["last_update"] = now_ms
        self.state["days_elapsed"] = max(0.0, (now_ms - int(self.state["started_at"])) / 86_400_000.0)

        trades = self.state.get("trades") or []
        if trades:
            wins = [float(t["net_pnl"]) for t in trades if float(t["net_pnl"]) > 0.0]
            losses = [float(t["net_pnl"]) for t in trades if float(t["net_pnl"]) <= 0.0]
            self.state["win_rate"] = round(len(wins) / len(trades) * 100.0, 1)
            loss_sum = abs(sum(losses))
            self.state["profit_factor"] = round(sum(wins) / loss_sum, 2) if loss_sum > 1e-9 else (99.0 if wins else 0.0)

        hist = self.state["history"]
        if not hist or (now_ms - int(hist[-1][0])) >= HOUR_MS:
            hist.append([now_ms, round(equity, 4)])
            if len(hist) > 4000:
                del hist[:-4000]

    def apply_funding_events(
        self,
        funding_events: Dict[str, List[Tuple[int, float]]],
        prices: Dict[str, float],
    ) -> float:
        total_delta = 0.0
        for sym, pos in self.state["positions"].items():
            events = funding_events.get(sym) or []
            last_f = int(pos.get("last_funding_ts") or pos["entry_ts"])
            side = int(pos["side"])
            qty = float(pos["qty"])
            px = prices.get(sym) or float(pos.get("mark_price") or pos["avg_entry"])
            for ts, rate in events:
                if ts <= last_f:
                    continue
                cf = -side * qty * px * rate
                self.state["cash"] = float(self.state["cash"]) + cf
                self.state["total_funding"] = float(self.state["total_funding"]) + cf
                pos["funding_usd"] = float(pos.get("funding_usd") or 0.0) + cf
                pos["last_funding_ts"] = ts
                total_delta += cf
        return total_delta

    def _log_signal(
        self,
        ts: int,
        symbol: str,
        side: int,
        price: float,
        atr: float,
        ema: float,
        accepted: bool,
        reason: str,
    ) -> None:
        sigs = self.state.setdefault("signals", [])
        sigs.append({
            "ts": ts,
            "symbol": symbol,
            "side": "long" if side == 1 else "short",
            "price": round(price, 6),
            "atr": round(atr, 6),
            "ema": round(ema, 6),
            "accepted": accepted,
            "reason": reason,
        })
        if len(sigs) > 300:
            del sigs[:-300]

    def _record_closed_trade(self, pos: Dict[str, Any], exit_px: float, exit_ts: int, reason: str) -> None:
        trades = self.state.setdefault("trades", [])
        net_pnl = float(pos["realized_usd"]) - float(pos["fee_usd"]) + float(pos.get("funding_usd") or 0.0)
        trades.append({
            "symbol": pos["symbol"],
            "side": "long" if int(pos["side"]) == 1 else "short",
            "entry_ts": int(pos["entry_ts"]),
            "exit_ts": exit_ts,
            "hold_hours": round(max(0.01, (exit_ts - int(pos["entry_ts"])) / float(HOUR_MS)), 2),
            "avg_entry": round(float(pos["avg_entry"]), 6),
            "exit_price": round(exit_px, 6),
            "init_notional": round(float(pos["init_notional"]), 2),
            "filled_tiers": int(pos.get("filled_tiers") or 1),
            "tp1_hit": bool(pos.get("tp1_hit")),
            "fee_usd": round(float(pos["fee_usd"]), 4),
            "funding_usd": round(float(pos.get("funding_usd") or 0.0), 4),
            "net_pnl": round(net_pnl, 4),
            "reason": reason,
        })
        if len(trades) > 500:
            del trades[:-500]

    def enqueue_signal(self, sig: SfpSignal, now_ms: int, prices: Dict[str, float]) -> bool:
        """Register a new 1:2:3 resting Maker limit order set for an SFP signal."""
        if sig.symbol in self.state["positions"] or sig.symbol in self.state["pending"]:
            self._log_signal(now_ms, sig.symbol, sig.side, sig.close, sig.atr, sig.ema, False, "已有持仓或挂单")
            return False

        equity = float(self.state["equity"])
        if equity <= 0.0:
            return False

        max_notional = self.cfg.slot_leverage * equity
        cur_gross = self._gross_notional(prices, include_pending=True)
        if cur_gross + max_notional > self.cfg.max_gross_leverage * equity * 1.25:
            self._log_signal(now_ms, sig.symbol, sig.side, sig.close, sig.atr, sig.ema, False, "共享保证金池已满")
            return False

        self.state["pending"][sig.symbol] = {
            "symbol": sig.symbol,
            "side": sig.side,
            "created_ts": now_ms,
            "expire_ts": now_ms + self.cfg.pending_expire_hours * HOUR_MS,
            "sig_bar_ms": sig.bar_time_ms,
            "sig_close": sig.close,
            "sig_atr": sig.atr,
            "levels": list(sig.levels),
            "sl_price": sig.sl_price,
            "tier_notionals": [w * max_notional for w in TIER_WEIGHTS],
            "max_notional": max_notional,
            "rem_notional": max_notional,
            "filled": [False, False, False],
            "filled_qty": 0.0,
            "filled_notional": 0.0,
            "filled_cost": 0.0,
            "fee_usd": 0.0,
        }
        self._log_signal(
            now_ms,
            sig.symbol,
            sig.side,
            sig.close,
            sig.atr,
            sig.ema,
            True,
            f"1:2:3挂单(影线{sig.wick_ratio_atr:.1f}ATR, 目标${max_notional:.0f})",
        )
        return True

    def process_pending_and_positions(
        self,
        prices: Dict[str, float],
        latest_bars: Optional[Dict[str, Bar1h]],
        atr_map: Dict[str, float],
        now_ms: int,
    ) -> None:
        """Process active position exits (SL/TP1/Trail/Time) and pending 1:2:3 Maker fills."""
        # 1. Active positions (conservative SL-first ordering)
        for sym in sorted(list(self.state["positions"].keys())):
            pos = self.state["positions"][sym]
            px = prices.get(sym)
            if not px or px <= 0.0:
                continue
            side = int(pos["side"])
            a = atr_map.get(sym) or float(pos.get("sig_atr") or px * 0.01)

            # Determine intrabar extreme if a fresh closed 1h bar finished after entry
            bar = latest_bars.get(sym) if latest_bars else None
            bar_ok = bar is not None and bar.open_time_ms >= int(pos["entry_ts"]) - HOUR_MS
            check_low = min(px, bar.low) if bar_ok else px
            check_high = max(px, bar.high) if bar_ok else px

            # 1a. Hard SL / Trailing SL FIRST
            cur_sl = float(pos["cur_sl"])
            sl_hit = (side == 1 and check_low <= cur_sl) or (side == -1 and check_high >= cur_sl)
            if sl_hit:
                fill_px = min(px, cur_sl) if side == 1 else max(px, cur_sl)
                qty = float(pos["qty"])
                pnl = qty * ((fill_px - float(pos["avg_entry"])) if side == 1 else (float(pos["avg_entry"]) - fill_px))
                fee = qty * fill_px * self.cfg.taker_fee
                self.state["cash"] = float(self.state["cash"]) + pnl - fee
                self.state["realized_pnl"] = float(self.state["realized_pnl"]) + pnl
                self.state["total_fees"] = float(self.state["total_fees"]) + fee
                pos["realized_usd"] = float(pos["realized_usd"]) + pnl
                pos["fee_usd"] = float(pos["fee_usd"]) + fee
                reason = "保本/移动止盈" if pos.get("tp1_hit") else "硬止损"
                self._record_closed_trade(pos, fill_px, now_ms, reason)
                del self.state["positions"][sym]
                continue

            # 1b. 75% TP1 (Maker 2bp)
            if not pos.get("tp1_hit"):
                tp1_px = float(pos["tp1_price"])
                hit_tp1 = (side == 1 and check_high >= tp1_px) or (side == -1 and check_low <= tp1_px)
                if hit_tp1:
                    take_qty = min(float(pos["init_qty"]) * self.cfg.tp1_frac, float(pos["qty"]))
                    pnl = take_qty * ((tp1_px - float(pos["avg_entry"])) if side == 1 else (float(pos["avg_entry"]) - tp1_px))
                    fee = take_qty * tp1_px * self.cfg.maker_fee
                    self.state["cash"] = float(self.state["cash"]) + pnl - fee
                    self.state["realized_pnl"] = float(self.state["realized_pnl"]) + pnl
                    self.state["total_fees"] = float(self.state["total_fees"]) + fee
                    pos["realized_usd"] = float(pos["realized_usd"]) + pnl
                    pos["fee_usd"] = float(pos["fee_usd"]) + fee
                    pos["qty"] = float(pos["qty"]) - take_qty
                    pos["tp1_hit"] = True
                    # Move 25% runner SL to breakeven (+5bp buffer)
                    pos["cur_sl"] = float(pos["avg_entry"]) * (1.0 + 0.0005 * side)
                    pos["best_price"] = max(float(pos["best_price"]), check_high) if side == 1 else min(float(pos["best_price"]), check_low)

            # 1c. Trailing stop ratchet for remaining 25% runner
            if pos.get("tp1_hit") and float(pos["qty"]) > 1e-12:
                if side == 1:
                    if check_high > float(pos["best_price"]):
                        pos["best_price"] = check_high
                    new_sl = float(pos["best_price"]) - self.cfg.trail_atr * a
                    if new_sl > float(pos["cur_sl"]):
                        pos["cur_sl"] = new_sl
                else:
                    if check_low < float(pos["best_price"]):
                        pos["best_price"] = check_low
                    new_sl = float(pos["best_price"]) + self.cfg.trail_atr * a
                    if new_sl < float(pos["cur_sl"]):
                        pos["cur_sl"] = new_sl

            # 1d. Max hold time stop (48h)
            if now_ms - int(pos["entry_ts"]) >= self.cfg.max_hold_hours * HOUR_MS:
                qty = float(pos["qty"])
                pnl = qty * ((px - float(pos["avg_entry"])) if side == 1 else (float(pos["avg_entry"]) - px))
                fee = qty * px * self.cfg.taker_fee
                self.state["cash"] = float(self.state["cash"]) + pnl - fee
                self.state["realized_pnl"] = float(self.state["realized_pnl"]) + pnl
                self.state["total_fees"] = float(self.state["total_fees"]) + fee
                pos["realized_usd"] = float(pos["realized_usd"]) + pnl
                pos["fee_usd"] = float(pos["fee_usd"]) + fee
                self._record_closed_trade(pos, px, now_ms, f"{self.cfg.max_hold_hours}h超时平仓")
                del self.state["positions"][sym]

        # 2. Pending 1:2:3 Maker limit orders
        for sym in sorted(list(self.state["pending"].keys())):
            pend = self.state["pending"][sym]
            px = prices.get(sym)
            if not px or px <= 0.0:
                continue
            side = int(pend["side"])
            bar = latest_bars.get(sym) if latest_bars else None
            bar_ok = bar is not None and bar.open_time_ms > int(pend["sig_bar_ms"])
            check_low = min(px, bar.low) if bar_ok else px
            check_high = max(px, bar.high) if bar_ok else px

            # Cancel completely unfilled order if price ran away >= TP1
            if float(pend["filled_qty"]) <= 0.0:
                runaway_px = float(pend["sig_close"]) + side * self.cfg.tp1_atr * float(pend["sig_atr"])
                ran_away = (side == 1 and check_high >= runaway_px) or (side == -1 and check_low <= runaway_px)
                if ran_away:
                    self.state["unfilled_runaway"] += 1
                    self._diag_event({
                        "ts": now_ms,
                        "symbol": sym,
                        "side": "long" if side == 1 else "short",
                        "stage": "unfilled_runaway",
                        "reason": "价格达到TP1距离，未成交挂单取消",
                    })
                    del self.state["pending"][sym]
                    continue

            # Check each unfilled tier
            for idx in range(3):
                if not pend["filled"][idx]:
                    lvl = float(pend["levels"][idx])
                    hit = (side == 1 and check_low <= lvl) or (side == -1 and check_high >= lvl)
                    if hit:
                        eq_now = float(self.state["equity"])
                        cur_gross = self._gross_notional(prices, include_pending=False)
                        tn = float(pend["tier_notionals"][idx])
                        if cur_gross + tn > self.cfg.max_gross_leverage * eq_now:
                            continue
                        fill_px = min(px, lvl) if side == 1 else max(px, lvl)
                        qty = tn / fill_px
                        fee = tn * self.cfg.maker_fee
                        self.state["cash"] = float(self.state["cash"]) - fee
                        self.state["total_fees"] = float(self.state["total_fees"]) + fee
                        pend["filled"][idx] = True
                        pend["filled_qty"] = float(pend["filled_qty"]) + qty
                        pend["filled_notional"] = float(pend["filled_notional"]) + tn
                        pend["filled_cost"] = float(pend["filled_cost"]) + qty * fill_px
                        pend["fee_usd"] = float(pend["fee_usd"]) + fee
                        pend["rem_notional"] = max(0.0, float(pend["rem_notional"]) - tn)

            # Promote to active position or stop out if SL breached while filling
            if float(pend["filled_qty"]) > 0.0:
                sl_px = float(pend["sl_price"])
                sl_hit = (side == 1 and check_low <= sl_px) or (side == -1 and check_high >= sl_px)
                avg_entry = float(pend["filled_cost"]) / float(pend["filled_qty"])
                if sl_hit:
                    qty = float(pend["filled_qty"])
                    fill_px = min(px, sl_px) if side == 1 else max(px, sl_px)
                    pnl = qty * ((fill_px - avg_entry) if side == 1 else (avg_entry - fill_px))
                    fee = qty * fill_px * self.cfg.taker_fee
                    self.state["cash"] = float(self.state["cash"]) + pnl - fee
                    self.state["realized_pnl"] = float(self.state["realized_pnl"]) + pnl
                    self.state["total_fees"] = float(self.state["total_fees"]) + fee
                    fake_pos = {
                        "symbol": sym,
                        "side": side,
                        "entry_ts": now_ms,
                        "avg_entry": avg_entry,
                        "init_notional": float(pend["filled_notional"]),
                        "filled_tiers": sum(1 for x in pend["filled"] if x),
                        "tp1_hit": False,
                        "realized_usd": pnl,
                        "fee_usd": float(pend["fee_usd"]) + fee,
                        "funding_usd": 0.0,
                    }
                    self._record_closed_trade(fake_pos, fill_px, now_ms, "接针即破位止损")
                    del self.state["pending"][sym]
                    continue

                tp1_px = avg_entry + side * self.cfg.tp1_atr * float(pend["sig_atr"])
                hit_tp1 = (side == 1 and check_high >= tp1_px) or (side == -1 and check_low <= tp1_px)
                all_filled = all(pend["filled"])
                expired = now_ms >= int(pend["expire_ts"])
                if hit_tp1 or all_filled or expired:
                    pos = {
                        "symbol": sym,
                        "side": side,
                        "entry_ts": now_ms,
                        "last_funding_ts": now_ms,
                        "qty": float(pend["filled_qty"]),
                        "init_qty": float(pend["filled_qty"]),
                        "avg_entry": avg_entry,
                        "mark_price": px,
                        "init_notional": float(pend["filled_notional"]),
                        "notional": float(pend["filled_notional"]),
                        "filled_tiers": sum(1 for x in pend["filled"] if x),
                        "sig_atr": float(pend["sig_atr"]),
                        "cur_sl": sl_px,
                        "tp1_price": tp1_px,
                        "tp1_hit": False,
                        "best_price": avg_entry,
                        "realized_usd": 0.0,
                        "unrealized_pnl": 0.0,
                        "fee_usd": float(pend["fee_usd"]),
                        "funding_usd": 0.0,
                    }
                    if hit_tp1:
                        take_qty = pos["init_qty"] * self.cfg.tp1_frac
                        pnl = take_qty * ((tp1_px - avg_entry) if side == 1 else (avg_entry - tp1_px))
                        fee = take_qty * tp1_px * self.cfg.maker_fee
                        self.state["cash"] = float(self.state["cash"]) + pnl - fee
                        self.state["realized_pnl"] = float(self.state["realized_pnl"]) + pnl
                        self.state["total_fees"] = float(self.state["total_fees"]) + fee
                        pos["realized_usd"] += pnl
                        pos["fee_usd"] += fee
                        pos["qty"] -= take_qty
                        pos["tp1_hit"] = True
                        pos["cur_sl"] = avg_entry * (1.0 + 0.0005 * side)
                        pos["best_price"] = max(avg_entry, check_high) if side == 1 else min(avg_entry, check_low)
                    self.state["positions"][sym] = pos
                    del self.state["pending"][sym]
                    continue

            if now_ms >= int(pend["expire_ts"]):
                self.state["unfilled_expired"] += 1
                self._diag_event({
                    "ts": now_ms,
                    "symbol": sym,
                    "side": "long" if side == 1 else "short",
                    "stage": "unfilled_expired",
                    "reason": "6小时到期，三档限价均未成交",
                })
                del self.state["pending"][sym]

        self.mark_to_market(prices, now_ms)

    def snapshot(self) -> Dict[str, Any]:
        snap = dict(self.state)
        snap["positions_list"] = sorted(
            self.state["positions"].values(),
            key=lambda p: -float(p.get("notional") or 0.0),
        )
        snap["pending_list"] = sorted(
            self.state["pending"].values(),
            key=lambda p: int(p.get("created_ts") or 0),
        )
        return snap


def step_once(
    ledger: FastSwingShadowLedger,
    state_path: Path = STATE_PATH,
    now_ms: Optional[int] = None,
) -> Dict[str, Any]:
    if now_ms is None:
        now_ms = int(time.time() * 1000)
    syms = list(ledger.cfg.symbols)
    try:
        prices = fetch_mark_prices(syms)
        if not prices:
            raise RuntimeError("empty mark prices from Binance")

        # Settle funding for open positions
        f_events: Dict[str, List[Tuple[int, float]]] = {}
        for sym, pos in ledger.state["positions"].items():
            last_f = int(pos.get("last_funding_ts") or pos["entry_ts"])
            if now_ms - last_f >= HOUR_MS:
                try:
                    f_events[sym] = fetch_funding_since(sym, last_f, now_ms)
                except Exception:
                    pass
        if f_events:
            ledger.apply_funding_events(f_events, prices)

        # Fetch closed 1h bars when a new hour has closed (or on first boot)
        last_closed_hour_ms = (now_ms // HOUR_MS) * HOUR_MS - HOUR_MS
        need_scan = int(ledger.state.get("last_scanned_bar_ms") or 0) < last_closed_hour_ms
        latest_bars: Dict[str, Bar1h] = {}
        atr_map: Dict[str, float] = {}
        bars_by_sym: Dict[str, List[Bar1h]] = {}

        if need_scan or ledger.state["positions"] or ledger.state["pending"]:
            bars_by_sym = fetch_closed_1h_bars(syms, limit=240, now_ms=now_ms)
            for sym, blist in bars_by_sym.items():
                if blist:
                    latest_bars[sym] = blist[-1]
                    atr_l, _ = compute_indicators(blist, ledger.cfg.atr_period, ledger.cfg.ema_period)
                    if atr_l:
                        atr_map[sym] = atr_l[-1]

        ledger.mark_to_market(prices, now_ms)
        ledger.process_pending_and_positions(prices, latest_bars, atr_map, now_ms)

        if need_scan and bars_by_sym:
            scan = {
                "bar_ms": last_closed_hour_ms,
                "ts": now_ms,
                "scanned_bars": 0,
                "raw_sweeps": 0,
                "wick_rejected": 0,
                "ema_rejected": 0,
                "split_rejected": 0,
                "complete_signals": 0,
            }
            for sym in syms:
                blist = bars_by_sym.get(sym)
                if not blist or blist[-1].open_time_ms < last_closed_hour_ms:
                    continue
                ledger.state["scanned_bars"] += 1
                scan["scanned_bars"] += 1
                side, wick_atr = ledger._scan_geometry(blist, atr_map.get(sym, 0.0))
                if side:
                    ledger.state["raw_sweeps"] += 1
                    scan["raw_sweeps"] += 1
                sig, reason = detect_sfp_signal(sym, blist, ledger.cfg)
                stage = ""
                if "拆股" in reason:
                    ledger.state["split_rejected"] += 1
                    scan["split_rejected"] += 1
                    stage = "split_rejected"
                elif sig is None and side and wick_atr < ledger.cfg.wick_atr_min:
                    ledger.state["wick_rejected"] += 1
                    scan["wick_rejected"] += 1
                    stage = "wick_rejected"
                elif "逆EMA" in reason:
                    ledger.state["ema_rejected"] += 1
                    scan["ema_rejected"] += 1
                    stage = "ema_rejected"
                elif sig is not None:
                    ledger.state["complete_signals"] += 1
                    scan["complete_signals"] += 1
                    accepted = ledger.enqueue_signal(sig, now_ms, prices)
                    ledger.state["accepted_signals" if accepted else "rejected_signals"] += 1
                    stage = "complete_signal" if accepted else "signal_rejected"
                if side:
                    ledger._diag_event({
                        "ts": now_ms,
                        "bar_ts": blist[-1].open_time_ms,
                        "symbol": sym,
                        "side": "long" if side == 1 else "short",
                        "stage": stage or "raw_sweep",
                        "reason": reason,
                        "wick_atr": round(wick_atr, 3),
                    })
            ledger.state["last_scan"] = scan
            ledger.state["last_scanned_bar_ms"] = last_closed_hour_ms

        ledger.state["last_error"] = None
    except Exception as exc:
        ledger.state["last_error"] = str(exc)
        ledger.state["last_update"] = now_ms

    ledger.save(state_path)
    return ledger.snapshot()


def main() -> None:
    parser = argparse.ArgumentParser(description="Binance Core-10 1h Trend-Filtered SFP forward shadow daemon")
    parser.add_argument("--state-file", default=str(STATE_PATH), help="Path to fastswing_shadow.json")
    parser.add_argument("--capital", type=float, default=CAPITAL_DEFAULT, help="Initial shadow capital (USD)")
    parser.add_argument("--reset", action="store_true", help="Reset shadow ledger and run one poll, then print JSON")
    parser.add_argument("--once", action="store_true", help="Run one poll cycle and exit")
    args = parser.parse_args()

    state_path = Path(args.state_file)
    cfg = FastSwingConfig(slot_leverage=SLOT_LEV_DEFAULT, max_gross_leverage=MAX_GROSS_LEV_DEFAULT)

    if args.reset:
        ledger = FastSwingShadowLedger(state=None, cfg=cfg, capital=args.capital)
        snap = step_once(ledger, state_path=state_path)
        print(json.dumps(snap, ensure_ascii=False))
        return

    if args.once:
        ledger = FastSwingShadowLedger.load(state_path, cfg=cfg, capital=args.capital)
        snap = step_once(ledger, state_path=state_path)
        print(json.dumps(snap, ensure_ascii=False))
        return

    ledger = FastSwingShadowLedger.load(state_path, cfg=cfg, capital=args.capital)
    print(
        f"[fastswing-shadow] started | path={state_path} | capital=${ledger.state['capital']:.0f} "
        f"| slot={cfg.slot_leverage}x | max_gross={cfg.max_gross_leverage}x",
        flush=True,
    )
    while True:
        ledger = FastSwingShadowLedger.load(state_path, cfg=cfg, capital=args.capital)
        step_once(ledger, state_path=state_path)
        print(
            f"[fastswing-shadow] eq=${ledger.state['equity']:.2f} ({ledger.state['pnl_pct']:+.2f}%) "
            f"pos={len(ledger.state['positions'])} pend={len(ledger.state['pending'])} "
            f"trades={len(ledger.state['trades'])} err={ledger.state['last_error']}",
            flush=True,
        )
        time.sleep(POLL_SEC)


if __name__ == "__main__":
    main()
