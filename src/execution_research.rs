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
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(50);
    let (tickers, mut complete, mut complete_scopes) = client.perp_ctxs_with_coverage().await?;
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
    let mut requests = stream::iter(wanted)
        .map(|coin| async move {
            let raw = client.l2_book(&coin).await;
            (coin, raw)
        })
        .buffer_unordered(5);
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
    tracing::info!(
        "HL execution research coverage: complete={} markets={} scopes={} books={}",
        complete,
        markets.len(),
        complete_scopes.len(),
        books.len()
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
