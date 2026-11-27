//! Cross-sectional factor panel, liquidity filter, and target weight construction.

use super::TradeConfig;
use crate::store::PanelEntry;
use std::collections::{BTreeMap, BTreeSet, HashMap};

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

