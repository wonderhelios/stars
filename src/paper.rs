use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use rust_decimal::Decimal;
use tokio::time::interval;
use tracing::{error, info};

use crate::okx::RestClient;
pub use crate::paper_store::PaperDb;
use crate::paper_store::{due_outcomes, SignalRow};
use crate::signal::{classify, Signal, TRADE_COST_PCT};

const SCAN_INTERVAL_SECS: u64 = 300;
const TRACK_INTERVAL_SECS: u64 = 60;

// ========== 扫描任务 ==========

pub async fn scan_once(
    client: &RestClient,
    db: &PaperDb,
    funding_threshold: Decimal,
) -> anyhow::Result<usize> {
    let tickers = client.all_tickers_usdt_swap().await?;

    let mut triggered = 0usize;
    let mut candidates = 0usize;

    for t in &tickers {
        if t.volume_quote_24h() < Decimal::from(500_000) {
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

        let funding = match client.funding_rate(&t.inst_id).await {
            Ok(f) => f,
            Err(e) => {
                error!("funding {}: {}", t.inst_id, e);
                continue;
            }
        };

        if funding <= funding_threshold {
            continue;
        }

        // 逐个请求资金费会耗时；重新读取此刻的价格和 24h 变化，避免旧快照充当入场价。
        let fresh = match client.ticker(&t.inst_id).await {
            Ok(row) => row,
            Err(e) => {
                error!("ticker {}: {}", t.inst_id, e);
                continue;
            }
        };
        if fresh.volume_quote_24h() < Decimal::from(500_000) || fresh.last <= Decimal::ZERO {
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
            inst_id: t.inst_id.clone(),
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
                    "信号触发: {} {} prior={:+.2}% funding={:+.4}% entry={}",
                    sig.inst_id,
                    sig.kind,
                    sig.prior_24h_return,
                    sig.funding_rate * Decimal::from(100),
                    sig.entry_price
                );
            }
            Ok(false) => {}
            Err(e) => error!("insert {}: {}", sig.inst_id, e),
        }
    }

    info!("扫描完成: 候选 {} 触发 {}", candidates, triggered);
    Ok(triggered)
}

// ========== 跟踪任务 ==========

pub async fn update_open_signals(client: &RestClient, db: &PaperDb) -> anyhow::Result<usize> {
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
                        error!("update {} {}: {}", row.id, col, e);
                    } else {
                        updated += 1;
                    }
                }
                Ok(None) => error!("no price for {} at ts {}", row.inst_id, target_ts),
                Err(e) => error!("fetch {} price at {}: {}", row.inst_id, target_ts, e),
            }
        }
    }

    if updated > 0 {
        info!("跟踪更新: {} 条价格", updated);
    }
    Ok(updated)
}

// ========== 报表 ==========

pub async fn report(db: &PaperDb) -> anyhow::Result<()> {
    let rows = db.all_signals().await?;
    if rows.is_empty() {
        println!("\n暂无信号记录\n");
        return Ok(());
    }

    let cost = Decimal::from_str(TRADE_COST_PCT).unwrap();

    println!("\n\n========== 纸上交易复盘 ==========");
    println!("  信号总数: {}", rows.len());

    let completed = rows.iter().filter(|r| r.t24_price.is_some()).count();
    let pending = rows.len() - completed;
    println!("  已完成 T+24h: {}  未完成: {}", completed, pending);

    use std::collections::BTreeMap;
    let mut by_kind: BTreeMap<String, Vec<&SignalRow>> = BTreeMap::new();
    for r in &rows {
        by_kind.entry(r.kind.clone()).or_default().push(r);
    }

    for (kind, group) in &by_kind {
        println!("\n  【{}】共 {} 个信号", kind, group.len());

        for (h, label) in [(1i64, "T+ 1h"), (4, "T+ 4h"), (8, "T+ 8h"), (24, "T+24h")] {
            let mut nets: Vec<Decimal> = Vec::new();
            for r in group {
                let p = match h {
                    1 => r.t1_price,
                    4 => r.t4_price,
                    8 => r.t8_price,
                    24 => r.t24_price,
                    _ => None,
                };
                let Some(p) = p else { continue };
                if r.entry_price.is_zero() {
                    continue;
                }
                let raw_ret = (p - r.entry_price) / r.entry_price * Decimal::from(100);
                nets.push(-raw_ret - cost);
            }

            if nets.is_empty() {
                println!("      {}: 暂无数据", label);
                continue;
            }

            let n = nets.len();
            let sum: Decimal = nets.iter().copied().sum();
            let avg = sum / Decimal::from(n as i64);
            let mut sorted = nets.clone();
            sorted.sort();
            let median = sorted[n / 2];
            let win = nets.iter().filter(|v| **v > Decimal::ZERO).count();
            let wr = Decimal::from(win as i64) / Decimal::from(n as i64) * Decimal::from(100);

            println!(
                "      {}: 净均{:+.2}%  中位{:+.2}%  胜率{:.1}%  n={}",
                label, avg, median, wr, n
            );
        }
    }

    Ok(())
}

// ========== 运行入口 ==========

pub async fn run_daemon(db_path: &str, funding_threshold: Decimal) -> anyhow::Result<()> {
    let db = Arc::new(PaperDb::open(db_path)?);
    let client = Arc::new(RestClient::new());

    info!(
        "纸上交易监控启动: 数据库={} funding阈值={}",
        db_path, funding_threshold
    );
    info!(
        "扫描间隔: {}s  跟踪间隔: {}s",
        SCAN_INTERVAL_SECS, TRACK_INTERVAL_SECS
    );

    let db1 = db.clone();
    let client1 = client.clone();
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(SCAN_INTERVAL_SECS));
        loop {
            ticker.tick().await;
            if let Err(e) = scan_once(&client1, &db1, funding_threshold).await {
                error!("scan_once: {}", e);
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
                error!("update_open_signals: {}", e);
            }
        }
    });

    tokio::signal::ctrl_c().await?;
    info!("收到 Ctrl+C，退出");
    Ok(())
}

pub async fn run_scan_once(db_path: &str, funding_threshold: Decimal) -> anyhow::Result<()> {
    let db = PaperDb::open(db_path)?;
    let client = RestClient::new();
    let n = scan_once(&client, &db, funding_threshold).await?;
    println!("扫描完成: 触发 {} 个信号", n);
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
