//! Momentum execution: turn the daily signal into a concrete order list and,
//! optionally, send it to Hyperliquid.
//!
//! The plan mirrors the paper engine exactly (long top N / short bottom N,
//! equal notional, position size scaled to current equity), so what you see in
//! the paper panel is what gets traded.

use crate::exchange::{round_size, Acct, Exec, MarketInfo};
use crate::momentum::PanelEntry;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug)]
pub struct TradeConfig {
    pub lookback: usize,
    pub top_frac: f64,
    pub min_vol_usd: f64,
    pub leverage: f64,
    pub target_positions: usize,
    /// cap on adverse price move accepted for an IOC fill
    pub slippage: f64,
    /// exchange minimum order value
    pub min_order_usd: f64,
    /// skip adjustments smaller than this share of the target notional
    pub rebalance_band: f64,
    /// fraction of equity actually deployed, leaving room for fees and slippage
    pub margin_buffer: f64,
}

impl Default for TradeConfig {
    fn default() -> Self {
        Self {
            lookback: 14,
            top_frac: 0.2,
            min_vol_usd: 5_000_000.0,
            leverage: 3.0,
            target_positions: 5,
            slippage: 0.005,
            min_order_usd: 10.0,
            rebalance_band: 0.02,
            margin_buffer: 0.90,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Order {
    pub coin: String,
    pub buy: bool,
    pub reduce_only: bool,
    pub size: f64,
    pub mid: f64,
    pub sz_decimals: u32,
    pub notional: f64,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub equity: f64,
    pub gross_notional: f64,
    pub per_coin: f64,
    pub long_leg: Vec<String>,
    pub short_leg: Vec<String>,
    /// Closes first, then opens/resizes.
    pub orders: Vec<Order>,
    pub notes: Vec<String>,
}

/// Rank the liquid universe by trailing momentum (identical rule to the paper
/// engine) and return (long leg, short leg) with equal counts.
pub fn ranking(
    panel: &[PanelEntry],
    cfg: &TradeConfig,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let vol_window = 30usize;
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
    let ts: Vec<i64> = timeline.into_iter().collect();
    if ts.len() < cfg.lookback + vol_window + 2 {
        return (Vec::new(), Vec::new(), Vec::new());
    }
    let i = ts.len() - 1;
    let t = ts[i];
    let mut sigs: Vec<(String, f64)> = Vec::new();
    let mut liquid: Vec<String> = Vec::new();
    for (coin, cm) in closes.iter() {
        // Only main-DEX perps: the executor does not handle HIP-3 namespaces.
        if coin.contains(':') {
            continue;
        }
        let Some(vm) = dollar_vol.get(coin) else { continue };
        let vols: Vec<f64> = ((i - vol_window)..i)
            .filter_map(|j| vm.get(&ts[j]).copied())
            .collect();
        let avg_vol = vols.iter().sum::<f64>() / vols.len() as f64;
        if vols.len() < 5 || avg_vol < cfg.min_vol_usd {
            continue;
        }
        let (Some(now), Some(past)) = (cm.get(&t), cm.get(&ts[i - cfg.lookback])) else {
            continue;
        };
        if *now <= 0.0 || *past <= 0.0 {
            continue;
        }
        liquid.push(coin.clone());
        sigs.push((coin.clone(), now / past - 1.0));
    }
    sigs.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let k = ((sigs.len() as f64 * cfg.top_frac).round() as usize).max(1);
    let n = k.min(cfg.target_positions).max(1);
    let long: Vec<String> = sigs[sigs.len() - n..].iter().map(|s| s.0.clone()).collect();
    let short: Vec<String> = sigs[..n].iter().map(|s| s.0.clone()).collect();
    (long, short, liquid)
}

/// Build the order list. `mids` must contain every coin we intend to trade.
#[allow(clippy::too_many_arguments)]
pub fn build_plan(
    long: &[String],
    short: &[String],
    acct: &Acct,
    markets: &HashMap<String, MarketInfo>,
    mids: &HashMap<String, f64>,
    cfg: &TradeConfig,
    equity_override: Option<f64>,
) -> Plan {
    let equity = equity_override.unwrap_or(acct.equity);
    let n = long.len().max(1);
    // Deploy only part of the equity so fees and adverse fills cannot push the
    // book past the margin limit mid-run.
    let deployable = equity * cfg.margin_buffer;
    let per_coin = deployable * cfg.leverage / 2.0 / n as f64;
    let mut plan = Plan {
        equity,
        per_coin,
        long_leg: long.to_vec(),
        short_leg: short.to_vec(),
        ..Default::default()
    };

    // wanted: coin -> signed target size
    let mut wanted: HashMap<String, (f64, bool)> = HashMap::new(); // (signed size, is_long)
    for coin in long {
        if let (Some(mid), Some(m)) = (mids.get(coin), markets.get(coin)) {
            let size = round_size(per_coin / mid, m.sz_decimals);
            if size * mid >= cfg.min_order_usd {
                wanted.insert(coin.clone(), (size, true));
            } else {
                plan.notes.push(format!(
                    "{coin}: 目标 ${:.0} 低于最小下单额，跳过（精度 {}）",
                    per_coin, m.sz_decimals
                ));
            }
        }
    }
    for coin in short {
        if let (Some(mid), Some(m)) = (mids.get(coin), markets.get(coin)) {
            let size = round_size(per_coin / mid, m.sz_decimals);
            if size * mid >= cfg.min_order_usd {
                wanted.insert(coin.clone(), (-size, false));
            } else {
                plan.notes.push(format!("{coin}: 目标 ${per_coin:.0} 低于最小下单额，跳过"));
            }
        }
    }

    // 1) close positions that are gone or must flip side
    let mut closes: Vec<Order> = Vec::new();
    let mut opens: Vec<Order> = Vec::new();
    for (coin, cur) in acct.positions.iter() {
        if cur.abs() < 1e-12 {
            continue;
        }
        let Some(mid) = mids.get(coin) else {
            plan.notes
                .push(format!("{coin}: 缺价格，无法处理现有仓位 {cur:.6}"));
            continue;
        };
        let Some(m) = markets.get(coin) else {
            plan.notes.push(format!("{coin}: 缺市场元数据"));
            continue;
        };
        let target = wanted.get(coin).map(|w| w.0).unwrap_or(0.0);
        let flip = cur * target < 0.0;
        if target == 0.0 || flip {
            let size = round_size(cur.abs(), m.sz_decimals);
            if size * mid >= cfg.min_order_usd {
                closes.push(Order {
                    coin: coin.clone(),
                    buy: *cur < 0.0, // buy to close a short
                    reduce_only: true,
                    size,
                    mid: *mid,
                    sz_decimals: m.sz_decimals,
                    notional: size * mid,
                    reason: if flip { "反手平仓".into() } else { "掉出名单平仓".into() },
                });
            } else {
                plan.notes
                    .push(format!("{coin}: 残余仓位 ${:.1} 低于最小下单额，无法平", size * mid));
            }
        } else {
            let delta = target - cur;
            let dsize = round_size(delta.abs(), m.sz_decimals);
            let dnotional = dsize * mid;
            let threshold = cfg.min_order_usd.max(per_coin * cfg.rebalance_band);
            if dnotional >= threshold {
                opens.push(Order {
                    coin: coin.clone(),
                    buy: delta > 0.0,
                    reduce_only: delta.abs() < cur.abs(), // reducing, not flipping
                    size: dsize,
                    mid: *mid,
                    sz_decimals: m.sz_decimals,
                    notional: dnotional,
                    reason: if delta > 0.0 { "加仓到目标".into() } else { "减仓到目标".into() },
                });
            }
        }
    }

    // 2) open positions that are missing entirely
    for (coin, (target, is_long)) in wanted.iter() {
        let cur = acct.positions.get(coin).copied().unwrap_or(0.0);
        let flip = cur * target < 0.0;
        // A flip is handled by the close above; reopen here once it is flat.
        if cur.abs() < 1e-12 || flip {
            let Some(mid) = mids.get(coin) else { continue };
            let Some(m) = markets.get(coin) else { continue };
            let size = round_size(target.abs(), m.sz_decimals);
            if size * mid >= cfg.min_order_usd {
                opens.push(Order {
                    coin: coin.clone(),
                    buy: *is_long,
                    reduce_only: false,
                    size,
                    mid: *mid,
                    sz_decimals: m.sz_decimals,
                    notional: size * mid,
                    reason: if *is_long { "开多".into() } else { "开空".into() },
                });
            }
        }
    }

    plan.gross_notional = opens.iter().map(|o| o.notional).sum();
    plan.orders = closes;
    plan.orders.extend(opens);
    plan
}

/// Send the plan. Returns a human-readable log line per order.
pub async fn execute(
    exec: &Exec,
    plan: &Plan,
    cfg: &TradeConfig,
    markets: &HashMap<String, MarketInfo>,
    live: bool,
) -> Result<Vec<String>> {
    let mut log = Vec::new();
    // Set leverage before any order, clamped to each coin's maximum.
    let mut lev_done: BTreeSet<String> = Default::default();
    for o in &plan.orders {
        if !lev_done.insert(o.coin.clone()) {
            continue;
        }
        let max_lev = markets.get(&o.coin).map(|m| m.max_leverage).unwrap_or(10);
        let want = cfg.leverage.round() as u32;
        let lev = want.clamp(1, max_lev.max(1));
        if lev != want {
            log.push(format!("{}: 最大杠杆 {max_lev}x，按 {lev}x 设置", o.coin));
        }
        if live {
            if let Err(e) = exec.set_leverage(&o.coin, lev).await {
                log.push(format!("{}: 设置杠杆失败 {e}", o.coin));
            }
        }
    }
    for o in &plan.orders {
        let side = if o.buy { "买" } else { "卖" };
        let tag = if o.reduce_only { "只减仓" } else { "开仓" };
        if !live {
            log.push(format!(
                "[DRY-RUN] {} {} {} {:.6} @≈{:.6} (${:.2}) · {}",
                side, o.coin, tag, o.size, o.mid, o.notional, o.reason
            ));
            continue;
        }
        match exec
            .ioc(&o.coin, o.buy, o.reduce_only, o.size, o.mid, cfg.slippage, o.sz_decimals)
            .await
        {
            Ok(status) => log.push(format!(
                "{} {} {} {:.6} · {} · {}",
                side,
                o.coin,
                tag,
                o.size,
                crate::exchange::describe(&status),
                o.reason
            )),
            Err(e) => log.push(format!("{} {} {} 失败: {e}", side, o.coin, tag)),
        }
    }
    Ok(log)
}

/// Fetch mids for every coin the plan touches.
pub async fn fetch_mids(exec: &Exec, coins: &[String]) -> HashMap<String, f64> {
    let mut out = HashMap::new();
    for coin in coins {
        match exec.mid(coin).await {
            Ok(m) if m > 0.0 => {
                out.insert(coin.clone(), m);
            }
            Ok(_) => {}
            Err(e) => eprintln!("{coin}: 取价失败 {e}"),
        }
    }
    out
}

pub fn load_panel(store: &crate::store::Store) -> Result<Vec<PanelEntry>> {
    Ok(store
        .all_panels()
        .context("read candle cache")?
        .into_iter()
        .map(|(coin, candles)| PanelEntry { coin, candles })
        .collect())
}
