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

/// 与 `TradeConfig::default().margin_buffer` 保持一致，否则纸交易会跑 3.0x、
/// 实盘只跑 2.7x，两边数字没法对照。
const MARGIN_BUFFER: f64 = 0.90;
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
    /// current notional; resized at each rebalance to follow equity (compounding)
    pub notional: f64,
    /// original entry price, kept for display
    pub entry_price: f64,
    /// original entry time, kept for display
    pub entry_ts: i64,
    /// PnL basis; resets to the mark price whenever the position is resized
    #[serde(default)]
    pub basis_price: f64,
    /// PnL realized when the position was resized (carried into the final trade)
    #[serde(default)]
    pub realized_carry: f64,
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
    /// price the position first opened at
    pub entry_price: f64,
    /// notional-weighted average entry after rebalancing (PnL is measured from this)
    #[serde(default)]
    pub avg_entry: f64,
    pub exit_ts: i64,
    pub exit_price: f64,
    pub notional: f64,
    pub pnl_usd: f64,
    pub pnl_pct: f64,
    /// "rebalance" (left the leg) or "liquidated" (hit the isolated liq price)
    #[serde(default)]
    pub reason: String,
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
    #[serde(default)]
    pub liquidations: usize,
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

/// Total PnL of a position at `px`, including PnL banked when it was resized.
fn position_pnl(pos: &PaperPosition, px: f64) -> f64 {
    pos.realized_carry + cum_pnl(pos.side, pos.notional, pos.basis_price, px)
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
    pos.unrealized_pnl = position_pnl(pos, px);
    // The liquidation price follows the basis the current size was opened at.
    let basis = if pos.basis_price > 0.0 { pos.basis_price } else { pos.entry_price };
    pos.liq_price = liq_price(pos.side, basis, leverage, pos.max_leverage);
}

/// Resize a position to `target` notional at the current price, keeping the cost
/// basis correct:
///   - adding: the basis becomes the notional-weighted average entry
///   - reducing: the basis is unchanged and the removed slice's PnL is banked
/// This is what makes the isolated liquidation price behave like a real
/// averaged position rather than tracking the latest mark.
fn resize(pos: &mut PaperPosition, target: f64, px: f64, leverage: f64) {
    if px <= 0.0 || target <= 0.0 {
        return;
    }
    let old = pos.notional;
    let basis = if pos.basis_price > 0.0 { pos.basis_price } else { pos.entry_price };
    if target > old {
        // Quantity-weighted average entry, so PnL is continuous across the add.
        let add = target - old;
        let qty = old / basis + add / px;
        pos.basis_price = (old + add) / qty;
    } else if target < old {
        let remove = old - target;
        pos.realized_carry += cum_pnl(pos.side, remove, basis, px);
    } else {
        return;
    }
    pos.notional = target;
    mark(pos, px, leverage);
}

struct Panel {
    ts: Vec<i64>,
    closes: HashMap<String, BTreeMap<i64, f64>>,
}

impl Panel {
    fn build(panel: &[PanelEntry]) -> Self {
        let mut timeline: BTreeSet<i64> = Default::default();
        let mut closes: HashMap<String, BTreeMap<i64, f64>> = Default::default();
        for e in panel {
            let mut cm = BTreeMap::new();
            let mut hm = BTreeMap::new();
            let mut lm = BTreeMap::new();
            for c in &e.candles {
                if c.c > 0.0 {
                    timeline.insert(c.t);
                    cm.insert(c.t, c.c);
                    hm.insert(c.t, c.h);
                    lm.insert(c.t, c.l);
                }
            }
            closes.insert(e.coin.clone(), cm);
        }
        Self {
            ts: timeline.into_iter().collect(),
            closes,
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
    // 与实盘共用同一套因子面板，避免两份实现漂移
    let fp = crate::trader::FactorPanel::build(panel);
    if p.ts.len() < cfg.lookback + 30 + 2 {
        return;
    }

    let fresh = state.last_ts.is_none();
    let days: Vec<usize> = if fresh {
        let last = p.ts.len() - 1;
        let start = last
            .saturating_sub(cfg.replay_days)
            .max(cfg.lookback + 30);
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
        advance(state, &cfg, &p, &fp, max_lev, i, is_first, is_replay);
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
    fp: &crate::trader::FactorPanel,
    max_lev: &HashMap<String, u32>,
    i: usize,
    is_first: bool,
    replayed: bool,
) {
    let t = p.ts[i];
    // 三因子打分（波动调整动量 / 低波动 / 成交量冲击）—— 与实盘同一份代码。
    let tc = crate::trader::TradeConfig {
        lookback: cfg.lookback,
        top_frac: cfg.top_frac,
        min_vol_usd: cfg.min_vol_usd,
        target_positions: cfg.target_positions,
        leverage: cfg.leverage,
        ..Default::default()
    };
    let weights = fp.weights_at(i, &tc, state.equity);
    // 这里曾经写成 `weights.len() < 12`，但重构后它统计的是「实际持仓数」
    // （受 cap 限制，小账户只有几个），而不是「合格币数」。后果是：小账户下
    // 每天都被判定为样本不足直接 return，而 return 发生在更新 last_ts 之前，
    // 于是 last_ts 永远为 None、fresh 永远为真 —— 纸交易永远不交易，净值和
    // 天数冻结在 0。现在改成与实盘一致的判据。
    if weights.is_empty() {
        return;
    }
    // 权重来自「三本账平均」，每仓名义 = |w| × 净值 × 杠杆
    let wmap: HashMap<String, f64> = weights.iter().cloned().collect();
    let long_leg: HashSet<String> = weights
        .iter()
        .filter(|(_, w)| *w > 0.0)
        .map(|(c, _)| c.clone())
        .collect();
    let short_leg: HashSet<String> = weights
        .iter()
        .filter(|(_, w)| *w < 0.0)
        .map(|(c, _)| c.clone())
        .collect();

    // Position size follows current equity so the strategy compounds. Each leg
    // deploys equity*leverage/2 across the names it actually holds, so gross
    // exposure stays at the target leverage even when fewer coins qualify.

    // ---------- first day: open the book ----------
    if is_first {
        for (leg, side) in [(&long_leg, Side::Long), (&short_leg, Side::Short)] {
            for coin in leg {
                let Some(px) = p.px(coin, t) else { continue };
                let ml = max_lev.get(coin).copied().unwrap_or(10);
                let mut pos = PaperPosition {
                    coin: coin.clone(),
                    side,
                    notional: wmap.get(coin).map(|w| w.abs()).unwrap_or(0.0)
                    * state.equity.max(0.0)
                    * MARGIN_BUFFER
                    * cfg.leverage,
                    entry_price: px,
                    entry_ts: t,
                    basis_price: px,
                    realized_carry: 0.0,
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
    // Positions whose isolated margin was wiped out by an intraday move.
    let mut liquidated: Vec<(PaperPosition, f64)> = Vec::new();
    let mut survivors: Vec<PaperPosition> = Vec::new();
    for mut pos in state.positions.drain(..) {
        let prev = p.px(&pos.coin, t_prev);
        let cur = p.px(&pos.coin, t);
        // 实盘是 cross margin：单仓不会单独被强平，只有「账户权益低于全组合
        // 维持保证金」才会。之前按逐仓逐个币判爆仓，模型能表示的失败模式和真正
        // 会终结账户的失败模式不是一回事（改由下面的账户级判定负责）。
        let hit = false;
        if hit {
            // Force-closed at the liquidation price.
            let liq = pos.liq_price;
            if let Some(prev) = prev {
                day_pnl += position_pnl(&pos, liq) - position_pnl(&pos, prev);
            }
            mark(&mut pos, liq, cfg.leverage);
            liquidated.push((pos, liq));
            continue;
        }
        if let (Some(prev), Some(cur)) = (prev, cur) {
            day_pnl += position_pnl(&pos, cur) - position_pnl(&pos, prev);
            mark(&mut pos, cur, cfg.leverage);
        }
        survivors.push(pos);
    }
    state.positions = survivors;
    for (pos, liq) in liquidated {
        let pnl = position_pnl(&pos, liq);
        state.trades.push(Trade {
            coin: pos.coin.clone(),
            side: pos.side,
            entry_ts: pos.entry_ts,
            entry_price: pos.entry_price,
            avg_entry: pos.basis_price,
            exit_ts: t,
            exit_price: liq,
            notional: pos.notional,
            pnl_usd: pnl,
            pnl_pct: if pos.notional > 0.0 { pnl / pos.notional * 100.0 } else { 0.0 },
            reason: "liquidated".into(),
            replayed: pos.replayed,
        });
        state.liquidations += 1;
    }
    // 等权市场基准：用当前流动宇宙全部币的等权涨跌
    let mkt: Vec<f64> = p
        .closes
        .keys()
        .filter_map(|coin| {
            let prev = p.px(coin, t_prev)?;
            let cur = p.px(coin, t)?;
            if prev > 0.0 { Some(cur / prev - 1.0) } else { None }
        })
        .collect();
    if !mkt.is_empty() {
        state.market *= 1.0 + mkt.iter().sum::<f64>() / mkt.len() as f64;
    }
    state.equity += day_pnl;

    // 账户级（全仓）强平：权益跌破全组合维持保证金就清盘。这是实盘真正会终结
    // 账户的机制，也是「能扛多大回撤」唯一该依据的模型。
    let maint: f64 = state
        .positions
        .iter()
        .map(|x| x.notional * maintenance_margin_rate(x.max_leverage))
        .sum();
    if !state.positions.is_empty() && state.equity < maint {
        let doomed: Vec<PaperPosition> = state.positions.drain(..).collect();
        for pos in doomed {
            let px = p.px(&pos.coin, t).unwrap_or(pos.mark_price);
            let pnl = position_pnl(&pos, px);
            state.trades.push(Trade {
                coin: pos.coin.clone(),
                side: pos.side,
                entry_ts: pos.entry_ts,
                entry_price: pos.entry_price,
                avg_entry: pos.basis_price,
                exit_ts: t,
                exit_price: px,
                notional: pos.notional,
                pnl_usd: pnl,
                pnl_pct: if pos.notional > 0.0 {
                    pnl / pos.notional * 100.0
                } else {
                    0.0
                },
                reason: "全仓强平".into(),
                replayed: pos.replayed,
            });
            state.liquidations += 1;
        }
        state.equity = 0.0;
    }

    // ---------- trade membership changes ----------
    // A new rebalance day: re-size every surviving position to the equity-based
    // target (compounding + equal weight), close the ones leaving a leg, and
    // open the ones entering.
    let mut keep: Vec<PaperPosition> = Vec::new();
    let mut turnover = 0.0;
    for pos in state.positions.drain(..) {
        let wanted = match pos.side {
            Side::Long => long_leg.contains(&pos.coin),
            Side::Short => short_leg.contains(&pos.coin),
        };
        let px = p.px(&pos.coin, t);
        if wanted {
            let mut pos = pos;
            if let Some(px) = px {
                let target = wmap.get(&pos.coin).map(|w| w.abs()).unwrap_or(0.0)
                    * state.equity.max(0.0)
                    * MARGIN_BUFFER
                    * cfg.leverage;
                turnover += (target - pos.notional).abs();
                resize(&mut pos, target, px, cfg.leverage);
            }
            keep.push(pos);
        } else {
            // Close the position. If today's close is missing, fall back to the
            // last mark so the realized PnL is not silently lost.
            let exit_px = px.unwrap_or(pos.mark_price);
            if exit_px > 0.0 {
                let pnl = position_pnl(&pos, exit_px);
                state.trades.push(Trade {
                    coin: pos.coin.clone(),
                    side: pos.side,
                    entry_ts: pos.entry_ts,
                    entry_price: pos.entry_price,
                    avg_entry: pos.basis_price,
                    exit_ts: t,
                    exit_price: exit_px,
                    notional: pos.notional,
                    pnl_usd: pnl,
                    pnl_pct: if pos.notional > 0.0 { pnl / pos.notional * 100.0 } else { 0.0 },
                    reason: "rebalance".into(),
                    replayed: pos.replayed,
                });
                turnover += pos.notional;
            }
        }
    }
    state.positions = keep;

    let have: HashSet<(String, Side)> =
        state.positions.iter().map(|x| (x.coin.clone(), x.side)).collect();
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
                notional: wmap.get(coin).map(|w| w.abs()).unwrap_or(0.0)
                    * state.equity.max(0.0)
                    * MARGIN_BUFFER
                    * cfg.leverage,
                entry_price: px,
                entry_ts: t,
                basis_price: px,
                realized_carry: 0.0,
                mark_price: px,
                unrealized_pnl: 0.0,
                liq_price: 0.0,
                max_leverage: ml,
                replayed,
            };
            mark(&mut pos, px, cfg.leverage);
            state.positions.push(pos);
            turnover += wmap.get(coin).map(|w| w.abs()).unwrap_or(0.0)
                * state.equity.max(0.0)
                * MARGIN_BUFFER
                * cfg.leverage;
        }
    }

    // Fee on everything that moved this rebalance: resizes, closes and opens.
    let cost = turnover * cfg.fee;
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
    pub net_notional: f64,
    pub margin_used: f64,
    pub margin_usage_pct: f64,
    /// maintenance margin required for the whole book (cross-margin basis)
    pub maintenance_margin: f64,
    /// account equity at which cross-margin liquidation triggers
    pub liq_equity: f64,
    /// how far equity can fall, in % of equity, before account liquidation
    pub liq_buffer_pct: f64,
    pub nearest_liq_pct: Option<f64>,
    pub days_elapsed: usize,
    pub total_cost: f64,
    pub liquidations: usize,
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
    let net: f64 = state
        .positions
        .iter()
        .map(|x| match x.side {
            Side::Long => x.notional,
            Side::Short => -x.notional,
        })
        .sum();
    let margin: f64 = state.positions.iter().map(|x| x.notional / cfg.leverage).sum();
    // Cross-margin maintenance requirement: each position contributes
    // notional * maintenance_rate(coin).
    let maint: f64 = state
        .positions
        .iter()
        .map(|x| x.notional * maintenance_margin_rate(x.max_leverage))
        .sum();
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
        net_notional: net,
        margin_used: margin,
        margin_usage_pct: if state.equity > 0.0 { margin / state.equity * 100.0 } else { 0.0 },
        maintenance_margin: maint,
        liq_equity: maint,
        liq_buffer_pct: if state.equity > 0.0 {
            ((state.equity - maint) / state.equity * 100.0).max(0.0)
        } else {
            0.0
        },
        nearest_liq_pct: nearest,
        days_elapsed: state.days_elapsed,
        total_cost: state.total_cost,
        liquidations: state.liquidations,
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
