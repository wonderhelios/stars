//! Contract classification, strictly causal indicators, and Sleeve A / B signal detectors.

use super::{
    Bar1h, BtcRegime, LimitOcoConfig, SignalOrder, COMMODITY_SYMBOLS, HOUR_MS,
    STABLE_OR_FX_EXCLUDED, US_STOCK_SYMBOLS,
};
use serde_json::Value;

// ---------------------------------------------------------------------------
// 合约分类 + 严格因果技术指标（零未来函数）
// ---------------------------------------------------------------------------

pub fn classify_binance_contract(sym_info: &Value) -> &'static str {
    let symbol = sym_info
        .get("symbol")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_uppercase();
    if !symbol.ends_with("USDT") {
        return "excluded";
    }
    if sym_info.get("status").and_then(|v| v.as_str()) != Some("TRADING") {
        return "excluded";
    }
    if sym_info.get("contractType").and_then(|v| v.as_str()) != Some("PERPETUAL") {
        return "excluded";
    }
    if sym_info.get("quoteAsset").and_then(|v| v.as_str()) != Some("USDT") {
        return "excluded";
    }
    if STABLE_OR_FX_EXCLUDED.contains(&symbol.as_str()) {
        return "excluded";
    }
    if COMMODITY_SYMBOLS.contains(&symbol.as_str()) {
        return "commodity";
    }
    if US_STOCK_SYMBOLS.contains(&symbol.as_str()) {
        return "stock";
    }
    if let Some(arr) = sym_info.get("underlyingSubType").and_then(|v| v.as_array()) {
        for item in arr {
            let up = item.as_str().unwrap_or("").to_uppercase();
            if up.contains("COMMODITY") || up.contains("METAL") || up.contains("ENERGY") || up.contains("PRECIOUS") {
                return "commodity";
            }
            if up.contains("EQUITY") || up.contains("STOCK") || up.contains("TRADFI") || up.contains("ETF") {
                return "stock";
            }
        }
    }
    let utype = sym_info
        .get("underlyingType")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_uppercase();
    if utype == "STOCK" || utype == "EQUITY" || utype == "TRADFI" {
        return "stock";
    }
    "crypto"
}

pub struct SeriesIndicators {
    pub ema168: Vec<f64>,
    pub ema336: Vec<f64>,
    pub atr24: Vec<f64>,
    pub rsi2_d: Vec<f64>,
    pub avg_qv24: Vec<f64>,
    pub avg_daily_qv7: Vec<f64>,
    pub split_block: Vec<bool>,
    pub last_funding: Vec<f64>,
}

pub(super) fn ema_vec(vals: &[f64], span: usize) -> Vec<f64> {
    let n = vals.len();
    if n == 0 {
        return Vec::new();
    }
    let mut out = vec![0.0; n];
    let alpha = 2.0 / (span as f64 + 1.0);
    let one_m = 1.0 - alpha;
    let mut prev = vals[0];
    out[0] = prev;
    for i in 1..n {
        prev = alpha * vals[i] + one_m * prev;
        out[i] = prev;
    }
    out
}

fn wilder_atr_vec(bars: &[Bar1h], period: usize) -> Vec<f64> {
    let n = bars.len();
    if n == 0 {
        return Vec::new();
    }
    let mut tr = vec![0.0; n];
    tr[0] = bars[0].high - bars[0].low;
    for i in 1..n {
        let pc = bars[i - 1].close;
        let b = &bars[i];
        tr[i] = (b.high - b.low).max((b.high - pc).abs()).max((b.low - pc).abs());
    }
    let mut atr = vec![0.0; n];
    let mut running = 0.0;
    let lim = n.min(period);
    for i in 0..lim {
        running += tr[i];
        atr[i] = running / ((i + 1) as f64);
    }
    if n > period {
        let inv = 1.0 / (period as f64);
        let keep = ((period - 1) as f64) * inv;
        let mut prev = atr[period - 1];
        for i in period..n {
            prev = prev * keep + tr[i] * inv;
            atr[i] = prev;
        }
    }
    atr
}

pub(super) fn rolling_mean_vec(vals: &[f64], win: usize) -> Vec<f64> {
    let n = vals.len();
    let mut out = vec![0.0; n];
    let mut acc = 0.0;
    for i in 0..n {
        acc += vals[i];
        if i >= win {
            acc -= vals[i - win];
            out[i] = acc / (win as f64);
        } else {
            out[i] = acc / ((i + 1) as f64);
        }
    }
    out
}

fn daily_rsi2_causal_vec(closes: &[f64]) -> Vec<f64> {
    let n = closes.len();
    let mut out = vec![50.0; n];
    if n < 72 {
        return out;
    }
    for i in 48..n {
        let d1 = closes[i] - closes[i - 24];
        let d2 = closes[i - 24] - closes[i - 48];
        let up = d1.max(0.0) + d2.max(0.0);
        let dn = (-d1).max(0.0) + (-d2).max(0.0);
        let tot = up + dn;
        out[i] = if tot > 1e-12 { 100.0 * up / tot } else { 50.0 };
    }
    out
}

/// Strictly causal funding alignment: latest settled rate with `fund_ts < open_time_ms + HOUR_MS`.
pub(super) fn align_last_funding_strict(open_times: &[i64], funding_events: &[(i64, f64)]) -> Vec<f64> {
    let n = open_times.len();
    let mut out = vec![0.0; n];
    if funding_events.is_empty() || n == 0 {
        return out;
    }
    let mut ev = funding_events.to_vec();
    ev.sort_by_key(|x| x.0);
    for (i, &ot) in open_times.iter().enumerate() {
        let bar_close = ot + HOUR_MS;
        let idx = ev.partition_point(|x| x.0 < bar_close);
        if idx > 0 {
            out[i] = ev[idx - 1].1;
        }
    }
    out
}

pub fn compute_indicators(
    bars: &[Bar1h],
    funding_events: &[(i64, f64)],
    split_jump_pct: f64,
    split_block_hours: usize,
) -> SeriesIndicators {
    let n = bars.len();
    let closes: Vec<f64> = bars.iter().map(|b| b.close).collect();
    let qvs: Vec<f64> = bars.iter().map(|b| b.quote_volume).collect();
    let ots: Vec<i64> = bars.iter().map(|b| b.open_time_ms).collect();

    let ema168 = ema_vec(&closes, 168);
    let ema336 = ema_vec(&closes, 336);
    let atr24 = wilder_atr_vec(bars, 24);
    let rsi2_d = daily_rsi2_causal_vec(&closes);
    let avg_qv24 = rolling_mean_vec(&qvs, 24);
    let avg_daily_qv7: Vec<f64> = rolling_mean_vec(&qvs, 168).into_iter().map(|v| v * 24.0).collect();

    let mut jump = vec![0usize; n];
    for i in 1..n {
        let pc = closes[i - 1];
        if pc > 1e-12 && (closes[i] / pc - 1.0).abs() >= split_jump_pct {
            jump[i] = 1;
        }
    }
    let mut split_block = vec![false; n];
    let mut win_sum = 0usize;
    for i in 0..n {
        win_sum += jump[i];
        if i >= split_block_hours {
            win_sum -= jump[i - split_block_hours];
        }
        split_block[i] = win_sum > 0;
    }

    let last_funding = align_last_funding_strict(&ots, funding_events);
    SeriesIndicators {
        ema168,
        ema336,
        atr24,
        rsi2_d,
        avg_qv24,
        avg_daily_qv7,
        split_block,
        last_funding,
    }
}

pub fn detect_sleeve_a(
    symbol: &str,
    bars: &[Bar1h],
    ind: &SeriesIndicators,
    idx: usize,
    btc: &BtcRegime,
    cfg: &LimitOcoConfig,
) -> (Option<SignalOrder>, &'static str) {
    const LOOKBACK: usize = 168;
    const LIMIT_ATR: f64 = 0.15;
    const SL_ATR: f64 = 2.8;
    const TP_RR: f64 = 1.2;
    const VOL_MULT: f64 = 1.3;

    if idx < 340 || idx >= bars.len() {
        return (None, "warmup");
    }
    if ind.split_block[idx] {
        return (None, "split_block");
    }
    let atr = ind.atr24[idx];
    let cl = bars[idx].close;
    let qv = bars[idx].quote_volume;
    let avg_qv = ind.avg_qv24[idx - 1];
    if atr <= 0.0 || cl <= 0.0 || avg_qv <= 0.0 {
        return (None, "invalid_bar");
    }

    let mut prev_hi = f64::NEG_INFINITY;
    let mut prev_lo = f64::INFINITY;
    for b in &bars[(idx - LOOKBACK)..idx] {
        if b.high > prev_hi {
            prev_hi = b.high;
        }
        if b.low < prev_lo {
            prev_lo = b.low;
        }
    }
    let prev_cl = bars[idx - 1].close;
    let ema168 = ind.ema168[idx];
    let ema336 = ind.ema336[idx];

    let side: i8 = if cl > prev_hi && prev_cl <= prev_hi && cl > ema168 && cl > ema336 {
        1
    } else if cl < prev_lo && prev_cl >= prev_lo && cl < ema168 {
        -1
    } else {
        0
    };
    if side == 0 {
        return (None, "no_cross");
    }
    if qv < VOL_MULT * avg_qv {
        return (None, "a_vol_blocked");
    }
    if side == 1 && !btc.bull_ok {
        return (None, "a_btc_blocked");
    }
    if side == -1 && !btc.short_ok {
        return (None, "a_btc_blocked");
    }
    let fr = ind.last_funding[idx];
    if side == 1 && fr > 0.00005 {
        return (None, "a_fund_blocked");
    }
    if side == -1 && fr < 0.0 {
        return (None, "a_fund_blocked");
    }

    let lim = if side == 1 { cl - LIMIT_ATR * atr } else { cl + LIMIT_ATR * atr };
    if (side == 1 && lim >= cl * 0.9998) || (side == -1 && lim <= cl * 1.0002) {
        return (None, "a_passive_rejected");
    }
    let r = SL_ATR * atr;
    let sl = if side == 1 { lim - r } else { lim + r };
    let tp = if side == 1 { lim + TP_RR * r } else { lim - TP_RR * r };
    let sl_pct = (lim - sl).abs() / lim;
    if sl_pct < cfg.min_sl_pct || sl_pct > cfg.max_sl_pct {
        return (None, "a_sl_rejected");
    }
    let tp_pct = (tp - lim).abs() / lim;
    let vol_ratio = qv / avg_qv;
    let tag = if side == 1 { "7d向上放量突破" } else { "7d向下放量跌破" };
    (
        Some(SignalOrder {
            sleeve: "A".into(),
            symbol: symbol.into(),
            side,
            limit_px: lim,
            sl_px: sl,
            tp_px: tp,
            expire_bars: 12,
            max_hold_bars: 168,
            score: vol_ratio,
            sl_pct,
            tp_pct,
            signal_close: cl,
            atr24: atr,
            funding_rate: fr,
            reason: format!("{tag} · 量比{vol_ratio:.2}x · 费率{:+.2}bp", fr * 10_000.0),
        }),
        "ok",
    )
}

pub fn detect_sleeve_b(
    symbol: &str,
    bars: &[Bar1h],
    ind: &SeriesIndicators,
    idx: usize,
    cfg: &LimitOcoConfig,
) -> (Option<SignalOrder>, &'static str) {
    const DROP_HOURS: usize = 72;
    const RIP_PCT: f64 = 0.06;
    const RSI2_MIN: f64 = 90.0;
    const LIMIT_ATR: f64 = 0.20;
    const SL_ATR: f64 = 2.7;
    const TP_RR: f64 = 0.9;

    if idx < 340 || idx >= bars.len() {
        return (None, "warmup");
    }
    if ind.split_block[idx] {
        return (None, "split_block");
    }
    let atr = ind.atr24[idx];
    let cl = bars[idx].close;
    let base_cl = bars[idx - DROP_HOURS].close;
    if atr <= 0.0 || cl <= 0.0 || base_cl <= 1e-12 {
        return (None, "invalid_bar");
    }
    let ret72 = cl / base_cl - 1.0;
    if ret72 < RIP_PCT || cl >= ind.ema336[idx] {
        return (None, "no_rip");
    }
    let rsi2 = ind.rsi2_d[idx];
    if rsi2 < RSI2_MIN {
        return (None, "b_rsi_blocked");
    }
    let fr = ind.last_funding[idx];
    if fr < -0.0002 {
        return (None, "b_fund_blocked");
    }
    let lim = cl + LIMIT_ATR * atr;
    if lim <= cl * 1.0002 {
        return (None, "b_passive_rejected");
    }
    let r = SL_ATR * atr;
    let sl = lim + r;
    let tp = lim - TP_RR * r;
    let sl_pct = (lim - sl).abs() / lim;
    if sl_pct < cfg.min_sl_pct || sl_pct > cfg.max_sl_pct {
        return (None, "b_sl_rejected");
    }
    let tp_pct = (tp - lim).abs() / lim;
    (
        Some(SignalOrder {
            sleeve: "B".into(),
            symbol: symbol.into(),
            side: -1,
            limit_px: lim,
            sl_px: sl,
            tp_px: tp,
            expire_bars: 12,
            max_hold_bars: 96,
            score: ret72.abs(),
            sl_pct,
            tp_pct,
            signal_close: cl,
            atr24: atr,
            funding_rate: fr,
            reason: format!(
                "空头趋势3日急拉+{:.1}% · 日RSI2={:.0} · 费率{:+.2}bp",
                ret72 * 100.0,
                rsi2,
                fr * 10_000.0
            ),
        }),
        "ok",
    )
}

