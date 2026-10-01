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
    pub path_high: Option<Decimal>,
    pub path_high_at: Option<i64>,
    pub path_low: Option<Decimal>,
    pub path_low_at: Option<i64>,
    pub path_samples: i64,
    pub stop_2_at: Option<i64>,
    pub stop_3_at: Option<i64>,
    pub stop_5_at: Option<i64>,
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
                 path_high TEXT,
                 path_high_at INTEGER,
                 path_low TEXT,
                 path_low_at INTEGER,
                 path_samples INTEGER NOT NULL DEFAULT 0,
                 stop_2_at INTEGER,
                 stop_3_at INTEGER,
                 stop_5_at INTEGER,
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
             CREATE TABLE IF NOT EXISTS execution_watch (coin TEXT PRIMARY KEY, expires_at INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS execution_frames (
                 at INTEGER PRIMARY KEY, frame_json TEXT NOT NULL
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
             CREATE TABLE IF NOT EXISTS research_freezes (
                 version TEXT PRIMARY KEY, frozen_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_funding_snapshot_time
                 ON funding_snapshots(snapshot_hour);",
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO research_freezes VALUES ('parallel-v1', ?1)",
            [crate::paper::now_ms()],
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
        for (name, definition) in [
            ("path_high", "TEXT"),
            ("path_high_at", "INTEGER"),
            ("path_low", "TEXT"),
            ("path_low_at", "INTEGER"),
            ("path_samples", "INTEGER NOT NULL DEFAULT 0"),
            ("stop_2_at", "INTEGER"),
            ("stop_3_at", "INTEGER"),
            ("stop_5_at", "INTEGER"),
        ] {
            let exists: i64 = conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('signals_v3') WHERE name=?1",
                [name],
                |row| row.get(0),
            )?;
            if exists == 0 {
                conn.execute(
                    &format!("ALTER TABLE signals_v3 ADD COLUMN {name} {definition}"),
                    [],
                )?;
            }
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 同一合约、同一形态在 24 小时观察期内只记录首个信号。
    /// 形态发生变化时保留新的研究样本，避免某一形态抢先触发后遮住另一形态。
    /// 单条 SQL 完成检查和插入，避免并发扫描绕过去重。
    pub async fn insert(&self, sig: &Signal) -> anyhow::Result<bool> {
        let conn = self.conn.clone();
        let sig = sig.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
            let c = conn.lock().unwrap();
            let rows = c.execute(
                "INSERT OR IGNORE INTO signals_v3
                 (inst_id, kind, triggered_at, funding_rate, prior_24h_return, entry_price,
                  path_high, path_high_at, path_low, path_low_at, path_samples)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?6, ?3, ?6, ?3, 1
                 WHERE NOT EXISTS (
                     SELECT 1 FROM signals_v3
                     WHERE inst_id = ?1 AND kind = ?2
                       AND triggered_at > ?7 AND triggered_at < ?8
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

    pub async fn execution_coverage(&self) -> anyhow::Result<serde_json::Value> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let c=conn.lock().unwrap();
            let since=crate::paper::now_ms()-24*3_600_000;
            let (count,complete,first,last,gaps):(i64,i64,Option<i64>,Option<i64>,i64)=c.query_row(
                "SELECT COUNT(*),COALESCE(SUM(json_extract(frame_json,'$.complete')=1),0),MIN(at),MAX(at),
                 COALESCE(SUM(previous IS NOT NULL AND at-previous>90000),0) FROM
                 (SELECT at,frame_json,LAG(at) OVER(ORDER BY at) AS previous FROM execution_frames WHERE at>=?1)",
                [since],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
            Ok(serde_json::json!({"frames":count,"complete_frames":complete,"first_at":first,"last_at":last,"gaps":gaps}))
        }).await?
    }

    /// Immutable completed-hour observations, read without holding the writer mutex.
    pub async fn research_history(&self) -> anyhow::Result<(i64, Vec<crate::research_lab::Point>)> {
        let path = self
            .conn
            .lock()
            .unwrap()
            .path()
            .context("missing database path")?
            .to_string();
        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let c = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            c.busy_timeout(std::time::Duration::from_secs(5))?;
            let frozen = c.query_row(
                "SELECT frozen_at FROM research_freezes WHERE version='parallel-v1'",
                [],
                |r| r.get(0),
            )?;
            let end = crate::paper::now_ms().div_euclid(3_600_000) * 3_600_000;
            let mut stmt = c.prepare(
                "SELECT observed_at, inst_id, reference_price, prior_24h_return,
                        funding_rate, funding_period_hours, volume_quote_24h
                 FROM funding_snapshots WHERE snapshot_hour>=?1 AND snapshot_hour<?2 AND funding_period_hours>0
                 ORDER BY observed_at, inst_id",
            )?;
            let rows = stmt
                .query_map(
                    params![end - 30 * 24 * 3_600_000, end],
                    crate::research_lab::Point::read_sql,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            Ok((frozen, rows))
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

    pub async fn execution_watch(
        &self,
        coins: Vec<String>,
        now: i64,
    ) -> anyhow::Result<Vec<String>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let mut c = conn.lock().unwrap();
            let tx = c.transaction()?;
            for coin in coins {
                tx.execute(
                    "INSERT OR REPLACE INTO execution_watch VALUES (?1,?2)",
                    params![coin, now + 24 * 3_600_000],
                )?;
            }
            tx.execute("DELETE FROM execution_watch WHERE expires_at<?1", [now])?;
            tx.commit()?;
            let mut stmt = c.prepare("SELECT coin FROM execution_watch ORDER BY coin")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?
    }

    pub async fn record_execution_frame(
        &self,
        frame: &trading_core::replay::Frame,
    ) -> anyhow::Result<()> {
        let at = frame.at;
        let raw = serde_json::to_string(frame)?;
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let c = conn.lock().unwrap();
            c.execute(
                "INSERT OR REPLACE INTO execution_frames VALUES (?1,?2)",
                params![at, raw],
            )?;
            c.execute(
                "DELETE FROM execution_frames WHERE at < ?1",
                [at - 30 * 86_400_000],
            )?;
            Ok(())
        })
        .await?
    }

    pub async fn execution_candidate(
        &self,
        mut strategy: trading_core::strategy::StrategyConfig,
        capital: f64,
    ) -> anyhow::Result<serde_json::Value> {
        // Use a separate WAL reader so a long replay cannot block market writers.
        let path = self
            .conn
            .lock()
            .unwrap()
            .path()
            .context("missing database path")?
            .to_string();
        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let c = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            c.execute_batch("BEGIN")?;
            let (start, end): (Option<i64>, Option<i64>) =
                c.query_row("SELECT MIN(at),MAX(at) FROM execution_frames", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
            let boundary = start
                .zip(end)
                .map(|(a, b)| a + (b - a) * 7 / 10)
                .unwrap_or(0);
            let mut stmt = c.prepare("SELECT frame_json FROM execution_frames ORDER BY at")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            let frames = rows.map(|r| -> anyhow::Result<_> {
                let mut frame: trading_core::replay::Frame = serde_json::from_str(&r?)?;
                frame.markets.retain(|coin, _| strategy.includes_coin(coin));
                frame.books.retain(|coin, _| strategy.includes_coin(coin));
                Ok(frame)
            });
            let report = trading_core::replay::run_iter(frames, &strategy, capital, boundary)?;
            let evidence = serde_json::to_value(report)?;
            let mut proof = evidence.clone();
            if let Some(fields) = proof.as_object_mut() {
                fields.remove("trades");
            }
            strategy.research_evidence = Some(proof);
            Ok(serde_json::json!({"report":evidence,"strategy":strategy}))
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
                        entry_price, t1_price, t4_price, t8_price, t24_price,
                        path_high, path_high_at, path_low, path_low_at, path_samples,
                        stop_2_at, stop_3_at, stop_5_at
                 FROM signals_v3
                 WHERE t1_price IS NULL OR t4_price IS NULL OR t8_price IS NULL OR t24_price IS NULL
                 ORDER BY triggered_at ASC"
            } else {
                "SELECT id, inst_id, kind, triggered_at, funding_rate, prior_24h_return,
                        entry_price, t1_price, t4_price, t8_price, t24_price,
                        path_high, path_high_at, path_low, path_low_at, path_samples,
                        stop_2_at, stop_3_at, stop_5_at
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

    /// Record the best observed path information for every still-open signal.
    /// Callers can pass a whole-market price map, so this adds no per-signal HTTP requests.
    pub async fn update_path_extremes(
        &self,
        observed_at: i64,
        prices: std::collections::HashMap<String, Decimal>,
    ) -> anyhow::Result<usize> {
        if prices.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
            let mut c = conn.lock().unwrap();
            let tx = c.transaction()?;
            let mut updated = 0usize;
            for (inst_id, price) in prices {
                if price <= Decimal::ZERO {
                    continue;
                }
                updated += tx.execute(
                    "UPDATE signals_v3 SET
                         path_high_at = CASE WHEN path_high IS NULL OR CAST(path_high AS REAL) < CAST(?1 AS REAL) THEN ?2 ELSE path_high_at END,
                         path_high = CASE WHEN path_high IS NULL OR CAST(path_high AS REAL) < CAST(?1 AS REAL) THEN ?1 ELSE path_high END,
                         path_low_at = CASE WHEN path_low IS NULL OR CAST(path_low AS REAL) > CAST(?1 AS REAL) THEN ?2 ELSE path_low_at END,
                         path_low = CASE WHEN path_low IS NULL OR CAST(path_low AS REAL) > CAST(?1 AS REAL) THEN ?1 ELSE path_low END,
                         path_samples = path_samples + 1,
                         stop_2_at = CASE WHEN stop_2_at IS NULL AND CAST(?1 AS REAL) >= CAST(entry_price AS REAL) * 1.02 THEN ?2 ELSE stop_2_at END,
                         stop_3_at = CASE WHEN stop_3_at IS NULL AND CAST(?1 AS REAL) >= CAST(entry_price AS REAL) * 1.03 THEN ?2 ELSE stop_3_at END,
                         stop_5_at = CASE WHEN stop_5_at IS NULL AND CAST(?1 AS REAL) >= CAST(entry_price AS REAL) * 1.05 THEN ?2 ELSE stop_5_at END
                     WHERE inst_id = ?3 AND triggered_at <= ?2 AND triggered_at > ?4",
                    params![
                        price.to_string(),
                        observed_at,
                        inst_id,
                        observed_at - OBSERVATION_WINDOW_MS,
                    ],
                )?;
            }
            tx.commit()?;
            Ok(updated)
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
        path_high: optional(11)?,
        path_high_at: row.get(12)?,
        path_low: optional(13)?,
        path_low_at: row.get(14)?,
        path_samples: row.get(15)?,
        stop_2_at: row.get(16)?,
        stop_3_at: row.get(17)?,
        stop_5_at: row.get(18)?,
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

    #[test]
    fn existing_signal_table_gets_path_columns() {
        let name = format!(
            "edgeboard-signal-migration-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(name);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE signals_v3 (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                inst_id TEXT NOT NULL, kind TEXT NOT NULL, triggered_at INTEGER NOT NULL,
                funding_rate TEXT NOT NULL, prior_24h_return TEXT NOT NULL,
                entry_price TEXT NOT NULL, t1_price TEXT, t4_price TEXT,
                t8_price TEXT, t24_price TEXT, UNIQUE(inst_id, triggered_at)
            );",
        )
        .unwrap();
        drop(conn);
        let db = PaperDb::open(path.to_str().unwrap()).unwrap();
        let conn = db.conn.lock().unwrap();
        for name in [
            "path_high",
            "path_high_at",
            "path_low",
            "path_low_at",
            "path_samples",
            "stop_2_at",
            "stop_3_at",
            "stop_5_at",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('signals_v3') WHERE name=?1",
                    [name],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "missing migrated column {name}");
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
    async fn one_entry_per_instrument_and_kind_for_24_hours() {
        let db = PaperDb::open(":memory:").unwrap();
        assert!(db.insert(&signal("up_pos_fund", 1_000_000)).await.unwrap());
        assert!(db
            .insert(&signal("pump_pos_fund", 1_000_000 + 30 * 60_000))
            .await
            .unwrap());
        assert!(!db
            .insert(&signal(
                "up_pos_fund",
                1_000_000 + OBSERVATION_WINDOW_MS - 1
            ))
            .await
            .unwrap());
        assert!(db
            .insert(&signal("up_pos_fund", 1_000_000 + OBSERVATION_WINDOW_MS))
            .await
            .unwrap());
        assert_eq!(db.all_signals().await.unwrap().len(), 3);
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
        let second_signal = signal("up_pos_fund", 1_000_000 + 30 * 60_000);
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

    #[tokio::test]
    async fn path_tracking_records_extremes_and_first_stop_times() {
        let db = PaperDb::open(":memory:").unwrap();
        let entered_at = 1_000_000;
        assert!(db.insert(&signal("up_pos_fund", entered_at)).await.unwrap());

        for (offset, price) in [
            (60_000, Decimal::new(99, 2)),
            (120_000, Decimal::new(1021, 3)),
            (180_000, Decimal::new(106, 2)),
        ] {
            db.update_path_extremes(
                entered_at + offset,
                [("GRAM".to_string(), price)].into_iter().collect(),
            )
            .await
            .unwrap();
        }

        let row = db.all_signals().await.unwrap().remove(0);
        assert_eq!(row.path_low, Some(Decimal::new(99, 2)));
        assert_eq!(row.path_high, Some(Decimal::new(106, 2)));
        assert_eq!(row.path_samples, 4);
        assert_eq!(row.stop_2_at, Some(entered_at + 120_000));
        assert_eq!(row.stop_3_at, Some(entered_at + 180_000));
        assert_eq!(row.stop_5_at, Some(entered_at + 180_000));
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
    #[tokio::test]
    async fn execution_evidence_is_recomputed_from_persisted_frames() {
        let path = std::env::temp_dir().join(format!(
            "execution-replay-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = PaperDb::open(path.to_str().unwrap()).unwrap();
        let frame = trading_core::replay::Frame {
            at: 1_000_000,
            complete: true,
            complete_scopes: std::collections::HashSet::from(["main".into()]),
            markets: std::collections::HashMap::new(),
            books: std::collections::HashMap::new(),
        };
        db.record_execution_frame(&frame).await.unwrap();
        let mut s = trading_core::strategy::StrategyConfig::default();
        s.research_evidence = Some(serde_json::json!({"deployable":true,"equity":99999}));
        let result = db.execution_candidate(s, 500.0).await.unwrap();
        assert_eq!(result["report"]["equity"], 500.0);
        assert_eq!(result["strategy"]["research_evidence"]["deployable"], false);
        assert_eq!(result["report"]["rule_hash"], trading_core::rule_hash());
        let watched = db
            .execution_watch(vec!["AAA".into()], 1_000_000)
            .await
            .unwrap();
        assert_eq!(watched, ["AAA"]);
        assert!(db
            .execution_watch(vec![], 1_000_000 + 25 * 3_600_000)
            .await
            .unwrap()
            .is_empty());
        drop(db);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }
    #[tokio::test]
    async fn fixed_experiments_keep_cutoff_and_exclude_unfinished_hours() {
        let path = std::env::temp_dir().join(format!(
            "research-history-{}.sqlite",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = PaperDb::open(path.to_str().unwrap()).unwrap();
        let hour = crate::paper::now_ms().div_euclid(3_600_000) * 3_600_000;
        db.record_funding_snapshots(vec![
            funding_snapshot(hour - 3_600_000, Decimal::new(1, 4)),
            funding_snapshot(hour, Decimal::new(2, 4)),
        ])
        .await
        .unwrap();
        let (cutoff, rows) = db.research_history().await.unwrap();
        assert_eq!(rows.len(), 1);
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE research_freezes SET frozen_at=123 WHERE version='parallel-v1'",
                [],
            )
            .unwrap();
        drop(db);
        let reopened = PaperDb::open(path.to_str().unwrap()).unwrap();
        assert!(cutoff > 123);
        assert_eq!(reopened.research_history().await.unwrap().0, 123);
        let frame = trading_core::replay::Frame {
            at: crate::paper::now_ms() - 120_000,
            complete: true,
            complete_scopes: std::collections::HashSet::new(),
            markets: std::collections::HashMap::new(),
            books: std::collections::HashMap::new(),
        };
        reopened.record_execution_frame(&frame).await.unwrap();
        assert_eq!(reopened.execution_coverage().await.unwrap()["frames"], 1);
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }
}
