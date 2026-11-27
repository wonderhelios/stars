//! Persistent live configuration, account state, and snapshot DTOs.

use crate::trader::TradeConfig;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Serialize, Deserialize)]
// 结构体级别的 default：反序列化时**从 LiveConfig::default() 开始**，文件里有什么就覆盖什么。
//
// 这比逐字段 #[serde(default)] 正确得多 —— 后者用的是「字段类型的默认值」
// （f64 → 0.0，bool → false），会让 leverage / margin_buffer 变成 0，
// 而这里会用真正想要的 3.0 / 0.9。字段缺失再也不会毁掉整个配置。
#[serde(default)]
pub struct LiveConfig {
    pub account: String,
    /// 子账户地址（vault address）。Some = 交易/查询都作用在这个子账户上；
    /// None = 用主账户 account。用于「两个子账户分别跑不同策略」。
    #[serde(default)]
    pub vault_address: Option<String>,
    /// ML 策略的子账户地址（第二个子账户）。只读监控用：live_status 会查这个
    /// 子账户的净值/仓位，与动量子账户并排展示 + 合计总资金。
    #[serde(default)]
    pub ml_vault_address: Option<String>,
    /// ML 服务用的 Python 可执行文件（conda ml 环境）。默认 "python"。
    #[serde(default = "d_ml_python")]
    pub ml_python: String,
    /// ML 脚本目录（predict.py/place.py 所在 ✗ 绝对路径最稳 ✗ 默认 "ml"）。
    #[serde(default = "d_ml_dir")]
    pub ml_dir: String,
    /// 目标日波动（0 = 关闭 ✗ 0.02 = 把组合日波动控制在约 2%）。
    /// 每天按 目标/实际 缩放仓位 ✗ 波动大时减仓 ✗ 小时加仓 ✓
    #[serde(default = "d_vol_target")]
    pub vol_target: f64,
    /// 波动目标的死区 ✗ 目标杠杆与当前杠杆偏离小于这个比例就不调仓 ✓（省手续费）
    #[serde(default = "d_vol_band")]
    pub vol_band: f64,
    /// 单币止损（0 = 关闭 ✗ 0.25 = 浮亏 25% 强制平 ✗）。
    #[serde(default = "d_stop_pct")]
    pub position_stop_pct: f64,
    /// 止损后的冷却天数（冷却期内不许再开同向仓 ✗）。
    #[serde(default = "d_stop_cool")]
    pub stop_cooldown_days: i64,
    /// 单币目标名义占账户净值的上限（0 = 关闭，默认 0.20 = 20%）。
    /// 作用在 vol_target 缩放后的最终目标权重上，超限部分在同侧其他币间等杠杆再分配。
    #[serde(default = "d_max_single_weight")]
    pub max_single_weight: f64,
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
    /// 错峰调仓档数：每个币每 N 天轮到一次。1 = 每日平滑/全量调仓（C3 默认 1）。
    #[serde(default = "default_slices")]
    pub rebalance_slices: u32,
    /// 平滑 EMA 调仓系数（0.1~1.0；1.0 = 一步到位）。仅当 `rebalance_slices == 1` 时生效。
    #[serde(default = "d_ema_alpha")]
    pub ema_alpha: f64,
    /// 同向微调忽略死区（占目标名义比例，0.0~0.50，默认 0.05 = 5%）。
    #[serde(default = "d_rebal_band")]
    pub rebal_band: f64,
    /// 总敞口硬上限（占账户净值倍数，默认 2.8 = 2.8x），防止低波动时 `vol_k` 放大超限。
    #[serde(default = "d_hard_gross_cap")]
    pub hard_gross_cap: f64,
    /// 止盈幅度（相对调仓时的中间价）。0 = 不挂止盈单。
    #[serde(default = "default_tp")]
    pub take_profit_pct: f64,
    /// 因子权重：动量占比（0~1，量能占其余）。默认 0.5 等权。
    /// 0.7 = 动量七成/量能三成（复盘检验支持的方向，尚未前向验证）。
    #[serde(default = "default_momentum_weight")]
    pub momentum_weight: f64,
    /// 三因子附加权重：距 14 日高点（0 = 关闭，1.0 = 与动量等权）。
    #[serde(default)]
    pub dd_weight: f64,
    /// 三因子附加权重：上涨天数占比（0 = 关闭，1.0 = 与动量等权）。
    #[serde(default)]
    pub upfrac_weight: f64,
    /// allow sending real orders; off until the user arms it
    pub armed: bool,
    /// run one rebalance per day automatically
    pub auto_run: bool,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            account: String::new(),
            vault_address: None,
            ml_vault_address: None,
            ml_python: d_ml_python(),
            ml_dir: d_ml_dir(),
            key_path: String::new(),
            target_positions: 8,
            leverage: 3.0,
            margin_buffer: 0.90,
            slippage: 0.005,
            lookback: 14,
            top_frac: 0.30,
            min_vol_usd: 5_000_000.0,
            min_order_pct: 0.005,
            take_profit_pct: 0.0,
            rebalance_slices: default_slices(),
            ema_alpha: d_ema_alpha(),
            rebal_band: d_rebal_band(),
            hard_gross_cap: d_hard_gross_cap(),
            momentum_weight: default_momentum_weight(),
            dd_weight: 0.0,
            upfrac_weight: 0.0,
            vol_target: d_vol_target(),
            vol_band: d_vol_band(),
            position_stop_pct: d_stop_pct(),
            stop_cooldown_days: d_stop_cool(),
            max_single_weight: d_max_single_weight(),
            armed: false,
            auto_run: false,
        }
    }
}

fn default_slices() -> u32 {
    // Phase 2 C3 默认：每日 EMA 平滑调仓（slices=1, ema_alpha=0.50）消除错峰相位运气与中性化互搏。
    1
}

fn d_ema_alpha() -> f64 {
    0.50
}

fn d_rebal_band() -> f64 {
    0.05
}

fn d_hard_gross_cap() -> f64 {
    2.8
}

fn default_momentum_weight() -> f64 {
    // 默认等权：不改旧行为。复盘检验支持 0.7（动量七成），但仍属样本内结论。
    0.5
}

fn d_ml_python() -> String {
    // ML 服务默认用 PATH 里的 python（部署时可在配置里改成 conda 环境的绝对路径）。
    "python".to_string()
}

fn d_ml_dir() -> String {
    // ML 脚本目录。部署时建议填绝对路径（如 /opt/stars/ml），否则依赖服务的工作目录。
    "ml".to_string()
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
            ema_alpha: self.ema_alpha.clamp(0.1, 1.0),
            rebal_band: self.rebal_band.clamp(0.0, 0.50),
            hard_gross_cap: self.hard_gross_cap.clamp(0.5, 5.0),
            // 这里漏掉过 rebalance_slices：`..Default::default()` 会静默用默认值，
            // 下面有 field_parity 测试守着。
            rebalance_slices: self.rebalance_slices,
            position_stop_pct: self.position_stop_pct.clamp(0.0, 0.9),
            momentum_weight: self.momentum_weight,
            dd_weight: self.dd_weight,
            upfrac_weight: self.upfrac_weight,
            take_profit_pct: self.take_profit_pct,
            max_single_weight: self.max_single_weight,
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

fn d_vol_band() -> f64 {
    0.20
}

fn d_vol_target() -> f64 {
    0.0
}

fn d_stop_pct() -> f64 {
    0.0
}

fn d_stop_cool() -> i64 {
    5
}

fn d_max_single_weight() -> f64 {
    0.20
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct LiveState {
    /// 止损冷却：币 -> 解禁时间（毫秒 ✗）
    #[serde(default)]
    pub stop_cooldowns: std::collections::HashMap<String, i64>,
    /// 上次调仓实际使用的波动目标缩放系数（0 = 还没用过 ✓）
    /// 死区要跟它比 ✗ 不能跟"当前杠杆"比 —— 入金会让当前杠杆被动变小 ✗
    /// 那样死区就会把新入金一直挡在门外 ✓
    #[serde(default)]
    pub vol_k: f64,
    #[serde(default)]
    pub capital_baseline: Option<CapitalBaseline>,
    pub config: LiveConfig,
    pub history: Vec<EquityPoint>,
    /// ML 子账户 B 的净值历史（默认全部账户曲线 = 动量 + ML 合并）。
    #[serde(default)]
    pub ml_history: Vec<EquityPoint>,
    /// ML 子账户出入金流水（用于计算 ML 真实 TWR 曲线，消除划转造成的假暴跌/假暴涨）。
    #[serde(default)]
    pub ml_capital_flows: Vec<CapitalFlow>,
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

    /// 统一按小时去重追加动量净值点（同一小时覆盖最新点，避免曲线重复时间戳与毛刺）。
    pub fn push_history_dedup(&mut self, ts: i64, equity: f64, pnl: f64) {
        if !equity.is_finite() || equity <= 0.0 || !pnl.is_finite() {
            return;
        }
        let hour = ts / 3_600_000;
        if self.history.last().map(|p| p.ts / 3_600_000) == Some(hour) {
            if let Some(last) = self.history.last_mut() {
                *last = EquityPoint { ts, equity, pnl };
            }
        } else {
            self.history.push(EquityPoint { ts, equity, pnl });
        }
        let len = self.history.len();
        if len > 6000 {
            self.history.drain(0..len - 6000);
        }
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

/// ML 策略子账户的只读快照（第二个子账户 ✗ 与动量子账户并排展示 + 合计）。
#[derive(Serialize)]
pub struct MlAccountSnapshot {
    pub account: String,
    pub equity: f64,
    pub positions: Vec<LivePosition>,
    pub error: Option<String>,
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
    /// 胜率（口径见字段注释 ✗ 三个维度分开算 ✓）
    pub win_rate_days: Option<f64>,
    pub win_rate_months: Option<f64>,
    pub win_rate_trades: Option<f64>,
    /// 交易日数 / 交易月数 / 平仓笔数（前端用来标注样本量）
    pub win_samples_days: usize,
    pub win_samples_months: usize,
    pub win_samples_trades: usize,
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
    /// ML 子账户 B 的净值历史（默认全部账户曲线 = 动量 + ML 合并）。
    #[serde(default)]
    pub ml_history: Vec<EquityPoint>,
    /// ML 子账户出入金流水（用于消除划转造成的假暴跌）。
    #[serde(default)]
    pub ml_capital_flows: Vec<CapitalFlow>,
    /// ML 累计净入金（由 ml_capital_flows 汇总）。
    #[serde(default)]
    pub ml_net_deposit: f64,
    /// 动量账户 TWR 净值指数（起点 = 1.0，跳过出入金阶跃）。
    #[serde(default)]
    pub mom_index_history: Vec<(i64, f64)>,
    /// ML 账户 TWR 净值指数（起点 = 1.0，跳过已记录流水及未登记的瞬时大额划转阶跃）。
    #[serde(default)]
    pub ml_index_history: Vec<(i64, f64)>,
    /// 动量账户策略累计盈亏（+/- USD，剔除本金与出入金）。
    #[serde(default)]
    pub mom_pnl_history: Vec<(i64, f64)>,
    /// ML 账户策略累计盈亏（+/- USD，剔除本金与划转）。
    #[serde(default)]
    pub ml_pnl_history: Vec<(i64, f64)>,
    pub records: Vec<LiveRecord>,
    pub last_run_at: Option<i64>,
    pub last_plan: Vec<String>,
    /// 上次调仓时各币的中间价。止盈幅度必须相对它计算，而不是相对当前价 ——
    /// 否则手动刷新会把止盈线随行情搬走（100 建仓、现价 80 的多头会被挂到 88）。
    pub tp_ref: HashMap<String, f64>,
    pub last_live: bool,
    /// ML 策略子账户（第二个子账户）的快照。None = 没配置 ml_vault_address。
    #[serde(default)]
    pub ml_account: Option<MlAccountSnapshot>,
}


