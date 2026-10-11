//! Read-only, account-scoped evidence. Never infer complete history from a capped API.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

#[derive(Default, Serialize, Deserialize)]
struct Archive {
    account: String,
    fills: BTreeMap<String, Value>,
    ledger: BTreeMap<String, Value>,
    #[serde(default)]
    orders: BTreeMap<String, crate::live::LiveRecord>,
}
fn number(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.parse().ok()).filter(|n| n.is_finite())
}
fn id(v: &Value) -> Option<String> {
    v.as_u64().map(|n| n.to_string()).or_else(|| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
}
fn ingest(a: &mut Archive, fills: &[Value], ledger: &[Value]) {
    for f in fills {
        // No timestamp watermark: delayed fills and same-millisecond fills survive.
        let key = id(&f["tid"]).map(|tid| format!("{}:{tid}", f["coin"]))
            .unwrap_or_else(|| format!("raw:{}", alloy::primitives::keccak256(f.to_string())));
        a.fills.insert(key, f.clone());
    }
    for l in ledger {
        let key = alloy::primitives::keccak256(l.to_string()).to_string();
        a.ledger.insert(key, l.clone());
    }
}
fn summarize(a: &Archive, since: i64, _records: &[crate::live::LiveRecord]) -> Value {
    let mut realized = 0.; let mut fees = 0.; let mut volume = 0.;
    let mut missing = 0; let mut matched = 0; let mut count = 0;
    let mut first: Option<i64> = None; let mut last: Option<i64> = None;
    let mut rows = Vec::new();
    for f in a.fills.values() {
        let Some(t) = f["time"].as_i64() else { missing += 1; continue };
        if t < since { continue; }
        count += 1; first = Some(first.map_or(t, |x| x.min(t))); last = Some(last.map_or(t, |x| x.max(t)));
        let pnl = number(&f["closedPnl"]); let fee = number(&f["fee"]);
        let px = number(&f["px"]).filter(|x| *x > 0.); let size = number(&f["sz"]).filter(|x| *x > 0.);
        // Unknown fee currency is not silently converted to dollars.
        let usd_fee = matches!(f["feeToken"].as_str(), Some("USDC") | Some("USD"));
        if pnl.is_none() || fee.is_none() || !usd_fee || px.is_none() || size.is_none() || id(&f["tid"]).is_none() { missing += 1; }
        realized += pnl.unwrap_or(0.); if usd_fee { fees += fee.unwrap_or(0.); }
        volume += px.zip(size).map_or(0., |(p,s)| p*s);
        let oid = id(&f["oid"]).filter(|s| s != "0");
        let linked = oid.as_ref().and_then(|oid| a.orders.values().find(|r|
            r.live && r.tid.is_none() && r.result.contains(&format!("[oid={oid}]"))));
        if linked.is_some() { matched += 1; }
        let slip = linked.filter(|r| r.price > 0.).and_then(|r| {
            // Execution may revise the planned delta after rereading positions.
            let sign = match r.result.split_whitespace().next() { Some("买") => Some(100.), Some("卖") => Some(-100.), _ => None };
            px.zip(sign).map(|(p, sign)| (p/r.price - 1.) * sign)
        });
        rows.push(json!({"time":t,"coin":f["coin"],"tid":f["tid"],"oid":oid,
            "size":size,"price":px,"realized":pnl,"fee_usd":if usd_fee { fee } else { None },
            "system_order":linked.is_some(),"slippage_pct":slip}));
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r["time"].as_i64().unwrap_or(0)));
    rows.truncate(100);
    json!({"fills":count,"known_realized":realized,"known_fees_usd":fees,"known_volume":volume,
        "incomplete_rows":missing,"matched_fills":matched,"unassigned_fills":count-matched,
        "first_fill_at":first,"last_fill_at":last,"funding":null,"history_complete":false,
        "ledger_rows":a.ledger.len(),"rows":rows,
        "limitation":"仅已抓取流水；资金费及完整历史未核验，未关联成交不能归因于策略。"})
}

/// Independent from clear-records. Call under the live-state lock to serialize file writes.
pub fn update(path: &Path, account: &str, since: i64, fills: &[Value], ledger: &[Value], records: &[crate::live::LiveRecord]) -> Result<Value> {
    let account = account.to_lowercase();
    let hash = alloy::primitives::keccak256(account.as_bytes());
    let path = path.with_extension(format!("audit-{hash}.json"));
    let mut a: Archive = if path.exists() {
        serde_json::from_slice(&std::fs::read(&path)?).context("成交审计文件损坏，拒绝覆盖")?
    } else { Archive { account: account.clone(), ..Default::default() } };
    anyhow::ensure!(a.account == account, "成交审计账户不匹配");
    ingest(&mut a, fills, ledger);
    let marker = format!("[account={hash}]");
    for r in records.iter().filter(|r| r.live && r.tid.is_none() && r.result.contains(&marker) && r.result.contains("[oid=")) {
        a.orders.insert(format!("{}:{}:{}", r.ts, r.coin, r.result), r.clone());
    }
    let bytes = serde_json::to_vec(&a)?;
    let tmp = path.with_extension("tmp");
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new(); opts.write(true).create(true).truncate(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; opts.mode(0o600); }
    let mut file = opts.open(&tmp)?; file.write_all(&bytes)?; file.sync_all()?;
    std::fs::rename(tmp, path)?;
    Ok(summarize(&a, since, records))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_same_timestamp_and_late_fills_without_duplicates() {
        let mut a = Archive::default();
        let rows = vec![json!({"tid":1,"coin":"A","time":10,"fee":"1","feeToken":"USDC","closedPnl":"2","px":"3","sz":"4"}),
            json!({"tid":2,"coin":"A","time":10,"fee":1,"feeToken":"USDC","closedPnl":2,"px":3,"sz":4})];
        ingest(&mut a,&rows,&[]); ingest(&mut a,&rows,&[]);
        ingest(&mut a,&[json!({"tid":3,"coin":"A","time":9})],&[]);
        let s = summarize(&a,0,&[]);
        assert_eq!(s["fills"],3); assert_eq!(s["known_fees_usd"],2.);
        assert_eq!(s["incomplete_rows"],1); assert_eq!(s["unassigned_fills"],3);
        assert!(s["funding"].is_null()); assert_eq!(s["history_complete"],false);
        assert_eq!(summarize(&a,10,&[])["fills"],2);
    }
    #[test]
    fn exact_order_evidence_survives_clearing_records_and_never_crosses_accounts() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&root).unwrap(); let path = root.join("live.json");
        let r: crate::live::LiveRecord = serde_json::from_value(json!({
            "ts":10,"coin":"A","side":"买","action":"开仓","size":4.,"price":2.,"notional":8.,
            "result":format!("买 A 开仓 4 · 成交 4@3 [oid=12] [account={}]", alloy::primitives::keccak256(b"a")),
            "live":true,"pnl":null,"tid":null,"entry_px":null,"reduce_only":false
        })).unwrap();
        let fills = vec![json!({"tid":1,"oid":12,"coin":"A","time":11,"px":3,"sz":4,"fee":1,"feeToken":"USDC","closedPnl":0}),
            json!({"tid":2,"oid":123,"coin":"A","time":11,"px":3,"sz":4,"fee":1,"feeToken":"USDC","closedPnl":0})];
        let a = update(&path,"a",0,&fills,&[],&[r.clone()]).unwrap();
        assert_eq!(a["matched_fills"],1); assert_eq!(a["unassigned_fills"],1);
        let matched = a["rows"].as_array().unwrap().iter().find(|r| r["system_order"] == true).unwrap();
        assert_eq!(matched["slippage_pct"],50.);
        assert_eq!(update(&path,"a",0,&[],&[],&[]).unwrap()["matched_fills"],1);
        assert_eq!(update(&path,"b",0,&fills,&[],&[r]).unwrap()["matched_fills"],0);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn points_weeks_use_utc_monday_and_do_not_estimate_points() {
        let root=std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());std::fs::create_dir(&root).unwrap();
        let path=root.join("live.json");let monday=11*86_400_000i64;
        let fills=vec![json!({"tid":1,"coin":"A","time":monday-1,"fee":1,"feeToken":"USDC","closedPnl":2,"px":3,"sz":4}),
            json!({"tid":2,"coin":"A","time":monday,"fee":2,"feeToken":"USDC","closedPnl":3,"px":4,"sz":5})];
        update(&path,"a",0,&fills,&[],&[]).unwrap();update(&path,"a",0,&fills,&[],&[]).unwrap();
        let a=weekly_activity(&path,"a").unwrap();let w=&a["weeks"];
        assert_eq!(w.as_array().unwrap().len(),2);assert_eq!(w[0]["week_start"],monday);
        assert_eq!(w[0]["fills"],1);assert_eq!(w[0]["volume"],20.);assert_eq!(w[0]["fees"],2.);
        assert_eq!(a["history_complete"],false);assert!(a.get("points").is_none());
        assert_eq!(weekly_activity(&path,"b").unwrap()["available"],false);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn archive_survives_reload_and_rejects_corruption() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&root).unwrap(); let path = root.join("live.json");
        let rows = vec![json!({"tid":1,"coin":"A","time":10})];
        update(&path,"a",0,&rows,&[],&[]).unwrap();
        assert_eq!(update(&path,"a",0,&[],&[],&[]).unwrap()["fills"],1);
        assert_eq!(update(&path,"b",0,&[],&[],&[]).unwrap()["fills"],0);
        let file = path.with_extension(format!("audit-{}.json",alloy::primitives::keccak256(b"a")));
        std::fs::write(file,"broken").unwrap();
        assert!(update(&path,"a",0,&[],&[],&[]).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Poll independently of page views; never acquires a signer or the trading gate.
pub async fn background(state: crate::web::AppState) {
    loop {
        let result = tokio::time::timeout(std::time::Duration::from_secs(45), async {
            let st = state.live.lock().await.clone();
            if st.config.account.is_empty() { return Ok::<(), anyhow::Error>(()); }
            let since = st.baseline().and_then(|b| b.ts).unwrap_or(0);
            let exec = crate::exchange::Exec::reader_config(&st.config).await?;
            if let Some(p) = exec.txflow_pnl(since).await {
                let p = p?;
                let current = state.live.lock().await;
                if current.config.account.eq_ignore_ascii_case(&st.config.account) {
                    update(&state.live_path, &st.config.account, since, &p.raw_fills, &p.raw_ledger, &current.records)?;
                }
            }
            Ok(())
        }).await;
        match result {
            Ok(Ok(())) => {},
            other => tracing::warn!("TxFlow 成交审计抓取未完成: {other:?}"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
    }
}

/// All observed account activity, not a prediction of eligible points or strategy profit.
pub fn weekly_activity(path: &Path, account: &str) -> Result<Value> {
    let hash = alloy::primitives::keccak256(account.to_lowercase().as_bytes());
    let path = path.with_extension(format!("audit-{hash}.json"));
    if !path.exists() { return Ok(json!({"available":false,"weeks":[]})); }
    let a: Archive = serde_json::from_slice(&std::fs::read(path)?).context("成交审计文件损坏")?;
    anyhow::ensure!(a.account.eq_ignore_ascii_case(account), "成交审计账户不匹配");
    #[derive(Default, serde::Serialize)]
    struct Week { fills: usize, known_volume: f64, known_fees_usd: f64, known_realized: f64,
        incomplete_rows: usize, active_days: std::collections::BTreeSet<i64> }
    let mut weeks: BTreeMap<i64,Week> = BTreeMap::new();
    for f in a.fills.values() {
        let Some(ts) = f["time"].as_i64() else { continue };
        let w = weeks.entry(crate::txflow_points::epoch_start(ts)).or_default();
        w.fills += 1; w.active_days.insert(ts.div_euclid(86_400_000));
        let volume=number(&f["px"]).filter(|p| *p > 0.).zip(number(&f["sz"]).filter(|s| *s > 0.)).map(|(p,s)|p*s);
        let fee=number(&f["fee"]).filter(|_| matches!(f["feeToken"].as_str(),Some("USDC")|Some("USD")));
        let pnl=number(&f["closedPnl"]);
        if volume.is_none() || fee.is_none() || pnl.is_none() || id(&f["tid"]).is_none() { w.incomplete_rows += 1; }
        w.known_volume += volume.unwrap_or(0.); w.known_fees_usd += fee.unwrap_or(0.); w.known_realized += pnl.unwrap_or(0.);
    }
    Ok(json!({"available":true,"history_complete":false,"weeks":weeks.iter().rev().map(|(start,w)|
        json!({"week_start":start,"fills":w.fills,"volume":w.known_volume,"fees":w.known_fees_usd,
            "realized":w.known_realized,"active_days":w.active_days.len(),"incomplete_rows":w.incomplete_rows})).collect::<Vec<_>>()}))
}
