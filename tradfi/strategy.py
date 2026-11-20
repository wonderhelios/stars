"""Pure-stdlib signal & portfolio engine for Binance TradFi M3b 4-Core Dual-Leg strategy.

Architecture (verified causal OOS on 2026-04 ~ 2026-11):
- Leg 1 (50% capital): US Stock 1x Perps Cross-Sectional 3-Factor (A4)
    * Universe: 18 1x US stock/ETF perps (excludes 3x leveraged SOXL/SOXS).
    * Corporate Action Guard: any symbol with |daily_ret| >= 40% in the last 14 closed bars
      (e.g. SNDK 8-for-1 stock split) is masked out of signal eligibility for 14 days.
    * Score: 0.5 * score_3f(7d) + 0.5 * score_3f(14d), where
      score_3f(L) = pct_rank(cum_ret_L / vol_L) + pct_rank(close / max_L - 1) + pct_rank(up_frac_L).
    * Liquidity filter: 7-day average quote volume >= $5,000,000.
    * Selection: top 30% Long, bottom 30% Short (min 2 per side, requires >= 6 eligible stocks).
    * Weighting: within-leg inverse-volatility (14d std), single-name cap = 30% of leg gross.
    * 2-Phase Slicing: even-day slice + odd-day slice averaged 50/50 to eliminate phase luck.
- Leg 2 (50% capital): 4 High-Liquidity Core Commodities Time-Series Trend (EMA5/20)
    * Universe: XAUUSDT (Gold), XAGUSDT (Silver), CLUSDT (WTI Crude Oil), NATGASUSDT (NatGas).
    * Signal: EMA(5) > EMA(20) -> Long (+1), else Short (-1).
    * Weighting: inverse-volatility (14d std), single-commodity cap = 40% of leg gross.
"""
from __future__ import annotations

import math
from dataclasses import dataclass, field
from typing import Dict, List, Optional, Tuple

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
    gross_leverage: float = 2.0
    stock_alloc: float = 0.50
    comm_alloc: float = 0.50
    min_vol_usd: float = 5_000_000.0
    split_ret_threshold: float = 0.40
    split_block_days: int = 14
    stock_top_frac: float = 0.30
    stock_cap_frac: float = 0.30
    comm_cap_frac: float = 0.40
    ema_fast: int = 5
    ema_slow: int = 20
    stock_universe: List[str] = field(default_factory=lambda: list(DEFAULT_STOCK_UNIVERSE))
    comm_universe: List[str] = field(default_factory=lambda: list(DEFAULT_COMMODITY_UNIVERSE))


@dataclass
class TargetPortfolio:
    asof_day_ms: int
    weights: Dict[str, float]  # symbol -> target weight as fraction of TOTAL account equity
    legs: Dict[str, str]       # symbol -> "stock_a4" | "comm_ema"
    blocked_symbols: List[str]
    stock_scores: Dict[str, float]
    comm_signals: Dict[str, int]
    stock_phase_weights: Dict[str, Dict[str, float]]  # {"p0": {...}, "p1": {...}}


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
    """Compute one single-phase A4 stock leg weight vector (gross = cfg.gross_leverage)."""
    rets_map: Dict[str, List[float]] = {}
    blocked: List[str] = []
    eligible: List[str] = []

    for sym in cfg.stock_universe:
        if sym in LEVERAGED_BLACKLIST:
            continue
        bars = bars_map.get(sym)
        if not bars or len(bars) < 15:
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
    common = sorted(set(s7) & set(s14))
    if len(common) < 6:
        return {}, {}, blocked

    scores = {s: 0.5 * s7[s] + 0.5 * s14[s] for s in common}
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
) -> Tuple[Dict[str, float], Dict[str, int]]:
    """Compute 4-core commodity EMA(5)/EMA(20) inverse-vol trend weights (gross = cfg.gross_leverage)."""
    raw: Dict[str, float] = {}
    signals: Dict[str, int] = {}

    for sym in cfg.comm_universe:
        bars = bars_map.get(sym)
        if not bars or len(bars) < max(cfg.ema_slow, 15):
            continue
        v_win = [b.quote_vol for b in bars[-7:]]
        avg_v = sum(v_win) / len(v_win) if v_win else 0.0
        if avg_v < cfg.min_vol_usd:
            continue
        closes = [b.close for b in bars]
        ef = _ema_series(closes, cfg.ema_fast)[-1]
        es = _ema_series(closes, cfg.ema_slow)[-1]
        sig = 1 if ef > es else -1
        rets, is_blocked = _clean_returns_and_block(bars, cfg.split_ret_threshold, cfg.split_block_days)
        if is_blocked or len(rets) < 7:
            continue
        vol = max(_std(rets[-14:]), 0.005)
        signals[sym] = sig
        raw[sym] = sig / vol

    tot = sum(abs(v) for v in raw.values())
    if tot <= 0.0:
        return {}, signals

    cap = cfg.comm_cap_frac * cfg.gross_leverage
    weights: Dict[str, float] = {}
    for sym, val in raw.items():
        w = cfg.gross_leverage * val / tot
        weights[sym] = max(-cap, min(cap, w))
    return weights, signals


def compute_m3b_target(
    bars_map: Dict[str, List[DailyBar]],
    cfg: StrategyConfig,
    prev_stock_phases: Optional[Dict[str, Dict[str, float]]] = None,
    day_index: Optional[int] = None,
) -> TargetPortfolio:
    """Compute combined M3b 4-core dual-leg target portfolio (strictly causal on closed daily bars).

    For 2-phase slicing on the US Stock A4 leg:
    - If `prev_stock_phases` is None/empty (cold start), both p0 and p1 are initialized to today's A4 slice.
    - Otherwise, phase `day_index % 2` updates to today's fresh A4 slice while the other phase holds its prior slice.
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

    comm_w, comm_signals = compute_comm_ema_weights(bars_map, cfg)

    combined: Dict[str, float] = {}
    legs: Dict[str, str] = {}
    for s, w in stock_w.items():
        combined[s] = combined.get(s, 0.0) + cfg.stock_alloc * w
        legs[s] = "stock_a4"
    for s, w in comm_w.items():
        combined[s] = combined.get(s, 0.0) + cfg.comm_alloc * w
        legs[s] = "comm_ema"

    return TargetPortfolio(
        asof_day_ms=asof_ms,
        weights=combined,
        legs=legs,
        blocked_symbols=blocked,
        stock_scores=stock_scores,
        comm_signals=comm_signals,
        stock_phase_weights=phases,
    )
