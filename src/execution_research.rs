use crate::{hyperliquid::HyperliquidRestClient, paper_store::PaperDb};
use anyhow::{Context, Result};
use futures_util::{stream, StreamExt};
use std::collections::{HashMap, HashSet};
use trading_core::{
    policy::{Book, Market},
    replay::{Frame, Quote},
};

/// Collect full eligibility snapshots, including non-candidates, to reconstruct crossings.
/// Books are fetched for liquid signal markets and still-observed contracts.
pub async fn collect(client: &HyperliquidRestClient, db: &PaperDb) -> Result<()> {
    let collection_start = tokio::time::Instant::now();
    let (tickers, mut complete, mut complete_scopes) =
        client.execution_perp_ctxs_with_coverage().await?;
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;
    let mut wanted: HashSet<String> = db
        .open_signals()
        .await?
        .into_iter()
        .filter(|r| started - r.triggered_at < 24 * 3_600_000)
        .map(|r| r.inst_id)
        .collect();
    let acquisition_times: HashMap<String, i64> = tickers
        .iter()
        .map(|t| (t.coin.clone(), t.acquisition_started_at))
        .collect();
    let count = tickers.len();
    let markets: HashMap<String, Market> = tickers
        .into_iter()
        .filter_map(|t| {
            if t.size_decimals.is_none() || t.max_leverage.is_none() {
                complete_scopes.remove(t.coin.split_once(':').map(|(d, _)| d).unwrap_or("main"));
            }
            if t.day_ntl_vlm >= rust_decimal::Decimal::from(500_000)
                && t.prior_24h_pct().abs() >= rust_decimal::Decimal::from(3)
                && t.funding * rust_decimal::Decimal::from(8) > rust_decimal::Decimal::new(5, 4)
            {
                wanted.insert(t.coin.clone());
            }
            Some((
                t.coin.clone(),
                Market {
                    collateral_usdc: t.collateral_usdc,
                    coin: t.coin,
                    mark: t.mark_px.to_string().parse().ok()?,
                    prev: t.prev_day_px.to_string().parse().ok()?,
                    funding_hourly: t.funding.to_string().parse().ok()?,
                    volume_usd: t.day_ntl_vlm.to_string().parse().ok()?,
                    size_decimals: t.size_decimals?,
                    max_leverage: t.max_leverage? as usize,
                },
            ))
        })
        .collect();
    complete &= count == markets.len();
    wanted.extend(
        db.execution_watch(wanted.iter().cloned().collect(), started)
            .await?,
    );
    let mut books = HashMap::new();
    let wanted_count = wanted.len();
    // Bound the book phase below the replay freshness window (15 seconds).
    // A stalled request must release its slot instead of occupying it for 3×30s.
    let book_start_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;
    let oldest_market_start = markets
        .keys()
        .filter_map(|coin| acquisition_times.get(coin))
        .min()
        .copied()
        .unwrap_or(book_start_ms);
    let budget_ms = book_budget_ms(book_start_ms, oldest_market_start);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(budget_ms);
    let mut wanted: Vec<_> = wanted.into_iter().collect();
    wanted.sort();
    let mut requests = stream::iter(wanted)
        .map(|coin| async move {
            let raw =
                tokio::time::timeout(std::time::Duration::from_secs(4), client.l2_book(&coin))
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("book request exceeded 4s budget")));
            (coin, raw)
        })
        .buffer_unordered(12);
    // Retain the market frame even when some books stall. Missing books remain
    // missing and replay rejects fills without a fresh, sufficient quote.
    loop {
        let next = tokio::time::timeout_at(deadline, requests.next()).await;
        let (coin, result) = match next {
            Ok(Some(next)) => next,
            Ok(None) => break,
            Err(_) => {
                tracing::warn!(
                    "HL execution research book budget exhausted; partial books retained"
                );
                break;
            }
        };
        match result {
            Ok(q) => {
                books.insert(coin, q);
            }
            Err(e) => tracing::warn!("research book {}: {}", coin, e),
        }
    }
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;
    // The upstream context response has no exchange timestamp. Use request start
    // as a conservative age bound; never advance old marks to the frame end.
    let stale_markets: Vec<_> = markets
        .keys()
        .filter(|coin| {
            !acquisition_times
                .get(*coin)
                .is_some_and(|at_start| fresh_at(at, *at_start))
        })
        .cloned()
        .collect();
    for coin in &stale_markets {
        complete_scopes.remove(coin.split_once(':').map(|(d, _)| d).unwrap_or("main"));
    }
    complete &= stale_markets.is_empty();
    let received_books = books.len();
    books.retain(|_, q| fresh_at(at, q.observed_at));
    tracing::info!(
        "HL execution research coverage: complete={} markets={} scopes={} books={} wanted_books={} missing_books={} stale_books={} stale_markets={} elapsed_ms={}",
        complete,
        markets.len(),
        complete_scopes.len(),
        books.len(),
        wanted_count,
        wanted_count.saturating_sub(books.len()),
        received_books - books.len(),
        stale_markets.len(),
        collection_start.elapsed().as_millis()
    );
    db.record_execution_frame(&Frame {
        at,
        complete,
        complete_scopes,
        markets,
        books,
    })
    .await?;
    Ok(())
}

pub fn parse_book(raw: serde_json::Value) -> Result<Quote> {
    let at = raw["time"].as_i64().context("missing book timestamp")?;
    let levels = raw["levels"].as_array().context("missing book levels")?;
    anyhow::ensure!(levels.len() == 2, "invalid book sides");
    let side = |i: usize| -> Result<Vec<(f64, f64)>> {
        levels[i]
            .as_array()
            .context("book side")?
            .iter()
            .map(|r| {
                Ok((
                    r["px"].as_str().context("price")?.parse()?,
                    r["sz"].as_str().context("size")?.parse()?,
                ))
            })
            .collect()
    };
    let bids = side(0)?;
    let asks = side(1)?;
    Ok(Quote {
        observed_at: at,
        book: Book {
            bid: bids.first().context("empty bids")?.0,
            ask: asks.first().context("empty asks")?.0,
            bid_depth: bids,
            ask_depth: asks,
        },
    })
}

// Reserve 100ms for frame assembly/recording without overstating mark freshness.
fn book_budget_ms(now_ms: i64, oldest_market_start: i64) -> u64 {
    (oldest_market_start
        .saturating_add(14_900)
        .saturating_sub(now_ms))
    .clamp(0, 10_000) as u64
}

/// Same 15s window as execution replay, including rejection of future quotes.
fn fresh_at(frame_at: i64, acquired_at: i64) -> bool {
    (0..=15_000).contains(&(frame_at - acquired_at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn book_budget_never_waits_beyond_oldest_mark_freshness() {
        assert_eq!(book_budget_ms(10_000, 10_000), 10_000);
        assert_eq!(book_budget_ms(18_000, 10_000), 6_900);
        assert_eq!(book_budget_ms(25_000, 10_000), 0);
    }

    #[test]
    fn slow_mark_fetch_and_stale_or_future_books_are_not_fresh() {
        assert!(fresh_at(100_000, 85_000));
        assert!(fresh_at(100_000, 100_000));
        assert!(!fresh_at(100_000, 84_999));
        assert!(!fresh_at(100_000, 100_001));
        // A mark requested 20 seconds ago cannot inherit a new frame timestamp.
        assert!(!fresh_at(100_000, 80_000));
    }
}
