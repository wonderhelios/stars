from pathlib import Path
p=Path('/Users/wonder/Code/stars/research/txflow_verify_20261008/snapshot/src/txflow.rs');s=p.read_text()
s=s.replace('let scenario=mode.to_string();\n let app=', 'let scenario=mode.to_string();let read_mode=mode.to_string();\n let app=')
s=s.replace('let a=a.clone(); let r=r.clone(); async move {\n     assert_eq!', 'let a=a.clone(); let r=r.clone();let read_mode=read_mode.clone(); async move {\n     use axum::response::IntoResponse;\n     let n=r.load(Ordering::SeqCst);\n     if n>=1 && read_mode=="readfail" {r.fetch_add(1,Ordering::SeqCst);return (axum::http::StatusCode::SERVICE_UNAVAILABLE,"unknown account").into_response();}\n     if n>=1 && read_mode=="readtimeout" {r.fetch_add(1,Ordering::SeqCst);tokio::time::sleep(Duration::from_millis(200)).await;}\n     assert_eq!')
s=s.replace('Json(json!({"marginSummary":{"accountValue":"2900"},"assetPositions":ps}))','Json(json!({"marginSummary":{"accountValue":"2900"},"assetPositions":ps})).into_response()')
pos=s.index(' #[tokio::test] async fn verify_attack_b_unplanned')
s=s[:pos]+r'''
 #[tokio::test] async fn verify_attack_b_final_read_failure() {for mode in ["readfail","readtimeout"] {let(o,r)=execution_scenario(mode).await;assert!(o.aborted);println!("B final {mode} fail closed reads={r}");}}
'''+s[pos:]
# clone incomplete test with stale nonempty cache
start=s.index(' #[tokio::test] async fn incomplete_metadata_is_rejected_atomically()');end=s.index('\n }',start)+3
f=s[start:end].replace('incomplete_metadata_is_rejected_atomically','verify_attack_c_stale_metadata')
f=f.replace(' let result = refresh_meta_only(&state).await;', ''' {let mut meta=state.meta.lock().await;meta.universe=(0..80).map(|i|crate::hl::CoinMeta{name:format!("OLD{i}-USDC"),sz_decimals:2,max_leverage:10,is_delisted:false}).collect();meta.refreshed_at=1;}
 let result = refresh_meta_only(&state).await;''')
f=f.replace('assert!(state.meta.lock().await.universe.is_empty());','assert_eq!(state.meta.lock().await.universe.len(),80);assert_eq!(state.meta.lock().await.refreshed_at,1);println!("C stale nonempty 80 kept but refresh error propagated; trade entry uses ?");')
s+= '\n#[cfg(test)] mod verify_c_extra { use super::fix_regressions::*; }\n' if False else ''
s=s[:end]+ '\n'+f+s[end:];p.write_text(s)
