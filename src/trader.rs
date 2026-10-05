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
    /// 下单前已持有仓位的币（这些币无法切换保证金模式）
    pub held: Vec<String>,
}

/// 共享的因子面板：把各币的收盘价与成交额对齐到统一时间轴。
/// 实盘与纸交易都调用这里，避免两份实现漂移。
pub struct FactorPanel {
    pub ts: Vec<i64>,
    closes: HashMap<String, BTreeMap<i64, f64>>,
    dvol: HashMap<String, BTreeMap<i64, f64>>,
}

impl FactorPanel {
    pub fn build(panel: &[PanelEntry]) -> Self {
        let mut timeline: BTreeSet<i64> = Default::default();
        let mut closes: HashMap<String, BTreeMap<i64, f64>> = Default::default();
        let mut dvol: HashMap<String, BTreeMap<i64, f64>> = Default::default();
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
            dvol.insert(e.coin.clone(), vm);
        }
        Self {
            ts: timeline.into_iter().collect(),
            closes,
            dvol,
        }
    }

    pub fn len(&self) -> usize {
        self.ts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ts.is_empty()
    }

    /// 某一时点上「滚动 30 日成交额达标」的币 —— 严格只用截至该时点的数据。
    pub fn liquid_at(&self, i: usize, min_vol_usd: f64, vol_win: usize) -> Vec<String> {
        if i < vol_win {
            return Vec::new();
        }
        let t = self.ts[i];
        let mut out = Vec::new();
        for (coin, cm) in self.closes.iter() {
            if coin.contains(':') {
                continue;
            }
            if !cm.contains_key(&t) {
                continue;
            }
            let Some(vm) = self.dvol.get(coin) else { continue };
            let mut sum = 0.0;
            let mut n = 0usize;
            for j in (i - vol_win)..i {
                if let Some(v) = vm.get(&self.ts[j]) {
                    sum += v;
                    n += 1;
                }
            }
            if n < 5 {
                continue;
            }
            if sum / n as f64 >= min_vol_usd {
                out.push(coin.clone());
            }
        }
        out
    }

    /// 三因子目标权重（带符号，绝对值之和为 1）。因子：
    ///   1. 波动调整动量 = 回看期收益 ÷ 20 日波动（偏好稳定上涨而非一根大阳线）
    ///   2. 低波动      = 负的 20 日波动（做多低波动、做空高波动）
    ///   3. 成交量冲击  = 当日成交额 ÷ 30 日均值
    /// 每个因子先在横截面上排名，再等权平均，避免量纲差异。
    pub fn weights_at(&self, i: usize, cfg: &TradeConfig) -> Vec<(String, f64)> {
        let vol_win = 30usize;
        let shock_win = 30usize;
        let vol_lookback = 20usize;
        if i < cfg.lookback.max(vol_win) + 2 {
            return Vec::new();
        }
        let t = self.ts[i];
        let t_past = self.ts[i - cfg.lookback];

        let mut coins: Vec<String> = Vec::new();
        let mut mom_adj: Vec<f64> = Vec::new();
        let mut low_vol: Vec<f64> = Vec::new();
        let mut shock_v: Vec<f64> = Vec::new();

        for (coin, cm) in self.closes.iter() {
            if coin.contains(':') {
                continue;
            }
            let Some(vm) = self.dvol.get(coin) else { continue };
            let mut sum = 0.0;
            let mut n = 0usize;
            for j in (i - vol_win)..i {
                if let Some(v) = vm.get(&self.ts[j]) {
                    sum += v;
                    n += 1;
                }
            }
            if n < 5 {
                continue;
            }
            let avg_vol = sum / n as f64;
            if avg_vol < cfg.min_vol_usd {
                continue;
            }
            let (Some(now), Some(past)) = (cm.get(&t), cm.get(&t_past)) else {
                continue;
            };
            if *now <= 0.0 || *past <= 0.0 {
                continue;
            }
            // 20 日波动
            let mut rets: Vec<f64> = Vec::with_capacity(vol_lookback);
            for j in (i - vol_lookback)..i {
                if j == 0 {
                    continue;
                }
                let (Some(a), Some(b)) = (cm.get(&self.ts[j]), cm.get(&self.ts[j - 1])) else {
                    continue;
                };
                if *b > 0.0 {
                    rets.push(a / b - 1.0);
                }
            }
            if rets.len() < 5 {
                continue;
            }
            let mean = rets.iter().sum::<f64>() / rets.len() as f64;
            let var = rets.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (rets.len() - 1) as f64;
            let vol = var.sqrt().max(1e-9);
            // 成交量冲击
            let mut vsum = 0.0;
            let mut vn = 0usize;
            for j in (i - shock_win)..i {
                if let Some(v) = vm.get(&self.ts[j]) {
                    vsum += v;
                    vn += 1;
                }
            }
            let shock = if vn > 0 && vsum > 0.0 {
                vm.get(&t).copied().unwrap_or(0.0) / (vsum / vn as f64)
            } else {
                1.0
            };
            coins.push(coin.clone());
            mom_adj.push((now / past - 1.0) / vol);
            low_vol.push(-vol);
            shock_v.push(shock);
        }
        if coins.len() < 8 {
            return Vec::new();
        }
        // 逐因子排名（升序名次），再等权平均
        let rank_of = |v: &[f64]| -> Vec<f64> {
            let mut idx: Vec<usize> = (0..v.len()).collect();
            idx.sort_by(|a, b| v[*a].partial_cmp(&v[*b]).unwrap_or(std::cmp::Ordering::Equal));
            let mut r = vec![0.0; v.len()];
            for (pos, &j) in idx.iter().enumerate() {
                r[j] = pos as f64;
            }
            r
        };
        let _ = rank_of;
        // 三个因子各自选一个等权组合，再把三个组合的权重平均。
        // 这样持有的其实是「三张名单的叠加」（最多 3x2k 个币），
        // 分散化明显好于「先把排名平均、再选一批」——后者回撤大一倍。
        let n = coins.len();
        let mut acc: HashMap<String, f64> = HashMap::new();
        for scores in [&mom_adj, &low_vol, &shock_v] {
            let mut order: Vec<usize> = (0..n).collect();
            order.sort_by(|a, b| {
                scores[*a]
                    .partial_cmp(&scores[*b])
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let k = ((n as f64 * cfg.top_frac).round() as usize)
                .max(1)
                .min(cfg.target_positions);
            for (pos, &j) in order.iter().enumerate() {
                let w = if pos >= n - k {
                    0.5 / k as f64
                } else if pos < k {
                    -0.5 / k as f64
                } else {
                    continue;
                };
                *acc.entry(coins[j].clone()).or_insert(0.0) += w / 3.0;
            }
        }
        let mut out: Vec<(String, f64)> = acc
            .into_iter()
            .filter(|(_, w)| w.abs() > 1e-12)
            .collect();
        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        out
    }
}

/// 目标权重（带符号，Σ|w| = 1）+ 当时的流动宇宙。
/// 实盘与纸交易都调用这里，保证只有一份实现。
pub fn target_weights(
    panel: &[PanelEntry],
    cfg: &TradeConfig,
) -> (Vec<(String, f64)>, Vec<String>) {
    let fp = FactorPanel::build(panel);
    if fp.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let i = fp.len() - 1;
    let liquid = fp.liquid_at(i, cfg.min_vol_usd, 30);
    let w = fp.weights_at(i, cfg);
    (w, liquid)
}

/// Build the order list. `mids` must contain every coin we intend to trade.
#[allow(clippy::too_many_arguments)]
pub fn build_plan(
    weights: &[(String, f64)],
    acct: &Acct,
    markets: &HashMap<String, MarketInfo>,
    mids: &HashMap<String, f64>,
    cfg: &TradeConfig,
    equity_override: Option<f64>,
) -> Plan {
    let equity = equity_override.unwrap_or(acct.equity);
    // Deploy only part of the equity so fees and adverse fills cannot push the
    // book past the margin limit mid-run.
    let deployable = equity * cfg.margin_buffer;
    let gross_w: f64 = weights.iter().map(|(_, w)| w.abs()).sum();
    let per_coin = if weights.is_empty() {
        0.0
    } else {
        deployable * cfg.leverage * gross_w / weights.len() as f64
    };
    let long_leg: Vec<String> = weights
        .iter()
        .filter(|(_, w)| *w > 0.0)
        .map(|(c, _)| c.clone())
        .collect();
    let short_leg: Vec<String> = weights
        .iter()
        .filter(|(_, w)| *w < 0.0)
        .map(|(c, _)| c.clone())
        .collect();
    let mut plan = Plan {
        equity,
        per_coin,
        long_leg,
        short_leg,
        held: acct
            .positions
            .iter()
            .filter(|(_, p)| p.size.abs() > 1e-12)
            .map(|(c, _)| c.clone())
            .collect(),
        ..Default::default()
    };

    // wanted: coin -> signed target size（按权重缩放名义）
    let mut wanted: HashMap<String, (f64, bool)> = HashMap::new();
    for (coin, w) in weights {
        if w.abs() < 1e-12 {
            continue;
        }
        let (Some(mid), Some(m)) = (mids.get(coin), markets.get(coin)) else {
            continue;
        };
        let notional = w.abs() * deployable * cfg.leverage * gross_w;
        let size = round_size(notional / mid, m.sz_decimals);
        if size * mid >= cfg.min_order_usd {
            wanted.insert(coin.clone(), (if *w > 0.0 { size } else { -size }, *w > 0.0));
        } else {
            plan.notes.push(format!(
                "{coin}: 目标 ${:.0} 低于最小下单额，跳过",
                size * mid
            ));
        }
    }

    // 1) close positions that are gone or must flip side
    let mut closes: Vec<Order> = Vec::new();
    let mut opens: Vec<Order> = Vec::new();
    for (coin, pos) in acct.positions.iter() {
        let cur = pos.size;
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
                    buy: cur < 0.0, // buy to close a short
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
                    // 只有「delta 与原仓位反向」才是减仓。之前用 |delta| < |cur|
                    // 判断，会把「加仓」误标成只减仓，实盘会被交易所拒绝。
                    reduce_only: cur * delta < 0.0,
                    size: dsize,
                    mid: *mid,
                    sz_decimals: m.sz_decimals,
                    notional: dnotional,
                    // 文案要看「相对原仓位是增还是减」，而不是买/卖方向：
                    // 空头买回是减仓、空头继续卖才是加仓。
                    reason: if cur * delta > 0.0 {
                        "加仓到目标".into()
                    } else {
                        "减仓到目标".into()
                    },
                });
            }
        }
    }

    // 2) open positions that are missing entirely
    for (coin, (target, is_long)) in wanted.iter() {
        let cur = acct.positions.get(coin).map(|p| p.size).unwrap_or(0.0);
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

/// 执行结果：前置日志（设杠杆等）与**逐条对应 plan.orders 的结果**分开返回，
/// 避免用下标对应时错位（曾经导致失败信息挂到别的币上）。
pub struct Outcome {
    pub prelim: Vec<String>,
    pub orders: Vec<String>,
}

/// Send the plan. `orders[i]` always corresponds to `plan.orders[i]`.
///
/// 顺序很重要：先平仓 → 重新读账户 → 给已经空仓的币设杠杆（Hyperliquid
/// 不允许持仓时切换保证金模式）→ 再开仓。这样反手的币也能切成全仓。
pub async fn execute(
    exec: &Exec,
    plan: &Plan,
    cfg: &TradeConfig,
    markets: &HashMap<String, MarketInfo>,
    live: bool,
) -> Result<Outcome> {
    let mut prelim: Vec<String> = Vec::new();
    let mut orders: Vec<String> = vec![String::new(); plan.orders.len()];

    if !live {
        for (i, o) in plan.orders.iter().enumerate() {
            orders[i] = format!(
                "[DRY-RUN] {} {} {} {:.6} @≈{:.6} (${:.2}) · {}",
                if o.buy { "买" } else { "卖" },
                o.coin,
                if o.reduce_only { "只减仓" } else { "开仓" },
                o.size,
                o.mid,
                o.notional,
                o.reason
            );
        }
        return Ok(Outcome { prelim, orders });
    }

    // ---- 1) 先平仓 ----
    for (i, o) in plan.orders.iter().enumerate() {
        if !o.reduce_only {
            continue;
        }
        orders[i] = match exec
            .ioc(&o.coin, o.buy, true, o.size, o.mid, cfg.slippage, o.sz_decimals)
            .await
        {
            Ok(st) => format!(
                "{} {} 只减仓 {:.6} · {} · {}",
                if o.buy { "买" } else { "卖" },
                o.coin,
                o.size,
                crate::exchange::describe(&st),
                o.reason
            ),
            Err(e) => format!("{} {} 平仓失败: {e}", if o.buy { "买" } else { "卖" }, o.coin),
        };
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }

    // ---- 2) 重新读账户，给已空仓的目标币设杠杆（全仓）----
    let mut all_coins: Vec<String> = Vec::new();
    all_coins.extend(plan.long_leg.iter().cloned());
    all_coins.extend(plan.short_leg.iter().cloned());
    all_coins.extend(plan.orders.iter().map(|o| o.coin.clone()));
    all_coins.sort();
    all_coins.dedup();
    match exec.account().await {
        Ok(after) => {
            for coin in &all_coins {
                if after
                    .positions
                    .get(coin)
                    .map(|p| p.size.abs() > 1e-12)
                    .unwrap_or(false)
                {
                    continue; // 仍有仓位，切不了模式，跳过
                }
                let max_lev = markets.get(coin).map(|m| m.max_leverage).unwrap_or(10);
                let want = cfg.leverage.round() as u32;
                let lev = want.clamp(1, max_lev.max(1));
                match exec.set_leverage(coin, lev).await {
                    Ok(_) => {}
                    Err(e) => prelim.push(format!("{coin}: 设置杠杆失败 {e}")),
                }
                tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            }
        }
        Err(e) => prelim.push(format!("重新读取账户失败，跳过杠杆设置: {e}")),
    }

    // ---- 3) 再开仓 ----
    for (i, o) in plan.orders.iter().enumerate() {
        if o.reduce_only {
            continue;
        }
        orders[i] = match exec
            .ioc(&o.coin, o.buy, false, o.size, o.mid, cfg.slippage, o.sz_decimals)
            .await
        {
            Ok(st) => format!(
                "{} {} 开仓 {:.6} · {} · {}",
                if o.buy { "买" } else { "卖" },
                o.coin,
                o.size,
                crate::exchange::describe(&st),
                o.reason
            ),
            Err(e) => format!("{} {} 开仓失败: {e}", if o.buy { "买" } else { "卖" }, o.coin),
        };
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }

    Ok(Outcome { prelim, orders })
}

/// Mids for the coins we care about, from one bulk request.
pub async fn fetch_mids(exec: &Exec, coins: &[String]) -> HashMap<String, f64> {
    let all = match exec.all_mids().await {
        Ok(m) => m,
        Err(e) => {
            eprintln!("取价失败: {e}");
            return HashMap::new();
        }
    };
    coins
        .iter()
        .filter_map(|c| all.get(c).map(|p| (c.clone(), *p)))
        .collect()
}

pub fn load_panel(store: &crate::store::Store) -> Result<Vec<PanelEntry>> {
    Ok(store
        .all_panels()
        .context("read candle cache")?
        .into_iter()
        .map(|(coin, candles)| PanelEntry { coin, candles })
        .collect())
}
