"""Pure-stdlib forward shadow daemon for Binance TradFi M3b 4-Core Dual-Leg strategy.

Reads ONLY public Binance USDⓈ-M Futures market data (`fapi.binance.com`):
  - Daily closed klines (`/fapi/v1/klines?interval=1d`)
  - Mark prices (`/fapi/v1/premiumIndex`)
  - Realized 8h funding rates (`/fapi/v1/fundingRate`)

No API keys, never places real orders. Writes atomic JSON snapshot to `STARS_TRADFI`
(default `/var/lib/stars/tradfi_shadow.json`) for the `stars` web console (`/api/tradfi`).
"""
from __future__ import annotations

import argparse
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
CONFIG_PATH = Path(os.environ.get("STARS_TRADFI_CONFIG", str(STATE_PATH.with_name("tradfi_config.json"))))
CAPITAL_DEFAULT = float(os.environ.get("STARS_TRADFI_CAPITAL", "2000"))
GROSS_LEVERAGE = float(os.environ.get("STARS_TRADFI_LEVERAGE", "3.0"))
FEE_RATE = float(os.environ.get("STARS_TRADFI_FEE_BP", "2.0")) / 10_000.0  # Maker 2bp default
POLL_SEC = int(os.environ.get("STARS_TRADFI_POLL_SEC", "60"))
REBAL_MINUTE_UTC = int(os.environ.get("STARS_TRADFI_REBAL_MINUTE", "5"))  # 00:05 UTC
POS_STOP_PCT_DEFAULT = float(os.environ.get("STARS_TRADFI_STOP_PCT", "15.0"))  # 15% intraday single-pos hard stop
TRAIL_TP_ACTIVATE_DEFAULT = float(os.environ.get("STARS_TRADFI_TRAIL_ACT", "12.0"))  # activate trailing TP at +12%
TRAIL_TP_CALLBACK_DEFAULT = float(os.environ.get("STARS_TRADFI_TRAIL_CB", "5.0"))  # lock profit on 5% pullback from peak
MAX_DD_PCT_DEFAULT = float(os.environ.get("STARS_TRADFI_MAX_DD_PCT", "20.0"))  # 20% account hard DD breaker
STOP_COOLDOWN_MS = 24 * 3_600_000


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


def fetch_closed_daily_bars(symbols: List[str], limit: int = 90, now_ms: Optional[int] = None) -> Dict[str, List[DailyBar]]:
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


def fetch_closed_4h_bars(symbols: List[str], limit: int = 120, now_ms: Optional[int] = None) -> Dict[str, List[DailyBar]]:
    """Fetch closed 4h klines strictly before the current 4h UTC bar."""
    if now_ms is None:
        now_ms = int(time.time() * 1000)
    bar_ms = 4 * 3_600_000
    cur_open_ms = (now_ms // bar_ms) * bar_ms
    out: Dict[str, List[DailyBar]] = {}
    for sym in symbols:
        try:
            rows = _http_get_json("/fapi/v1/klines", {"symbol": sym, "interval": "4h", "limit": limit})
        except Exception:
            continue
        bars: List[DailyBar] = []
        for r in rows:
            try:
                ot = int(r[0])
                if ot >= cur_open_ms:
                    continue
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

    def __init__(
        self,
        state: Optional[Dict[str, Any]] = None,
        cfg: Optional[StrategyConfig] = None,
        capital: Optional[float] = None,
    ):
        self.cfg = cfg or StrategyConfig(gross_leverage=GROSS_LEVERAGE)
        if state:
            self.state = state
        else:
            cap = float(capital) if (capital is not None and float(capital) > 0.0) else CAPITAL_DEFAULT
            now_ms = int(time.time() * 1000)
            self.state = {
                "strategy": "binance_tradfi_m3b_4core",
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
                "started_at": now_ms,
                "last_update": now_ms,
                "last_rebalance_day": None,
                "last_rebalance_4h_slot": None,
                "last_rebalance_ts": None,
                "last_error": None,
                "days_elapsed": 0.0,
                "peak_equity": cap,
                "max_dd_pct_seen": 0.0,
                "breaker_tripped": False,
                "stop_cooldowns": {},  # symbol -> expire_ts_ms
                "stock_phases": {"p0": {}, "p1": {}},
                "vbounce_positions": {},
                "blocked_symbols": [],
                "positions": {},   # symbol -> dict
                "history": [],     # [[ts_ms, equity], ...]
                "rebalances": [],  # recent rebalance logs
                "trades": [],      # detailed per-symbol fill log
                "config": {
                    "strategy": "binance_tradfi_m3b_4core",
                    "gross_leverage": self.cfg.gross_leverage,
                    "stock_alloc": self.cfg.stock_alloc,
                    "comm_alloc": self.cfg.comm_alloc,
                    "vbounce_alloc": self.cfg.vbounce_alloc,
                    "comm_4h_frac": self.cfg.comm_4h_frac,
                    "stock_min_bars": self.cfg.stock_min_bars,
                    "comm_mode": "S4_daily+4h_dual",
                    "pos_stop_pct": POS_STOP_PCT_DEFAULT,
                    "trail_tp_activate_pct": TRAIL_TP_ACTIVATE_DEFAULT,
                    "trail_tp_callback_pct": TRAIL_TP_CALLBACK_DEFAULT,
                    "max_dd_pct": MAX_DD_PCT_DEFAULT,
                    "fee_bp": round(FEE_RATE * 10_000.0, 2),
                    "min_vol_usd": self.cfg.min_vol_usd,
                    "split_ret_threshold": self.cfg.split_ret_threshold,
                    "comm_universe": list(self.cfg.comm_universe),
                },
            }

    @classmethod
    def load(
        cls,
        path: Path,
        cfg: Optional[StrategyConfig] = None,
        capital: Optional[float] = None,
    ) -> "TradFiShadowLedger":
        if path.exists():
            try:
                data = json.loads(path.read_text(encoding="utf-8"))
                if isinstance(data, dict) and data.get("strategy") == "binance_tradfi_m3b_4core":
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

    def _record_trade(
        self,
        ts: int,
        symbol: str,
        leg: str,
        action: str,
        side: str,
        qty: float,
        price: float,
        notional: float,
        fee_usd: float,
        realized_pnl: Optional[float],
        reason: str,
    ) -> None:
        trades = self.state.setdefault("trades", [])
        trades.append({
            "ts": ts,
            "symbol": symbol,
            "leg": leg,
            "action": action,
            "side": side,
            "qty": round(abs(qty), 6),
            "price": round(price, 6),
            "notional": round(abs(notional), 2),
            "fee_usd": round(fee_usd, 4),
            "realized_pnl": round(realized_pnl, 4) if realized_pnl is not None else None,
            "reason": reason,
        })
        if len(trades) > 500:
            del trades[:-500]

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

            leg_name = target.legs.get(sym, (old.get("leg", "") if old else ""))
            if old is None:
                if abs(target_qty) > 1e-12:
                    side_str = "long" if target_qty > 0 else "short"
                    positions[sym] = {
                        "symbol": sym,
                        "leg": leg_name,
                        "side": side_str,
                        "qty": target_qty,
                        "entry_price": px,
                        "mark_price": px,
                        "notional": abs(target_notional),
                        "target_weight": tw,
                        "unrealized_pnl": 0.0,
                        "funding_usd": 0.0,
                        "peak_fav_pct": 0.0,
                        "entry_ts": now_ms,
                        "last_funding_ts": now_ms,
                    }
                    changes.append(f"+{sym}({tw*100:+.0f}%)")
                    self._record_trade(now_ms, sym, leg_name, "开多" if target_qty > 0 else "开空", side_str, delta_qty, px, delta_notional, fee, None, f"目标权重 {tw*100:+.1f}%")
            elif abs(target_qty) <= 1e-12:
                # Full close
                realized = old_qty * (px - float(old["entry_price"]))
                self.state["cash"] = float(self.state["cash"]) + realized
                self.state["realized_pnl"] = float(self.state["realized_pnl"]) + realized
                old_side = old.get("side", "long" if old_qty > 0 else "short")
                del positions[sym]
                changes.append(f"-{sym}")
                self._record_trade(now_ms, sym, leg_name, "平多" if old_qty > 0 else "平空", old_side, delta_qty, px, delta_notional, fee, realized, "每日调仓平仓")
            elif (old_qty > 0 and target_qty < 0) or (old_qty < 0 and target_qty > 0):
                # Flip side
                realized = old_qty * (px - float(old["entry_price"]))
                self.state["cash"] = float(self.state["cash"]) + realized
                self.state["realized_pnl"] = float(self.state["realized_pnl"]) + realized
                side_str = "long" if target_qty > 0 else "short"
                positions[sym] = {
                    "symbol": sym,
                    "leg": leg_name,
                    "side": side_str,
                    "qty": target_qty,
                    "entry_price": px,
                    "mark_price": px,
                    "notional": abs(target_notional),
                    "target_weight": tw,
                    "unrealized_pnl": 0.0,
                    "funding_usd": 0.0,
                    "peak_fav_pct": 0.0,
                    "entry_ts": now_ms,
                    "last_funding_ts": now_ms,
                }
                changes.append(f"⇄{sym}({tw*100:+.0f}%)")
                self._record_trade(now_ms, sym, leg_name, "反手开多" if target_qty > 0 else "反手开空", side_str, delta_qty, px, delta_notional, fee, realized, f"方向切换至 {tw*100:+.1f}%")
            else:
                # Same direction resize
                side_str = old.get("side", "long" if old_qty > 0 else "short")
                if abs(target_qty) < abs(old_qty):
                    # Partial reduce: realize proportional PnL
                    closed_qty = old_qty - target_qty
                    realized = closed_qty * (px - float(old["entry_price"]))
                    self.state["cash"] = float(self.state["cash"]) + realized
                    self.state["realized_pnl"] = float(self.state["realized_pnl"]) + realized
                    old["qty"] = target_qty
                    self._record_trade(now_ms, sym, leg_name, "减多" if old_qty > 0 else "减空", side_str, delta_qty, px, delta_notional, fee, realized, f"权重调至 {tw*100:+.1f}%")
                else:
                    # Scale up: weighted average entry price
                    added_qty = target_qty - old_qty
                    new_entry = (abs(old_qty) * float(old["entry_price"]) + abs(added_qty) * px) / abs(target_qty)
                    old["qty"] = target_qty
                    old["entry_price"] = new_entry
                    self._record_trade(now_ms, sym, leg_name, "加多" if old_qty > 0 else "加空", side_str, delta_qty, px, delta_notional, fee, None, f"权重调至 {tw*100:+.1f}%")
                old["leg"] = leg_name
                old["target_weight"] = tw
                old["mark_price"] = px
                changes.append(f"~{sym}({tw*100:+.0f}%)")

        self.state["stock_phases"] = target.stock_phase_weights
        self.state["vbounce_positions"] = dict(target.vbounce_positions)
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

    def check_intraday_risk(
        self,
        prices: Dict[str, float],
        now_ms: int,
        fee_rate: float = FEE_RATE,
    ) -> Dict[str, Any]:
        """Enforce 5-minute intraday single-position hard stop and account DD breaker."""
        self.mark_to_market(prices, now_ms)
        equity = float(self.state["equity"])
        peak = max(float(self.state.get("peak_equity") or self.state["capital"]), equity)
        self.state["peak_equity"] = peak
        dd_pct = ((peak - equity) / peak * 100.0) if peak > 0.0 else 0.0
        self.state["max_dd_pct_seen"] = max(float(self.state.get("max_dd_pct_seen") or 0.0), dd_pct)

        c = self.state.setdefault("config", {})
        max_dd_limit = float(c.get("max_dd_pct", MAX_DD_PCT_DEFAULT))
        pos_stop_limit = float(c.get("pos_stop_pct", POS_STOP_PCT_DEFAULT))
        trail_act = float(c.get("trail_tp_activate_pct", TRAIL_TP_ACTIVATE_DEFAULT))
        trail_cb = float(c.get("trail_tp_callback_pct", TRAIL_TP_CALLBACK_DEFAULT))
        cooldowns = self.state.setdefault("stop_cooldowns", {})
        for sym in [s for s, exp in list(cooldowns.items()) if int(exp or 0) <= now_ms]:
            del cooldowns[sym]

        positions = self.state["positions"]
        day_str = time.strftime("%Y-%m-%d", time.gmtime(now_ms / 1000.0))

        # 1. Account-level hard drawdown breaker
        if max_dd_limit > 0.0 and dd_pct >= max_dd_limit:
            self.state["breaker_tripped"] = True
            if positions:
                turnover = 0.0
                fees = 0.0
                closed: List[str] = []
                for sym in sorted(list(positions.keys())):
                    pos = positions.pop(sym)
                    px = prices.get(sym) or float(pos.get("mark_price") or pos["entry_price"])
                    qty = float(pos["qty"])
                    realized = qty * (px - float(pos["entry_price"]))
                    ntl = abs(qty * px)
                    fee = ntl * fee_rate
                    self.state["cash"] = float(self.state["cash"]) + realized - fee
                    self.state["realized_pnl"] = float(self.state["realized_pnl"]) + realized
                    self.state["total_fees"] = float(self.state["total_fees"]) + fee
                    turnover += ntl
                    fees += fee
                    closed.append(sym)
                    self._record_trade(now_ms, sym, pos.get("leg", ""), "熔断平仓", pos.get("side", "long" if qty > 0 else "short"), qty, px, ntl, fee, realized, f"账户回撤熔断 -{dd_pct:.1f}%")
                self.mark_to_market(prices, now_ms)
                self.state["rebalances"].append({
                    "ts": now_ms,
                    "day": day_str,
                    "equity": round(float(self.state["equity"]), 2),
                    "turnover_usd": round(turnover, 2),
                    "fees_usd": round(fees, 4),
                    "n_positions": 0,
                    "blocked": list(self.state.get("blocked_symbols") or []),
                    "changes": [f"⚠回撤熔断平仓(-{dd_pct:.1f}%)"] + [f"-{s}" for s in closed[:20]],
                })
                return {"breaker_tripped": True, "stopped": closed, "dd_pct": dd_pct}
            return {"breaker_tripped": True, "stopped": [], "dd_pct": dd_pct}

        if self.state.get("breaker_tripped"):
            return {"breaker_tripped": True, "stopped": [], "dd_pct": dd_pct}

        # 2. Single-position intraday hard stop-loss & trailing take-profit
        stopped: List[str] = []
        taken: List[str] = []
        changes: List[str] = []
        turnover = 0.0
        fees = 0.0
        if positions:
            for sym in sorted(list(positions.keys())):
                pos = positions[sym]
                entry = float(pos["entry_price"])
                if entry <= 0.0:
                    continue
                px = prices.get(sym) or float(pos.get("mark_price") or entry)
                qty = float(pos["qty"])
                fav_pct = ((px - entry) / entry * 100.0) if qty > 0 else ((entry - px) / entry * 100.0)
                adv_pct = -fav_pct
                peak_fav = max(float(pos.get("peak_fav_pct") or 0.0), fav_pct)
                pos["peak_fav_pct"] = peak_fav

                hit_stop = pos_stop_limit > 0.0 and adv_pct >= pos_stop_limit
                hit_trail = (
                    not hit_stop
                    and trail_act > 0.0
                    and trail_cb > 0.0
                    and peak_fav >= trail_act
                    and (peak_fav - fav_pct) >= trail_cb
                )
                if hit_stop or hit_trail:
                    positions.pop(sym)
                    realized = qty * (px - entry)
                    ntl = abs(qty * px)
                    fee = ntl * fee_rate
                    self.state["cash"] = float(self.state["cash"]) + realized - fee
                    self.state["realized_pnl"] = float(self.state["realized_pnl"]) + realized
                    self.state["total_fees"] = float(self.state["total_fees"]) + fee
                    cooldowns[sym] = now_ms + STOP_COOLDOWN_MS
                    if isinstance(self.state.get("vbounce_positions"), dict):
                        self.state["vbounce_positions"].pop(sym, None)
                    turnover += ntl
                    fees += fee
                    p_side = pos.get("side", "long" if qty > 0 else "short")
                    if hit_stop:
                        stopped.append(sym)
                        changes.append(f"⛔盘中止损:{sym}(-{adv_pct:.1f}%)")
                        self._record_trade(now_ms, sym, pos.get("leg", ""), "盘中止损", p_side, qty, px, ntl, fee, realized, f"逆向亏损 -{adv_pct:.1f}%")
                    else:
                        taken.append(sym)
                        changes.append(f"🎯盘中移动止盈:{sym}({fav_pct:+.1f}%/峰+{peak_fav:.1f}%)")
                        self._record_trade(now_ms, sym, pos.get("leg", ""), "盘中移动止盈", p_side, qty, px, ntl, fee, realized, f"峰值+{peak_fav:.1f}%回撤至{fav_pct:+.1f}%")
        if stopped or taken:
            self.mark_to_market(prices, now_ms)
            self.state["rebalances"].append({
                "ts": now_ms,
                "day": day_str,
                "equity": round(float(self.state["equity"]), 2),
                "turnover_usd": round(turnover, 2),
                "fees_usd": round(fees, 4),
                "n_positions": len(positions),
                "blocked": list(self.state.get("blocked_symbols") or []),
                "changes": changes[:24],
            })
            if len(self.state["rebalances"]) > 200:
                del self.state["rebalances"][:-200]
        return {"breaker_tripped": False, "stopped": stopped, "taken": taken, "dd_pct": dd_pct}

    def snapshot(self) -> Dict[str, Any]:
        snap = dict(self.state)
        def _leg_order(leg: str) -> int:
            if leg.startswith("stock_a4"):
                return 0
            if leg.startswith("comm_ema"):
                return 1
            return 2
        pos_list = sorted(
            self.state["positions"].values(),
            key=lambda p: (_leg_order(str(p.get("leg") or "")), -float(p.get("notional") or 0.0)),
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


def should_rebalance_4h_now(ledger: TradFiShadowLedger, now_ms: int) -> bool:
    if ledger.cfg.comm_4h_frac <= 0.0:
        return False
    bar_ms = 4 * 3_600_000
    slot = now_ms // bar_ms
    last_slot = ledger.state.get("last_rebalance_4h_slot")
    if last_slot is not None and int(last_slot) == slot:
        return False
    elapsed_min = (now_ms % bar_ms) / 60_000.0
    return elapsed_min >= REBAL_MINUTE_UTC


def _read_config_file(config_path: Path) -> Dict[str, Any]:
    if not config_path.exists():
        return {}
    try:
        data = json.loads(config_path.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except Exception:
        return {}


def _capital_from_config(config_path: Path) -> float:
    data = _read_config_file(config_path)
    try:
        cap = float(data.get("capital", CAPITAL_DEFAULT))
        if cap > 0.0:
            return max(50.0, min(10_000_000.0, cap))
    except (TypeError, ValueError):
        pass
    return CAPITAL_DEFAULT


def sync_shadow_config_from_file(ledger: TradFiShadowLedger, config_path: Path = CONFIG_PATH) -> bool:
    """Sync non-secret strategy parameters from tradfi_config.json into the shadow daemon.

    Returns True if any portfolio-affecting parameter (gross_leverage, stock_alloc,
    vbounce_alloc, stock_min_bars) changed relative to the ledger's recorded config.
    """
    data = _read_config_file(config_path)
    if not data:
        return False
    c = ledger.state.setdefault("config", {})
    old_lev = float(c.get("gross_leverage", ledger.cfg.gross_leverage))
    old_sa = float(c.get("stock_alloc", ledger.cfg.stock_alloc))
    old_va = float(c.get("vbounce_alloc", ledger.cfg.vbounce_alloc))
    old_mb = int(c.get("stock_min_bars", ledger.cfg.stock_min_bars))
    try:
        if "gross_leverage" in data:
            ledger.cfg.gross_leverage = max(1.0, min(5.0, float(data["gross_leverage"])))
        sa = max(0.0, min(1.0, float(data.get("stock_alloc", ledger.cfg.stock_alloc))))
        va = max(0.0, min(0.50, float(data.get("vbounce_alloc", ledger.cfg.vbounce_alloc))))
        if sa + va > 1.0:
            va = max(0.0, 1.0 - sa)
        ledger.cfg.stock_alloc = sa
        ledger.cfg.vbounce_alloc = va
        ledger.cfg.comm_alloc = max(0.0, 1.0 - sa - va)
        if "stock_min_bars" in data:
            ledger.cfg.stock_min_bars = max(6, min(30, int(data["stock_min_bars"])))
        proxy_url = str(data.get("proxy_url") or "").strip()
        if proxy_url and not os.environ.get("HTTPS_PROXY"):
            os.environ["HTTPS_PROXY"] = proxy_url
            os.environ["HTTP_PROXY"] = proxy_url
        c["gross_leverage"] = ledger.cfg.gross_leverage
        c["stock_alloc"] = ledger.cfg.stock_alloc
        c["comm_alloc"] = ledger.cfg.comm_alloc
        c["vbounce_alloc"] = ledger.cfg.vbounce_alloc
        c["stock_min_bars"] = ledger.cfg.stock_min_bars
        if "max_dd_pct" in data:
            c["max_dd_pct"] = max(0.0, min(80.0, float(data["max_dd_pct"])))
        if "pos_stop_pct" in data:
            c["pos_stop_pct"] = max(0.0, min(50.0, float(data["pos_stop_pct"])))
    except Exception:
        return False
    return (
        abs(ledger.cfg.gross_leverage - old_lev) > 1e-6
        or abs(ledger.cfg.stock_alloc - old_sa) > 1e-6
        or abs(ledger.cfg.vbounce_alloc - old_va) > 1e-6
        or ledger.cfg.stock_min_bars != old_mb
    )


def step_once(
    ledger: TradFiShadowLedger,
    state_path: Path = STATE_PATH,
    config_path: Path = CONFIG_PATH,
    force_rebalance: bool = False,
) -> Dict[str, Any]:
    now_ms = int(time.time() * 1000)
    cfg_changed = sync_shadow_config_from_file(ledger, config_path)
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

        # Intraday 5-minute risk guard (single-position hard stop + account DD breaker)
        risk = ledger.check_intraday_risk(prices, now_ms, fee_rate=FEE_RATE)

        do_daily = force_rebalance or cfg_changed or should_rebalance_now(ledger, now_ms)
        do_4h = (not do_daily) and should_rebalance_4h_now(ledger, now_ms)
        if not risk["breaker_tripped"] and (do_daily or do_4h):
            bars = fetch_closed_daily_bars(all_syms, limit=90, now_ms=now_ms)
            bars_4h = fetch_closed_4h_bars(list(ledger.cfg.comm_universe), limit=120, now_ms=now_ms)
            day_idx = now_ms // 86_400_000
            target = compute_m3b_target(
                bars,
                ledger.cfg,
                prev_stock_phases=ledger.state.get("stock_phases"),
                day_index=day_idx,
                prev_vbounce_positions=ledger.state.get("vbounce_positions"),
                bars_4h_map=bars_4h,
                freeze_daily_state=do_4h,
            )
            cooldowns = ledger.state.get("stop_cooldowns") or {}
            active_cool = {s for s, exp in cooldowns.items() if int(exp or 0) > now_ms}
            if active_cool:
                for s in active_cool:
                    target.weights.pop(s, None)
                    target.vbounce_positions.pop(s, None)
            if target.weights:
                ledger.rebalance(target, prices, now_ms, fee_rate=FEE_RATE)
            else:
                ledger.mark_to_market(prices, now_ms)
            ledger.state["last_rebalance_4h_slot"] = now_ms // (4 * 3_600_000)

        ledger.state["last_error"] = None
    except Exception as exc:
        ledger.state["last_error"] = str(exc)
        ledger.state["last_update"] = now_ms
    ledger.save(state_path)
    return ledger.snapshot()


def main() -> None:
    parser = argparse.ArgumentParser(description="Binance TradFi M3b 4-Core forward shadow daemon")
    parser.add_argument("--state-file", default=str(STATE_PATH), help="Path to tradfi_shadow.json")
    parser.add_argument("--config", default=str(CONFIG_PATH), help="Path to tradfi_config.json")
    parser.add_argument("--reset", action="store_true", help="Reset shadow ledger using config capital & params, rebalance once, and print JSON")
    parser.add_argument("--rebalance-now", action="store_true", help="Sync latest config params and rebalance existing shadow immediately, then print JSON")
    args = parser.parse_args()

    state_path = Path(args.state_file)
    config_path = Path(args.config)
    cfg = StrategyConfig(gross_leverage=GROSS_LEVERAGE)
    cap = _capital_from_config(config_path)

    if args.reset:
        ledger = TradFiShadowLedger(state=None, cfg=cfg, capital=cap)
        snap = step_once(ledger, state_path=state_path, config_path=config_path, force_rebalance=True)
        print(json.dumps(snap, ensure_ascii=False))
        return

    if args.rebalance_now:
        ledger = TradFiShadowLedger.load(state_path, cfg=cfg, capital=cap)
        snap = step_once(ledger, state_path=state_path, config_path=config_path, force_rebalance=True)
        print(json.dumps(snap, ensure_ascii=False))
        return

    ledger = TradFiShadowLedger.load(state_path, cfg=cfg, capital=cap)
    print(f"[tradfi-shadow] started | path={state_path} | capital=${ledger.state['capital']:.0f} | leverage={cfg.gross_leverage}x", flush=True)
    while True:
        # Reload from disk each cycle so external Web UI resets/rebalances are never overwritten
        loop_cfg = StrategyConfig(gross_leverage=GROSS_LEVERAGE)
        loop_cap = _capital_from_config(config_path)
        ledger = TradFiShadowLedger.load(state_path, cfg=loop_cfg, capital=loop_cap)
        step_once(ledger, state_path=state_path, config_path=config_path)
        print(
            f"[tradfi-shadow] eq=${ledger.state['equity']:.2f} ({ledger.state['pnl_pct']:+.2f}%) "
            f"pos={len(ledger.state['positions'])} err={ledger.state['last_error']}",
            flush=True,
        )
        time.sleep(POLL_SEC)


if __name__ == "__main__":
    main()
