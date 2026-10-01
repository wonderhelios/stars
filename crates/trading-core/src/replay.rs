//! The historical runner uses the same policy functions as the live runner.
//! Missing books or mark intervals are explicit evidence gaps, never synthetic fills.
use crate::{
    policy::{self, Book, Market},
    strategy::StrategyConfig,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Quote {
    pub observed_at: i64,
    pub book: Book,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    pub at: i64,
    pub complete: bool,
    #[serde(default)]
    pub complete_scopes: HashSet<String>,
    pub markets: HashMap<String, Market>,
    pub books: HashMap<String, Quote>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplayTrade {
    pub coin: String,
    pub entry_at: i64,
    pub exit_at: Option<i64>,
    pub entry: f64,
    pub exit: Option<f64>,
    pub size: f64,
    pub leverage: u32,
    pub margin: f64,
    pub pnl: Option<f64>,
    pub reason: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct ReplayReport {
    pub execution_model: &'static str,
    pub rule_hash: String,
    pub strategy_hash: String,
    pub initial_equity: f64,
    pub equity: f64,
    pub return_pct: f64,
    pub max_drawdown_pct: f64,
    pub trades: Vec<ReplayTrade>,
    pub data_gaps: usize,
    pub missing_books: usize,
    pub incomplete_frames: usize,
    pub skipped_risk: usize,
    pub skipped_capacity: usize,
    pub days: usize,
    pub validation_n: usize,
    pub validation_mean_usd: Option<f64>,
    pub validation_median_usd: Option<f64>,
    pub funding_included: bool,
    pub funding_model: &'static str,
    pub deployable: bool,
    pub blockers: Vec<String>,
}

/// Stable ordering is essential: HashMap iteration must not select different coins.
pub fn ranked_crossings(
    markets: &HashMap<String, Market>,
    previous: Option<&HashSet<String>>,
    current: &HashSet<String>,
    gap_ms: i64,
    now: i64,
    triggers: &HashMap<String, i64>,
    strategy: &StrategyConfig,
) -> Vec<String> {
    let mut fresh: Vec<_> = policy::new_crossings(previous, current, gap_ms)
        .into_iter()
        .filter(|coin| {
            triggers
                .get(coin)
                .is_none_or(|at| now - at >= strategy.cooldown_hours as i64 * 3_600_000)
        })
        .collect();
    fresh.sort_by(|a, b| {
        markets[b]
            .entry_leverage(strategy)
            .cmp(&markets[a].entry_leverage(strategy))
            .then_with(|| markets[b].volume_usd.total_cmp(&markets[a].volume_usd))
            .then_with(|| a.cmp(b))
    });
    fresh
}

pub fn run(
    frames: &[Frame],
    strategy: &StrategyConfig,
    capital: f64,
) -> anyhow::Result<ReplayReport> {
    let boundary = frames
        .first()
        .zip(frames.last())
        .map(|(a, b)| a.at + (b.at - a.at) * 7 / 10)
        .unwrap_or(0);
    run_iter(frames.iter().cloned().map(Ok), strategy, capital, boundary)
}
/// Stream a consistent SQLite snapshot; memory does not grow with the frame history.
pub fn run_iter<I>(
    frames: I,
    strategy: &StrategyConfig,
    capital: f64,
    boundary: i64,
) -> anyhow::Result<ReplayReport>
where
    I: IntoIterator<Item = anyhow::Result<Frame>>,
{
    strategy.validate()?;
    anyhow::ensure!(
        capital.is_finite() && capital > 0.0,
        "invalid replay capital"
    );
    let mut out = ReplayReport {
        execution_model: crate::EXECUTION_MODEL,
        rule_hash: crate::rule_hash(),
        strategy_hash: crate::strategy_hash(strategy)?,
        initial_equity: capital,
        equity: capital,
        return_pct: 0.0,
        max_drawdown_pct: 0.0,
        trades: vec![],
        data_gaps: 0,
        missing_books: 0,
        incomplete_frames: 0,
        skipped_risk: 0,
        skipped_capacity: 0,
        days: 0,
        validation_n: 0,
        validation_mean_usd: None,
        validation_median_usd: None,
        funding_included: false,
        funding_model: "negative-rate reserve; positive receipts excluded",
        deployable: false,
        blockers: vec![],
    };
    let mut previous: Option<HashSet<String>> = None;
    let mut last_scan = None;
    let mut last_frame = None;
    let mut triggers = HashMap::new();
    let mut cash = capital;
    let mut peak = capital;
    let mut day_starts = HashMap::new();
    let mut stop_hits = HashSet::new();
    let mut funding = HashMap::<usize, f64>::new();
    let mut ordered_at = None;
    for item in frames {
        let owned = item?;
        let frame = &owned;
        anyhow::ensure!(
            ordered_at.is_none_or(|at| at < frame.at),
            "frames must be strictly chronological"
        );
        ordered_at = Some(frame.at);
        if !frame.complete && !frame.complete_scopes.contains(&strategy.dex_scope) {
            out.incomplete_frames += 1;
            previous = None;
            continue;
        }
        let elapsed = last_frame.map(|at| frame.at - at).unwrap_or(0);
        if last_frame.is_some_and(|at| frame.at - at > 90_000) {
            out.data_gaps += 1;
        }
        last_frame = Some(frame.at);
        for (i, trade) in out
            .trades
            .iter_mut()
            .enumerate()
            .filter(|(_, t)| t.exit_at.is_none())
        {
            let Some(market) = frame.markets.get(&trade.coin) else {
                out.data_gaps += 1;
                continue;
            };
            *funding.entry(i).or_default() +=
                policy::funding_reserve(market.funding_hourly, market.mark * trade.size, elapsed);
            if market.mark >= policy::stop_price(trade.entry, market.size_decimals, strategy) {
                stop_hits.insert(i);
            }
            let due = frame.at >= trade.entry_at + strategy.hold_hours as i64 * 3_600_000;
            let stopped = stop_hits.contains(&i);
            if !due && !stopped {
                continue;
            }
            let Some(book) = fresh_book(frame, &trade.coin) else {
                out.missing_books += 1;
                continue;
            };
            let Some(exit) =
                book.buy_vwap(trade.size, book.mid() * if stopped { 1.10 } else { 1.01 })
            else {
                out.missing_books += 1;
                continue;
            };
            let pnl = (trade.entry - exit) * trade.size
                - (trade.entry + exit) * trade.size * policy::estimated_fee(&trade.coin)
                - funding.get(&i).copied().unwrap_or(0.0);
            cash += pnl;
            trade.exit_at = Some(frame.at);
            trade.exit = Some(exit);
            trade.pnl = Some(pnl);
            trade.reason = Some(if stopped { "stop" } else { "hold" }.into());
        }
        let mut unrealized = 0.0;
        for (i, t) in out
            .trades
            .iter()
            .enumerate()
            .filter(|(_, t)| t.exit_at.is_none())
        {
            let mark = frame
                .markets
                .get(&t.coin)
                .map(|m| m.mark)
                .unwrap_or(t.entry);
            let ask = fresh_book(frame, &t.coin).and_then(|b| b.buy_vwap(t.size, b.mid() * 1.10));
            if ask.is_none() {
                out.missing_books += 1;
            }
            let exit = ask.unwrap_or(mark);
            unrealized += (t.entry - exit) * t.size
                - (t.entry + exit) * t.size * policy::estimated_fee(&t.coin)
                - funding.get(&i).copied().unwrap_or(0.0);
        }
        let equity = cash + unrealized;
        peak = peak.max(equity);
        out.max_drawdown_pct = out.max_drawdown_pct.max((peak - equity) / peak * 100.0);
        out.equity = equity;
        let day = frame.at.div_euclid(86_400_000);
        let day_start = *day_starts.entry(day).or_insert(equity);
        if !policy::risk_allows_equity(capital, day_start, equity, strategy) {
            out.skipped_risk += 1;
            continue;
        }
        if last_scan.is_some_and(|at| frame.at - at < 300_000) {
            continue;
        }
        let current: HashSet<_> = frame
            .markets
            .values()
            .filter(|m| m.qualifies(strategy))
            .map(|m| m.coin.clone())
            .collect();
        let gap = last_scan.map(|at| frame.at - at).unwrap_or(i64::MAX);
        let fresh = ranked_crossings(
            &frame.markets,
            previous.as_ref(),
            &current,
            gap,
            frame.at,
            &triggers,
            strategy,
        );
        for coin in policy::new_crossings(previous.as_ref(), &current, gap) {
            triggers.insert(coin, frame.at);
        }
        previous = Some(current);
        last_scan = Some(frame.at);
        let mut free = equity
            - out
                .trades
                .iter()
                .filter(|t| t.exit_at.is_none())
                .map(|t| t.entry * t.size / t.leverage as f64)
                .sum::<f64>();
        for coin in fresh {
            let active: Vec<_> = out.trades.iter().filter(|t| t.exit_at.is_none()).collect();
            let m = &frame.markets[&coin];
            let leverage = m.entry_leverage(strategy).unwrap();
            if active.iter().any(|t| t.coin == coin)
                || !policy::slot_available(
                    leverage,
                    active.len(),
                    active.iter().any(|t| t.leverage == 5),
                    active.iter().any(|t| t.leverage == 3),
                    strategy,
                )
            {
                out.skipped_capacity += 1;
                continue;
            }
            let required = policy::required_margin_for(&coin, equity, leverage, strategy);
            if free < required {
                out.skipped_capacity += 1;
                continue;
            }
            let Some(book) = fresh_book(frame, &coin) else {
                out.missing_books += 1;
                continue;
            };
            let notional = policy::notional_at(equity, leverage, strategy);
            if !book.executable(notional) {
                continue;
            }
            let size = m.size(book.bid, notional);
            if size * book.bid < 10.0 {
                continue;
            }
            let Some(entry) = book.sell_vwap(size, book.mid() * (1.0 - policy::MAX_SPREAD)) else {
                continue;
            };
            out.trades.push(ReplayTrade {
                coin,
                entry_at: frame.at,
                exit_at: None,
                entry,
                exit: None,
                size,
                leverage,
                margin: policy::target_margin(equity, strategy),
                pnl: None,
                reason: None,
            });
            free -= required;
            // Include terminal open positions and fees even if this is the last frame.
            if let Some(ask) = book.buy_vwap(size, book.mid() * 1.10) {
                out.equity +=
                    (entry - ask) * size - (entry + ask) * size * policy::estimated_fee(&m.coin);
            } else {
                out.missing_books += 1;
            }
            peak = peak.max(out.equity);
            out.max_drawdown_pct = out.max_drawdown_pct.max((peak - out.equity) / peak * 100.0);
        }
    }
    out.days = day_starts.len();
    out.return_pct = (out.equity / capital - 1.0) * 100.0;
    // Split by wall-clock time and purge training trades overlapping validation.
    let mut validation: Vec<_> = out
        .trades
        .iter()
        .filter(|t| t.entry_at >= boundary)
        .filter_map(|t| t.pnl)
        .collect();
    out.validation_n = validation.len();
    if !validation.is_empty() {
        out.validation_mean_usd = Some(validation.iter().sum::<f64>() / validation.len() as f64);
        validation.sort_by(f64::total_cmp);
        out.validation_median_usd =
            Some((validation[(validation.len() - 1) / 2] + validation[validation.len() / 2]) / 2.0);
    }
    if out.trades.iter().filter(|t| t.pnl.is_some()).count() < 30 {
        out.blockers.push("已平仓样本不足30笔".into());
    }
    if out.validation_n < 10 {
        out.blockers.push("后30%验证段不足10笔".into());
    }
    if out.days < 5 {
        out.blockers.push("观察不足5个UTC日".into());
    }
    if out.validation_mean_usd.is_none_or(|x| x <= 0.0)
        || out.validation_median_usd.is_none_or(|x| x <= 0.0)
    {
        out.blockers.push("验证收益未通过".into());
    }
    if out.data_gaps + out.missing_books + out.incomplete_frames > 0 {
        out.blockers.push("存在价格路径、盘口或扫描缺口".into());
    }
    // Funding is excluded from this price-path study and cannot prove a funding arbitrage.
    // Enabling a price strategy additionally requires independent forward validation.
    out.deployable = out.blockers.is_empty();
    Ok(out)
}
fn fresh_book<'a>(frame: &'a Frame, coin: &str) -> Option<&'a Book> {
    frame
        .books
        .get(coin)
        .filter(|q| (0..=15_000).contains(&(frame.at - q.observed_at)))
        .map(|q| &q.book)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn market(coin: &str, mark: f64, leverage: usize) -> Market {
        Market {
            collateral_usdc: true,
            coin: coin.into(),
            mark,
            prev: 100.0,
            funding_hourly: 0.0001,
            volume_usd: 1_000_000.0,
            size_decimals: 2,
            max_leverage: leverage,
        }
    }
    fn frame(at: i64, mark: f64, with_book: bool) -> Frame {
        let m = market("AAA", mark, 10);
        let book = Book {
            bid: mark - 0.01,
            ask: mark + 0.01,
            bid_depth: vec![(mark - 0.01, 1000.0)],
            ask_depth: vec![(mark + 0.01, 1000.0)],
        };
        Frame {
            at,
            complete: true,
            complete_scopes: HashSet::new(),
            markets: HashMap::from([("AAA".into(), m)]),
            books: if with_book {
                HashMap::from([(
                    "AAA".into(),
                    Quote {
                        observed_at: at,
                        book,
                    },
                )])
            } else {
                HashMap::new()
            },
        }
    }
    #[test]
    fn baseline_is_not_an_entry_and_stop_requires_an_observed_fill() {
        let s = StrategyConfig::default();
        assert!(run(&[frame(0, 105.0, true)], &s, 500.0)
            .unwrap()
            .trades
            .is_empty());
        let frames = [
            frame(0, 100.0, true),
            frame(300_000, 105.0, true),
            frame(360_000, 109.0, false),
            frame(420_000, 110.0, true),
        ];
        let r = run(&frames, &s, 500.0).unwrap();
        assert_eq!(r.trades.len(), 1);
        let t = &r.trades[0];
        assert_eq!(t.margin, 100.0);
        assert_eq!(t.leverage, 10);
        assert_eq!(t.exit_at, Some(420_000));
        assert_eq!(t.exit, Some(110.01)); // Never fabricate an exit at the stop threshold.
        assert_eq!(t.reason.as_deref(), Some("stop"));
        assert!(r.missing_books >= 1);
        assert!(!r.deployable);
    }
    #[test]
    fn future_book_and_partial_scan_are_not_evidence() {
        let s = StrategyConfig::default();
        let mut future = frame(300_000, 105.0, true);
        future.books.get_mut("AAA").unwrap().observed_at += 20_000;
        assert!(run(&[frame(0, 100.0, true), future], &s, 500.0)
            .unwrap()
            .trades
            .is_empty());
        let mut partial = frame(300_000, 105.0, true);
        partial.complete = false;
        let r = run(
            &[frame(0, 100.0, true), partial, frame(600_000, 105.0, true)],
            &s,
            500.0,
        )
        .unwrap();
        assert!(r.trades.is_empty());
        assert_eq!(r.incomplete_frames, 1);
    }
    #[test]
    fn sorting_and_low_leverage_slots_match_live_policy() {
        let s = StrategyConfig::default();
        let mut f = frame(300_000, 105.0, true);
        for coin in ["BBB", "CCC"] {
            f.markets.insert(coin.into(), market(coin, 105.0, 5));
            f.books.insert(coin.into(), f.books["AAA"].clone());
        }
        let r = run(&[frame(0, 100.0, true), f], &s, 500.0).unwrap();
        assert_eq!(
            r.trades.iter().map(|t| t.coin.as_str()).collect::<Vec<_>>(),
            ["AAA", "BBB"]
        );
        assert_eq!(r.trades[1].leverage, 5);
        assert_eq!(r.skipped_capacity, 1);
    }
    #[test]
    fn main_scope_survives_an_unrelated_dex_failure_and_values_terminal_fees() {
        let mut second = frame(300_000, 105.0, true);
        second.complete = false;
        second.complete_scopes.insert("main".into());
        let frames = [frame(0, 100.0, true), second];
        let main = run(&frames, &StrategyConfig::default(), 500.0).unwrap();
        assert_eq!(main.trades.len(), 1);
        assert!(main.equity < 500.0);
        let all = run(
            &frames,
            &StrategyConfig {
                dex_scope: "all".into(),
                ..StrategyConfig::default()
            },
            500.0,
        )
        .unwrap();
        assert!(all.trades.is_empty());
        assert_eq!(all.incomplete_frames, 1);
    }
    #[test]
    fn collateral_and_changed_parameters_cannot_pass_deployment() {
        let mut m = market("para:ABC", 105.0, 10);
        m.collateral_usdc = false;
        let mut s = StrategyConfig {
            dex_scope: "all".into(),
            ..StrategyConfig::default()
        };
        assert!(!m.qualifies(&s));
        s.research_evidence = Some(serde_json::json!({"execution_model":crate::EXECUTION_MODEL,
            "rule_hash":crate::rule_hash(),"strategy_hash":crate::strategy_hash(&s).unwrap(),"deployable":true}));
        s.validate_deployment().unwrap();
        s.hold_hours = 8;
        assert!(s.validate_deployment().is_err());
        assert!(policy::funding_reserve(-0.001, 1000.0, 3_600_000) > 0.99);
        assert_eq!(policy::funding_reserve(0.001, 1000.0, 3_600_000), 0.0);
    }
}
