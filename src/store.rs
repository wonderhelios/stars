//! SQLite cache for Hyperliquid daily candles.

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
            CREATE INDEX IF NOT EXISTS idx_candles_t ON candles(t);",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
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

    pub fn latest_ts(&self) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT MAX(t) FROM candles", [], |r| r.get(0))?)
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
