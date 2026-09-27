use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use rusqlite::{params, Connection};
use rust_decimal::Decimal;
use serde::Serialize;
use tokio::time::interval;
use tracing::{error, info};

use super::rest::HyperliquidRestClient;
use crate::signal::{classify, TRADE_COST_PCT};

const SCAN_INTERVAL_SECS: u64 = 300;
const TRACK_INTERVAL_SECS: u64 = 60;

/// Hyperliquid 是每小时结算一次；CEX 的费率是每 8 小时结算。
/// 为了和 CEX 的 0.05% 阈值可比，把 HL 的小时费率乘以 8 换算成等效 8h 费率。
const HL_HOURS_PER_8H: i64 = 8;

#[derive(Debug, Clone, Serialize)]
pub struct HyperliquidSignalRow {
    pub id: i64,
    pub coin: String,
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

pub struct HyperliquidPaperDb {
    conn: Arc<Mutex<Connection>>,
}

impl HyperliquidPaperDb {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("open sqlite at {}", path))?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS hyperliquid_signals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                coin TEXT NOT NULL,
                kind TEXT NOT NULL,
                triggered_at INTEGER NOT NULL,
                funding_rate TEXT NOT NULL,
                prior_24h_return TEXT NOT NULL,
                entry_price TEXT NOT NULL,
                t1_price TEXT,
                t4_price TEXT,
                t8_price TEXT,
                t24_price TEXT,
                UNIQUE(coin, triggered_at)
            );
            CREATE INDEX IF NOT EXISTS idx_hl_signals_triggered
                ON hyperliquid_signals(triggered_at);
            ",
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub async fn has_recent_signal(
        &self,
        coin: &str,
        kind: &str,
        now_ms: i64,
        window_ms: i64,
    ) -> anyhow::Result<bool> {
        let conn = self.conn.clone();
        let coin = coin.to_string();
        let kind = kind.to_string();
        let cutoff = now_ms - window_ms;
        tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
            let c = conn.lock().unwrap();
            let count: i64 = c.query_row(
                "SELECT COUNT(*) FROM hyperliquid_signals
                 WHERE coin = ?1 AND kind = ?2 AND triggered_at >= ?3",
                params![coin, kind, cutoff],
                |row| row.get(0),
            )?;
            Ok(count > 0)
        })
        .await?
    }

    pub async fn insert(
        &self,
        coin: &str,
        kind: &str,
        triggered_at: i64,
        funding_rate: Decimal,
        prior_24h_return: Decimal,
        entry_price: Decimal,
    ) -> anyhow::Result<bool> {
        let conn = self.conn.clone();
        let coin = coin.to_string();
        let kind = kind.to_string();
        tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
            let c = conn.lock().unwrap();
            let rows = c.execute(
                "INSERT OR IGNORE INTO hyperliquid_signals
                 (coin, kind, triggered_at, funding_rate, prior_24h_return, entry_price)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    coin,
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

    pub async fn open_signals(&self) -> anyhow::Result<Vec<HyperliquidSignalRow>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<HyperliquidSignalRow>> {
            let c = conn.lock().unwrap();
            let mut stmt = c.prepare(
                "SELECT id, coin, kind, triggered_at, funding_rate, prior_24h_return, entry_price,
                        t1_price, t4_price, t8_price, t24_price
                 FROM hyperliquid_signals
                 WHERE t24_price IS NULL
                 ORDER BY triggered_at ASC",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(HyperliquidSignalRow {
                        id: row.get(0)?,
                        coin: row.get(1)?,
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

    pub async fn all_signals(&self) -> anyhow::Result<Vec<HyperliquidSignalRow>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<HyperliquidSignalRow>> {
            let c = conn.lock().unwrap();
            let mut stmt = c.prepare(
                "SELECT id, coin, kind, triggered_at, funding_rate, prior_24h_return, entry_price,
                        t1_price, t4_price, t8_price, t24_price
                 FROM hyperliquid_signals
                 ORDER BY triggered_at ASC",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(HyperliquidSignalRow {
                        id: row.get(0)?,
                        coin: row.get(1)?,
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
                "t1_price" => "UPDATE hyperliquid_signals SET t1_price = ?1 WHERE id = ?2",
                "t4_price" => "UPDATE hyperliquid_signals SET t4_price = ?1 WHERE id = ?2",
                "t8_price" => "UPDATE hyperliquid_signals SET t8_price = ?1 WHERE id = ?2",
                "t24_price" => "UPDATE hyperliquid_signals SET t24_price = ?1 WHERE id = ?2",
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
    client: &HyperliquidRestClient,
    db: &HyperliquidPaperDb,
    funding_threshold: Decimal,
) -> anyhow::Result<usize> {
    let tickers = client.all_perp_ctxs().await?;
    let now_ms = now_ms();
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

        // 【关键修复】小时费率 × 8 = 等效 8 小时费率，和 CEX 阈值对齐
        let funding_8h_equiv = t.funding * Decimal::from(HL_HOURS_PER_8H);

        if funding_8h_equiv <= funding_threshold {
            continue;
        }
        let Some(kind) = classify(prior_pct, funding_8h_equiv) else {
            continue;
        };

        match db
            .has_recent_signal(&t.coin, kind, now_ms, 6 * 3_600_000)
            .await
        {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                error!("HL has_recent {}: {}", t.coin, e);
                continue;
            }
        }

        // 存库时保留原始小时费率
        match db
            .insert(&t.coin, kind, now_ms, t.funding, prior_pct, t.mark_px)
            .await
        {
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
        let elapsed_h = (now - row.triggered_at) / 3_600_000;
        for (h, col, cur) in [
            (1i64, "t1_price", row.t1_price),
            (4, "t4_price", row.t4_price),
            (8, "t8_price", row.t8_price),
            (24, "t24_price", row.t24_price),
        ] {
            if cur.is_none() && elapsed_h >= h {
                let target_ts = row.triggered_at + h * 3_600_000;
                match client.price_at_time(&row.coin, target_ts).await {
                    Ok(Some(price)) => {
                        if let Err(e) = db.update_price(row.id, col, price).await {
                            error!("HL update {} {}: {}", row.id, col, e);
                        } else {
                            updated += 1;
                        }
                    }
                    Ok(None) => {
                        error!("HL no price for {} at ts {}", row.coin, target_ts);
                    }
                    Err(e) => {
                        error!("HL fetch {} price: {}", row.coin, e);
                    }
                }
            }
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
