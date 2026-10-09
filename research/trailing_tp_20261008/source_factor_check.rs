use std::collections::{HashMap,BTreeMap,BTreeSet};
struct Candle {t:i64,c:f64,v:f64}
struct PanelEntry {coin:String,candles:Vec<Candle>}
struct TradeConfig {margin_buffer:f64,leverage:f64,min_position_usd:f64,target_positions:usize,lookback:usize,min_vol_usd:f64,top_frac:f64}
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

fn main(){
let txt=std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
let mut data: BTreeMap<String,Vec<Candle>>=BTreeMap::new();
for line in txt.lines(){let v:Vec<_>=line.split('\t').collect();data.entry(v[0].to_string()).or_default().push(Candle{t:v[1].parse().unwrap(),c:v[2].parse().unwrap(),v:v[3].parse().unwrap()});}
let panel:Vec<_>=data.into_iter().map(|(coin,candles)|PanelEntry{coin,candles}).collect();let fp=FactorPanel::build(&panel);
let cfg=TradeConfig{margin_buffer:0.9,leverage:3.0,min_position_usd:15.0,target_positions:5,lookback:14,min_vol_usd:5000000.0,top_frac:0.2};
for i in 32..fp.ts.len(){for (c,w) in fp.weights_at(i,&cfg,1000000.0){println!("{}\t{}\t{:.17}",i,c,w);}}
}
