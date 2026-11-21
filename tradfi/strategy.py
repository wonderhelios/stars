"""Pure-stdlib signal & portfolio engine for Binance TradFi M3b 3-Leg strategy.

Architecture (verified causal OOS on 2026-04 ~ 2026-11):
- Leg 1 (40% capital): US Stock 1x Perps Cross-Sectional 3-Factor (A4)
    * Universe: 18 1x US stock/ETF perps (excludes 3x leveraged SOXL/SOXS).
    * Corporate Action Guard: any symbol with |daily_ret| >= 40% in the last 14 closed bars
      (e.g. SNDK 8-for-1 stock split) is masked out of signal eligibility for 14 days.
    * Score: 0.5 * score_3f(7d) + 0.5 * score_3f(14d) for mature stocks (>= 15 bars), with
      7-day fast onboarding fallback (score_3f(7d) once >= 7 closed daily bars are available).
      score_3f(L) = pct_rank(cum_ret_L / vol_L) + pct_rank(close / max_L - 1) + pct_rank(up_frac_L).
    * Liquidity filter: 7-day average quote volume >= $5,000,000.
    * Selection: top 30% Long, bottom 30% Short (min 2 per side, requires >= 6 eligible stocks).
    * Weighting: within-leg inverse-volatility (up to 14d std), single-name cap = 30% of leg gross.
    * 2-Phase Slicing: even-day slice + odd-day slice averaged 50/50 to eliminate phase luck.
- Leg 2 (40% capital): 4 High-Liquidity Core Commodities Dual-Timeframe (50% Daily S4 + 50% 4h EMA12/48)
    * Universe: XAUUSDT (Gold), XAGUSDT (Silver), CLUSDT (WTI Crude Oil), NATGASUSDT (NatGas).
    * Daily sub-leg (50%): EMA(5/20) + 5-day reverse-channel exit + RSI(14) <=25/>=75 half-size.
    * 4h sub-leg (50%): EMA(12/48) + 18-bar (3d) reverse-channel exit + RSI(14) <=25/>=75 half-size.
    * Weighting: inverse-volatility normalized across all valid commodities; single-commodity cap = 40% of leg gross.
- Leg 3 (20% capital): Oversold V-Bounce Long-Only (RSI(2) <= 5 & 3d drop >= 5%)
    * Universe: liquid non-blocked symbols across US stocks + 4 core commodities.
    * Entry: RSI(2) <= 5.0 and 3-day return <= -5.0% on closed daily bars, max 4 equal-weight slots.
    * Exit: close >= MA(5), OR hold_days >= 3, OR adverse return <= -8.0%.
"""
from __future__ import annotations

import math
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Tuple

DEFAULT_STOCK_UNIVERSE: List[str] = [
    "AAPLUSDT", "AMDUSDT", "AMZNUSDT", "ANTUSDT", "AVGOUSDT", "COINUSDT",
    "GOOGLUSDT", "IGVUSDT", "INTCUSDT", "METAUSDT", "MSFTUSDT", "MSTRUSDT",
    "MUUSDT", "NVDAUSDT", "PLTRUSDT", "SNDKUSDT", "SOXXUSDT", "TSLAUSDT",
]

DEFAULT_COMMODITY_UNIVERSE: List[str] = [
    "XAUUSDT", "XAGUSDT", "CLUSDT", "NATGASUSDT",
]

LEVERAGED_BLACKLIST = {"SOXLUSDT", "SOXSUSDT"}


@dataclass
class DailyBar:
    open_time_ms: int
    close: float
    quote_vol: float


@dataclass
class StrategyConfig:
    gross_leverage: float = 3.0
    stock_alloc: float = 0.40
    comm_alloc: float = 0.40
    vbounce_alloc: float = 0.20
    min_vol_usd: float = 5_000_000.0
    stock_min_bars: int = 7
    split_ret_threshold: float = 0.40
    split_block_days: int = 14
    stock_top_frac: float = 0.30
    stock_cap_frac: float = 0.30
    comm_cap_frac: float = 0.40
    ema_fast: int = 5
    ema_slow: int = 20
    comm_trail_days: int = 5
    comm_rsi_lo: float = 25.0
    comm_rsi_hi: float = 75.0
    comm_4h_frac: float = 0.50
    comm_4h_ema_fast: int = 12
    comm_4h_ema_slow: int = 48
    comm_4h_trail_bars: int = 18
    comm_4h_rsi_lo: float = 25.0
    comm_4h_rsi_hi: float = 75.0
    vbounce_rsi_period: int = 2
    vbounce_rsi_max: float = 5.0
    vbounce_drop3_min: float = -0.05
    vbounce_max_pos: int = 4
    vbounce_max_hold_days: int = 3
    vbounce_stop_pct: float = 0.08
    vbounce_ma_exit: int = 5
    vbounce_trend_ma: int = 60
    vbounce_cash_sweep: bool = True
    vol_target_ann: float = 0.40
    vol_target_lookback: int = 14
    vol_target_min_scale: float = 0.35
    stock_universe: List[str] = field(default_factory=lambda: list(DEFAULT_STOCK_UNIVERSE))
    comm_universe: List[str] = field(default_factory=lambda: list(DEFAULT_COMMODITY_UNIVERSE))


@dataclass
class TargetPortfolio:
    asof_day_ms: int
    weights: Dict[str, float]  # symbol -> target weight as fraction of TOTAL account equity
    legs: Dict[str, str]       # symbol -> "stock_a4" | "comm_ema" | "vbounce" | combo
    blocked_symbols: List[str]
    stock_scores: Dict[str, float]
    comm_signals: Dict[str, float]
    stock_phase_weights: Dict[str, Dict[str, float]]  # {"p0": {...}, "p1": {...}}
    vbounce_positions: Dict[str, Dict[str, Any]] = field(default_factory=dict)


def _std(xs: List[float]) -> float:
    n = len(xs)
    if n < 2:
        return 0.0
    m = sum(xs) / n
    var = sum((x - m) ** 2 for x in xs) / (n - 1)
    return math.sqrt(max(var, 0.0))


def _ema_series(vals: List[float], span: int) -> List[float]:
    if not vals:
        return []
    alpha = 2.0 / (span + 1.0)
    out = [vals[0]]
    for v in vals[1:]:
        out.append(alpha * v + (1.0 - alpha) * out[-1])
    return out


def _rsi_latest(vals: List[float], period: int = 14) -> float:
    if len(vals) < period + 1:
        return 50.0
    diffs = [vals[i] - vals[i - 1] for i in range(len(vals) - period, len(vals))]
    if not diffs:
        return 50.0
    up = sum(d for d in diffs if d > 0.0) / len(diffs)
    dn = sum(-d for d in diffs if d < 0.0) / len(diffs)
    if dn <= 1e-12:
        return 100.0 if up > 0.0 else 50.0
    rs = up / dn
    return 100.0 - 100.0 / (1.0 + rs)


def _pct_ranks(values: Dict[str, float]) -> Dict[str, float]:
    """Average-tie percentile rank in (0, 1], matching pandas rank(pct=True)."""
    n = len(values)
    if n == 0:
        return {}
    items = sorted(values.items(), key=lambda kv: kv[1])
    ranks: Dict[str, float] = {}
    i = 0
    while i < n:
        j = i
        while j + 1 < n and items[j + 1][1] == items[i][1]:
            j += 1
        avg_rank = (i + j) / 2.0 + 1.0  # 1-indexed
        for k in range(i, j + 1):
            ranks[items[k][0]] = avg_rank / n
        i = j + 1
    return ranks


def _clean_returns_and_block(
    bars: List[DailyBar],
    split_threshold: float,
    block_days: int,
) -> Tuple[List[float], bool]:
    """Return daily returns (with split days zeroed) and whether symbol is currently blocked."""
    if len(bars) < 2:
        return [], False
    rets: List[float] = []
    split_flags: List[bool] = []
    for i in range(1, len(bars)):
        prev_c = bars[i - 1].close
        cur_c = bars[i].close
        if prev_c <= 0.0 or cur_c <= 0.0:
            rets.append(0.0)
            split_flags.append(False)
            continue
        r = cur_c / prev_c - 1.0
        if abs(r) >= split_threshold:
            rets.append(0.0)
            split_flags.append(True)
        else:
            rets.append(r)
            split_flags.append(False)
    blocked = any(split_flags[-block_days:]) if split_flags else False
    return rets, blocked


def _stock_3f_score_for_window(
    bars_map: Dict[str, List[DailyBar]],
    rets_map: Dict[str, List[float]],
    eligible: List[str],
    lookback: int,
) -> Dict[str, float]:
    min_p = max(5, int(lookback * 0.7))
    mom_raw: Dict[str, float] = {}
    dd_raw: Dict[str, float] = {}
    up_raw: Dict[str, float] = {}
    for sym in eligible:
        bars = bars_map[sym]
        rets = rets_map[sym]
        if len(rets) < min_p or len(bars) < min_p + 1:
            continue
        win_r = rets[-lookback:]
        win_c = [b.close for b in bars[-lookback:]]
        vol = max(_std(win_r), 0.004)
        cum_r = 1.0
        for r in win_r:
            cum_r *= (1.0 + r)
        cum_r -= 1.0
        mx = max(win_c)
        if mx <= 0.0:
            continue
        mom_raw[sym] = cum_r / vol
        dd_raw[sym] = bars[-1].close / mx - 1.0
        up_raw[sym] = sum(1 for r in win_r if r > 0.0) / len(win_r)

    common = sorted(set(mom_raw) & set(dd_raw) & set(up_raw))
    if len(common) < 6:
        return {}
    rm = _pct_ranks({s: mom_raw[s] for s in common})
    rd = _pct_ranks({s: dd_raw[s] for s in common})
    ru = _pct_ranks({s: up_raw[s] for s in common})
    return {s: rm[s] + rd[s] + ru[s] for s in common}


def compute_stock_a4_slice(
    bars_map: Dict[str, List[DailyBar]],
    cfg: StrategyConfig,
) -> Tuple[Dict[str, float], Dict[str, float], List[str]]:
    """Compute one single-phase A4 stock leg weight vector (gross = cfg.gross_leverage).

    Mature stocks (>= 15 bars) use 0.5 * score_3f(7d) + 0.5 * score_3f(14d).
    Newly listed liquid stocks (cfg.stock_min_bars <= len < 15, default 7) use pure
    score_3f(7d) fallback so fresh contracts (e.g. ANTUSDT, IGVUSDT, SOXXUSDT) onboard
    after 7 trading days without waiting 15 days.
    """
    rets_map: Dict[str, List[float]] = {}
    blocked: List[str] = []
    eligible: List[str] = []
    min_bars = max(6, int(cfg.stock_min_bars))

    for sym in cfg.stock_universe:
        if sym in LEVERAGED_BLACKLIST:
            continue
        bars = bars_map.get(sym)
        if not bars or len(bars) < min_bars:
            continue
        rets, is_blocked = _clean_returns_and_block(bars, cfg.split_ret_threshold, cfg.split_block_days)
        rets_map[sym] = rets
        if is_blocked:
            blocked.append(sym)
            continue
        v_win = [b.quote_vol for b in bars[-7:]]
        avg_v = sum(v_win) / len(v_win) if v_win else 0.0
        if avg_v < cfg.min_vol_usd:
            continue
        eligible.append(sym)

    s7 = _stock_3f_score_for_window(bars_map, rets_map, eligible, 7)
    s14 = _stock_3f_score_for_window(bars_map, rets_map, eligible, 14)
    common = sorted(s7.keys())
    if len(common) < 6:
        return {}, {}, blocked

    scores = {
        s: (0.5 * s7[s] + 0.5 * s14[s]) if s in s14 else s7[s]
        for s in common
    }
    ranked = sorted(common, key=lambda s: (-scores[s], s))
    k = max(2, int(round(len(ranked) * cfg.stock_top_frac)))
    k = min(k, len(ranked) // 2)
    longs = ranked[:k]
    shorts = ranked[-k:]

    def _inv_vol(sym: str) -> float:
        r = rets_map[sym][-14:]
        v = max(_std(r), 0.008)
        return 1.0 / v

    half_gross = cfg.gross_leverage / 2.0
    cap = cfg.stock_cap_frac * cfg.gross_leverage

    iv_l = {s: _inv_vol(s) for s in longs}
    iv_s = {s: _inv_vol(s) for s in shorts}
    sum_l = sum(iv_l.values()) or 1.0
    sum_s = sum(iv_s.values()) or 1.0

    w: Dict[str, float] = {}
    for s in longs:
        w[s] = min(half_gross * iv_l[s] / sum_l, cap)
    for s in shorts:
        w[s] = -min(half_gross * iv_s[s] / sum_s, cap)

    # Renormalize each leg after single-name cap so market neutrality holds
    pos_sum = sum(v for v in w.values() if v > 0.0)
    neg_sum = sum(-v for v in w.values() if v < 0.0)
    if pos_sum > 0.0:
        scale = half_gross / pos_sum
        for s in longs:
            w[s] = min(w[s] * scale, cap)
    if neg_sum > 0.0:
        scale = half_gross / neg_sum
        for s in shorts:
            w[s] = -min(-w[s] * scale, cap)

    return w, scores, sorted(blocked)


def compute_comm_ema_weights(
    bars_map: Dict[str, List[DailyBar]],
    cfg: StrategyConfig,
) -> Tuple[Dict[str, float], Dict[str, float]]:
    """Compute 4-core commodity S4 swing weights (EMA5/20 + 5d trailing exit + RSI extreme half-size)."""
    inv_vols: Dict[str, float] = {}
    signals: Dict[str, float] = {}

    for sym in cfg.comm_universe:
        bars = bars_map.get(sym)
        if not bars or len(bars) < max(cfg.ema_slow, 15):
            continue
        v_win = [b.quote_vol for b in bars[-7:]]
        avg_v = sum(v_win) / len(v_win) if v_win else 0.0
        if avg_v < cfg.min_vol_usd:
            continue
        rets, is_blocked = _clean_returns_and_block(bars, cfg.split_ret_threshold, cfg.split_block_days)
        if is_blocked or len(rets) < 7:
            continue
        closes = [b.close for b in bars]
        ef = _ema_series(closes, cfg.ema_fast)[-1]
        es = _ema_series(closes, cfg.ema_slow)[-1]
        rsi14 = _rsi_latest(closes, 14)
        cur = closes[-1]
        trail_n = max(2, cfg.comm_trail_days)
        prev_win = closes[-(trail_n + 1):-1]
        hi_n = max(prev_win) if prev_win else cur
        lo_n = min(prev_win) if prev_win else cur

        if ef > es:
            if cur < lo_n:
                sig = 0.0
            elif rsi14 >= cfg.comm_rsi_hi:
                sig = 0.5
            else:
                sig = 1.0
        else:
            if cur > hi_n:
                sig = 0.0
            elif rsi14 <= cfg.comm_rsi_lo:
                sig = -0.5
            else:
                sig = -1.0

        vol = max(_std(rets[-14:]), 0.005)
        inv_vols[sym] = 1.0 / vol
        signals[sym] = sig

    denom = sum(inv_vols.values())
    if denom <= 0.0:
        return {}, signals

    cap = cfg.comm_cap_frac * cfg.gross_leverage
    weights: Dict[str, float] = {}
    for sym, iv in inv_vols.items():
        sig = signals[sym]
        if abs(sig) < 1e-9:
            continue
        w = cfg.gross_leverage * (sig * iv) / denom
        weights[sym] = max(-cap, min(cap, w))
    return weights, signals


def compute_comm_4h_weights(
    bars_4h_map: Dict[str, List[DailyBar]],
    cfg: StrategyConfig,
) -> Tuple[Dict[str, float], Dict[str, float]]:
    """Compute 4-core commodity 4h fast-trend weights (EMA12/48 + 18-bar trailing exit + RSI half-size)."""
    inv_vols: Dict[str, float] = {}
    signals: Dict[str, float] = {}
    min_bars = max(cfg.comm_4h_ema_slow, 50)

    for sym in cfg.comm_universe:
        bars = bars_4h_map.get(sym)
        if not bars or len(bars) < min_bars:
            continue
        v_win = [b.quote_vol for b in bars[-42:]]
        avg_daily_v = (sum(v_win) / len(v_win)) * 6.0 if v_win else 0.0
        if avg_daily_v < cfg.min_vol_usd:
            continue
        rets, is_blocked = _clean_returns_and_block(bars, cfg.split_ret_threshold, cfg.split_block_days * 6)
        if is_blocked or len(rets) < 42:
            continue
        closes = [b.close for b in bars]
        ef = _ema_series(closes, cfg.comm_4h_ema_fast)[-1]
        es = _ema_series(closes, cfg.comm_4h_ema_slow)[-1]
        rsi14 = _rsi_latest(closes, 14)
        cur = closes[-1]
        trail_n = max(2, cfg.comm_4h_trail_bars)
        prev_win = closes[-(trail_n + 1):-1]
        hi_n = max(prev_win) if prev_win else cur
        lo_n = min(prev_win) if prev_win else cur

        if ef > es:
            if cur < lo_n:
                sig = 0.0
            elif rsi14 >= cfg.comm_4h_rsi_hi:
                sig = 0.5
            else:
                sig = 1.0
        else:
            if cur > hi_n:
                sig = 0.0
            elif rsi14 <= cfg.comm_4h_rsi_lo:
                sig = -0.5
            else:
                sig = -1.0

        vol = max(_std(rets[-84:]), 0.002)
        inv_vols[sym] = 1.0 / vol
        signals[sym] = sig

    denom = sum(inv_vols.values())
    if denom <= 0.0:
        return {}, signals

    cap = cfg.comm_cap_frac * cfg.gross_leverage
    weights: Dict[str, float] = {}
    for sym, iv in inv_vols.items():
        sig = signals[sym]
        if abs(sig) < 1e-9:
            continue
        w = cfg.gross_leverage * (sig * iv) / denom
        weights[sym] = max(-cap, min(cap, w))
    return weights, signals


def compute_vbounce_weights(
    bars_map: Dict[str, List[DailyBar]],
    cfg: StrategyConfig,
    prev_vbounce_positions: Optional[Dict[str, Dict[str, Any]]] = None,
) -> Tuple[Dict[str, float], Dict[str, Dict[str, Any]]]:
    """Compute oversold V-bounce long-only leg weights (RSI(2) <= 5 & 3d drop >= 5%).

    Exit any active slot when:
      (a) close >= MA(vbounce_ma_exit), OR
      (b) hold_days >= vbounce_max_hold_days, OR
      (c) return from entry_price <= -vbounce_stop_pct.
    """
    if cfg.vbounce_alloc <= 0.0 or cfg.vbounce_max_pos <= 0:
        return {}, {}

    universe = sorted(set(cfg.stock_universe) | set(cfg.comm_universe))
    min_bars = max(cfg.vbounce_ma_exit, cfg.vbounce_rsi_period + 1, 7)
    eligible_bars: Dict[str, List[DailyBar]] = {}

    for sym in universe:
        if sym in LEVERAGED_BLACKLIST:
            continue
        bars = bars_map.get(sym)
        if not bars or len(bars) < min_bars:
            continue
        _, is_blocked = _clean_returns_and_block(bars, cfg.split_ret_threshold, cfg.split_block_days)
        if is_blocked:
            continue
        v_win = [b.quote_vol for b in bars[-7:]]
        avg_v = sum(v_win) / len(v_win) if v_win else 0.0
        if avg_v < cfg.min_vol_usd:
            continue
        eligible_bars[sym] = bars

    next_positions: Dict[str, Dict[str, Any]] = {}
    exited_today: set = set()
    for sym, pos in (prev_vbounce_positions or {}).items():
        bars = eligible_bars.get(sym)
        if not bars:
            exited_today.add(sym)
            continue
        entry_px = float(pos.get("entry_price") or 0.0)
        if entry_px <= 0.0:
            exited_today.add(sym)
            continue
        hold_days = int(pos.get("hold_days") or 0) + 1
        closes = [b.close for b in bars]
        cur = closes[-1]
        ma_n = sum(closes[-cfg.vbounce_ma_exit:]) / cfg.vbounce_ma_exit
        ret_from_entry = cur / entry_px - 1.0
        if (
            cur >= ma_n
            or hold_days >= cfg.vbounce_max_hold_days
            or ret_from_entry <= -cfg.vbounce_stop_pct
        ):
            exited_today.add(sym)
            continue
        next_positions[sym] = {"entry_price": entry_px, "hold_days": hold_days}

    if len(next_positions) < cfg.vbounce_max_pos:
        candidates: List[Tuple[float, float, str, float]] = []
        for sym, bars in eligible_bars.items():
            if sym in next_positions or sym in exited_today:
                continue
            closes = [b.close for b in bars]
            if len(closes) < 4 or closes[-4] <= 0.0:
                continue
            if cfg.vbounce_trend_ma > 0:
                if len(closes) < cfg.vbounce_trend_ma:
                    continue
                trend_ma = sum(closes[-cfg.vbounce_trend_ma:]) / float(cfg.vbounce_trend_ma)
                if closes[-1] < trend_ma:
                    continue
            rsi2 = _rsi_latest(closes, cfg.vbounce_rsi_period)
            ret3 = closes[-1] / closes[-4] - 1.0
            if rsi2 <= cfg.vbounce_rsi_max and ret3 <= cfg.vbounce_drop3_min:
                candidates.append((rsi2, ret3, sym, closes[-1]))
        candidates.sort(key=lambda x: (x[0], x[1], x[2]))
        for _, _, sym, cur_close in candidates:
            if len(next_positions) >= cfg.vbounce_max_pos:
                break
            next_positions[sym] = {"entry_price": cur_close, "hold_days": 0}

    slot_w = cfg.gross_leverage / float(cfg.vbounce_max_pos)
    weights = {sym: slot_w for sym in sorted(next_positions)}
    return weights, next_positions


def _portfolio_ann_vol(
    weights: Dict[str, float],
    bars_map: Dict[str, List[DailyBar]],
    lookback: int = 14,
    split_threshold: float = 0.40,
) -> float:
    """Compute trailing realized annualized volatility of `weights` on closed daily bars."""
    ret_by_day: Dict[int, float] = {}
    for sym, w in weights.items():
        if abs(w) < 1e-9:
            continue
        bars = bars_map.get(sym)
        if not bars or len(bars) < 8:
            continue
        sub = bars[-(lookback + 1):]
        for i in range(1, len(sub)):
            c0, c1 = sub[i - 1].close, sub[i].close
            if c0 > 0.0 and c1 > 0.0:
                r = c1 / c0 - 1.0
                if abs(r) >= split_threshold:
                    r = 0.0
                t = sub[i].open_time_ms
                ret_by_day[t] = ret_by_day.get(t, 0.0) + w * r
    series = [v for _, v in sorted(ret_by_day.items())[-lookback:]]
    if len(series) < 7:
        return 0.0
    return _std(series) * math.sqrt(365.0)


def compute_m3b_target(
    bars_map: Dict[str, List[DailyBar]],
    cfg: StrategyConfig,
    prev_stock_phases: Optional[Dict[str, Dict[str, float]]] = None,
    day_index: Optional[int] = None,
    prev_vbounce_positions: Optional[Dict[str, Dict[str, Any]]] = None,
    bars_4h_map: Optional[Dict[str, List[DailyBar]]] = None,
    freeze_daily_state: bool = False,
) -> TargetPortfolio:
    """Compute combined M3b 3-leg target portfolio (strictly causal on closed daily & 4h bars).

    For 2-phase slicing on the US Stock A4 leg:
    - If `prev_stock_phases` is None/empty (cold start), both p0 and p1 are initialized to today's A4 slice.
    - Otherwise, phase `day_index % 2` updates to today's fresh A4 slice while the other phase holds its prior slice.
    - When `freeze_daily_state=True` (intraday 4h commodity-only refresh), `stock_phases` and `vbounce_positions`
      stay frozen while the 4h commodity sub-leg updates.
    """
    asof_ms = 0
    for bars in bars_map.values():
        if bars:
            asof_ms = max(asof_ms, bars[-1].open_time_ms)
    if day_index is None:
        day_index = asof_ms // 86_400_000

    fresh_stock, stock_scores, blocked = compute_stock_a4_slice(bars_map, cfg)
    phases: Dict[str, Dict[str, float]] = {"p0": {}, "p1": {}}
    if prev_stock_phases and (prev_stock_phases.get("p0") or prev_stock_phases.get("p1")):
        phases["p0"] = dict(prev_stock_phases.get("p0") or fresh_stock)
        phases["p1"] = dict(prev_stock_phases.get("p1") or fresh_stock)
        if not freeze_daily_state:
            active_key = f"p{day_index % 2}"
            phases[active_key] = dict(fresh_stock)
    else:
        phases["p0"] = dict(fresh_stock)
        phases["p1"] = dict(fresh_stock)

    # Average the two stock phases
    stock_syms = sorted(set(phases["p0"]) | set(phases["p1"]))
    stock_w: Dict[str, float] = {}
    for s in stock_syms:
        if s in blocked:
            continue
        v = 0.5 * phases["p0"].get(s, 0.0) + 0.5 * phases["p1"].get(s, 0.0)
        if abs(v) > 1e-6:
            stock_w[s] = v

    comm_daily_w, comm_signals = compute_comm_ema_weights(bars_map, cfg)
    f4 = max(0.0, min(1.0, float(cfg.comm_4h_frac)))
    if bars_4h_map and f4 > 0.0:
        comm_4h_w, comm_4h_sigs = compute_comm_4h_weights(bars_4h_map, cfg)
        if comm_4h_sigs:
            comm_w: Dict[str, float] = {}
            for s in sorted(set(comm_daily_w) | set(comm_4h_w)):
                v = (1.0 - f4) * comm_daily_w.get(s, 0.0) + f4 * comm_4h_w.get(s, 0.0)
                if abs(v) > 1e-6:
                    comm_w[s] = v
            for s in sorted(set(comm_signals) | set(comm_4h_sigs)):
                comm_signals[s] = (1.0 - f4) * comm_signals.get(s, 0.0) + f4 * comm_4h_sigs.get(s, 0.0)
        else:
            comm_w = comm_daily_w
    else:
        comm_w = comm_daily_w

    if freeze_daily_state and prev_vbounce_positions is not None:
        next_vbounce_pos = {s: dict(p) for s, p in prev_vbounce_positions.items() if s not in blocked}
        slot_w = (cfg.gross_leverage / float(cfg.vbounce_max_pos)) if (cfg.vbounce_alloc > 0.0 and cfg.vbounce_max_pos > 0) else 0.0
        vbounce_w = {s: slot_w for s in sorted(next_vbounce_pos)} if slot_w > 0.0 else {}
    else:
        vbounce_w, next_vbounce_pos = compute_vbounce_weights(bars_map, cfg, prev_vbounce_positions)

    # Sweep idle vbounce allocation proportionally into stock_a4 & comm_ema
    eff_stock_alloc = cfg.stock_alloc
    eff_comm_alloc = cfg.comm_alloc
    if cfg.vbounce_cash_sweep and cfg.vbounce_alloc > 0.0 and cfg.vbounce_max_pos > 0:
        used_frac = min(1.0, len(vbounce_w) / float(cfg.vbounce_max_pos))
        idle_alloc = cfg.vbounce_alloc * (1.0 - used_frac)
        base_sum = eff_stock_alloc + eff_comm_alloc
        if base_sum > 1e-9:
            eff_stock_alloc += idle_alloc * (eff_stock_alloc / base_sum)
            eff_comm_alloc += idle_alloc * (eff_comm_alloc / base_sum)

    combined: Dict[str, float] = {}
    legs: Dict[str, str] = {}
    for s, w in stock_w.items():
        combined[s] = combined.get(s, 0.0) + eff_stock_alloc * w
        legs[s] = "stock_a4"
    for s, w in comm_w.items():
        combined[s] = combined.get(s, 0.0) + eff_comm_alloc * w
        legs[s] = "comm_ema"
    for s, w in vbounce_w.items():
        combined[s] = combined.get(s, 0.0) + cfg.vbounce_alloc * w
        legs[s] = f"{legs[s]}+vbounce" if s in legs else "vbounce"

    # Downside-only portfolio-level volatility targeting (never scales above 1.0x)
    if cfg.vol_target_ann > 0.0 and combined:
        pvol = _portfolio_ann_vol(
            combined,
            bars_map,
            lookback=max(7, int(cfg.vol_target_lookback)),
            split_threshold=cfg.split_ret_threshold,
        )
        if pvol > cfg.vol_target_ann:
            scale = max(cfg.vol_target_min_scale, min(1.0, cfg.vol_target_ann / pvol))
            for s in combined:
                combined[s] *= scale

    for s in [k for k, v in combined.items() if abs(v) <= 1e-6]:
        del combined[s]
        legs.pop(s, None)

    return TargetPortfolio(
        asof_day_ms=asof_ms,
        weights=combined,
        legs=legs,
        blocked_symbols=blocked,
        stock_scores=stock_scores,
        comm_signals=comm_signals,
        stock_phase_weights=phases,
        vbounce_positions=next_vbounce_pos,
    )
