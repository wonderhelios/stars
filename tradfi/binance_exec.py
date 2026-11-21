"""Binance USDⓈ-M Futures live executor for TradFi M3b 4-Core Dual-Leg strategy.

Pure Python stdlib (HMAC-SHA256 signing, no third-party SDK required).
Safety features:
  - Defaults to `--dry-run` (computes exact target & diff orders, sends ZERO orders).
  - Live execution requires BOTH `--live` AND `BINANCE_API_KEY` + `BINANCE_API_SECRET`.
  - Corporate Action / Split Guard built into `tradfi.strategy`.
  - Hard Drawdown Circuit Breaker (`--max-dd-pct`, default 15%): refuses new risk if
    account equity drops more than `max_dd_pct` from recorded peak equity.
  - Maker-first (`GTX` Post-Only limit at best bid/ask) with configurable wait &
    IOC/MARKET fallback for unfilled remainder.
  - `--flatten` flag for one-shot emergency reduce-only close of all TradFi positions.

Usage:
  python3 -m tradfi.binance_exec --dry-run --capital 2000
  BINANCE_API_KEY=... BINANCE_API_SECRET=... python3 -m tradfi.binance_exec --live --leverage 2.0
  BINANCE_API_KEY=... BINANCE_API_SECRET=... python3 -m tradfi.binance_exec --flatten --live
"""
from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import math
import os
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

from tradfi.shadow import FAPI_BASE, fetch_closed_4h_bars, fetch_closed_daily_bars, fetch_mark_prices
from tradfi.strategy import StrategyConfig, TargetPortfolio, compute_m3b_target


@dataclass
class SymbolFilters:
    symbol: str
    step_size: float
    min_qty: float
    tick_size: float
    min_notional: float


@dataclass
class PlannedOrder:
    symbol: str
    side: str          # "BUY" | "SELL"
    qty: float
    price: float
    notional: float
    reduce_only: bool
    leg: str
    reason: str


def _step_decimals(step: float) -> int:
    if step <= 0.0 or step >= 1.0:
        return 0
    s = f"{step:.12f}".rstrip("0")
    return len(s.split(".")[1]) if "." in s else 0


def round_step(val: float, step: float) -> float:
    if step <= 0.0:
        return val
    # Floor to step size to avoid overshooting available margin
    n = math.floor(val / step + 1e-12)
    return round(n * step, _step_decimals(step))


def round_tick(price: float, tick: float, side: str) -> float:
    if tick <= 0.0:
        return price
    # Passive Maker rounding: BUY floors to tick, SELL ceils to tick
    n = math.floor(price / tick + 1e-12) if side == "BUY" else math.ceil(price / tick - 1e-12)
    return round(n * tick, _step_decimals(tick))


def format_decimal(val: float, step: float) -> str:
    if step <= 0.0:
        return f"{val:.6f}".rstrip("0").rstrip(".")
    return f"{val:.{_step_decimals(step)}f}"


def exchange_leverage_for_gross(gross_leverage: float) -> int:
    """Compute Binance per-symbol cross leverage ceiling (>= 2x target gross, min 5x)."""
    return max(5, int(math.ceil(max(1.0, float(gross_leverage)) * 2.0)))


class BinanceFuturesClient:
    """Minimal HMAC-SHA256 Binance USDⓈ-M Futures REST client (pure stdlib)."""

    def __init__(self, api_key: str = "", api_secret: str = "", base_url: str = FAPI_BASE):
        self.api_key = api_key.strip()
        self.api_secret = api_secret.strip()
        self.base_url = base_url.rstrip("/")

    def _request(self, method: str, path: str, params: Optional[Dict[str, Any]] = None, signed: bool = False, timeout: int = 15) -> Any:
        q = dict(params or {})
        headers = {"User-Agent": "stars-tradfi-exec/1.0"}
        if signed:
            if not self.api_key or not self.api_secret:
                raise RuntimeError("Missing BINANCE_API_KEY or BINANCE_API_SECRET for signed request")
            q["timestamp"] = int(time.time() * 1000)
            q.setdefault("recvWindow", 5000)
            qs = urllib.parse.urlencode(q)
            sig = hmac.new(self.api_secret.encode("utf-8"), qs.encode("utf-8"), hashlib.sha256).hexdigest()
            qs = f"{qs}&signature={sig}"
            headers["X-MBX-APIKEY"] = self.api_key
        else:
            qs = urllib.parse.urlencode(q)

        url = f"{self.base_url}{path}"
        data = None
        if method in ("GET", "DELETE"):
            if qs:
                url = f"{url}?{qs}"
        else:
            data = qs.encode("utf-8")
            headers["Content-Type"] = "application/x-www-form-urlencoded"

        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return json.loads(resp.read().decode("utf-8"))
        except urllib.error.HTTPError as exc:
            body = exc.read().decode("utf-8", errors="replace")
            raise RuntimeError(f"Binance {method} {path} HTTP {exc.code}: {body}") from exc

    def exchange_filters(self, symbols: List[str]) -> Dict[str, SymbolFilters]:
        info = self._request("GET", "/fapi/v1/exchangeInfo")
        want = set(symbols)
        out: Dict[str, SymbolFilters] = {}
        for s in info.get("symbols", []):
            sym = s.get("symbol")
            if sym not in want:
                continue
            step, min_q, tick, min_not = 0.001, 0.001, 0.01, 5.0
            for f in s.get("filters", []):
                ft = f.get("filterType")
                if ft == "LOT_SIZE":
                    step = float(f.get("stepSize", step))
                    min_q = float(f.get("minQty", min_q))
                elif ft == "PRICE_FILTER":
                    tick = float(f.get("tickSize", tick))
                elif ft == "MIN_NOTIONAL":
                    min_not = float(f.get("notional", min_not))
            out[sym] = SymbolFilters(sym, step, min_q, tick, min_not)
        return out

    def book_ticker(self, symbol: str) -> Tuple[float, float]:
        r = self._request("GET", "/fapi/v1/ticker/bookTicker", {"symbol": symbol})
        return float(r["bidPrice"]), float(r["askPrice"])

    def account_equity_and_positions(self, universe: List[str]) -> Tuple[float, Dict[str, float]]:
        acct = self._request("GET", "/fapi/v2/account", signed=True)
        equity = float(acct.get("totalMarginBalance") or acct.get("totalWalletBalance") or 0.0)
        want = set(universe)
        pos: Dict[str, float] = {}
        for p in acct.get("positions", []):
            sym = p.get("symbol")
            amt = float(p.get("positionAmt") or 0.0)
            if sym in want and abs(amt) > 1e-12:
                pos[sym] = amt
        return equity, pos

    def ensure_cross_and_leverage(self, symbol: str, leverage: int = 5) -> None:
        try:
            self._request("POST", "/fapi/v1/marginType", {"symbol": symbol, "marginType": "CROSSED"}, signed=True)
        except RuntimeError as exc:
            # -4046 = No need to change margin type
            if "-4046" not in str(exc):
                raise
        self._request("POST", "/fapi/v1/leverage", {"symbol": symbol, "leverage": leverage}, signed=True)

    def place_limit_gtx(self, symbol: str, side: str, qty_str: str, price_str: str, reduce_only: bool = False) -> Dict[str, Any]:
        params: Dict[str, Any] = {
            "symbol": symbol,
            "side": side,
            "type": "LIMIT",
            "timeInForce": "GTX",
            "quantity": qty_str,
            "price": price_str,
        }
        if reduce_only:
            params["reduceOnly"] = "true"
        return self._request("POST", "/fapi/v1/order", params, signed=True)

    def place_market(self, symbol: str, side: str, qty_str: str, reduce_only: bool = False) -> Dict[str, Any]:
        params: Dict[str, Any] = {
            "symbol": symbol,
            "side": side,
            "type": "MARKET",
            "quantity": qty_str,
        }
        if reduce_only:
            params["reduceOnly"] = "true"
        return self._request("POST", "/fapi/v1/order", params, signed=True)

    def query_order(self, symbol: str, order_id: int) -> Dict[str, Any]:
        return self._request("GET", "/fapi/v1/order", {"symbol": symbol, "orderId": order_id}, signed=True)

    def cancel_order(self, symbol: str, order_id: int) -> Dict[str, Any]:
        return self._request("DELETE", "/fapi/v1/order", {"symbol": symbol, "orderId": order_id}, signed=True)


def check_dd_breaker(equity: float, state_path: Path, max_dd_pct: float) -> Tuple[bool, float, float]:
    """Track peak equity in `state_path` and check hard drawdown breaker."""
    data: Dict[str, Any] = {}
    if state_path.exists():
        try:
            data = json.loads(state_path.read_text(encoding="utf-8"))
        except Exception:
            data = {}
    peak = max(float(data.get("peak_equity") or 0.0), equity)
    dd_pct = ((peak - equity) / peak * 100.0) if peak > 0 else 0.0
    tripped = max_dd_pct > 0.0 and dd_pct >= max_dd_pct
    data["peak_equity"] = peak
    data["last_equity"] = equity
    data["last_dd_pct"] = round(dd_pct, 3)
    data["breaker_tripped"] = tripped
    data["updated_at"] = int(time.time() * 1000)
    state_path.parent.mkdir(parents=True, exist_ok=True)
    state_path.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
    return tripped, peak, dd_pct


def build_rebalance_orders(
    equity: float,
    current_pos: Dict[str, float],
    target: TargetPortfolio,
    prices: Dict[str, float],
    filters: Dict[str, SymbolFilters],
    min_trade_usd: float = 20.0,
    flatten: bool = False,
) -> List[PlannedOrder]:
    """Compute exact diff orders (closes/reduces first, then opens/increases)."""
    syms = sorted(set(current_pos.keys()) | ({} if flatten else set(target.weights.keys())))
    orders: List[PlannedOrder] = []
    for sym in syms:
        px = prices.get(sym, 0.0)
        if px <= 0.0:
            continue
        flt = filters.get(sym, SymbolFilters(sym, 0.001, 0.001, 0.01, 5.0))
        cur_qty = float(current_pos.get(sym, 0.0))
        tw = 0.0 if flatten else float(target.weights.get(sym, 0.0))
        tgt_qty = (tw * equity / px) if not flatten else 0.0
        delta = tgt_qty - cur_qty
        if abs(delta) < 1e-12:
            continue

        notional = abs(delta * px)
        closing_completely = abs(tgt_qty) < 1e-12 and abs(cur_qty) > 1e-12
        if not closing_completely and notional < max(min_trade_usd, flt.min_notional):
            continue

        qty = round_step(abs(delta), flt.step_size)
        if qty < flt.min_qty:
            continue

        side = "BUY" if delta > 0 else "SELL"
        reduce_only = closing_completely or (cur_qty > 0 and delta < 0 and abs(delta) <= abs(cur_qty)) or (cur_qty < 0 and delta > 0 and abs(delta) <= abs(cur_qty))
        reason = "平仓" if closing_completely else ("减仓" if reduce_only else ("开仓" if abs(cur_qty) < 1e-12 else "调仓"))
        orders.append(PlannedOrder(
            symbol=sym,
            side=side,
            qty=qty,
            price=px,
            notional=qty * px,
            reduce_only=reduce_only,
            leg=target.legs.get(sym, "close"),
            reason=reason,
        ))

    # Execute reduce-only / closes before new risk opens
    orders.sort(key=lambda o: (0 if o.reduce_only else 1, -o.notional))
    return orders


def execute_order_maker_first(
    client: BinanceFuturesClient,
    order: PlannedOrder,
    flt: SymbolFilters,
    maker_wait_sec: float = 8.0,
    taker_only: bool = False,
) -> Dict[str, Any]:
    qty_str = format_decimal(order.qty, flt.step_size)
    if taker_only:
        res = client.place_market(order.symbol, order.side, qty_str, reduce_only=order.reduce_only)
        return {"symbol": order.symbol, "mode": "MARKET", "status": res.get("status"), "orderId": res.get("orderId")}

    bid, ask = client.book_ticker(order.symbol)
    px = round_tick(bid if order.side == "BUY" else ask, flt.tick_size, order.side)
    px_str = format_decimal(px, flt.tick_size)
    try:
        placed = client.place_limit_gtx(order.symbol, order.side, qty_str, px_str, reduce_only=order.reduce_only)
    except RuntimeError as exc:
        # GTX immediately rejected as Post-Only cross (-5022) -> fallback to MARKET
        if "-5022" in str(exc):
            res = client.place_market(order.symbol, order.side, qty_str, reduce_only=order.reduce_only)
            return {"symbol": order.symbol, "mode": "MARKET_FALLBACK", "status": res.get("status"), "orderId": res.get("orderId")}
        raise

    oid = int(placed["orderId"])
    deadline = time.time() + max(1.0, maker_wait_sec)
    status = placed.get("status", "NEW")
    executed = float(placed.get("executedQty") or 0.0)
    while time.time() < deadline and status not in ("FILLED", "CANCELED", "EXPIRED", "REJECTED"):
        time.sleep(1.5)
        q = client.query_order(order.symbol, oid)
        status = q.get("status", status)
        executed = float(q.get("executedQty") or executed)

    if status == "FILLED":
        return {"symbol": order.symbol, "mode": "MAKER_GTX", "status": "FILLED", "orderId": oid, "filled": executed}

    try:
        client.cancel_order(order.symbol, oid)
    except Exception:
        pass
    q = client.query_order(order.symbol, oid)
    executed = float(q.get("executedQty") or executed)
    rem = round_step(max(0.0, order.qty - executed), flt.step_size)
    if rem >= flt.min_qty and rem * order.price >= flt.min_notional:
        rem_str = format_decimal(rem, flt.step_size)
        res = client.place_market(order.symbol, order.side, rem_str, reduce_only=order.reduce_only)
        return {"symbol": order.symbol, "mode": "MAKER+MARKET", "maker_filled": executed, "taker_qty": rem, "status": res.get("status")}
    return {"symbol": order.symbol, "mode": "MAKER_PARTIAL", "filled": executed, "status": status}


def load_tradfi_config(path: Path) -> Dict[str, Any]:
    if not path.exists():
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except Exception:
        return {}


def main() -> None:
    p = argparse.ArgumentParser(description="Binance TradFi M3b 4-Core Dual-Leg Executor")
    p.add_argument("--config", type=str, default=os.environ.get("STARS_TRADFI_CONFIG", ""), help="Optional JSON config path")
    p.add_argument("--live", action="store_true", help="Actually place orders on Binance (default: dry-run)")
    p.add_argument("--dry-run", action="store_true", help="Preview plan only without placing orders")
    p.add_argument("--flatten", action="store_true", help="Close all open TradFi positions (reduce-only)")
    p.add_argument("--capital", type=float, default=None, help="Simulated equity in dry-run when API key absent")
    p.add_argument("--leverage", type=float, default=None, help="Total gross leverage (default 3.0x)")
    p.add_argument("--stock-alloc", type=float, default=None, help="US Stock A4 leg allocation 0..1 (default 0.4)")
    p.add_argument("--vbounce-alloc", type=float, default=None, help="Oversold V-bounce leg allocation 0..0.5 (default 0.2)")
    p.add_argument("--stock-min-bars", type=int, default=None, help="Min daily bars for fast onboarding (default 7)")
    p.add_argument("--max-dd-pct", type=float, default=None, help="Hard peak-to-trough DD breaker pct (0=off)")
    p.add_argument("--maker-wait-sec", type=float, default=None, help="Seconds to wait for GTX Post-Only fill")
    p.add_argument("--taker-only", action="store_true", help="Skip GTX Maker and send MARKET directly")
    p.add_argument("--state-file", type=str, default="/var/lib/stars/tradfi_live_state.json")
    p.add_argument("--json", action="store_true", help="Print structured JSON output")
    args = p.parse_args()

    file_cfg = load_tradfi_config(Path(args.config)) if args.config else {}
    capital = float(args.capital if args.capital is not None else file_cfg.get("capital", 2000.0))
    gross_lev = float(args.leverage if args.leverage is not None else file_cfg.get("gross_leverage", 3.0))
    stock_alloc = float(args.stock_alloc if args.stock_alloc is not None else file_cfg.get("stock_alloc", 0.40))
    stock_alloc = max(0.0, min(1.0, stock_alloc))
    vbounce_alloc = float(args.vbounce_alloc if args.vbounce_alloc is not None else file_cfg.get("vbounce_alloc", 0.20))
    vbounce_alloc = max(0.0, min(0.50, vbounce_alloc))
    if stock_alloc + vbounce_alloc > 1.0:
        vbounce_alloc = max(0.0, 1.0 - stock_alloc)
    comm_alloc = max(0.0, 1.0 - stock_alloc - vbounce_alloc)
    stock_min_bars = int(args.stock_min_bars if args.stock_min_bars is not None else file_cfg.get("stock_min_bars", 7))
    max_dd_pct = float(args.max_dd_pct if args.max_dd_pct is not None else file_cfg.get("max_dd_pct", 20.0))
    maker_wait_sec = float(args.maker_wait_sec if args.maker_wait_sec is not None else file_cfg.get("maker_wait_sec", 8.0))
    taker_only = bool(args.taker_only or file_cfg.get("taker_only", False))

    proxy_url = str(file_cfg.get("proxy_url") or "").strip()
    if proxy_url and not os.environ.get("HTTPS_PROXY"):
        os.environ["HTTPS_PROXY"] = proxy_url
        os.environ["HTTP_PROXY"] = proxy_url

    live = bool(args.live and not args.dry_run)
    cfg = StrategyConfig(
        gross_leverage=gross_lev,
        stock_alloc=stock_alloc,
        comm_alloc=comm_alloc,
        vbounce_alloc=vbounce_alloc,
        stock_min_bars=stock_min_bars,
    )
    all_syms = sorted(set(cfg.stock_universe) | set(cfg.comm_universe))
    state_path = Path(args.state_file)

    api_key = (os.environ.get("BINANCE_API_KEY") or str(file_cfg.get("api_key") or "")).strip()
    api_secret = (os.environ.get("BINANCE_API_SECRET") or str(file_cfg.get("api_secret") or "")).strip()
    key_file = str(file_cfg.get("key_file") or "").strip()
    if (not api_key or not api_secret) and key_file:
        kf_data = load_tradfi_config(Path(key_file))
        api_key = api_key or str(kf_data.get("api_key") or "").strip()
        api_secret = api_secret or str(kf_data.get("api_secret") or "").strip()
    client = BinanceFuturesClient(api_key, api_secret)

    # Load persisted 2-phase stock state and vbounce positions
    prev_phases = None
    prev_vbounce = None
    if state_path.exists():
        try:
            saved_state = json.loads(state_path.read_text(encoding="utf-8"))
            prev_phases = saved_state.get("stock_phases")
            prev_vbounce = saved_state.get("vbounce_positions")
        except Exception:
            prev_phases = None
            prev_vbounce = None

    bars = fetch_closed_daily_bars(all_syms, limit=90)
    bars_4h = fetch_closed_4h_bars(list(cfg.comm_universe), limit=120)
    prices = fetch_mark_prices(all_syms)
    filters = client.exchange_filters(all_syms)
    target = compute_m3b_target(bars, cfg, prev_stock_phases=prev_phases, prev_vbounce_positions=prev_vbounce, bars_4h_map=bars_4h)

    equity_source = "config_capital"
    if api_key and api_secret:
        equity, cur_pos = client.account_equity_and_positions(all_syms)
        equity_source = "binance_account"
    else:
        if live:
            raise SystemExit("ERROR: --live requires BINANCE_API_KEY and BINANCE_API_SECRET")
        equity, cur_pos = capital, {}

    breaker_tripped, peak_eq, dd_pct = check_dd_breaker(equity, state_path, max_dd_pct)
    if breaker_tripped and live and not args.flatten:
        raise SystemExit(
            f"HARD DD BREAKER TRIPPED: equity=${equity:.2f} vs peak=${peak_eq:.2f} (DD={dd_pct:.2f}% >= {max_dd_pct:.1f}%). "
            f"Refusing new orders. Run with --flatten --live to close all positions."
        )

    orders = build_rebalance_orders(equity, cur_pos, target, prices, filters, flatten=args.flatten)
    summary: Dict[str, Any] = {
        "mode": "LIVE" if live else "DRY_RUN",
        "equity": round(equity, 2),
        "equity_source": equity_source,
        "gross_leverage": gross_lev,
        "stock_alloc": stock_alloc,
        "comm_alloc": comm_alloc,
        "vbounce_alloc": vbounce_alloc,
        "peak_equity": round(peak_eq, 2),
        "dd_pct": round(dd_pct, 2),
        "breaker_tripped": breaker_tripped,
        "blocked_symbols": target.blocked_symbols,
        "target_weights": {k: round(v, 4) for k, v in sorted(target.weights.items())},
        "orders": [o.__dict__ for o in orders],
        "results": [],
    }

    if live:
        exch_lev = exchange_leverage_for_gross(gross_lev)
        for o in orders:
            flt = filters.get(o.symbol, SymbolFilters(o.symbol, 0.001, 0.001, 0.01, 5.0))
            if not o.reduce_only:
                client.ensure_cross_and_leverage(o.symbol, leverage=exch_lev)
            res = execute_order_maker_first(client, o, flt, maker_wait_sec=maker_wait_sec, taker_only=taker_only)
            summary["results"].append(res)
        # Persist updated stock_phases and vbounce_positions after live rebalance
        try:
            saved = json.loads(state_path.read_text(encoding="utf-8")) if state_path.exists() else {}
            saved["stock_phases"] = target.stock_phase_weights
            saved["vbounce_positions"] = target.vbounce_positions
            state_path.write_text(json.dumps(saved, ensure_ascii=False, indent=2), encoding="utf-8")
        except Exception:
            pass

    if args.json:
        print(json.dumps(summary, ensure_ascii=False, indent=2))
    else:
        print(f"[{summary['mode']}] equity=${equity:.2f} ({equity_source}) | lev={gross_lev:.1f}x | peak=${peak_eq:.2f} | DD={dd_pct:.2f}% | blocked={target.blocked_symbols}")
        print(f"Orders ({len(orders)}):")
        for o in orders:
            print(f"  {o.side:<4} {o.symbol:<11} qty={o.qty:<10} @≈{o.price:<10.4f} (${o.notional:7.1f}) [{o.leg}] {o.reason}")
        if not live:
            print("[DRY-RUN] No orders sent. Pass --live with BINANCE_API_KEY & BINANCE_API_SECRET to execute.")


if __name__ == "__main__":
    main()
