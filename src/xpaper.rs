//! 跨所价差策略 —— 纸面引擎（OKX vs 币安现货）。
//!
//! 真实行情、模拟下单、完整核算。不发任何真实订单。
//!
//! 规则：
//! - 开仓：同一币 |OKX/币安 − 1| ≥ entry_pct，未持有、对数未满 → 开仓。
//!   价差为正（OKX 更贵）→ 空 OKX / 多币安；为负 → 多 OKX / 空币安。
//!   每对每腿名义 = 预算 ÷ max_pairs。
//! - 核算：两腿逐腿按当前价盯市；手续费按现货 taker（默认 0.1%/腿/次 ✗
//!   比永续贵得多 ✗ 这是这个策略的门槛 ✗）。
//! - 平仓：价差收敛到 exit_pct 以内 / 持有超时 / 亏损超过 stop_pct。
//!
//! 诚实标注：现货 0.1% 一腿，来回 4 腿 = 0.4% 成本 ✗
//! 所以价差要 > 0.4% 才可能赚 ✗ 平时 0.1~0.3% 的价差全是亏的 ✗

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct XPaperConfig {
    /// 开仓阈值（百分比，0.5 = 0.5%）。
    pub entry_pct: f64,
    /// 出场阈值（价差收敛到多少以内平仓）。
    pub exit_pct: f64,
    pub max_pairs: usize,
    /// 最长持有小时。
    pub max_holding_hours: i64,
    /// 单对亏损止损（占名义的比例）。
    pub stop_pct: f64,
    /// 每腿每次成交的手续费（0.001 = 0.1%）。
    pub fee: f64,
    /// 价差必须【连续 N 次刷新】都 ≥ 开仓阈值才开仓。
    /// 刷新频率 10 秒 ✗ 所以 N=3 ≈ 30 秒 ✗ N=6 ≈ 1 分钟 ✓
    #[serde(default = "d_confirm")]
    pub confirm_steps: u32,
    /// 两边 24h 成交额门槛（美元 ✗ 默认 $5M ✗ 排除薄币的假价差 ✗）。
    #[serde(default = "d_minvol")]
    pub min_vol_usd: f64,
}

fn d_confirm() -> u32 {
    // 10 秒一次刷新 ✗ 3 次 ≈ 30 秒 ✓
    3
}

fn d_minvol() -> f64 {
    // OKX 侧的 24h 成交额普遍只有 1~4M ✗ 门槛设 5M 会把绝大多数真实价差排除掉 ✗
    1_000_000.0
}

impl Default for XPaperConfig {
    fn default() -> Self {
        XPaperConfig {
            entry_pct: 0.5,
            exit_pct: 0.15,
            max_pairs: 5,
            max_holding_hours: 72,
            stop_pct: 0.8,
            fee: 0.001,
            confirm_steps: d_confirm(),
            min_vol_usd: d_minvol(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct XPair {
    pub coin: String,
    /// +1 = 空 OKX / 多币安；−1 = 多 OKX / 空币安。
    pub direction: i32,
    /// 每腿名义（美元）。
    pub notional: f64,
    pub entry_okx: f64,
    pub entry_bn: f64,
    /// 进场时的价差（%）。
    pub entry_diff: f64,
    pub entry_ts: i64,
    /// 进场手续费（两腿 ✗ 已扣 ✗）。
    pub entry_fee: f64,
    /// 实时浮动盈亏（已扣进场费 ✗ 未扣出场费 ✗）。
    pub pnl: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct XLog {
    pub ts: i64,
    pub msg: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct XPaperState {
    pub config: XPaperConfig,
    pub pairs: Vec<XPair>,
    pub closed_pnl: f64,
    pub log: Vec<XLog>,
    pub history: Vec<(i64, f64)>,
    pub start_equity: f64,
    pub budget_usd: f64,
    pub updated: Option<i64>,
    /// 止损后的冷却（coin -> 平仓时间）✗ 防止"止损 → 立刻重开 → 再止损"✗
    #[serde(default)]
    pub cooldowns: std::collections::HashMap<String, i64>,
    /// 价差连续达标的次数（coin -> 连续次数 ✗）
    #[serde(default)]
    pub streaks: std::collections::HashMap<String, u32>,
}

impl Default for XPaperState {
    fn default() -> Self {
        XPaperState {
            config: XPaperConfig::default(),
            pairs: Vec::new(),
            closed_pnl: 0.0,
            log: Vec::new(),
            history: Vec::new(),
            start_equity: 0.0,
            budget_usd: 0.0,
            updated: None,
            cooldowns: std::collections::HashMap::new(),
            streaks: std::collections::HashMap::new(),
        }
    }
}

impl XPaperState {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, path: &Path) -> Result<(), anyhow::Error> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
    pub fn total_pnl(&self) -> f64 {
        let open: f64 = self.pairs.iter().map(|p| p.pnl).sum();
        self.closed_pnl + open
    }
    pub fn equity(&self) -> f64 {
        self.start_equity + self.total_pnl()
    }
    fn push_log(&mut self, now: i64, msg: String) {
        self.log.push(XLog { ts: now, msg });
        if self.log.len() > 500 {
            self.log.drain(0..self.log.len() - 500);
        }
    }
}

/// 一条行情（同一币在两所的现货价）。
pub struct XRow {
    pub okx: f64,
    pub bn: f64,
    /// 两边 24h 成交额（美元 ✗）
    pub okx_vol: f64,
    pub bn_vol: f64,
    /// 盘口 ✗ 可执行价差要按买一/卖一算 ✗ 薄币的 last 可能是几秒前的 ✓
    pub okx_bid: f64,
    pub okx_ask: f64,
    pub bn_bid: f64,
    pub bn_ask: f64,
}

impl XRow {
    /// 可执行价差（%）✗ 取两个方向里更大的那个 ✓
    /// 买 OKX 卖币安 = 用 OKX 的卖一买 ✗ 币安的买一卖 ✓
    /// 买币安卖 OKX = 反过来 ✓
    pub fn exec_edge(&self) -> f64 {
        if self.okx_ask <= 0.0 || self.bn_bid <= 0.0 || self.bn_ask <= 0.0 || self.okx_bid <= 0.0 {
            return 0.0;
        }
        let a = (self.bn_bid / self.okx_ask - 1.0) * 100.0;
        let b = (self.okx_bid / self.bn_ask - 1.0) * 100.0;
        let (best, signed) = if a >= b { (a, -1.0) } else { (b, 1.0) };
        best.max(0.0) * signed
    }
}

/// 一对的浮动盈亏（未扣出场费）。
fn pair_pnl(p: &XPair, okx: f64, bn: f64) -> f64 {
    if p.entry_okx <= 0.0 || p.entry_bn <= 0.0 || okx <= 0.0 || bn <= 0.0 {
        return p.pnl;
    }
    let r = (bn / p.entry_bn) - (okx / p.entry_okx);
    p.notional * p.direction as f64 * r - p.entry_fee
}

/// 推进一步：盯市、平仓、开仓、记权益。
pub fn step(state: &mut XPaperState, budget_usd: f64, market: &std::collections::HashMap<String, XRow>) {
    let now = crate::live::now_ms_pub();
    state.updated = Some(now);
    state.budget_usd = budget_usd;
    if state.pairs.is_empty() && state.start_equity <= 0.0 && budget_usd > 0.0 {
        state.start_equity = budget_usd;
    }
    let fee = state.config.fee.max(0.0);

    // ---- 持续性计数：价差连续达标多少次（每次刷新 60 秒 ✗）----
    // 1 分钟就消失的价差是报价噪音 ✗ 只在连续 N 次达标后才允许开仓 ✗
    {
        let mut next: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        for (coin, row) in market.iter() {
            if row.okx <= 0.0 || row.bn <= 0.0 {
                continue;
            }
            // 用【可执行】价差（盘口）✗ 不是 last 价 ✗
            let edge = row.exec_edge();
            if edge.abs() >= state.config.entry_pct {
                let n = state.streaks.get(coin).copied().unwrap_or(0) + 1;
                next.insert(coin.clone(), n);
            }
        }
        state.streaks = next;
    }

    // ---- 盯市 + 平仓 ----
    let mut i = 0;
    while i < state.pairs.len() {
        let coin = state.pairs[i].coin.clone();
        let Some(row) = market.get(&coin) else {
            i += 1;
            continue;
        };
        if row.okx <= 0.0 || row.bn <= 0.0 {
            i += 1;
            continue;
        }
        let diff_now = row.exec_edge();
        {
            let p = &mut state.pairs[i];
            p.pnl = pair_pnl(p, row.okx, row.bn);
        }
        let exit_fee = fee * 2.0 * state.pairs[i].notional;
        let reason = {
            let p = &state.pairs[i];
            let converged =
                (diff_now.abs() <= state.config.exit_pct) || (diff_now.signum() != p.entry_diff.signum() && diff_now.abs() <= state.config.exit_pct * 2.0);
            if converged {
                Some("价差收敛")
            } else if now - p.entry_ts > state.config.max_holding_hours * 3_600_000 {
                Some("持有超时")
            } else if p.pnl - exit_fee < -state.config.stop_pct / 100.0 * p.notional {
                Some("价差扩大止损")
            } else {
                None
            }
        };
        if let Some(why) = reason {
            let pnl = state.pairs[i].pnl;
            let realized = pnl - exit_fee;
            state.closed_pnl += realized;
            state.pairs.remove(i);
            if why == "价差扩大止损" {
                // 冷却 12 小时：止损后价差往往还在扩大 ✗ 立刻重开就是白交费 ✗
                state.cooldowns.insert(coin.clone(), now);
            }
            state.push_log(
                now,
                format!(
                    "平仓 {}（{}）：浮动 {:+.2} − 出场费 {:.2} = 净 {:+.2}",
                    coin, why, pnl, exit_fee, realized
                ),
            );
            continue;
        }
        i += 1;
    }

    // ---- 开仓 ----
    if budget_usd > 0.0 && state.pairs.len() < state.config.max_pairs {
        state.cooldowns.retain(|_, t| now - *t < 12 * 3_600_000);
        let notional = budget_usd / state.config.max_pairs.max(1) as f64;
        let mut cands: Vec<(&String, &XRow, f64)> = market
            .iter()
            .filter(|(_, r)| r.okx > 0.0 && r.bn > 0.0)
            .map(|(c, r)| (c, r, r.exec_edge()))
            .filter(|(_, _, d)| d.abs() >= state.config.entry_pct)
            // 持续性：必须连续 N 次刷新都达标
            .filter(|(c, _, _)| {
                state.streaks.get(*c).copied().unwrap_or(0) >= state.config.confirm_steps.max(1)
            })
            // 流动性：两边 24h 成交额都要达标
            .filter(|(_, r, _)| {
                r.okx_vol >= state.config.min_vol_usd && r.bn_vol >= state.config.min_vol_usd
            })
            .filter(|(c, _, _)| !state.pairs.iter().any(|p| &p.coin == *c))
            .filter(|(c, _, _)| !state.cooldowns.contains_key(*c))
            .collect();
        cands.sort_by(|a, b| b.2.abs().partial_cmp(&a.2.abs()).unwrap_or(std::cmp::Ordering::Equal));
        let slots = state.config.max_pairs - state.pairs.len();
        for (coin, row, diff) in cands.into_iter().take(slots) {
            let entry_fee = fee * 2.0 * notional;
            state.pairs.push(XPair {
                coin: coin.clone(),
                direction: if diff > 0.0 { 1 } else { -1 },
                notional,
                entry_okx: row.okx,
                entry_bn: row.bn,
                entry_diff: diff,
                entry_ts: now,
                entry_fee,
                pnl: -entry_fee,
            });
            state.push_log(
                now,
                format!(
                    "开仓 {}：差 {:+.2}%（{}）✗ 每腿 ${:.0} ✗ 进场费 ${:.2}",
                    coin,
                    diff,
                    if diff > 0.0 { "空 OKX / 多币安" } else { "多 OKX / 空币安" },
                    notional,
                    entry_fee
                ),
            );
        }
    }

    // ---- 权益（每小时一点）----
    let last_ts = state.history.last().map(|p| p.0).unwrap_or(0);
    if now - last_ts >= 3_600_000 {
        state.history.push((now, state.equity()));
        if state.history.len() > 24 * 90 {
            let cut = state.history.len() - 24 * 90;
            state.history.drain(0..cut);
        }
    }
}

/// 清空纸面状态（保留参数）。
pub fn reset(state: &mut XPaperState, budget_usd: f64) {
    let now = crate::live::now_ms_pub();
    state.pairs.clear();
    state.closed_pnl = 0.0;
    state.history.clear();
    state.log.clear();
    state.cooldowns.clear();
    state.start_equity = budget_usd;
    state.budget_usd = budget_usd;
    state.push_log(now, format!("纸面重置：期初预算 ${:.0}", budget_usd));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn row(okx: f64, bn: f64) -> XRow {
        // 盘口按中间价上下 0.05% 构造 ✗ 让"可执行价差"近似等于 last 差 ✓
        XRow {
            okx, bn, okx_vol: 1e9, bn_vol: 1e9,
            okx_bid: okx * 0.9995, okx_ask: okx * 1.0005,
            bn_bid: bn * 0.9995, bn_ask: bn * 1.0005,
        }
    }

    #[test]
    fn opens_on_a_wide_gap_and_books_the_convergence() {
        let mut st = XPaperState::default();
        // 持续性默认 2 次刷新；单次 step 的单元测试把它设为 1
        st.config.confirm_steps = 1;
        let mut m = HashMap::new();
        // OKX 贵 1% → 空 OKX / 多币安
        m.insert("G".to_string(), row(101.0, 100.0));
        step(&mut st, 1000.0, &m);
        assert_eq!(st.pairs.len(), 1);
        let p = &st.pairs[0];
        assert_eq!(p.direction, 1);
        assert_eq!(p.notional, 200.0); // 1000 / 5
        assert!((p.entry_fee - 0.4).abs() < 1e-9); // 2 × 0.1% × 200

        // 价差收敛到 0.05%：两腿价差从 1% 掉到 0.05% → 赚约 0.95% × 200 = 1.9
        let mut m2 = HashMap::new();
        m2.insert("G".to_string(), row(100.05, 100.0));
        step(&mut st, 1000.0, &m2);
        assert!(st.pairs.is_empty(), "应已平仓: {:?}", st.log);
        // 收盘: 浮动 1.9 − 进场费 0.4 − 出场费 0.4 = 1.1
        assert!((st.closed_pnl - 1.1).abs() < 0.05, "closed={}", st.closed_pnl);
    }

    #[test]
    fn ignores_small_gaps() {
        let mut st = XPaperState::default();
        // 持续性默认 2 次刷新；单次 step 的单元测试把它设为 1
        st.config.confirm_steps = 1;
        let mut m = HashMap::new();
        m.insert("G".to_string(), row(100.3, 100.0)); // 0.3% < 0.5%
        step(&mut st, 1000.0, &m);
        assert!(st.pairs.is_empty());
    }

    #[test]
    fn stops_when_the_gap_widens() {
        let mut st = XPaperState::default();
        // 持续性默认 2 次刷新；单次 step 的单元测试把它设为 1
        st.config.confirm_steps = 1;
        let mut m = HashMap::new();
        m.insert("G".to_string(), row(101.0, 100.0));
        step(&mut st, 1000.0, &m);
        // 价差扩大到 2%：空 OKX 腿亏 1% × 200 = 2 → 超过止损 0.8% × 200 = 1.6
        let mut m2 = HashMap::new();
        m2.insert("G".to_string(), row(102.0, 100.0));
        step(&mut st, 1000.0, &m2);
        assert!(st.pairs.is_empty(), "应止损: {:?}", st.log);
        assert!(st.log.iter().any(|l| l.msg.contains("止损")));
    }
}
