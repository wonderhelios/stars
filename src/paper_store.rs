use std::str::FromStr;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use rusqlite::{params, Connection, Row};
use rust_decimal::Decimal;
use serde::Serialize;

use crate::signal::{outcome_bar_ready, Signal, OBSERVATION_WINDOW_MS};

/// 每家交易所使用独立数据库文件；表结构和去重规则共用。
pub struct PaperDb {
    conn: Arc<Mutex<Connection>>,
}

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

/// A qualifying observation from a scan, recorded even when the signal is in cooldown.
#[derive(Debug, Clone)]
pub struct CandidateSnapshot {
    pub scan_started_at: i64,
    pub observed_at: i64,
    pub inst_id: String,
    pub kind: String,
    pub funding_rate: Decimal,
    pub funding_period_hours: Option<i64>,
    pub next_funding_at: Option<i64>,
    pub prior_24h_return: Decimal,
    pub reference_price: Decimal,
    pub bid_price: Option<Decimal>,
    pub ask_price: Option<Decimal>,
    pub quote_kind: Option<&'static str>,
    pub quote_observed_at: Option<i64>,
    pub volume_quote_24h: Decimal,
    pub open_interest_base: Option<Decimal>,
    pub max_leverage: Option<u32>,
    pub size_decimals: Option<u32>,
}

/// Hourly whole-market observation used for cross-venue funding research.
#[derive(Debug, Clone)]
pub struct FundingSnapshot {
    pub observed_at: i64,
    pub inst_id: String,
    pub funding_rate: Decimal,
    pub funding_period_hours: i64,
    pub next_funding_at: Option<i64>,
    pub prior_24h_return: Decimal,
    pub reference_price: Decimal,
    pub bid_price: Option<Decimal>,
    pub ask_price: Option<Decimal>,
    pub quote_kind: Option<&'static str>,
    pub quote_observed_at: Option<i64>,
    pub volume_quote_24h: Decimal,
    pub open_interest_base: Option<Decimal>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FundingSnapshotRow {
    pub snapshot_hour: i64,
    pub observed_at: i64,
    pub inst_id: String,
    pub funding_rate: Decimal,
    pub funding_period_hours: i64,
    pub next_funding_at: Option<i64>,
    pub prior_24h_return: Decimal,
    pub reference_price: Decimal,
    pub bid_price: Option<Decimal>,
    pub ask_price: Option<Decimal>,
    pub quote_kind: Option<String>,
    pub quote_observed_at: Option<i64>,
    pub volume_quote_24h: Decimal,
    pub open_interest_base: Option<Decimal>,
}

impl PaperDb {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("open sqlite at {}", path))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;
             CREATE TABLE IF NOT EXISTS signals_v3 (
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
             CREATE INDEX IF NOT EXISTS idx_signals_v3_inst_time
                 ON signals_v3(inst_id, triggered_at);
             CREATE TABLE IF NOT EXISTS candidate_snapshots (
                 scan_started_at INTEGER NOT NULL,
                 observed_at INTEGER NOT NULL,
                 inst_id TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 funding_rate TEXT NOT NULL,
                 funding_period_hours INTEGER,
                 next_funding_at INTEGER,
                 prior_24h_return TEXT NOT NULL,
                 reference_price TEXT NOT NULL,
                 bid_price TEXT,
                 ask_price TEXT,
                 quote_kind TEXT,
                 quote_observed_at INTEGER,
                 volume_quote_24h TEXT NOT NULL,
                 open_interest_base TEXT,
                 max_leverage INTEGER,
                 size_decimals INTEGER,
                 PRIMARY KEY(scan_started_at, inst_id)
             );
             CREATE INDEX IF NOT EXISTS idx_candidate_observed
                 ON candidate_snapshots(observed_at);
             CREATE TABLE IF NOT EXISTS scan_runs (
                 scan_started_at INTEGER PRIMARY KEY,
                 completed_at INTEGER NOT NULL,
                 market_count INTEGER NOT NULL,
                 prefiltered_count INTEGER NOT NULL,
                 recorded_count INTEGER NOT NULL,
                 failed_count INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS funding_snapshots (
                 snapshot_hour INTEGER NOT NULL,
                 observed_at INTEGER NOT NULL,
                 inst_id TEXT NOT NULL,
                 funding_rate TEXT NOT NULL,
                 funding_period_hours INTEGER NOT NULL,
                 next_funding_at INTEGER,
                 prior_24h_return TEXT NOT NULL,
                 reference_price TEXT NOT NULL,
                 bid_price TEXT,
                 ask_price TEXT,
                 quote_kind TEXT,
                 quote_observed_at INTEGER,
                 volume_quote_24h TEXT NOT NULL,
                 open_interest_base TEXT,
                 PRIMARY KEY(snapshot_hour, inst_id)
             );
             CREATE INDEX IF NOT EXISTS idx_funding_snapshot_time
                 ON funding_snapshots(snapshot_hour);",
        )?;
        for name in ["max_leverage", "size_decimals"] {
            let exists: i64 = conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('candidate_snapshots') WHERE name=?1",
                [name],
                |row| row.get(0),
            )?;
            if exists == 0 {
                conn.execute(
                    &format!("ALTER TABLE candidate_snapshots ADD COLUMN {name} INTEGER"),
                    [],
                )?;
            }
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 同一合约在 24 小时观察期内只允许首个信号，类别变化也不能重复入场。
    /// 单条 SQL 完成检查和插入，避免并发扫描绕过去重。
    pub async fn insert(&self, sig: &Signal) -> anyhow::Result<bool> {
        let conn = self.conn.clone();
        let sig = sig.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
            let c = conn.lock().unwrap();
            let rows = c.execute(
                "INSERT OR IGNORE INTO signals_v3
                 (inst_id, kind, triggered_at, funding_rate, prior_24h_return, entry_price)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6
                 WHERE NOT EXISTS (
                     SELECT 1 FROM signals_v3
                     WHERE inst_id = ?1 AND triggered_at > ?7 AND triggered_at < ?8
                 )",
                params![
                    sig.inst_id,
                    sig.kind,
                    sig.triggered_at,
                    sig.funding_rate.to_string(),
                    sig.prior_24h_return.to_string(),
                    sig.entry_price.to_string(),
                    sig.triggered_at - OBSERVATION_WINDOW_MS,
                    sig.triggered_at + OBSERVATION_WINDOW_MS,
                ],
            )?;
            Ok(rows == 1)
        })
        .await?
    }

    pub async fn record_candidate(&self, snapshot: CandidateSnapshot) -> anyhow::Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let c = conn.lock().unwrap();
            c.execute(
                "INSERT OR REPLACE INTO candidate_snapshots
                 (scan_started_at, observed_at, inst_id, kind, funding_rate,
                  funding_period_hours, next_funding_at, prior_24h_return,
                  reference_price, bid_price, ask_price, quote_kind, quote_observed_at,
                  volume_quote_24h, open_interest_base, max_leverage, size_decimals)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                params![
                    snapshot.scan_started_at,
                    snapshot.observed_at,
                    snapshot.inst_id,
                    snapshot.kind,
                    snapshot.funding_rate.to_string(),
                    snapshot.funding_period_hours,
                    snapshot.next_funding_at,
                    snapshot.prior_24h_return.to_string(),
                    snapshot.reference_price.to_string(),
                    snapshot.bid_price.map(|value| value.to_string()),
                    snapshot.ask_price.map(|value| value.to_string()),
                    snapshot.quote_kind,
                    snapshot.quote_observed_at,
                    snapshot.volume_quote_24h.to_string(),
                    snapshot.open_interest_base.map(|value| value.to_string()),
                    snapshot.max_leverage,
                    snapshot.size_decimals,
                ],
            )?;
            Ok(())
        })
        .await?
    }

    /// Keep the newest observation in each UTC hour. Hourly data is enough for
    /// funding studies and avoids growing the SQLite files every five minutes.
    pub async fn record_funding_snapshots(
        &self,
        snapshots: Vec<FundingSnapshot>,
    ) -> anyhow::Result<usize> {
        if snapshots.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
            let mut c = conn.lock().unwrap();
            let tx = c.transaction()?;
            let mut written = 0usize;
            let newest = snapshots
                .iter()
                .map(|snapshot| snapshot.observed_at)
                .max()
                .unwrap_or(0);
            for snapshot in snapshots {
                if snapshot.funding_period_hours <= 0 || snapshot.reference_price <= Decimal::ZERO {
                    continue;
                }
                let snapshot_hour = snapshot.observed_at.div_euclid(3_600_000) * 3_600_000;
                tx.execute(
                    "INSERT OR REPLACE INTO funding_snapshots
                     (snapshot_hour, observed_at, inst_id, funding_rate,
                      funding_period_hours, next_funding_at, prior_24h_return,
                      reference_price, bid_price, ask_price, quote_kind,
                      quote_observed_at, volume_quote_24h, open_interest_base)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                    params![
                        snapshot_hour,
                        snapshot.observed_at,
                        snapshot.inst_id,
                        snapshot.funding_rate.to_string(),
                        snapshot.funding_period_hours,
                        snapshot.next_funding_at,
                        snapshot.prior_24h_return.to_string(),
                        snapshot.reference_price.to_string(),
                        snapshot.bid_price.map(|value| value.to_string()),
                        snapshot.ask_price.map(|value| value.to_string()),
                        snapshot.quote_kind,
                        snapshot.quote_observed_at,
                        snapshot.volume_quote_24h.to_string(),
                        snapshot.open_interest_base.map(|value| value.to_string()),
                    ],
                )?;
                written += 1;
            }
            // Bound storage while retaining enough history for rolling research.
            tx.execute(
                "DELETE FROM funding_snapshots WHERE snapshot_hour < ?1",
                [newest - 90 * 24 * 3_600_000],
            )?;
            tx.commit()?;
            Ok(written)
        })
        .await?
    }

    pub async fn latest_funding_snapshots(&self) -> anyhow::Result<Vec<FundingSnapshotRow>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<FundingSnapshotRow>> {
            let c = conn.lock().unwrap();
            let mut stmt = c.prepare(
                "SELECT snapshot_hour, observed_at, inst_id, funding_rate,
                        funding_period_hours, next_funding_at, prior_24h_return,
                        reference_price, bid_price, ask_price, quote_kind,
                        quote_observed_at, volume_quote_24h, open_interest_base
                 FROM funding_snapshots
                 WHERE snapshot_hour = (SELECT MAX(snapshot_hour) FROM funding_snapshots)
                 ORDER BY inst_id",
            )?;
            let rows = stmt
                .query_map([], parse_funding_snapshot_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?
    }

    /// Only rows linked to a finished scan should be used for portfolio research.
    pub async fn finish_scan(
        &self,
        started_at: i64,
        completed_at: i64,
        market_count: usize,
        prefiltered_count: usize,
        recorded_count: usize,
        failed_count: usize,
    ) -> anyhow::Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let c = conn.lock().unwrap();
            c.execute(
                "INSERT OR REPLACE INTO scan_runs
                 (scan_started_at, completed_at, market_count, prefiltered_count,
                  recorded_count, failed_count) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    started_at,
                    completed_at,
                    market_count as i64,
                    prefiltered_count as i64,
                    recorded_count as i64,
                    failed_count as i64,
                ],
            )?;
            Ok(())
        })
        .await?
    }

    pub async fn open_signals(&self) -> anyhow::Result<Vec<SignalRow>> {
        self.read_rows(true).await
    }

    pub async fn all_signals(&self) -> anyhow::Result<Vec<SignalRow>> {
        self.read_rows(false).await
    }

    async fn read_rows(&self, only_open: bool) -> anyhow::Result<Vec<SignalRow>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<SignalRow>> {
            let c = conn.lock().unwrap();
            let sql = if only_open {
                "SELECT id, inst_id, kind, triggered_at, funding_rate, prior_24h_return,
                        entry_price, t1_price, t4_price, t8_price, t24_price
                 FROM signals_v3
                 WHERE t1_price IS NULL OR t4_price IS NULL OR t8_price IS NULL OR t24_price IS NULL
                 ORDER BY triggered_at ASC"
            } else {
                "SELECT id, inst_id, kind, triggered_at, funding_rate, prior_24h_return,
                        entry_price, t1_price, t4_price, t8_price, t24_price
                 FROM signals_v3 ORDER BY triggered_at ASC"
            };
            let mut stmt = c.prepare(sql)?;
            let rows = stmt
                .query_map([], parse_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?
    }

    pub async fn update_price(
        &self,
        id: i64,
        column: &'static str,
        price: Decimal,
    ) -> anyhow::Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let c = conn.lock().unwrap();
            let sql = match column {
                "t1_price" => "UPDATE signals_v3 SET t1_price = ?1 WHERE id = ?2",
                "t4_price" => "UPDATE signals_v3 SET t4_price = ?1 WHERE id = ?2",
                "t8_price" => "UPDATE signals_v3 SET t8_price = ?1 WHERE id = ?2",
                "t24_price" => "UPDATE signals_v3 SET t24_price = ?1 WHERE id = ?2",
                _ => anyhow::bail!("bad outcome column: {}", column),
            };
            c.execute(sql, params![price.to_string(), id])?;
            Ok(())
        })
        .await?
    }
}

fn parse_row(row: &Row<'_>) -> rusqlite::Result<SignalRow> {
    let decimal = |index| -> rusqlite::Result<Decimal> {
        let raw: String = row.get(index)?;
        Decimal::from_str(&raw).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })
    };
    let optional = |index| -> rusqlite::Result<Option<Decimal>> {
        let raw: Option<String> = row.get(index)?;
        raw.map(|value| {
            Decimal::from_str(&value).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })
        })
        .transpose()
    };
    Ok(SignalRow {
        id: row.get(0)?,
        inst_id: row.get(1)?,
        kind: row.get(2)?,
        triggered_at: row.get(3)?,
        funding_rate: decimal(4)?,
        prior_24h_return: decimal(5)?,
        entry_price: decimal(6)?,
        t1_price: optional(7)?,
        t4_price: optional(8)?,
        t8_price: optional(9)?,
        t24_price: optional(10)?,
    })
}

fn parse_funding_snapshot_row(row: &Row<'_>) -> rusqlite::Result<FundingSnapshotRow> {
    let decimal = |index| -> rusqlite::Result<Decimal> {
        let raw: String = row.get(index)?;
        Decimal::from_str(&raw).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
    };
    let optional_decimal = |index| -> rusqlite::Result<Option<Decimal>> {
        let raw: Option<String> = row.get(index)?;
        raw.map(|value| {
            Decimal::from_str(&value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()
    };
    Ok(FundingSnapshotRow {
        snapshot_hour: row.get(0)?,
        observed_at: row.get(1)?,
        inst_id: row.get(2)?,
        funding_rate: decimal(3)?,
        funding_period_hours: row.get(4)?,
        next_funding_at: row.get(5)?,
        prior_24h_return: decimal(6)?,
        reference_price: decimal(7)?,
        bid_price: optional_decimal(8)?,
        ask_price: optional_decimal(9)?,
        quote_kind: row.get(10)?,
        quote_observed_at: row.get(11)?,
        volume_quote_24h: decimal(12)?,
        open_interest_base: optional_decimal(13)?,
    })
}

pub fn due_outcomes(row: &SignalRow, now: i64) -> Vec<(i64, &'static str, i64)> {
    [
        (1, "t1_price", row.t1_price),
        (4, "t4_price", row.t4_price),
        (8, "t8_price", row.t8_price),
        (24, "t24_price", row.t24_price),
    ]
    .into_iter()
    .filter_map(|(hours, column, price)| {
        let target = row.triggered_at + hours * 3_600_000;
        (price.is_none() && outcome_bar_ready(target, now)).then_some((hours, column, target))
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_candidate_table_gets_leverage_columns() {
        let name = format!(
            "edgeboard-migration-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(name);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE candidate_snapshots (
                scan_started_at INTEGER NOT NULL, observed_at INTEGER NOT NULL,
                inst_id TEXT NOT NULL, PRIMARY KEY(scan_started_at,inst_id)
            );",
        )
        .unwrap();
        drop(conn);
        let db = PaperDb::open(path.to_str().unwrap()).unwrap();
        let conn = db.conn.lock().unwrap();
        for name in ["max_leverage", "size_decimals"] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('candidate_snapshots') WHERE name=?1",
                    [name],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1);
        }
        drop(conn);
        drop(db);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }

    fn signal(kind: &str, triggered_at: i64) -> Signal {
        Signal {
            inst_id: "GRAM".into(),
            kind: kind.into(),
            triggered_at,
            funding_rate: Decimal::new(6, 4),
            prior_24h_return: Decimal::from(10),
            entry_price: Decimal::ONE,
        }
    }

    #[tokio::test]
    async fn one_entry_per_instrument_across_kinds_for_24_hours() {
        let db = PaperDb::open(":memory:").unwrap();
        assert!(db.insert(&signal("up_pos_fund", 1_000_000)).await.unwrap());
        assert!(!db
            .insert(&signal("pump_pos_fund", 1_000_000 + 30 * 60_000))
            .await
            .unwrap());
        assert!(!db
            .insert(&signal(
                "pump_pos_fund",
                1_000_000 + OBSERVATION_WINDOW_MS - 1
            ))
            .await
            .unwrap());
        assert!(db
            .insert(&signal("pump_pos_fund", 1_000_000 + OBSERVATION_WINDOW_MS))
            .await
            .unwrap());
        assert_eq!(db.all_signals().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn concurrent_scanners_share_the_same_cooldown() {
        let name = format!(
            "edgeboard-test-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(name);
        let db1 = PaperDb::open(path.to_str().unwrap()).unwrap();
        let db2 = PaperDb::open(path.to_str().unwrap()).unwrap();
        let first_signal = signal("up_pos_fund", 1_000_000);
        let second_signal = signal("pump_pos_fund", 1_000_000 + 30 * 60_000);
        let (first, second) = tokio::join!(db1.insert(&first_signal), db2.insert(&second_signal));
        assert_eq!(first.unwrap() as u8 + second.unwrap() as u8, 1);
        assert_eq!(db1.all_signals().await.unwrap().len(), 1);
        drop(db1);
        drop(db2);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }

    #[tokio::test]
    async fn cooldown_does_not_hide_repeated_candidate_observations() {
        let db = PaperDb::open(":memory:").unwrap();
        let first = signal("up_pos_fund", 1_000_000);
        assert!(db.insert(&first).await.unwrap());
        let snapshot = |scan_started_at| CandidateSnapshot {
            scan_started_at,
            observed_at: scan_started_at + 100,
            inst_id: first.inst_id.clone(),
            kind: first.kind.clone(),
            funding_rate: first.funding_rate,
            funding_period_hours: Some(1),
            next_funding_at: Some(3_600_000),
            prior_24h_return: first.prior_24h_return,
            reference_price: Decimal::ONE,
            bid_price: Some(Decimal::new(99, 2)),
            ask_price: Some(Decimal::new(101, 2)),
            quote_kind: Some("impact"),
            quote_observed_at: Some(scan_started_at),
            volume_quote_24h: Decimal::from(1_000_000),
            open_interest_base: Some(Decimal::from(20)),
            max_leverage: Some(3),
            size_decimals: Some(2),
        };
        db.record_candidate(snapshot(1_000_000)).await.unwrap();
        db.record_candidate(snapshot(1_300_000)).await.unwrap();
        db.finish_scan(1_000_000, 1_000_200, 100, 1, 1, 0)
            .await
            .unwrap();
        assert!(!db.insert(&signal("up_pos_fund", 1_300_100)).await.unwrap());
        let c = db.conn.lock().unwrap();
        let observed: i64 = c
            .query_row("SELECT COUNT(*) FROM candidate_snapshots", [], |r| r.get(0))
            .unwrap();
        let (leverage, decimals): (i64, i64) = c
            .query_row(
                "SELECT max_leverage,size_decimals FROM candidate_snapshots LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((leverage, decimals), (3, 2));
        let finished: i64 = c
            .query_row("SELECT COUNT(*) FROM scan_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!((observed, finished), (2, 1));
    }

    fn funding_snapshot(observed_at: i64, rate: Decimal) -> FundingSnapshot {
        FundingSnapshot {
            observed_at,
            inst_id: "BTC-USDT-SWAP".into(),
            funding_rate: rate,
            funding_period_hours: 8,
            next_funding_at: Some(observed_at + 3_600_000),
            prior_24h_return: Decimal::ONE,
            reference_price: Decimal::from(100),
            bid_price: Some(Decimal::from(99)),
            ask_price: Some(Decimal::from(101)),
            quote_kind: Some("top"),
            quote_observed_at: Some(observed_at),
            volume_quote_24h: Decimal::from(1_000_000),
            open_interest_base: None,
        }
    }

    #[tokio::test]
    async fn funding_research_keeps_latest_value_per_hour() {
        let db = PaperDb::open(":memory:").unwrap();
        db.record_funding_snapshots(vec![funding_snapshot(3_600_000, Decimal::new(1, 4))])
            .await
            .unwrap();
        db.record_funding_snapshots(vec![funding_snapshot(3_900_000, Decimal::new(2, 4))])
            .await
            .unwrap();
        let rows = db.latest_funding_snapshots().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].snapshot_hour, 3_600_000);
        assert_eq!(rows[0].observed_at, 3_900_000);
        assert_eq!(rows[0].funding_rate, Decimal::new(2, 4));
    }
}
