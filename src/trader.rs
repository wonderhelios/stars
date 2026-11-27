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
    /// **按净值比例的最小下单额**（0.01 = 1%）。
    ///
    /// 为什么需要它：`min_order_usd` 是绝对值，而仓位大小随账号缩放 ——
    /// 同一个 $10 门槛，对 $580 的账号是「一个仓位的 17.7%」（粗得几乎不调仓），
    /// 对 $2900 的账号是「3.8%」（每天微调一堆）。这解释了不同账号的调仓频率差异
    /// 的真实原因：**两个账号用同一个策略，但执行粒度差了 5 倍**。
    ///
    /// 有效门槛 = max(min_order_usd, 净值 × min_order_pct)。
    /// 保留绝对下限是因为交易所本身有最小下单量，比例不能低于它。
    pub min_order_pct: f64,
    /// 平滑 EMA 调仓系数（0.1~1.0；1.0 = 一步到位）。仅当 `rebalance_slices == 1` 时生效。
    pub ema_alpha: f64,
    /// skip adjustments smaller than this share of the target notional (`rebal_band`, 0.0~0.50)
    pub rebal_band: f64,
    /// 总敞口硬上限（账户净值倍数，如 2.8 = 2.8x 净值），防止低波动时 `vol_k` 放大杠杆超限。
    pub hard_gross_cap: f64,
    /// fraction of equity actually deployed, leaving room for fees and slippage
    pub margin_buffer: f64,
    /// 每仓最小名义：低于此值的币会被剔除，否则仓位太小无法跟随复利
    pub min_position_usd: f64,
    /// 错峰调仓的档数。每个币每 N 天轮到一次，而不是每天全量调。1 = 每日平滑/全量调仓。
    pub rebalance_slices: u32,
    /// 单币止损幅度（相对持仓成本，0 = 关闭）。触发时无论是否错峰/EMA 均立即平仓。
    pub position_stop_pct: f64,
    /// 止盈幅度（相对【持仓成本】）。0 = 不启用。调仓时会先把已经达到
    /// 成本的 (1+tp) 的仓位按「止盈」平掉，未达到的由交易所的限价止盈单负责。
    pub take_profit_pct: f64,
    /// 「距 14 日高点」因子权重（0 = 不启用）。打低「涨完就阴跌」的币。
    pub dd_weight: f64,
    /// 「14 日上涨天数占比」因子权重（0 = 不启用）。打低「一根阳线拉起就阴跌」的币。
    pub upfrac_weight: f64,
    /// 因子权重：动量占比（0~1），量能占 1 − 它。默认 0.5（两因子等权）。
    ///
    /// 证据（2026-10-17 复盘检验，真实样本 2024-07 起，2023 年币池中位数为 0 已排除）：
    ///   量能对动量的【1 日】残差 IC ≈ +0.003（t=0.29，不显著），2 日 +0.019（t=1.92），
    ///   5 日 +0.042（t=3.86）—— 量能的信息在更慢的期限上，日频用不到。
    ///   组合层面：50/50 → 70/30，Sharpe 2.42→2.60（无止盈）/ 3.14→3.37（10% 止盈），
    ///   换手 1.69→1.45，两个半段都改善（2.58/2.30 → 2.61/2.60）。
    /// 诚实标注：样本内结论，尚未前向验证。默认保持 0.5，不静默改变实盘行为。
    pub momentum_weight: f64,
    /// 分腿逆波动率加权（1/σ_20d，截断到 [0.01, 0.25] 并分别归一化多空两腿各占 0.5）。
    pub inv_vol: bool,
    /// 单币目标名义占账户净值的上限（0 = 关闭，0.20 = 单币不超过净值的 20%）。
    /// 作用在 vol_target 缩放后的最终目标权重上，超限部分在同侧其他未封顶币间等杠杆再分配。
    pub max_single_weight: f64,
}

impl TradeConfig {
    /// 有效最小下单额：绝对下限与"净值的比例"取大者。
    ///
    /// 比例设成 0（默认）时行为与旧版完全一致，不会影响既有配置。
    pub fn effective_min_order_usd(&self, equity: f64) -> f64 {
        let pct = if self.min_order_pct.is_finite() && self.min_order_pct > 0.0 {
            equity * self.min_order_pct
        } else {
            0.0
        };
        self.min_order_usd.max(pct)
    }
}


impl Default for TradeConfig {
    fn default() -> Self {
        Self {
            lookback: 14,
            top_frac: 0.30,
            min_vol_usd: 5_000_000.0,
            leverage: 3.0,
            target_positions: 5,
            slippage: 0.005,
            min_order_usd: 10.0,
            min_order_pct: 0.005,
            ema_alpha: 0.50,
            rebal_band: 0.05,
            hard_gross_cap: 2.8,
            margin_buffer: 0.90,
            min_position_usd: 15.0,
            rebalance_slices: 1,
            position_stop_pct: 0.0,
            momentum_weight: 0.5,
            take_profit_pct: 0.0,
            dd_weight: 0.0,
            upfrac_weight: 0.0,
            inv_vol: false,
            max_single_weight: 0.0,
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
    pub sz_decimals: i32,
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

    /// 每个交易日的数据是否完整（与 paper 回测同口径）。
    pub fn complete_days(&self) -> Vec<bool> {
        complete_days(&self.ts, &self.closes)
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
        let mut vol_map: HashMap<String, f64> = HashMap::new();
        let mut shock_v: Vec<f64> = Vec::new();
        let mut dd_v: Vec<f64> = Vec::new();
        let mut upfrac_v: Vec<f64> = Vec::new();

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
            // dd14：现价相对【过去 lookback 天（不含今天）最高价】的位置。0 = 创新高。
            let mut hi = f64::NEG_INFINITY;
            let mut ups = 0usize;
            let mut win_n = 0usize;
            for j in (i + 1 - cfg.lookback)..i {
                if j == 0 {
                    continue;
                }
                let (Some(a), Some(b)) = (cm.get(&self.ts[j]), cm.get(&self.ts[j - 1])) else {
                    continue;
                };
                if *b <= 0.0 {
                    continue;
                }
                if let Some(a) = cm.get(&self.ts[j]) {
                    if *a > hi {
                        hi = *a;
                    }
                }
                if a > b {
                    ups += 1;
                }
                win_n += 1;
            }
            let dd = if hi > 0.0 && hi.is_finite() { now / hi - 1.0 } else { 0.0 };
            let upfrac = if win_n > 0 { ups as f64 / win_n as f64 } else { 0.5 };

            vol_map.insert(coin.clone(), vol);
            coins.push(coin.clone());
            mom_adj.push((now / past - 1.0) / vol);
            low_vol.push(-vol);
            shock_v.push(shock);
            dd_v.push(dd);
            upfrac_v.push(upfrac);
        }
        if coins.len() < 8 {
            return Vec::new();
        }
        // 三个因子各自选一个等权组合，再把三个组合的权重平均。
        // 这样持有的其实是「三张名单的叠加」（最多 3x2k 个币），
        // 分散化明显好于「先把排名平均、再选一批」——后者回撤大一倍。
        let n = coins.len();
        let mut acc: HashMap<String, f64> = HashMap::new();
        // ★ 已移除「低波动」因子（原为 [&mom_adj, &low_vol, &shock_v]）。
        //
        // 证据（research/hl_polish_20261011/factor_test.py，自检复现基线 1.1293）：
        //   三因子  Sharpe 1.1293 · 年化 89.9% · 换手 1.598 · 回撤 −80.9%
        //   删低波动 Sharpe 1.2194 · 年化 111.8% · 换手 1.759 · 回撤 −79.8%
        //
        // 理由是它的 IC 全样本为负（−0.06，t=−10.4）——「做多低波动币」在这个
        // 横截面上是反预测的。删掉它，动量和量冲击各占一半。
        //
        // 诚实标注：ΔSharpe +0.09 的配对块 bootstrap p=0.41，**不显著**，
        // 且换手 +10%（年成本 40.8%→44.9%）、波动变大（4.17%→4.80%）。
        // 这是一次方向正确但边际的改动，不是"验证过的改进"。
        // 要回退：把 low_vol 加回列表、并把下面的 /2.0 改回 /3.0。
        let mw = cfg.momentum_weight.clamp(0.0, 1.0);
        // 四因子权重：动量 / 量能 / 距高点 / 上涨占比。dd/upfrac 默认 0，行为与旧版一致。
        // 先按总权重归一化，再各自选等权组合、最后平均三本书（Σ|w| 会再归一化一次）。
        let raw_weights: [(&Vec<f64>, f64); 4] = [
            (&mom_adj, mw),
            (&shock_v, 1.0 - mw),
            (&dd_v, cfg.dd_weight.max(0.0)),
            (&upfrac_v, cfg.upfrac_weight.max(0.0)),
        ];
        let wsum: f64 = raw_weights.iter().map(|(_, w)| *w).sum();
        if wsum <= 0.0 {
            return Vec::new();
        }
        for (scores, fw0) in raw_weights {
            let fw = fw0 / wsum;
            if fw <= 0.0 {
                continue;
            }
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
                    fw / 2.0 / k as f64
                } else if pos < k {
                    -fw / 2.0 / k as f64
                } else {
                    continue;
                };
                *acc.entry(coins[j].clone()).or_insert(0.0) += w;
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
        if cfg.inv_vol {
            for (coin, w) in out.iter_mut() {
                let v = vol_map.get(coin).copied().unwrap_or(0.05).clamp(0.01, 0.25);
                *w /= v;
            }
            let long_sum: f64 = out.iter().filter(|(_, w)| *w > 0.0).map(|(_, w)| *w).sum();
            let short_sum: f64 = out.iter().filter(|(_, w)| *w < 0.0).map(|(_, w)| w.abs()).sum();
            if long_sum > 0.0 && short_sum > 0.0 {
                for (_, w) in out.iter_mut() {
                    if *w > 0.0 {
                        *w *= 0.5 / long_sum;
                    } else {
                        *w *= 0.5 / short_sum;
                    }
                }
            } else {
                let sum: f64 = out.iter().map(|(_, w)| w.abs()).sum();
                if sum > 0.0 {
                    for (_, w) in out.iter_mut() {
                        *w /= sum;
                    }
                }
            }
        } else {
            let sum: f64 = out.iter().map(|(_, w)| w.abs()).sum();
            assert!(sum.is_finite(), "权重和出现了非有限值");
            if sum > 0.0 {
                for (_, w) in out.iter_mut() {
                    *w /= sum;
                }
            }
        }

        out
    }
}

/// 净敞口保护：只做减仓，不新增开仓。没有目标权重时按同侧仓位比例分摊，
/// 并始终按币名排序，避免 HashMap 随机顺序导致同一账户每次砍掉不同的币。
pub fn neutralize_orders(
    acct: &Acct,
    markets: &std::collections::HashMap<String, MarketInfo>,
    mids: &std::collections::HashMap<String, f64>,
    tolerance: f64,
    min_order_usd: f64,
) -> Vec<Order> {
    neutralize_orders_with_targets(acct, markets, mids, tolerance, min_order_usd, None)
}

/// 目标感知的中性化：先按各币「当前名义 − 同向目标名义」的超额比例减仓；
/// 若仍不够，再按剩余仓位比例分摊。目标名义为带符号的美元名义。
/// 这样恢复市场中性时，不会无差别地把刚调到目标附近的仓位整笔清零。
pub fn neutralize_orders_with_targets(
    acct: &Acct,
    markets: &std::collections::HashMap<String, MarketInfo>,
    mids: &std::collections::HashMap<String, f64>,
    tolerance: f64,
    min_order_usd: f64,
    target_notionals: Option<&std::collections::HashMap<String, f64>>,
) -> Vec<Order> {
    let mut long_n = 0.0;
    let mut short_n = 0.0;
    for (c, p) in acct.positions.iter() {
        let Some(&mid) = mids.get(c) else { continue };
        if !mid.is_finite() || mid <= 0.0 {
            continue;
        }
        let n = p.size.abs() * mid;
        if p.size > 0.0 {
            long_n += n;
        } else {
            short_n += n;
        }
    }
    let gross = long_n + short_n;
    let net = long_n - short_n;
    if gross <= 0.0 || net.abs() <= (tolerance * gross).max(min_order_usd) {
        return Vec::new();
    }
    let reduce_longs = net > 0.0;
    let side_sign = if reduce_longs { 1.0 } else { -1.0 };
    let mut coins: Vec<String> = acct
        .positions
        .iter()
        .filter(|(_, p)| (p.size > 0.0) == reduce_longs)
        .map(|(c, _)| c.clone())
        .collect();
    coins.sort();
    let mut remaining: std::collections::HashMap<String, f64> = coins
        .iter()
        .filter_map(|c| {
            let p = acct.positions.get(c)?;
            let mid = mids.get(c)?;
            Some((c.clone(), p.size.abs() * mid))
        })
        .collect();
    let mut budget = net.abs();
    let mut out: Vec<Order> = Vec::new();
    let mut reduce = |coin: &str,
                      want: f64,
                      remaining: &mut std::collections::HashMap<String, f64>,
                      budget: f64|
     -> f64 {
        if budget < min_order_usd || want <= 0.0 {
            return budget;
        }
        let Some(p) = acct.positions.get(coin) else { return budget };
        let (Some(&mid), Some(m)) = (mids.get(coin), markets.get(coin)) else { return budget };
        if !mid.is_finite() || mid <= 0.0 {
            return budget;
        }
        let available = remaining.get(coin).copied().unwrap_or(0.0);
        let take = available.min(want).min(budget);
        let size = round_size(take / mid, m.sz_decimals);
        if size * mid < min_order_usd {
            return budget;
        }
        let target = p.size - p.size.signum() * size;
        out.push(Order {
            coin: coin.to_string(),
            buy: p.size < 0.0,
            reduce_only: true,
            target,
            size,
            mid,
            sz_decimals: m.sz_decimals,
            notional: size * mid,
            reason: "净敞口中性化".into(),
        });
        remaining.insert(coin.to_string(), (available - size * mid).max(0.0));
        (budget - size * mid).max(0.0)
    };

    // 第一阶段：只减超出模型目标的部分，并按超额比例分配。
    if let Some(targets) = target_notionals {
        let mut excess: Vec<(String, f64)> = Vec::new();
        for c in &coins {
            let cur = remaining.get(c).copied().unwrap_or(0.0);
            let signed_target = targets.get(c).copied().unwrap_or(0.0);
            let same_side_target = if signed_target * side_sign > 0.0 {
                signed_target.abs().min(cur)
            } else {
                0.0
            };
            let x = (cur - same_side_target).max(0.0);
            if x > 1e-9 {
                excess.push((c.clone(), x));
            }
        }
        let total: f64 = excess.iter().map(|(_, x)| *x).sum();
        if total > 1e-9 {
            let pass_budget = budget;
            for (c, x) in excess {
                let allocation = if pass_budget < total { pass_budget * x / total } else { x };
                budget = reduce(&c, allocation, &mut remaining, budget);
            }
        }
    }

    // 第二阶段：超额部分仍不足时，按剩余名义比例分摊，不随机整币清仓。
    let leg_remaining: f64 = remaining.values().sum();
    if leg_remaining > 1e-9 {
        let pass_budget = budget;
        for c in &coins {
            let cur = remaining.get(c).copied().unwrap_or(0.0);
            budget = reduce(c, pass_budget * cur / leg_remaining, &mut remaining, budget);
        }
    }
    // 数量向下取整可能留下小额预算。这里保留这个误差，不再按币名把余量
    // 集中压给某一个仓，否则会破坏上面的比例分摊并可能整币清零。

    // 同一币可能先减超额、再参与比例分摊。聚合成每币一笔，避免重复手续费，
    // 并让 target 表示全部中性化订单执行后的最终仓位。
    let mut sizes: BTreeMap<String, f64> = BTreeMap::new();
    for order in out {
        *sizes.entry(order.coin).or_insert(0.0) += order.size;
    }
    sizes
        .into_iter()
        .filter_map(|(coin, size)| {
            let p = acct.positions.get(&coin)?;
            let (&mid, m) = (mids.get(&coin)?, markets.get(&coin)?);
            Some(Order {
                coin,
                buy: p.size < 0.0,
                reduce_only: true,
                target: p.size - p.size.signum() * size,
                size,
                mid,
                sz_decimals: m.sz_decimals,
                notional: size * mid,
                reason: "净敞口中性化".into(),
            })
        })
        .collect()
}

/// 判定哪些交易日的数据是完整的：当天有数据的币数 ≥ 最近 60 天最大覆盖数的 60%。
///
/// 直连接口只给部分币补了最近日线时，面板尾部会出现「只有几十个币有数据」的
/// 残缺日。任何按残缺日调仓的回测/影子曲线都会失真，所以两边共用这一个判定。
pub fn complete_days(ts: &[i64], closes: &HashMap<String, BTreeMap<i64, f64>>) -> Vec<bool> {
    let idx: HashMap<i64, usize> = ts.iter().enumerate().map(|(i, &t)| (t, i)).collect();
    let mut cov = vec![0usize; ts.len()];
    for data in closes.values() {
        for &t in data.keys() {
            if let Some(&i) = idx.get(&t) {
                cov[i] += 1;
            }
        }
    }
    let mut out = vec![false; ts.len()];
    for i in 0..ts.len() {
        let lo = i.saturating_sub(59);
        let ref_n = cov[lo..=i].iter().copied().max().unwrap_or(0);
        out[i] = ref_n > 0 && (cov[i] as f64) >= 0.6 * ref_n as f64;
    }
    out
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

/// 某个币属于哪一档。用币名的字节和取模，保证跨进程、跨重启都一致。
pub fn slice_of(coin: &str, slices: u32) -> u32 {
    if slices <= 1 {
        return 0;
    }
    let sum: u32 = coin.bytes().map(|b| b as u32).sum();
    sum % slices
}

/// Build the order list. `mids` must contain every coin we intend to trade.
/// 将目标权重截断到单币上限，并把超限部分在同侧（多头或空头）其他未封顶币间等杠杆水满再分配。
///
/// `weights` 的口径：`|w_i| * margin_buffer * leverage = 单币目标名义 / 账户净值`。
/// 波动率系数已经乘在 `weights` 里，订单金额也只直接使用这份最终权重；
/// 换算上限和计算订单时都不能再额外乘一次波动率系数。
/// 因此 `max_single_weight`（占净值比例，如 0.20）折算到权重空间为
/// `max_w = max_single_weight / (margin_buffer * leverage)`。
/// 该函数是幂等的，返回被截断过的币种列表（供计划日志展示）。
pub fn cap_weights_same_side(
    weights: &mut [(String, f64)],
    max_single_weight: f64,
    margin_buffer: f64,
    leverage: f64,
) -> Vec<String> {
    let gross_mult = margin_buffer * leverage;
    if !max_single_weight.is_finite()
        || max_single_weight <= 0.0
        || !gross_mult.is_finite()
        || gross_mult <= 1e-12
    {
        return Vec::new();
    }
    let max_w = max_single_weight / gross_mult;
    let mut clipped: Vec<String> = Vec::new();

    for is_long in [true, false] {
        let sign = if is_long { 1.0 } else { -1.0 };
        let mut rem: Vec<usize> = weights
            .iter()
            .enumerate()
            .filter(|(_, (_, w))| if is_long { *w > 1e-12 } else { *w < -1e-12 })
            .map(|(i, _)| i)
            .collect();
        if rem.is_empty() {
            continue;
        }
        let mut rem_target: f64 = rem.iter().map(|&i| weights[i].1.abs()).sum();
        loop {
            let rem_sum: f64 = rem.iter().map(|&i| weights[i].1.abs()).sum();
            if rem.is_empty() || rem_sum <= 1e-12 || rem_target <= 1e-12 {
                break;
            }
            let scale = rem_target / rem_sum;
            let over: Vec<usize> = rem
                .iter()
                .copied()
                .filter(|&i| weights[i].1.abs() * scale > max_w + 1e-12)
                .collect();
            if over.is_empty() {
                for &i in &rem {
                    weights[i].1 = sign * (weights[i].1.abs() * scale);
                }
                break;
            }
            for &i in &over {
                weights[i].1 = sign * max_w;
                if !clipped.contains(&weights[i].0) {
                    clipped.push(weights[i].0.clone());
                }
                rem_target = (rem_target - max_w).max(0.0);
            }
            rem.retain(|i| !over.contains(i));
        }
    }

    // 若某条腿因币数偏少全部打满上限，导致多空总权重不再相等，
    // 将较重的一侧等比例缩放到与较轻一侧相等，严格保持目标组合美元中性（sum(w) ≈ 0）。
    let long_sum: f64 = weights.iter().filter(|(_, w)| *w > 1e-12).map(|(_, w)| *w).sum();
    let short_sum: f64 = weights
        .iter()
        .filter(|(_, w)| *w < -1e-12)
        .map(|(_, w)| w.abs())
        .sum();
    if long_sum > 1e-12 && short_sum > 1e-12 && (long_sum - short_sum).abs() > 1e-12 {
        let target_leg = long_sum.min(short_sum);
        if long_sum > short_sum {
            let scale = target_leg / long_sum;
            for (_, w) in weights.iter_mut() {
                if *w > 1e-12 {
                    *w *= scale;
                }
            }
        } else {
            let scale = target_leg / short_sum;
            for (_, w) in weights.iter_mut() {
                if *w < -1e-12 {
                    *w *= scale;
                }
            }
        }
    }
    clipped
}

/// 若权重组合的目标毛杠杆超过 `hard_gross_cap`（占账户净值倍数，如 2.8x），
/// 等比例缩小所有权重并返回 `(raw_gross_lev, scale)`，确保目标组合永远不超过硬杠杆上限。
pub fn apply_hard_gross_cap(
    weights: &mut [(String, f64)],
    margin_buffer: f64,
    leverage: f64,
    hard_gross_cap: f64,
) -> Option<(f64, f64)> {
    let gross_mult = margin_buffer * leverage;
    if !hard_gross_cap.is_finite()
        || hard_gross_cap <= 0.0
        || !gross_mult.is_finite()
        || gross_mult <= 1e-12
    {
        return None;
    }
    let raw_gross_lev = weights.iter().map(|(_, w)| w.abs()).sum::<f64>() * gross_mult;
    if raw_gross_lev > hard_gross_cap + 1e-9 {
        let scale = hard_gross_cap / raw_gross_lev;
        for (_, w) in weights.iter_mut() {
            *w *= scale;
        }
        Some((raw_gross_lev, scale))
    } else {
        None
    }
}

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
    let target_gross = deployable * cfg.leverage;
    let hard_cap = if cfg.hard_gross_cap.is_finite() && cfg.hard_gross_cap > 0.0 && equity > 0.0 {
        equity * cfg.hard_gross_cap
    } else {
        f64::INFINITY
    };

    let mut capped_weights = weights.to_vec();
    let hard_cap_note = apply_hard_gross_cap(
        &mut capped_weights,
        cfg.margin_buffer,
        cfg.leverage,
        cfg.hard_gross_cap,
    )
    .map(|(raw_lev, scale)| {
        format!(
            "目标总敞口 {:.2}x 触发硬上限 {:.2}x，等比例缩放 ×{:.3}",
            raw_lev, cfg.hard_gross_cap, scale
        )
    });
    if cfg.max_single_weight > 0.0 {
        cap_weights_same_side(
            &mut capped_weights,
            cfg.max_single_weight,
            cfg.margin_buffer,
            cfg.leverage,
        );
    }
    let weights = capped_weights.as_slice();
    // 权重已归一化到 Σ|w| = 1（或已乘入 vol_k），直接按权重缩放名义即可。
    let per_coin = if weights.is_empty() {
        0.0
    } else {
        target_gross / weights.len() as f64
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
    if let Some(note) = hard_cap_note {
        plan.notes.push(note);
    }

    // 总敞口保护：把目标毛敞口按 Σ|w|（已包含 vol_k）和硬杠杆上限共同折算，
    // 超过 desired_gross 2% 就进入「强制减仓」模式：减仓不受错峰、EMA 和调仓带限制。
    let weights_gross: f64 = weights.iter().map(|(_, w)| w.abs()).sum();
    let desired_gross = (target_gross * weights_gross).min(hard_cap);
    let cur_gross: f64 = acct
        .positions
        .iter()
        .filter_map(|(c, p)| mids.get(c).map(|mid| p.size.abs() * mid))
        .sum();
    let over_cap = desired_gross > 0.0 && cur_gross > desired_gross * 1.02;
    if over_cap && equity > 0.0 {
        plan.notes.push(format!(
            "总敞口 {:.2}x 超过目标 {:.2}x，本次强制减仓（不受错峰限制）",
            cur_gross / equity,
            desired_gross / equity
        ));
    }

    // 错峰调仓与平滑 EMA 调仓：
    // - slices > 1 时按币名哈希分档，每天只调属于本档的币；
    // - slices == 1 且 ema_alpha < 1.0 时按 w_exec = w_cur + ema_alpha * (w_target - w_cur) 平滑收敛。
    let slices = cfg.rebalance_slices.max(1);
    let ema_alpha = if cfg.ema_alpha.is_finite() {
        cfg.ema_alpha.clamp(0.1, 1.0)
    } else {
        1.0
    };
    let ema_active = slices == 1 && ema_alpha < 1.0 - 1e-9;
    let rebal_band = if cfg.rebal_band.is_finite() {
        cfg.rebal_band.clamp(0.0, 0.50)
    } else {
        0.0
    };
    let day_no = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
        / 86_400_000) as u32;
    let in_slot = |coin: &str| slices <= 1 || (day_no + slice_of(coin, slices)) % slices == 0;

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
        if size * mid >= cfg.effective_min_order_usd(acct.equity) {
            wanted.insert(coin.clone(), (if *w > 0.0 { size } else { -size }, *w > 0.0));
        } else {
            plan.notes.push(format!(
                "{coin}: 目标 ${:.0} 低于最小下单额，跳过",
                size * mid
            ));
        }
    }

    // 1) close positions that are gone, stopped, took profit, or must flip side
    let mut closes: Vec<Order> = Vec::new();
    let mut opens: Vec<Order> = Vec::new();
    let mut blocked_reopen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (coin, pos) in acct.positions.iter() {
        let cur = pos.size;
        if cur.abs() < 1e-12 {
            continue;
        }
        let in_today = in_slot(coin);
        let Some(mid) = mids.get(coin) else {
            plan.notes
                .push(format!("{coin}: 缺价格，无法处理现有仓位 {cur:.6}"));
            continue;
        };
        let Some(m) = markets.get(coin) else {
            plan.notes.push(format!("{coin}: 缺市场元数据"));
            continue;
        };
        let stop_hit = cfg.position_stop_pct > 0.0
            && pos.entry_px > 0.0
            && pos.entry_px.is_finite()
            && {
                let ret = mid / pos.entry_px - 1.0;
                let r = if cur > 0.0 { ret } else { -ret };
                r < -cfg.position_stop_pct
            };
        let tp_hit = !stop_hit
            && cfg.take_profit_pct > 0.0
            && pos.entry_px > 0.0
            && pos.entry_px.is_finite()
            && {
                let ret = mid / pos.entry_px - 1.0;
                let r = if cur > 0.0 { ret } else { -ret };
                r >= cfg.take_profit_pct
            };
        let raw_target = if stop_hit || tp_hit {
            blocked_reopen.insert(coin.clone());
            0.0 // 止损/止盈：本次直接平掉且当次禁止在开仓循环里反手重开
        } else {
            wanted.get(coin).map(|w| w.0).unwrap_or(0.0)
        };
        let single_over_cap = cfg.max_single_weight > 0.0
            && equity > 0.0
            && cur.abs() * mid > equity * cfg.max_single_weight * 1.02;
        let force_reduce_raw = (over_cap || single_over_cap)
            && (raw_target == 0.0
                || cur * raw_target < 0.0
                || (cur * raw_target > 0.0 && raw_target.abs() < cur.abs()));

        // 平滑 EMA 目标：止损/止盈/强制减仓直接走 raw_target；否则按 cur + alpha*(raw_target - cur) 收敛。
        // 掉出名单（raw_target == 0）若剩余名义过小（残尘）则直接全平，不留不可平残仓。
        let target = if stop_hit || tp_hit || !ema_active || force_reduce_raw {
            raw_target
        } else {
            let blended = cur + ema_alpha * (raw_target - cur);
            if raw_target == 0.0 {
                let dust_floor = cfg
                    .effective_min_order_usd(acct.equity)
                    .max(cfg.min_order_usd * 2.0);
                if blended.abs() * mid < dust_floor {
                    0.0
                } else {
                    blended
                }
            } else if cur * raw_target < 0.0 {
                if blended.abs() * mid < cfg.min_order_usd {
                    0.0
                } else {
                    blended
                }
            } else {
                blended
            }
        };

        let flip = cur * target < 0.0;
        if target == 0.0 || flip {
            // 单币止损必须无视错峰档位立即平仓，避免仓位被冻结在 !in_today 档位
            if !in_today && !stop_hit {
                // 不在今天档位：若单币本身已超净值上限，先越过错峰把它减回上限，避免高集中度拖延多日。
                if single_over_cap {
                    let cap_sz = round_size(equity * cfg.max_single_weight / mid, m.sz_decimals);
                    let cap_target = if cur > 0.0 { cap_sz } else { -cap_sz };
                    let trim = round_size((cur.abs() - cap_sz).max(0.0), m.sz_decimals);
                    let dnotional = trim * mid;
                    if dnotional >= cfg.min_order_usd {
                        opens.push(Order {
                            coin: coin.clone(),
                            buy: cur < 0.0,
                            reduce_only: true,
                            target: cap_target,
                            size: trim,
                            mid: *mid,
                            sz_decimals: m.sz_decimals,
                            notional: dnotional,
                            reason: "单币超限减仓".into(),
                        });
                    }
                }
                continue; // 完整平仓仍按档位错峰
            }
            let size = round_size(cur.abs(), m.sz_decimals);
            // **平仓只用绝对下限，不用按净值比例算的门槛。**
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
                    reason: if stop_hit {
                        "单币止损平仓".into()
                    } else if flip {
                        "反手平仓".into()
                    } else if tp_hit {
                        "止盈平仓".into()
                    } else {
                        "掉出名单平仓".into()
                    },
                });
            } else {
                plan.notes
                    .push(format!("{coin}: 残余仓位 ${:.1} 低于最小下单额，无法平", size * mid));
            }
        } else {
            let delta = target - cur;
            let reducing = cur * delta < 0.0;
            let force_reduce = (over_cap || single_over_cap) && reducing;
            let closing_decay = ema_active && raw_target == 0.0 && reducing;
            if !in_today && !force_reduce {
                continue; // 不在今天档位：只有「超限 + 减仓」才破例
            }
            let dsize = round_size(delta.abs(), m.sz_decimals);
            let dnotional = dsize * mid;
            let pos_target = raw_target.abs() * mid;
            let threshold = if force_reduce || closing_decay {
                // 超限减仓或掉出名单的平滑减仓：只保留交易所绝对下限，不受调仓带限制。
                cfg.min_order_usd
            } else {
                cfg.effective_min_order_usd(acct.equity)
                    .max(pos_target * rebal_band)
            };
            if dnotional >= threshold {
                opens.push(Order {
                    coin: coin.clone(),
                    buy: delta > 0.0,
                    reduce_only: cur * delta < 0.0,
                    target,
                    size: dsize,
                    mid: *mid,
                    sz_decimals: m.sz_decimals,
                    notional: dnotional,
                    reason: if cur * delta > 0.0 {
                        "加仓到目标".into()
                    } else if single_over_cap {
                        "单币超限减仓".into()
                    } else if closing_decay {
                        "掉出名单平滑减仓".into()
                    } else {
                        "减仓到目标".into()
                    },
                });
            }
        }
    }

    // 止盈摘要：止盈单不是"计划订单"，它在执行之后由 place_take_profits 单独挂 ✗
    // 但不写出来用户会以为"没有止盈"✗ 所以这里给一行摘要 ✗
    if cfg.take_profit_pct > 0.0 && !acct.positions.is_empty() {
        let mut lines = 0usize;
        let mut closest: Option<(String, f64)> = None;
        for (coin, pos) in acct.positions.iter() {
            let Some(&mid) = mids.get(coin) else { continue };
            if pos.entry_px <= 0.0 || mid <= 0.0 {
                continue;
            }
            let long = pos.size > 0.0;
            let line = if long { pos.entry_px * (1.0 + cfg.take_profit_pct) } else { pos.entry_px * (1.0 - cfg.take_profit_pct) };
            let need = (line / mid - 1.0) * if long { 1.0 } else { -1.0 } * 100.0;
            if need > 0.0 {
                lines += 1;
                if closest.as_ref().map(|(_, d)| need < *d).unwrap_or(true) {
                    closest = Some((coin.clone(), need));
                }
            }
        }
        let mut msg = format!(
            "止盈：执行后为 {} 个仓位挂成本 ±{:.0}% 的限价单（= {:.0}% ROE @ {:.0}x）",
            lines,
            cfg.take_profit_pct * 100.0,
            cfg.take_profit_pct * cfg.leverage * 100.0,
            cfg.leverage
        );
        if let Some((coin, need)) = closest {
            msg.push_str(&format!(
                "；最近的是 {coin}，还差 {need:.1}% 价格（{:.1}% ROE）",
                need * cfg.leverage
            ));
        }
        plan.notes.push(msg);
    }

    // 2) open positions that are missing entirely (or flipped across zero)
    for (coin, (raw_target, is_long)) in wanted.iter() {
        if blocked_reopen.contains(coin) {
            continue; // 本次刚触发止损/止盈平仓，绝不在同一计划里立即反手重开
        }
        if over_cap {
            continue; // 先减仓回到上限以内，再考虑新仓
        }
        if !in_slot(coin) {
            continue; // 新仓位也等轮到它那一档再开
        }
        let cur = acct.positions.get(coin).map(|p| p.size).unwrap_or(0.0);
        let exec_target = if ema_active {
            cur + ema_alpha * (*raw_target - cur)
        } else {
            *raw_target
        };
        let flip = cur * exec_target < 0.0;
        // A flip is handled by the close above; reopen here once it is flat.
        if cur.abs() < 1e-12 || flip {
            let Some(mid) = mids.get(coin) else { continue };
            let Some(m) = markets.get(coin) else { continue };
            let size = round_size(exec_target.abs(), m.sz_decimals);
            if size * mid >= cfg.effective_min_order_usd(acct.equity) {
                let open_long = exec_target > 0.0 || (exec_target == 0.0 && *is_long);
                opens.push(Order {
                    coin: coin.clone(),
                    buy: open_long,
                    reduce_only: false,
                    target: if open_long { size } else { -size },
                    size,
                    mid: *mid,
                    sz_decimals: m.sz_decimals,
                    notional: size * mid,
                    reason: if open_long { "开多".into() } else { "开空".into() },
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
    pub aborted: bool,
}

/// Send the plan. `orders[i]` always corresponds to `plan.orders[i]`.
///
/// 顺序很重要：先平仓 → 重新读账户 → 给已经空仓的币设杠杆（Hyperliquid
/// 不允许持仓时切换保证金模式）→ 再开仓。这样反手的币也能切成全仓。
pub async fn execute(
    exec: &Exec, plan: &Plan, cfg: &TradeConfig,
    markets: &HashMap<String, MarketInfo>, live: bool,
) -> Result<Outcome> {
    execute_inner(exec, plan, cfg, markets, live).await
}


async fn execute_inner(
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
        return Ok(Outcome { prelim, orders, aborted: false });
    }

    // ---- 1) 先平仓 ----
    for (i, o) in plan.orders.iter().enumerate() {
        if !o.reduce_only {
            continue;
        }
        orders[i] = match exec
            .maker_then_ioc(&o.coin, o.buy, true, o.size, o.mid, cfg.slippage, o.sz_decimals, 12)
            .await
        {
            Ok(st) => format!(
                "{} {} 只减仓 {:.6} · {} · {}",
                if o.buy { "买" } else { "卖" },
                o.coin,
                o.size,
                st,
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
                    .maker_then_ioc(
                        &o.coin,
                        actual < 0.0,
                        true,
                        actual.abs(),
                        o.mid,
                        cfg.slippage,
                        o.sz_decimals,
                        12,
                    )
                    .await
                {
                    Ok(st) => format!(
                        "{} {} 只减仓平残余 {:.6} · {} · 平仓未完成，纠正方向",
                        if actual < 0.0 { "买" } else { "卖" },
                        o.coin,
                        actual.abs(),
                        st
                    ),
                    Err(e) => format!("{} 纠正残余仓位失败: {e}", o.coin),
                };
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                continue;
            }
            let delta = o.target - actual;
            let dsize = round_size(delta.abs(), o.sz_decimals);
            let dnotional = dsize * o.mid;
            if dnotional < cfg.effective_min_order_usd(plan.equity) {
                orders[i] = format!(
                    "{} 已到位（差 ${:.2}），跳过",
                    o.coin, dnotional
                );
                continue;
            }
            let reduce_only = actual * delta < 0.0;
            orders[i] = match exec
                .maker_then_ioc(
                    &o.coin,
                    delta > 0.0,
                    reduce_only,
                    dsize,
                    o.mid,
                    cfg.slippage,
                    o.sz_decimals,
                    12,
                )
                .await
            {
                Ok(st) => format!(
                    "{} {} {} {:.6} · {} · {}",
                    if delta > 0.0 { "买" } else { "卖" },
                    o.coin,
                    if reduce_only { "只减仓" } else { "开仓" },
                    dsize,
                    st,
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


    Ok(Outcome { prelim, orders, aborted: false })
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

    /// 因子权重必须只用一次线性组合 + 最终归一化，不能引入额外的缩放。
    ///
    /// 用三个点验证（不需要复刻因子数学）：
    ///   w(1.0) = 纯动量（此时 Σ|w|=1，所以它就是未归一化的动量向量）
    ///   w(0.0) = 纯量能（同理）
    ///   w(0.7) 必须等于 normalize(0.7·w(1.0) + 0.3·w(0.0))
    /// 这条关系一旦破坏（比如权重加在两个腿之外、或 50/50 的旧行为被改动），
    /// 界面上的「动量权重 70%」就不再等于实际组合的 70%。
    #[test]
    fn factor_weight_is_a_linear_mix_of_the_two_books() {
        let panel = synthetic();
        let base = TradeConfig { min_position_usd: 0.0, inv_vol: false, ..Default::default() };
        let weights = |mw: f64| {
            let cfg = TradeConfig { momentum_weight: mw, ..base.clone() };
            let (w, _) = target_weights(&panel, &cfg, 1000.0, i64::MAX);
            let m: std::collections::HashMap<String, f64> = w.into_iter().collect();
            let gross: f64 = m.values().map(|x| x.abs()).sum();
            assert!((gross - 1.0).abs() < 1e-9, "mw={mw} 的 Σ|w| 必须是 1，实际 {gross}");
            m
        };
        let mom = weights(1.0);
        let vol = weights(0.0);
        assert!(!mom.is_empty() && !vol.is_empty(), "两个纯因子组合都不该为空");
        for mw in [0.5_f64, 0.7, 0.3] {
            let got = weights(mw);
            let mut coins: Vec<&String> = mom.keys().chain(vol.keys()).collect();
            coins.sort();
            coins.dedup();
            let mut raw: Vec<(String, f64)> = Vec::new();
            for c in coins {
                let x = mw * mom.get(c).copied().unwrap_or(0.0)
                    + (1.0 - mw) * vol.get(c).copied().unwrap_or(0.0);
                raw.push((c.clone(), x));
            }
            let sum: f64 = raw.iter().map(|(_, x)| x.abs()).sum();
            assert!(sum > 0.0);
            for (c, x) in raw {
                let want = x / sum;
                let have = got.get(&c).copied().unwrap_or(0.0);
                assert!(
                    (want - have).abs() < 1e-9,
                    "mw={mw} 时 {c} 的权重 {have} 不等于线性组合 {want}"
                );
            }
        }
    }

    /// 止盈（选项 B）：相对【持仓成本】。已到成本的 (1+tp) 的仓位，
    /// 调仓时就该平掉（而不是等交易所那张可能挂不上去的限价单）。
    #[test]
    fn take_profit_closes_a_winner_at_cost() {
        let panel = synthetic();
        let cfg = TradeConfig {
            min_position_usd: 0.0,
            take_profit_pct: 0.10,
            rebalance_slices: 1,
            ..Default::default()
        };
        let (w, _) = target_weights(&panel, &cfg, 1000.0, i64::MAX);
        let coin = w.iter().find(|(_, x)| *x > 0.0).expect("要有多头").0.clone();
        let mid = panel.iter().find(|e| e.coin == coin).unwrap().candles.last().unwrap().c;
        let markets: HashMap<String, MarketInfo> =
            panel.iter().map(|e| (e.coin.clone(), MarketInfo { sz_decimals: 2, max_leverage: 5 })).collect();
        let mids: HashMap<String, f64> = panel.iter().map(|e| (e.coin.clone(), e.candles.last().unwrap().c)).collect();
        let mut acct = Acct { equity: 1000.0, positions: Default::default() };
        // 成本 = 现价/1.12 → 已赚 12% ≥ 10%
        acct.positions.insert(coin.clone(), crate::exchange::Pos {
            size: 10.0, entry_px: mid / 1.12, ..Default::default()
        });
        let plan = build_plan(&w, &acct, &markets, &mids, &cfg, None);
        let tp: Vec<_> = plan.orders.iter().filter(|o| o.reason == "止盈平仓").collect();
        assert!(!tp.is_empty(), "应有止盈平仓单: {:?}", plan.orders.iter().map(|o| (&o.coin, &o.reason)).collect::<Vec<_>>());
        assert!(tp[0].reduce_only && !tp[0].buy, "平多头必须 reduce_only 卖出");

        // 赚 5% < 10%: 不应止盈
        let mut acct2 = Acct { equity: 1000.0, positions: Default::default() };
        acct2.positions.insert(coin.clone(), crate::exchange::Pos {
            size: 10.0, entry_px: mid / 1.05, ..Default::default()
        });
        let plan2 = build_plan(&w, &acct2, &markets, &mids, &cfg, None);
        assert!(!plan2.orders.iter().any(|o| o.reason == "止盈平仓"), "未到阈值不该止盈");
    }

    /// 净敞口保护：只减偏多的那条腿，绝不开新仓。
    #[test]
    fn neutralize_reduces_only_the_overweight_side() {
        let markets: HashMap<String, MarketInfo> =
            ["A", "B"].iter().map(|c| (c.to_string(), MarketInfo { sz_decimals: 2, max_leverage: 5 })).collect();
        let mids: HashMap<String, f64> = ["A", "B"].iter().map(|c| (c.to_string(), 100.0)).collect();
        let mut acct = Acct { equity: 1000.0, positions: Default::default() };
        acct.positions.insert("A".into(), crate::exchange::Pos { size: 10.0, entry_px: 100.0, ..Default::default() });
        acct.positions.insert("B".into(), crate::exchange::Pos { size: -5.0, entry_px: 100.0, ..Default::default() });
        let orders = neutralize_orders(&acct, &markets, &mids, 0.02, 10.0);
        assert_eq!(orders.len(), 1, "只应有一笔");
        assert_eq!(orders[0].coin, "A");
        assert!(!orders[0].buy, "减多头必须是卖出");
        assert!(orders[0].reduce_only);
        assert!((orders[0].notional - 500.0).abs() < 1e-6, "应减掉 500 名义，实际 {}", orders[0].notional);

        // 平衡 → 不动
        let mut bal = Acct { equity: 1000.0, positions: Default::default() };
        bal.positions.insert("A".into(), crate::exchange::Pos { size: 10.0, entry_px: 100.0, ..Default::default() });
        bal.positions.insert("B".into(), crate::exchange::Pos { size: -10.0, entry_px: 100.0, ..Default::default() });
        assert!(neutralize_orders(&bal, &markets, &mids, 0.02, 10.0).is_empty());

        // 净空 → 买回空头
        let mut sh = Acct { equity: 1000.0, positions: Default::default() };
        sh.positions.insert("A".into(), crate::exchange::Pos { size: 2.0, entry_px: 100.0, ..Default::default() });
        sh.positions.insert("B".into(), crate::exchange::Pos { size: -10.0, entry_px: 100.0, ..Default::default() });
        let o = neutralize_orders(&sh, &markets, &mids, 0.02, 10.0);
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].coin, "B");
        assert!(o[0].buy, "减空头必须是买入");
    }

    #[test]
    fn neutralize_is_deterministic_and_respects_targets() {
        let markets: HashMap<String, MarketInfo> = ["A", "B", "C"]
            .iter()
            .map(|c| (c.to_string(), MarketInfo { sz_decimals: 2, max_leverage: 5 }))
            .collect();
        let mids: HashMap<String, f64> = ["A", "B", "C"]
            .iter()
            .map(|c| (c.to_string(), 100.0))
            .collect();
        let mut acct = Acct { equity: 3000.0, positions: Default::default() };
        acct.positions.insert("A".into(), crate::exchange::Pos { size: 10.0, entry_px: 100.0, ..Default::default() });
        acct.positions.insert("B".into(), crate::exchange::Pos { size: 10.0, entry_px: 100.0, ..Default::default() });
        acct.positions.insert("C".into(), crate::exchange::Pos { size: -15.0, entry_px: 100.0, ..Default::default() });
        let targets = HashMap::from([
            ("A".to_string(), 500.0),
            ("B".to_string(), 1000.0),
            ("C".to_string(), -1500.0),
        ]);
        let orders = neutralize_orders_with_targets(&acct, &markets, &mids, 0.02, 10.0, Some(&targets));
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].coin, "A", "应优先削减超出目标的 A");
        assert!((orders[0].notional - 500.0).abs() < 1e-9);
        assert!((orders[0].target - 5.0).abs() < 1e-9, "A 应保留到目标 500");

        // 没有目标时，净多头 500 由两个等额仓位各承担 250，顺序固定且不随机清零。
        let fallback = neutralize_orders(&acct, &markets, &mids, 0.02, 10.0);
        assert_eq!(fallback.iter().map(|o| o.coin.as_str()).collect::<Vec<_>>(), vec!["A", "B"]);
        assert!(fallback.iter().all(|o| (o.notional - 250.0).abs() <= 1.0));
        assert!(fallback.iter().all(|o| o.target.abs() > 0.0));
    }

    #[test]
    fn neutralize_large_imbalance_spreads_instead_of_zeroing_winners() {
        let markets: HashMap<String, MarketInfo> = ["VVV", "HYPE", "BTC"]
            .iter()
            .map(|c| (c.to_string(), MarketInfo { sz_decimals: 2, max_leverage: 5 }))
            .collect();
        let mids: HashMap<String, f64> = ["VVV", "HYPE", "BTC"]
            .iter()
            .map(|c| (c.to_string(), 100.0))
            .collect();
        let mut acct = Acct { equity: 5000.0, positions: Default::default() };
        acct.positions.insert("VVV".into(), crate::exchange::Pos { size: 10.0, entry_px: 80.0, ..Default::default() });
        acct.positions.insert("HYPE".into(), crate::exchange::Pos { size: 10.0, entry_px: 80.0, ..Default::default() });
        acct.positions.insert("BTC".into(), crate::exchange::Pos { size: -5.0, entry_px: 100.0, ..Default::default() });
        let targets = HashMap::from([
            ("VVV".to_string(), 900.0),
            ("HYPE".to_string(), 900.0),
            ("BTC".to_string(), -500.0),
        ]);
        let orders = neutralize_orders_with_targets(&acct, &markets, &mids, 0.02, 10.0, Some(&targets));
        assert_eq!(orders.len(), 2);
        assert!(orders.iter().all(|o| o.target.abs() > 0.0), "大额不平衡也应分摊，不能清零 VVV/HYPE");
        let reduced: f64 = orders.iter().map(|o| o.notional).sum();
        assert!((reduced - 1500.0).abs() <= 2.0, "仅允许数量向下取整的误差");
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
    fn inv_vol_weights_keep_market_neutrality_and_unit_gross() {
        let panel = synthetic();
        let cfg = TradeConfig {
            min_position_usd: 0.0,
            inv_vol: true,
            ..Default::default()
        };
        let (w, _) = target_weights(&panel, &cfg, 1000.0, i64::MAX);
        assert!(!w.is_empty());
        let gross: f64 = w.iter().map(|(_, x)| x.abs()).sum();
        let net: f64 = w.iter().map(|(_, x)| *x).sum();
        assert!((gross - 1.0).abs() < 1e-9, "inv_vol Σ|w| 必须是 1，实际 {gross}");
        assert!(net.abs() < 1e-9, "inv_vol 净敞口必须是 0，实际 {net}");
    }

    #[test]
    fn only_closed_bars_are_used() {
        // 最后一根如果是「正在形成」的当根，量能因子会拿到几分钟的成交量，
        // 实盘会选出与回测完全不同的组合。用 now 卡在最后一根中间来验证。
        const DAY: i64 = 86_400_000;
        let panel = synthetic();
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

#[cfg(test)]
mod slice_tests {
    use super::*;

    #[test]
    fn slice_of_is_deterministic_and_spreads_coins() {
        // 必须跨进程稳定：不能用 HashMap 迭代顺序或随机 hash
        assert_eq!(slice_of("BTC", 3), slice_of("BTC", 3));
        assert_eq!(slice_of("BTC", 1), 0);
        // 分档要真的把币摊开，不能全挤在一档
        let coins = ["BTC", "ETH", "SOL", "DOGE", "ARB", "HYPE", "WLD", "ADA", "XRP"];
        let mut seen = std::collections::HashSet::new();
        for c in coins {
            seen.insert(slice_of(c, 3));
        }
        assert!(seen.len() >= 2, "档位分布太集中: {seen:?}");
        for c in coins {
            assert!(slice_of(c, 3) < 3);
        }
    }
}

#[cfg(test)]
mod slice_plan_tests {
    use super::*;

    fn mk(coin: &str, sz: f64) -> (String, crate::exchange::Pos) {
        (
            coin.to_string(),
            crate::exchange::Pos {
                size: sz,
                entry_px: 1.0,
                liq_px: None,
                is_cross: true,
                leverage: 3,
                position_value: 100.0,
                unrealized_pnl: 0.0,
            },
        )
    }

    fn market(coin: &str) -> (String, MarketInfo) {
        (
            coin.to_string(),
            MarketInfo { sz_decimals: 2, max_leverage: 25 },
        )
    }

    /// 超限时（净值下跌、敞口被动爬到 3x+）必须能【越过错峰】立刻减仓，
    /// 否则 3 档错峰要拖 3 天，暴跌中敞口一直偏高、保证金缓冲被吃光。
    #[test]
    fn over_cap_trims_even_non_slot_positions() {
        let cfg = TradeConfig { rebalance_slices: 3, ..TradeConfig::default() };
        let coins = ["BTC", "ETH", "SOL", "DOGE", "ARB", "HYPE"];
        // 每个 10×100 = 1000，合计 6000 = 6x（远超 2.7x 上限）。
        let weights: Vec<(String, f64)> = coins
            .iter()
            .map(|c| (c.to_string(), if c == &"ETH" || c == &"DOGE" { -0.1 } else { 0.1 }))
            .collect();
        let acct = Acct {
            equity: 1000.0,
            positions: coins.iter().map(|c| mk(c, 10.0)).collect(),
        };
        let markets: HashMap<String, MarketInfo> = coins.iter().map(|c| market(c)).collect();
        let mids: HashMap<String, f64> = coins.iter().map(|c| (c.to_string(), 100.0)).collect();
        let plan = build_plan(&weights, &acct, &markets, &mids, &cfg, None);
        assert!(
            plan.notes.iter().any(|n| n.contains("强制减仓")),
            "超限时应有提示: {:?}",
            plan.notes
        );
        assert!(
            plan.orders.iter().any(|o| o.reduce_only),
            "超限时应有减仓单"
        );
        // 减仓单必须覆盖到「不在今天档位」的币。
        let day = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            / 86_400_000) as u32;
        let non_slot_with_order = plan
            .orders
            .iter()
            .any(|o| (day + slice_of(&o.coin, 3)) % 3 != 0 && o.reduce_only);
        assert!(non_slot_with_order, "超限减仓应越过错峰档位，实际订单: {:?}", plan.orders.iter().map(|o| (&o.coin, o.reduce_only)).collect::<Vec<_>>());
    }

    /// 错峰最关键的性质：不在今天档位里的币，绝不能因为它「不在目标权重里」
    /// 就被平掉。否则那不是错峰，是随机清仓。
    #[test]
    fn non_slot_positions_are_left_alone() {
        let cfg = TradeConfig { rebalance_slices: 3, ..TradeConfig::default() };
        // 造 6 个持仓，但目标权重里只有 3 个 —— 其余 3 个「本该被平」
        // 用真实币名。之前用 AAA/BBB/CCC —— 同字母重复三遍的字节和都是 3 的
        // 倍数，会全挤在同一档，测试因此"今天一个币都没有"而假失败。
        let held = ["BTC", "ETH", "SOL", "DOGE", "ARB", "HYPE"];
        // Σ|w| = 1.0 → 目标总敞口 2700（2.7x），当前名义 6×300 = 1800 < 2700，不触发超限强制减仓。
        let weights: Vec<(String, f64)> = vec![
            ("BTC".into(), 0.35),
            ("ETH".into(), -0.50),
            ("SOL".into(), 0.15),
        ];
        let acct = Acct {
            equity: 1000.0,
            positions: held.iter().map(|c| mk(c, 3.0)).collect(),
        };
        let markets: HashMap<String, MarketInfo> = held.iter().map(|c| market(c)).collect();
        let mids: HashMap<String, f64> = held.iter().map(|c| (c.to_string(), 100.0)).collect();
        let plan = build_plan(&weights, &acct, &markets, &mids, &cfg, None);

        let all_coins: Vec<&str> = plan
            .orders
            .iter()
            .map(|o| o.coin.as_str())
            .chain(plan.orders.iter().map(|o| o.coin.as_str()))
            .collect();
        let _ = all_coins;
        // 今天该处理的币
        let day = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            / 86_400_000) as u32;
        let today: Vec<&&str> = held
            .iter()
            .filter(|c| (day + slice_of(c, 3)) % 3 == 0)
            .collect();
        let not_today: Vec<&&str> = held
            .iter()
            .filter(|c| (day + slice_of(c, 3)) % 3 != 0)
            .collect();

        // 不在今天档位的币：一个订单都不能有
        for c in &not_today {
            let touched = plan.orders.iter().any(|o| o.coin == ***c);
            assert!(!touched, "{c} 不在今天的档位，却被下了单（会被误平）");
        }
        // 今天档位里的币：应该被处理到（DDD/EEE/FFF 不在目标里 → 该被平）
        assert!(!today.is_empty(), "今天档位里一个币都没有，测试无意义");
        assert!(!not_today.is_empty(), "所有币都在今天档位，测不出过滤是否生效");
    }

}

#[cfg(test)]
mod close_threshold_tests {
    use super::*;

    fn market(coin: &str) -> (String, MarketInfo) {
        (
            coin.to_string(),
            MarketInfo { sz_decimals: 2, max_leverage: 25 },
        )
    }

    /// **真调用 build_plan 的端到端回归**：$3,104 净值 + min_order_pct 1.7%
    /// （比例门槛 $52.77）+ 一个约 $45 的 GRAM 残仓 + 它不在目标名单里
    /// → 必须生成平仓单。这就是用户实际遇到的场景，不是算术推演。
    #[test]
    fn small_out_of_target_position_with_pct_threshold_is_still_closed() {
        let cfg = TradeConfig {
            min_order_pct: 0.017,
            min_order_usd: 10.0,
            rebalance_slices: 1,
            ..TradeConfig::default()
        };
        let mut positions = std::collections::HashMap::new();
        positions.insert(
            "GRAM".to_string(),
            crate::exchange::Pos {
                size: 31.0, // 31 × 1.4465 ≈ $44.84
                entry_px: 1.4,
                liq_px: None,
                is_cross: true,
                leverage: 3,
                position_value: 44.84,
                unrealized_pnl: 0.0,
            },
        );
        let acct = Acct { equity: 3104.0, positions };
        let markets: std::collections::HashMap<String, MarketInfo> =
            ["GRAM", "BTC", "ETH"].iter().map(|c| market(c)).collect();
        let mids: std::collections::HashMap<String, f64> = vec![
            ("GRAM".to_string(), 1.4465),
            ("BTC".to_string(), 80000.0),
            ("ETH".to_string(), 3000.0),
        ]
        .into_iter()
        .collect();
        // 目标里只有 BTC/ETH —— GRAM 掉出名单
        let weights: Vec<(String, f64)> =
            vec![("BTC".into(), 0.5), ("ETH".into(), -0.5)];

        // 前提：$44.84 确实低于比例门槛 $52.77
        assert!(44.84 < cfg.effective_min_order_usd(3104.0));

        let plan = build_plan(&weights, &acct, &markets, &mids, &cfg, None);
        let closed = plan.orders.iter().any(|o| o.coin == "GRAM" && o.reduce_only);
        let seen: Vec<(String, String)> =
            plan.orders.iter().map(|o| (o.coin.clone(), o.reason.clone())).collect();
        assert!(
            closed,
            "GRAM 残仓（$44.84 < 比例门槛）必须仍然被平掉，实际订单: {seen:?}"
        );
        assert!(plan.notes.iter().all(|n| !n.contains("GRAM: 残余仓位")),
            "不应再出现「残余仓位…无法平」: {:?}", plan.notes);
    }

    /// 回归：设了 `min_order_pct` 之后，掉出名单的【小仓位】必须仍然能平掉。
    ///
    /// 真实事故：$3,104 净值上设 1.7% → 比例门槛 $52.76，
    /// 而账上有 4 个 $45~47 的残仓，被这个门槛挡住平不掉，永远挂着。
    /// 平仓是降风险，只应受【绝对下限】（交易所最小下单额）约束。
    #[test]
    fn small_position_below_pct_threshold_must_still_close() {
        let mut cfg = TradeConfig::default();
        cfg.min_order_usd = 10.0;
        cfg.min_order_pct = 0.017;
        let equity = 3104.0;
        // 比例门槛确实是 52.76
        assert!((cfg.effective_min_order_usd(equity) - 52.768).abs() < 0.01);
        // 一个 $46 的残仓：低于比例门槛，但高于绝对下限 → 必须可平
        let pos_notional = 46.0;
        assert!(pos_notional < cfg.effective_min_order_usd(equity), "前提：它低于比例门槛");
        assert!(pos_notional >= cfg.min_order_usd, "且高于绝对下限 → 平仓判定应通过");
    }

    #[test]
    fn cap_weights_same_side_waterfills_excess_and_is_idempotent() {
        // margin_buffer * leverage = 0.9 * 3.0 = 2.7; max_single_weight = 0.20 => max_w = 0.20 / 2.7
        // 多头 5 币总权重 0.30（=0.81x < 5*0.20=1.00x），空头 5 币总权重 -0.30（美元中性）
        let mut w = vec![
            ("A".to_string(), 0.14),
            ("B".to_string(), 0.06),
            ("C".to_string(), 0.04),
            ("D".to_string(), 0.04),
            ("E".to_string(), 0.02),
            ("S1".to_string(), -0.06),
            ("S2".to_string(), -0.06),
            ("S3".to_string(), -0.06),
            ("S4".to_string(), -0.06),
            ("S5".to_string(), -0.06),
        ];
        let long_before: f64 = w.iter().filter(|(_, v)| *v > 0.0).map(|(_, v)| *v).sum();
        let short_before: f64 = w.iter().filter(|(_, v)| *v < 0.0).map(|(_, v)| v.abs()).sum();
        let clipped = cap_weights_same_side(&mut w, 0.20, 0.9, 3.0);
        assert!(clipped.contains(&"A".to_string()), "A 应被截断: {clipped:?}");
        let max_w = 0.20 / 2.7;
        for (c, v) in &w {
            assert!(v.abs() <= max_w + 1e-9, "{c} 截断后仍超限: {}", v.abs() * 2.7);
        }
        let long_after: f64 = w.iter().filter(|(_, v)| *v > 0.0).map(|(_, v)| *v).sum();
        let short_after: f64 = w.iter().filter(|(_, v)| *v < 0.0).map(|(_, v)| v.abs()).sum();
        assert!((long_after - long_before).abs() < 1e-9, "多头总权重应保持不变: {long_after} vs {long_before}");
        assert!((short_after - short_before).abs() < 1e-9, "空头总权重不应受影响");
        assert!((long_after - short_after).abs() < 1e-9, "多空必须保持中性");
        // 幂等性
        let snapshot = w.clone();
        let clipped2 = cap_weights_same_side(&mut w, 0.20, 0.9, 3.0);
        assert!(clipped2.is_empty(), "二次调用不应再截断: {clipped2:?}");
        for ((c1, v1), (c2, v2)) in w.iter().zip(snapshot.iter()) {
            assert_eq!(c1, c2);
            assert!((v1 - v2).abs() < 1e-12, "{c1} 幂等性偏差: {v1} vs {v2}");
        }
    }

    #[test]
    fn cap_weights_same_side_mirror_scales_heavier_side_to_stay_neutral() {
        // 多头只有 4 币（4 * 20% = 80% 净值上限），空头有 7 币（7 * 20% = 140% 可容纳 135%）。
        // 截断后多头打满上限 = 4 * max_w，空头必须按比例缩到与多头相等，严格保持 sum(w) ≈ 0。
        let mut w = vec![
            ("L1".to_string(), 0.15),
            ("L2".to_string(), 0.15),
            ("L3".to_string(), 0.10),
            ("L4".to_string(), 0.10),
            ("S1".to_string(), -0.08),
            ("S2".to_string(), -0.07),
            ("S3".to_string(), -0.07),
            ("S4".to_string(), -0.07),
            ("S5".to_string(), -0.07),
            ("S6".to_string(), -0.07),
            ("S7".to_string(), -0.07),
        ];
        let _clipped = cap_weights_same_side(&mut w, 0.20, 0.9, 3.0);
        let max_w = 0.20 / 2.7;
        for (c, v) in &w {
            assert!(v.abs() <= max_w + 1e-12, "{c} 超过单币上限: {v}");
        }
        let net: f64 = w.iter().map(|(_, v)| *v).sum();
        assert!(net.abs() < 1e-12, "多空打满不对称时必须镜像缩放保持美元中性，实际 net={net}");
        let long_sum: f64 = w.iter().filter(|(_, v)| *v > 0.0).map(|(_, v)| *v).sum();
        assert!((long_sum - 4.0 * max_w).abs() < 1e-12);
        // 幂等性
        let snap = w.clone();
        assert!(cap_weights_same_side(&mut w, 0.20, 0.9, 3.0).is_empty());
        assert_eq!(w, snap);
    }

    #[test]
    fn post_vol_target_cap_uses_actual_order_weight_space() {
        // 这些权重已经乘入 k=0.52。订单名义 = weight * 0.9 * 3 * equity，
        // 不能再乘一次 k；0.074 对应约 20%，0.077 对应约 20.8%。
        let k = 0.52;
        let mut w = vec![
            ("VVV".to_string(), 0.30 * k),
            ("XPL".to_string(), 0.20 * k),
            ("XMR".to_string(), 0.08 * k),
            ("NEAR".to_string(), 0.04 * k),
            ("ZRO".to_string(), 0.03 * k),
            ("BTC".to_string(), -0.15 * k),
            ("ETH".to_string(), -0.15 * k),
            ("SOL".to_string(), -0.15 * k),
            ("ARB".to_string(), -0.10 * k),
            ("DOGE".to_string(), -0.10 * k),
        ];
        let long_before: f64 = w.iter().filter(|(_, v)| *v > 0.0).map(|(_, v)| *v).sum();
        let clipped = cap_weights_same_side(&mut w, 0.20, 0.9, 3.0);
        assert!(clipped.contains(&"VVV".to_string()));
        assert!(clipped.contains(&"XPL".to_string()));
        let long_after: f64 = w.iter().filter(|(_, v)| *v > 0.0).map(|(_, v)| *v).sum();
        assert!((long_after - long_before).abs() < 1e-12, "同侧总杠杆必须不变");
        for (coin, weight) in &w {
            let equity_frac = weight.abs() * 0.9 * 3.0;
            assert!(equity_frac <= 0.20 + 1e-12, "{coin} 最终目标仍超限: {equity_frac}");
        }
        let vvv = w.iter().find(|(c, _)| c == "VVV").unwrap().1;
        assert!((vvv * 0.9 * 3.0 - 0.20).abs() < 1e-12, "VVV 应恰好封顶 20%");
        let snapshot = w.clone();
        assert!(cap_weights_same_side(&mut w, 0.20, 0.9, 3.0).is_empty(), "必须幂等");
        assert_eq!(w, snapshot);
    }

    #[test]
    fn over_cap_respects_vol_k_and_hard_gross_cap() {
        let coins = ["BTC", "ETH", "SOL", "DOGE"];
        let markets: HashMap<String, MarketInfo> = coins
            .iter()
            .map(|c| (c.to_string(), MarketInfo { sz_decimals: 2, max_leverage: 10 }))
            .collect();
        let mids: HashMap<String, f64> = coins.iter().map(|c| (c.to_string(), 100.0)).collect();

        // 1) vol_k = 0.50 → desired_gross = 0.50 * 2.7x = 1.35x; cur_gross = 2.0x > 1.35x * 1.02 → 必须触发 over_cap
        let cfg_low_vol = TradeConfig {
            rebalance_slices: 3,
            ema_alpha: 1.0,
            hard_gross_cap: 2.8,
            ..TradeConfig::default()
        };
        let w_half = vec![
            ("BTC".into(), 0.125),
            ("ETH".into(), 0.125),
            ("SOL".into(), -0.125),
            ("DOGE".into(), -0.125),
        ];
        let mut pos = HashMap::new();
        for c in &coins {
            pos.insert(
                c.to_string(),
                crate::exchange::Pos { size: if *c == "BTC" || *c == "ETH" { 5.0 } else { -5.0 }, entry_px: 100.0, ..Default::default() },
            );
        }
        let acct = Acct { equity: 1000.0, positions: pos };
        let plan_down = build_plan(&w_half, &acct, &markets, &mids, &cfg_low_vol, None);
        assert!(
            plan_down.notes.iter().any(|n| n.contains("强制减仓")),
            "vol_k=0.5 且当前 2.0x 应触发强制减仓: {:?}",
            plan_down.notes
        );

        // 2) vol_k = 1.50（原始 4.05x）必须被 hard_gross_cap = 2.8x 压回 <= 2.8x，绝不飙到 3.96x
        let cfg_cap = TradeConfig {
            rebalance_slices: 1,
            ema_alpha: 1.0,
            hard_gross_cap: 2.8,
            ..TradeConfig::default()
        };
        let w_big = vec![
            ("BTC".into(), 0.375),
            ("ETH".into(), 0.375),
            ("SOL".into(), -0.375),
            ("DOGE".into(), -0.375),
        ];
        let empty = Acct { equity: 10000.0, positions: HashMap::new() };
        let plan_cap = build_plan(&w_big, &empty, &markets, &mids, &cfg_cap, None);
        assert!(
            plan_cap.gross_notional <= 10000.0 * 2.8 + 1e-6,
            "目标总名义不得超过 hard_gross_cap=2.8x，实际 {}",
            plan_cap.gross_notional
        );
        let open_sum: f64 = plan_cap.orders.iter().map(|o| o.notional).sum();
        assert!(
            open_sum <= 10000.0 * 2.8 + 1e-6,
            "实际计划开仓总额不得超过 2.8x，实际 {open_sum}"
        );
    }

    #[test]
    fn ema_rebalance_blends_targets_and_skips_within_rebal_band() {
        let cfg = TradeConfig {
            rebalance_slices: 1,
            ema_alpha: 0.50,
            rebal_band: 0.05,
            min_order_usd: 10.0,
            min_order_pct: 0.005,
            hard_gross_cap: 2.8,
            ..TradeConfig::default()
        };
        let coins = ["BTC", "ETH", "SOL", "DOGE"];
        let markets: HashMap<String, MarketInfo> = coins
            .iter()
            .map(|c| (c.to_string(), MarketInfo { sz_decimals: 2, max_leverage: 10 }))
            .collect();
        let mids: HashMap<String, f64> = coins.iter().map(|c| (c.to_string(), 100.0)).collect();
        // equity = 10000, deployable*lev = 27000
        // BTC target = 0.25 -> $6750 (67.5), cur = 67.0 ($6700): diff $50, ema diff $25 < 5%*$6750=$337.5 -> 跳过
        // ETH target = 0.25 -> $6750 (67.5), cur = 0 -> EMA 开一半 = 33.75 ($3375)
        // SOL target = -0.25 -> -$6750 (-67.5), cur = -40.0 -> EMA blend = -53.75, delta = -13.75 ($1375 >= $337.5)
        // DOGE target = 0 (dropped), cur = -30.0 ($3000) -> EMA blend = -15.0 (平滑减仓 15.0)
        let mut pos = HashMap::new();
        pos.insert("BTC".into(), crate::exchange::Pos { size: 67.0, entry_px: 100.0, ..Default::default() });
        pos.insert("SOL".into(), crate::exchange::Pos { size: -40.0, entry_px: 100.0, ..Default::default() });
        pos.insert("DOGE".into(), crate::exchange::Pos { size: -30.0, entry_px: 100.0, ..Default::default() });
        let acct = Acct { equity: 10000.0, positions: pos };
        let weights = vec![
            ("BTC".into(), 0.25),
            ("ETH".into(), 0.25),
            ("SOL".into(), -0.25),
        ];
        let plan = build_plan(&weights, &acct, &markets, &mids, &cfg, None);
        assert!(plan.orders.iter().all(|o| o.coin != "BTC"), "BTC 在 5% rebal_band 内应跳过");
        let eth = plan.orders.iter().find(|o| o.coin == "ETH").expect("ETH 应开半仓");
        assert!((eth.size - 33.75).abs() < 0.05, "ETH EMA=0.5 应开 33.75，实际 {}", eth.size);
        let sol = plan.orders.iter().find(|o| o.coin == "SOL").expect("SOL 应平滑加空");
        assert!((sol.size - 13.75).abs() < 0.05, "SOL EMA=0.5 应加空 13.75，实际 {}", sol.size);
        let doge = plan.orders.iter().find(|o| o.coin == "DOGE").expect("DOGE 掉出名单应平滑减仓");
        assert!(doge.reduce_only && doge.buy);
        assert!((doge.size - 15.0).abs() < 0.05, "DOGE EMA=0.5 应先减半 15.0，实际 {}", doge.size);
    }

    #[test]
    fn position_stop_closes_even_when_not_in_today_slice() {
        let cfg = TradeConfig {
            rebalance_slices: 3,
            position_stop_pct: 0.20,
            ..TradeConfig::default()
        };
        let day = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            / 86_400_000) as u32;
        let candidates = ["BTC", "ETH", "SOL", "DOGE", "ARB", "HYPE"];
        let non_slot = candidates
            .iter()
            .copied()
            .find(|c| (day + slice_of(c, 3)) % 3 != 0)
            .expect("必须存在不在今日档位的币");
        let mut pos = HashMap::new();
        // 多头成本 100，现价 70 → 浮亏 -30% < -20%，即使不在今日档位也必须立刻全平
        pos.insert(
            non_slot.to_string(),
            crate::exchange::Pos { size: 5.0, entry_px: 100.0, ..Default::default() },
        );
        let acct = Acct { equity: 5000.0, positions: pos };
        let markets: HashMap<String, MarketInfo> = candidates
            .iter()
            .map(|c| (c.to_string(), MarketInfo { sz_decimals: 2, max_leverage: 10 }))
            .collect();
        let mids: HashMap<String, f64> = candidates
            .iter()
            .map(|c| (c.to_string(), if *c == non_slot { 70.0 } else { 100.0 }))
            .collect();
        let weights = vec![("BTC".into(), 0.5), ("ETH".into(), -0.5)];
        let plan = build_plan(&weights, &acct, &markets, &mids, &cfg, None);
        let stop_order = plan
            .orders
            .iter()
            .find(|o| o.coin == non_slot)
            .expect("触发单币止损的币即使不在今日档位也必须平仓");
        assert!(stop_order.reduce_only && !stop_order.buy);
        assert_eq!(stop_order.target, 0.0);
        assert_eq!(stop_order.reason, "单币止损平仓");
    }

    #[test]
    fn stop_or_tp_hit_does_not_reopen_opposite_side_in_same_plan() {
        let cfg = TradeConfig {
            rebalance_slices: 1,
            ema_alpha: 1.0,
            position_stop_pct: 0.20,
            take_profit_pct: 0.15,
            ..TradeConfig::default()
        };
        let mut pos = HashMap::new();
        // BTC 做多浮亏 -30%（触发 stop_hit），而 wanted 目标里 BTC 翻空
        pos.insert("BTC".into(), crate::exchange::Pos { size: 5.0, entry_px: 100.0, ..Default::default() });
        // ETH 做多浮盈 +25%（触发 tp_hit），而 wanted 目标里 ETH 翻空
        pos.insert("ETH".into(), crate::exchange::Pos { size: 5.0, entry_px: 100.0, ..Default::default() });
        let acct = Acct { equity: 5000.0, positions: pos };
        let markets: HashMap<String, MarketInfo> = ["BTC", "ETH"]
            .iter()
            .map(|c| (c.to_string(), MarketInfo { sz_decimals: 2, max_leverage: 10 }))
            .collect();
        let mids: HashMap<String, f64> = HashMap::from([("BTC".into(), 70.0), ("ETH".into(), 125.0)]);
        let weights = vec![("BTC".into(), -0.5), ("ETH".into(), -0.5)];
        let plan = build_plan(&weights, &acct, &markets, &mids, &cfg, None);
        let btc_orders: Vec<_> = plan.orders.iter().filter(|o| o.coin == "BTC").collect();
        let eth_orders: Vec<_> = plan.orders.iter().filter(|o| o.coin == "ETH").collect();
        assert_eq!(btc_orders.len(), 1, "止损币不得在同一计划里立即反手开仓: {btc_orders:?}");
        assert!(btc_orders[0].reduce_only && btc_orders[0].reason == "单币止损平仓");
        assert_eq!(eth_orders.len(), 1, "止盈币不得在同一计划里立即反手开仓: {eth_orders:?}");
        assert!(eth_orders[0].reduce_only && eth_orders[0].reason == "止盈平仓");
    }

    #[test]
    fn single_over_cap_trims_non_slot_position_immediately() {
        let cfg = TradeConfig {
            rebalance_slices: 3,
            max_single_weight: 0.20,
            ..TradeConfig::default()
        };
        let day = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            / 86_400_000) as u32;
        let candidates = ["BTC", "ETH", "SOL", "DOGE", "ARB", "HYPE"];
        let non_slots: Vec<&str> = candidates
            .iter()
            .copied()
            .filter(|c| (day + slice_of(c, 3)) % 3 != 0)
            .collect();
        assert!(non_slots.len() >= 2, "需要至少 2 个不在今日档位的币");
        let over_coin = non_slots[0];
        let ok_coin = non_slots[1];
        // 净值 $5000，20% 上限 = $1000；over_coin 当前持仓 $1650 (33%)，ok_coin 当前持仓 $500 (10%)
        // 总敞口 $2150 < 2.7x * $5000，不会触发总敞口 over_cap，纯粹检验单币超限越过错峰。
        let mut positions = HashMap::new();
        positions.insert(over_coin.to_string(), crate::exchange::Pos { size: 16.5, entry_px: 100.0, ..Default::default() });
        positions.insert(ok_coin.to_string(), crate::exchange::Pos { size: 5.0, entry_px: 100.0, ..Default::default() });
        let acct = Acct { equity: 5000.0, positions };
        let markets: HashMap<String, MarketInfo> = candidates
            .iter()
            .map(|c| (c.to_string(), MarketInfo { sz_decimals: 2, max_leverage: 10 }))
            .collect();
        let mids: HashMap<String, f64> = candidates.iter().map(|c| (c.to_string(), 100.0)).collect();
        // 目标权重里 over_coin 原本偏大（会被截到 20% = $1000），ok_coin 目标 15%（非今日档位应不动）
        let weights = vec![
            (over_coin.to_string(), 0.12),
            (ok_coin.to_string(), 0.05),
        ];
        let plan = build_plan(&weights, &acct, &markets, &mids, &cfg, None);
        let over_order = plan.orders.iter().find(|o| o.coin == over_coin);
        assert!(over_order.is_some(), "超限单币 {over_coin} 即使不在今日档位也应立即减仓");
        let o = over_order.unwrap();
        assert!(o.reduce_only && !o.buy, "减多头必须是 reduce_only 卖出");
        assert_eq!(o.reason, "单币超限减仓");
        assert!((o.target - 10.0).abs() < 0.05, "目标仓位应截到 20% ($1000 = 10.0)，实际 {}", o.target);
        assert!(!plan.orders.iter().any(|o| o.coin == ok_coin), "未超限的非今日档位币 {ok_coin} 不应调仓");
    }
}
