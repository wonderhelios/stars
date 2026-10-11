//! Read-only, account-scoped evidence. Never infer complete history from a capped API.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

#[derive(Default, Serialize, Deserialize)]
struct Archive {
    account: String,
    #[serde(default)]
    checkpoints: Vec<Checkpoint>,
    #[serde(default)]
    funding: BTreeMap<String, Value>,
    #[serde(default)]
    funding_error: Option<String>,
    #[serde(default)]
    funding_read_until: Option<i64>,
    #[serde(default)]
    capital_flows: Vec<crate::live::CapitalFlow>,
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
        "ledger_rows":a.ledger.len(),"rows":rows,"reconciliation":reconcile(a),
        "limitation":"仅已抓取流水；资金费及完整历史未核验，未关联成交不能归因于策略。"})
}

#[derive(Clone, Serialize, Deserialize)]
struct Checkpoint { ts: i64, equity: f64, unrealized: f64 }

fn reconcile(a: &Archive) -> Value {
    let (Some(start), Some(end)) = (a.checkpoints.first(), a.checkpoints.last()) else {
        return json!({"status":"waiting","label":"等待账户对账快照","gaps":["升级后首次采集净值和浮盈，旧流水不会被当作完整账本"]});
    };
    if end.ts <= start.ts { return json!({"status":"waiting","label":"已建立对账起点，等待下次采样","start":start.ts,"gaps":[]}); }
    let inside = |t:i64| t > start.ts && t <= end.ts;
    let mut gaps = vec!["交易所流水可能截断；采样非原子，数值一致也不代表历史完整".to_string()];
    let mut realized=0.; let mut fees=0.; let mut incomplete=0;
    for f in a.fills.values() {
        let Some(t)=f["time"].as_i64() else { incomplete+=1; continue };
        if !inside(t) {continue;}
        match number(&f["closedPnl"]) { Some(v)=>realized+=v, None=>incomplete+=1 }
        match number(&f["fee"]).filter(|_|matches!(f["feeToken"].as_str(),Some("USDC")|Some("USD"))) {
            Some(v)=>fees+=v, None=>incomplete+=1
        }
    }
    if incomplete>0 { gaps.push(format!("{incomplete} 个成交字段缺失，已知部分仍列出")); }
    let mut deposits=0.; let mut flow_time_unknown=false;
    for f in &a.capital_flows {
        match f.effective_ts {
            Some(t) if inside(t) => deposits+=f.amount,
            None => { if f.ts > start.ts { flow_time_unknown=true; gaps.push("有登记出入金缺少实际到账时间".into()); } },
            _=>{}
        }
    }
    let mut exchange_deposits=0.; let mut ledger_missing=false;
    for row in a.ledger.values() {
        let Some(t)=row["time"].as_i64() else { ledger_missing=true;continue; };
        if !inside(t) {continue;}
        let d=&row["delta"];
        match (d["type"].as_str(),number(&d["amount"])) {
            (Some("deposit"),Some(v))=>exchange_deposits+=v,
            (Some("withdraw"),Some(v))=>exchange_deposits-=v,
            _=>ledger_missing=true,
        }
    }
    if ledger_missing {gaps.push("账户流水包含未解析调整或缺少时间".into());}
    let flows_disagree=(exchange_deposits-deposits).abs()>0.01;
    if flows_disagree {gaps.push("登记出入金与交易所返回流水不一致，请核对资金流水".into());}
    let mut funding=0.;let mut funding_missing=false;
    for row in a.funding.values() {
        let Some(t)=row["time"].as_i64() else {funding_missing=true;continue;};
        if !inside(t) {continue;}
        // userFunding delta.usdc is signed account income, not a rate or nominal value.
        match number(&row["delta"]["usdc"]) {Some(v)=>funding+=v,None=>funding_missing=true}
    }
    let funding_available=a.funding_error.is_none() && a.funding_read_until.map(|t|t>=end.ts).unwrap_or(false) && !funding_missing;
    if !funding_available { gaps.push(a.funding_error.clone().unwrap_or("资金费流水未完整取得或格式未识别".into())); }
    let net=end.equity-start.equity-deposits;
    let unrealized=end.unrealized-start.unrealized;
    let explained=realized-fees+unrealized+funding;
    let residual=net-explained;
    let label=if !funding_available || incomplete>0 || ledger_missing || flows_disagree || flow_time_unknown {"部分数据待补齐"}
        else if residual.abs()>0.01 {"存在待解释差额"} else {"已采集项目数值一致"};
    json!({"status":if label=="已采集项目数值一致" {"observed_balanced"} else {"incomplete"},"label":label,
        "start":start.ts,"end":end.ts,"opening_equity":start.equity,"closing_equity":end.equity,
        "net_deposit":deposits,"exchange_net_deposit":exchange_deposits,"account_net":net,
        "realized":realized,"fees":fees,"unrealized_change":unrealized,
        "known_funding":funding,"funding_available":funding_available,"explained":explained,
        "unexplained":residual,"gaps":gaps,"history_complete":false,"tolerance_usd":0.01})
}

/// Independent from clear-records. Call under the live-state lock to serialize file writes.
pub fn update(path: &Path, account: &str, since: i64, fills: &[Value], ledger: &[Value], records: &[crate::live::LiveRecord]) -> Result<Value> {
    update_with_checkpoint(path, account, since, fills, ledger, records, None)
}

fn update_with_checkpoint(path: &Path, account: &str, since: i64, fills: &[Value], ledger: &[Value], records: &[crate::live::LiveRecord],
    checkpoint: Option<(Checkpoint, Result<Vec<Value>>, Vec<crate::live::CapitalFlow>)>) -> Result<Value> {
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
    if let Some((point, funding, flows)) = checkpoint {
        match funding {
            Ok(rows) => {
                for row in rows { a.funding.insert(alloy::primitives::keccak256(row.to_string()).to_string(), row); }
                a.funding_error=None; a.funding_read_until=Some(point.ts);
            },
            Err(e)=>a.funding_error=Some(format!("资金费读取失败: {e}")),
        }
        a.capital_flows=flows;
        if a.checkpoints.last().map(|p|p.ts<point.ts).unwrap_or(true) { a.checkpoints.push(point); }
        if a.checkpoints.len()>2 { let last=a.checkpoints.pop().unwrap(); a.checkpoints.truncate(1);a.checkpoints.push(last); }
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
    fn balanced_archive() -> Archive {
        let mut a=Archive::default();
        a.checkpoints=vec![Checkpoint {ts:100,equity:500.,unrealized:5.},Checkpoint {ts:200,equity:612.,unrealized:8.}];
        a.capital_flows=vec![crate::live::CapitalFlow{ts:155,effective_ts:Some(150),amount:100.,note:String::new()}];
        ingest(&mut a,&[json!({"tid":1,"time":200,"coin":"A","closedPnl":10,"fee":2,"feeToken":"USDC"}),
            json!({"tid":2,"time":100,"coin":"A","closedPnl":999,"fee":999,"feeToken":"USDC"}),
            json!({"tid":3,"time":201,"coin":"A","closedPnl":999,"fee":999,"feeToken":"USDC"})],
            &[json!({"time":150,"delta":{"type":"deposit","amount":"100"}})]);
        a.funding.insert("1".into(),json!({"time":180,"delta":{"usdc":"1"}}));a.funding_read_until=Some(200);a
    }
    #[test] fn reconciles_same_interval_without_order_associations() {
        let a=balanced_archive();let r=reconcile(&a);
        assert!(a.orders.is_empty());assert_eq!(r["account_net"],12.);assert_eq!(r["explained"],12.);
        assert_eq!(r["unexplained"],0.);assert_eq!(r["status"],"observed_balanced");
        assert_eq!(r["history_complete"],false);assert_eq!(r["unrealized_change"],3.);
    }
    #[test] fn missing_funding_never_means_verified_zero() {
        let mut a=balanced_archive();a.funding.clear();a.funding_error=Some("资金费接口不支持".into());
        let r=reconcile(&a);assert_eq!(r["funding_available"],false);assert_eq!(r["status"],"incomplete");assert_eq!(r["unexplained"],1.);
        a.checkpoints[1].equity-=1.;assert_eq!(reconcile(&a)["status"],"incomplete","zero residual is not completeness");
    }
    #[test] fn cashflow_conflicts_and_unknown_ledger_adjustments_are_visible() {
        let mut a=balanced_archive();a.capital_flows[0].amount=50.;
        let r=reconcile(&a);assert_eq!(r["unexplained"],50.);assert!(r["gaps"].to_string().contains("出入金"));
        a=balanced_archive();a.ledger.insert("unknown".into(),json!({"time":160,"delta":{"type":"transfer","amount":1}}));
        assert_eq!(reconcile(&a)["status"],"incomplete");
    }
    #[test] fn old_archives_wait_for_real_snapshots_and_missing_fields_stay_incomplete() {
        assert_eq!(reconcile(&Archive::default())["status"],"waiting");
        let mut a=balanced_archive();a.fills.insert("missing".into(),json!({"time":170,"fee":1,"closedPnl":2}));
        assert_eq!(reconcile(&a)["status"],"incomplete");
        a.checkpoints.truncate(1);assert_eq!(reconcile(&a)["status"],"waiting");
    }
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
            let acct = exec.account().await?;
            let point = Checkpoint {ts:crate::live::now_ms_pub(),equity:acct.equity,
                unrealized:acct.positions.values().map(|p|p.unrealized_pnl).sum()};
            anyhow::ensure!(point.equity.is_finite() && point.unrealized.is_finite(), "无效的对账账户快照");
            // Read receipts after the equity sample and restrict them to the sample boundary.
            if let Some(p) = exec.txflow_pnl(since).await {
                let p = p?;
                let funding = match tokio::time::timeout(std::time::Duration::from_secs(10),exec.txflow_account_funding(since,point.ts)).await {
                    Ok(v)=>v, Err(_)=>Err(anyhow::anyhow!("资金费接口超时")),
                };
                let current = state.live.lock().await;
                if current.config.account.eq_ignore_ascii_case(&st.config.account) {
                    update_with_checkpoint(&state.live_path, &st.config.account, since, &p.raw_fills, &p.raw_ledger, &current.records,
                        Some((point,funding,current.capital_flows.clone())))?;
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
