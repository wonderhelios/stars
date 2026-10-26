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
use serde_json::{json, Value};
use std::path::Path;

const HL_INFO: &str = "https://api.hyperliquid.xyz/info";

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
}

impl Default for Params {
    fn default() -> Self {
        Params {
            top_k: 5,
            // 10.95%/年是 HL 常见档位；1% 的币基本在空转，不值得占一腿。
            min_funding_ann_pct: 3.0,
            // $1M/24h 是能容纳 $5k 级别下单的下限。
            min_spot_volume_usd: 1_000_000.0,
            max_leg_weight: 0.40,
            // 30 天回本：一次全换 10.88bp，摊到 30 天约 1.32%/年。
            payback_days: 30.0,
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
}

/// 目标持仓：币、现货对、目标权重。
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub coin: String,
    pub spot: String,
    pub weight: f64,
    pub funding_ann: f64,
}

/// 从全市场挑候选：现货够厚 + 资金费过门槛，按资金费从高到低取前 K。
pub fn select_candidates(market: &Market, params: &Params) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = market
        .latched
        .iter()
        .filter(|l| l.spot_volume >= params.min_spot_volume_usd)
        .filter(|l| l.funding_ann > params.min_funding_ann_pct)
        .map(|l| Candidate {
            coin: l.coin.clone(),
            spot: l.spot.clone(),
            funding_ann: l.funding_ann,
            spot_volume: l.spot_volume,
        })
        .collect();
    out.sort_by(|a, b| {
        b.funding_ann
            .partial_cmp(&a.funding_ann)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.coin.cmp(&b.coin))
    });
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
    /// 触发换仓所需的年化门槛（%）。
    pub threshold: f64,
    /// 本次换仓付出的手续费（USD）。
    pub fee_usd: f64,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// (coin, 旧权重, 新权重)
    pub resized: Vec<(String, f64, f64)>,
    /// 换仓后的目标篮子：(coin, 权重, 资金费%/年)，按资金费降序。
    pub target: Vec<(String, f64, f64)>,
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

    /// 当前持仓的加权年化资金费（%），用于和候选篮子比较。
    pub fn current_weighted_funding(&self) -> f64 {
        self.legs.iter().map(|l| l.weight * l.funding_ann).sum()
    }

    fn log(&mut self, now: i64, msg: String) {
        self.log.push(CarryLog { ts: now, msg });
        if self.log.len() > 300 {
            let drop = self.log.len() - 300;
            self.log.drain(0..drop);
        }
    }

    /// 按预算建仓（首次开仓 / 重置）。
    pub fn open(&mut self, budget_usd: f64, market: &Market, now: i64) {
        self.legs.clear();
        self.closed_pnl = 0.0;
        self.history.clear();
        self.fees_paid_total = 0.0;
        self.funding_total = 0.0;
        self.rebalances.clear();
        self.daily.clear();
        self.budget_usd = budget_usd;
        self.start_equity = budget_usd;
        self.last_history_ms = Some(now);
        self.last_check_ms = Some(now);
        // 先立当日的基线，再建仓——否则开仓手续费会落在基线之前，
        // 当日统计里就看不见这笔成本。
        self.roll_daily(now);
        let targets = target_weights(&select_candidates(market, &self.params), &self.params);
        if targets.is_empty() {
            self.log(now, "没有满足条件的标的（现货成交额 / 资金费门槛）".into());
            return;
        }
        let eq = self.equity();
        let mut n = 0usize;
        for t in targets {
            if self.add_leg(&t, eq, market, now) {
                n += 1;
            }
        }
        self.log(now, format!("开仓：{} 腿，总名义 ${:.0}", n, eq));
    }

    fn add_leg(&mut self, t: &Target, equity: f64, market: &Market, now: i64) -> bool {
        let Some(q) = market.quote(&t.coin, &t.spot) else {
            return false;
        };
        let notional = equity * t.weight;
        if notional <= 0.0 || q.spot <= 0.0 || q.perp <= 0.0 {
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
        leg.last_accrual_ms = now;
        // 调整前先把已经发生的基差盈亏落袋：入场价要换，基差从零重新起算。
        // 不落袋的话，equity 会凭空跳一下（旧基差从腿里消失又没进 closed_pnl）。
        let realized_basis = leg.basis_pnl;
        self.closed_pnl += realized_basis;
        leg.fee_paid += fee;
        leg.basis_pnl = 0.0;
        leg.leg_pnl = leg.funding_accrued - leg.fee_paid;
    }

    /// 评估并在满足闸门时换仓。返回是否真的动了仓位。
    pub fn maybe_rebalance(&mut self, market: &Market, now: i64) -> bool {
        if self.legs.is_empty() {
            return false;
        }
        self.last_check_ms = Some(now);
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
        // 移仓后不再保留旧腿，因此把该腿全部已计入的损益结转到已实现盈亏。
        // 开仓时预扣的手续费已包含在 leg_pnl 中；这里不重复收，只避免删腿时抹账。
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
                "换仓：换手 {:.0}%，加权资金费 {:.2}% → {:.2}%/年（门槛 {:.2}%），手续费累计 ${:.2}",
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
            let dt_hours = ((now - leg.last_accrual_ms).max(0) as f64) / 3_600_000.0;
            if dt_hours > 0.0 && q.funding_hourly.is_finite() {
                let delta = leg.notional * q.funding_hourly * dt_hours;
                leg.funding_accrued += delta;
                self.funding_total += delta;
                leg.last_accrual_ms = now;
            }
            leg.spot_px = q.spot;
            leg.perp_px = q.perp;
            leg.funding_ann = q.funding_ann;
            leg.basis_bp = (q.perp - q.spot) / q.spot * 10_000.0;
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

// ---------------------------------------------------------------------------
// 行情
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Quote {
    pub spot: f64,
    pub perp: f64,
    pub funding_hourly: f64,
    pub funding_ann: f64,
}

/// 一个"两边都有盘、且能配对"的标的快照。
#[derive(Clone, Debug)]
pub struct Latch {
    pub coin: String,
    pub spot: String,
    pub funding_ann: f64,
    pub spot_volume: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Market {
    pub perp_mid: std::collections::HashMap<String, f64>,
    pub spot_mid: std::collections::HashMap<String, f64>,
    pub funding: std::collections::HashMap<String, f64>,
    /// 现货 24h 成交额（按现货对名）。
    pub spot_volume: std::collections::HashMap<String, f64>,
    /// 已配对的标的（永续名 + 现货对），带成交额与资金费。
    pub latched: Vec<Latch>,
}

impl Market {
    pub fn quote(&self, coin: &str, spot: &str) -> Option<Quote> {
        let perp = *self.perp_mid.get(coin)?;
        let sp = *self.spot_mid.get(spot)?;
        let fh = *self.funding.get(coin).unwrap_or(&0.0);
        Some(Quote {
            spot: sp,
            perp,
            funding_hourly: fh,
            funding_ann: fh * 24.0 * 365.0 * 100.0,
        })
    }
}

/// 现货 token 名 → 永续币名。HL 现货用桥接符（UBTC/UETH/USOL/XMR1/USDT0）。
pub fn perp_of_spot_token(token: &str, perps: &std::collections::HashSet<String>) -> Option<String> {
    if perps.contains(token) {
        return Some(token.to_string());
    }
    if let Some(rest) = token.strip_prefix('U') {
        if !rest.is_empty() && perps.contains(rest) {
            return Some(rest.to_string());
        }
    }
    let trimmed = token.trim_end_matches(|c: char| c.is_ascii_digit());
    if !trimmed.is_empty() && perps.contains(trimmed) {
        return Some(trimmed.to_string());
    }
    None
}

async fn post(http: &reqwest::Client, payload: Value) -> Result<Value> {
    let resp = http
        .post(HL_INFO)
        .json(&payload)
        .send()
        .await
        .context("HL info 请求失败")?;
    let v: Value = resp.json().await.context("HL info 解析失败")?;
    Ok(v)
}

/// 拉一次全市场快照：3 个公开请求。
pub async fn fetch_market(http: &reqwest::Client) -> Result<Market> {
    let mids = post(http, json!({"type": "allMids"})).await?;
    let ctxs = post(http, json!({"type": "metaAndAssetCtxs"})).await?;
    let spot = post(http, json!({"type": "spotMetaAndAssetCtxs"})).await?;
    Ok(parse_market(&mids, &ctxs, &spot))
}

/// 纯解析：三个接口的原始 JSON → Market。抽出来便于用真实报文做单测。
pub fn parse_market(mids: &Value, ctxs: &Value, spot: &Value) -> Market {
    let mut m = Market::default();

    if let Some(obj) = mids.as_object() {
        for (k, v) in obj {
            let Some(px) = v.as_str().and_then(|s| s.parse::<f64>().ok()) else {
                continue;
            };
            if !(px > 0.0) {
                continue;
            }
            if k.starts_with('@') || k.contains('/') {
                m.spot_mid.insert(k.clone(), px);
            } else {
                m.perp_mid.insert(k.clone(), px);
            }
        }
    }

    let mut perp_set = std::collections::HashSet::new();
    if let Some(arr) = ctxs.as_array() {
        if arr.len() > 1 {
            if let (Some(meta), Some(list)) = (arr[0].get("universe"), arr[1].as_array()) {
                let empty = vec![];
                let uni = meta.as_array().unwrap_or(&empty);
                for (u, c) in uni.iter().zip(list.iter()) {
                    let Some(coin) = u.get("name").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    perp_set.insert(coin.to_string());
                    let f = c
                        .get("funding")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    m.funding.insert(coin.to_string(), f);
                }
            }
        }
    }

    // 现货：meta 给 token 名与 pair→token 映射，ctxs 给 pair 的成交额。
    let (smeta, sctxs) = match spot.as_array() {
        Some(a) if a.len() > 1 => (a[0].clone(), a[1].as_array().cloned().unwrap_or_default()),
        _ => (Value::Null, Vec::new()),
    };
    let tokens: Vec<String> = smeta
        .get("tokens")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .map(|t| {
                    t.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    let mut vol_by_pair: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for c in sctxs.iter() {
        let Some(pair) = c.get("coin").and_then(|v| v.as_str()) else {
            continue;
        };
        let v = c
            .get("dayNtlVlm")
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        vol_by_pair.insert(pair.to_string(), v);
    }

    if let Some(uni) = smeta.get("universe").and_then(|v| v.as_array()) {
        for u in uni {
            let Some(name) = u.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(idx) = u.get("tokens").and_then(|v| v.as_array()) else {
                continue;
            };
            let Some(base_i) = idx.first().and_then(|v| v.as_u64()) else {
                continue;
            };
            let Some(base) = tokens.get(base_i as usize) else {
                continue;
            };
            let Some(coin) = perp_of_spot_token(base, &perp_set) else {
                continue;
            };
            let vol = *vol_by_pair.get(name).unwrap_or(&0.0);
            m.spot_volume.insert(name.to_string(), vol);
            let fh = *m.funding.get(&coin).unwrap_or(&0.0);
            if m.spot_mid.contains_key(name) && m.perp_mid.contains_key(&coin) {
                m.latched.push(Latch {
                    coin,
                    spot: name.to_string(),
                    funding_ann: fh * 24.0 * 365.0 * 100.0,
                    spot_volume: vol,
                });
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (coin, spot, funding_hourly, spot_vol, px)
    fn market(rows: &[(&str, &str, f64, f64, f64)]) -> Market {
        let mut m = Market::default();
        for (coin, spot, fh, vol, px) in rows {
            m.perp_mid.insert(coin.to_string(), *px);
            m.spot_mid.insert(spot.to_string(), *px);
            m.funding.insert(coin.to_string(), *fh);
            m.spot_volume.insert(spot.to_string(), *vol);
            m.latched.push(Latch {
                coin: coin.to_string(),
                spot: spot.to_string(),
                funding_ann: fh * 24.0 * 365.0 * 100.0,
                spot_volume: *vol,
            });
        }
        m
    }

    #[test]
    fn selects_by_funding_and_filters() {
        let p = Params::default();
        let m = market(&[
            ("AAA", "@1", 0.000_05, 5e6, 10.0),   // 43.8%/年
            ("BBB", "@2", 0.000_0125, 5e6, 10.0), // 10.95%/年
            ("CCC", "@3", 0.000_0001, 5e6, 10.0), // 0.09%/年 -> 低于门槛
            ("DDD", "@4", 0.000_05, 1e4, 10.0),   // 现货太薄
        ]);
        let c = select_candidates(&m, &p);
        let names: Vec<&str> = c.iter().map(|x| x.coin.as_str()).collect();
        assert_eq!(names, vec!["AAA", "BBB"], "只留过门槛且够厚的，按资金费降序");

        let t = target_weights(&c, &p);
        assert_eq!(t.len(), 2);
        // AAA 原始权重 0.837 被 0.40 上限压住；余量留现金，不回补给 BBB
        assert!((t[0].weight - 0.40).abs() < 1e-9, "高资金费腿应被压到上限");
        assert!(t[1].weight > 0.0 && t[1].weight < 0.40);
        let sum: f64 = t.iter().map(|x| x.weight).sum();
        assert!(sum <= 1.0 + 1e-9, "权重和不得超过 1（余额是现金）");
        assert!(sum < 1.0 - 1e-6, "上限生效时应留出现金");
    }

    #[test]
    fn weights_respect_cap() {
        let p = Params {
            top_k: 3,
            max_leg_weight: 0.5,
            ..Params::default()
        };
        let m = market(&[
            ("AAA", "@1", 0.000_2, 5e6, 10.0),
            ("BBB", "@2", 0.000_01, 5e6, 10.0),
        ]);
        let t = target_weights(&select_candidates(&m, &p), &p);
        for x in &t {
            assert!(x.weight <= 0.5 + 1e-9, "单腿不得超过上限");
        }
        let sum: f64 = t.iter().map(|x| x.weight).sum();
        assert!(sum <= 1.0 + 1e-9, "权重和不得超过 1");
        assert!(sum > 0.0);
    }

    #[test]
    fn rebalance_gate_blocks_small_improvements() {
        let p = Params::default();
        let mut st = HlCarryState {
            params: p.clone(),
            ..HlCarryState::default()
        };
        let m1 = market(&[
            ("AAA", "@1", 0.000_0125, 5e6, 10.0),
            ("BBB", "@2", 0.000_0120, 5e6, 10.0),
        ]);
        st.open(1000.0, &m1, 1);
        assert_eq!(st.legs.len(), 2);
        let m2 = market(&[
            ("AAA", "@1", 0.000_0128, 5e6, 10.0),
            ("BBB", "@2", 0.000_0122, 5e6, 10.0),
        ]);
        assert!(!st.maybe_rebalance(&m2, 2), "改进低于门槛时不应换仓");
    }

    #[test]
    fn rebalance_switches_when_clearly_better() {
        let p = Params::default();
        let mut st = HlCarryState {
            params: p.clone(),
            ..HlCarryState::default()
        };
        let m1 = market(&[
            ("AAA", "@1", 0.000_0125, 5e6, 10.0),
            ("BBB", "@2", 0.000_0125, 5e6, 10.0),
        ]);
        st.open(1000.0, &m1, 1);
        let fee_before = st.fees_paid_total;
        let m2 = market(&[
            ("AAA", "@1", 0.000_0125, 5e6, 10.0),
            ("BBB", "@2", 0.000_0125, 5e6, 10.0),
            ("ZZZ", "@9", 0.000_4, 5e6, 10.0),
        ]);
        assert!(st.maybe_rebalance(&m2, 2), "改进远超门槛时应换仓");
        assert!(
            st.legs.iter().any(|l| l.coin == "ZZZ"),
            "应纳入高资金费的新币"
        );
        assert!(st.fees_paid_total > fee_before, "换仓必须记手续费");
    }

    #[test]
    fn turnover_and_threshold_math() {
        let p = Params {
            payback_days: 30.0,
            ..Params::default()
        };
        let th = rebalance_threshold_ann_pct(1.0, &p);
        assert!((th - 10.88 / 100.0 * 365.0 / 30.0).abs() < 1e-9);
        assert_eq!(rebalance_threshold_ann_pct(0.0, &p), 0.0);
    }

    #[test]
    fn open_then_accrue_funding() {
        let mut st = HlCarryState {
            params: Params {
                max_leg_weight: 1.0,
                ..Params::default()
            },
            ..HlCarryState::default()
        };
        let m = market(&[("BTC", "@142", 0.000_0125, 5e6, 80_000.0)]);
        st.open(1000.0, &m, 1_000_000);
        assert_eq!(st.legs.len(), 1);
        let fee = st.legs[0].fee_paid;
        assert!((fee - 1000.0 * ROUND_TRIP_FEE_BP / 10_000.0).abs() < 1e-9);
        st.step(&m, 1_000_000 + 10 * 3_600_000);
        let expect = 1000.0 * 0.000_0125 * 10.0;
        assert!((st.legs[0].funding_accrued - expect).abs() < 1e-9);
        assert!(st.legs[0].basis_pnl.abs() < 1e-6);
    }

    #[test]
    fn basis_pnl_offsets_price_move() {
        let mut st = HlCarryState::default();
        let m0 = market(&[("BTC", "@142", 0.000_0125, 5e6, 80_000.0)]);
        st.open(1000.0, &m0, 1_000_000);
        assert_eq!(st.legs.len(), 1, "正资金费标的应能建仓");
        let m1 = market(&[("BTC", "@142", 0.000_0125, 5e6, 88_000.0)]);
        st.step(&m1, 1_000_000 + 3_600_000);
        assert!(st.legs[0].basis_pnl.abs() < 1e-6, "delta 中性后基差盈亏应≈0");
    }

    #[test]
    fn ymd_formats_utc() {
        assert_eq!(ymd_from_ms(0), "1970-01-01");
        // 2026-10-27T00:00:00Z
        assert_eq!(ymd_from_ms(1_793_059_200_000), "2026-10-27");
        assert_eq!(ymd_from_ms(1_793_059_200_000 + 86_400_000 - 1), "2026-10-27");
        assert_eq!(ymd_from_ms(1_793_059_200_000 + 86_400_000), "2026-10-28");
    }

    #[test]
    fn daily_aggregates_across_days() {
        let mut st = HlCarryState {
            params: Params {
                max_leg_weight: 1.0,
                ..Params::default()
            },
            ..HlCarryState::default()
        };
        let m = market(&[("BTC", "@142", 0.000_0125, 5e6, 80_000.0)]);
        let t0 = 1_793_059_200_000i64; // 2026-10-27T00:00Z
        st.open(1000.0, &m, t0);
        let fee0 = st.fees_paid_total;
        assert!(fee0 > 0.0);

        // 同一天内计提 10 小时
        st.step(&m, t0 + 10 * 3_600_000);
        let (d0_date, d0_fees, d0_funding) = {
            let d0 = st.daily.last().unwrap();
            (d0.date.clone(), d0.fees, d0.funding)
        };
        assert_eq!(d0_date, "2026-10-27");
        assert!((d0_fees - fee0).abs() < 1e-9, "手续费应计入当日");
        assert!(d0_funding > 0.0, "资金费应计入当日");

        // 跨到第二天
        st.step(&m, t0 + 30 * 3_600_000);
        assert_eq!(st.daily.len(), 2, "跨日应新开一条");
        let d1 = st.daily.last().unwrap();
        assert_eq!(d1.date, "2026-10-28");
        // 新一天的增量从零开始
        assert!(d1.funding < d0_funding, "次日资金费只算次日增量");
    }

    #[test]
    fn equity_is_continuous_across_rebalance() {
        let mut st = HlCarryState::default();
        let m1 = market(&[
            ("AAA", "@1", 0.000_2, 5e6, 100.0),
            ("BBB", "@2", 0.000_2, 5e6, 100.0),
        ]);
        st.open(1000.0, &m1, 1);
        // 价格变动制造基差盈亏
        let m2 = market(&[
            ("AAA", "@1", 0.000_2, 5e6, 110.0),
            ("BBB", "@2", 0.000_2, 5e6, 110.0),
        ]);
        st.step(&m2, 2);
        let eq_before = st.equity();
        // 出现强得多的币，触发换仓（会 resize 旧腿）
        let m3 = market(&[
            ("AAA", "@1", 0.000_2, 5e6, 110.0),
            ("BBB", "@2", 0.000_2, 5e6, 110.0),
            ("ZZZ", "@9", 0.002, 5e6, 100.0),
        ]);
        let moved = st.maybe_rebalance(&m3, 3);
        assert!(moved);
        // 换仓只应扣手续费，不应让权益凭空跳变
        let eq_after = st.equity();
        assert!(
            (eq_after - eq_before).abs() < eq_before * 0.02,
            "换仓后权益跳变过大：{eq_before} -> {eq_after}"
        );
    }

    #[test]
    fn rebalance_record_explains_the_move() {
        let mut st = HlCarryState::default();
        let m1 = market(&[
            ("AAA", "@1", 0.000_0125, 5e6, 100.0),
            ("BBB", "@2", 0.000_0125, 5e6, 100.0),
        ]);
        st.open(1000.0, &m1, 1);
        let m2 = market(&[
            ("AAA", "@1", 0.000_0125, 5e6, 100.0),
            ("BBB", "@2", 0.000_0125, 5e6, 100.0),
            ("ZZZ", "@9", 0.000_4, 5e6, 100.0),
        ]);
        assert!(st.maybe_rebalance(&m2, 2));
        let r = st.rebalances.last().expect("应记录一次换仓");
        assert!(r.turnover > 0.0);
        assert!(r.funding_after > r.funding_before, "换仓后资金费必须更高");
        assert!(r.fee_usd > 0.0, "换仓必须记手续费");
        assert!(r.added.contains(&"ZZZ".to_string()), "新币应出现在 added");
        assert!(
            r.target.iter().any(|(c, _, _)| c == "ZZZ"),
            "目标篮子应包含新币"
        );
        assert_eq!(st.daily.last().unwrap().rebalances, 1, "当日换仓次数应 +1");
    }

    #[test]
    fn removed_leg_pnl_is_carried_into_closed_pnl() {
        let mut st = HlCarryState {
            params: Params {
                top_k: 1,
                max_leg_weight: 1.0,
                ..Params::default()
            },
            ..HlCarryState::default()
        };
        let t0 = 1_793_059_200_000i64;
        let m1 = market(&[("AAA", "@1", 0.000_0125, 5e6, 100.0)]);
        st.open(1000.0, &m1, t0);
        st.step(&m1, t0 + 3_600_000);
        let equity_before = st.equity();
        let fees_before = st.fees_paid_total;

        let m2 = market(&[
            ("AAA", "@1", 0.000_0125, 5e6, 100.0),
            ("ZZZ", "@9", 0.000_4, 5e6, 100.0),
        ]);
        assert!(st.maybe_rebalance(&m2, t0 + 3_600_000));
        let fee_delta = st.fees_paid_total - fees_before;
        assert_eq!(st.legs.len(), 1);
        assert_eq!(st.legs[0].coin, "ZZZ");
        assert!(st.closed_pnl < 0.0, "旧腿的净损益（含已付手续费）要结转");
        assert!(
            (st.equity() - (equity_before - fee_delta)).abs() < 1e-7,
            "换仓权益应只减少新产生的手续费：{} -> {}，新费 {}",
            equity_before,
            st.equity(),
            fee_delta
        );
    }

    #[test]
    fn save_is_atomic_and_load_roundtrips() {
        let root = std::env::temp_dir().join(format!(
            "stars-hlcarry-{}-{}",
            std::process::id(),
            crate::live::now_ms_pub()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("state.json");
        let blocked = root.join("blocked.json");
        let state = HlCarryState {
            budget_usd: 1234.5,
            start_equity: 1234.5,
            ..HlCarryState::default()
        };

        state.save(&path).unwrap();
        let loaded = HlCarryState::load(&path);
        assert_eq!(loaded.budget_usd, 1234.5);
        assert_eq!(loaded.start_equity, 1234.5);

        std::fs::create_dir(&blocked).unwrap();
        assert!(state.save(&blocked).is_err(), "写入失败必须返回错误");
        assert!(blocked.is_dir(), "原目标应保持完整，不能被部分写入覆盖");
        assert!(!root.join(".blocked.json.tmp").exists(), "失败的临时文件应清理");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_real_api_payload() {
        let mids = serde_json::json!({
            "BTC": "79948.5", "ETH": "2313.45",
            "@142": "79871.5", "@151": "2311.25"
        });
        let ctxs = serde_json::json!([
            {"universe": [{"name": "BTC"}, {"name": "ETH"}]},
            [{"funding": "0.0000125"}, {"funding": "0.0000125"}]
        ]);
        let spot = serde_json::json!([
            {"tokens": [{"name": "USDC"}, {"name": "UBTC"}, {"name": "UETH"}],
             "universe": [
                {"name": "@142", "tokens": [1, 0]},
                {"name": "@151", "tokens": [2, 0]}
             ]},
            [{"coin": "@142", "dayNtlVlm": "19357783"},
             {"coin": "@151", "dayNtlVlm": "9321402"}]
        ]);
        let m = parse_market(&mids, &ctxs, &spot);
        assert_eq!(m.perp_mid.get("BTC"), Some(&79948.5));
        assert_eq!(m.spot_mid.get("@142"), Some(&79871.5));
        assert_eq!(m.spot_volume.get("@142"), Some(&19357783.0));
        let latched: Vec<&str> = m.latched.iter().map(|l| l.coin.as_str()).collect();
        assert!(latched.contains(&"BTC"), "UBTC 应映射到永续 BTC");
        assert!(latched.contains(&"ETH"), "UETH 应映射到永续 ETH");
        let q = m.quote("BTC", "@142").unwrap();
        assert!((q.funding_ann - 10.95).abs() < 0.01);
    }
}
