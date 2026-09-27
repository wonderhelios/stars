use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// 正费率触发阈值：0.05%
pub const FUNDING_THRESHOLD_STR: &str = "0.0005";

/// 单次交易成本估算（%）
pub const TRADE_COST_PCT: &str = "0.15";
pub const OBSERVATION_WINDOW_MS: i64 = 24 * 3_600_000;

pub const MINUTE_MS: i64 = 60_000;

/// 第一根在目标时刻或之后收完的 1 分钟 K 线开盘时间。
pub fn outcome_bar_open(target_ts: i64) -> i64 {
    (target_ts + MINUTE_MS - 1).div_euclid(MINUTE_MS) * MINUTE_MS - MINUTE_MS
}

/// 只读取已经收完并留出几秒供交易所发布的 K 线。
pub fn outcome_bar_ready(target_ts: i64, now_ts: i64) -> bool {
    now_ts >= outcome_bar_open(target_ts) + MINUTE_MS + 5_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub inst_id: String,
    pub kind: String,
    pub triggered_at: i64,
    pub funding_rate: Decimal,
    pub prior_24h_return: Decimal,
    pub entry_price: Decimal,
}

/// 判定信号类别。返回 None 表示不是信号。
pub fn classify(prior_24h_pct: Decimal, funding: Decimal) -> Option<&'static str> {
    if funding <= Decimal::ZERO {
        return None;
    }

    let pct = prior_24h_pct;
    let neg10 = Decimal::from(-10);
    let neg3 = Decimal::from(-3);
    let pos3 = Decimal::from(3);
    let pos10 = Decimal::from(10);

    if pct < neg10 {
        Some("crash_pos_fund")
    } else if pct < neg3 {
        Some("down_pos_fund")
    } else if pct >= pos3 && pct < pos10 {
        Some("up_pos_fund")
    } else if pct >= pos10 {
        Some("pump_pos_fund")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_uses_first_completed_minute_after_target() {
        assert_eq!(outcome_bar_open(3_600_000), 3_540_000);
        assert_eq!(outcome_bar_open(3_600_001), 3_600_000);
        assert_eq!(outcome_bar_open(3_659_999), 3_600_000);
        assert!(!outcome_bar_ready(3_600_001, 3_664_999));
        assert!(outcome_bar_ready(3_600_001, 3_665_000));
    }
}
