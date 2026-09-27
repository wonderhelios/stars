use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// 全市场行情快照。服务端内存中每个 inst_id 一条，实时更新。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub inst_id: String,
    pub base: String,
    pub quote: String,

    pub last: Decimal,
    pub bid: Decimal,
    pub ask: Decimal,
    pub open_24h: Decimal,
    pub high_24h: Decimal,
    pub low_24h: Decimal,
    /// 以计价币（USDT）计量的 24h 成交额
    pub volume_quote_24h: Decimal,

    /// 当期资金费率（小数，如 0.0001 = 0.01%）
    pub funding_rate: Option<Decimal>,
    /// 下次结算时间（毫秒）
    pub next_funding_time: Option<i64>,
    /// 最近一次收到资金费率的时间（毫秒）
    pub funding_ts: i64,

    /// 最近更新时间（毫秒）
    pub ts: i64,
}

impl MarketSnapshot {
    pub fn empty_for(inst_id: &str) -> Self {
        let mut parts = inst_id.split('-');
        let base = parts.next().unwrap_or("").to_string();
        let quote = parts.next().unwrap_or("").to_string();

        Self {
            inst_id: inst_id.to_string(),
            base,
            quote,
            last: Decimal::ZERO,
            bid: Decimal::ZERO,
            ask: Decimal::ZERO,
            open_24h: Decimal::ZERO,
            high_24h: Decimal::ZERO,
            low_24h: Decimal::ZERO,
            volume_quote_24h: Decimal::ZERO,
            funding_rate: None,
            next_funding_time: None,
            funding_ts: 0,
            ts: 0,
        }
    }
}
