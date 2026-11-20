"""Pure-stdlib forward shadow daemon for Binance TradFi M3b 4-Core Dual-Leg strategy.

Reads ONLY public Binance USDⓈ-M Futures market data (`fapi.binance.com`):
  - Daily closed klines (`/fapi/v1/klines?interval=1d`)
  - Mark prices (`/fapi/v1/premiumIndex`)
  - Realized 8h funding rates (`/fapi/v1/fundingRate`)

No API keys, never places real orders. Writes atomic JSON snapshot to `STARS_TRADFI`
(default `/var/lib/stars/tradfi_shadow.json`) for the `stars` web console (`/api/tradfi`).
"""
from __future__ import annotations

import json
import os
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

from tradfi.strategy import (
    DailyBar,
    StrategyConfig,
    TargetPortfolio,
    compute_m3b_target,
)

FAPI_BASE = os.environ.get("BINANCE_FAPI_BASE", "https://fapi.binance.com").rstrip("/")
STATE_PATH = Path(os.environ.get("STARS_TRADFI", "/var/lib/stars/tradfi_shadow.json"))
CAPITAL_DEFAULT = float(os.environ.get("STARS_TRADFI_CAPITAL", "2000"))
GROSS_LEVERAGE = float(os.environ.get("STARS_TRADFI_LEVERAGE", "3.0"))
FEE_RATE = float(os.environ.get("STARS_TRADFI_FEE_BP", "2.0")) / 10_000.0  # Maker 2bp default
POLL_SEC = int(os.environ.get("STARS_TRADFI_POLL_SEC", "300"))
REBAL_MINUTE_UTC = int(os.environ.get("STARS_TRADFI_REBAL_MINUTE", "5"))  # 00:05 UTC


def _http_get_json(path: str, params: Optional[Dict[str, Any]] = None, timeout: int = 20) -> Any:
    url = f"{FAPI_BASE}{path}"
    if params:
        url += "?" + urllib.parse.urlencode(params)
    req = urllib.request.Request(url, headers={"User-Agent": "stars-tradfi-shadow/1.0"})
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


def fetch_closed_daily_bars(symbols: List[str], limit: int = 40, now_ms: Optional[int] = None) -> Dict[str, List[DailyBar]]:
    """Fetch closed daily klines strictly before the current UTC day."""
    if now_ms is None:
        now_ms = int(time.time() * 1000)
    today_open_ms = (now_ms // 86_400_000) * 86_400_000
    out: Dict[str, List[DailyBar]] = {}
    for sym in symbols:
        try:
            rows = _http_get_json("/fapi/v1/klines", {"symbol": sym, "interval": "1d", "limit": limit})
        except Exception:
            continue
        bars: List[DailyBar] = []
        for r in rows:
            try:
                ot = int(r[0])
                if ot >= today_open_ms:
                    continue  # strictly closed daily bars only
                bars.append(DailyBar(open_time_ms=ot, close=float(r[4]), quote_vol=float(r[7])))
            except (IndexError, TypeError, ValueError):
                continue
        bars.sort(key=lambda b: b.open_time_ms)
        if bars:
            out[sym] = bars
        time.sleep(0.05)
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


class TradFiShadowLedger:
    """In-memory + atomic-JSON shadow account for M3b 4-Core Dual-Leg."""

    def __init__(self, state: Optional[Dict[str, Any]] = None, cfg: Optional[StrategyConfig] = None):
        self.cfg = cfg or StrategyConfig(gross_leverage=GROSS_LEVERAGE)
        if state:
            self.state = state
        else:
            now_ms = int(time.time() * 1000)
            self.state = {
                "strategy": "binance_tradfi_m3b_4core",
                "running": True,
                "capital": CAPITAL_DEFAULT,
                "cash": CAPITAL_DEFAULT,
                "equity": CAPITAL_DEFAULT,
                "pnl": 0.0,
                "pnl_pct": 0.0,
                "realized_pnl": 0.0,
                "unrealized_pnl": 0.0,
                "total_funding": 0.0,
                "total_fees": 0.0,
                "gross_notional": 0.0,
                "net_notional": 0.0,
                "started_at": now_ms,
                "last_update": now_ms,
                "last_rebalance_day": None,
                "last_rebalance_ts": None,
                "last_error": None,
                "days_elapsed": 0.0,
                "stock_phases": {"p0": {}, "p1": {}},
                "blocked_symbols": [],
                "positions": {},   # symbol -> dict
                "history": [],     # [[ts_ms, equity], ...]
                "rebalances": [],  # recent rebalance logs
                "config": {
                    "strategy": "binance_tradfi_m3b_4core",
                    "gross_leverage": self.cfg.gross_leverage,
                    "stock_alloc": self.cfg.stock_alloc,
                    "comm_alloc": self.cfg.comm_alloc,
                    "stock_min_bars": self.cfg.stock_min_bars,
                    "comm_mode": "S4_swing",
                    "fee_bp": round(FEE_RATE * 10_000.0, 2),
                    "min_vol_usd": self.cfg.min_vol_usd,
                    "split_ret_threshold": self.cfg.split_ret_threshold,
                    "comm_universe": list(self.cfg.comm_universe),
                },
            }

    @classmethod
    def load(cls, path: Path, cfg: Optional[StrategyConfig] = None) -> "TradFiShadowLedger":
        if path.exists():
            try:
                data = json.loads(path.read_text(encoding="utf-8"))
                if isinstance(data, dict) and data.get("strategy") == "binance_tradfi_m3b_4core":
                    return cls(state=data, cfg=cfg)
            except Exception:
                pass
        return cls(state=None, cfg=cfg)

    def save(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(".json.tmp")
        snap = self.snapshot()
        tmp.write_text(json.dumps(snap, ensure_ascii=False, indent=2), encoding="utf-8")
        os.replace(tmp, path)

    def mark_to_market(self, prices: Dict[str, float], now_ms: int) -> None:
        unrl = 0.0
        gross = 0.0
        net = 0.0
        for sym, pos in self.state["positions"].items():
            px = prices.get(sym)
            if px and px > 0.0:
                pos["mark_price"] = px
            else:
                px = float(pos.get("mark_price") or pos["entry_price"])
            qty = float(pos["qty"])  # signed: >0 long, <0 short
            entry = float(pos["entry_price"])
            u = qty * (px - entry)
            pos["unrealized_pnl"] = u
            pos["notional"] = abs(qty * px)
            unrl += u
            gross += abs(qty * px)
            net += qty * px

        equity = float(self.state["cash"]) + unrl
        cap = float(self.state["capital"])
        self.state["unrealized_pnl"] = unrl
        self.state["equity"] = equity
        self.state["pnl"] = equity - cap
        self.state["pnl_pct"] = ((equity / cap - 1.0) * 100.0) if cap > 0 else 0.0
        self.state["gross_notional"] = gross
        self.state["net_notional"] = net
        self.state["last_update"] = now_ms
        self.state["days_elapsed"] = max(0.0, (now_ms - int(self.state["started_at"])) / 86_400_000.0)

        # Record hourly equity point
        hist = self.state["history"]
        if not hist or (now_ms - int(hist[-1][0])) >= 3_600_000:
            hist.append([now_ms, round(equity, 4)])
            if len(hist) > 4000:
                del hist[:-4000]

    def apply_funding_events(self, funding_events: Dict[str, List[Tuple[int, float]]], prices: Dict[str, float]) -> float:
        """Settle realized 8h funding rates for open positions."""
        total_delta = 0.0
        for sym, pos in self.state["positions"].items():
            events = funding_events.get(sym) or []
            last_f = int(pos.get("last_funding_ts") or pos["entry_ts"])
            qty = float(pos["qty"])
            px = prices.get(sym) or float(pos.get("mark_price") or pos["entry_price"])
            for ts, rate in events:
                if ts <= last_f:
                    continue
                # Long pays +rate, Short receives +rate -> cash flow = -qty * px * rate
                cf = -qty * px * rate
                self.state["cash"] = float(self.state["cash"]) + cf
                self.state["total_funding"] = float(self.state["total_funding"]) + cf
                pos["funding_usd"] = float(pos.get("funding_usd") or 0.0) + cf
                pos["last_funding_ts"] = ts
                total_delta += cf
        return total_delta

    def rebalance(self, target: TargetPortfolio, prices: Dict[str, float], now_ms: int, fee_rate: float = FEE_RATE) -> Dict[str, Any]:
        """Rebalance simulated positions to `target.weights` at `prices`."""
        self.mark_to_market(prices, now_ms)
        equity = float(self.state["equity"])
        if equity <= 0.0:
            return {"turnover_usd": 0.0, "fees_usd": 0.0, "changes": 0}

        positions = self.state["positions"]
        all_syms = sorted(set(positions.keys()) | set(target.weights.keys()))
        turnover_usd = 0.0
        fees_usd = 0.0
        changes: List[str] = []

        for sym in all_syms:
            px = prices.get(sym)
            if not px or px <= 0.0:
                continue
            tw = float(target.weights.get(sym, 0.0))
            target_notional = tw * equity
            target_qty = target_notional / px

            old = positions.get(sym)
            old_qty = float(old["qty"]) if old else 0.0
            delta_qty = target_qty - old_qty
            delta_notional = abs(delta_qty * px)

            # Skip tiny dust adjustments (< $15) unless closing completely
            if abs(target_qty) > 1e-12 and delta_notional < 15.0:
                if old:
                    old["leg"] = target.legs.get(sym, old.get("leg", ""))
                    old["target_weight"] = tw
                continue

            fee = delta_notional * fee_rate
            turnover_usd += delta_notional
            fees_usd += fee
            self.state["cash"] = float(self.state["cash"]) - fee
            self.state["total_fees"] = float(self.state["total_fees"]) + fee

            if old is None:
                if abs(target_qty) > 1e-12:
                    positions[sym] = {
                        "symbol": sym,
                        "leg": target.legs.get(sym, ""),
                        "side": "long" if target_qty > 0 else "short",
                        "qty": target_qty,
                        "entry_price": px,
                        "mark_price": px,
                        "notional": abs(target_notional),
                        "target_weight": tw,
                        "unrealized_pnl": 0.0,
                        "funding_usd": 0.0,
                        "entry_ts": now_ms,
                        "last_funding_ts": now_ms,
                    }
                    changes.append(f"+{sym}({tw*100:+.0f}%)")
            elif abs(target_qty) <= 1e-12:
                # Full close
                realized = old_qty * (px - float(old["entry_price"]))
                self.state["cash"] = float(self.state["cash"]) + realized
                self.state["realized_pnl"] = float(self.state["realized_pnl"]) + realized
                del positions[sym]
                changes.append(f"-{sym}")
            elif (old_qty > 0 and target_qty < 0) or (old_qty < 0 and target_qty > 0):
                # Flip side
                realized = old_qty * (px - float(old["entry_price"]))
                self.state["cash"] = float(self.state["cash"]) + realized
                self.state["realized_pnl"] = float(self.state["realized_pnl"]) + realized
                positions[sym] = {
                    "symbol": sym,
                    "leg": target.legs.get(sym, old.get("leg", "")),
                    "side": "long" if target_qty > 0 else "short",
                    "qty": target_qty,
                    "entry_price": px,
                    "mark_price": px,
                    "notional": abs(target_notional),
                    "target_weight": tw,
                    "unrealized_pnl": 0.0,
                    "funding_usd": 0.0,
                    "entry_ts": now_ms,
                    "last_funding_ts": now_ms,
                }
                changes.append(f"⇄{sym}({tw*100:+.0f}%)")
            else:
                # Same direction resize
                if abs(target_qty) < abs(old_qty):
                    # Partial reduce: realize proportional PnL
                    closed_qty = old_qty - target_qty
                    realized = closed_qty * (px - float(old["entry_price"]))
                    self.state["cash"] = float(self.state["cash"]) + realized
                    self.state["realized_pnl"] = float(self.state["realized_pnl"]) + realized
                    old["qty"] = target_qty
                else:
                    # Scale up: weighted average entry price
                    added_qty = target_qty - old_qty
                    new_entry = (abs(old_qty) * float(old["entry_price"]) + abs(added_qty) * px) / abs(target_qty)
                    old["qty"] = target_qty
                    old["entry_price"] = new_entry
                old["leg"] = target.legs.get(sym, old.get("leg", ""))
                old["target_weight"] = tw
                old["mark_price"] = px
                changes.append(f"~{sym}({tw*100:+.0f}%)")

        self.state["stock_phases"] = target.stock_phase_weights
        self.state["blocked_symbols"] = list(target.blocked_symbols)
        day_str = time.strftime("%Y-%m-%d", time.gmtime(now_ms / 1000.0))
        self.state["last_rebalance_day"] = day_str
        self.state["last_rebalance_ts"] = now_ms
        self.mark_to_market(prices, now_ms)

        rec = {
            "ts": now_ms,
            "day": day_str,
            "equity": round(float(self.state["equity"]), 2),
            "turnover_usd": round(turnover_usd, 2),
            "fees_usd": round(fees_usd, 4),
            "n_positions": len(positions),
            "blocked": list(target.blocked_symbols),
            "changes": changes[:24],
        }
        self.state["rebalances"].append(rec)
        if len(self.state["rebalances"]) > 200:
            del self.state["rebalances"][:-200]
        return rec

    def snapshot(self) -> Dict[str, Any]:
        snap = dict(self.state)
        pos_list = sorted(
            self.state["positions"].values(),
            key=lambda p: (0 if p.get("leg") == "stock_a4" else 1, -float(p.get("notional") or 0.0)),
        )
        snap["positions_list"] = pos_list
        return snap


def should_rebalance_now(ledger: TradFiShadowLedger, now_ms: int) -> bool:
    day_str = time.strftime("%Y-%m-%d", time.gmtime(now_ms / 1000.0))
    if ledger.state.get("last_rebalance_day") is None:
        return True
    if ledger.state.get("last_rebalance_day") == day_str:
        return False
    gm = time.gmtime(now_ms / 1000.0)
    return gm.tm_hour > 0 or gm.tm_min >= REBAL_MINUTE_UTC


def step_once(ledger: TradFiShadowLedger) -> None:
    now_ms = int(time.time() * 1000)
    all_syms = sorted(set(ledger.cfg.stock_universe) | set(ledger.cfg.comm_universe) | set(ledger.state["positions"].keys()))
    try:
        prices = fetch_mark_prices(all_syms)
        if not prices:
            raise RuntimeError("empty mark prices from Binance")
        # Settle funding on open positions
        f_events: Dict[str, List[Tuple[int, float]]] = {}
        for sym, pos in ledger.state["positions"].items():
            last_f = int(pos.get("last_funding_ts") or pos["entry_ts"])
            if now_ms - last_f >= 3_600_000:
                try:
                    f_events[sym] = fetch_funding_since(sym, last_f, now_ms)
                except Exception:
                    pass
        if f_events:
            ledger.apply_funding_events(f_events, prices)

        if should_rebalance_now(ledger, now_ms):
            bars = fetch_closed_daily_bars(all_syms, limit=40, now_ms=now_ms)
            day_idx = now_ms // 86_400_000
            target = compute_m3b_target(
                bars,
                ledger.cfg,
                prev_stock_phases=ledger.state.get("stock_phases"),
                day_index=day_idx,
            )
            if target.weights:
                ledger.rebalance(target, prices, now_ms, fee_rate=FEE_RATE)
            else:
                ledger.mark_to_market(prices, now_ms)
        else:
            ledger.mark_to_market(prices, now_ms)

        ledger.state["last_error"] = None
    except Exception as exc:
        ledger.state["last_error"] = str(exc)
        ledger.state["last_update"] = now_ms
    ledger.save(STATE_PATH)


def main() -> None:
    cfg = StrategyConfig(gross_leverage=GROSS_LEVERAGE)
    ledger = TradFiShadowLedger.load(STATE_PATH, cfg=cfg)
    print(f"[tradfi-shadow] started | path={STATE_PATH} | capital=${ledger.state['capital']:.0f} | leverage={cfg.gross_leverage}x", flush=True)
    while True:
        step_once(ledger)
        print(
            f"[tradfi-shadow] eq=${ledger.state['equity']:.2f} ({ledger.state['pnl_pct']:+.2f}%) "
            f"pos={len(ledger.state['positions'])} err={ledger.state['last_error']}",
            flush=True,
        )
        time.sleep(POLL_SEC)


if __name__ == "__main__":
    main()
