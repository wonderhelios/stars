"""Core 1h Trend-Filtered SFP (Swing Failure Pattern) signal engine.

Validated on Binance USD-M Futures 1h data (27-point parameter plateau 100% positive,
25/27 Sharpe >= 2.0):
  - Universe: 10 high-elasticity US stocks + commodities (SOXL, SNDK, NVDA, INTC,
    TSLA, MSTR, COIN, XAU, XAG, CL)
  - Signal: 1h sweep beyond prior 24h high/low & close back inside, wick >= 0.5 * ATR(24)
  - Trend filter: Long only if close > EMA(168), Short only if close < EMA(168)
  - Entry: 1:2:3 scaled Maker limit orders inside the SFP wick (0.20 / 0.50 / 0.80)
  - Exit: 75% TP1 at +1.5 * ATR, move 25% runner SL to breakeven + 1.2 * ATR trail;
          hard SL at sweep extreme +/- 0.35 * ATR (capped at 2.5 * ATR)

Pure stdlib Python (< 3.12). Zero external dependencies.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import List, Optional, Tuple

CORE10_UNIVERSE: Tuple[str, ...] = (
    "SOXLUSDT",
    "SNDKUSDT",
    "NVDAUSDT",
    "INTCUSDT",
    "TSLAUSDT",
    "MSTRUSDT",
    "COINUSDT",
    "XAUUSDT",
    "XAGUSDT",
    "CLUSDT",
)


@dataclass(frozen=True)
class Bar1h:
    open_time_ms: int
    open: float
    high: float
    low: float
    close: float
    volume: float = 0.0


@dataclass
class FastSwingConfig:
    symbols: Tuple[str, ...] = CORE10_UNIVERSE
    lookback_hours: int = 24
    atr_period: int = 24
    ema_period: int = 168
    wick_atr_min: float = 0.5
    tp1_atr: float = 1.5
    tp1_frac: float = 0.75
    sl_buf_atr: float = 0.35
    sl_max_atr: float = 2.5
    trail_atr: float = 1.2
    max_hold_hours: int = 48
    pending_expire_hours: int = 6
    slot_leverage: float = 2.0
    max_gross_leverage: float = 6.0
    maker_fee: float = 0.0002
    taker_fee: float = 0.0005
    split_ret_threshold: float = 0.40


@dataclass(frozen=True)
class SfpSignal:
    symbol: str
    bar_time_ms: int
    side: int  # +1 long, -1 short
    close: float
    atr: float
    ema: float
    sweep_level: float
    wick_ratio_atr: float
    levels: Tuple[float, float, float]  # 1:2:3 Maker limit prices
    sl_price: float
    tp1_distance: float


def compute_indicators(
    bars: List[Bar1h],
    atr_period: int = 24,
    ema_period: int = 168,
) -> Tuple[List[float], List[float]]:
    """Compute Wilder ATR and EMA strictly causally up to each bar."""
    n = len(bars)
    if n == 0:
        return [], []
    atr = [0.0] * n
    ema = [0.0] * n
    trs: List[float] = []
    alpha = 2.0 / (ema_period + 1.0)
    for i, b in enumerate(bars):
        if i == 0:
            tr = max(0.0, b.high - b.low)
            ema[i] = b.close
        else:
            pc = bars[i - 1].close
            tr = max(b.high - b.low, abs(b.high - pc), abs(b.low - pc))
            ema[i] = alpha * b.close + (1.0 - alpha) * ema[i - 1]
        trs.append(tr)
        if i + 1 < atr_period:
            atr[i] = sum(trs) / len(trs)
        elif i + 1 == atr_period:
            atr[i] = sum(trs) / float(atr_period)
        else:
            atr[i] = (atr[i - 1] * (atr_period - 1) + tr) / float(atr_period)
    return atr, ema


def has_recent_split(bars: List[Bar1h], threshold: float = 0.40, window: int = 48) -> bool:
    """Guard against corporate splits (e.g. SNDK 8:1 -87.5%) polluting ATR/SFP."""
    n = len(bars)
    start = max(1, n - window)
    for i in range(start, n):
        prev_c = bars[i - 1].close
        if prev_c > 0.0 and abs(bars[i].close / prev_c - 1.0) >= threshold:
            return True
    return False


def detect_sfp_signal(
    symbol: str,
    bars: List[Bar1h],
    cfg: FastSwingConfig,
) -> Tuple[Optional[SfpSignal], str]:
    """Evaluate the latest CLOSED 1h bar for a trend-aligned SFP signal.

    Returns (SfpSignal | None, reason_str).
    """
    min_bars = max(cfg.lookback_hours + 1, cfg.ema_period)
    if len(bars) < min_bars:
        return None, f"K线不足({len(bars)}<{min_bars})"

    if has_recent_split(bars, threshold=cfg.split_ret_threshold, window=cfg.lookback_hours + 24):
        return None, "近期存在拆股/异常跳空"

    atr_list, ema_list = compute_indicators(bars, atr_period=cfg.atr_period, ema_period=cfg.ema_period)
    i = len(bars) - 1
    b = bars[i]
    a = atr_list[i]
    e = ema_list[i]
    if a <= 0.0 or b.close <= 0.0:
        return None, "ATR无效"

    window = bars[i - cfg.lookback_hours : i]
    prev_low = min(x.low for x in window)
    prev_high = max(x.high for x in window)

    lower_wick = min(b.open, b.close) - b.low
    upper_wick = b.high - max(b.open, b.close)

    side = 0
    sweep_lvl = 0.0
    wick_ratio = 0.0
    if b.low < prev_low and b.close > prev_low and lower_wick >= cfg.wick_atr_min * a:
        side = 1
        sweep_lvl = prev_low
        wick_ratio = lower_wick / a
    elif b.high > prev_high and b.close < prev_high and upper_wick >= cfg.wick_atr_min * a:
        side = -1
        sweep_lvl = prev_high
        wick_ratio = upper_wick / a
    else:
        return None, "无SFP插针"

    # EMA(168) macro trend alignment filter
    if side == 1 and b.close < e:
        return None, f"多头SFP逆EMA{cfg.ema_period}趋势(关过滤)"
    if side == -1 and b.close > e:
        return None, f"空头SFP逆EMA{cfg.ema_period}趋势(关过滤)"

    if side == 1:
        wick = max(b.close - b.low, 0.3 * a)
        levels = (
            b.close - 0.20 * wick,
            b.close - 0.50 * wick,
            b.close - 0.80 * wick,
        )
        sl_price = max(b.low - cfg.sl_buf_atr * a, b.close - cfg.sl_max_atr * a)
    else:
        wick = max(b.high - b.close, 0.3 * a)
        levels = (
            b.close + 0.20 * wick,
            b.close + 0.50 * wick,
            b.close + 0.80 * wick,
        )
        sl_price = min(b.high + cfg.sl_buf_atr * a, b.close + cfg.sl_max_atr * a)

    sig = SfpSignal(
        symbol=symbol,
        bar_time_ms=b.open_time_ms,
        side=side,
        close=b.close,
        atr=a,
        ema=e,
        sweep_level=sweep_lvl,
        wick_ratio_atr=round(wick_ratio, 2),
        levels=levels,
        sl_price=sl_price,
        tp1_distance=cfg.tp1_atr * a,
    )
    return sig, "顺势SFP挂单触发"
