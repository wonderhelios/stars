//! 策略组合分析：日频动量 vs 4 小时反转的日收益序列、相关性、组合效果。
//!
//! 两套信号的计算口径与各自独立回测保持一致，且成交量窗口严格只用「当时之前」
//! 的数据（避免前视）。默认只在有请求时计算并缓存。

use crate::hl::Candle;
use crate::momentum::PanelEntry;
use serde::Serialize;
use std::collections::BTreeMap;

const DAY_MS: i64 = 86_400_000;

#[derive(Clone, Serialize)]
pub struct Series {
    pub name: String,
    /// 按 UTC 日期对齐的日收益（小数）
    pub dates: Vec<i64>,
    pub rets: Vec<f64>,
    pub ann_pct: f64,
    pub vol_pct: f64,
    pub sharpe: f64,
    pub t: f64,
    pub cum_pct: f64,
}

fn summarize(name: &str, map: BTreeMap<i64, f64>) -> Series {
    let dates: Vec<i64> = map.keys().copied().collect();
    let rets: Vec<f64> = map.values().copied().collect();
    let n = rets.len();
    if n < 10 {
        return Series {
            name: name.into(),
            dates,
            rets,
            ann_pct: 0.0,
            vol_pct: 0.0,
            sharpe: 0.0,
            t: 0.0,
            cum_pct: 0.0,
        };
    }
    let m = rets.iter().sum::<f64>() / n as f64;
    let var = rets.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n as f64 - 1.0);
    let sd = var.sqrt();
    let cum = rets.iter().fold(1.0, |acc, r| acc * (1.0 + r));
    Series {
        name: name.into(),
        dates,
        rets,
        ann_pct: m * 365.0 * 100.0,
        vol_pct: sd * 365.0_f64.sqrt() * 100.0,
        sharpe: if sd > 0.0 { m / sd * 365.0_f64.sqrt() } else { 0.0 },
        t: if sd > 0.0 { m / (sd / (n as f64).sqrt()) } else { 0.0 },
        cum_pct: (cum - 1.0) * 100.0,
    }
}

/// 日频横截面动量：14 天回看、做多前 20%、做空后 20%、等权、市场中性。
/// 成交额过滤用「滚动 30 天」，严格截至当日。
pub fn momentum_series(panel: &[PanelEntry], taker_fee: f64) -> Series {
    let lookback = 14usize;
    let top = 0.2;
    let min_vol = 5_000_000.0;
    let vol_win = 30usize;
    // 与实盘/纸交易一致：每腿最多 8 个仓位
    let max_positions = 8usize;

    let mut ts: Vec<i64> = panel
        .iter()
        .flat_map(|e| e.candles.iter().map(|c| c.t))
        .collect();
    ts.sort_unstable();
    ts.dedup();
    if ts.len() < lookback + vol_win + 2 {
        return summarize("日频动量", BTreeMap::new());
    }
    let n_t = ts.len();
    let idx_of: std::collections::HashMap<i64, usize> =
        ts.iter().enumerate().map(|(i, t)| (*t, i)).collect();

    // 每个币：时间 -> (收盘, 成交额)
    let mut closes: Vec<Vec<Option<f64>>> = Vec::new();
    let mut dvol: Vec<Vec<f64>> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for e in panel {
        if e.coin.contains(':') {
            continue;
        }
        let mut cm = vec![None; n_t];
        let mut vm = vec![0.0f64; n_t];
        for c in &e.candles {
            if c.c <= 0.0 {
                continue;
            }
            if let Some(&i) = idx_of.get(&c.t) {
                cm[i] = Some(c.c);
                vm[i] = c.v * c.c;
            }
        }
        names.push(e.coin.clone());
        closes.push(cm);
        dvol.push(vm);
    }
    let n_c = names.len();
    if n_c < 8 {
        return summarize("日频动量", BTreeMap::new());
    }

    let mut w = vec![0.0f64; n_c];
    let mut prev: Option<usize> = None;
    let mut out: BTreeMap<i64, f64> = BTreeMap::new();

    for i in (lookback + 1)..n_t {
        let mut r_prev = 0.0;
        if let Some(p) = prev {
            for j in 0..n_c {
                if let (Some(a), Some(b)) = (closes[j][i], closes[j][p]) {
                    if b > 0.0 {
                        r_prev += w[j] * (a / b - 1.0);
                    }
                }
            }
        }
        prev = Some(i);

        let lo = i.saturating_sub(vol_win);
        let mut sigs: Vec<(usize, f64)> = Vec::new();
        for j in 0..n_c {
            let (Some(now), Some(past)) = (closes[j][i], closes[j][i - lookback]) else {
                continue;
            };
            if now <= 0.0 || past <= 0.0 {
                continue;
            }
            let mut sum = 0.0;
            let mut n = 0usize;
            for k in lo..i {
                sum += dvol[j][k];
                n += 1;
            }
            let avg = if n == 0 { 0.0 } else { sum / n as f64 };
            if avg < min_vol {
                continue;
            }
            sigs.push((j, now / past - 1.0));
        }
        if sigs.len() < 8 {
            out.insert(ts[i], r_prev);
            w = vec![0.0; n_c];
            continue;
        }
        sigs.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let k = ((sigs.len() as f64 * top).round() as usize)
            .max(1)
            .min(max_positions);
        let mut tgt = vec![0.0f64; n_c];
        for (j, _) in &sigs[sigs.len() - k..] {
            tgt[*j] = 0.5 / k as f64;
        }
        for (j, _) in &sigs[..k] {
            tgt[*j] = -0.5 / k as f64;
        }
        let one_way: f64 = (0..n_c).map(|j| (tgt[j] - w[j]).abs()).sum::<f64>() / 2.0;
        out.insert(ts[i], r_prev - 2.0 * one_way * taker_fee);
        w = tgt;
    }
    summarize("日频动量", out)
}

/// 4 小时反转：4 小时回看、做多跌得最多的 25%、做空涨得最多的 25%。
/// 成交额过滤用「滚动 720 小时」，严格截至当时。
pub fn reversal_series(panel: &[(String, Vec<Candle>)], maker_fee: f64) -> Series {
    let lookback = 4usize;
    let rebal = 4usize;
    let top = 0.25;
    let min_vol = 150_000.0;
    let vol_win = 720usize;

    let mut ts: Vec<i64> = panel
        .iter()
        .flat_map(|(_, cs)| cs.iter().map(|c| c.t))
        .collect();
    ts.sort_unstable();
    ts.dedup();
    if ts.len() < vol_win + lookback + 4 {
        return summarize("4h反转", BTreeMap::new());
    }
    let n_t = ts.len();
    let idx_of: std::collections::HashMap<i64, usize> =
        ts.iter().enumerate().map(|(i, t)| (*t, i)).collect();

    let mut closes: Vec<Vec<Option<f64>>> = Vec::new();
    let mut hvol: Vec<Vec<f64>> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for (coin, cs) in panel {
        if coin.contains(':') {
            continue;
        }
        let mut cm = vec![None; n_t];
        let mut vm = vec![0.0f64; n_t];
        for c in cs {
            if c.c <= 0.0 {
                continue;
            }
            if let Some(&i) = idx_of.get(&c.t) {
                cm[i] = Some(c.c);
                vm[i] = c.v * c.c;
            }
        }
        names.push(coin.clone());
        closes.push(cm);
        hvol.push(vm);
    }
    let n_c = names.len();
    if n_c < 8 {
        return summarize("4h反转", BTreeMap::new());
    }

    let mut w = vec![0.0f64; n_c];
    let mut prev: Option<usize> = None;
    let mut day: BTreeMap<i64, f64> = BTreeMap::new();

    let mut i = vol_win + lookback;
    while i + rebal < n_t {
        let mut r_prev = 0.0;
        if let Some(p) = prev {
            for j in 0..n_c {
                if let (Some(a), Some(b)) = (closes[j][i], closes[j][p]) {
                    if b > 0.0 {
                        r_prev += w[j] * (a / b - 1.0);
                    }
                }
            }
        }
        prev = Some(i);

        let lo = i.saturating_sub(vol_win);
        let mut sigs: Vec<(usize, f64)> = Vec::new();
        for j in 0..n_c {
            let (Some(now), Some(past)) = (closes[j][i], closes[j][i - lookback]) else {
                continue;
            };
            if now <= 0.0 || past <= 0.0 {
                continue;
            }
            let mut sum = 0.0;
            let mut n = 0usize;
            for k in lo..i {
                sum += hvol[j][k];
                n += 1;
            }
            let avg = if n == 0 { 0.0 } else { sum / n as f64 };
            if avg < min_vol {
                continue;
            }
            sigs.push((j, now / past - 1.0));
        }
        if sigs.len() >= 8 {
            sigs.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            let k = ((sigs.len() as f64 * top).round() as usize).max(1);
            let mut tgt = vec![0.0f64; n_c];
            for (j, _) in &sigs[..k] {
                tgt[*j] = 0.5 / k as f64; // 多跌得多的
            }
            for (j, _) in &sigs[sigs.len() - k..] {
                tgt[*j] = -0.5 / k as f64; // 空涨得多的
            }
            let one_way: f64 = (0..n_c).map(|j| (tgt[j] - w[j]).abs()).sum::<f64>() / 2.0;
            let d = ts[i] / DAY_MS * DAY_MS;
            *day.entry(d).or_insert(0.0) += r_prev - 2.0 * one_way * maker_fee;
            w = tgt;
        } else {
            w = vec![0.0; n_c];
        }
        i += rebal;
    }
    summarize("4h反转", day)
}

#[derive(Serialize)]
pub struct Combo {
    pub weight_momentum: f64,
    pub ann_pct: f64,
    pub vol_pct: f64,
    pub sharpe: f64,
    pub t: f64,
}

/// 对齐两个序列的日期，计算相关系数与权重扫描。
pub fn combine(a: &Series, b: &Series) -> (f64, Vec<Combo>, f64) {
    let ma: BTreeMap<i64, f64> = a.dates.iter().copied().zip(a.rets.iter().copied()).collect();
    let mb: BTreeMap<i64, f64> = b.dates.iter().copied().zip(b.rets.iter().copied()).collect();
    let days: Vec<i64> = ma.keys().filter(|d| mb.contains_key(d)).copied().collect();
    if days.len() < 20 {
        return (0.0, Vec::new(), 0.0);
    }
    let x: Vec<f64> = days.iter().map(|d| ma[d]).collect();
    let y: Vec<f64> = days.iter().map(|d| mb[d]).collect();
    let n = days.len() as f64;
    let mx = x.iter().sum::<f64>() / n;
    let my = y.iter().sum::<f64>() / n;
    let mut cxy = 0.0;
    let mut vx = 0.0;
    let mut vy = 0.0;
    for i in 0..days.len() {
        let dx = x[i] - mx;
        let dy = y[i] - my;
        cxy += dx * dy;
        vx += dx * dx;
        vy += dy * dy;
    }
    let corr = if vx > 0.0 && vy > 0.0 {
        cxy / (vx.sqrt() * vy.sqrt())
    } else {
        0.0
    };
    let mut combos = Vec::new();
    let mut best = (0.0f64, f64::NEG_INFINITY);
    for wi in 0..=10 {
        let wm = wi as f64 / 10.0;
        let port: Vec<f64> = (0..days.len())
            .map(|i| wm * x[i] + (1.0 - wm) * y[i])
            .collect();
        let m = port.iter().sum::<f64>() / n;
        let var = port.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (n - 1.0);
        let sd = var.sqrt();
        let sharpe = if sd > 0.0 { m / sd * 365.0_f64.sqrt() } else { 0.0 };
        if sharpe > best.1 {
            best = (wm, sharpe);
        }
        combos.push(Combo {
            weight_momentum: wm,
            ann_pct: m * 365.0 * 100.0,
            vol_pct: sd * 365.0_f64.sqrt() * 100.0,
            sharpe,
            t: if sd > 0.0 { m / (sd / n.sqrt()) } else { 0.0 },
        });
    }
    (corr, combos, best.0)
}
