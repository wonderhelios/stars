//! Live trading on top of the same signal the paper engine uses.
//!
//! This is the only place in the project that can send real orders. It reuses
//! `trader::ranking` (identical to the paper engine) and `exchange::Exec`, so the
//! live book cannot drift from what was validated.

use crate::exchange::{order_price, round_size, Exec, MarketInfo};
use crate::hl::maintenance_margin_rate;
use crate::trader::{self, TradeConfig};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Build the market map from the cached universe so no meta request is needed.
pub fn markets_from_meta(universe: &[crate::hl::CoinMeta]) -> HashMap<String, MarketInfo> {
    universe
        .iter()
        .filter(|c| !c.is_delisted)
        .map(|c| {
            (
                c.name.clone(),
                MarketInfo {
                    sz_decimals: c.sz_decimals,
                    max_leverage: c.max_leverage,
                },
            )
        })
        .collect()
}

#[derive(Clone, Serialize, Deserialize)]
// 结构体级别的 default：反序列化时**从 LiveConfig::default() 开始**，文件里有什么就覆盖什么。
//
// 这比逐字段 #[serde(default)] 正确得多 —— 后者用的是「字段类型的默认值」
// （f64 → 0.0，bool → false），会让 leverage / margin_buffer 变成 0，
// 而这里会用真正想要的 3.0 / 0.9。字段缺失再也不会毁掉整个配置。
#[serde(default)]
pub struct LiveConfig {
    pub account: String,
    pub key_path: String,
    pub target_positions: usize,
    pub leverage: f64,
    pub margin_buffer: f64,
    pub slippage: f64,
    pub lookback: usize,
    pub top_frac: f64,
    pub min_vol_usd: f64,
    /// 最小下单额占净值的比例（0.01 = 1%）。0 表示只用绝对下限，行为与旧版一致。
    ///
    /// 为什么需要：`min_order_usd` 是绝对值，而仓位随账号缩放。同一个 $10 门槛，
    /// 对 $585 的账号是一个仓位的 17.7%（几乎不调仓），对 $2900 的账号是 3.8%
    /// （每天微调一堆）。这解释了不同账号的调仓频率差异。
    /// 设成比例之后，两边的**执行粒度**才可比。
    pub min_order_pct: f64,
    /// 错峰调仓档数：每个币每 N 天轮到一次。1 = 每日全量。
    ///
    /// 实测（相位平均 + 两个子区间均通过）：换手 29%→13%、成本 16.2%→7.3%、
    /// Sharpe 1.62→1.79，弱势区间 0.37→0.73。信号是 14 日动量，滞后几天对它
    /// 的影响远小于省下的成本。
    #[serde(default = "default_slices")]
    pub rebalance_slices: u32,
    /// 止盈幅度（相对调仓时的中间价）。0 = 不挂止盈单。
    #[serde(default = "default_tp")]
    pub take_profit_pct: f64,
    /// 因子权重：动量占比（0~1，量能占其余）。默认 0.5 等权。
    /// 0.7 = 动量七成/量能三成（复盘检验支持的方向，尚未前向验证）。
    #[serde(default = "default_momentum_weight")]
    pub momentum_weight: f64,
    /// allow sending real orders; off until the user arms it
    pub armed: bool,
    /// run one rebalance per day automatically
    pub auto_run: bool,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            account: String::new(),
            key_path: String::new(),
            target_positions: 8,
            leverage: 3.0,
            margin_buffer: 0.90,
            slippage: 0.005,
            lookback: 14,
            top_frac: 0.2,
            min_vol_usd: 5_000_000.0,
            min_order_pct: 0.0,
            take_profit_pct: 0.0,
            rebalance_slices: 3,
            momentum_weight: default_momentum_weight(),
            armed: false,
            auto_run: false,
        }
    }
}

fn default_slices() -> u32 {
    // 实测最佳：换手 29%→13%，Sharpe 1.62→1.79，两个子区间均改善。
    3
}

fn default_momentum_weight() -> f64 {
    // 默认等权：不改旧行为。复盘检验支持 0.7（动量七成），但仍属样本内结论。
    0.5
}

fn default_tp() -> f64 {
    // 默认关闭：实测 10% 止盈在真实模型下是中性偏负（Sharpe 1.62 → 1.59），
    // 且会持续打薄某一侧腿、造成需要额外机制去补的漂移。
    0.0
}

impl LiveConfig {
    pub fn effective_leverage(&self) -> f64 {
        self.leverage
    }

    pub fn trade_config(&self) -> TradeConfig {
        TradeConfig {
            lookback: self.lookback,
            top_frac: self.top_frac,
            min_vol_usd: self.min_vol_usd,
            leverage: self.effective_leverage(),
            target_positions: self.target_positions,
            slippage: self.slippage,
            margin_buffer: self.margin_buffer,
            min_order_pct: self.min_order_pct,
            // 这里漏掉过 rebalance_slices：`..Default::default()` 会静默用默认值 3，
            // 界面上改成 1 也毫无作用。下面有 field_parity 测试守着。
            rebalance_slices: self.rebalance_slices,
            momentum_weight: self.momentum_weight,
            take_profit_pct: self.take_profit_pct,
            ..Default::default()
        }
    }

    pub fn can_sign(&self) -> bool {
        !self.account.is_empty() && !self.key_path.is_empty()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct EquityPoint {
    pub ts: i64,
    pub equity: f64,
    /// Historical auxiliary field; the account equity chart uses equity, including cash flows.
    pub pnl: f64,
}

/// One order we sent (or planned) and what the exchange said about it.
#[derive(Clone, Serialize, Deserialize)]
pub struct LiveRecord {
    pub ts: i64,
    pub coin: String,
    pub side: String,
    pub action: String,
    pub size: f64,
    pub price: f64,
    pub notional: f64,
    pub result: String,
    pub live: bool,
    /// 交易所成交回执的已实现盈亏，不据此推断订单意图。
    pub pnl: Option<f64>,
    /// 交易所成交号，用于对账去重。
    pub tid: Option<u64>,
    /// Position entry price when this order closes/reduces, so the UI can show
    /// the realised P&L of the close. None for opening orders.
    pub entry_px: Option<f64>,
    /// True when the order shrinks/closes a position (so the UI can label the
    /// row with the *position* direction rather than the order direction).
    pub reduce_only: bool,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct LiveState {
    #[serde(default)]
    pub capital_baseline: Option<CapitalBaseline>,
    pub config: LiveConfig,
    pub history: Vec<EquityPoint>,
    pub records: Vec<LiveRecord>,
    pub last_run_at: Option<i64>,
    pub last_plan: Vec<String>,
    /// 上次调仓时各币的中间价。止盈幅度必须相对它计算，而不是相对当前价 ——
    /// 否则手动刷新会把止盈线随行情搬走（100 建仓、现价 80 的多头会被挂到 88）。
    pub tp_ref: HashMap<String, f64>,
    /// 我们挂出去的止盈单 oid ✗ 对账时据此把回执标成「止盈成交」✗
    #[serde(default)]
    pub tp_oids: Vec<u64>,
    /// 已对账到哪个成交时间戳，避免重复导入。
    pub reconciled_to: i64,
    /// 上次向交易所对账的时间，用于节流。
    pub last_reconcile_ms: i64,
    pub last_live: bool,
    /// **策略开始时的净值** —— 收益率的固定分母。
    ///
    /// 之前前端用「当前净值 − 累计盈亏」反推基数，但累计盈亏并不完整
    /// （资金费、未记录成交都会造成偏差），于是反推出来的基数会自己漂：
    /// 实测几秒内从 $560.02 变成 $563.17，收益率跟着上下跳。
    /// 这里在**第一次记录净值时**写死一个值，之后再也不改。
    /// 旧 state 仅能从留存净值迁移，不能从不完整的交易盈亏反推。
    #[serde(default)]
    pub start_equity: Option<f64>,
    /// 累计净入金（入金为正、出金为负）。净投入收益率 = (净值 − 净入金 − 起始资金) / (起始资金 + 净入金)，分母须为正。
    #[serde(default)]
    pub net_deposit: f64,
    /// 每一笔入金/出金流水。
    #[serde(default)]
    pub capital_flows: Vec<CapitalFlow>,
    /// 策略矩阵资金分配（动量 / carry / 尖峰狩猎），仅记录与展示。
    #[serde(default)]
    pub capital_alloc: CapitalAlloc,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CapitalBaseline {
    pub account: String,
    pub equity: f64,
    pub ts: Option<i64>,
    pub source: String,
}

/// 一天的执行指标 —— 一个月后判断"跑得对不对"的依据。
///
/// 从成交记录【按需聚合】，不单独落盘：记录本身不截断，随时可重算；
/// 存第二份历史只会引入不一致。
#[derive(Clone, Serialize)]
pub struct DailyExec {
    /// UTC 天序号（ts / 86_400_000）
    pub day: i64,
    /// 真实下单笔数
    pub orders: usize,
    /// 真实下单名义合计
    pub notional: f64,
    /// 换手 = 名义 / 快照净值
    pub turnover: f64,
    /// 成交记录里的平均【不利】滑点（%），正数 = 吃亏
    pub avg_slip_pct: Option<f64>,
}

/// 从 `"成交 512.0@0.084119"` 里取出成交价。
fn fill_px(result: &str) -> Option<f64> {
    let at = result.split("成交 ").nth(1)?.split('@').nth(1)?;
    let num: String = at
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    num.parse().ok()
}

/// 把成交记录聚合成每日执行指标。
///
/// **换手的分母必须是【当天的净值】**，不是当前净值。
/// 真实 bug：用当前净值当所有天的分母，账户从 $550 充到 $3,100 之后，
/// 10-07 那天的换手被算成 0.44（真实 2.41），低估 5 倍 —— 正好把
/// "线上换手和回测差不多的 1.7/天"这个结论掩盖成"线上换手只有 0.3/天"。
pub fn daily_execution(
    records: &[LiveRecord],
    history: &[EquityPoint],
    fallback_equity: f64,
) -> Vec<DailyExec> {
    use std::collections::BTreeMap;
    // 每天用【当天最后一条】净值；没有当天记录就沿用最近一个更早的值。
    let mut eq_by_day: BTreeMap<i64, f64> = BTreeMap::new();
    for p in history.iter().filter(|p| p.equity.is_finite() && p.equity > 0.0) {
        eq_by_day.insert(p.ts / 86_400_000, p.equity);
    }
    let equity_on = |day: i64| -> f64 {
        eq_by_day
            .range(..=day)
            .next_back()
            .map(|(_, v)| *v)
            .unwrap_or(fallback_equity)
    };
    // day -> (笔数, 名义, 滑点合计, 有滑点的笔数)
    let mut acc: BTreeMap<i64, (usize, f64, f64, usize)> = BTreeMap::new();
    for r in records.iter().filter(|r| r.live) {
        let e = acc.entry(r.ts / 86_400_000).or_insert((0, 0.0, 0.0, 0));
        e.0 += 1;
        e.1 += r.notional.abs();
        if let Some(fill) = fill_px(&r.result) {
            if r.price > 0.0 && fill > 0.0 {
                // 买入成交价偏高 = 吃亏；卖出成交价偏低 = 吃亏。统一成"正数=不利"。
                let slip = if r.side.starts_with('买') {
                    (fill - r.price) / r.price
                } else {
                    (r.price - fill) / r.price
                };
                e.2 += slip;
                e.3 += 1;
            }
        }
    }
    acc.into_iter()
        .map(|(day, (orders, notional, ss, sn))| {
            let eq = equity_on(day);
            DailyExec {
                day,
                orders,
                notional,
                turnover: if eq > 0.0 { notional / eq } else { 0.0 },
                avg_slip_pct: (sn > 0).then(|| ss / sn as f64 * 100.0),
            }
        })
        .collect()
}

/// 一笔入金或出金（amount 正 = 入金，负 = 出金）。收益率必须扣除它，
/// 否则充钱会让净值跳升、被误当成策略赚的。
#[derive(Clone, Serialize, Deserialize)]
pub struct CapitalFlow {
    /// Registration timestamp; retained for audit and legacy records.
    pub ts: i64,
    #[serde(default)]
    pub effective_ts: Option<i64>,
    pub amount: f64,
    pub note: String,
}

/// 策略矩阵的资金分配（用户自填，仅作记录与展示，不驱动任何自动执行）。
/// 三种策略：① 横截面动量 ② 资金费 carry ③ 尖峰狩猎。
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct CapitalAlloc {
    pub momentum_usd: f64,
    pub carry_usd: f64,
    pub hunting_usd: f64,
}

pub fn merge_reconciliation(current: &mut LiveState, snapshot: &LiveState) {
    if !current.config.account.eq_ignore_ascii_case(&snapshot.config.account) { return; }
    let mut known:std::collections::HashSet<u64>=current.records.iter().filter_map(|r|r.tid).collect();
    for record in &snapshot.records {
        if let Some(tid)=record.tid {
            if known.insert(tid) {current.records.push(record.clone());}
        }
    }
    current.reconciled_to=current.reconciled_to.max(snapshot.reconciled_to);
    current.last_reconcile_ms=current.last_reconcile_ms.max(snapshot.last_reconcile_ms);
}

impl LiveState {
    pub fn reset_capital_for_account_change(&mut self, previous_account: &str) {
        if !self.config.account.eq_ignore_ascii_case(previous_account) {
            self.capital_baseline = None;
            self.net_deposit = 0.0;
            self.capital_flows.clear();
            self.start_equity = None;
            self.history.clear();
        }
    }
    /// Only observed equity is a valid baseline; never infer it from trade PnL.
    pub fn baseline(&self) -> Option<CapitalBaseline> {
        if let Some(b) = &self.capital_baseline {
            return (b.account.eq_ignore_ascii_case(&self.config.account)
                && b.equity.is_finite() && b.equity > 0.).then(|| b.clone());
        }
        let first = self.history.iter().filter(|p| p.equity.is_finite() && p.equity > 0.)
            .min_by_key(|p| p.ts);
        let saved = self.start_equity.filter(|v| v.is_finite() && *v > 0.);
        let equity = saved.or_else(|| first.map(|p| p.equity))?;
        Some(CapitalBaseline {
            account: self.config.account.clone(), equity,
            ts: if saved.is_some() { None } else { first.map(|p| p.ts) },
            source: if saved.is_some() { "已保存的起始资金" } else { "最早留存净值（启动前数据未知）" }.into(),
        })
    }
    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes=serde_json::to_vec_pretty(self)?;
        std::fs::write(path,bytes)?;
        Ok(())
    }
}

#[derive(Serialize)]
pub struct LivePosition {
    pub coin: String,
    pub side: String,
    pub size: f64,
    pub entry_px: f64,
    pub mark_px: f64,
    pub notional: f64,
    pub unrealized: f64,
    pub liq_px: Option<f64>,
    pub dist_pct: Option<f64>,
    /// true = 全仓，false = 逐仓
    pub is_cross: bool,
    pub leverage: u32,
}

#[derive(Serialize)]
pub struct LiveSnapshot {
    pub capital_baseline: Option<CapitalBaseline>,
    pub configured: bool,
    pub error: Option<String>,
    pub account: String,
    pub equity: f64,
    /// 收益率的分母（策略开始时的净值）。前端直接用它，不要再用
    /// 「净值 − 累计盈亏」反推 —— 那个基数会漂。
    pub start_equity: f64,
    pub positions: Vec<LivePosition>,
    pub gross_notional: f64,
    pub net_notional: f64,
    pub margin_used: f64,
    pub maintenance_margin: f64,
    pub liq_buffer_pct: f64,
    pub nearest_liq_pct: Option<f64>,
    /// 仍是逐仓的仓位数（Hyperliquid 不允许持仓时切换模式）
    pub isolated_count: usize,
    /// 当前持仓的未实现盈亏合计，与入金无关。
    ///
    /// 注意：这个字段**不含已实现盈亏**（历史保存点里也是同一口径，
    /// 中途改成"已实现+未实现"会让新旧数据点不可比）。前端展示的
    /// 「策略盈亏」是另一条路径：净值 − 记录起点 − 累计净入金。
    pub cumulative_pnl: f64,
    /// 用户手动记录的累计净入金。
    pub net_deposit: f64,
    /// 入金/出金流水。
    pub capital_flows: Vec<CapitalFlow>,
    /// 资金矩阵分配（动量 / carry / 尖峰狩猎），仅展示。
    pub capital_alloc: CapitalAlloc,
    /// 每日执行指标（换手/笔数/滑点），用于月度执行质量复查。
    pub exec_daily: Vec<DailyExec>,
    /// 拿不到行情、只能用交易所市值反推估值的币。非空时要显眼提示 ——
    /// 这些仓位的清算价、距离强平都算不出来，可能有隐藏风险。
    pub unpriced: Vec<String>,
    /// 影子回测净值曲线（从实盘开始那天起，起点归一为 1.0），供页面与实盘并排对照。
    pub shadow: Vec<(i64, f64)>,
    /// 盘口上挂着的止盈单
    pub tp_orders: Vec<crate::exchange::OpenOrder>,
    /// 我们挂出去的止盈单 oid ✗ 对账时据此把成交标成「止盈成交」✗ 而不是
    /// 「意图未确认」✓（否则用户看不出这笔试交易所回执到底是止盈还是手动平仓 ✗）
    #[serde(default)]
    pub tp_oids: Vec<u64>,
    pub config: LiveConfig,
    pub history: Vec<EquityPoint>,
    pub records: Vec<LiveRecord>,
    pub last_run_at: Option<i64>,
    pub last_plan: Vec<String>,
    /// 上次调仓时各币的中间价。止盈幅度必须相对它计算，而不是相对当前价 ——
    /// 否则手动刷新会把止盈线随行情搬走（100 建仓、现价 80 的多头会被挂到 88）。
    pub tp_ref: HashMap<String, f64>,
    pub last_live: bool,
}

/// Read-only view of the live account: positions, exposure and liquidation risk.
pub async fn snapshot(
    state: &LiveState,
    markets: &HashMap<String, MarketInfo>,
    http: Option<reqwest::Client>,
) -> LiveSnapshot {
    let cfg = state.config.clone();
    let mut snap = LiveSnapshot {
        capital_baseline: state.baseline(),
        configured: cfg.can_sign(),
        error: None,
        account: cfg.account.clone(),
        equity: 0.0,
        positions: Vec::new(),
        gross_notional: 0.0,
        net_notional: 0.0,
        margin_used: 0.0,
        maintenance_margin: 0.0,
        liq_buffer_pct: 0.0,
        nearest_liq_pct: None,
        isolated_count: 0,
        tp_orders: Vec::new(),
        tp_oids: state.tp_oids.clone(),
        start_equity: 0.0,
        cumulative_pnl: 0.0,
        shadow: Vec::new(),
        unpriced: Vec::new(),
        net_deposit: 0.0,
        capital_flows: Vec::new(),
        capital_alloc: state.capital_alloc.clone(),
        exec_daily: Vec::new(),
        tp_ref: HashMap::new(),
        config: cfg.clone(),
        history: state.history.clone(),
        records: state.records.clone(),
        last_run_at: state.last_run_at,
        last_plan: state.last_plan.clone(),
        last_live: state.last_live,
    };
    if cfg.account.is_empty() {
        snap.error = Some("未配置账户地址".into());
        return snap;
    }
    let exec_res = match http {
        Some(h) => Exec::reader_shared(h, Some(&cfg.account)).await,
        None => Exec::reader_for(Some(&cfg.account)).await,
    };
    let exec = match exec_res {
        Ok(e) => e,
        Err(e) => {
            snap.error = Some(format!("{e}"));
            return snap;
        }
    };
    let acct = match exec.account().await {
        Ok(a) => a,
        Err(e) => {
            snap.error = Some(format!("读取账户失败: {e}"));
            return snap;
        }
    };
    snap.equity = acct.equity;
    let coins: Vec<String> = acct.positions.keys().cloned().collect();
    let mids = trader::fetch_mids(&exec, &coins).await;
    let mut unpriced: Vec<String> = Vec::new();

    for (coin, pos) in acct.positions.iter() {
        // 拿不到中间价时**绝不能退回开仓价** —— 那会把未实现盈亏算成 0，
        // 一个正在亏损的退市仓位会显示成持平，虚增净值和盈亏。改用交易所
        // 报的市值反推标记价（positionValue = |size| × mark），它永远存在。
        let (mark, price_known) = match mids.get(coin).copied() {
            Some(m) if m > 0.0 => (m, true),
            _ => {
                let known = pos.size.abs() > 1e-12 && pos.position_value > 0.0;
                let m = if known {
                    pos.position_value / pos.size.abs()
                } else {
                    pos.entry_px
                };
                // 只有**连交易所市值都没有**才算真的定价不了。
                if !known {
                    unpriced.push(coin.clone());
                }
                (m, known)
            }
        };
        let notional = (pos.size * mark).abs();
        // 有中间价就自己算（口径统一），否则直接用交易所的权威值。
        let unrealized = if price_known {
            pos.size * (mark - pos.entry_px)
        } else {
            pos.unrealized_pnl
        };
        let dist = pos.liq_px.and_then(|liq| {
            if mark > 0.0 && liq > 0.0 {
                Some(if pos.size > 0.0 {
                    (mark - liq) / mark * 100.0
                } else {
                    (liq - mark) / mark * 100.0
                })
            } else {
                None
            }
        });
        snap.gross_notional += notional;
        snap.net_notional += pos.size * mark;
        if let Some(m) = markets.get(coin.as_str()) {
            snap.maintenance_margin += notional * maintenance_margin_rate(m.max_leverage);
        }
        if let Some(d) = dist {
            snap.nearest_liq_pct = Some(snap.nearest_liq_pct.map_or(d, |x: f64| x.min(d)));
        }
        snap.positions.push(LivePosition {
            coin: coin.clone(),
            side: if pos.size > 0.0 { "long".into() } else { "short".into() },
            size: pos.size,
            entry_px: pos.entry_px,
            mark_px: mark,
            notional,
            unrealized,
            liq_px: pos.liq_px,
            dist_pct: dist,
            is_cross: pos.is_cross,
            leverage: pos.leverage,
        });
    }
    snap.positions.sort_by(|a, b| a.coin.cmp(&b.coin));
    snap.isolated_count = snap.positions.iter().filter(|p| !p.is_cross).count();
    snap.cumulative_pnl = state.unrealized_pnl(&snap.positions);
    // Missing historical equity remains unknown until an observation is persisted.
    snap.start_equity = state.baseline().map(|b| b.equity).unwrap_or(0.);
    snap.net_deposit = state.net_deposit;
    snap.capital_flows = state.capital_flows.clone();
    snap.exec_daily = daily_execution(&state.records, &state.history, snap.equity);
    // 不按 reduceOnly 过滤：该字段在 openOrders 里不一定存在，过滤会导致
    // 表格永远是空的。程序只挂只减仓单，所以全部展示即可。
    if let Ok(orders) = exec.open_order_details().await {
        snap.tp_orders = orders;
    }
    snap.margin_used = if cfg.effective_leverage() > 0.0 {
        snap.gross_notional / cfg.effective_leverage()
    } else {
        0.0
    };
    snap.liq_buffer_pct = if snap.equity > 0.0 {
        ((snap.equity - snap.maintenance_margin) / snap.equity * 100.0).max(0.0)
    } else {
        0.0
    };
    snap.unpriced = unpriced;
    snap
}

#[derive(Serialize)]
pub struct RunResult {
    pub aborted: bool,
    pub equity: f64,
    pub long_leg: Vec<String>,
    pub short_leg: Vec<String>,
    pub per_coin: f64,
    pub plan_lines: Vec<String>,
    /// 本次调仓使用的参考价，供上层持久化（止盈幅度相对它计算）。
    pub tp_ref: Option<HashMap<String, f64>>,
    pub executed: Vec<String>,
    pub live: bool,
    /// 本次挂出的止盈单 oid，供上层写入状态（对账时标记「止盈成交」）。
    pub tp_oids: Vec<u64>,
}

fn ensure_candidates(weights: &[(String, f64)]) -> Result<()> {
    if !weights.is_empty() {
        return Ok(());
    }
    anyhow::bail!("流动性过滤后没有候选")
}

fn signal_panel(store: &crate::store::Store) -> Result<Vec<crate::momentum::PanelEntry>> {
    trader::load_panel(store)
}

/// Compute the target book and, when `live` is true and the config is armed,
/// send the orders. Ranking is identical to the paper engine.
pub async fn run(
    store: &crate::store::Store,
    state: &LiveState,
    markets: &HashMap<String, MarketInfo>,
    live: bool,
) -> Result<(RunResult, Vec<LiveRecord>)> {
    let cfg = state.config.clone();
    anyhow::ensure!(!cfg.account.is_empty(), "未配置账户地址");
    if live {
        // A dry run is read-only, so only real trading needs the signing key.
        anyhow::ensure!(!cfg.key_path.is_empty(), "未配置 API 钱包密钥路径");
        anyhow::ensure!(cfg.armed, "实盘未启用（需先打开「启用实盘」开关）");
    }

    let exec = if live {
        Exec::signer(&cfg.account, std::path::Path::new(&cfg.key_path)).await?
    } else {
        Exec::reader_for(Some(&cfg.account)).await?
    };
    let tc = cfg.trade_config();

    let panel = signal_panel(store)?;
    anyhow::ensure!(
        panel.len() >= 20,
        "K 线缓存不足（{} 币）—— 请确认对应信号源的日线缓存已就绪",
        panel.len()
    );
    let acct0 = exec.account().await?;
    let (weights, liquid) = trader::target_weights(&panel, &tc, acct0.equity, crate::live::now_ms_pub() as i64);
    ensure_candidates(&weights)?;
    let acct = exec.account().await?;
    let mut coins: Vec<String> = weights.iter().map(|(c, _)| c.clone()).collect();
    coins.extend(acct.positions.keys().cloned());
    coins.sort();
    coins.dedup();
    let mids = trader::fetch_mids(&exec, &coins).await;

    let plan = trader::build_plan(&weights, &acct, markets, &mids, &tc, None);

    let mut plan_lines = vec![format!(
        "流动宇宙 {} 币 · 多头腿 {} · 空头腿 {} · 账户净值 ${:.2} · 每仓 ${:.2} · 杠杆 {:.0}x · 缓冲 {:.0}%",
        liquid.len(),
        weights.iter().filter(|(_, w)| *w > 0.0).count(),
        weights.iter().filter(|(_, w)| *w < 0.0).count(),
        plan.equity,
        plan.per_coin,
        cfg.effective_leverage(),
        cfg.margin_buffer * 100.0
    )];
    // 腿的名单**按权重强弱排序**并显示权重值。
    //
    // 原来直接用 Vec 的插入顺序 —— 那是三因子合并后的任意顺序，看不出谁强谁弱。
    // 用户想知道"为什么某个币被做空、它排第几"，光有名字是答不了的。
    {
        let wmap: std::collections::HashMap<&str, f64> =
            weights.iter().map(|(c, w)| (c.as_str(), *w)).collect();
        let legs = |names: &[String]| -> String {
            let mut v: Vec<(&String, f64)> = names
                .iter()
                .map(|c| (c, wmap.get(c.as_str()).copied().unwrap_or(0.0)))
                .collect();
            // 做多按权重从大到小，做空按 |权重| 从大到小 —— 最强的排最前
            v.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap_or(std::cmp::Ordering::Equal));
            v.iter()
                .map(|(c, w)| format!("{c}({:.3})", w.abs()))
                .collect::<Vec<_>>()
                .join(" ")
        };
    plan_lines.push("信号源: Hyperliquid".into());
    plan_lines.push(format!("多头腿（按权重降序）: {}", legs(&plan.long_leg)));
        plan_lines.push(format!("空头腿（按权重降序）: {}", legs(&plan.short_leg)));
    }
    for o in &plan.orders {
        plan_lines.push(format!(
            "{} {} {} {:.6} @≈{:.6} (${:.2}) · {}",
            if o.buy { "买入" } else { "卖出" },
            o.coin,
            if o.reduce_only { "只减仓" } else { "开仓" },
            o.size,
            o.mid,
            o.notional,
            o.reason
        ));
    }
    for n in &plan.notes {
        plan_lines.push(format!("注意: {n}"));
    }
    if !plan.held.is_empty() {
        plan_lines.push(format!(
            "注意: {} 个已有仓位无法切换保证金模式（Hyperliquid 不允许持仓时切换），\
             如需全部转为全仓请用「转为全仓」逐币处理",
            plan.held.len()
        ));
    }
    if plan.orders.is_empty() {
        plan_lines.push("无需调仓（已在目标状态）".into());
    }

    let now = now_ms_pub();
    let mut records = Vec::new();
    for o in &plan.orders {
        // 减仓/平仓的单子带上原持仓的入场价，前端据此算这笔的已实现盈亏
        let entry_px = if o.reduce_only {
            acct.positions.get(&o.coin).map(|p| p.entry_px)
        } else {
            None
        };
        records.push(LiveRecord {
            pnl: None,
            tid: None,
            ts: now,
            coin: o.coin.clone(),
            side: if o.buy { "买".into() } else { "卖".into() },
            action: o.reason.clone(),
            size: o.size,
            price: o.mid,
            notional: o.notional,
            result: if live { "已发送".into() } else { "计划".into() },
            live,
            entry_px,
            reduce_only: o.reduce_only,
        });
    }

    // 调仓前先撤掉自己的旧止盈单。必须在 execute 之前：否则昨天的止盈单会在
    // 调仓过程中成交，把计划要减的仓位提前平掉，之后的减仓单被拒、开仓单照发。
    //
    // 失败必须中止，不能只记录：撤不掉的旧单会在新单之外继续存在，同一个仓位上
    // 出现两张只减仓单（交易所只按仓位校验，不会去重）。
    if live {
        match cancel_our_take_profits(&exec).await {
            Ok(n) if n > 0 => plan_lines.push(format!("撤销旧止盈单 {n} 个")),
            Ok(_) => {}
            Err(e) => {
                return Err(e.context("撤销旧止盈单失败，已在调仓前中止"));
            }
        }
    }

    let outcome = trader::execute(&exec, &plan, &tc, markets, live).await?;
    // 前置日志（设杠杆失败等）单独放在最前面，绝不与订单结果混用下标
    for line in &outcome.prelim {
        plan_lines.push(format!("注意: {line}"));
    }
    // 逐条按同一下标对应，保证失败信息挂在正确的币上
    for (i, line) in outcome.orders.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        if let Some(r) = records.get_mut(i) {
            r.result = line.clone();
        }
    }
    let aborted = outcome.aborted;
    let executed = outcome.orders;
    let mut tp_ref_out: HashMap<String, f64> = HashMap::new();
    let mut tp_oids_out: Vec<u64> = Vec::new();
    if live && !aborted {
        // 基准价用本次调仓的目标币中间价，而不是挂单时的实时价 ——
        // 后者会让手动「刷新止盈单」把止盈线随行情一起搬走。
        let tp_ref: HashMap<String, f64> = plan
            .long_leg
            .iter()
            .chain(plan.short_leg.iter())
            .filter_map(|c| mids.get(c).map(|m| (c.clone(), *m)))
            .collect();
        tp_ref_out = tp_ref.clone();
        match place_take_profits(&exec, markets, cfg.take_profit_pct, &tp_ref).await {
            Ok((lines, oids)) => {
                plan_lines.extend(lines);
                tp_oids_out = oids;
            }
            Err(e) => plan_lines.push(format!("止盈单挂单失败: {e}")),
        }
    }

    Ok((
        RunResult {
            aborted,
            equity: plan.equity,
            long_leg: plan.long_leg.clone(),
            short_leg: plan.short_leg.clone(),
            per_coin: plan.per_coin,
            tp_ref: (!aborted).then_some(tp_ref_out),
            tp_oids: tp_oids_out,
            plan_lines,
            executed,
            live,
        },
        records,
    ))
}

pub fn now_ms_pub() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}


/// 逐币把仓位转成全仓：平掉 → 切 cross → 按目标重新开仓。
///
/// Hyperliquid 不允许在持仓状态下切换保证金模式（"Cannot switch leverage
/// while a position is open"），所以必须平掉再开。逐币处理的好处是组合里
/// 其余仓位始终保持对冲，任一时刻只有 1 个仓位处于裸奔状态。
pub async fn rebuild_cross(
    store: &crate::store::Store,
    state: &LiveState,
    markets: &HashMap<String, MarketInfo>,
) -> Result<Vec<String>> {
    let cfg = state.config.clone();
    anyhow::ensure!(cfg.armed, "实盘未启用（需先打开「启用实盘」开关）");
    anyhow::ensure!(cfg.can_sign(), "未配置 API 钱包密钥");
    let exec = Exec::signer(&cfg.account, std::path::Path::new(&cfg.key_path)).await?;
    let tc = cfg.trade_config();

    // 先读账户：仓位数上限（cap）依赖净值，必须在算权重之前拿到。
    let acct = exec.account().await?;
    if acct.positions.is_empty() {
        return Ok(vec!["账户没有持仓，无需转换".into()]);
    }
    let panel = signal_panel(store)?;
    let (weights, _) = trader::target_weights(
        &panel,
        &tc,
        acct.equity,
        crate::live::now_ms_pub() as i64,
    );
    ensure_candidates(&weights)?;
    let coins: Vec<String> = acct.positions.keys().cloned().collect();
    let mids = trader::fetch_mids(&exec, &coins).await;
    // 按各自权重还原仓位。之前用「净值×杠杆÷仓位数」等权重建，会把权重抹平，
    // 总敞口也从 Σ|w| 变成 1 —— 点一次「转为全仓」就悄悄改了权重和杠杆。
    let deployable = acct.equity * cfg.margin_buffer;
    let wmap: HashMap<String, f64> = weights.iter().cloned().collect();
    let per_coin = deployable * cfg.effective_leverage() / weights.len().max(1) as f64;

    let mut log = vec![format!(
        "逐币转全仓：{} 个持仓 · 目标每仓 ${:.2}",
        acct.positions.len(),
        per_coin
    )];
    for coin in &coins {
        let pos = &acct.positions[coin];
        // 0) 已经是全仓的不要动。审计指出：原来的代码会把已有全仓仓也平掉重建，
        //    白付两次成本，而且重建期间组合是裸的。
        if pos.is_cross {
            log.push(format!("{coin}: 已是全仓，跳过"));
            continue;
        }
        let Some(&mid) = mids.get(coin) else {
            log.push(format!("{coin}: 取价失败，跳过"));
            continue;
        };
        let Some(m) = markets.get(coin) else {
            log.push(format!("{coin}: 缺市场元数据，跳过"));
            continue;
        };
        let lev = (cfg.effective_leverage().round() as u32).clamp(1, m.max_leverage.max(1));

        // 1) 平仓
        let size = crate::exchange::round_size(pos.size.abs(), m.sz_decimals);
        if size * mid >= 10.0 {
            let buy = pos.size < 0.0;
            match exec
                .ioc(coin, buy, true, size, mid, cfg.slippage, m.sz_decimals)
                .await
            {
                Ok(s) => log.push(format!("平 {coin} {size} · {}", crate::exchange::describe(&s))),
                Err(e) => {
                    log.push(format!("平 {coin} 失败：{e}（保持原样）"));
                    continue;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // 1b) **必须确认真的平掉了**。IOC 可能只成交一部分，而返回仍然是 Ok；
        //     旧代码把任何 Ok 当"平完了"，然后按完整目标开仓 —— 审计给的反例是
        //     原多 10 只平掉 2 又买 10，最终多 18（超额 80%）；目标反向时甚至会
        //     做成和目标相反的净方向。用 200ms 睡眠代替确认是不成立的。
        let after = exec.account().await?;
        let residual = after.positions.get(coin).map(|p| p.size).unwrap_or(0.0);
        let mid_now = trader::fetch_mids(&exec, std::slice::from_ref(coin))
            .await
            .get(coin)
            .copied()
            .unwrap_or(mid);
        if (residual * mid_now).abs() >= 10.0 {
            log.push(format!(
                "{coin}: 平仓未完成（残留 {residual}），跳过该币 —— 继续开仓会变成反手加仓"
            ));
            continue;
        }

        // 2) 切全仓。**失败必须停手**：带着逐仓继续开，等于给单币强平敞口。
        if let Err(e) = exec.set_leverage(coin, lev).await {
            log.push(format!("{coin}: 切全仓失败 {e}，跳过该币（不开仓）"));
            continue;
        }
        log.push(format!("{coin}: 已切 {lev}x 全仓"));

        // 3) 按 target − actual 开仓，而不是按完整目标
        let w = weights.iter().find(|(c, _)| c == coin).map(|(_, w)| *w).unwrap_or(0.0);
        let want_long = w > 0.0;
        if want_long || w < 0.0 {
            let notional = wmap.get(coin).map(|x| x.abs()).unwrap_or(0.0)
                * deployable
                * cfg.effective_leverage();
            let tsize = crate::exchange::round_size(notional / mid_now, m.sz_decimals);
            let want = if want_long { tsize } else { -tsize };
            let delta = want - residual;
            let dsize = crate::exchange::round_size(delta.abs(), m.sz_decimals);
            if dsize * mid_now >= 10.0 {
                match exec
                    .ioc(coin, delta > 0.0, false, dsize, mid_now, cfg.slippage, m.sz_decimals)
                    .await
                {
                    Ok(s) => log.push(format!(
                        "重开 {coin} {dsize} · {}",
                        crate::exchange::describe(&s)
                    )),
                    Err(e) => log.push(format!("重开 {coin} 失败：{e}")),
                }
            }
        } else {
            log.push(format!("{coin}: 不在目标名单，保持平仓"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    Ok(log)
}


/// 撤掉本程序挂出的止盈单。
///
/// 只撤带 cloid 前缀的单，不会误撤用户手动挂的单。返回撤销数量。
/// 注意：`open_orders` 失败会直接冒泡 —— 撤不掉旧单就绝不能继续下单，
/// 否则同一个仓位上会同时存在两张只减仓单（交易所只按仓位校验，不会去重）。
pub async fn cancel_our_take_profits(exec: &Exec) -> Result<usize> {
    let orders = exec.open_order_details().await?;
    let ours: Vec<(String, u64)> = orders
        .iter()
        .filter(|o| crate::exchange::is_ours(o.cloid.as_deref()))
        .map(|o| (o.coin.clone(), o.oid))
        .collect();
    if ours.is_empty() {
        return Ok(0);
    }
    exec.cancel_orders(&ours).await
}

/// 为当前所有持仓挂止盈单。
///
/// `reference` 是**调仓时**记录的参考价：止盈幅度必须相对那个价格算，而不是
/// 相对当前中间价。否则手动「刷新止盈单」会把止盈线随行情一起搬走 —— 例如
/// 100 建仓、现价 80 的多头会被挂到 88，变成一笔自动的亏损出场。
/// 参考价缺失（或已越过止盈线）时退回当前中间价，避免挂出立即成交的单。
pub async fn place_take_profits(
    exec: &Exec,
    markets: &HashMap<String, MarketInfo>,
    tp_pct: f64,
    reference: &HashMap<String, f64>,
) -> Result<(Vec<String>, Vec<u64>)> {
    let mut log = Vec::new();
    let mut oids: Vec<u64> = Vec::new();
    if tp_pct <= 0.0 {
        log.push("未启用止盈".into());
        return Ok((log, oids));
    }
    anyhow::ensure!(!markets.is_empty(), "市场元数据为空，拒绝挂止盈单");
    let acct = exec.account().await?;
    if acct.positions.is_empty() {
        log.push("无持仓，不挂止盈".into());
        return Ok((log, oids));
    }
    let coins: Vec<String> = acct.positions.keys().cloned().collect();
    let mids = trader::fetch_mids(exec, &coins).await;
    anyhow::ensure!(!mids.is_empty(), "取不到中间价，拒绝挂止盈单");

    let mut placed = 0usize;
    let mut filled = 0usize;
    let mut failed = Vec::new();
    for (coin, pos) in acct.positions.iter() {
        let Some(m) = markets.get(coin.as_str()) else {
            failed.push(format!("{coin}: 缺市场元数据"));
            continue;
        };
        let Some(&mid) = mids.get(coin) else {
            failed.push(format!("{coin}: 缺中间价"));
            continue;
        };
        if mid <= 0.0 {
            continue;
        }
        let is_long = pos.size > 0.0;
        // 基准价 = 【持仓成本】（选项 B）。成本缺失才退回调仓参考价、再退回中间价。
        //
        // 为什么改成成本价：相对"上次调仓的中间价"挂止盈，止盈线会随行情每天搬家，
        // 变成"每天从开盘价再涨 10% 才走"，而不是"这笔仓位赚到 10% 就走"。
        // 实测（2026-10-17，234 币日线）：相对成本·日内触线 Sharpe 2.63、暴跌周 −11.2%，
        // 相对开盘·日内触线 Sharpe 3.18 但暴跌周 −16.8%。选成本版（用户选项 B）。
        let base = if pos.entry_px > 0.0 && pos.entry_px.is_finite() {
            pos.entry_px
        } else {
            reference.get(coin).copied().unwrap_or(mid)
        };
        // 现价已经越过止盈线：不挂"一挂就成交"的单子，交给调仓计划按「止盈」平掉。
        if (is_long && base * (1.0 + tp_pct) <= mid) || (!is_long && base * (1.0 - tp_pct) >= mid) {
            log.push(format!("{coin}: 已越过止盈线，由调仓计划平仓"));
            continue;
        }
        let (buy, px) = take_profit_order(is_long, base, tp_pct, m.sz_decimals);
        if px <= 0.0 {
            failed.push(format!("{coin}: 价位精度不足，无法挂出有效止盈价"));
            continue;
        }
        let size = round_size(pos.size.abs(), m.sz_decimals);
        if size <= 0.0 || size * mid < 10.0 {
            continue;
        }
        match exec.resting_reduce_order(coin, buy, size, px).await {
            // oid 为 0 表示挂出去的瞬间就成交了（价格已经越过止盈线）。
            // 这不是失败，但也不能报成「挂上」。
            Ok(0) => filled += 1,
            Ok(oid) => {
                oids.push(oid);
                placed += 1;
            }
            Err(e) => failed.push(format!("{coin}: {e}")),
        }
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }
    log.push(format!(
        "止盈单：挂上 {placed} 个{}（±{:.0}%）{}",
        if filled > 0 {
            format!("，{filled} 个挂出即成交")
        } else {
            String::new()
        },
        tp_pct * 100.0,
        if failed.is_empty() {
            String::new()
        } else {
            format!("，未挂 {}", failed.join("; "))
        }
    ));
    Ok((log, oids))
}

/// 供外部按钮调用：先撤自己的旧单，再按参考价重挂。
pub async fn refresh_take_profits(
    exec: &Exec,
    markets: &HashMap<String, MarketInfo>,
    tp_pct: f64,
    reference: &HashMap<String, f64>,
) -> Result<(Vec<String>, Vec<u64>)> {
    let mut log = Vec::new();
    let mut oids: Vec<u64> = Vec::new();
    // 先确认「一定挂得上」再去撤旧单：撤单成功而挂单失败（取不到价、缺元数据）
    // 会让账户在下次调仓前完全失去止盈保护，而且日志还会显示成功。
    if tp_pct > 0.0 {
        anyhow::ensure!(!markets.is_empty(), "市场元数据为空，拒绝撤销现有止盈单");
        let acct = exec.account().await?;
        if !acct.positions.is_empty() {
            let coins: Vec<String> = acct.positions.keys().cloned().collect();
            let mids = trader::fetch_mids(exec, &coins).await;
            anyhow::ensure!(!mids.is_empty(), "取不到中间价，拒绝撤销现有止盈单");
        }
    }
    match cancel_our_take_profits(exec).await {
        Ok(n) if n > 0 => log.push(format!("撤销旧止盈单 {n} 个")),
        Ok(_) => {}
        Err(e) => return Err(e.context("撤销旧止盈单失败")),
    }
    let (lines, placed_oids) = place_take_profits(exec, markets, tp_pct, reference).await?;
    oids.extend(placed_oids);
    log.extend(lines);
    Ok((log, oids))
}

/// 交易所允许的最小价位（和 `order_price` 的精度规则一致）。
fn price_tick(px: f64, sz_decimals: i32) -> f64 {
    let dp = (4 - px.log10().floor() as i32).min(6 - sz_decimals);
    10_f64.powi(-dp)
}

/// 止盈单的方向与挂单价。多头要「卖出」且挂在市价上方，空头要「买入」且挂在
/// 市价下方 —— 方向写反会变成加仓，价格写反会立刻成交变成市价单。
///
/// 低价币的价位精度可能不够（一个 tick 就超过止盈幅度），取整会把价格推到
/// 市价的错误一侧。这时至少推离一个 tick；推不动就返回 0，让调用方跳过该币。
pub fn take_profit_order(is_long: bool, mid: f64, tp_pct: f64, sz_decimals: i32) -> (bool, f64) {
    if !mid.is_finite() || mid <= 0.0 {
        return (true, 0.0);
    }
    let (buy, raw) = if is_long {
        (false, mid * (1.0 + tp_pct))
    } else {
        (true, mid * (1.0 - tp_pct))
    };
    if !raw.is_finite() || raw <= 0.0 {
        return (buy, 0.0);
    }
    let tick = price_tick(raw, sz_decimals);
    // 挂单要和 IOC 相反：取整方向朝「远离市价」，避免变成立即成交的市价单。
    let mut px = order_price(raw, sz_decimals, !buy);
    let right_side = |p: f64| p > 0.0 && if is_long { p > mid } else { p < mid };
    for _ in 0..4 {
        if right_side(px) {
            return (buy, px);
        }
        px = if is_long { px + tick } else { px - tick };
    }
    if right_side(px) { (buy, px) } else { (buy, 0.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_profit_sells_above_market_for_longs() {
        let (buy, px) = take_profit_order(true, 100.0, 0.10, 2);
        assert!(!buy, "多头止盈必须是卖出");
        assert!(px > 100.0, "多头止盈必须挂在市价上方，实际 {px}");
    }

    #[test]
    fn take_profit_buys_below_market_for_shorts() {
        let (buy, px) = take_profit_order(false, 100.0, 0.10, 2);
        assert!(buy, "空头止盈必须是买入");
        assert!(px < 100.0, "空头止盈必须挂在市价下方，实际 {px}");
    }

    #[test]
    fn take_profit_stays_on_the_correct_side_after_rounding() {
        for &mid in &[0.00001234_f64, 0.5, 3.14159, 87.65, 1234.5, 98765.4] {
            for &sd in &[0i32, 1, 2, 4, 6] {
                let (buy, px) = take_profit_order(true, mid, 0.10, sd);
                assert!(!buy);
                assert!(px > mid, "多头 mid={mid} sd={sd} 得到 {px}");
                let (buy, px) = take_profit_order(false, mid, 0.10, sd);
                assert!(buy);
                assert!(px < mid, "空头 mid={mid} sd={sd} 得到 {px}");
            }
        }
    }

    #[test]
    fn take_profit_price_is_never_zero_or_negative() {
        for &mid in &[0.000001_f64, 1.0, 100.0] {
            for &tp in &[0.01, 0.1, 0.5] {
                for &sd in &[0i32, 2, 6] {
                    let (_, up) = take_profit_order(true, mid, tp, sd);
                    let (_, dn) = take_profit_order(false, mid, tp, sd);
                    assert!(up >= 0.0 && dn >= 0.0, "mid={mid} tp={tp} sd={sd} 得到 {up}/{dn}");
                    assert!(up > mid || up == 0.0, "多头挂单价必须高于市价: {up} vs {mid}");
                    assert!(dn < mid || dn == 0.0, "空头挂单价必须低于市价: {dn} vs {mid}");
                }
            }
        }
    }
}


impl LiveState {
    /// Current position unrealized PnL; separate from capital change and realized receipts.
    pub fn unrealized_pnl(&self, positions: &[crate::live::LivePosition]) -> f64 {
        positions.iter().map(|p| p.unrealized).sum()
    }
}


/// Import exchange close-fill receipts. An unmatched fill does not establish
/// order intent: it may be a split rebalance fill, a manual close or a trigger.
pub async fn reconcile_fills(exec: &Exec, state: &mut LiveState) -> Result<usize> {
    let v = exec.user_fills(500).await?;
    let arr = v.as_array().context("成交回执格式错误")?;
    reconcile_fill_rows(arr, state)
}

fn reconcile_fill_rows(arr: &[serde_json::Value], state: &mut LiveState) -> Result<usize> {
    // 首次对账绝不能回溯：水位为 0 时直接对齐到「现在」，导入 0 笔。
    //
    // 否则 userFills 会把账户有史以来的成交全倒进来 —— 包括这个策略上线前的
    // 几个月、上一套策略的、甚至别的品种的，已实现盈亏会瞬间变成 −623 这种
    // 荒谬数字。这个错误真实发生过。
    if state.reconciled_to == 0 {
        state.reconciled_to = now_ms_pub();
        return Ok(0);
    }
    let mut known: std::collections::HashSet<u64> =
        state.records.iter().filter_map(|r| r.tid).collect();
    // 我们自己下的单在记录里没有 tid（下单回执里就没有），所以**只靠 tid 去重
    // 不够** —— 每次调仓的平仓都会被当成"被动成交"再导入一遍，盈亏双计，
    // 前端也会把换仓写成"止盈"。这里用 (币, 数量, 时间) 再兜一层。
    let ours: Vec<(String, f64, i64)> = state
        .records
        .iter()
        .filter(|r| r.tid.is_none())
        .map(|r| (r.coin.clone(), r.size.abs(), r.ts))
        .collect();
    let watermark = state.reconciled_to;
    let mut added = 0usize;
    let mut max_ts = watermark;

    // ★ 先按【交易所订单号 oid】把成交分组，再用【组总额】去比我们自己的汇总订单。
    //
    // 真实 bug：一笔 85 AERO 的调仓被交易所拆成 13/14/14/2/14… 共 7 笔成交。
    // 我们自己的记录是【汇总的 85】，而原来的逐笔去重拿 13 去比 85，差 72，
    // 远超 1% 容差，于是整组被判成"不是重复"全部导入 —— 实测名义被重复计算
    // 约 9.4%（$2,156 / $22,875），换手与成本指标跟着虚高。
    //
    // 同一笔订单的所有成交共享 oid，所以按 oid 汇总出来的总额才和我们的汇总记录
    // 可比。没有 oid 的成交用 tid 当键（每笔自成一组），行为与原来一致。
    let mut groups: std::collections::BTreeMap<(String, u64), Vec<&serde_json::Value>> =
        std::collections::BTreeMap::new();
    for f in arr {
        let ts = f["time"].as_i64().unwrap_or(0);
        if ts <= watermark {
            continue;
        }
        let Some(tid) = f["tid"].as_u64() else { continue };
        if known.contains(&tid) {
            continue;
        }
        let coin = f["coin"].as_str().unwrap_or("").to_string();
        // 只认主 DEX 的币：HIP-3 的名字带 ':'，我们从不交易它们。
        if coin.is_empty() || coin.contains(':') {
            continue;
        }
        let key = f["oid"].as_u64().unwrap_or(tid);
        groups.entry((coin, key)).or_default().push(f);
    }

    for ((coin, oid), fills) in groups {
        // 这个 oid 是我们自己挂的止盈单吗？是 → 成交标成「止盈成交」✗
        // 否则标「交易所成交补录」✗ 用户才分得清是止盈还是手动/被动平仓 ✗
        let is_tp = oid != 0 && state.tp_oids.contains(&oid);
        // 这一笔交易所订单【总共】成交了多少
        let total: f64 = fills
            .iter()
            .map(|f| {
                f["sz"]
                    .as_str()
                    .and_then(|x| x.parse::<f64>().ok())
                    .unwrap_or(0.0)
                    .abs()
            })
            .sum();
        let group_ts = fills
            .iter()
            .map(|f| f["time"].as_i64().unwrap_or(0))
            .max()
            .unwrap_or(0);
        // 我们自己发过的单（汇总一条）只要和这一【整组】对得上，整组跳过。
        let dupe = ours
            .iter()
            .any(|(c, s, t)| {
                c == &coin
                    && (s - total).abs() <= 1e-9_f64.max(total * 0.01)
                    && (t - group_ts).abs() <= 180_000
            });
        if dupe {
            continue;
        }
        // 【按交易订单合并】✗ 交易所一笔订单会被拆成很多条成交 ✗
        // 逐条记录会让页面出现 8 行 MON ✗ 和交易所开着 Aggregate 的视图对不上 ✗
        // 这里把同一 oid 的成交合并成一条 ✗ 数量相加 ✗ 成交价按名义加权 ✗
        let mut total_sz = 0.0f64;
        let mut total_notional = 0.0f64;
        let mut total_pnl = 0.0f64;
        let mut last_ts = 0i64;
        let mut first_tid: Option<u64> = None;
        let mut side_buy = false;
        let mut any = false;
        for f in fills {
            let ts = f["time"].as_i64().unwrap_or(0);
            let Some(tid) = f["tid"].as_u64() else { continue };
            // 组内也要查 known：没有 oid 的成交用 tid 当分组键，同一 tid 的
            // 重复行会落进同一组，必须在导入前再挡一次（旧代码是逐笔挡的）。
            if known.contains(&tid) {
                continue;
            }
            let sz: f64 = f["sz"].as_str().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0).abs();
            let px: f64 = f["px"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0.0);
            let dir = f["dir"].as_str().unwrap_or("");
            let pnl: f64 = f["closedPnl"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0.0);
            // 只补「不是我们主动发单」的成交；我们自己发的单在执行时已经记过了。
            if !dir.contains("Close") {
                continue;
            }
            total_sz += sz;
            total_notional += sz * px;
            total_pnl += pnl;
            last_ts = last_ts.max(ts);
            side_buy = f["side"].as_str() == Some("B");
            if first_tid.is_none() {
                first_tid = Some(tid);
            }
            known.insert(tid);
            any = true;
        }
        if any && total_sz > 0.0 {
            let avg_px = total_notional / total_sz;
            state.records.push(LiveRecord {
                ts: last_ts,
                coin: coin.clone(),
                side: if side_buy { "买".into() } else { "卖".into() },
                action: if is_tp { "止盈成交".into() } else { "交易所成交补录".into() },
                size: total_sz,
                price: avg_px,
                notional: total_notional,
                result: if is_tp {
                    format!("止盈单成交 · 已实现 {:+.2}", total_pnl)
                } else {
                    format!("交易所回执 · 已实现 {:+.2}", total_pnl)
                },
                live: true,
                pnl: Some(total_pnl),
                tid: first_tid,
                entry_px: None,
                reduce_only: true,
            });
            added += 1;
            if last_ts > max_ts {
                max_ts = last_ts;
            }
        }
    }
    if added > 0 || max_ts > watermark {
        state.reconciled_to = max_ts;
    }
    Ok(added)
}




/// 影子回测：从实盘开始那天起，用同一套信号、同样的成本假设重算一遍，
/// 看模型"应该"赚多少。
///
/// 用途是拿它和实盘的真实净值曲线并排看：如果实盘明显跑输影子，说明差额来
/// 自执行（滑点、成交质量、时机），而不是信号 —— 那是回测给不了的信息。
///
/// 成本假设与回测一致：双边 × (taker 费率 + 配置里的滑点上限)。
pub fn shadow_curve(store: &crate::store::Store, from_ts: i64, cfg: &LiveConfig) -> Vec<(i64, f64)> {
    let Ok(panel) = crate::trader::load_panel(store) else {
        return Vec::new();
    };
    let tc = cfg.trade_config();
    let fp = crate::trader::FactorPanel::build(&panel);
    if fp.is_empty() {
        return Vec::new();
    }
    let day = 86_400_000i64;
    let start = from_ts / day * day;
    let mut eq = 1.0f64;
    let mut out: Vec<(i64, f64)> = Vec::new();
    let mut prev_w: Vec<(String, f64)> = Vec::new();
    let mut last_close: std::collections::HashMap<String, f64> = Default::default();
    // 与纸面回测同一套完整性判定：残缺日（只有部分币有日线）不参与，
    // 且绝不越过最后一个完整日。否则影子曲线尾部会被陈旧价格污染。
    let complete = fp.complete_days();
    let Some(last_ok) = (0..fp.len()).rev().find(|&i| complete[i]) else {
        return Vec::new();
    };
    let mut idx = 30usize;
    while idx + 1 <= last_ok {
        let t = fp.ts[idx];
        if !complete[idx] {
            idx += 1;
            continue;
        }
        if t < start {
            idx += 1;
            continue;
        }
        // 先按昨日持仓结算今日涨跌
        let mut ret = 0.0f64;
        for (coin, w) in &prev_w {
            let c_now = fp.close_at(coin, fp.ts[idx + 1]);
            let c_prev = last_close.get(coin).copied();
            if let (Some(a), Some(b)) = (c_now, c_prev) {
                if b > 0.0 {
                    ret += w * (a / b - 1.0);
                }
            }
        }
        // 再按信号调仓
        let new = fp.weights_at(idx, &tc, 1_000.0);
        if !new.is_empty() {
            let mut turn = 0.0f64;
            let mut map: std::collections::HashMap<String, f64> = Default::default();
            for (c, x) in &new {
                *map.entry(c.clone()).or_insert(0.0) += x;
            }
            let mut keys: std::collections::HashSet<String> = Default::default();
            for (c, _) in prev_w.iter().chain(new.iter()) {
                keys.insert(c.clone());
            }
            for c in &keys {
                let a = prev_w.iter().find(|(x, _)| x == c).map(|(_, v)| *v).unwrap_or(0.0);
                let b = map.get(c).copied().unwrap_or(0.0);
                turn += (b - a).abs();
            }
            ret -= turn / 2.0 * 2.0 * (0.00045 + cfg.slippage);
            prev_w = new;
        }
        eq *= 1.0 + ret;
        for (c, _) in &prev_w {
            if let Some(v) = fp.close_at(c, fp.ts[idx + 1]) {
                last_close.insert(c.clone(), v);
            }
        }
        out.push((t, eq));
        idx += 1;
    }
    out
}

#[cfg(test)]
mod config_parity_tests {
    use super::*;

    /// 实盘配置的每个可调字段都必须真的传进 TradeConfig。
    ///
    /// 之前用 `..Default::default()` 兜底，漏传 `rebalance_slices`，
    /// 结果是界面上改了参数、实盘行为完全不变，而且不留任何痕迹。
    #[test]
    fn every_live_field_reaches_trade_config() {
        let cfg = LiveConfig {
            lookback: 7,
            top_frac: 0.11,
            min_vol_usd: 7_000_000.0,
            leverage: 5.0,
            target_positions: 13,
            slippage: 0.007,
            margin_buffer: 0.77,
            rebalance_slices: 4,
            momentum_weight: 0.7,
            take_profit_pct: 0.1,
            ..LiveConfig::default()
        };
        let tc = cfg.trade_config();
        assert_eq!(tc.lookback, 7, "lookback 没传");
        assert_eq!(tc.top_frac, 0.11, "top_frac 没传");
        assert_eq!(tc.min_vol_usd, 7_000_000.0, "min_vol_usd 没传");
        assert_eq!(tc.leverage, 5.0, "leverage 没传");
        assert_eq!(tc.target_positions, 13, "target_positions 没传");
        assert_eq!(tc.slippage, 0.007, "slippage 没传");
        assert_eq!(tc.margin_buffer, 0.77, "margin_buffer 没传");
        assert_eq!(tc.rebalance_slices, 4, "rebalance_slices 没传（UI 会失效）");
        assert_eq!(tc.momentum_weight, 0.7, "momentum_weight 没传（UI 会失效）");
        assert_eq!(tc.take_profit_pct, 0.1, "take_profit_pct 没传（止盈会失效）");
    }
}


#[cfg(test)]
mod config_robustness {
    use super::*;

    /// 配置文件里缺字段时，**不能**把 account / key_path 静默清空。
    ///
    /// 真实事故：LiveConfig 里 11 个字段没有 #[serde(default)]，只要旧配置少任何一个，
    /// 整个结构反序列化失败并回退默认值 —— 账户地址和密钥路径变成空字符串，
    /// 页面显示"未启用 / 净值 $0.00"，而没有任何报错。
    #[test]
    fn missing_fields_do_not_wipe_the_account() {
        // 只给必填的两个字段，其余全缺
        let json = r#"{"account":"0xABC","key_path":"/tmp/k.key"}"#;
        let cfg: LiveConfig = serde_json::from_str(json).expect("缺字段不应导致解析失败");
        assert_eq!(cfg.account, "0xABC", "account 被清空了");
        assert_eq!(cfg.key_path, "/tmp/k.key", "key_path 被清空了");
        // 其余字段走默认值
        assert_eq!(cfg.leverage, LiveConfig::default().leverage);
        assert_eq!(cfg.rebalance_slices, 3);
    }

    #[test]
    fn empty_object_still_parses() {
        let cfg: LiveConfig = serde_json::from_str("{}").expect("空对象也应能解析");
        assert_eq!(cfg.account, "");
    }
}


/// 记录一个净值点。**判重和写入必须在同一把锁里**。
///
/// 之前是在页面 handler 里用「进入 handler 时拿到的克隆」判断这个小时是否已记录，
/// 然后才加锁写入 —— 两个并发请求都会认为"该小时还没记"，于是同一分钟写进两三个点。
/// 而且它只在页面被访问时才写，不看页面就不记，曲线缺失掉整段时间。
/// 现在统一走这里：锁内判重、写入、裁剪、落盘。
pub async fn record_equity(
    live: &std::sync::Arc<tokio::sync::Mutex<LiveState>>,
    path: &std::path::Path,
    account: &str,
    equity: f64,
    pnl: f64,
) -> Result<()> {
    if !equity.is_finite() || equity < 0.0 || !pnl.is_finite() {
        anyhow::bail!("账户净值或盈亏不是有效数值");
    }
    let now = now_ms_pub();
    let hour = now / 3_600_000;
    let mut g = live.lock().await;
    anyhow::ensure!(g.config.account.eq_ignore_ascii_case(account), "账户已改变，丢弃旧账户净值");
    let previous = g.clone();
    let mut changed = false;
    if g.capital_baseline.is_none() {
        g.capital_baseline = g.baseline().or_else(|| (equity > 0.).then(|| CapitalBaseline {
            account: account.into(), equity, ts: Some(now), source: "首次记录净值".into(),
        }));
        if let Some(b) = &g.capital_baseline { g.start_equity = Some(b.equity); changed = true; }
    }
    // 锁内判重：同一小时内已有记录就不再写。
    if g.history.last().map(|p| p.ts / 3_600_000) == Some(hour) {
        if changed { if let Err(e) = g.save(path) { *g = previous; return Err(e.context("起始资金保存失败")); } }
        return Ok(());
    }
    g.history.push(EquityPoint { ts: now, equity, pnl });
    let len = g.history.len();
    if len > 6000 {
        g.history.drain(0..len - 6000);
    }
    if let Err(e) = g.save(path) { *g = previous; return Err(e.context("净值记录保存失败")); }
    Ok(())
}


/// 从交易所读一次账户并记一个净值点 —— 给后台定时器用，与页面是否打开无关。
///
/// 这是净值曲线**唯一可靠的来源**：只靠 handler 记录的话，不看页面就不记，
/// 曲线会缺掉整段时间，而它恰恰是判断策略赚亏的唯一视图。
pub async fn record_equity_now(
    live: &std::sync::Arc<tokio::sync::Mutex<LiveState>>,
    path: &std::path::Path,
) -> Result<()> {
    let st = live.lock().await.clone();
    if st.config.account.is_empty() {
        return Ok(());
    }
    let exec = Exec::reader_config(&st.config).await?;
    let acct = exec.account().await?;
    let unreal: f64 = acct.positions.values().map(|p| p.unrealized_pnl).sum();
    record_equity(live, path, &st.config.account, acct.equity, unreal).await?;
    Ok(())
}

#[cfg(test)]
mod reconciliation_tests {
    use super::*;
    #[test]
    fn reconciliation_preserves_concurrent_orders_and_deduplicates_fills() {
        let record=|tid:u64| LiveRecord{ts:tid as i64,coin:"C".into(),side:"买".into(),action:"开".into(),size:1.,price:100.,notional:100.,result:"ok".into(),live:true,pnl:None,tid:Some(tid),entry_px:None,reduce_only:false};
        let mut current=LiveState::default();current.records.push(record(1));let mut snapshot=current.clone();
        current.records.push(record(2));snapshot.records.push(record(3));
        merge_reconciliation(&mut current,&snapshot);
        assert_eq!(current.records.len(),3,"stale snapshot must not erase a concurrent order");
        merge_reconciliation(&mut current,&snapshot);assert_eq!(current.records.len(),3);
    }
}

#[cfg(test)]
mod hl_signal_tests {
    use super::*;
    #[test]
    fn nonempty_weights_do_not_require_extra_candidate_limits() {
        let one = vec![("A".into(), 1.)];
        assert!(ensure_candidates(&one).is_ok());
    }
    #[test]
    fn signal_panel_keeps_the_hl_store_bars() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let hl = crate::store::Store::open(&root.join("hl")).unwrap();
        let yesterday = now_ms_pub()/86_400_000*86_400_000-86_400_000;
        for i in 0..24 {
            let candles:Vec<_>=(0..40).map(|d|crate::hl::Candle{t:yesterday-(40-d)*86_400_000,o:100.,h:100.,l:100.,c:100.,v:6000000.}).collect();
            hl.upsert_candles(&format!("C{i}"), &candles).unwrap();
        }
        assert_eq!(signal_panel(&hl).unwrap().len(), 24);
        drop(hl); std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn signal_panel_uses_the_hl_universe() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let hl = crate::store::Store::open(&root.join("hl")).unwrap();
        let yesterday=now_ms_pub()/86400000*86400000-86400000;
        let candles:Vec<_>=(0..40).map(|d|crate::hl::Candle{t:yesterday-(39-d)*86400000,o:2.,h:2.,l:2.,c:2.,v:6000000.}).collect();
        hl.upsert_candles("HL_ONLY", &candles).unwrap();
        assert_eq!(signal_panel(&hl).unwrap()[0].coin, "HL_ONLY");
        drop(hl); std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn fractional_leverage_is_preserved() {
        let hl = LiveConfig { leverage: 3.49, ..Default::default() };
        assert_eq!(hl.trade_config().leverage, 3.49);
    }
}

#[cfg(test)]
mod min_order_pct_tests {
    use super::*;

    /// 配置字段漏传是踩过的坑（`rebalance_slices` 曾经被 `..Default::default()` 静默吞掉）。
    /// 这条确保 `min_order_pct` 真的会到 TradeConfig。
    #[test]
    fn min_order_pct_reaches_trade_config() {
        let mut cfg = LiveConfig::default();
        cfg.min_order_pct = 0.02;
        assert_eq!(cfg.trade_config().min_order_pct, 0.02);
    }

    /// 有效门槛 = max(绝对下限, 净值 × 比例)。
    #[test]
    fn effective_threshold_scales_with_equity() {
        let mut cfg = TradeConfig::default();
        cfg.min_order_usd = 10.0;
        // 比例 0 → 与旧版一致
        cfg.min_order_pct = 0.0;
        assert_eq!(cfg.effective_min_order_usd(100_000.0), 10.0);
        // 比例 1% × 大账号 → 比例项接管
        cfg.min_order_pct = 0.01;
        assert_eq!(cfg.effective_min_order_usd(2900.0), 29.0);
        // 小账号 → 绝对下限仍然接管，不会低到无法下单
        assert_eq!(cfg.effective_min_order_usd(585.0), 10.0);
    }
}

#[cfg(test)]
mod capital_regressions {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn legacy_baseline_migrates_before_hour_dedup_and_survives_clear_restart() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let path = dir.join("live.json");
        let now = now_ms_pub();
        let mut old = LiveState::default();
        old.config.account = "account-a".into();
        old.history = vec![EquityPoint { ts: now - 3_600_000, equity: 500., pnl: 0. },
            EquityPoint { ts: now, equity: 569.09, pnl: 9.03 }];
        let state = Arc::new(Mutex::new(old));
        record_equity(&state, &path, "account-a", 569.09, 9.03).await.unwrap();
        let persisted = LiveState::load(&path);
        assert_eq!(persisted.baseline().unwrap().equity, 500.);
        assert_eq!(persisted.history.len(), 2);
        { let mut s = state.lock().await; s.history.clear(); s.records.clear(); s.save(&path).unwrap(); }
        let restarted = Arc::new(Mutex::new(LiveState::load(&path)));
        record_equity(&restarted, &path, "account-a", 600., 100.).await.unwrap();
        assert_eq!(restarted.lock().await.baseline().unwrap().equity, 500.);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn baseline_is_observed_not_inferred_and_rejects_old_account_and_failed_save() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let path = dir.join("live.json");
        let mut initial = LiveState::default(); initial.config.account = "a".into();
        assert!(initial.baseline().is_none());
        let state = Arc::new(Mutex::new(initial));
        record_equity(&state, &path, "a", 569.09, 9.03).await.unwrap();
        assert_eq!(state.lock().await.baseline().unwrap().equity, 569.09);
        assert!(record_equity(&state, &path, "a", f64::INFINITY, 0.).await.is_err());
        { let mut s = state.lock().await; s.config.account = "b".into(); s.reset_capital_for_account_change("a"); }
        assert!(record_equity(&state, &path, "a", 1000., 0.).await.is_err());
        assert!(state.lock().await.baseline().is_none());
        record_equity(&state, &path, "b", 200., 0.).await.unwrap();
        assert_eq!(state.lock().await.baseline().unwrap().equity, 200.);
        { let mut s = state.lock().await; s.history.clear(); }
        record_equity(&state, &path, "b", 0., 0.).await.unwrap();
        assert_eq!(state.lock().await.history.last().unwrap().equity, 0.);
        let fresh = Arc::new(Mutex::new(LiveState::default()));
        assert!(record_equity(&fresh, &dir, "", 100., 0.).await.is_err());
        assert!(fresh.lock().await.baseline().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saved_baseline_wins_over_trimmed_history_and_cannot_cross_accounts() {
        let mut s = LiveState::default();
        s.start_equity = Some(500.);
        s.history.push(EquityPoint { ts: 123, equity: 700., pnl: 0. });
        let b = s.baseline().unwrap(); assert_eq!(b.equity, 500.); assert!(b.ts.is_none());
        s.capital_baseline = Some(b); s.config.account = "other".into();
        assert!(s.baseline().is_none());
    }


}

#[cfg(test)]
mod monitor_receipt_tests {
    use super::*;
    #[test]
    fn split_close_receipts_are_not_assumed_to_be_take_profits() {
        let mut state = LiveState::default(); state.reconciled_to = 100;
        let rows = vec![
            serde_json::json!({"time":200,"tid":1,"coin":"AERO","sz":"57","px":"0.80177","dir":"Close Short","side":"B","closedPnl":"-1.27"}),
            serde_json::json!({"time":200,"tid":2,"coin":"AERO","sz":"46","px":"0.80171","dir":"Close Short","side":"B","closedPnl":"-1.03"}),
            serde_json::json!({"time":200,"tid":2,"coin":"AERO","sz":"46","px":"0.80171","dir":"Close Short","side":"B","closedPnl":"-1.03"}),
        ];
        assert_eq!(reconcile_fill_rows(&rows, &mut state).unwrap(), 2);
        assert!(state.records.iter().all(|r| r.action == "交易所成交补录" && !r.result.contains("被动") && r.reduce_only && r.side == "买"));
        assert!((state.records.iter().map(|r| r.pnl.unwrap()).sum::<f64>() + 2.30).abs() < 1e-8);
        assert_eq!(reconcile_fill_rows(&rows, &mut state).unwrap(), 0);
    }
    #[test]
    fn first_receipt_watermark_survives_merge_without_fills() {
        let mut current = LiveState::default(); let mut snapshot = current.clone();
        assert_eq!(reconcile_fill_rows(&[], &mut snapshot).unwrap(), 0);
        merge_reconciliation(&mut current, &snapshot);
        assert!(current.reconciled_to > 0);
    }
}


#[cfg(test)]
mod capital_flow_tests {
    use super::*;

    #[test]
    fn cash_flows_follow_the_account_baseline() {
        let mut st = LiveState::default();
        st.config.account = "a".into();
        st.net_deposit = 100.0;
        st.capital_flows.push(CapitalFlow { ts: 1, effective_ts: None, amount: 100.0, note: String::new() });
        st.reset_capital_for_account_change("A");
        assert_eq!(st.net_deposit, 100.0);
        st.config.account = "b".into();
        st.reset_capital_for_account_change("a");
        assert_eq!(st.net_deposit, 0.0);
        assert!(st.capital_flows.is_empty());
    }

    /// 充钱只会增加 net_deposit，绝不改变"策略赚出来的部分"。
    /// 入金不改变净收益金额；简单收益率会因投入本金增加而变化。
    #[test]
    fn deposit_does_not_change_trading_pnl() {
        let mut st = LiveState::default();
        st.start_equity = Some(1000.0);
        st.net_deposit = 0.0;
        let equity_before = 1100.0; // 策略赚了 100
        let trading_before = equity_before - st.net_deposit - st.start_equity.unwrap();
        assert!((trading_before - 100.0).abs() < 1e-9);

        // 充 500
        st.net_deposit += 500.0;
        let equity_after = equity_before + 500.0;
        let trading_after = equity_after - st.net_deposit - st.start_equity.unwrap();
        assert!((trading_after - 100.0).abs() < 1e-9,
            "入金后策略赚出的部分变了：{trading_after}");
    }
}


#[cfg(test)]
mod fill_dedup_tests {
    use super::*;
    use serde_json::json;

    fn recorded_order(ts: i64, coin: &str, size: f64) -> LiveRecord {
        LiveRecord {
            ts,
            coin: coin.into(),
            side: "卖".into(),
            action: "减仓到目标".into(),
            size,
            price: 0.9455,
            notional: size * 0.9455,
            result: format!("卖 {coin} 只减仓 {size}"),
            live: true,
            pnl: None,
            tid: None, // 我们自己发的单在回执里没有 tid
            entry_px: None,
            reduce_only: true,
        }
    }

    fn close_fill(t: i64, tid: u64, oid: u64, sz: f64) -> serde_json::Value {
        json!({
            "time": t, "tid": tid, "oid": oid, "coin": "AERO",
            "dir": "Close Long", "side": "A",
            "sz": format!("{sz}"), "px": "0.9455", "closedPnl": "0.10"
        })
    }

    /// ★ 真实事故回归：一笔 85 AERO 的调仓被交易所拆成 13/14/14/2/14… 多笔成交。
    /// 我们的记录是【汇总的 85】。旧代码逐笔拿 13 去比 85，差太远，于是整组被当成
    /// "不是重复"全部导入，名义重复计算约 9.4%。
    /// 现在按 oid 分组，用组总额 85 去比 → 整组跳过。
    #[test]
    fn split_fills_matching_our_aggregate_order_are_not_imported() {
        let t0 = 1_700_000_000_000i64;
        let mut st = LiveState::default();
        st.reconciled_to = t0 - 1_000;
        st.records.push(recorded_order(t0, "AERO", 85.0));
        // 同一笔订单（oid=999）被拆成 60 + 20 + 5 = 85
        let arr = vec![
            close_fill(t0, 1, 999, 60.0),
            close_fill(t0, 2, 999, 20.0),
            close_fill(t0, 3, 999, 5.0),
        ];
        let added = reconcile_fill_rows(&arr, &mut st).unwrap();
        assert_eq!(added, 0, "整组 85 已被我们的汇总记录覆盖，不应再导入");
        assert_eq!(st.records.len(), 1, "不应新增补录记录");
    }

    /// 反向：交易所上有一笔我们【没有】记录过的成交（比如被强平），
    /// 必须仍然被导入 —— 否则已实现盈亏会漏。
    #[test]
    fn fills_with_no_matching_order_are_still_imported() {
        let t0 = 1_700_000_000_000i64;
        let mut st = LiveState::default();
        st.reconciled_to = t0 - 1_000;
        // 我们没有任何 AERO 记录
        let arr = vec![
            close_fill(t0, 11, 777, 30.0),
            close_fill(t0, 12, 777, 10.0),
        ];
        let added = reconcile_fill_rows(&arr, &mut st).unwrap();
        // 同一 oid 的两条成交合并成一条记录（和交易所的 Aggregate 视图一致 ✗）
        assert_eq!(added, 1, "同一笔订单的成交应合并为一条记录");
        assert_eq!(st.records.len(), 1);
        let r = &st.records[0];
        assert!((r.size - 40.0).abs() < 1e-9, "数量应相加，实际 {}", r.size);
        // 每条 fill 的 closedPnl 都是 0.10 → 两条合并应得 0.20
        assert!((r.pnl.unwrap_or(0.0) - 0.2).abs() < 1e-9, "已实现应收拢，实际 {:?}", r.pnl);
    }
}
