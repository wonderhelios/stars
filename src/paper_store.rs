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
                 ON signals_v3(inst_id, triggered_at);",
        )?;
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
}
