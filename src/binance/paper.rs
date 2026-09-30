use std::sync::Arc;
use std::time::Duration;

use futures_util::{stream, StreamExt};
use rust_decimal::Decimal;
use tokio::time::interval;
use tracing::{error, info};

use super::rest::BinanceRestClient;
use crate::paper_store::{due_outcomes, CandidateSnapshot, FundingSnapshot, PaperDb};
use crate::signal::{classify, Signal};

pub type BinancePaperDb = PaperDb;

const SCAN_INTERVAL_SECS: u64 = 300;
const TRACK_INTERVAL_SECS: u64 = 60;

// ========== 扫描 ==========

pub async fn scan_once(
    client: &BinanceRestClient,
    db: &BinancePaperDb,
    funding_threshold: Decimal,
) -> anyhow::Result<usize> {
    let scan_started_at = now_ms();
    let tickers = client.all_tickers().await?;
    let (funding_result, intervals_result, (quotes_result, quote_at)) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(18), client.all_funding_now()),
        tokio::time::timeout(Duration::from_secs(12), client.funding_intervals()),
        async {
            let result = tokio::time::timeout(Duration::from_secs(12), client.best_quotes()).await;
            (result, now_ms())
        }
    );
    let funding = match funding_result {
        Ok(Ok(rows)) => rows,
        other => {
            error!(
                "binance bulk funding unavailable ({:?}); trying limited parallel requests",
                other
            );
            let symbols: Vec<String> = tickers
                .iter()
                .filter(|t| {
                    t.vol_quote >= Decimal::from(500_000)
                        && !t.open_24h.is_zero()
                        && t.prior_24h_pct().abs() >= Decimal::from(3)
                })
                .map(|t| t.symbol.clone())
                .collect();
            stream::iter(symbols)
                .map(|symbol| async move {
                    let result =
                        tokio::time::timeout(Duration::from_secs(8), client.funding_now(&symbol))
                            .await;
                    (symbol, result)
                })
                .buffer_unordered(16)
                .filter_map(|(symbol, result)| async move {
                    match result {
                        Ok(Ok(row)) => Some((symbol, row)),
                        _ => None,
                    }
                })
                .collect()
                .await
        }
    };
    let intervals = match intervals_result {
        Ok(Ok(value)) => Some(value),
        Err(e) => {
            error!("binance funding intervals: {}", e);
            None
        }
        Ok(Err(e)) => {
            error!("binance funding intervals: {}", e);
            None
        }
    };
    let quotes = match quotes_result {
        Ok(Ok(value)) => Some(value),
        Err(e) => {
            error!("binance book quotes: {}", e);
            None
        }
        Ok(Err(e)) => {
            error!("binance book quotes: {}", e);
            None
        }
    };
    let funding_snapshots = tickers
        .iter()
        .filter(|ticker| ticker.vol_quote >= Decimal::from(100_000))
        .filter_map(|ticker| {
            let rate = funding.get(&ticker.symbol)?;
            let quote = quotes
                .as_ref()
                .and_then(|rows| rows.get(&ticker.symbol))
                .copied();
            Some(FundingSnapshot {
                observed_at: scan_started_at,
                inst_id: ticker.symbol.clone(),
                funding_rate: rate.rate,
                funding_period_hours: intervals
                    .as_ref()
                    .map(|rows| *rows.get(&ticker.symbol).unwrap_or(&8))
                    .unwrap_or(8),
                next_funding_at: rate.next_funding_at,
                prior_24h_return: ticker.prior_24h_pct(),
                reference_price: ticker.last,
                bid_price: quote.map(|(bid, _)| bid),
                ask_price: quote.map(|(_, ask)| ask),
                quote_kind: Some("top"),
                quote_observed_at: quote.map(|_| quote_at),
                volume_quote_24h: ticker.vol_quote,
                open_interest_base: None,
            })
        })
        .collect();
    match db.record_funding_snapshots(funding_snapshots).await {
        Ok(count) => info!("BN funding research snapshot: {} markets", count),
        Err(error) => error!("BN funding research snapshot: {}", error),
    }
    let mut triggered = 0usize;
    let mut candidates = 0usize;
    let mut recorded = 0usize;
    let mut failed = 0usize;
    if intervals.is_none() {
        failed += 1;
    }
    if quotes.is_none() {
        failed += 1;
    }

    for t in &tickers {
        if t.vol_quote < Decimal::from(500_000) {
            continue;
        }
        if t.open_24h.is_zero() {
            continue;
        }
        let prior_pct = t.prior_24h_pct();
        if prior_pct.abs() < Decimal::from(3) {
            continue;
        }
        candidates += 1;

        let funding_now = match funding.get(&t.symbol) {
            Some(f) => f,
            None => {
                error!("binance missing funding {}", t.symbol);
                failed += 1;
                continue;
            }
        };
        let funding = funding_now.rate;
        if funding <= funding_threshold {
            continue;
        }
        let fresh = match client.ticker(&t.symbol).await {
            Ok(row) => row,
            Err(e) => {
                error!("binance ticker {}: {}", t.symbol, e);
                failed += 1;
                continue;
            }
        };
        if fresh.vol_quote < Decimal::from(500_000) || fresh.last <= Decimal::ZERO {
            continue;
        }
        let prior_pct = fresh.prior_24h_pct();
        if prior_pct.abs() < Decimal::from(3) {
            continue;
        }
        let Some(kind) = classify(prior_pct, funding) else {
            continue;
        };
        let triggered_at = now_ms();
        let quote = quotes
            .as_ref()
            .and_then(|rows| rows.get(&t.symbol))
            .copied();
        let snapshot = CandidateSnapshot {
            scan_started_at,
            observed_at: triggered_at,
            inst_id: t.symbol.clone(),
            kind: kind.to_string(),
            funding_rate: funding,
            funding_period_hours: intervals
                .as_ref()
                .map(|rows| *rows.get(&t.symbol).unwrap_or(&8)),
            next_funding_at: funding_now.next_funding_at,
            prior_24h_return: prior_pct,
            reference_price: fresh.last,
            bid_price: quote.map(|(bid, _)| bid),
            ask_price: quote.map(|(_, ask)| ask),
            quote_kind: Some("top"),
            quote_observed_at: quote.map(|_| quote_at),
            volume_quote_24h: fresh.vol_quote,
            open_interest_base: None,
            max_leverage: None,
            size_decimals: None,
        };
        if let Err(e) = db.record_candidate(snapshot).await {
            error!("BN candidate {}: {}", t.symbol, e);
            failed += 1;
        } else {
            recorded += 1;
        }

        let sig = Signal {
            inst_id: t.symbol.clone(),
            kind: kind.to_string(),
            triggered_at,
            funding_rate: funding,
            prior_24h_return: prior_pct,
            entry_price: fresh.last,
        };
        match db.insert(&sig).await {
            Ok(true) => {
                triggered += 1;
                info!(
                    "BN 信号: {} {} prior={:+.2}% funding={:+.4}% entry={}",
                    t.symbol,
                    kind,
                    prior_pct,
                    funding * Decimal::from(100),
                    fresh.last
                );
            }
            Ok(false) => {}
            Err(e) => error!("binance insert {}: {}", t.symbol, e),
        }
    }
    db.finish_scan(
        scan_started_at,
        now_ms(),
        tickers.len(),
        candidates,
        recorded,
        failed,
    )
    .await?;
    info!(
        "BN 扫描完成: 候选 {} 快照 {} 失败 {} 触发 {}",
        candidates, recorded, failed, triggered
    );
    Ok(triggered)
}

// ========== 跟踪 ==========

pub async fn update_open_signals(
    client: &BinanceRestClient,
    db: &BinancePaperDb,
) -> anyhow::Result<usize> {
    let open = db.open_signals().await?;
    if open.is_empty() {
        return Ok(0);
    }
    let now = now_ms();
    let mut updated = 0usize;

    for row in &open {
        for (_, col, target_ts) in due_outcomes(row, now) {
            match client.price_at_time(&row.inst_id, target_ts).await {
                Ok(Some(price)) => {
                    if let Err(e) = db.update_price(row.id, col, price).await {
                        error!("BN update {} {}: {}", row.id, col, e);
                    } else {
                        updated += 1;
                    }
                }
                Ok(None) => error!("BN no price for {} at ts {}", row.inst_id, target_ts),
                Err(e) => error!("BN fetch {} price: {}", row.inst_id, e),
            }
        }
    }
    if updated > 0 {
        info!("BN 跟踪更新: {} 条价格", updated);
    }
    Ok(updated)
}

// ========== 后台任务 ==========

pub async fn run_daemon(db_path: &str, funding_threshold: Decimal) -> anyhow::Result<()> {
    let db = Arc::new(BinancePaperDb::open(db_path)?);
    let client = Arc::new(BinanceRestClient::new());

    info!("BN 纸上交易启动: 数据库={}", db_path);

    let db1 = db.clone();
    let client1 = client.clone();
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(SCAN_INTERVAL_SECS));
        loop {
            ticker.tick().await;
            if let Err(e) = scan_once(&client1, &db1, funding_threshold).await {
                error!("BN scan_once: {}", e);
            }
        }
    });

    let db2 = db.clone();
    let client2 = client.clone();
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(TRACK_INTERVAL_SECS));
        loop {
            ticker.tick().await;
            if let Err(e) = update_open_signals(&client2, &db2).await {
                error!("BN update_open_signals: {}", e);
            }
        }
    });

    tokio::signal::ctrl_c().await?;
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
