//! HL 单账户 carry 影子引擎（2026-10）。
//!
//! 策略：**同一账户内**「买现货 + 等名义空永续」，赚取永续资金费。
//!
//! 篮子**不是写死的**：每次评估都按当时的资金费排名重新选币、按资金费高低
//! 分配权重。但"哪个高干哪个"必须过一道**换手闸门**——每次换仓要付
//! [`ROUND_TRIP_FEE_BP`]，追着资金费搬家会把收益全部交给手续费。
//!
//! 闸门的口径是"回本天数"：把这次换仓的一次性成本，按 [`Params::payback_days`]
//! 摊成年化，只有当目标篮子的加权资金费比当前篮子高出这个数，才真的动仓位。
//! 这条规则让"追高"和"别乱动"同时成立，而且两个参数都是可调的。
//!
//! 成本口径（HL 实际费率，2026-10）：
//!   - 现货 maker 4.0bp/side，永续 maker 1.44bp/side
//!   - 每腿单边 5.44bp，往返 10.88bp；在**交易发生的那一刻**按成交量预扣往返
//!
//! **不发任何真实订单。** 纯影子。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// 现货 maker 4bp + 永续 maker 1.44bp，往返 ×2。
pub const ROUND_TRIP_FEE_BP: f64 = 10.88;

// ---------------------------------------------------------------------------
// 参数
// ---------------------------------------------------------------------------

/// 选币与换仓规则。全部可在页面上改，不写死在代码里。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Params {
    /// 最多持有几条腿。
    pub top_k: usize,
    /// 资金费门槛（年化 %）。低于它的币不进入候选。
    pub min_funding_ann_pct: f64,
    /// 现货 24h 成交额门槛（USD）。太薄的现货买不进也卖不出。
    pub min_spot_volume_usd: f64,
    /// 单腿权重上限，避免全仓压在一个币上。
    pub max_leg_weight: f64,
    /// 换仓回本天数：一次性成本按这么多天摊成年化，作为换仓门槛。
    pub payback_days: f64,
    /// 最短持有小时数；只限制资金费轮动，不阻止基差止盈。
    #[serde(default = "d_min_hold")]
    pub min_hold_hours: f64,
    /// 相对入场基差收敛多少 bp 就止盈该腿；0 表示关闭。
    #[serde(default = "d_basis_take_profit")]
    pub basis_take_profit_bp: f64,
    /// 新开/补仓时 perp/spot 至少升水多少 bp；0 表示关闭基差入场过滤。
    #[serde(default = "d_min_entry_basis")]
    pub min_entry_basis_bp: f64,
}

fn d_min_hold() -> f64 {
    48.0
}

fn d_basis_take_profit() -> f64 {
    50.0
}

fn d_min_entry_basis() -> f64 {
    20.0
}

impl Default for Params {
    fn default() -> Self {
        Params {
            top_k: 5,
            min_funding_ann_pct: 3.0,
            min_spot_volume_usd: 1_000_000.0,
            max_leg_weight: 0.40,
            payback_days: 90.0,
            min_hold_hours: 48.0,
            basis_take_profit_bp: 50.0,
            min_entry_basis_bp: 20.0,
        }
    }
}

impl Params {
    pub fn sanitized(self) -> Self {
        Params {
            top_k: self.top_k.clamp(1, 12),
            min_funding_ann_pct: self.min_funding_ann_pct.clamp(-50.0, 500.0),
            min_spot_volume_usd: self.min_spot_volume_usd.clamp(0.0, 1e12),
            max_leg_weight: self.max_leg_weight.clamp(0.05, 1.0),
            payback_days: self.payback_days.clamp(1.0, 365.0),
            min_hold_hours: self.min_hold_hours.clamp(0.0, 720.0),
            basis_take_profit_bp: self.basis_take_profit_bp.clamp(0.0, 2_000.0),
            min_entry_basis_bp: self.min_entry_basis_bp.clamp(0.0, 2_000.0),
        }
    }
}

// ---------------------------------------------------------------------------
// 候选与权重
// ---------------------------------------------------------------------------

/// 一个可做 carry 的标的一次快照。
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub coin: String,
    pub spot: String,
    /// 年化资金费（%）。
    pub funding_ann: f64,
    /// 现货 24h 成交额（USD）。
    pub spot_volume: f64,
    /// 当前 perp/spot 升水（bp）；买现货空永续需要从更高升水向下收敛。
    pub basis_bp: f64,
}

/// 目标持仓：币、现货对、目标权重。
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub coin: String,
    pub spot: String,
    pub weight: f64,
    pub funding_ann: f64,
}

/// 从全市场挑候选：现货够厚 + 资金费与入场基差都过门槛，按资金费取前 K。
pub fn select_candidates(market: &Market, params: &Params) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = market
        .latched
        .iter()
        .filter(|l| l.spot_volume >= params.min_spot_volume_usd)
        .filter(|l| l.funding_ann > params.min_funding_ann_pct)
        .filter_map(|l| {
            let q = market.quote(&l.coin, &l.spot)?;
            if !(q.spot > 0.0 && q.perp > 0.0) {
                return None;
            }
            let basis_bp = (q.perp - q.spot) / q.spot * 10_000.0;
            if basis_bp < params.min_entry_basis_bp {
                return None;
            }
            Some(Candidate {
                coin: l.coin.clone(),
                spot: l.spot.clone(),
                funding_ann: l.funding_ann,
                spot_volume: l.spot_volume,
                basis_bp,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.funding_ann
            .partial_cmp(&a.funding_ann)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.spot_volume.partial_cmp(&a.spot_volume).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.coin.cmp(&b.coin))
    });
    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert(c.coin.clone()));
    out.truncate(params.top_k);
    out
}

/// 按资金费高低分配权重：权重 ∝（资金费 − 门槛），再做单腿上限制并归一。
pub fn target_weights(cands: &[Candidate], params: &Params) -> Vec<Target> {
    let floor = params.min_funding_ann_pct;
    let raw: Vec<f64> = cands
        .iter()
        .map(|c| (c.funding_ann - floor).max(0.0))
        .collect();
    let total: f64 = raw.iter().sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let cap = params.max_leg_weight;
    // 单腿上限制：触发时**不把余量摊给别的腿**。
    // 摊给别人的话，被砍掉的那部分会流向资金费更低的标的——
    // 等于因为风险约束去买更差的 carry。余量留作现金（权重和 ≤ 1）。
    let w: Vec<f64> = raw
        .iter()
        .map(|r| (r / total).min(cap))
        .collect();

    cands
        .iter()
        .zip(w.iter())
        .filter(|(_, x)| **x > 1e-9)
        .map(|(c, x)| Target {
            coin: c.coin.clone(),
            spot: c.spot.clone(),
            weight: *x,
            funding_ann: c.funding_ann,
        })
        .collect()
}

/// 换手率：Σ|目标权重 − 当前权重| / 2。0 = 完全不动，1 = 全换。
pub fn turnover(current: &[(String, f64)], target: &[Target]) -> f64 {
    let mut coins: Vec<&str> = current.iter().map(|(c, _)| c.as_str()).collect();
    for t in target {
        if !coins.contains(&t.coin.as_str()) {
            coins.push(&t.coin);
        }
    }
    let mut sum = 0.0;
    for c in coins {
        let cur = current
            .iter()
            .find(|(k, _)| k == c)
            .map(|(_, w)| *w)
            .unwrap_or(0.0);
        let tgt = target
            .iter()
            .find(|t| t.coin == c)
            .map(|t| t.weight)
            .unwrap_or(0.0);
        sum += (tgt - cur).abs();
    }
    sum / 2.0
}

/// 一次换仓的一次性成本，按 payback_days 摊成年化（%）。
pub fn rebalance_threshold_ann_pct(turn: f64, params: &Params) -> f64 {
    let one_time_pct = turn * ROUND_TRIP_FEE_BP / 100.0; // bp -> %
    one_time_pct * 365.0 / params.payback_days
}

// ---------------------------------------------------------------------------
// 状态
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Leg {
    pub coin: String,
    pub spot: String,
    pub weight: f64,
    pub notional: f64,
    pub spot_qty: f64,
    pub perp_qty_abs: f64,
    pub spot_entry: f64,
    pub perp_entry: f64,
    pub entry_funding_ann: f64,
    pub funding_accrued: f64,
    pub basis_pnl: f64,
    pub fee_paid: f64,
    pub last_accrual_ms: i64,
    pub spot_px: f64,
    pub perp_px: f64,
    pub funding_ann: f64,
    pub basis_bp: f64,
    /// 当前 perp/spot 相对入场时的变化（bp）；负值表示收敛、该腿获利。
    #[serde(default)]
    pub basis_move_bp: f64,
    pub leg_pnl: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarryLog {
    pub ts: i64,
    pub msg: String,
}

/// 一次换仓的完整记录，用来在页面上解释"为什么动了仓位"。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RebalanceRecord {
    pub ts: i64,
    /// 换手率 0~1。
    pub turnover: f64,
    /// 换仓前后的加权年化资金费（%）。
    pub funding_before: f64,
    pub funding_after: f64,
    /// 资金费轮动时为年化门槛（%），基差止盈时为 bp 门槛。
    pub threshold: f64,
    /// 本次换仓付出的手续费（USD）。
    pub fee_usd: f64,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// (coin, 旧权重, 新权重)
    pub resized: Vec<(String, f64, f64)>,
    /// 换仓后的目标篮子：(coin, 权重, 资金费%/年)，按资金费降序。
    pub target: Vec<(String, f64, f64)>,
    #[serde(default)]
    pub reason: String,
    /// 基差止盈时各币相对入场基差的变化（bp）。
    #[serde(default)]
    pub basis_moves: Vec<(String, f64)>,
}

/// 按 UTC 自然日聚合的表现。增量口径，重启后继续累计。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DailyStat {
    pub date: String,
    pub equity_start: f64,
    pub equity_end: f64,
    /// 当日资金费（USD）。
    pub funding: f64,
    /// 当日基差盈亏变化（USD）。
    pub basis: f64,
    /// 当日手续费（USD）。
    pub fees: f64,
    pub rebalances: u32,
    /// 当日起点的累计基线（用于算增量）。
    #[serde(default)]
    pub base_funding: f64,
    #[serde(default)]
    pub base_basis: f64,
    #[serde(default)]
    pub base_fees: f64,
}

/// UTC 毫秒 → "YYYY-MM-DD"。不引 chrono，用 civil-from-days。
pub fn ymd_from_ms(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HlCarryState {
    #[serde(default)]
    pub params: Params,
    pub legs: Vec<Leg>,
    pub budget_usd: f64,
    pub start_equity: f64,
    pub closed_pnl: f64,
    pub history: Vec<(i64, f64)>,
    pub log: Vec<CarryLog>,
    pub updated: Option<i64>,
    #[serde(default)]
    pub last_history_ms: Option<i64>,
    /// 上一次评估换仓的时间。
    #[serde(default)]
    pub last_check_ms: Option<i64>,
    /// 累计为换仓付出的手续费（只增不减，用来暴露 churn 成本）。
    #[serde(default)]
    pub fees_paid_total: f64,
    /// 累计收到的资金费（只增不减；腿被移出也不回退）。
    #[serde(default)]
    pub funding_total: f64,
    /// 最近若干次换仓的结构化记录。
    #[serde(default)]
    pub rebalances: Vec<RebalanceRecord>,
    /// 每日聚合。
    #[serde(default)]
    pub daily: Vec<DailyStat>,
    /// 当前轮动周期内已基差止盈的币（同周期不立刻买回，防止来回刷手续费）。
    #[serde(default)]
    pub tp_cooldown_coins: Vec<String>,
}

impl Default for HlCarryState {
    fn default() -> Self {
        HlCarryState {
            params: Params::default(),
            legs: Vec::new(),
            budget_usd: 0.0,
            start_equity: 0.0,
            closed_pnl: 0.0,
            history: Vec::new(),
            log: Vec::new(),
            updated: None,
            last_history_ms: None,
            last_check_ms: None,
            fees_paid_total: 0.0,
            funding_total: 0.0,
            rebalances: Vec::new(),
            daily: Vec::new(),
            tp_cooldown_coins: Vec::new(),
        }
    }
}

impl HlCarryState {
    pub fn load(path: &Path) -> Self {
        let mut st: HlCarryState = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        st.params = st.params.clone().sanitized();
        st
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        use std::ffi::OsString;

        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录 {}", parent.display()))?;
        let file_name = path.file_name().context("状态文件路径缺少文件名")?;
        let mut tmp_name = OsString::from(".");
        tmp_name.push(file_name);
        tmp_name.push(".tmp");
        let tmp = parent.join(tmp_name);
        let bytes = serde_json::to_vec_pretty(self)?;
        if let Err(e) = std::fs::write(&tmp, bytes) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("写入临时文件 {}", tmp.display()));
        }
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("替换状态文件 {}", path.display()));
        }
        Ok(())
    }

    pub fn equity(&self) -> f64 {
        self.start_equity + self.legs.iter().map(|l| l.leg_pnl).sum::<f64>() + self.closed_pnl
    }

    pub fn annualized_pct(&self) -> Option<f64> {
        let first = *self.history.first()?;
        let last = *self.history.last()?;
        let hours = (last.0 - first.0) as f64 / 3_600_000.0;
        if hours < 1.0 || self.start_equity <= 0.0 {
            return None;
        }
        let ret = self.equity() / self.start_equity - 1.0;
        Some(ret * (365.0 * 24.0 / hours) * 100.0)
    }

    fn log(&mut self, now: i64, msg: String) {
        self.log.push(CarryLog { ts: now, msg });
        if self.log.len() > 300 {
            let drop = self.log.len() - 300;
            self.log.drain(0..drop);
        }
    }

    /// 按预算建仓（首次开仓 / 手动重置）。只有这里可以清空历史。
    pub fn open(&mut self, budget_usd: f64, market: &Market, now: i64) {
        self.legs.clear();
        self.closed_pnl = 0.0;
        self.history.clear();
        self.fees_paid_total = 0.0;
        self.funding_total = 0.0;
        self.rebalances.clear();
        self.daily.clear();
        self.tp_cooldown_coins.clear();
        self.budget_usd = budget_usd;
        self.start_equity = budget_usd;
        self.last_history_ms = Some(now);
        self.last_check_ms = Some(now);
        // 先立当日的基线，再建仓——否则开仓手续费会落在基线之前。
        self.roll_daily(now);
        if !self.reopen(market, now) {
            self.log(now, "没有满足条件的标的（现货成交额 / 资金费 / 入场基差门槛）".into());
        }
    }

    /// 空仓后重新寻找机会，保留累计盈亏、历史曲线和已实现资金费。
    pub fn reopen(&mut self, market: &Market, now: i64) -> bool {
        if self.start_equity <= 0.0 || !self.legs.is_empty() {
            return false;
        }
        self.tp_cooldown_coins.clear();
        self.last_check_ms = Some(now);
        let targets = target_weights(&select_candidates(market, &self.params), &self.params);
        if targets.is_empty() {
            return false;
        }
        let eq = self.equity();
        let mut n = 0usize;
        for t in targets {
            if self.add_leg(&t, eq, market, now) {
                n += 1;
            }
        }
        if n > 0 {
            self.roll_daily(now);
            self.log(now, format!("开仓：{} 腿，总名义 ${:.0}", n, eq));
        }
        n > 0
    }

    fn add_leg(&mut self, t: &Target, equity: f64, market: &Market, now: i64) -> bool {
        let Some(q) = market.quote(&t.coin, &t.spot) else {
            return false;
        };
        let notional = equity * t.weight;
        if notional <= 0.0 || q.spot <= 0.0 || q.perp <= 0.0 {
            return false;
        }
        let entry_basis_bp = (q.perp - q.spot) / q.spot * 10_000.0;
        if entry_basis_bp < self.params.min_entry_basis_bp {
            return false;
        }
        let fee = notional * ROUND_TRIP_FEE_BP / 10_000.0;
        self.fees_paid_total += fee;
        self.legs.push(Leg {
            coin: t.coin.clone(),
            spot: t.spot.clone(),
            weight: t.weight,
            notional,
            spot_qty: notional / q.spot,
            perp_qty_abs: notional / q.perp,
            spot_entry: q.spot,
            perp_entry: q.perp,
            entry_funding_ann: q.funding_ann,
            funding_accrued: 0.0,
            basis_pnl: 0.0,
            fee_paid: fee,
            last_accrual_ms: now,
            spot_px: q.spot,
            perp_px: q.perp,
            funding_ann: q.funding_ann,
            basis_bp: (q.perp - q.spot) / q.spot * 10_000.0,
            basis_move_bp: 0.0,
            leg_pnl: -fee,
        });
        true
    }

    /// 把一条腿调整到新权重：只对**变动的名义**收往返费。
    fn resize_leg(&mut self, i: usize, t: &Target, equity: f64, market: &Market, now: i64) {
        let Some(q) = market.quote(&t.coin, &t.spot) else {
            return;
        };
        if !(q.spot > 0.0 && q.perp > 0.0) {
            return;
        }
        let new_notional = equity * t.weight;
        let old = self.legs[i].notional;
        let delta = (new_notional - old).abs();
        let mut fee = 0.0;
        if delta > 1e-9 {
            fee = delta * ROUND_TRIP_FEE_BP / 10_000.0;
        }
        self.fees_paid_total += fee;
        let leg = &mut self.legs[i];
        leg.weight = t.weight;
        leg.spot = t.spot.clone();
        leg.notional = new_notional;
        leg.spot_qty = new_notional / q.spot;
        leg.perp_qty_abs = new_notional / q.perp;
        leg.spot_entry = q.spot;
        leg.perp_entry = q.perp;
        leg.spot_px = q.spot;
        leg.perp_px = q.perp;
        leg.funding_ann = q.funding_ann;
        leg.basis_bp = (q.perp - q.spot) / q.spot * 10_000.0;
        leg.basis_move_bp = 0.0;
        leg.last_accrual_ms = now;
        // 调整前先把已经发生的基差盈亏落袋：入场价要换，基差从零重新起算。
        let realized_basis = leg.basis_pnl;
        self.closed_pnl += realized_basis;
        leg.fee_paid += fee;
        leg.basis_pnl = 0.0;
        leg.leg_pnl = leg.funding_accrued - leg.fee_paid;
    }

    /// 先执行基差止盈，再评估资金费轮动。返回是否真的动了仓位。
    pub fn maybe_rebalance(&mut self, market: &Market, now: i64) -> bool {
        self.last_check_ms = Some(now);
        if self.legs.is_empty() {
            return self.reopen(market, now);
        }

        let cur_f = self
            .legs
            .iter()
            .map(|l| {
                let f = market
                    .quote(&l.coin, &l.spot)
                    .map(|q| q.funding_ann)
                    .unwrap_or(l.funding_ann);
                l.weight * f
            })
            .sum::<f64>();

        // 买现货 + 空永续在 perp/spot 相对入场下降时获利。止盈不受 48 小时
        // 资金费轮动锁限制；往返费已在开仓时预扣，所以这里不重复收费。
        if self.params.basis_take_profit_bp > 0.0 {
            let triggered: Vec<(String, f64, f64)> = self
                .legs
                .iter()
                .filter(|l| l.basis_move_bp <= -self.params.basis_take_profit_bp)
                .map(|l| (l.coin.clone(), l.weight, l.basis_move_bp))
                .collect();
            if !triggered.is_empty() {
                let mut turn = triggered.iter().map(|(_, w, _)| *w).sum::<f64>() / 2.0;
                let realized = self
                    .legs
                    .iter()
                    .filter(|l| triggered.iter().any(|(c, _, _)| c == &l.coin))
                    .map(|l| l.leg_pnl)
                    .sum::<f64>();
                self.closed_pnl += realized;
                self.legs.retain(|l| !triggered.iter().any(|(c, _, _)| c == &l.coin));
                let removed: Vec<String> = triggered.iter().map(|(c, _, _)| c.clone()).collect();
                for c in &removed {
                    if !self.tp_cooldown_coins.contains(c) {
                        self.tp_cooldown_coins.push(c.clone());
                    }
                }
                // 止盈释放的闲置权重立即补入其他合规候选币（同周期不买回刚止盈的币），避免资金长期空转。
                let fee_before = self.fees_paid_total;
                let refill_equity = self.equity();
                let mut available: Vec<Candidate> = select_candidates(market, &self.params)
                    .into_iter()
                    .filter(|c| {
                        !self.legs.iter().any(|l| l.coin == c.coin)
                            && !self.tp_cooldown_coins.contains(&c.coin)
                    })
                    .collect();
                let mut added: Vec<String> = Vec::new();
                for (_, freed_w, _) in &triggered {
                    if available.is_empty() {
                        break;
                    }
                    let cand = available.remove(0);
                    let t = Target {
                        coin: cand.coin.clone(),
                        spot: cand.spot.clone(),
                        weight: (*freed_w).min(self.params.max_leg_weight),
                        funding_ann: cand.funding_ann,
                    };
                    if self.add_leg(&t, refill_equity, market, now) {
                        added.push(t.coin);
                        turn += t.weight / 2.0;
                    }
                }
                let refill_fee = self.fees_paid_total - fee_before;
                let funding_after: f64 = self.legs.iter().map(|l| l.weight * l.funding_ann).sum();
                let basis_moves = triggered.iter().map(|(c, _, b)| (c.clone(), *b)).collect();
                let target = self
                    .legs
                    .iter()
                    .map(|l| (l.coin.clone(), l.weight, l.funding_ann))
                    .collect();
                self.rebalances.push(RebalanceRecord {
                    ts: now,
                    turnover: turn,
                    funding_before: cur_f,
                    funding_after,
                    threshold: self.params.basis_take_profit_bp,
                    fee_usd: refill_fee,
                    added: added.clone(),
                    removed: removed.clone(),
                    resized: Vec::new(),
                    target,
                    reason: "basis_take_profit".into(),
                    basis_moves,
                });
                self.roll_daily(now);
                let today = ymd_from_ms(now);
                if let Some(d) = self.daily.last_mut() {
                    if d.date == today {
                        d.rebalances += 1;
                    }
                }
                if self.rebalances.len() > 100 {
                    let drop = self.rebalances.len() - 100;
                    self.rebalances.drain(0..drop);
                }
                let tail = if added.is_empty() {
                    "暂无其他合规标的，仓位暂转现金等待".to_string()
                } else {
                    format!("已立即补仓：{}", added.join(" / "))
                };
                self.log(
                    now,
                    format!(
                        "基差止盈：{}，实现盈亏 ${:.2}；{}",
                        removed.join(" / "),
                        realized,
                        tail
                    ),
                );
                return true;
            }
        }

        // 最短持有期只看上次「资金费轮动」，单腿基差止盈不再重置 48h 轮动锁。
        if let Some(last) = self
            .rebalances
            .iter()
            .rev()
            .find(|r| r.reason != "basis_take_profit")
            .map(|r| r.ts)
        {
            let hold_h = ((now - last).max(0) as f64) / 3_600_000.0;
            if hold_h < self.params.min_hold_hours {
                self.sync_funding(market);
                return false;
            }
        }
        let targets = target_weights(&select_candidates(market, &self.params), &self.params);
        if targets.is_empty() {
            self.sync_funding(market);
            return false;
        }

        let cur: Vec<(String, f64)> = self.legs.iter().map(|l| (l.coin.clone(), l.weight)).collect();
        let turn = turnover(&cur, &targets);
        if turn < 1e-9 {
            self.sync_funding(market);
            return false;
        }

        let tgt_f: f64 = targets.iter().map(|t| t.weight * t.funding_ann).sum();
        let improve = tgt_f - cur_f;
        let threshold = rebalance_threshold_ann_pct(turn, &self.params);

        if improve <= threshold {
            self.sync_funding(market);
            return false;
        }

        let equity = self.equity();
        let target_coins: Vec<String> = targets.iter().map(|t| t.coin.clone()).collect();
        let removed: Vec<String> = self
            .legs
            .iter()
            .filter(|l| !target_coins.contains(&l.coin))
            .map(|l| l.coin.clone())
            .collect();
        self.closed_pnl += self
            .legs
            .iter()
            .filter(|l| !target_coins.contains(&l.coin))
            .map(|l| l.leg_pnl)
            .sum::<f64>();
        self.legs.retain(|l| target_coins.contains(&l.coin));
        for c in &removed {
            self.log(now, format!("移出 {c}（资金费不再靠前）"));
        }

        let fee_before = self.fees_paid_total;
        let mut added: Vec<String> = Vec::new();
        let mut resized: Vec<(String, f64, f64)> = Vec::new();
        for t in targets.iter() {
            if let Some(i) = self.legs.iter().position(|l| l.coin == t.coin) {
                let old_w = self.legs[i].weight;
                self.resize_leg(i, t, equity, market, now);
                resized.push((t.coin.clone(), old_w, t.weight));
            } else if self.add_leg(t, equity, market, now) {
                added.push(t.coin.clone());
                self.log(now, format!("加入 {}（{:.2}%/年）", t.coin, t.funding_ann));
            }
        }
        let fee_usd = self.fees_paid_total - fee_before;
        self.tp_cooldown_coins.clear();

        self.rebalances.push(RebalanceRecord {
            ts: now,
            turnover: turn,
            funding_before: cur_f,
            funding_after: tgt_f,
            threshold,
            fee_usd,
            added,
            removed: removed.clone(),
            resized,
            target: targets
                .iter()
                .map(|t| (t.coin.clone(), t.weight, t.funding_ann))
                .collect(),
            reason: "funding_rotation".into(),
            basis_moves: Vec::new(),
        });
        self.roll_daily(now);
        let today = ymd_from_ms(now);
        if let Some(d) = self.daily.last_mut() {
            if d.date == today {
                d.rebalances += 1;
            }
        }
        if self.rebalances.len() > 100 {
            let drop = self.rebalances.len() - 100;
            self.rebalances.drain(0..drop);
        }

        self.log(
            now,
            format!(
                "资金费轮动：换手 {:.0}%，加权资金费 {:.2}% → {:.2}%/年（门槛 {:.2}%），手续费累计 ${:.2}",
                turn * 100.0,
                cur_f,
                tgt_f,
                threshold,
                self.fees_paid_total
            ),
        );
        true
    }

    fn sync_funding(&mut self, market: &Market) {
        for leg in self.legs.iter_mut() {
            if let Some(q) = market.quote(&leg.coin, &leg.spot) {
                leg.funding_ann = q.funding_ann;
            }
        }
    }

    /// 推进一步：计提资金费、盯市、记录权益。
    pub fn step(&mut self, market: &Market, now: i64) {
        self.updated = Some(now);
        for leg in self.legs.iter_mut() {
            let Some(q) = market.quote(&leg.coin, &leg.spot) else {
                continue;
            };
            if !(q.spot > 0.0 && q.perp > 0.0 && q.spot.is_finite() && q.perp.is_finite()) {
                continue;
            }
            if let Some(history) = market.funding_history.get(&leg.coin) {
                // 只计已结算事件；不得用当前资金费率回填过去几小时。
                let delta = history.iter()
                    .filter(|(t, _)| *t > leg.last_accrual_ms && *t <= now)
                    .map(|(_, rate)| leg.notional * rate)
                    .sum::<f64>();
                leg.funding_accrued += delta;
                self.funding_total += delta;
                leg.last_accrual_ms = now;
            }
            leg.spot_px = q.spot;
            leg.perp_px = q.perp;
            leg.funding_ann = q.funding_ann;
            leg.basis_bp = (q.perp - q.spot) / q.spot * 10_000.0;
            leg.basis_move_bp =
                ((q.perp / q.spot) / (leg.perp_entry / leg.spot_entry) - 1.0) * 10_000.0;
            leg.basis_pnl =
                (q.spot - leg.spot_entry) * leg.spot_qty + (leg.perp_entry - q.perp) * leg.perp_qty_abs;
            leg.leg_pnl = leg.basis_pnl + leg.funding_accrued - leg.fee_paid;
        }

        self.roll_daily(now);

        let should = match self.last_history_ms {
            None => true,
            Some(t) => now - t >= 3_600_000,
        };
        if should {
            self.last_history_ms = Some(now);
            let eq = self.equity();
            self.history.push((now, eq));
            if self.history.len() > 24 * 400 {
                let drop = self.history.len() - 24 * 400;
                self.history.drain(0..drop);
            }
        }
    }

    /// 当前累计基差盈亏（含已落袋部分）。equity = start + funding + basis − fees。
    pub fn basis_total(&self) -> f64 {
        self.equity() - self.start_equity - self.funding_total + self.fees_paid_total
    }

    /// 日切：按 UTC 自然日维护增量统计。每次 step 都会调，幂等。
    fn roll_daily(&mut self, now: i64) {
        if self.start_equity <= 0.0 {
            return;
        }
        let date = ymd_from_ms(now);
        let eq = self.equity();
        let f = self.funding_total;
        let b = self.basis_total();
        let fee = self.fees_paid_total;

        let last_is_today = self.daily.last().map(|d| d.date == date).unwrap_or(false);
        if !last_is_today {
            self.daily.push(DailyStat {
                date,
                equity_start: eq,
                equity_end: eq,
                funding: 0.0,
                basis: 0.0,
                fees: 0.0,
                rebalances: 0,
                base_funding: f,
                base_basis: b,
                base_fees: fee,
            });
            if self.daily.len() > 400 {
                let drop = self.daily.len() - 400;
                self.daily.drain(0..drop);
            }
            return;
        }
        let d = self.daily.last_mut().expect("just checked");
        d.equity_end = eq;
        d.funding = f - d.base_funding;
        d.basis = b - d.base_basis;
        d.fees = fee - d.base_fees;
    }

    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        for leg in &self.legs {
            if leg.funding_ann < 0.0 {
                out.push(format!(
                    "{} 资金费转负（{:.2}%/年）——这条腿在付钱",
                    leg.coin, leg.funding_ann
                ));
            } else if leg.funding_ann < self.params.min_funding_ann_pct {
                out.push(format!(
                    "{} 资金费 {:.2}%/年，已低于门槛 {:.2}%",
                    leg.coin, leg.funding_ann, self.params.min_funding_ann_pct
                ));
            }
            if leg.basis_bp < -30.0 {
                out.push(format!("{} 永续贴水 {:.0}bp——基差不利", leg.coin, leg.basis_bp));
            }
        }
        out
    }
}


mod market;
pub use market::{fetch_market, fetch_market_with_funding, Market};
#[cfg(test)]
use market::{parse_market, perp_of_spot_token, Latch};

#[cfg(test)]
mod tests;
