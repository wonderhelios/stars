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


mod executor;
mod factors;
mod planner;

pub use executor::{execute, fetch_mids, load_panel};
pub use factors::{target_weights, FactorPanel};
pub use planner::{
    apply_hard_gross_cap, build_plan, cap_weights_same_side, neutralize_orders,
    neutralize_orders_with_targets,
};
#[cfg(test)]
use planner::slice_of;

#[cfg(test)]
mod tests;
