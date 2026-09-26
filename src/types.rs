use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Interval {
    M1,
    M5,
    M15,
    H1,
    H4,
    D1,
}

impl Interval {
    pub fn as_okx(&self) -> &'static str {
        match self {
            Self::M1 => "1m",
            Self::M5 => "5m",
            Self::M15 => "15m",
            Self::H1 => "1H",
            Self::H4 => "4H",
            Self::D1 => "1D",
        }
    }
}

/// 永续合约交易对。inst_id 是权威标识（如 "BTC-USDT-SWAP"）
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Symbol {
    pub base: String,
    pub quote: String,
    pub settle: String,
    pub inst_id: String,
}

impl Symbol {
    /// 解析 OKX SWAP instId，形如 "BTC-USDT-SWAP"
    pub fn from_swap_inst_id(inst_id: &str) -> Option<Self> {
        let parts: Vec<&str> = inst_id.split('-').collect();
        if parts.len() != 3 || parts[2] != "SWAP" {
            return None;
        }
        Some(Self {
            base: parts[0].into(),
            quote: parts[1].into(),
            settle: parts[1].into(), // U 本位
            inst_id: inst_id.into(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Instrument {
    pub symbol: Symbol,
    pub tick_size: Decimal,
    pub lot_size: Decimal,
    pub min_size: Decimal,
    /// 1 张合约的面值（币数量）。BTC-USDT-SWAP = 0.01
    pub ct_val: Decimal,
    pub max_leverage: Decimal,
}

impl Instrument {
    /// 把"币数量"换算成"下单张数"
    pub fn qty_to_contracts(&self, qty: Decimal) -> Decimal {
        if self.ct_val.is_zero() {
            Decimal::ZERO
        } else {
            qty / self.ct_val
        }
    }
}

#[derive(Debug, Clone)]
pub struct Ticker {
    pub symbol: Symbol,
    pub last: Decimal,
    pub bid: Decimal,
    pub ask: Decimal,
    pub open_24h: Decimal,
    pub high_24h: Decimal,
    pub low_24h: Decimal,
    pub volume_24h: Decimal,
    pub volume_quote_24h: Decimal,
    pub ts: i64,
}

impl Ticker {
    pub fn change_pct(&self) -> Decimal {
        if self.open_24h.is_zero() {
            Decimal::ZERO
        } else {
            (self.last - self.open_24h) / self.open_24h * Decimal::from(100)
        }
    }

    pub fn spread_pct(&self) -> Decimal {
        let mid = (self.bid + self.ask) / Decimal::from(2);
        if mid.is_zero() {
            Decimal::ZERO
        } else {
            (self.ask - self.bid) / mid * Decimal::from(100)
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candle {
    pub open_time: i64,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: Decimal,
}

#[derive(Debug, Clone)]
pub struct FundingRate {
    pub symbol: Symbol,
    pub rate: Decimal,
    pub next_time: Option<i64>,
    pub ts: i64,
}
