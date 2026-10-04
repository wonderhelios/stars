//! Cross-sectional momentum backtest engine.
//!
//! Signal = trailing return over `lookback` days. Long the top `top_frac`
//! fraction of the liquid universe, hedge with either the bottom fraction
//! (long-short) or the equal-weight universe (pure alpha). Reports market-neutral
//! statistics: alpha, Sharpe, t-stat, beta to BTC, per-year, cost sensitivity.

use crate::hl::Candle;
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Copy, Debug)]
pub enum HedgeMode {
    LongShort,
    EqualWeight,
}

#[derive(Clone, Copy, Debug)]
pub struct BacktestParams {
    pub lookback: usize,
    pub top_frac: f64,
    pub min_vol_usd: f64,
    pub hedge: HedgeMode,
}

impl Default for BacktestParams {
    fn default() -> Self {
        Self {
            lookback: 14,
            top_frac: 0.2,
            min_vol_usd: 5_000_000.0,
            hedge: HedgeMode::EqualWeight,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PanelEntry {
    pub coin: String,
    pub candles: Vec<Candle>,
}

#[derive(serde::Serialize, Clone)]
pub struct YearRow {
    pub year: i32,
    pub days: usize,
    pub alpha_annual: f64,
    pub t_stat: f64,
}

#[derive(serde::Serialize, Clone)]
pub struct CostRow {
    pub fee: f64,        // per-side fee, fraction
    pub net_annual: f64, // net annualized alpha after cost
}

#[derive(serde::Serialize, Clone)]
pub struct BacktestResult {
    pub days: usize,
    pub coins_traded: usize,
    pub alpha_daily: f64,     // winner - hedge, per day (fraction)
    pub alpha_annual: f64,
    pub sharpe: f64,
    pub t_stat: f64,
    pub win_rate: f64,
    pub beta: f64,
    pub corr_btc: f64,
    pub long_leg_daily: f64,
    pub short_leg_daily: f64,
    pub ew_daily: f64,
    pub turnover_daily: f64,
    pub per_year: Vec<YearRow>,
    pub cost_sensitivity: Vec<CostRow>,
    pub top_coins: Vec<(String, usize)>,
}

struct Day {
    year: i32,
    alpha: f64,
    long_leg: f64,
    short_leg: f64,
    ew: f64,
    btc: Option<f64>,
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}

fn std(xs: &[f64]) -> f64 {
    let m = mean(xs);
    let n = xs.len();
    if n < 2 {
        return 0.0;
    }
    (xs.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (n - 1) as f64).sqrt()
}

fn t_stat(xs: &[f64]) -> f64 {
    let sd = std(xs);
    if sd <= 0.0 {
        return 0.0;
    }
    mean(xs) / sd * (xs.len() as f64).sqrt()
}

/// Run the momentum backtest over a candle panel (already point-in-time aware:
/// a coin only participates on days it has a close).
pub fn run(panel: &[PanelEntry], params: &BacktestParams) -> BacktestResult {
    // coin -> {ts_index -> (close, dollar_vol)}
    // Build the union timeline first.
    let mut timeline: BTreeMap<i64, usize> = BTreeMap::new();
    for e in panel {
        for c in &e.candles {
            timeline.entry(c.t).or_insert(0);
        }
    }
    let ts: Vec<i64> = timeline.keys().copied().collect();
    for (i, t) in ts.iter().enumerate() {
        timeline.insert(*t, i);
    }
    let n = ts.len();

    // index per coin
    let mut idx: HashMap<&str, HashMap<usize, (f64, f64)>> = HashMap::new();
    for e in panel {
        let mut m = HashMap::new();
        for c in &e.candles {
            if c.c > 0.0 {
                let i = timeline[&c.t];
                m.insert(i, (c.c, c.v * c.c));
            }
        }
        idx.insert(&e.coin, m);
    }

    let mut days: Vec<Day> = Vec::new();
    let mut long_picks: HashMap<String, usize> = HashMap::new();
    let mut turnover_sum = 0.0;
    let mut prev_long: Option<std::collections::HashSet<String>> = None;

    let min_coin = 15usize;
    let vol_window = 30usize;

    for i in (params.lookback + vol_window)..(n - 1) {
        // collect eligible coins with signal + forward + rolling volume
        let mut sigs: Vec<(String, f64, f64)> = Vec::new(); // (coin, signal, fwd)
        for (coin, m) in idx.iter() {
            let now = m.get(&i);
            let past = m.get(&(i - params.lookback));
            let fwd = m.get(&(i + 1));
            let (now, past, fwd) = match (now, past, fwd) {
                (Some(a), Some(b), Some(c)) => (a, b, c),
                _ => continue,
            };
            if now.0 <= 0.0 || past.0 <= 0.0 || fwd.0 <= 0.0 {
                continue;
            }
            // rolling 30d mean dollar volume (trailing, point-in-time)
            let mut vols: Vec<f64> = Vec::with_capacity(vol_window);
            for j in (i - vol_window)..i {
                if let Some(v) = m.get(&j) {
                    vols.push(v.1);
                }
            }
            if vols.len() < 5 {
                continue;
            }
            let avg_vol = mean(&vols);
            if avg_vol < params.min_vol_usd {
                continue;
            }
            let signal = now.0 / past.0 - 1.0;
            let fwd_ret = fwd.0 / now.0 - 1.0;
            sigs.push((coin.to_string(), signal, fwd_ret));
        }
        if sigs.len() < min_coin {
            continue;
        }
        sigs.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let k = ((sigs.len() as f64 * params.top_frac).round() as usize).max(1);
        let winners = &sigs[sigs.len() - k..];
        let losers = &sigs[..k];

        let long_ret = mean(&winners.iter().map(|s| s.2).collect::<Vec<_>>());
        let short_ret = mean(&losers.iter().map(|s| s.2).collect::<Vec<_>>());
        let ew_ret = mean(&sigs.iter().map(|s| s.2).collect::<Vec<_>>());

        let alpha = match params.hedge {
            HedgeMode::LongShort => long_ret - short_ret,
            HedgeMode::EqualWeight => long_ret - ew_ret,
        };

        // turnover: fraction of long names that changed vs previous day
        let long_set: std::collections::HashSet<String> =
            winners.iter().map(|s| s.0.clone()).collect();
        if let Some(prev) = &prev_long {
            let changed = prev.symmetric_difference(&long_set).count();
            turnover_sum += changed as f64 / (prev.len() + long_set.len()).max(1) as f64;
        }
        prev_long = Some(long_set);
        for w in winners {
            *long_picks.entry(w.0.clone()).or_insert(0) += 1;
        }

        let btc = idx.get("BTC").and_then(|m| {
            let now = m.get(&i)?;
            let fwd = m.get(&(i + 1))?;
            if now.0 > 0.0 {
                Some(fwd.0 / now.0 - 1.0)
            } else {
                None
            }
        });

        let year = epoch_year(ts[i]);

        days.push(Day {
            year,
            alpha,
            long_leg: long_ret,
            short_leg: short_ret,
            ew: ew_ret,
            btc,
        });
    }

    let alphas: Vec<f64> = days.iter().map(|d| d.alpha).collect();
    let sd = std(&alphas);
    let sharpe = if sd > 0.0 {
        mean(&alphas) / sd * (365.0_f64).sqrt()
    } else {
        0.0
    };
    let win_rate = alphas.iter().filter(|x| **x > 0.0).count() as f64 / alphas.len().max(1) as f64;

    // beta vs BTC
    let (mut beta, mut corr) = (0.0, 0.0);
    {
        let pairs: Vec<(f64, f64)> = days
            .iter()
            .filter_map(|d| d.btc.map(|b| (b, d.alpha)))
            .collect();
        if pairs.len() > 2 {
            let bx = mean(&pairs.iter().map(|p| p.0).collect::<Vec<_>>());
            let by = mean(&pairs.iter().map(|p| p.1).collect::<Vec<_>>());
            let cov = pairs.iter().map(|p| (p.0 - bx) * (p.1 - by)).sum::<f64>() / pairs.len() as f64;
            let var = pairs.iter().map(|p| (p.0 - bx).powi(2)).sum::<f64>() / pairs.len() as f64;
            if var > 0.0 {
                beta = cov / var;
            }
            let sx = std(&pairs.iter().map(|p| p.0).collect::<Vec<_>>());
            let sy = std(&pairs.iter().map(|p| p.1).collect::<Vec<_>>());
            if sx > 0.0 && sy > 0.0 {
                corr = cov / (sx * sy);
            }
        }
    }

    // per-year
    let mut years: HashMap<i32, Vec<f64>> = HashMap::new();
    for d in &days {
        years.entry(d.year).or_default().push(d.alpha);
    }
    let mut per_year: Vec<YearRow> = years
        .into_iter()
        .map(|(year, v)| YearRow {
            year,
            days: v.len(),
            alpha_annual: mean(&v) * 365.0,
            t_stat: t_stat(&v),
        })
        .collect();
    per_year.sort_by_key(|r| r.year);

    // cost sensitivity (daily rebalance, turnover-aware)
    let turnover = if days.len() > 1 {
        turnover_sum / (days.len() - 1) as f64
    } else {
        0.0
    };
    let mut cost_sensitivity = Vec::new();
    for fee in [0.0001, 0.0002, 0.00045, 0.0007, 0.0010] {
        let net_daily = mean(&alphas) - turnover * 2.0 * fee;
        cost_sensitivity.push(CostRow {
            fee,
            net_annual: net_daily * 365.0,
        });
    }

    // top long coins
    let mut top_coins: Vec<(String, usize)> = long_picks.into_iter().collect();
    top_coins.sort_by(|a, b| b.1.cmp(&a.1));
    top_coins.truncate(12);

    BacktestResult {
        days: days.len(),
        coins_traded: idx.len(),
        alpha_daily: mean(&alphas),
        alpha_annual: mean(&alphas) * 365.0,
        sharpe,
        t_stat: t_stat(&alphas),
        win_rate,
        beta,
        corr_btc: corr,
        long_leg_daily: mean(&days.iter().map(|d| d.long_leg).collect::<Vec<_>>()),
        short_leg_daily: mean(&days.iter().map(|d| d.short_leg).collect::<Vec<_>>()),
        ew_daily: mean(&days.iter().map(|d| d.ew).collect::<Vec<_>>()),
        turnover_daily: turnover,
        per_year,
        cost_sensitivity,
        top_coins,
    }
}

fn epoch_year(ms: i64) -> i32 {
    // days since 1970-01-01
    let days = ms / 86_400_000;
    let mut year = 1970;
    let mut d = days;
    loop {
        let ylen = if is_leap(year) { 366 } else { 365 };
        if d < ylen {
            return year;
        }
        d -= ylen;
        year += 1;
    }
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
