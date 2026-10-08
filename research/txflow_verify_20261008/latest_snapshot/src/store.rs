//! SQLite cache for Hyperliquid candles (daily + hourly).

use crate::hl::Candle;
use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS candles (
                coin TEXT NOT NULL,
                t INTEGER NOT NULL,
                o REAL, h REAL, l REAL, c REAL, v REAL,
                PRIMARY KEY (coin, t)
            );
            CREATE INDEX IF NOT EXISTS idx_candles_t ON candles(t);
            CREATE TABLE IF NOT EXISTS oi_snapshots (
                ts INTEGER NOT NULL,
                coin TEXT NOT NULL,
                oi_usd REAL NOT NULL,
                mark_px REAL NOT NULL,
                PRIMARY KEY (ts, coin)
            );
            CREATE INDEX IF NOT EXISTS idx_oi_ts ON oi_snapshots(ts);
        ",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// TxFlow-only scheduler state. SQLite serializes increments across processes.
    pub fn txflow_cursor(&self, total: usize, advance: usize) -> Result<usize> {
        anyhow::ensure!(total>0,"TxFlow cursor universe is empty");
        let mut conn=self.conn.lock().unwrap();
        conn.execute_batch("CREATE TABLE IF NOT EXISTS txflow_refresh_cursor (id INTEGER PRIMARY KEY CHECK(id=1), value INTEGER NOT NULL); INSERT OR IGNORE INTO txflow_refresh_cursor VALUES(1,0);")?;
        let tx=conn.transaction()?;
        let cursor:usize=tx.query_row("SELECT value FROM txflow_refresh_cursor WHERE id=1",[],|r|r.get::<_,u64>(0))? as usize % total;
        if advance>0 { tx.execute("UPDATE txflow_refresh_cursor SET value=? WHERE id=1",[((cursor+advance)%total) as u64])?; }
        tx.commit()?;Ok(cursor)
    }

    pub fn upsert_candles(&self, coin: &str, candles: &[Candle]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let mut n = 0;
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO candles (coin, t, o, h, l, c, v)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(coin, t) DO UPDATE SET
                   o=excluded.o, h=excluded.h, l=excluded.l, c=excluded.c, v=excluded.v",
            )?;
            for c in candles {
                stmt.execute(rusqlite::params![coin, c.t, c.o, c.h, c.l, c.c, c.v])?;
                n += 1;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// 记录一次持仓量快照（按 UTC 日对齐，同日重复写入会覆盖）。
    pub fn upsert_oi_snapshot(&self, ts: i64, rows: &[(String, f64, f64)]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let day = ts / 86_400_000 * 86_400_000;
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO oi_snapshots (ts, coin, oi_usd, mark_px)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(ts, coin) DO UPDATE SET oi_usd=excluded.oi_usd, mark_px=excluded.mark_px",
            )?;
            for (coin, oi, px) in rows {
                stmt.execute(rusqlite::params![day, coin, oi, px])?;
            }
        }
        tx.commit()?;
        Ok(rows.len())
    }

    /// 已采集的持仓量快照：天数、币数、时间跨度。
    pub fn oi_coverage(&self) -> Result<(usize, usize, Option<i64>, Option<i64>)> {
        let conn = self.conn.lock().unwrap();
        let days: usize = conn.query_row(
            "SELECT COUNT(DISTINCT ts) FROM oi_snapshots", [], |r| r.get(0))?;
        let coins: usize = conn.query_row(
            "SELECT COUNT(DISTINCT coin) FROM oi_snapshots", [], |r| r.get(0))?;
        let first: Option<i64> = conn.query_row(
            "SELECT MIN(ts) FROM oi_snapshots", [], |r| r.get(0)).ok().flatten();
        let last: Option<i64> = conn.query_row(
            "SELECT MAX(ts) FROM oi_snapshots", [], |r| r.get(0)).ok().flatten();
        Ok((days, coins, first, last))
    }

    pub fn latest_ts(&self) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT MAX(t) FROM candles", [], |r| r.get(0))?)
    }

    /// Newest cached candle timestamp for one coin.
    pub fn coin_latest_ts(&self, coin: &str) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT MAX(t) FROM candles WHERE coin = ?1",
            [coin],
            |r| r.get(0),
        )?)
    }

    pub fn all_panels(&self) -> Result<Vec<(String, Vec<Candle>)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT coin,t,o,h,l,c,v FROM candles ORDER BY coin, t")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Candle {
                    t: r.get(1)?,
                    o: r.get(2)?,
                    h: r.get(3)?,
                    l: r.get(4)?,
                    c: r.get(5)?,
                    v: r.get(6)?,
                },
            ))
        })?;
        let mut map: std::collections::BTreeMap<String, Vec<Candle>> = Default::default();
        for row in rows.flatten() {
            map.entry(row.0).or_default().push(row.1);
        }
        Ok(map.into_iter().collect())
    }

    pub fn cached_coins(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT DISTINCT coin FROM candles ORDER BY coin")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}
