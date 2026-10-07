//! Signal and order planning for the live book.
//!
//! The tradeable signal is a composite of three cross-sectional factors, each
//! forming its own equal-weight book (long the top k, short the bottom k) with
//! the three weight vectors averaged:
//!
//!   1. volatility-adjusted momentum - trailing return divided by 20-day vol
//!   2. low volatility                - long calm names, short wild ones
//!   3. volume shock                  - today's dollar volume over the 30-day mean
//!
//! Averaging the *weights* rather than the ranks matters: coins the three
//! factors disagree on net out to zero and drop out, so the book holds the
//! names they agree on. Weights are renormalised to sum|w| = 1 afterwards, and
//! `build_plan` sizes each name as |w| * equity * margin_buffer * leverage.
//!
//! `FactorPanel` owns the panel, the point-in-time liquidity filter and the
//! weights; both the live path and the paper engine call it, so they cannot
//! drift apart.

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
    /// 每仓最小名义：低于此值的币会被剔除，否则仓位太小无法跟随复利
    pub min_position_usd: f64,
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
            min_position_usd: 15.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Order {
    pub coin: String,
    pub buy: bool,
    pub reduce_only: bool,
    /// 带符号的目标仓位（币本位数量）。execute 在平仓后重读账户，用它把开仓
    /// 量算成「目标 − 实际」，避免部分成交/被拒导致方向做反。
    pub target: f64,
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

    /// 某币在某时刻的收盘价，供影子回测取价。
    pub fn close_at(&self, coin: &str, t: i64) -> Option<f64> {
        self.closes.get(coin).and_then(|m| m.get(&t)).copied()
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.ts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ts.is_empty()
    }

    /// 最后一根**已经收盘**的日线索引。
    ///
    /// Hyperliquid 的 candleSnapshot 会把「正在形成」的当根一起返回。如果直接取
    /// len()-1，量能因子拿到的分子是当天的几分钟成交量（而非一整天），除以 30 日
    /// 均值后冲击值接近 0，实盘会选出与回测完全不同的组合；而且最后一根是否已收盘
    /// 取决于刷新任务有没有在午夜后跑过，组合变得依赖时序。
    pub fn last_closed_index(&self, now_ms: i64) -> usize {
        const DAY: i64 = 86_400_000;
        let mut i = self.ts.len().saturating_sub(1);
        while i > 0 && self.ts[i].saturating_add(DAY) > now_ms {
            i -= 1;
        }
        i
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
    pub fn weights_at(&self, i: usize, cfg: &TradeConfig, equity: f64) -> Vec<(String, f64)> {
        // 账户小的时候自动收缩每腿仓位数：交易所最小下单额 $10，仓位太小就
        // 永远跟不上净值增长（复利被卡死）。三本书 × 两条腿最多 6k 个不同币，
        // 所以要求 gross / (6k) >= min_position_usd。
        // 复利需要每个仓位足够大（交易所最小下单额是 $10，仓位太小就调不动），
        // 但分散化更重要：实测 6 个仓位的组合在 90 天里回撤 −92%，而 28 个仓位
        // 是 −26%。所以这里只在仓位会逼近最小下单额时才收缩，且用实测的
        // 「名字数 ≈ 4k」（三本账相互抵消后每腿约 3~4k 个名字）来换算。
        let gross = equity * cfg.margin_buffer * cfg.leverage;
        let cap = if equity > 0.0 && cfg.min_position_usd > 0.0 {
            let max_names = (gross / cfg.min_position_usd).floor().max(2.0) as usize;
            cfg.target_positions.min((max_names / 4).max(1))
        } else {
            cfg.target_positions
        };
        let cap = cap.max(1);

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
            // 窗口必须包含第 i 根：动量的分子用 close[i]、量能的分子用 dvol[i]，
            // 波动若只到 i-1 就比另外两个因子旧一天（不是前视，但口径不一致）。
            let mut rets: Vec<f64> = Vec::with_capacity(vol_lookback);
            for j in (i + 1 - vol_lookback)..=i {
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
        // 三个因子各自选一个等权组合，再把三个组合的权重平均。
        // 这样持有的其实是「三张名单的叠加」（最多 3x2k 个币），
        // 分散化明显好于「先把排名平均、再选一批」——后者回撤大一倍。
        let n = coins.len();
        let mut acc: HashMap<String, f64> = HashMap::new();
        for scores in [&mom_adj, &low_vol, &shock_v] {
            let mut order: Vec<usize> = (0..n).collect();
            // 平局时按币名定序：分数来自 HashMap 迭代，顺序随进程哈希种子变化，
            // 只按分数排会让同一天的组合在不同进程里不一样。
            order.sort_by(|a, b| {
                scores[*a]
                    .partial_cmp(&scores[*b])
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| coins[*a].cmp(&coins[*b]))
            });
            // 2k > n 时 [n-k, k) 会同时落在两条腿里，而判断顺序把重叠区
            // 全给了多头 —— 空头腿不足 k 个，组合变成净多头。所以硬性限制 k <= n/2。
            let k = ((n as f64 * cfg.top_frac).round() as usize)
                .max(1)
                .min(cap)
                .min((n / 2).max(1));
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
        out.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        // 跨因子抵消会把 Σ|w| 压到 1 以下：某币一个因子看多、另一个看空时会相互
        // 抵消（实测约 44 个流动币里有 23 个归零）。不重新归一化的话，实际敞口会
        // 比设置低 13%~30%，而且恰好在三因子分歧最大时最低。
        let sum: f64 = out.iter().map(|(_, w)| w.abs()).sum();
        assert!(sum.is_finite(), "权重和出现了非有限值");
        if sum > 0.0 {
            for (_, w) in out.iter_mut() {
                *w /= sum;
            }
        }

        out
    }
}

/// 目标权重（带符号，Σ|w| = 1）+ 当时的流动宇宙。
/// 实盘与纸交易都调用这里，保证只有一份实现。
pub fn target_weights(
    panel: &[PanelEntry],
    cfg: &TradeConfig,
    equity_hint: f64,
    now_ms: i64,
) -> (Vec<(String, f64)>, Vec<String>) {
    let fp = FactorPanel::build(panel);
    if fp.is_empty() {
        return (Vec::new(), Vec::new());
    }
    // 只用已收盘的日线，避免把正在形成的当根当成收盘价（量能因子会被毁掉）。
    let i = fp.last_closed_index(now_ms);
    let liquid = fp.liquid_at(i, cfg.min_vol_usd, 30);
    let w = fp.weights_at(i, cfg, equity_hint);
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
    // 权重已归一化到 Σ|w| = 1，所以直接按权重缩放名义即可。
    let per_coin = if weights.is_empty() {
        0.0
    } else {
        deployable * cfg.leverage / weights.len() as f64
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
        let notional = w.abs() * deployable * cfg.leverage;
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
                    target: 0.0,
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
            // 复利门槛用「该仓自身的目标名义」算比例带，而不是全组合平均值：
            // 小仓位过去被平均值抬高门槛，长期跟不上净值增长。
            let pos_target = (cur + delta).abs() * mid;
            let threshold = cfg.min_order_usd.max(pos_target * cfg.rebalance_band);
            if dnotional >= threshold {
                opens.push(Order {
                    coin: coin.clone(),
                    buy: delta > 0.0,
                    // 只有「delta 与原仓位反向」才是减仓。之前用 |delta| < |cur|
                    // 判断，会把「加仓」误标成只减仓，实盘会被交易所拒绝。
                    reduce_only: cur * delta < 0.0,
                    target,
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
                    target: if *is_long { size } else { -size },
                    size,
                    mid: *mid,
                    sz_decimals: m.sz_decimals,
                    notional: size * mid,
                    reason: if *is_long { "开多".into() } else { "开空".into() },
                });
            }
        }
    }

    // 真实的目标总名义 = Σ|w| × 可部署 × 杠杆。之前用「开仓单求和」，那是本次
    // 的增量而不是目标敞口，用来展示会误导（平均每仓 × 2 × 腿数同样不准）。
    plan.gross_notional = weights.iter().map(|(_, w)| w.abs()).sum::<f64>()
        * deployable
        * cfg.leverage;
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

    // ---- 2) 重新读账户：既用于设杠杆，也用于把开仓量算成「目标 − 实际」----
    //
    // 之前的做法是直接按计划里的数量开仓。如果某笔平仓只成交了一部分（或被拒），
    // 仓位就与计划不符，而开仓单仍按旧计划发出 —— 结果可能把仓位做成反方向，
    // 而且止盈单还会挂到错误的一侧，把错误固定一整天。
    let after = match exec.account().await {
        Ok(a) => Some(a),
        Err(e) => {
            prelim.push(format!("开仓前重读账户失败，出于安全跳过全部开仓: {e}"));
            None
        }
    };
    let mut all_coins: Vec<String> = Vec::new();
    all_coins.extend(plan.long_leg.iter().cloned());
    all_coins.extend(plan.short_leg.iter().cloned());
    all_coins.extend(plan.orders.iter().map(|o| o.coin.clone()));
    all_coins.sort();
    all_coins.dedup();
    if let Some(after) = after.as_ref() {
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

    // ---- 3) 再开仓 ----
    if let Some(after) = after {
        for (i, o) in plan.orders.iter().enumerate() {
            if o.reduce_only {
                continue;
            }
            let actual = after.positions.get(&o.coin).map(|p| p.size).unwrap_or(0.0);
            // 平仓后方向仍然相反 —— 残余仓位没平掉，只能用只减仓把它清掉，
            // 绝不能按计划继续加另一个方向。
            if actual * o.target < 0.0 && actual.abs() > 1e-12 {
                orders[i] = match exec
                    .ioc(
                        &o.coin,
                        actual < 0.0,
                        true,
                        actual.abs(),
                        o.mid,
                        cfg.slippage,
                        o.sz_decimals,
                    )
                    .await
                {
                    Ok(st) => format!(
                        "{} {} 只减仓平残余 {:.6} · {} · 平仓未完成，纠正方向",
                        if actual < 0.0 { "买" } else { "卖" },
                        o.coin,
                        actual.abs(),
                        crate::exchange::describe(&st)
                    ),
                    Err(e) => format!("{} 纠正残余仓位失败: {e}", o.coin),
                };
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                continue;
            }
            let delta = o.target - actual;
            let dsize = round_size(delta.abs(), o.sz_decimals);
            let dnotional = dsize * o.mid;
            if dnotional < cfg.min_order_usd {
                orders[i] = format!(
                    "{} 已到位（差 ${:.2}），跳过",
                    o.coin, dnotional
                );
                continue;
            }
            let reduce_only = actual * delta < 0.0;
            orders[i] = match exec
                .ioc(
                    &o.coin,
                    delta > 0.0,
                    reduce_only,
                    dsize,
                    o.mid,
                    cfg.slippage,
                    o.sz_decimals,
                )
                .await
            {
                Ok(st) => format!(
                    "{} {} {} {:.6} · {} · {}",
                    if delta > 0.0 { "买" } else { "卖" },
                    o.coin,
                    if reduce_only { "只减仓" } else { "开仓" },
                    dsize,
                    crate::exchange::describe(&st),
                    if (dsize - o.size).abs() > 1e-12 {
                        format!("{}（按实际仓位修正）", o.reason)
                    } else {
                        o.reason.clone()
                    }
                ),
                Err(e) => format!("{} 开仓失败: {e}", o.coin),
            };
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
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


#[cfg(test)]
mod tests {
    use super::*;
    use crate::hl::Candle;

    /// 造一个合成面板：24 个币、50 天，价格与成交量都带确定性噪声。
    fn synthetic() -> Vec<PanelEntry> {
        const DAY: i64 = 86_400_000;
        let t0 = 1_700_000_000_000i64;
        (0..24)
            .map(|j| {
                let mut px = 1.0 + j as f64 * 0.37;
                let candles = (0..50)
                    .map(|d| {
                        // 让各币的动量/波动/量能互不相同，制造跨因子分歧
                        let drift = ((j * 7 + d * 13) % 11) as f64 / 100.0 - 0.05;
                        px *= 1.0 + drift;
                        // 成交量要高于 min_vol_usd（$5M）才会进入流动宇宙
                        let vol = 6_000_000.0 + ((j * 31 + d * 17) % 23) as f64 * 90_000.0;
                        Candle {
                            t: t0 + d as i64 * DAY,
                            o: px,
                            h: px * 1.02,
                            l: px * 0.98,
                            c: px,
                            v: vol,
                        }
                    })
                    .collect();
                PanelEntry {
                    coin: format!("C{j:02}"),
                    candles,
                }
            })
            .collect()
    }

    #[test]
    fn weights_are_normalised_to_unit_gross() {
        // 跨因子会相互抵消，如果不重新归一化，Σ|w| 会小于 1，实际杠杆就会低于设置。
        // 这个不变量一旦退化，实盘敞口会静默缩水 13%~30%。
        let panel = synthetic();
        let cfg = TradeConfig {
            min_position_usd: 0.0,
            ..Default::default()
        };
        let (w, _) = target_weights(&panel, &cfg, 1000.0, i64::MAX);
        assert!(!w.is_empty(), "应该选出组合");
        let gross: f64 = w.iter().map(|(_, x)| x.abs()).sum();
        assert!(
            (gross - 1.0).abs() < 1e-9,
            "Σ|w| 必须是 1，实际 {gross}"
        );
    }

    #[test]
    fn long_and_short_legs_are_balanced() {
        let panel = synthetic();
        let cfg = TradeConfig {
            min_position_usd: 0.0,
            ..Default::default()
        };
        let (w, _) = target_weights(&panel, &cfg, 1000.0, i64::MAX);
        let net: f64 = w.iter().map(|(_, x)| *x).sum();
        assert!(net.abs() < 1e-9, "组合必须市场中性，净敞口 {net}");
    }

    #[test]
    fn only_closed_bars_are_used() {
        // 最后一根如果是「正在形成」的当根，量能因子会拿到几分钟的成交量，
        // 实盘会选出与回测完全不同的组合。用 now 卡在最后一根中间来验证。
        const DAY: i64 = 86_400_000;
        let panel = synthetic();
        let cfg = TradeConfig {
            min_position_usd: 0.0,
            ..Default::default()
        };
        let fp = FactorPanel::build(&panel);
        let last = *fp.ts.last().unwrap();
        let i = fp.last_closed_index(last + DAY / 2);
        assert!(
            fp.ts[i] < last,
            "当根还没收盘，就不能用最后一根；得到索引 {} / {}", i, fp.len()
        );
        let i2 = fp.last_closed_index(last + DAY);
        assert_eq!(i2, fp.len() - 1, "收盘后应能用最后一根");
    }

    #[test]
    fn k_never_exceeds_half_the_universe() {
        // 2k > n 时重叠区会被判成多头，空头腿不足 k 个，组合变成净多头。
        let panel = synthetic();
        let cfg = TradeConfig {
            top_frac: 0.9,
            min_position_usd: 0.0,
            ..Default::default()
        };
        let (w, _) = target_weights(&panel, &cfg, 1000.0, i64::MAX);
        let net: f64 = w.iter().map(|(_, x)| *x).sum();
        assert!(net.abs() < 1e-9, "top_frac=0.9 也不能变成净多头，净敞口 {net}");
    }
}
