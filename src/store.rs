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
                funding REAL,
                PRIMARY KEY (ts, coin)
            );
            CREATE INDEX IF NOT EXISTS idx_oi_ts ON oi_snapshots(ts);
            CREATE TABLE IF NOT EXISTS depth_daily (
                day INTEGER NOT NULL,
                coin TEXT NOT NULL,
                spread_bp REAL NOT NULL,
                bid5_usd REAL NOT NULL,
                ask5_usd REAL NOT NULL,
                mid REAL NOT NULL,
                PRIMARY KEY (day, coin)
            );
            CREATE INDEX IF NOT EXISTS idx_depth_day ON depth_daily(day);
        ",
        )?;
        // 迁移：老库的 oi_snapshots 没有 funding 列 ✗ 补上
        {
            let mut cols: Vec<String> = Vec::new();
            let has = conn
                .prepare("PRAGMA table_info(oi_snapshots)")
                .ok()
                .map(|mut st| {
                    let _ = st.query_map([], |r| r.get::<_, String>(1)).map(|m| {
                        cols = m.filter_map(|c| c.ok()).collect();
                    });
                });
            let _ = has;
            if !cols.is_empty() && !cols.iter().any(|c| c == "funding") {
                let _ = conn.execute("ALTER TABLE oi_snapshots ADD COLUMN funding REAL", []);
            }
        }
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

    /// 记录一次持仓量快照（按 UTC 日对齐，同日重复写入会覆盖）。funding 是当日资金费率。
    pub fn upsert_oi_snapshot(&self, ts: i64, rows: &[(String, f64, f64, f64)]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let day = ts / 86_400_000 * 86_400_000;
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO oi_snapshots (ts, coin, oi_usd, mark_px, funding)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(ts, coin) DO UPDATE SET
                   oi_usd=excluded.oi_usd, mark_px=excluded.mark_px, funding=excluded.funding",
            )?;
            for (coin, oi, px, f) in rows {
                stmt.execute(rusqlite::params![day, coin, oi, px, f])?;
            }
        }
        tx.commit()?;
        Ok(rows.len())
    }

    /// 记录每日盘口汇总（30 个流动性最好的币 ✗ 只存点差和前 5 档深度 ✗ 不是原始盘口）。
    pub fn upsert_depth(&self, ts: i64, rows: &[(String, f64, f64, f64, f64)]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let day = ts / 86_400_000 * 86_400_000;
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO depth_daily (day, coin, spread_bp, bid5_usd, ask5_usd, mid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(day, coin) DO UPDATE SET
                   spread_bp=excluded.spread_bp, bid5_usd=excluded.bid5_usd,
                   ask5_usd=excluded.ask5_usd, mid=excluded.mid",
            )?;
            for (coin, spread, bid5, ask5, mid) in rows {
                stmt.execute(rusqlite::params![day, coin, spread, bid5, ask5, mid])?;
            }
        }
        tx.commit()?;
        Ok(rows.len())
    }

    /// 只保留最近 max_days 天的快照 ✗ 磁盘上限保险 ✓
    pub fn prune_snapshots(&self, ts: i64, max_days: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        // 保留 today-399 .. today 共 400 个日桶 ✗ 所以阈值是 today-(max_days-1) 天
        let cutoff = ts / 86_400_000 * 86_400_000 - (max_days - 1) * 86_400_000;
        conn.execute("DELETE FROM oi_snapshots WHERE ts < ?1", [cutoff])?;
        conn.execute("DELETE FROM depth_daily WHERE day < ?1", [cutoff])?;
        Ok(())
    }

    pub fn depth_coverage(&self) -> Result<(usize, usize, Option<i64>, Option<i64>)> {
        let conn = self.conn.lock().unwrap();
        let days: usize = conn.query_row(
            "SELECT COUNT(DISTINCT day) FROM depth_daily", [], |r| r.get(0))?;
        let coins: usize = conn.query_row(
            "SELECT COUNT(DISTINCT coin) FROM depth_daily", [], |r| r.get(0))?;
        let first: Option<i64> = conn.query_row(
            "SELECT MIN(day) FROM depth_daily", [], |r| r.get(0)).ok().flatten();
        let last: Option<i64> = conn.query_row(
            "SELECT MAX(day) FROM depth_daily", [], |r| r.get(0)).ok().flatten();
        Ok((days, coins, first, last))
    }

    /// 某一天的全部 OI 行（供前端查看）。
    pub fn oi_day(&self, day: i64) -> Result<Vec<(String, f64, f64, f64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT coin, oi_usd, mark_px, funding FROM oi_snapshots WHERE ts = ?1 ORDER BY oi_usd DESC",
        )?;
        let rows = stmt.query_map([day], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, f64>(2)?,
                r.get::<_, f64>(3).unwrap_or(0.0),
            ))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
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
