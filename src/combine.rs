//! 组合策略：日线动量（A ✗ 现有实盘） + 12h 区间位置（B ✗ 新波段因子）。
//!
//! 两个信号都是【纯函数】，纸面和实盘共用同一份：
//!   · A 腿 = `trader::FactorPanel::weights_at`（动量）
//!   · B 腿 = `range_position_weights`（24h 区间位置 ✗ 12h 调仓）
//!   · 合并 = `blend`（各 0.5 ✗ 总杠杆 2.7x 时各 1.35x）
//!
//! 纸面引擎按【日】步进：A 按日线开仓持有到次日开仓；B 在日内拆成两个
//! 12h 子段，每段用 1h 数据重算信号。这与组合回测（combo.py）的口径一致。
//! 将来切实盘时，实盘引擎调用同样的 `range_position_weights` + `blend`，
//! 只是把 B 的调仓放到真实的 00:30/12:30 UTC，无需重写信号逻辑。
use crate::momentum::PanelEntry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 区间位置因子：对每个币算（现价 − 24h 最低）/（24h 最高 − 24h 最低）。
/// 只取【收盘价快照】✗ 用 1h bar 的 h/l 构造过去 24h 的高低。
/// 返回 (币, 因子值) ✗ 0=贴 24h 低点 ✗ 1=贴 24h 高点。
pub fn range_positions(
    panel_1h: &[PanelEntry],
    at_ms: i64,
) -> Vec<(String, f64)> {
    let lo = at_ms - 24 * 3_600_000;
    let mut out = Vec::new();
    for p in panel_1h {
        if p.coin.contains(':') {
            continue;
        }
        let (mut hi, mut low, mut last) = (f64::MIN, f64::MAX, f64::NAN);
        let mut n = 0usize;
        for k in p.candles.iter() {
            if k.t < lo || k.t > at_ms {
                continue;
            }
            if k.h > 0.0 {
                hi = hi.max(k.h);
            }
            if k.l > 0.0 {
                low = low.min(k.l);
            }
            if k.c > 0.0 {
                last = k.c;
            }
            n += 1;
        }
        // 至少要有 12 根小时 bar ✗ 否则 24h 区间不可信
        if n < 12 || !(hi > low) || !(last.is_finite() && last > 0.0) {
            continue;
        }
        out.push((p.coin.clone(), (last - low) / (hi - low)));
    }
    out
}

/// B 腿权重：区间位置最高 k 个做多 ✗ 最低 k 个做空 ✗ 各占 0.5/k。
/// 只保留流动性 ≥ min_vol_usd 的币（用过去 24h 成交额近似 ✗ 由调用方过滤）。
pub fn range_position_weights(
    positions: &[(String, f64)],
    k: usize,
) -> Vec<(String, f64)> {
    let kk = k.max(1);
    let mut sorted = positions.to_vec();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = Vec::new();
    if sorted.len() < 2 * kk {
        return out;
    }
    for (c, _) in sorted.iter().take(kk) {
        out.push((c.clone(), 0.5 / kk as f64));
    }
    for (c, _) in sorted.iter().rev().take(kk) {
        out.push((c.clone(), -0.5 / kk as f64));
    }
    out
}

/// 把 A、B 两腿权重合并成一本账（同币相加 ✗ 再整体归一化）。用于展示和实盘下单。
pub fn blend(a: &[(String, f64)], b: &[(String, f64)], wb: f64) -> Vec<(String, f64)> {
    let mut m: BTreeMap<String, f64> = BTreeMap::new();
    for (c, w) in a {
        *m.entry(c.clone()).or_default() += w * (1.0 - wb);
    }
    for (c, w) in b {
        *m.entry(c.clone()).or_default() += w * wb;
    }
    m.into_iter().filter(|(_, w)| w.abs() > 1e-12).collect()
}

// ---------------------------------------------------------------------------
// 纸面引擎
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CombineConfig {
    pub capital: f64,
    pub leverage: f64,
    pub min_vol_usd: f64,
    pub target_positions: usize,
    pub range_k: usize,
    /// B 腿在组合里的权重（0.5 = 各一半）。
    #[serde(default = "d_wb")]
    pub b_weight: f64,
    pub fee: f64,
    pub slippage: f64,
    pub momentum_weight: f64,
    pub dd_weight: f64,
    pub upfrac_weight: f64,
    pub replay_days: usize,
}

fn d_wb() -> f64 {
    0.5
}

impl Default for CombineConfig {
    fn default() -> Self {
        CombineConfig {
            capital: 5000.0,
            leverage: 2.7,
            min_vol_usd: 1_000_000.0,
            target_positions: 8,
            range_k: 3,
            b_weight: 0.5,
            fee: 0.00045,
            slippage: 0.0003,
            momentum_weight: 1.0,
            dd_weight: 0.0,
            upfrac_weight: 0.0,
            replay_days: 1,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct CombineSnapshot {
    pub equity: f64,
    pub pnl_pct: f64,
    pub days_elapsed: usize,
    pub history: Vec<(i64, f64)>,
    pub positions: Vec<(String, f64)>,
    pub running: bool,
    pub config: Option<CombineConfig>,
    pub last_a: Vec<(String, f64)>,
    pub last_b: Vec<(String, f64)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CombineState {
    pub config: Option<CombineConfig>,
    pub equity: f64,
    pub history: Vec<(i64, f64)>,
    pub last_ts: Option<i64>,
    pub last_a: Vec<(String, f64)>,
    pub last_b: Vec<(String, f64)>,
    pub running: bool,
    pub total_fee: f64,
}

impl CombineState {
    pub fn load_or_init(capital: f64) -> Self {
        let mut s = CombineState {
            config: Some(CombineConfig { capital, ..CombineConfig::default() }),
            equity: capital,
            history: Vec::new(),
            last_ts: None,
            last_a: Vec::new(),
            last_b: Vec::new(),
            running: true,
            total_fee: 0.0,
        };
        s.history.push((now_ms(), capital));
        s
    }

    pub fn snapshot(&self) -> CombineSnapshot {
        let capital = self.config.as_ref().map(|c| c.capital).unwrap_or(5000.0);
        CombineSnapshot {
            equity: self.equity,
            pnl_pct: if capital > 0.0 { (self.equity / capital - 1.0) * 100.0 } else { 0.0 },
            days_elapsed: self.history.len().saturating_sub(1),
            history: self.history.clone(),
            positions: blend(&self.last_a, &self.last_b, self.config.as_ref().map(|c| c.b_weight).unwrap_or(0.5)),
            running: self.running,
            config: self.config.clone(),
            last_a: self.last_a.clone(),
            last_b: self.last_b.clone(),
        }
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 纸面步进：日线 A + 12h B
// ---------------------------------------------------------------------------

/// 组合纸面的一步。panel_1d 是日线面板 ✗ panel_1h 是 1h 面板（同一批币）。
/// 只在【最后一天已完整收盘】时推进 ✗ 避免半根日线。
pub fn step(
    state: &mut CombineState,
    panel_1d: &[PanelEntry],
    panel_1h: &[PanelEntry],
) {
    let Some(cfg) = state.config.clone() else { return };
    // 日线时间轴
    let mut ts_all: Vec<i64> = Vec::new();
    for p in panel_1d {
        for k in p.candles.iter() {
            ts_all.push(k.t);
        }
    }
    ts_all.sort_unstable();
    ts_all.dedup();
    let n = ts_all.len();
    if n < 60 {
        return;
    }
    // 只处理到"最后一个完整日"的前一根 ✗ 因为需要该日的 o/h/l/c 完整
    let last_closed = ts_all[n - 1];
    // 找到上次推进到哪
    let mut i0 = 61usize;
    if let Some(last) = state.last_ts {
        if let Ok(j) = ts_all.binary_search(&last) {
            i0 = (j + 1).max(61);
        }
    }
    // 1h 收盘价索引
    let mut hclose: BTreeMap<String, BTreeMap<i64, f64>> = BTreeMap::new();
    let mut hvol: BTreeMap<String, BTreeMap<i64, f64>> = BTreeMap::new();
    for p in panel_1h {
        let c = hclose.entry(p.coin.clone()).or_default();
        let v = hvol.entry(p.coin.clone()).or_default();
        for k in p.candles.iter() {
            if k.c > 0.0 {
                c.insert(k.t, k.c);
                v.insert(k.t, k.c * k.v);
            }
        }
    }
    // 日线 FactorPanel（A 腿 ✗ 与实盘同一份）
    let fp = crate::trader::FactorPanel::build(panel_1d);
    let tcfg = crate::trader::TradeConfig {
        momentum_weight: cfg.momentum_weight,
        dd_weight: cfg.dd_weight,
        upfrac_weight: cfg.upfrac_weight,
        min_vol_usd: cfg.min_vol_usd,
        target_positions: cfg.target_positions,
        leverage: cfg.leverage,
        ..crate::trader::TradeConfig::default()
    };
    // 只在有完整新日时推进 ✗ 且一次只推一天（复利 ✗ 且 12h 依赖日内数据）
    let mut advanced = 0;
    for i in i0..n {
        if ts_all[i] >= last_closed {
            break; // 最后一天还没收盘 ✗ 下一轮再处理
        }
        let day_start = ts_all[i];
        let day_end = ts_all[i + 1];
        // ---- A 腿：动量权重（信号用 ≤ day_start 的数据 ✗ 已由 weights_at 保证）----
        let a_w = fp.weights_at(i, &tcfg, state.equity);
        // ---- B 腿：两个 12h 子段 ----
        let mut b_ret = 0.0;
        let mut b_w_final: Vec<(String, f64)> = Vec::new();
        for (s0, s1) in [(day_start, day_start + 12 * 3_600_000), (day_start + 12 * 3_600_000, day_start + 24 * 3_600_000)] {
            // 信号：s0 时刻的区间位置（用 ≤ s0 的 1h bar）
            let mut vols: BTreeMap<String, f64> = BTreeMap::new();
            for (c, vm) in hvol.iter() {
                let mut sum = 0.0;
                let mut cnt = 0usize;
                for (_t, v) in vm.range((s0 - 24 * 3_600_000)..s0) {
                    sum += v;
                    cnt += 1;
                }
                if cnt >= 6 {
                    vols.insert(c.clone(), sum / cnt as f64);
                }
            }
            // 用纯函数 range_positions（正确的 h/l 口径）✗ 再按流动性过滤
            let all_pos = range_positions(panel_1h, s0);
            let positions: Vec<(String, f64)> = all_pos
                .into_iter()
                .filter(|(c, _)| vols.get(c).map(|v| *v >= cfg.min_vol_usd).unwrap_or(false))
                .collect();
            let b_w = range_position_weights(&positions, cfg.range_k);
            b_w_final = b_w.clone();
            // 12h 收益：s0 的收盘价 → s1 的收盘价
            let mut sub = 0.0;
            for (c, w) in b_w.iter() {
                if let Some(cm) = hclose.get(c) {
                    let p0 = cm.range(..=s0).next_back().map(|(_, v)| *v);
                    let p1 = cm.range(..=s1).next_back().map(|(_, v)| *v);
                    if let (Some(a), Some(b)) = (p0, p1) {
                        if a > 0.0 {
                            sub += w * (b / a - 1.0);
                        }
                    }
                }
            }
            b_ret += sub;
        }
        // ---- A 腿收益：day_start 的开盘 → day_end 的开盘（open-to-open ✗ 与纸面同口径）----
        let a_ret = {
            let mut r = 0.0;
            for (c, w) in a_w.iter() {
                let o0 = panel_1d.iter().find(|p| p.coin == *c).and_then(|p| p.candles.iter().find(|k| k.t == day_start).map(|k| k.o));
                let o1 = panel_1d.iter().find(|p| p.coin == *c).and_then(|p| p.candles.iter().find(|k| k.t == day_end).map(|k| k.o));
                if let (Some(a), Some(b)) = (o0, o1) {
                    if a > 0.0 {
                        r += w * (b / a - 1.0);
                    }
                }
            }
            r
        };
        // 组合日收益 = (1-wb)*A + wb*B ✗ 扣换手成本（近似按全量重平衡 ✗ 保守）
        let wb = cfg.b_weight;
        let daily = (1.0 - wb) * a_ret + wb * b_ret;
        // 成本：A 每日换手 ~0.3 ✗ B 每 12h 全换 ✗ 近似
        let cost_a = 0.3 * (cfg.fee + cfg.slippage);
        let cost_b = 2.0 * (cfg.fee + cfg.slippage);
        let cost = (1.0 - wb) * cost_a + wb * cost_b;
        state.equity *= 1.0 + daily - cost;
        state.total_fee += state.equity * cost;
        state.last_ts = Some(day_start);
        state.last_a = a_w;
        state.last_b = b_w_final;
        state.history.push((day_start, state.equity));
        advanced += 1;
    }
    let _ = advanced;
}
