use std::sync::Arc;
use std::time::Duration;

use rust_decimal::Decimal;
use tokio::time::interval;
use tracing::{error, info};

use super::rest::BinanceRestClient;
use crate::paper_store::{due_outcomes, PaperDb};
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
    let tickers = client.all_tickers().await?;
    let mut triggered = 0usize;
    let mut candidates = 0usize;

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

        let funding = match client.funding_rate(&t.symbol).await {
            Ok(f) => f,
            Err(e) => {
                error!("binance funding {}: {}", t.symbol, e);
                continue;
            }
        };
        if funding <= funding_threshold {
            continue;
        }
        let fresh = match client.ticker(&t.symbol).await {
            Ok(row) => row,
            Err(e) => {
                error!("binance ticker {}: {}", t.symbol, e);
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
    info!("BN 扫描完成: 候选 {} 触发 {}", candidates, triggered);
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
