//! 资金费 carry 策略 —— 纸面引擎。
//!
//! 真实行情、模拟下单、完整核算。不上实盘，不发任何真实订单。
//!
//! 策略规则（每 60 秒随行情刷新 step 一次）：
//! - 开仓：某币 |年化资金费差| ≥ entry_threshold，且未持有、对数未满，
//!   按 |差| 从大到小开仓。方向：差 > 0 → 空 HL / 多币安；差 < 0 → 多 HL / 空币安。
//!   每对名义 = carry 预算 ÷ max_pairs（双边等名义）。
//! - 核算：每步按当前资金费率计提（HL 小时费、币安 8h 费折算小时），
//!   基差按两边标记价逐腿盯市。手续费按 taker 4.5bp × 两腿，进出各收一次（共 18bp）。
//! - 平仓：价差跌破 exit_threshold / 价差翻负 / 持有超时 / 基差亏损超过止损线。
//!
//! 诚实标注（纸面近似，实盘需复核）：
//! - 资金费用「当前费率」计提，不是结算时点费率；数据缺失期间补计提也用恢复后的费率。
//! - 标记价来自公开接口，与真实成交/强平价有差异。

use serde::{Deserialize, Serialize};
use std::path::Path;

/// taker 费率（HL 与币安基础档都是 4.5bp）。
const TAKER: f64 = 0.00045;
/// 进场费 = 两腿各一次 taker。
const ENTRY_FEE_RATE: f64 = TAKER * 2.0;
/// 出场费同上。
const EXIT_FEE_RATE: f64 = TAKER * 2.0;
/// 一年小时数（资金费年化换算）。
const HOURS_PER_YEAR: f64 = 365.0 * 24.0;
/// 基差止损后的冷却时长（毫秒）：24 小时。
const COOLDOWN_MS: i64 = 24 * 3_600_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarryConfig {
    /// 年化资金费差达到多少才开仓（0.06 = 6%）。
    pub entry_threshold: f64,
    /// 年化资金费差跌破多少就平仓（0.02 = 2%）。
    pub exit_threshold: f64,
    /// 最多同时持有几对。
    pub max_pairs: usize,
    /// 最长持有小时数（超时强制平仓）。
    pub max_holding_hours: i64,
    /// 最短持有小时数：价差收窄的平仓要等至少持有这么久（否则资金费还没覆盖
    /// 18bp 的来回手续费就平了，数学上必亏 ✗ 2026-10-18 实测：6% 年化需要
    /// 263 小时才回本 ✗ 30% 需要 53 小时 ✗ 50% 需要 32 小时 ✗）。
    #[serde(default = "default_min_hold")]
    pub min_hold_hours: i64,
    /// 基差止损：单对基差亏损占名义的比例（0.02 = 2%）。
    pub stop_loss_pct: f64,
}

fn default_min_hold() -> i64 {
    72
}

impl Default for CarryConfig {
    fn default() -> Self {
        // 默认值必须能覆盖成本 ✗ 6%/2% 的旧默认会让策略每小时开平一次 ✗
        // 18bp 来回成本下：40% 年化持有 72h ≈ 33bp 收入 ✗ 才有点肉 ✗
        CarryConfig {
            entry_threshold: 0.40,
            exit_threshold: 0.10,
            max_pairs: 5,
            max_holding_hours: 720,
            min_hold_hours: 72,
            stop_loss_pct: 0.02,
        }
    }
}

/// 一对纸面持仓（HL 腿 + 币安腿，等名义、反向）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarryPair {
    pub coin: String,
    /// +1 = 空 HL / 多币安（差 > 0 时开）；−1 = 多 HL / 空币安。
    pub direction: i32,
    /// 每腿名义（美元）。
    pub notional: f64,
    pub entry_hl_px: f64,
    pub entry_bn_px: f64,
    pub entry_ts: i64,
    /// 进场时的年化价差（记录用）。
    pub entry_diff: f64,
    /// 累计净资金费（正 = 净收）。
    pub funding_accrued: f64,
    /// 实时基差盈亏（两腿合计，按当前标记价）。
    pub basis_pnl: f64,
    /// 进场手续费（已从盈亏中扣除）。
    pub entry_fee: f64,
    pub last_accrual_ts: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarryLog {
    pub ts: i64,
    pub msg: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarryState {
    pub config: CarryConfig,
    pub pairs: Vec<CarryPair>,
    /// 已平仓对累计净盈亏（含费）。
    pub closed_pnl: f64,
    pub log: Vec<CarryLog>,
    /// (ts, 权益) 每小时一个点。权益 = start_equity + 累计盈亏。
    pub history: Vec<(i64, f64)>,
    /// 期初预算快照（第一次开仓或 reset 时的 carry 预算）。
    pub start_equity: f64,
    /// 最近读到的 carry 预算（资金矩阵里配的）。
    pub budget_usd: f64,
    pub updated: Option<i64>,
    /// 平仓后的冷却期：coin -> 平仓时间。冷却期内不再开同名币
    /// （防止"止损/超时/翻负 → 立刻再开 → 再交费"的连环 churn）。
    /// 「价差收窄」的平仓不进冷却——重开需 |diff| ≥ 开仓阈值，本身有滞回。
    #[serde(default)]
    pub cooldowns: std::collections::HashMap<String, i64>,
}

impl Default for CarryState {
    fn default() -> Self {
        CarryState {
            config: CarryConfig::default(),
            pairs: Vec::new(),
            closed_pnl: 0.0,
            log: Vec::new(),
            history: Vec::new(),
            start_equity: 0.0,
            budget_usd: 0.0,
            updated: None,
            cooldowns: std::collections::HashMap::new(),
        }
    }
}

impl CarryState {
    pub fn load(path: &Path) -> Self {
        let mut st: CarryState = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        // 迁移：6%/2% 是旧默认值 ✗ 实测下 6% 年化要持有 263 小时才覆盖 18bp 成本 ✗
        // 每小时开平一次 = 必亏 ✗ 这不是用户的选择 ✗ 自动升到能覆盖成本的默认 ✗
        if (st.config.entry_threshold - 0.06).abs() < 1e-9
            && (st.config.exit_threshold - 0.02).abs() < 1e-9
        {
            st.config.entry_threshold = 0.40;
            st.config.exit_threshold = 0.10;
        }
        st
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

    /// 当前总盈亏（未实现 + 已实现）。
    pub fn total_pnl(&self) -> f64 {
        let open: f64 = self
            .pairs
            .iter()
            .map(|p| p.funding_accrued + p.basis_pnl - p.entry_fee)
            .sum();
        self.closed_pnl + open
    }

    /// 当前权益 = 期初预算 + 总盈亏。
    pub fn equity(&self) -> f64 {
        self.start_equity + self.total_pnl()
    }

    fn push_log(&mut self, now: i64, msg: String) {
        self.log.push(CarryLog { ts: now, msg });
        if self.log.len() > 500 {
            self.log.drain(0..self.log.len() - 500);
        }
    }
}

/// 行情快照里 carry 需要的一条数据。
pub struct MarketRow {
    pub hl_px: f64,
    pub bn_px: f64,
    pub hl_ann: f64,
    pub bn_ann: f64,
}

/// 由资金费面板数据构造行情表（coin -> 行情）。
pub fn market_rows(st: &crate::funding::FundingState) -> std::collections::HashMap<String, MarketRow> {
    st.pairs
        .iter()
        .map(|p| {
            (
                p.coin.clone(),
                MarketRow {
                    hl_px: p.hl_px,
                    bn_px: p.bn_px,
                    hl_ann: p.hl_ann,
                    bn_ann: p.bn_ann,
                },
            )
        })
        .collect()
}

/// 推进一步：按预算与最新行情开/平仓、计提资金费、盯市、记权益。
pub fn step(state: &mut CarryState, budget_usd: f64, market: &std::collections::HashMap<String, MarketRow>) {
    let now = crate::live::now_ms_pub();
    state.updated = Some(now);
    state.budget_usd = budget_usd;
    if state.pairs.is_empty() && state.start_equity <= 0.0 && budget_usd > 0.0 {
        state.start_equity = budget_usd;
    }

    // ---- ① 计提资金费 + 盯市 + 检查平仓 ----
    let mut i = 0;
    while i < state.pairs.len() {
        let coin = state.pairs[i].coin.clone();
        // 行情缺失或异常（币安掉线、px=0/NaN）：本轮跳过该对，下轮恢复后补计提。
        // px 校验防止用 0 价盯市产生 ±notional 的幽灵基差、触发假止损。
        let Some(row) = market.get(&coin).filter(|r| {
            r.hl_px > 0.0
                && r.bn_px > 0.0
                && r.hl_px.is_finite()
                && r.bn_px.is_finite()
                && r.hl_ann.is_finite()
                && r.bn_ann.is_finite()
        }) else {
            i += 1;
            continue;
        };

        // 计提资金费 + 盯市（可变借用收在本作用域内）。
        {
            let pair = &mut state.pairs[i];
            let dt_hours = ((now - pair.last_accrual_ts).max(0) as f64) / 3_600_000.0;
            if dt_hours > 0.0 && dt_hours < 48.0 {
                let hl_hourly = row.hl_ann / HOURS_PER_YEAR;
                let bn_hourly = row.bn_ann / HOURS_PER_YEAR;
                let net = pair.notional * pair.direction as f64 * (hl_hourly - bn_hourly) * dt_hours;
                pair.funding_accrued += net;
                pair.last_accrual_ts = now;
            } else if dt_hours >= 48.0 {
                // 断档过长：放弃缺口段、时间戳前进到本轮，防止按恢复后的费率
                // 一次性巨额计提，也防止时间戳停滞导致之后永远不再计提。
                pair.last_accrual_ts = now;
            }
            let qty_hl = pair.notional / pair.entry_hl_px;
            let qty_bn = pair.notional / pair.entry_bn_px;
            let (hl_leg, bn_leg) = if pair.direction > 0 {
                // 空 HL / 多币安
                ((pair.entry_hl_px - row.hl_px) * qty_hl, (row.bn_px - pair.entry_bn_px) * qty_bn)
            } else {
                // 多 HL / 空币安
                ((row.hl_px - pair.entry_hl_px) * qty_hl, (pair.entry_bn_px - row.bn_px) * qty_bn)
            };
            pair.basis_pnl = hl_leg + bn_leg;
        }

        // 平仓条件（先算原因，再动作）。
        let diff = row.hl_ann - row.bn_ann;
        let exit_reason: Option<String> = {
            let p = &state.pairs[i];
            let held_hours = (now - p.entry_ts) / 3_600_000;
            let flipped = diff * (p.direction as f64) < 0.0 && diff.abs() >= 0.05;
            if flipped {
                // 价差实质性反转（≥5%）：carry 已经反向，不等了
                Some("价差反转".into())
            } else if diff.abs() < state.config.exit_threshold
                && held_hours >= state.config.min_hold_hours
            {
                Some("价差收窄到出场线".into())
            } else if diff * (p.direction as f64) < 0.0 {
                Some("价差翻负".into())
            } else if now - p.entry_ts > state.config.max_holding_hours * 3_600_000 {
                Some("持有超时".into())
            } else if p.basis_pnl < (-state.config.stop_loss_pct * p.notional) {
                Some("基差止损".into())
            } else {
                None
            }
        };
        if let Some(why) = exit_reason {
            let (fa, bp, ef, n) = {
                let p = &state.pairs[i];
                (p.funding_accrued, p.basis_pnl, p.entry_fee, p.notional)
            };
            let exit_fee = EXIT_FEE_RATE * n;
            let realized = fa + bp - ef - exit_fee;
            state.closed_pnl += realized;
            state.pairs.remove(i);
            // 冷却 24 小时，防止同一步/下一轮立刻重开同名币：
            // 基差止损（背离还在）、超时（条件还在）、翻负（大震荡来回 churn）。
            // 只有「价差收窄」不需要冷却——重开要 |diff|≥6%，出场阈值 2% 本身滞回。
            if why != "价差收窄到出场线" {
                state.cooldowns.insert(coin.clone(), now);
            }
            state.push_log(
                now,
                format!(
                    "平仓 {}（{}）：资金费 {:+.2} + 基差 {:+.2} − 费 {:.2} = 净 {:+.2}",
                    coin, why, fa, bp, ef + exit_fee, realized
                ),
            );
            continue;
        }
        i += 1;
    }

    // ---- ② 开仓 ----
    if budget_usd > 0.0 && state.pairs.len() < state.config.max_pairs {
        // 清理过期冷却。
        state.cooldowns.retain(|_, t| now - *t < COOLDOWN_MS);
        let mut candidates: Vec<(&String, &MarketRow)> = market
            .iter()
            .filter(|(_, r)| r.hl_ann.is_finite() && r.bn_ann.is_finite() && r.hl_px > 0.0 && r.bn_px > 0.0)
            .filter(|(_, r)| (r.hl_ann - r.bn_ann).abs() >= state.config.entry_threshold)
            .filter(|(c, _)| !state.cooldowns.contains_key(*c))
            // 不重复开已在持仓的币（否则单币名义翻倍、占双份名额）。
            .filter(|(c, _)| !state.pairs.iter().any(|p| &p.coin == *c))
            .collect();
        candidates.sort_by(|a, b| {
            let da = (a.1.hl_ann - a.1.bn_ann).abs();
            let db = (b.1.hl_ann - b.1.bn_ann).abs();
            db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
        });
        let slots = state.config.max_pairs - state.pairs.len();
        let notional = budget_usd / state.config.max_pairs as f64;
        for (coin, row) in candidates.into_iter().take(slots) {
            let diff = row.hl_ann - row.bn_ann;
            let entry_fee = ENTRY_FEE_RATE * notional;
            state.pairs.push(CarryPair {
                coin: coin.clone(),
                direction: if diff > 0.0 { 1 } else { -1 },
                notional,
                entry_hl_px: row.hl_px,
                entry_bn_px: row.bn_px,
                entry_ts: now,
                entry_diff: diff,
                funding_accrued: 0.0,
                basis_pnl: 0.0,
                entry_fee,
                last_accrual_ts: now,
            });
            state.push_log(
                now,
                format!(
                    "开仓 {}（差 {:+.1}% 年化）：{}，每腿名义 ${:.0}，进场费 ${:.2}",
                    coin,
                    diff * 100.0,
                    if diff > 0.0 { "空 HL / 多币安" } else { "多 HL / 空币安" },
                    notional,
                    entry_fee
                ),
            );
        }
    }

    // ---- ③ 记权益（每小时一点）----
    let last_ts = state.history.last().map(|p| p.0).unwrap_or(0);
    if now - last_ts >= 3_600_000 {
        state.history.push((now, state.equity()));
        if state.history.len() > 24 * 90 {
            state.history.drain(0..state.history.len() - 24 * 90);
        }
    }
}

/// 清空纸面状态（保留配置与期初预算）。
pub fn reset(state: &mut CarryState, budget_usd: f64) {
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

    fn row(hl_px: f64, bn_px: f64, hl_ann: f64, bn_ann: f64) -> MarketRow {
        MarketRow { hl_px, bn_px, hl_ann, bn_ann }
    }

    fn mk_pair(coin: &str, direction: i32, entry_hl: f64, entry_bn: f64, last_accrual_ts: i64) -> CarryPair {
        let now = crate::live::now_ms_pub();
        CarryPair {
            coin: coin.into(),
            direction,
            notional: 200.0,
            entry_hl_px: entry_hl,
            entry_bn_px: entry_bn,
            entry_ts: now,
            entry_diff: 0.09,
            funding_accrued: 0.0,
            basis_pnl: 0.0,
            entry_fee: 0.18,
            last_accrual_ts,
        }
    }

    fn test_cfg(entry: f64, exit: f64) -> CarryConfig {
        CarryConfig { entry_threshold: entry, exit_threshold: exit, min_hold_hours: 0, ..CarryConfig::default() }
    }

    #[test]
    fn opens_pair_when_diff_above_threshold() {
        let mut st = CarryState { config: test_cfg(0.06, 0.02), ..CarryState::default() };
        let mut m = HashMap::new();
        m.insert("SOL".into(), row(100.0, 100.0, 0.11, 0.02)); // diff 9% ≥ 6%
        step(&mut st, 1000.0, &m);
        assert_eq!(st.pairs.len(), 1);
        assert_eq!(st.pairs[0].coin, "SOL");
        assert_eq!(st.pairs[0].direction, 1);
        assert_eq!(st.pairs[0].notional, 200.0); // 1000 / max_pairs 5
        assert_eq!(st.start_equity, 1000.0);
    }

    #[test]
    fn opens_short_direction_for_negative_diff() {
        let mut st = CarryState { config: test_cfg(0.06, 0.02), ..CarryState::default() };
        let mut m = HashMap::new();
        m.insert("X".into(), row(10.0, 10.0, -0.04, 0.05)); // diff −9%
        step(&mut st, 1000.0, &m);
        assert_eq!(st.pairs.len(), 1);
        assert_eq!(st.pairs[0].direction, -1); // 多 HL / 空币安
    }

    #[test]
    fn does_not_open_below_threshold() {
        let mut st = CarryState { config: test_cfg(0.06, 0.02), ..CarryState::default() };
        let mut m = HashMap::new();
        m.insert("SOL".into(), row(100.0, 100.0, 0.05, 0.02)); // diff 3% < 6%
        step(&mut st, 1000.0, &m);
        assert!(st.pairs.is_empty());
    }

    #[test]
    fn accrues_funding_and_marks_basis() {
        let now = crate::live::now_ms_pub();
        let mut st = CarryState { config: test_cfg(0.06, 0.02), ..CarryState::default() };
        st.start_equity = 1000.0;
        st.pairs.push(mk_pair("SOL", 1, 100.0, 100.0, now - 3_600_000)); // 1 小时前
        let mut m = HashMap::new();
        // HL 涨到 101（空单亏）、币安涨到 100.5（多单赚）。
        m.insert("SOL".into(), row(101.0, 100.5, 0.11, 0.02));
        step(&mut st, 1000.0, &m);
        let p = &st.pairs[0];
        // 资金费 1 小时: 200 × (0.11−0.02)/8760 × 1 ≈ 0.0020548（空HL收钱）。
        let expected = 200.0 * (0.11 - 0.02) / HOURS_PER_YEAR;
        assert!((p.funding_accrued - expected).abs() < 1e-4, "funding={} expected={}", p.funding_accrued, expected);
        // 基差: 空HL (100−101)×2 = −2；多币安 (100.5−100)×2 = +1 → −1.0。
        assert!((p.basis_pnl - (-1.0)).abs() < 1e-9, "basis={}", p.basis_pnl);
    }

    #[test]
    fn exits_on_spread_collapse() {
        let now = crate::live::now_ms_pub();
        let mut st = CarryState { config: test_cfg(0.06, 0.02), ..CarryState::default() };
        st.start_equity = 1000.0;
        st.pairs.push(mk_pair("SOL", 1, 100.0, 100.0, now));
        let mut m = HashMap::new();
        m.insert("SOL".into(), row(100.0, 100.0, 0.03, 0.02)); // diff 1% < 2%
        step(&mut st, 1000.0, &m);
        assert!(st.pairs.is_empty());
        assert!(st.log.iter().any(|l| l.msg.contains("价差收窄")));
    }

    #[test]
    fn exits_on_basis_stop() {
        let now = crate::live::now_ms_pub();
        let mut st = CarryState { config: test_cfg(0.06, 0.02), ..CarryState::default() };
        st.start_equity = 1000.0;
        st.pairs.push(mk_pair("SOL", 1, 100.0, 100.0, now));
        let mut m = HashMap::new();
        // HL 暴涨到 103 → 空腿亏 3×2=6 = 3% 名义 → 超过 2% 止损线。
        m.insert("SOL".into(), row(103.0, 100.0, 0.11, 0.02));
        step(&mut st, 1000.0, &m);
        assert!(st.pairs.is_empty());
        assert!(st.log.iter().any(|l| l.msg.contains("基差止损")));
    }

    #[test]
    fn reset_clears_but_keeps_config() {
        let now = crate::live::now_ms_pub();
        let mut st = CarryState::default();
        st.config.entry_threshold = 0.08;
        st.pairs.push(mk_pair("SOL", 1, 100.0, 100.0, now));
        st.closed_pnl = 5.0;
        reset(&mut st, 500.0);
        assert!(st.pairs.is_empty());
        assert_eq!(st.closed_pnl, 0.0);
        assert_eq!(st.config.entry_threshold, 0.08);
        assert_eq!(st.start_equity, 500.0);
        assert!(st.history.is_empty());
        assert!(st.cooldowns.is_empty());
    }

    #[test]
    fn does_not_duplicate_held_coin() {
        let now = crate::live::now_ms_pub();
        let mut st = CarryState { config: test_cfg(0.06, 0.02), ..CarryState::default() };
        st.start_equity = 1000.0;
        st.pairs.push(mk_pair("SOL", 1, 100.0, 100.0, now));
        let mut m = HashMap::new();
        // SOL 仍是最大价差, 但已在持仓, 不能再开一份。
        m.insert("SOL".into(), row(100.0, 100.0, 0.12, 0.02));
        m.insert("A".into(), row(10.0, 10.0, 0.09, 0.02));
        step(&mut st, 1000.0, &m);
        assert_eq!(st.pairs.len(), 2);
        assert_eq!(st.pairs.iter().filter(|p| p.coin == "SOL").count(), 1);
    }

    #[test]
    fn funding_resumes_after_48h_gap_without_huge_accrual() {
        let now = crate::live::now_ms_pub();
        let mut st = CarryState { config: test_cfg(0.06, 0.02), ..CarryState::default() };
        st.start_equity = 1000.0;
        st.pairs.push(mk_pair("SOL", 1, 100.0, 100.0, now - 50 * 3_600_000)); // 50h 断档
        let mut m = HashMap::new();
        m.insert("SOL".into(), row(100.0, 100.0, 0.11, 0.02));
        step(&mut st, 1000.0, &m);
        // 断档 ≥48h: 放弃缺口, 不按 50h 巨额计提; 且时间戳前进, 之后恢复计提。
        assert_eq!(st.pairs[0].funding_accrued, 0.0);
        let ts_after = st.pairs[0].last_accrual_ts;
        let now2 = crate::live::now_ms_pub();
        assert!(ts_after >= now2 - 10_000);
        // 再来一小时, 恢复正常计提。
        st.pairs[0].last_accrual_ts = now2 - 3_600_000;
        step(&mut st, 1000.0, &m);
        let expected = 200.0 * (0.11 - 0.02) / HOURS_PER_YEAR;
        assert!((st.pairs[0].funding_accrued - expected).abs() < 1e-4);
    }

    #[test]
    fn negative_direction_accrues_positive_funding() {
        let now = crate::live::now_ms_pub();
        let mut st = CarryState { config: test_cfg(0.06, 0.02), ..CarryState::default() };
        st.start_equity = 1000.0;
        // direction=-1: 多HL/空币安, 而 bn_ann > hl_ann → 净收 = (bn-hl)。
        st.pairs.push(mk_pair("X", -1, 10.0, 10.0, now - 3_600_000));
        let mut m = HashMap::new();
        m.insert("X".into(), row(10.0, 10.0, 0.02, 0.11));
        step(&mut st, 1000.0, &m);
        let expected = 200.0 * (0.11 - 0.02) / HOURS_PER_YEAR;
        assert!((st.pairs[0].funding_accrued - expected).abs() < 1e-4, "funding={}", st.pairs[0].funding_accrued);
    }

    #[test]
    fn no_open_with_zero_budget() {
        let mut st = CarryState { config: test_cfg(0.06, 0.02), ..CarryState::default() };
        let mut m = HashMap::new();
        m.insert("SOL".into(), row(100.0, 100.0, 0.11, 0.02));
        step(&mut st, 0.0, &m);
        assert!(st.pairs.is_empty());
    }

    #[test]
    fn skips_marking_when_price_zero() {
        let now = crate::live::now_ms_pub();
        let mut st = CarryState { config: test_cfg(0.06, 0.02), ..CarryState::default() };
        st.start_equity = 1000.0;
        st.pairs.push(mk_pair("SOL", 1, 100.0, 100.0, now));
        let mut m = HashMap::new();
        // 币安标记价瞬时为 0（数据异常）→ 本轮跳过, 不产生幽灵基差, 不假止损。
        m.insert("SOL".into(), row(101.0, 0.0, 0.11, 0.02));
        step(&mut st, 1000.0, &m);
        assert_eq!(st.pairs.len(), 1);
        assert_eq!(st.pairs[0].basis_pnl, 0.0);
    }

    #[test]
    fn timeout_exit_goes_to_cooldown() {
        let now = crate::live::now_ms_pub();
        let mut st = CarryState { config: test_cfg(0.06, 0.02), ..CarryState::default() };
        st.start_equity = 1000.0;
        let mut p = mk_pair("SOL", 1, 100.0, 100.0, now);
        p.entry_ts = now - 721 * 3_600_000; // 721 小时 > 720
        st.pairs.push(p);
        let mut m = HashMap::new();
        // 价差仍 ≥ 阈值: 超时平仓后若没有冷却会立刻重开。
        m.insert("SOL".into(), row(100.0, 100.0, 0.11, 0.02));
        step(&mut st, 1000.0, &m);
        assert!(st.pairs.is_empty());
        assert!(st.log.iter().any(|l| l.msg.contains("持有超时")));
        assert!(st.cooldowns.contains_key("SOL"));
    }
}
