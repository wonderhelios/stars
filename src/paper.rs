//! Paper-trading engine: runs the momentum strategy forward with daily
//! rebalance, tracking strategy equity vs an equal-weight benchmark to validate
//! live execution against the backtest.

use crate::momentum::PanelEntry;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaperConfig {
    pub lookback: usize,
    pub top_frac: f64,
    pub min_vol_usd: f64,
    pub fee: f64, // per-side
    pub capital: f64,
}

impl Default for PaperConfig {
    fn default() -> Self {
        Self {
            lookback: 14,
            top_frac: 0.2,
            min_vol_usd: 5_000_000.0,
            fee: 0.00045,
            capital: 10_000.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaperPosition {
    pub coin: String,
    pub notional: f64,
    pub entry_close: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EquityPoint {
    pub ts: i64,
    pub strategy: f64,
    pub benchmark: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PaperState {
    pub running: bool,
    pub config: Option<PaperConfig>,
    pub equity: f64,
    pub benchmark: f64,
    pub positions: Vec<PaperPosition>,
    pub last_ts: Option<i64>,
    pub started_at: Option<i64>,
    pub days_elapsed: usize,
    pub total_cost: f64,
    pub history: Vec<EquityPoint>,
}

impl PaperState {
    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Advance the paper trade by one daily close. On the first call it only sets
/// the initial positions at the latest close (no return is booked yet); each
/// later call marks positions to market for the new day, then rebalances.
pub fn step(state: &mut PaperState, panel: &[PanelEntry]) {
    let Some(cfg) = state.config.clone() else {
        return;
    };

    // Build timeline + per-coin close & dollar-volume maps.
    let mut timeline: std::collections::BTreeSet<i64> = Default::default();
    let mut closes: std::collections::HashMap<String, std::collections::BTreeMap<i64, f64>> =
        Default::default();
    let mut dollar_vol: std::collections::HashMap<String, std::collections::BTreeMap<i64, f64>> =
        Default::default();
    for e in panel {
        let mut cm = std::collections::BTreeMap::new();
        let mut vm = std::collections::BTreeMap::new();
        for c in &e.candles {
            if c.c > 0.0 {
                timeline.insert(c.t);
                cm.insert(c.t, c.c);
                vm.insert(c.t, c.v * c.c);
            }
        }
        closes.insert(e.coin.clone(), cm);
        dollar_vol.insert(e.coin.clone(), vm);
    }
    let ts: Vec<i64> = timeline.into_iter().collect();
    let vol_window = 30usize;
    if ts.len() < cfg.lookback + vol_window + 2 {
        return;
    }

    // Target day index: latest close on first call, next close afterwards.
    let first = state.last_ts.is_none();
    let i = if first {
        ts.len() - 1
    } else {
        let lt = state.last_ts.unwrap();
        match ts.binary_search(&lt) {
            Ok(j) if j + 1 < ts.len() => j + 1,
            _ => return, // no new close yet
        }
    };
    if i < cfg.lookback + vol_window {
        return;
    }
    let t = ts[i];

    // Signal = trailing `lookback` return at close t, restricted to coins with
    // rolling `vol_window` mean dollar volume >= min_vol_usd.
    let mut sigs: Vec<(String, f64)> = Vec::new();
    for (coin, cm) in closes.iter() {
        let Some(vm) = dollar_vol.get(coin) else { continue };
        let mut vols: Vec<f64> = Vec::with_capacity(vol_window);
        for j in (i - vol_window)..i {
            if let Some(v) = vm.get(&ts[j]) {
                vols.push(*v);
            }
        }
        if vols.len() < 5 {
            continue;
        }
        let avg_vol: f64 = vols.iter().sum::<f64>() / vols.len() as f64;
        if avg_vol < cfg.min_vol_usd {
            continue;
        }
        let now = cm.get(&t);
        let past = cm.get(&ts[i - cfg.lookback]);
        let (Some(now), Some(past)) = (now, past) else { continue };
        if *now <= 0.0 || *past <= 0.0 {
            continue;
        }
        sigs.push((coin.clone(), now / past - 1.0));
    }
    if sigs.len() < 10 {
        return;
    }
    sigs.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let k = ((sigs.len() as f64 * cfg.top_frac).round() as usize).max(1);
    let winners: Vec<&(String, f64)> = sigs[sigs.len() - k..].iter().collect();

    // First call: only establish positions, no return booked.
    if first {
        let per = cfg.capital / k as f64;
        state.positions = winners
            .iter()
            .map(|w| PaperPosition {
                coin: w.0.clone(),
                notional: per,
                entry_close: closes.get(&w.0).and_then(|m| m.get(&t)).copied().unwrap_or(0.0),
            })
            .collect();
        state.last_ts = Some(t);
        state.started_at = Some(t);
        state.days_elapsed = 1;
        state.history.push(EquityPoint {
            ts: t,
            strategy: state.equity,
            benchmark: state.benchmark,
        });
        return;
    }

    // Mark current positions to market (prev close -> t).
    let t_prev = ts[i - 1];
    let mut strat_ret = 0.0;
    let mut weight = 0.0;
    for p in &state.positions {
        if let Some(cm) = closes.get(&p.coin) {
            if let (Some(prev), Some(cur)) = (cm.get(&t_prev), cm.get(&t)) {
                if *prev > 0.0 {
                    strat_ret += (cur / prev - 1.0) * p.notional;
                    weight += p.notional;
                }
            }
        }
    }
    if weight > 0.0 {
        strat_ret /= weight;
    }
    let bench_rets: Vec<f64> = sigs
        .iter()
        .filter_map(|(coin, _)| {
            let cm = closes.get(coin)?;
            let prev = cm.get(&t_prev)?;
            let cur = cm.get(&t)?;
            if *prev > 0.0 {
                Some(cur / prev - 1.0)
            } else {
                None
            }
        })
        .collect();
    let bench_ret = if bench_rets.is_empty() {
        0.0
    } else {
        bench_rets.iter().sum::<f64>() / bench_rets.len() as f64
    };

    state.equity *= 1.0 + strat_ret;
    state.benchmark *= 1.0 + bench_ret;

    // Rebalance to winners, equal weight, apply turnover cost.
    let per = cfg.capital / k as f64;
    let mut turnover = 0.0;
    let old: std::collections::HashSet<String> =
        state.positions.iter().map(|p| p.coin.clone()).collect();
    let mut new_positions = Vec::new();
    for w in &winners {
        let entry = closes.get(&w.0).and_then(|m| m.get(&t)).copied().unwrap_or(0.0);
        new_positions.push(PaperPosition {
            coin: w.0.clone(),
            notional: per,
            entry_close: entry,
        });
        if !old.contains(&w.0) {
            turnover += per;
        }
    }
    let cost = turnover * 2.0 * cfg.fee;
    state.total_cost += cost;
    state.equity -= cost;

    state.positions = new_positions;
    state.last_ts = Some(t);
    state.days_elapsed += 1;
    state.history.push(EquityPoint {
        ts: t,
        strategy: state.equity,
        benchmark: state.benchmark,
    });
}

#[derive(Serialize)]
pub struct PaperSnapshot {
    pub running: bool,
    pub equity: f64,
    pub benchmark: f64,
    pub alpha_pct: f64,
    pub strategy_ret_pct: f64,
    pub benchmark_ret_pct: f64,
    pub days_elapsed: usize,
    pub total_cost: f64,
    pub positions: Vec<PaperPosition>,
    pub history: Vec<EquityPoint>,
    pub config: Option<PaperConfig>,
}

pub fn snapshot(state: &PaperState) -> PaperSnapshot {
    let capital = state.config.as_ref().map(|c| c.capital).unwrap_or(1.0);
    let strat_ret = state.equity / capital - 1.0;
    let bench_ret = state.benchmark / capital - 1.0;
    PaperSnapshot {
        running: state.running,
        equity: state.equity,
        benchmark: state.benchmark,
        alpha_pct: (strat_ret - bench_ret) * 100.0,
        strategy_ret_pct: strat_ret * 100.0,
        benchmark_ret_pct: bench_ret * 100.0,
        days_elapsed: state.days_elapsed,
        total_cost: state.total_cost,
        positions: state.positions.clone(),
        history: state.history.clone(),
        config: state.config.clone(),
    }
}
