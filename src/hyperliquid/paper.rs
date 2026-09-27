use std::sync::Arc;
use std::time::Duration;

use rust_decimal::Decimal;
use tokio::time::interval;
use tracing::{error, info};

use super::rest::HyperliquidRestClient;
use crate::paper_store::{due_outcomes, PaperDb};
use crate::signal::{classify, Signal};

pub type HyperliquidPaperDb = PaperDb;

const SCAN_INTERVAL_SECS: u64 = 300;
const TRACK_INTERVAL_SECS: u64 = 60;
/// HL 小时费率 × 8 = 等效 8h 费率
const HL_HOURS_PER_8H: i64 = 8;

// ========== 扫描 ==========

pub async fn scan_once(
    client: &HyperliquidRestClient,
    db: &HyperliquidPaperDb,
    funding_threshold: Decimal,
) -> anyhow::Result<usize> {
    let tickers = client.all_perp_ctxs_all_dexes().await?;
    let mut triggered = 0usize;
    let mut candidates = 0usize;

    for t in &tickers {
        if t.day_ntl_vlm < Decimal::from(500_000) {
            continue;
        }
        if t.prev_day_px.is_zero() {
            continue;
        }
        let prior_pct = t.prior_24h_pct();
        if prior_pct.abs() < Decimal::from(3) {
            continue;
        }
        candidates += 1;

        let funding_8h_equiv = t.funding * Decimal::from(HL_HOURS_PER_8H);
        if funding_8h_equiv <= funding_threshold {
            continue;
        }
        let Some(kind) = classify(prior_pct, funding_8h_equiv) else {
            continue;
        };
        let triggered_at = now_ms();

        let sig = Signal {
            inst_id: t.coin.clone(),
            kind: kind.to_string(),
            triggered_at,
            funding_rate: t.funding,
            prior_24h_return: prior_pct,
            entry_price: t.mark_px,
        };
        match db.insert(&sig).await {
            Ok(true) => {
                triggered += 1;
                info!(
                    "HL 信号: {} {} prior={:+.2}% fund(h)={:+.5}% fund(8h_eq)={:+.4}% entry={}",
                    t.coin,
                    kind,
                    prior_pct,
                    t.funding * Decimal::from(100),
                    funding_8h_equiv * Decimal::from(100),
                    t.mark_px
                );
            }
            Ok(false) => {}
            Err(e) => error!("HL insert {}: {}", t.coin, e),
        }
    }
    info!("HL 扫描完成: 候选 {} 触发 {}", candidates, triggered);
    Ok(triggered)
}

// ========== 跟踪 ==========

pub async fn update_open_signals(
    client: &HyperliquidRestClient,
    db: &HyperliquidPaperDb,
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
                        error!("HL update {} {}: {}", row.id, col, e);
                    } else {
                        updated += 1;
                    }
                }
                Ok(None) => error!("HL no price for {} at ts {}", row.inst_id, target_ts),
                Err(e) => error!("HL fetch {} price: {}", row.inst_id, e),
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
    if updated > 0 {
        info!("HL 跟踪更新: {} 条价格", updated);
    }
    Ok(updated)
}

// ========== 后台任务 ==========

#[allow(dead_code)]
pub async fn run_daemon(db_path: &str, funding_threshold: Decimal) -> anyhow::Result<()> {
    let db = Arc::new(HyperliquidPaperDb::open(db_path)?);
    let client = Arc::new(HyperliquidRestClient::new());

    info!("HL 纸上交易启动: 数据库={}", db_path);

    let db1 = db.clone();
    let client1 = client.clone();
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(SCAN_INTERVAL_SECS));
        loop {
            ticker.tick().await;
            if let Err(e) = scan_once(&client1, &db1, funding_threshold).await {
                error!("HL scan_once: {}", e);
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
                error!("HL update_open_signals: {}", e);
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
