//! Paper-trading engine: a market-neutral long/short portfolio that mirrors what
//! would actually be traded live.
//!
//! Each day: rank the liquid universe by trailing momentum, go long the top
//! `top_frac` and short the bottom `top_frac`, equal weight within each leg.
//! Gross notional = capital * leverage, split evenly between the two legs.
//! Positions are only traded when leg membership changes, so turnover reflects
//! the real signal rather than constant rebalancing.
//!
//! On start the engine replays `replay_days` of history so the trade log and
//! equity curve are populated immediately, then continues forward as new daily
//! closes arrive.

use crate::hl::maintenance_margin_rate;
use crate::momentum::PanelEntry;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Long,
    Short,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaperConfig {
    pub lookback: usize,
    pub top_frac: f64,
    pub min_vol_usd: f64,
    pub capital: f64,
    pub leverage: f64,
    /// Per-side fee (Hyperliquid taker by default).
    pub fee: f64,
    /// History replayed on start so the book is not empty.
    #[serde(default = "d_replay")]
    pub replay_days: usize,
    /// Target positions per leg; each gets gross_notional/2/target_positions.
    /// Keeps position size stable instead of drifting with the daily count of
    /// liquid coins.
    #[serde(default = "d_target_pos")]
    pub target_positions: usize,
}

fn d_replay() -> usize {
    90
}
fn d_target_pos() -> usize {
    8
}

impl Default for PaperConfig {
    fn default() -> Self {
        Self {
            lookback: 14,
            top_frac: 0.2,
            min_vol_usd: 5_000_000.0,
            capital: 2_000.0,
            leverage: 3.0,
            fee: 0.00045,
            replay_days: 90,
            target_positions: 8,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaperPosition {
    pub coin: String,
    pub side: Side,
    pub notional: f64,
    pub entry_price: f64,
    pub entry_ts: i64,
    pub mark_price: f64,
    pub unrealized_pnl: f64,
    pub liq_price: f64,
    pub max_leverage: u32,
    /// position opened during history replay rather than live
    #[serde(default)]
    pub replayed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Trade {
    pub coin: String,
    pub side: Side,
    pub entry_ts: i64,
    pub entry_price: f64,
    pub exit_ts: i64,
    pub exit_price: f64,
    pub notional: f64,
    pub pnl_usd: f64,
    pub pnl_pct: f64,
    #[serde(default)]
    pub replayed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EquityPoint {
    pub ts: i64,
    pub equity: f64,
    pub market: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PaperState {
    pub running: bool,
    pub config: Option<PaperConfig>,
    pub equity: f64,
    pub market: f64,
    pub positions: Vec<PaperPosition>,
    pub trades: Vec<Trade>,
    pub last_ts: Option<i64>,
    pub started_at: Option<i64>,
    pub replay_until: Option<i64>,
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

fn cum_pnl(side: Side, notional: f64, entry: f64, px: f64) -> f64 {
    if entry <= 0.0 {
        return 0.0;
    }
    match side {
        Side::Long => notional * (px / entry - 1.0),
        Side::Short => notional * (1.0 - px / entry),
    }
}

/// Isolated liquidation price for a position of the given side and leverage.
pub fn liq_price(side: Side, entry: f64, leverage: f64, max_leverage: u32) -> f64 {
    let m = maintenance_margin_rate(max_leverage);
    let adverse = (1.0 / leverage - m).max(0.0);
    match side {
        Side::Long => entry * (1.0 - adverse),
        Side::Short => entry * (1.0 + adverse),
    }
}

fn mark(pos: &mut PaperPosition, px: f64, leverage: f64) {
    pos.mark_price = px;
    pos.unrealized_pnl = cum_pnl(pos.side, pos.notional, pos.entry_price, px);
    pos.liq_price = liq_price(pos.side, pos.entry_price, leverage, pos.max_leverage);
}

struct Panel {
    ts: Vec<i64>,
    closes: HashMap<String, BTreeMap<i64, f64>>,
    dollar_vol: HashMap<String, BTreeMap<i64, f64>>,
}

impl Panel {
    fn build(panel: &[PanelEntry]) -> Self {
        let mut timeline: BTreeSet<i64> = Default::default();
        let mut closes: HashMap<String, BTreeMap<i64, f64>> = Default::default();
        let mut dollar_vol: HashMap<String, BTreeMap<i64, f64>> = Default::default();
        for e in panel {
            let mut cm = BTreeMap::new();
            let mut vm = BTreeMap::new();
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
        Self {
            ts: timeline.into_iter().collect(),
            closes,
            dollar_vol,
        }
    }

    fn px(&self, coin: &str, t: i64) -> Option<f64> {
        self.closes.get(coin).and_then(|m| m.get(&t)).copied()
    }
}

/// Advance the paper trade. On a fresh start this replays `replay_days` of
/// history; otherwise it processes every new daily close since the last run.
pub fn step(state: &mut PaperState, panel: &[PanelEntry], max_lev: &HashMap<String, u32>) {
    let Some(cfg) = state.config.clone() else {
        return;
    };
    let p = Panel::build(panel);
    let vol_window = 30usize;
    if p.ts.len() < cfg.lookback + vol_window + 2 {
        return;
    }

    let fresh = state.last_ts.is_none();
    let days: Vec<usize> = if fresh {
        let last = p.ts.len() - 1;
        let start = last
            .saturating_sub(cfg.replay_days)
            .max(cfg.lookback + vol_window);
        (start..=last).collect()
    } else {
        match p.ts.binary_search(&state.last_ts.unwrap()) {
            Ok(j) if j + 1 < p.ts.len() => (j + 1..p.ts.len()).collect(),
            _ => Vec::new(),
        }
    };
    if days.is_empty() {
        return;
    }
    if fresh {
        state.replay_until = None;
    }

    for (n, &i) in days.iter().enumerate() {
        let is_first = fresh && n == 0;
        let is_replay = fresh;
        advance(state, &cfg, &p, max_lev, i, is_first, is_replay, vol_window);
        if fresh && n == 0 {
            state.started_at = Some(p.ts[i]);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn advance(
    state: &mut PaperState,
    cfg: &PaperConfig,
    p: &Panel,
    max_lev: &HashMap<String, u32>,
    i: usize,
    is_first: bool,
    replayed: bool,
    vol_window: usize,
) {
    let t = p.ts[i];
    // Rank the point-in-time liquid universe by trailing momentum.
    let mut sigs: Vec<(String, f64)> = Vec::new();
    for (coin, cm) in p.closes.iter() {
        let Some(vm) = p.dollar_vol.get(coin) else { continue };
        let mut vols: Vec<f64> = Vec::with_capacity(vol_window);
        for j in (i - vol_window)..i {
            if let Some(v) = vm.get(&p.ts[j]) {
                vols.push(*v);
            }
        }
        if vols.len() < 5 {
            continue;
        }
        let avg = vols.iter().sum::<f64>() / vols.len() as f64;
        if avg < cfg.min_vol_usd {
            continue;
        }
        let (Some(now), Some(past)) = (cm.get(&t), cm.get(&p.ts[i - cfg.lookback])) else {
            continue;
        };
        if *now <= 0.0 || *past <= 0.0 {
            continue;
        }
        sigs.push((coin.clone(), now / past - 1.0));
    }
    if sigs.len() < 12 {
        return;
    }
    sigs.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let k = ((sigs.len() as f64 * cfg.top_frac).round() as usize).max(1);
    // Hold at most `target_positions` names per leg, but never more than the
    // number the momentum ranking actually offers.
    let n = k.min(cfg.target_positions).max(1);
    let long_leg: HashSet<String> = sigs[sigs.len() - n..].iter().map(|s| s.0.clone()).collect();
    let short_leg: HashSet<String> = sigs[..n].iter().map(|s| s.0.clone()).collect();

    // Fixed position size so the book does not balloon when fewer coins are
    // liquid: each leg deploys gross/2 across `target_positions` slots.
    let leg_notional = cfg.capital * cfg.leverage / 2.0;
    let per_coin = leg_notional / cfg.target_positions.max(1) as f64;

    // ---------- first day: open the book ----------
    if is_first {
        for (leg, side) in [(&long_leg, Side::Long), (&short_leg, Side::Short)] {
            for coin in leg {
                let Some(px) = p.px(coin, t) else { continue };
                let ml = max_lev.get(coin).copied().unwrap_or(10);
                let mut pos = PaperPosition {
                    coin: coin.clone(),
                    side,
                    notional: per_coin,
                    entry_price: px,
                    entry_ts: t,
                    mark_price: px,
                    unrealized_pnl: 0.0,
                    liq_price: 0.0,
                    max_leverage: ml,
                    replayed,
                };
                mark(&mut pos, px, cfg.leverage);
                state.positions.push(pos);
            }
        }
        let cost = state.positions.iter().map(|x| x.notional * cfg.fee).sum::<f64>();
        state.total_cost += cost;
        state.equity -= cost;
        state.last_ts = Some(t);
        state.days_elapsed = 1;
        state.history.push(EquityPoint { ts: t, equity: state.equity, market: state.market });
        return;
    }

    // ---------- settle the previous close ----------
    let t_prev = p.ts[i - 1];
    let mut day_pnl = 0.0;
    for pos in state.positions.iter_mut() {
        if let (Some(prev), Some(cur)) = (p.px(&pos.coin, t_prev), p.px(&pos.coin, t)) {
            day_pnl += cum_pnl(pos.side, pos.notional, pos.entry_price, cur)
                - cum_pnl(pos.side, pos.notional, pos.entry_price, prev);
            mark(pos, cur, cfg.leverage);
        }
    }
    let mkt: Vec<f64> = sigs
        .iter()
        .filter_map(|(coin, _)| {
            let prev = p.px(coin, t_prev)?;
            let cur = p.px(coin, t)?;
            if prev > 0.0 { Some(cur / prev - 1.0) } else { None }
        })
        .collect();
    if !mkt.is_empty() {
        state.market *= 1.0 + mkt.iter().sum::<f64>() / mkt.len() as f64;
    }
    state.equity += day_pnl;

    // ---------- trade membership changes ----------
    let mut keep: Vec<PaperPosition> = Vec::new();
    let mut closed = 0.0;
    for pos in state.positions.drain(..) {
        let wanted = match pos.side {
            Side::Long => long_leg.contains(&pos.coin),
            Side::Short => short_leg.contains(&pos.coin),
        };
        let px = p.px(&pos.coin, t);
        if wanted {
            let mut pos = pos;
            if let Some(px) = px {
                mark(&mut pos, px, cfg.leverage);
            }
            keep.push(pos);
        } else {
            // Close the position. If today's close is missing, fall back to the
            // last mark so the realized PnL is not silently lost.
            let exit_px = px.unwrap_or(pos.mark_price);
            if exit_px > 0.0 {
                let pnl = cum_pnl(pos.side, pos.notional, pos.entry_price, exit_px);
                state.trades.push(Trade {
                    coin: pos.coin.clone(),
                    side: pos.side,
                    entry_ts: pos.entry_ts,
                    entry_price: pos.entry_price,
                    exit_ts: t,
                    exit_price: exit_px,
                    notional: pos.notional,
                    pnl_usd: pnl,
                    pnl_pct: if pos.notional > 0.0 { pnl / pos.notional * 100.0 } else { 0.0 },
                    replayed: pos.replayed,
                });
                closed += pos.notional;
            }
        }
    }
    state.positions = keep;

    let have: HashSet<(String, Side)> =
        state.positions.iter().map(|x| (x.coin.clone(), x.side)).collect();
    let mut opened = 0.0;
    for (leg, side) in [(&long_leg, Side::Long), (&short_leg, Side::Short)] {
        for coin in leg {
            if have.contains(&(coin.clone(), side)) {
                continue;
            }
            let Some(px) = p.px(coin, t) else { continue };
            let ml = max_lev.get(coin).copied().unwrap_or(10);
            let mut pos = PaperPosition {
                coin: coin.clone(),
                side,
                notional: per_coin,
                entry_price: px,
                entry_ts: t,
                mark_price: px,
                unrealized_pnl: 0.0,
                liq_price: 0.0,
                max_leverage: ml,
                replayed,
            };
            mark(&mut pos, px, cfg.leverage);
            state.positions.push(pos);
            opened += per_coin;
        }
    }

    let cost = (closed + opened) * cfg.fee;
    state.total_cost += cost;
    state.equity -= cost;

    state.last_ts = Some(t);
    state.days_elapsed += 1;
    state.history.push(EquityPoint { ts: t, equity: state.equity, market: state.market });
}

#[derive(Serialize)]
pub struct PaperSnapshot {
    pub running: bool,
    pub equity: f64,
    pub capital: f64,
    pub leverage: f64,
    pub pnl: f64,
    pub pnl_pct: f64,
    pub market_pct: f64,
    pub alpha_pct: f64,
    pub gross_notional: f64,
    pub margin_used: f64,
    pub margin_usage_pct: f64,
    pub nearest_liq_pct: Option<f64>,
    pub days_elapsed: usize,
    pub total_cost: f64,
    pub realized_pnl: f64,
    pub unrealized_pnl: f64,
    pub win_rate: f64,
    pub positions: Vec<PaperPosition>,
    pub trades: Vec<Trade>,
    pub history: Vec<EquityPoint>,
    pub config: Option<PaperConfig>,
}

pub fn snapshot(state: &PaperState) -> PaperSnapshot {
    let cfg = state.config.clone().unwrap_or_default();
    let pnl = state.equity - cfg.capital;
    let gross: f64 = state.positions.iter().map(|x| x.notional).sum();
    let margin: f64 = state.positions.iter().map(|x| x.notional / cfg.leverage).sum();
    let unrealized: f64 = state.positions.iter().map(|x| x.unrealized_pnl).sum();
    let realized: f64 = state.trades.iter().map(|x| x.pnl_usd).sum();
    let wins = state.trades.iter().filter(|x| x.pnl_usd > 0.0).count();
    let nearest = state
        .positions
        .iter()
        .filter(|x| x.mark_price > 0.0 && x.liq_price > 0.0)
        .map(|x| match x.side {
            Side::Long => (x.mark_price - x.liq_price) / x.mark_price * 100.0,
            Side::Short => (x.liq_price - x.mark_price) / x.mark_price * 100.0,
        })
        .fold(None, |acc: Option<f64>, v| Some(acc.map_or(v, |a| a.min(v))));

    PaperSnapshot {
        running: state.running,
        equity: state.equity,
        capital: cfg.capital,
        leverage: cfg.leverage,
        pnl,
        pnl_pct: if cfg.capital > 0.0 { pnl / cfg.capital * 100.0 } else { 0.0 },
        market_pct: (state.market - 1.0) * 100.0,
        alpha_pct: (state.equity / cfg.capital - state.market) * 100.0,
        gross_notional: gross,
        margin_used: margin,
        margin_usage_pct: if state.equity > 0.0 { margin / state.equity * 100.0 } else { 0.0 },
        nearest_liq_pct: nearest,
        days_elapsed: state.days_elapsed,
        total_cost: state.total_cost,
        realized_pnl: realized,
        unrealized_pnl: unrealized,
        win_rate: if state.trades.is_empty() {
            0.0
        } else {
            wins as f64 / state.trades.len() as f64 * 100.0
        },
        positions: state.positions.clone(),
        trades: state.trades.clone(),
        history: state.history.clone(),
        config: state.config.clone(),
    }
}
