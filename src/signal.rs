use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// 正费率触发阈值：0.05%
pub const FUNDING_THRESHOLD_STR: &str = "0.0005";

/// 单次交易成本估算（%）
pub const TRADE_COST_PCT: &str = "0.15";

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

/// 把 kind 转换为可读的中文标签
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "crash_pos_fund" => "暴跌+正费率",
        "down_pos_fund" => "下跌+正费率",
        "up_pos_fund" => "上涨+正费率",
        "pump_pos_fund" => "暴涨+正费率",
        _ => "未知",
    }
}
