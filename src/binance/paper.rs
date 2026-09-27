use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use rusqlite::{params, Connection};
use rust_decimal::Decimal;
use serde::Serialize;
use tokio::time::interval;
use tracing::{error, info};

use super::rest::BinanceRestClient;
use crate::signal::{classify, TRADE_COST_PCT};

const SCAN_INTERVAL_SECS: u64 = 300;
const TRACK_INTERVAL_SECS: u64 = 60;

#[derive(Debug, Clone, Serialize)]
pub struct BinanceSignalRow {
    pub id: i64,
    pub symbol: String,
    pub kind: String,
    pub triggered_at: i64,
    pub funding_rate: Decimal,
    pub prior_24h_return: Decimal,
    pub entry_price: Decimal,
    pub t1_price: Option<Decimal>,
    pub t4_price: Option<Decimal>,
    pub t8_price: Option<Decimal>,
    pub t24_price: Option<Decimal>,
}

pub struct BinancePaperDb {
    conn: Arc<Mutex<Connection>>,
}

impl BinancePaperDb {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("open sqlite at {}", path))?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS binance_signals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                symbol TEXT NOT NULL,
                kind TEXT NOT NULL,
                triggered_at INTEGER NOT NULL,
                funding_rate TEXT NOT NULL,
                prior_24h_return TEXT NOT NULL,
                entry_price TEXT NOT NULL,
                t1_price TEXT,
                t4_price TEXT,
                t8_price TEXT,
                t24_price TEXT,
                UNIQUE(symbol, triggered_at)
            );
            CREATE INDEX IF NOT EXISTS idx_binance_signals_triggered
                ON binance_signals(triggered_at);
            ",
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub async fn has_recent_signal(
        &self,
        symbol: &str,
        kind: &str,
        now_ms: i64,
        window_ms: i64,
    ) -> anyhow::Result<bool> {
        let conn = self.conn.clone();
        let symbol = symbol.to_string();
        let kind = kind.to_string();
        let cutoff = now_ms - window_ms;
        tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
            let c = conn.lock().unwrap();
            let count: i64 = c.query_row(
                "SELECT COUNT(*) FROM binance_signals
                 WHERE symbol = ?1 AND kind = ?2 AND triggered_at >= ?3",
                params![symbol, kind, cutoff],
                |row| row.get(0),
            )?;
            Ok(count > 0)
        })
        .await?
    }

    pub async fn insert(
        &self,
        symbol: &str,
        kind: &str,
        triggered_at: i64,
        funding_rate: Decimal,
        prior_24h_return: Decimal,
        entry_price: Decimal,
    ) -> anyhow::Result<bool> {
        let conn = self.conn.clone();
        let symbol = symbol.to_string();
        let kind = kind.to_string();
        tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
            let c = conn.lock().unwrap();
            let rows = c.execute(
                "INSERT OR IGNORE INTO binance_signals
                 (symbol, kind, triggered_at, funding_rate, prior_24h_return, entry_price)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    symbol,
                    kind,
                    triggered_at,
                    funding_rate.to_string(),
                    prior_24h_return.to_string(),
                    entry_price.to_string(),
                ],
            )?;
            Ok(rows > 0)
        })
        .await?
    }

    pub async fn open_signals(&self) -> anyhow::Result<Vec<BinanceSignalRow>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<BinanceSignalRow>> {
            let c = conn.lock().unwrap();
            let mut stmt = c.prepare(
                "SELECT id, symbol, kind, triggered_at, funding_rate, prior_24h_return, entry_price,
                        t1_price, t4_price, t8_price, t24_price
                 FROM binance_signals
                 WHERE t24_price IS NULL
                 ORDER BY triggered_at ASC",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(BinanceSignalRow {
                        id: row.get(0)?,
                        symbol: row.get(1)?,
                        kind: row.get(2)?,
                        triggered_at: row.get(3)?,
                        funding_rate: Decimal::from_str(&row.get::<_, String>(4)?)
                            .unwrap_or(Decimal::ZERO),
                        prior_24h_return: Decimal::from_str(&row.get::<_, String>(5)?)
                            .unwrap_or(Decimal::ZERO),
                        entry_price: Decimal::from_str(&row.get::<_, String>(6)?)
                            .unwrap_or(Decimal::ZERO),
                        t1_price: row
                            .get::<_, Option<String>>(7)?
                            .and_then(|s| Decimal::from_str(&s).ok()),
                        t4_price: row
                            .get::<_, Option<String>>(8)?
                            .and_then(|s| Decimal::from_str(&s).ok()),
                        t8_price: row
                            .get::<_, Option<String>>(9)?
                            .and_then(|s| Decimal::from_str(&s).ok()),
                        t24_price: row
                            .get::<_, Option<String>>(10)?
                            .and_then(|s| Decimal::from_str(&s).ok()),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?
    }

    pub async fn all_signals(&self) -> anyhow::Result<Vec<BinanceSignalRow>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<BinanceSignalRow>> {
            let c = conn.lock().unwrap();
            let mut stmt = c.prepare(
                "SELECT id, symbol, kind, triggered_at, funding_rate, prior_24h_return, entry_price,
                        t1_price, t4_price, t8_price, t24_price
                 FROM binance_signals
                 ORDER BY triggered_at ASC",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(BinanceSignalRow {
                        id: row.get(0)?,
                        symbol: row.get(1)?,
                        kind: row.get(2)?,
                        triggered_at: row.get(3)?,
                        funding_rate: Decimal::from_str(&row.get::<_, String>(4)?)
                            .unwrap_or(Decimal::ZERO),
                        prior_24h_return: Decimal::from_str(&row.get::<_, String>(5)?)
                            .unwrap_or(Decimal::ZERO),
                        entry_price: Decimal::from_str(&row.get::<_, String>(6)?)
                            .unwrap_or(Decimal::ZERO),
                        t1_price: row
                            .get::<_, Option<String>>(7)?
                            .and_then(|s| Decimal::from_str(&s).ok()),
                        t4_price: row
                            .get::<_, Option<String>>(8)?
                            .and_then(|s| Decimal::from_str(&s).ok()),
                        t8_price: row
                            .get::<_, Option<String>>(9)?
                            .and_then(|s| Decimal::from_str(&s).ok()),
                        t24_price: row
                            .get::<_, Option<String>>(10)?
                            .and_then(|s| Decimal::from_str(&s).ok()),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?
    }

    async fn update_price(
        &self,
        id: i64,
        column: &'static str,
        price: Decimal,
    ) -> anyhow::Result<()> {
        let conn = self.conn.clone();
        let price_s = price.to_string();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let c = conn.lock().unwrap();
            let sql = match column {
                "t1_price" => "UPDATE binance_signals SET t1_price = ?1 WHERE id = ?2",
                "t4_price" => "UPDATE binance_signals SET t4_price = ?1 WHERE id = ?2",
                "t8_price" => "UPDATE binance_signals SET t8_price = ?1 WHERE id = ?2",
                "t24_price" => "UPDATE binance_signals SET t24_price = ?1 WHERE id = ?2",
                _ => return Err(anyhow::anyhow!("bad column: {}", column)),
            };
            c.execute(sql, params![price_s, id])?;
            Ok(())
        })
        .await?
    }
}

// ========== 扫描 ==========

pub async fn scan_once(
    client: &BinanceRestClient,
    db: &BinancePaperDb,
    funding_threshold: Decimal,
) -> anyhow::Result<usize> {
    let tickers = client.all_tickers().await?;
    let now_ms = now_ms();
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
        let Some(kind) = classify(prior_pct, funding) else {
            continue;
        };

        // 去重：6 小时内同 symbol + kind 不重复记录
        match db
            .has_recent_signal(&t.symbol, kind, now_ms, 6 * 3_600_000)
            .await
        {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                error!("binance has_recent {}: {}", t.symbol, e);
                continue;
            }
        }

        match db
            .insert(&t.symbol, kind, now_ms, funding, prior_pct, t.last)
            .await
        {
            Ok(true) => {
                triggered += 1;
                info!(
                    "BN 信号: {} {} prior={:+.2}% funding={:+.4}% entry={}",
                    t.symbol,
                    kind,
                    prior_pct,
                    funding * Decimal::from(100),
                    t.last
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
        let elapsed_h = (now - row.triggered_at) / 3_600_000;
        for (h, col, cur) in [
            (1i64, "t1_price", row.t1_price),
            (4, "t4_price", row.t4_price),
            (8, "t8_price", row.t8_price),
            (24, "t24_price", row.t24_price),
        ] {
            if cur.is_none() && elapsed_h >= h {
                let target_ts = row.triggered_at + h * 3_600_000;
                match client.price_at_time(&row.symbol, target_ts).await {
                    Ok(Some(price)) => {
                        if let Err(e) = db.update_price(row.id, col, price).await {
                            error!("BN update {} {}: {}", row.id, col, e);
                        } else {
                            updated += 1;
                        }
                    }
                    Ok(None) => {
                        error!("BN no price for {} at ts {}", row.symbol, target_ts);
                    }
                    Err(e) => {
                        error!("BN fetch {} price: {}", row.symbol, e);
                    }
                }
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
