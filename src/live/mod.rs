//! Live trading domain: account state, TWR metrics, rebalance runner, and exchange reconciliation.

use crate::exchange::MarketInfo;
use std::collections::HashMap;

mod metrics;
mod orders;
mod runner;
mod state;

pub use metrics::{adjusted_history, daily_execution, index_history, ml_index_history, scorecard};
pub use orders::{cancel_our_take_profits, place_take_profits, rebuild_cross, reconcile_fills, refresh_take_profits};
#[cfg(test)]
use orders::take_profit_order;
pub use runner::{current_book_vol, ml_account_snapshot, record_equity, record_equity_now, record_ml_equity, run, snapshot};
pub use state::{
    merge_reconciliation, CapitalAlloc, CapitalBaseline, CapitalFlow, DailyExec, EquityPoint,
    LiveConfig, LivePosition, LiveRecord, LiveSnapshot, LiveState, MlAccountSnapshot,
};

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

#[cfg(test)]
mod tests;
