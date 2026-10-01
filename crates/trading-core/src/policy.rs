use crate::strategy::StrategyConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const CAPITAL: f64 = 500.0;
pub const LEVERAGE: u32 = 10;
pub const FALLBACK_LEVERAGE: u32 = 5;
pub const LOW_LEVERAGE: u32 = 3;
pub const MAX_SPREAD: f64 = 0.002;
pub const DAILY_LOSS_LIMIT_FRACTION: f64 = 0.06;
pub const TOTAL_LOSS_LIMIT: f64 = 50.0;
pub const ESTIMATED_TAKER_FEE: f64 = 0.00045;

/// Builder perps may charge twice the base taker fee; this is an estimate.
pub fn estimated_fee(coin: &str) -> f64 {
    ESTIMATED_TAKER_FEE * if coin.contains(':') { 2.0 } else { 1.0 }
}
/// Ignore positive funding receipts and reserve observed negative-rate costs.
/// This is a conservative sampled-rate model, not a settlement ledger.
pub fn funding_reserve(rate_hourly: f64, notional: f64, elapsed_ms: i64) -> f64 {
    (-rate_hourly.min(0.0)) * notional * elapsed_ms.max(0) as f64 / 3_600_000.0
}
pub fn required_margin_for(
    coin: &str,
    equity: f64,
    leverage: u32,
    strategy: &StrategyConfig,
) -> f64 {
    target_margin(equity, strategy)
        + notional_at(equity, leverage, strategy) * estimated_fee(coin) * 2.0
}

pub fn daily_loss_floor(day_start_equity: f64) -> f64 {
    day_start_equity * (1.0 - DAILY_LOSS_LIMIT_FRACTION)
}

pub fn risk_allows_equity(
    initial: f64,
    day_start: f64,
    equity: f64,
    strategy: &StrategyConfig,
) -> bool {
    equity.is_finite()
        && target_notional(equity, strategy) >= 10.0
        && equity > initial - TOTAL_LOSS_LIMIT
        && equity > daily_loss_floor(day_start)
}
pub fn stop_execution_limit(trigger: f64, size_decimals: u32) -> f64 {
    order_price(trigger * 1.05, size_decimals, true)
}
pub fn stop_price(entry: f64, size_decimals: u32, strategy: &StrategyConfig) -> f64 {
    order_price(
        entry * (1.0 + strategy.stop_loss_pct / 100.0),
        size_decimals,
        false,
    )
}

pub fn target_margin(equity: f64, strategy: &StrategyConfig) -> f64 {
    // Round the target margin down to cents before deriving order notional.
    (equity * strategy.margin_fraction * 100.0).floor() / 100.0
}

pub fn target_notional(equity: f64, strategy: &StrategyConfig) -> f64 {
    target_margin(equity, strategy)
        * strategy
            .allowed_leverages
            .iter()
            .copied()
            .max()
            .unwrap_or(0) as f64
}

pub fn notional_at(equity: f64, leverage: u32, strategy: &StrategyConfig) -> f64 {
    target_margin(equity, strategy) * leverage as f64
}

pub fn required_free_margin(equity: f64, leverage: u32, strategy: &StrategyConfig) -> f64 {
    target_margin(equity, strategy)
        + notional_at(equity, leverage, strategy) * ESTIMATED_TAKER_FEE * 2.0
}

pub fn slot_available(
    leverage: u32,
    occupied: usize,
    five_x_occupied: bool,
    three_x_occupied: bool,
    strategy: &StrategyConfig,
) -> bool {
    occupied < strategy.max_positions
        && (leverage != FALLBACK_LEVERAGE || strategy.max_five_x_positions > 0 && !five_x_occupied)
        && (leverage != LOW_LEVERAGE || strategy.max_three_x_positions > 0 && !three_x_occupied)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Market {
    #[serde(default)]
    pub collateral_usdc: bool,
    pub coin: String,
    pub mark: f64,
    pub prev: f64,
    pub funding_hourly: f64,
    pub volume_usd: f64,
    pub size_decimals: u32,
    pub max_leverage: usize,
}

enum RejectReason {
    InvalidMarket,
    Collateral,
    Leverage,
    Rise,
    Funding,
    Volume,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ScanFilterCounts {
    pub total: usize,
    pub leverage_10: usize,
    pub leverage_5: usize,
    pub leverage_3: usize,
    pub leverage_below_3: usize,
    pub invalid_market: usize,
    pub unsupported_collateral: usize,
    pub leverage: usize,
    pub rise: usize,
    pub funding: usize,
    pub volume: usize,
    pub eligible: usize,
}

impl ScanFilterCounts {
    pub fn count<'a>(
        markets: impl IntoIterator<Item = &'a Market>,
        strategy: &StrategyConfig,
    ) -> Self {
        let mut counts = Self::default();
        for market in markets {
            counts.total += 1;
            match market.entry_leverage(strategy) {
                Some(LEVERAGE) => counts.leverage_10 += 1,
                Some(FALLBACK_LEVERAGE) => counts.leverage_5 += 1,
                Some(LOW_LEVERAGE) => counts.leverage_3 += 1,
                _ => counts.leverage_below_3 += 1,
            }
            match market.rejection(strategy) {
                Some(RejectReason::InvalidMarket) => counts.invalid_market += 1,
                Some(RejectReason::Collateral) => counts.unsupported_collateral += 1,
                Some(RejectReason::Leverage) => counts.leverage += 1,
                Some(RejectReason::Rise) => counts.rise += 1,
                Some(RejectReason::Funding) => counts.funding += 1,
                Some(RejectReason::Volume) => counts.volume += 1,
                None => counts.eligible += 1,
            }
        }
        counts
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Book {
    pub bid: f64,
    pub ask: f64,
    pub bid_depth: Vec<(f64, f64)>,
    pub ask_depth: Vec<(f64, f64)>,
}

impl Book {
    pub fn mid(&self) -> f64 {
        (self.bid + self.ask) / 2.0
    }

    pub fn executable(&self, notional: f64) -> bool {
        if !self.bid.is_finite() || !self.ask.is_finite() || self.bid <= 0.0 || self.ask <= self.bid
        {
            return false;
        }
        let mid = self.mid();
        let spread = (self.ask - self.bid) / mid;
        let depth: f64 = self
            .bid_depth
            .iter()
            .filter(|(px, _)| *px >= mid * (1.0 - MAX_SPREAD))
            .map(|(px, size)| px * size)
            .sum();
        spread <= MAX_SPREAD && depth >= notional
    }

    pub fn sell_vwap(&self, size: f64, min_px: f64) -> Option<f64> {
        fill_vwap(&self.bid_depth, size, |px| px >= min_px)
    }

    pub fn buy_vwap(&self, size: f64, max_px: f64) -> Option<f64> {
        fill_vwap(&self.ask_depth, size, |px| px <= max_px)
    }

    // Estimate the net result of fully buying back a short against visible asks.
    pub fn short_close_net(&self, entry: f64, size: f64, max_px: f64) -> Option<f64> {
        if !entry.is_finite() || entry <= 0.0 || !size.is_finite() || size <= 0.0 {
            return None;
        }
        let mut remaining = size;
        let mut exit_notional = 0.0;
        for &(px, available) in &self.ask_depth {
            if !px.is_finite() || !available.is_finite() || px <= 0.0 || available <= 0.0 {
                return None;
            }
            if px > max_px {
                break;
            }
            let filled = remaining.min(available);
            exit_notional += filled * px;
            remaining -= filled;
            if remaining <= 1e-8 {
                let entry_notional = entry * size;
                return Some(
                    entry_notional
                        - exit_notional
                        - (entry_notional + exit_notional) * ESTIMATED_TAKER_FEE,
                );
            }
        }
        None
    }
}

fn fill_vwap(levels: &[(f64, f64)], size: f64, allowed: impl Fn(f64) -> bool) -> Option<f64> {
    if !size.is_finite() || size <= 0.0 {
        return None;
    }
    let mut remaining = size;
    let mut total = 0.0;
    for &(px, available) in levels {
        if !px.is_finite() || !available.is_finite() || px <= 0.0 || available <= 0.0 {
            return None;
        }
        if !allowed(px) {
            break;
        }
        let filled = remaining.min(available);
        total += filled * px;
        remaining -= filled;
        if remaining <= 1e-8 {
            return Some(total / size);
        }
    }
    None
}

impl Market {
    pub fn qualifies(&self, strategy: &StrategyConfig) -> bool {
        self.rejection(strategy).is_none()
    }

    fn rejection(&self, strategy: &StrategyConfig) -> Option<RejectReason> {
        if !self.mark.is_finite()
            || self.mark <= 0.0
            || !self.prev.is_finite()
            || self.prev <= 0.0
            || !self.funding_hourly.is_finite()
            || !self.volume_usd.is_finite()
        {
            return Some(RejectReason::InvalidMarket);
        }
        if !self.collateral_usdc {
            return Some(RejectReason::Collateral);
        }
        if !strategy.includes_coin(&self.coin) || self.entry_leverage(strategy).is_none() {
            return Some(RejectReason::Leverage);
        }
        let prior = self.mark / self.prev - 1.0;
        let kind = StrategyConfig::kind_for_return(prior * 100.0);
        if kind.is_none_or(|kind| !strategy.signal_kinds.iter().any(|allowed| allowed == kind)) {
            return Some(RejectReason::Rise);
        }
        if self.funding_hourly * 8.0 <= strategy.min_funding_8h {
            return Some(RejectReason::Funding);
        }
        if self.volume_usd < strategy.min_volume_usd {
            return Some(RejectReason::Volume);
        }
        None
    }

    pub fn size(&self, bid: f64, notional: f64) -> f64 {
        let factor = 10_f64.powi(self.size_decimals as i32);
        ((notional / bid) * factor).floor() / factor
    }

    pub fn entry_leverage(&self, strategy: &StrategyConfig) -> Option<u32> {
        strategy
            .allowed_leverages
            .iter()
            .copied()
            .filter(|leverage| self.max_leverage >= *leverage as usize)
            .max()
    }
}

pub fn new_crossings(
    previous: Option<&HashSet<String>>,
    current: &HashSet<String>,
    gap_ms: i64,
) -> HashSet<String> {
    if gap_ms > 600_000 {
        return HashSet::new();
    }
    match previous {
        None => HashSet::new(),
        Some(before) => current.difference(before).cloned().collect(),
    }
}

pub fn order_price(px: f64, sz_decimals: u32, round_up: bool) -> f64 {
    if !px.is_finite() || px <= 0.0 {
        return 0.0;
    }
    let decimal_places = (4 - px.log10().floor() as i32).min(6 - sz_decimals as i32);
    let factor = 10_f64.powi(decimal_places);
    let scaled = px * factor;
    if round_up {
        scaled.ceil() / factor
    } else {
        scaled.floor() / factor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_loss_floor_is_six_percent_below_day_start() {
        assert!((daily_loss_floor(500.0) - 470.0).abs() < 1e-9);
        assert!((daily_loss_floor(542.14) - 509.6116).abs() < 1e-9);
    }

    #[test]
    fn sizing_and_crossing() {
        let strategy = StrategyConfig::default();
        assert_eq!(target_margin(500.0, &strategy), 100.0);
        assert_eq!(target_notional(500.0, &strategy), 1000.0);
        assert_eq!(target_margin(600.0, &strategy), 120.0);
        assert_eq!(target_notional(600.0, &strategy), 1200.0);
        assert_eq!(target_margin(530.6658705, &strategy), 106.13);
        assert_eq!(target_notional(530.6658705, &strategy), 1061.3);
        assert_eq!(
            notional_at(530.6658705, FALLBACK_LEVERAGE, &strategy),
            530.65
        );
        assert_eq!(notional_at(530.6658705, LOW_LEVERAGE, &strategy), 318.39);
        assert!(required_free_margin(500.0, LEVERAGE, &strategy) > 100.0);
        assert!(required_free_margin(500.0, LOW_LEVERAGE, &strategy) < 101.0);
        let m = Market {
            collateral_usdc: true,
            coin: "A".into(),
            mark: 104.0,
            prev: 100.0,
            funding_hourly: 0.0001,
            volume_usd: 1_000_000.0,
            size_decimals: 3,
            max_leverage: 20,
        };
        assert!(m.qualifies(&strategy));
        assert_eq!(m.size(100.0, target_notional(500.0, &strategy)), 10.0);
        assert_eq!(m.size(100.0, target_notional(600.0, &strategy)), 12.0);
        let book = Book {
            bid: 100.0,
            ask: 100.1,
            bid_depth: vec![(100.0, 11.0)],
            ask_depth: vec![(100.1, 11.0)],
        };
        assert!(book.executable(target_notional(500.0, &strategy)));
        assert!(!book.executable(target_notional(600.0, &strategy)));
        let current = HashSet::from(["A".to_string()]);
        assert!(new_crossings(None, &current, 300_000).is_empty());
        assert_eq!(
            new_crossings(Some(&HashSet::new()), &current, 300_000),
            current
        );
        assert!(new_crossings(Some(&HashSet::new()), &current, 700_000).is_empty());
        assert_eq!(order_price(1234.567, 2, true), 1234.6);
        assert_eq!(order_price(0.01234567, 3, false), 0.012);
        assert_eq!(order_price(0.01234567, 3, true), 0.013);
    }

    #[test]
    fn take_profit_requires_full_executable_size_and_fees() {
        let book = Book {
            bid: 95.0,
            ask: 95.1,
            bid_depth: vec![],
            ask_depth: vec![(95.1, 5.0), (95.2, 5.0)],
        };
        let net = book.short_close_net(100.0, 10.0, 96.0).unwrap();
        assert!(net > 40.0 && net < 50.0);
        assert_eq!(book.short_close_net(100.0, 10.0, 95.1), None);
        assert_eq!(book.short_close_net(100.0, 11.0, 96.0), None);
    }

    #[test]
    fn paper_fills_use_full_visible_depth_and_price_limits() {
        let book = Book {
            bid: 100.0,
            ask: 101.0,
            bid_depth: vec![(100.0, 1.0), (99.9, 2.0)],
            ask_depth: vec![(101.0, 1.0), (101.1, 2.0)],
        };
        assert!((book.sell_vwap(2.0, 99.8).unwrap() - 99.95).abs() < 1e-9);
        assert!((book.buy_vwap(2.0, 101.2).unwrap() - 101.05).abs() < 1e-9);
        assert_eq!(book.sell_vwap(2.0, 100.0), None);
        assert_eq!(book.buy_vwap(4.0, 102.0), None);
    }

    #[test]
    fn scan_counts_each_market_at_first_failed_gate() {
        let strategy = StrategyConfig::default();
        let mut markets = Vec::new();
        let base = Market {
            collateral_usdc: true,
            coin: "A".into(),
            mark: 104.0,
            prev: 100.0,
            funding_hourly: 0.0001,
            volume_usd: 600_000.0,
            size_decimals: 2,
            max_leverage: 10,
        };
        markets.push(base.clone());
        let mut fallback_leverage = base.clone();
        fallback_leverage.max_leverage = 5;
        assert_eq!(fallback_leverage.entry_leverage(&strategy), Some(5));
        markets.push(fallback_leverage);
        let mut low_leverage = base.clone();
        low_leverage.max_leverage = 3;
        assert_eq!(low_leverage.entry_leverage(&strategy), Some(3));
        markets.push(low_leverage);
        let mut too_low_leverage = base.clone();
        too_low_leverage.max_leverage = 2;
        markets.push(too_low_leverage);
        let mut low_rise = base.clone();
        low_rise.mark = 101.0;
        markets.push(low_rise);
        let mut low_funding = base.clone();
        low_funding.funding_hourly = 0.0;
        markets.push(low_funding);
        let mut low_volume = base.clone();
        low_volume.volume_usd = 400_000.0;
        markets.push(low_volume);
        let mut invalid = base;
        invalid.mark = f64::NAN;
        markets.push(invalid);
        let counts = ScanFilterCounts::count(&markets, &strategy);
        assert_eq!(counts.total, 8);
        assert_eq!(
            (
                counts.leverage_10,
                counts.leverage_5,
                counts.leverage_3,
                counts.leverage_below_3
            ),
            (5, 1, 1, 1)
        );
        assert_eq!(
            (
                counts.invalid_market,
                counts.leverage,
                counts.rise,
                counts.funding,
                counts.volume,
                counts.eligible
            ),
            (1, 1, 1, 1, 1, 3)
        );
        assert_eq!(
            markets.iter().filter(|m| m.qualifies(&strategy)).count(),
            counts.eligible
        );
    }

    #[test]
    fn five_x_and_three_x_each_get_one_of_five_slots() {
        let strategy = StrategyConfig::default();
        assert!(slot_available(5, 0, false, false, &strategy));
        assert!(slot_available(3, 1, true, false, &strategy));
        assert!(!slot_available(5, 2, true, false, &strategy));
        assert!(!slot_available(3, 2, false, true, &strategy));
        assert!(slot_available(10, 4, true, true, &strategy));
        assert!(!slot_available(10, 5, true, true, &strategy));
    }
}
