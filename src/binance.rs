//! Import already unit-adjusted Binance daily CSVs. Never contacts an exchange.
use anyhow::{Context, Result};
use std::{path::{Path, PathBuf}, sync::Arc};
use crate::{hl::Candle, store::Store};

pub fn separate(a: &Path, b: &Path) -> Result<()> {
    fn resolved(p: &Path) -> Result<PathBuf> {
        if p.exists() { return Ok(p.canonicalize()?); }
        let abs = if p.is_absolute() { p.to_owned() } else { std::env::current_dir()?.join(p) };
        // Resolve existing ancestors without creating unrelated HL directories.
        let mut normalized=PathBuf::new();
        for c in abs.components() {
            match c { std::path::Component::ParentDir=>{normalized.pop();}, std::path::Component::CurDir=>{}, _=>normalized.push(c.as_os_str()) }
        }
        let mut ancestor=normalized.as_path();let mut suffix=Vec::new();
        while !ancestor.exists() {
            suffix.push(ancestor.file_name().context("database filename")?.to_owned());
            ancestor=ancestor.parent().context("database parent")?;
        }
        let mut resolved=ancestor.canonicalize()?;
        for component in suffix.into_iter().rev() { resolved.push(component); }
        Ok(resolved)
    }
    anyhow::ensure!(resolved(a)? != resolved(b)?, "币安库不能与 HL/TxFlow 行情库相同");
    #[cfg(unix)]
    if a.exists() && b.exists() {
        use std::os::unix::fs::MetadataExt;
        let (a,b)=(a.metadata()?,b.metadata()?);
        anyhow::ensure!((a.dev(),a.ino()) != (b.dev(),b.ino()), "数据库硬链接重叠");
    }
    Ok(())
}
pub fn open_signal_store(hl: &str, tx: &str) -> Option<Arc<Store>> {
    let path = std::env::var("STARS_BINANCE_DB").unwrap_or_else(|_| Path::new(tx).with_file_name("binance-candles.sqlite").to_string_lossy().into_owned());
    let result = (|| { separate(Path::new(hl),Path::new(&path))?; separate(Path::new(tx),Path::new(&path))?; Store::open(Path::new(&path)) })();
    match result { Ok(s)=>Some(Arc::new(s)), Err(e)=>{ tracing::warn!("币安库不可用，TxFlow 拒绝计划，HL 不受影响: {e:#}"); None } }
}
#[derive(Debug, serde::Serialize)]
pub struct ImportReport { pub imported: Vec<(String, usize)>, pub skipped: Vec<(String,String)> }
pub fn import(store: &Store, mapping: &Path, daily: &Path) -> Result<ImportReport> {
    let entries: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(mapping)?)?;
    import_entries(store, &entries, daily, true)
}

pub fn import_entries(store: &Store, entries: &[serde_json::Value], daily: &Path, strict: bool) -> Result<ImportReport> {
    let mut report=ImportReport{imported:vec![],skipped:vec![]};
    let mut ready=Vec::new();
    let mut names=std::collections::HashSet::new();
    for entry in entries {
        let name=entry["txflow_name"].as_str().context("missing txflow_name")?;
        anyhow::ensure!(name.ends_with("-USDC") && !name.contains('/') && !name.contains('\\') && names.insert(name.to_owned()), "invalid/duplicate mapping name: {name}");
        let result=(|| -> Result<Vec<Candle>> {
            let text=std::fs::read_to_string(daily.join(format!("{name}.csv")))?;
            let mut lines=text.lines();
            anyhow::ensure!(lines.next()==Some("ts,open,high,low,close,volume"), "invalid CSV header");
            let mut candles=Vec::new();
            for line in lines {
                let fields:Vec<_>=line.split(',').collect();
                anyhow::ensure!(fields.len()==6,"expected six columns");
                let c=Candle{t:fields[0].parse()?,o:fields[1].parse()?,h:fields[2].parse()?,l:fields[3].parse()?,c:fields[4].parse()?,v:fields[5].parse()?};
                anyhow::ensure!(c.t>1_000_000_000_000 && c.t%86_400_000==0,"expected UTC daily milliseconds");
                anyhow::ensure!([c.o,c.h,c.l,c.c].iter().all(|x|x.is_finite() && *x>0.) && c.v.is_finite() && c.v>=0. && c.h>=c.o.max(c.c) && c.l<=c.o.min(c.c),"invalid OHLCV");
                candles.push(c);
            }
            anyhow::ensure!(candles.len()>=33,"too short: {} bars, need at least 33",candles.len());
            anyhow::ensure!(candles.windows(2).all(|w|w[1].t-w[0].t==86_400_000),"non-contiguous daily bars");
            Ok(candles)
        })();
        match result { Ok(c)=>{report.imported.push((name.into(),c.len()));ready.push((name.to_owned(),c));},Err(e)=>report.skipped.push((name.into(),format!("{e:#}"))) }
    }
    anyhow::ensure!(store.cached_coins()?.iter().all(|coin|
        names.contains(coin) && (!strict || ready.iter().any(|(name,_)|name==coin))),
        "destination contains unmapped/skipped coins; use a clean dedicated Binance DB");
    for (name,candles) in ready { store.upsert_candles(&name,&candles)?; }
    Ok(report)
}
pub fn command(args: &[String], hl: &Path) -> Result<()> {
    anyhow::ensure!(args.len()==5,"usage: stars import-binance DB MAPPING_JSON DAILY_DIR");
    let db=Path::new(&args[2]); separate(hl,db)?;
    let tx=std::env::var("STARS_TXFLOW_DB").map(PathBuf::from).unwrap_or_else(|_|hl.with_file_name("txflow-candles.sqlite"));
    separate(&tx,db)?;
    let store=Store::open(db)?;
    println!("{}",serde_json::to_string_pretty(&import(&store,Path::new(&args[3]),Path::new(&args[4]))?)?);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn import_command_rejects_hl_and_tx_database_aliases() {
        let root=std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());std::fs::create_dir_all(&root).unwrap();
        let hl=root.join("hl.sqlite");
        for db in [hl.clone(),root.join("txflow-candles.sqlite")] {
            let args=vec!["stars".into(),"import-binance".into(),db.to_string_lossy().into_owned(),"missing.json".into(),"missing".into()];
            assert!(command(&args,&hl).unwrap_err().to_string().contains("币安库不能"));
            assert!(!db.exists());
        }
        std::fs::write(&hl,"").unwrap();
        #[cfg(unix)] {
            let alias=root.join("alias");std::os::unix::fs::symlink(&hl,&alias).unwrap();assert!(separate(&hl,&alias).is_err());
            let hard=root.join("hard");std::fs::hard_link(&hl,&hard).unwrap();assert!(separate(&hl,&hard).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn import_is_idempotent_filters_mapping_preserves_units_and_skips_short_missing() {
        let root=std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());std::fs::create_dir_all(&root).unwrap();
        let mapping=root.join("mapping.json");std::fs::write(&mapping,r#"[{"txflow_name":"1000BONK-USDC"},{"txflow_name":"SHORT-USDC"},{"txflow_name":"MISSING-USDC"}]"#).unwrap();
        let mut csv="ts,open,high,low,close,volume\n".to_string();for d in 0..33 {csv+=&format!("{},2,3,1,2.5,17\n",1705017600000i64+d*86400000);}
        std::fs::write(root.join("1000BONK-USDC.csv"),&csv).unwrap();std::fs::write(root.join("EXTRA-USDC.csv"),&csv).unwrap();std::fs::write(root.join("SHORT-USDC.csv"),"ts,open,high,low,close,volume\n").unwrap();
        let store=Store::open(&root.join("db")).unwrap();for _ in 0..2 {let r=import(&store,&mapping,&root).unwrap();assert_eq!(r.imported.len(),1);assert_eq!(r.skipped.len(),2);}
        let panels=crate::trader::load_panel(&store).unwrap();assert_eq!(panels.len(),1);assert_eq!(panels[0].candles.len(),33);assert_eq!(panels[0].candles[0].c,2.5);assert_eq!(panels[0].candles[0].v,17.);
        assert!(separate(&root.join("db"),&root.join("./db")).is_err());
        drop(store);std::fs::remove_dir_all(root).unwrap();
    }
}


// ===================== 币安日线抓取（让面板保持新鲜）=====================
//
// 导入器（`import`）是**离线入口**：CSV 一旦过期，策略会因为"没有昨日的连续 K 线"而
// 拒绝生成计划（这是对的，fail-closed）。所以必须有人持续把 CSV 更新到最近已收盘日 ——
// 这就是本模块存在的理由。
//
// 币安公开接口无需密钥。`reqwest` 默认读 `HTTPS_PROXY`/`HTTP_PROXY`：
// 服务器（香港/新加坡）通常可直连；需要代理的环境只要设好环境变量即可，代码不用改。

#[derive(serde::Serialize)]
pub struct FetchReport {
    pub updated: Vec<(String, usize)>,
    pub skipped: Vec<(String, String)>,
    pub last_closed_day: String,
}

/// 只保留**已收盘**的 UTC 日线：最后一根如果是今天，就丢掉。
fn closed_only(rows: Vec<(i64, f64, f64, f64, f64, f64)>) -> Vec<(i64, f64, f64, f64, f64, f64)> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let day = 86_400_000i64;
    let today_open = now / day * day;
    rows.into_iter().filter(|r| r.0 < today_open).collect()
}

pub async fn fetch(mapping: &Path, daily: &Path, limit: usize) -> Result<FetchReport> {
    let raw = std::fs::read_to_string(mapping).with_context(|| format!("读不了映射表 {}", mapping.display()))?;
    let entries: Vec<serde_json::Value> = serde_json::from_str(&raw).context("映射表不是 JSON 数组")?;
    fetch_entries(&entries, daily, limit).await
}

pub fn default_mapping() -> Result<Vec<serde_json::Value>> {
    Ok(serde_json::from_str(include_str!("../config/binance-mapping.json"))?)
}

pub async fn fetch_entries(entries: &[serde_json::Value], daily: &Path, limit: usize) -> Result<FetchReport> {
    std::fs::create_dir_all(daily)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(40))
        .build()?;
    let mut report = FetchReport { updated: vec![], skipped: vec![], last_closed_day: String::new() };

    for e in entries {
        let tx = e.get("txflow_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let sym = e.get("binance_symbol").and_then(|v| v.as_str()).unwrap_or("").to_string();
        // 映射表给的是**价格和成交量各自的乘数**（不是互为倒数 —— 例如 1000BONK 那类
        // 合约单位不同，两个乘数并不总是一对倒数），照它给的用。
        let pmul = e.get("bn_price_to_tx_mult").and_then(|v| v.as_f64()).unwrap_or(1.0);
        let vmul = e.get("bn_volume_to_tx_mult").and_then(|v| v.as_f64()).unwrap_or(1.0);
        if !tx.ends_with("-USDC") || !tx.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
            || sym.is_empty() || !sym.bytes().all(|c| c.is_ascii_alphanumeric()) {
            report.skipped.push((tx, "映射缺 txflow 或 binance_symbol".into()));
            continue;
        }
        if !(pmul.is_finite() && pmul > 0.0 && vmul.is_finite() && vmul > 0.0) {
            report.skipped.push((tx, format!("倍数非法 price={pmul} volume={vmul}")));
            continue;
        }
        let url = format!(
            "https://api.binance.com/api/v3/klines?symbol={sym}&interval=1d&limit={limit}"
        );
        // 瞬态失败重试两次；418/429/451 是限流或封禁，不盲重试
        let mut body: Option<String> = None;
        let mut last_err = String::new();
        for attempt in 0..3 {
            match client.get(&url).send().await {
                Ok(resp) => {
                    let code = resp.status();
                    if code.is_success() {
                        body = Some(resp.text().await.unwrap_or_default());
                        break;
                    }
                    last_err = format!("HTTP {code}");
                    if matches!(code.as_u16(), 403 | 418 | 429 | 451) {
                        anyhow::bail!("币安公共行情暂不可用: {code}（未继续请求其余币种）");
                    }
                }
                Err(err) => last_err = err.to_string(),
            }
            if attempt < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(800 * (attempt as u64 + 1))).await;
            }
        }
        let Some(text) = body else {
            report.skipped.push((tx, format!("币安请求失败: {last_err}")));
            continue;
        };
        let arr: Vec<Vec<serde_json::Value>> = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(err) => { report.skipped.push((tx, format!("币安回执无法解析: {err}"))); continue; }
        };
        let mut rows = Vec::with_capacity(arr.len());
        for k in &arr {
            if k.len() < 6 { continue; }
            let num = |i: usize| -> Option<f64> {
                k[i].as_str().and_then(|s| s.parse::<f64>().ok())
                    .or_else(|| k[i].as_f64())
            };
            let (Some(t), Some(o), Some(h), Some(l), Some(c), Some(v)) =
                (k[0].as_i64(), num(1), num(2), num(3), num(4), num(5)) else { continue };
            if !(o.is_finite() && h.is_finite() && l.is_finite() && c.is_finite() && v.is_finite())
                || c <= 0.0 || v < 0.0 { continue; }
            rows.push((t, o * pmul, h * pmul, l * pmul, c * pmul, v * vmul));
        }
        let rows = closed_only(rows);
        if rows.len() < 40 {
            report.skipped.push((tx, format!("可用的已收盘日线只有 {} 根（<40）", rows.len())));
            continue;
        }
        let mut out = String::from("ts,open,high,low,close,volume\n");
        for (t, o, h, l, c, v) in &rows {
            out.push_str(&format!("{t},{o},{h},{l},{c},{v}\n"));
        }
        // 先写临时文件再原子改名：避免导入器读到写了一半的 CSV
        let path = daily.join(format!("{tx}.csv"));
        let tmp = daily.join(format!(".{tx}.csv.tmp"));
        std::fs::write(&tmp, out.as_bytes())?;
        std::fs::rename(&tmp, &path)?;
        if let Some(last) = rows.last() {
            let day = last.0 / 86_400_000;
            report.last_closed_day = format!("{day}");
        }
        report.updated.push((tx, rows.len()));
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
    Ok(report)
}

/// `stars fetch-binance MAPPING_JSON DAILY_DIR [LIMIT]`
pub async fn fetch_command(args: &[String]) -> Result<()> {
    anyhow::ensure!(args.len() == 4 || args.len() == 5,
        "usage: stars fetch-binance MAPPING_JSON DAILY_DIR [LIMIT]");
    let limit = if args.len() == 5 { args[4].parse::<usize>().context("LIMIT 不是数字")? } else { 1000 };
    let rep = fetch(Path::new(&args[2]), Path::new(&args[3]), limit).await?;
    println!("{}", serde_json::to_string_pretty(&rep)?);
    Ok(())
}

#[cfg(test)]
mod refresh_regressions {
    use super::*;
    #[test]
    fn embedded_mapping_preserves_verified_units_without_research_files() {
        let mapping = default_mapping().unwrap();
        assert_eq!(mapping.len(), 82);
        let names: std::collections::HashSet<_> = mapping.iter().map(|e| e["txflow_name"].as_str().unwrap()).collect();
        assert_eq!(names.len(), 82);
        let bonk = mapping.iter().find(|e| e["txflow_name"] == "1000BONK-USDC").unwrap();
        assert_eq!(bonk["bn_price_to_tx_mult"], 1000);
        assert_eq!(bonk["bn_volume_to_tx_mult"], 0.001);
    }
    #[test]
    fn refresh_import_keeps_missing_symbols_but_updates_healthy_ones_and_rejects_foreign_db() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let store = Store::open(&dir.join("db")).unwrap();
        let entries = serde_json::from_str::<Vec<serde_json::Value>>(r#"[{"txflow_name":"A-USDC"},{"txflow_name":"B-USDC"}]"#).unwrap();
        let bars: Vec<_> = (0..40).map(|i| Candle { t:1705017600000+i*86400000, o:2.,h:3.,l:1.,c:2.,v:10. }).collect();
        store.upsert_candles("A-USDC", &bars).unwrap();
        store.upsert_candles("B-USDC", &bars).unwrap();
        let mut csv = "ts,open,high,low,close,volume\n".to_string();
        for c in &bars { csv += &format!("{},2,3,1,2.5,17\n", c.t); }
        std::fs::write(dir.join("A-USDC.csv"), csv).unwrap();
        assert!(import_entries(&store, &entries, &dir, true).is_err());
        let rep = import_entries(&store, &entries, &dir, false).unwrap();
        assert_eq!(rep.imported.len(), 1); assert_eq!(rep.skipped.len(), 1);
        let panel = crate::trader::load_panel(&store).unwrap();
        assert_eq!(panel.iter().find(|e| e.coin == "A-USDC").unwrap().candles[0].c, 2.5);
        assert_eq!(panel.iter().find(|e| e.coin == "B-USDC").unwrap().candles[0].c, 2.);
        store.upsert_candles("FOREIGN", &bars).unwrap();
        assert!(import_entries(&store, &entries, &dir, false).is_err());
        drop(store); std::fs::remove_dir_all(dir).unwrap();
    }
}
