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

use crate::okx::RestClient;
use crate::signal::{classify, Signal, TRADE_COST_PCT};

const SCAN_INTERVAL_SECS: u64 = 300;
const TRACK_INTERVAL_SECS: u64 = 60;

#[derive(Debug, Clone, Serialize)]
pub struct SignalRow {
    pub id: i64,
    pub inst_id: String,
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

pub struct PaperDb {
    conn: Arc<Mutex<Connection>>,
}

impl PaperDb {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("open sqlite at {}", path))?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS signals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                inst_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                triggered_at INTEGER NOT NULL,
                funding_rate TEXT NOT NULL,
                prior_24h_return TEXT NOT NULL,
                entry_price TEXT NOT NULL,
                t1_price TEXT,
                t4_price TEXT,
                t8_price TEXT,
                t24_price TEXT,
                UNIQUE(inst_id, triggered_at)
            );
            CREATE INDEX IF NOT EXISTS idx_signals_triggered
                ON signals(triggered_at);
            ",
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub async fn insert(&self, sig: &Signal) -> anyhow::Result<bool> {
        let conn = self.conn.clone();
        let sig = sig.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
            let c = conn.lock().unwrap();
            let rows = c.execute(
                "INSERT OR IGNORE INTO signals
                 (inst_id, kind, triggered_at, funding_rate, prior_24h_return, entry_price)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    sig.inst_id,
                    sig.kind,
                    sig.triggered_at,
                    sig.funding_rate.to_string(),
                    sig.prior_24h_return.to_string(),
                    sig.entry_price.to_string(),
                ],
            )?;
            Ok(rows > 0)
        })
        .await?
    }

    /// 检查同一 inst_id + kind 在 window_ms 毫秒内是否已有信号
    pub async fn has_recent_signal(
        &self,
        inst_id: &str,
        kind: &str,
        now_ms: i64,
        window_ms: i64,
    ) -> anyhow::Result<bool> {
        let conn = self.conn.clone();
        let inst_id = inst_id.to_string();
        let kind = kind.to_string();
        let cutoff = now_ms - window_ms;

        tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
            let c = conn.lock().unwrap();
            let count: i64 = c.query_row(
                "SELECT COUNT(*) FROM signals
                 WHERE inst_id = ?1 AND kind = ?2 AND triggered_at >= ?3",
                params![inst_id, kind, cutoff],
                |row| row.get(0),
            )?;
            Ok(count > 0)
        })
        .await?
    }

    pub async fn open_signals(&self) -> anyhow::Result<Vec<SignalRow>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<SignalRow>> {
            let c = conn.lock().unwrap();
            let mut stmt = c.prepare(
                "SELECT id, inst_id, kind, triggered_at, funding_rate, prior_24h_return, entry_price,
                        t1_price, t4_price, t8_price, t24_price
                 FROM signals
                 WHERE t24_price IS NULL
                 ORDER BY triggered_at ASC",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(SignalRow {
                        id: row.get(0)?,
                        inst_id: row.get(1)?,
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

    pub async fn all_signals(&self) -> anyhow::Result<Vec<SignalRow>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<SignalRow>> {
            let c = conn.lock().unwrap();
            let mut stmt = c.prepare(
                "SELECT id, inst_id, kind, triggered_at, funding_rate, prior_24h_return, entry_price,
                        t1_price, t4_price, t8_price, t24_price
                 FROM signals
                 ORDER BY triggered_at ASC",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(SignalRow {
                        id: row.get(0)?,
                        inst_id: row.get(1)?,
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
                "t1_price" => "UPDATE signals SET t1_price = ?1 WHERE id = ?2",
                "t4_price" => "UPDATE signals SET t4_price = ?1 WHERE id = ?2",
                "t8_price" => "UPDATE signals SET t8_price = ?1 WHERE id = ?2",
                "t24_price" => "UPDATE signals SET t24_price = ?1 WHERE id = ?2",
                _ => return Err(anyhow::anyhow!("bad column: {}", column)),
            };
            c.execute(sql, params![price_s, id])?;
            Ok(())
        })
        .await?
    }
}

// ========== 扫描任务 ==========

pub async fn scan_once(
    client: &RestClient,
    db: &PaperDb,
    funding_threshold: Decimal,
) -> anyhow::Result<usize> {
    let tickers = client.all_tickers_usdt_swap().await?;
    let now_ms = now_ms();

    let mut triggered = 0usize;
    let mut candidates = 0usize;

    for t in &tickers {
        if t.vol_ccy_24h < Decimal::from(500_000) {
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

        let Some(kind) = classify(prior_pct, funding) else {
            continue;
        };

        let sig = Signal {
            inst_id: t.inst_id.clone(),
            kind: kind.to_string(),
            triggered_at: now_ms,
            funding_rate: funding,
            prior_24h_return: prior_pct,
            entry_price: t.last,
        };

        match db
            .has_recent_signal(&t.inst_id, kind, now_ms, 6 * 3_600_000)
            .await
        {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                error!("has_recent {}: {}", t.inst_id, e);
                continue;
            }
        }

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

    let tickers = client.all_tickers_usdt_swap().await?;
    let mut price_map: HashMap<String, Decimal> = HashMap::new();
    for t in tickers {
        price_map.insert(t.inst_id, t.last);
    }

    let now = now_ms();
    let mut updated = 0usize;

    for row in &open {
        let Some(&price) = price_map.get(&row.inst_id) else {
            continue;
        };
        let elapsed_h = (now - row.triggered_at) / 3_600_000;

        for (h, col, cur) in [
            (1i64, "t1_price", row.t1_price),
            (4, "t4_price", row.t4_price),
            (8, "t8_price", row.t8_price),
            (24, "t24_price", row.t24_price),
        ] {
            if cur.is_none() && elapsed_h >= h {
                if let Err(e) = db.update_price(row.id, col, price).await {
                    error!("update {} {}: {}", row.id, col, e);
                } else {
                    updated += 1;
                }
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
