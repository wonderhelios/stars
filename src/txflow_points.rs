//! Official dashboard values entered by the user, never estimated from volume.
use anyhow::{Context, Result};
use axum::{extract::State, response::{IntoResponse, Response}, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::BTreeMap, path::{Path, PathBuf}};
use crate::web::AppState;
const DAY: i64 = 86_400_000;
pub fn epoch_start(ts: i64) -> i64 { (ts - 4 * DAY).div_euclid(7 * DAY) * 7 * DAY + 4 * DAY }
#[derive(Clone, Deserialize, Serialize)]
pub struct Entry {
    pub week_start: i64,
    pub trader_points: f64,
    pub referral_points: f64,
    pub final_allocation: bool,
    pub updated_at: i64,
}
#[derive(Default, Deserialize, Serialize)]
struct Book { account: String, entries: BTreeMap<i64, Entry> }
#[derive(Deserialize)]
pub struct Input {
    account: String,
    week_start: i64,
    trader_points: f64,
    referral_points: f64,
    final_allocation: bool,
}
fn file(path: &Path, account: &str) -> PathBuf {
    path.with_extension(format!("points-{}.json", alloy::primitives::keccak256(account.to_lowercase().as_bytes())))
}
fn load(path: &Path, account: &str) -> Result<Book> {
    let path = file(path, account);
    let b: Book = if path.exists() { serde_json::from_slice(&std::fs::read(path)?).context("积分记录损坏，拒绝覆盖")? }
        else { Book { account: account.to_lowercase(), ..Default::default() } };
    anyhow::ensure!(b.account.eq_ignore_ascii_case(account), "积分账户不匹配");
    Ok(b)
}
fn record(path: &Path, input: Input, now: i64) -> Result<()> {
    anyhow::ensure!(!input.account.is_empty(), "请先配置 TxFlow 账户");
    anyhow::ensure!(input.week_start >= 0 && epoch_start(input.week_start) == input.week_start && input.week_start <= epoch_start(now), "周期必须是 UTC 周一零点，不能填写未来周");
    anyhow::ensure!([input.trader_points,input.referral_points].iter().all(|x| x.is_finite() && *x >= 0. && *x <= 1e12), "积分必须为有效非负数");
    let mut b = load(path, &input.account)?;
    b.entries.insert(input.week_start, Entry { week_start: input.week_start, trader_points: input.trader_points,
        referral_points: input.referral_points, final_allocation: input.final_allocation, updated_at: now });
    let path = file(path, &input.account); let tmp = path.with_extension("tmp");
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new(); opts.write(true).create(true).truncate(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; opts.mode(0o600); }
    let mut f = opts.open(&tmp)?; f.write_all(&serde_json::to_vec(&b)?)?; f.sync_all()?;
    std::fs::rename(tmp,path)?; Ok(())
}
pub async fn get(State(state): State<AppState>) -> Response {
    let st = state.live.lock().await;
    let result = (|| -> Result<_> {
        anyhow::ensure!(st.config.txflow, "非 TxFlow 账户");
        let b = load(&state.live_path, &st.config.account)?;
        let now = crate::live::now_ms_pub();
        let activity = crate::txflow_audit::weekly_activity(&state.live_path, &st.config.account)?;
        Ok(json!({"ok":true,"account":st.config.account,"week_start":epoch_start(now),
            "entries":b.entries.values().rev().collect::<Vec<_>>(),"activity":activity,
            "source":"official_dashboard_manual","automatic_points_available":false}))
    })();
    match result { Ok(v) => Json(v).into_response(), Err(e) => Json(json!({"ok":false,"error":e.to_string()})).into_response() }
}
pub async fn save(State(state): State<AppState>, Json(input): Json<Input>) -> Response {
    let st = state.live.lock().await;
    if !st.config.txflow || !st.config.account.eq_ignore_ascii_case(&input.account) {
        return Json(json!({"ok":false,"error":"账户已变化，请刷新后再登记"})).into_response();
    }
    match record(&state.live_path,input,crate::live::now_ms_pub()) {
        Ok(()) => Json(json!({"ok":true})).into_response(),
        Err(e) => Json(json!({"ok":false,"error":e.to_string()})).into_response(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn monday_boundaries() {
        assert_eq!(epoch_start(4*DAY),4*DAY);
        assert_eq!(epoch_start(11*DAY-1),4*DAY);
        assert_eq!(epoch_start(11*DAY),11*DAY);
    }
    #[test] fn correction_replaces_and_accounts_are_isolated() {
        let dir=std::env::temp_dir().join(uuid::Uuid::new_v4().to_string()); std::fs::create_dir(&dir).unwrap();
        let p=dir.join("live.json");
        for points in [10.,20.] { record(&p,Input { account:"a".into(),week_start:4*DAY,trader_points:points,referral_points:1.,final_allocation:false },11*DAY).unwrap(); }
        let b=load(&p,"A").unwrap(); assert_eq!(b.entries.len(),1);assert_eq!(b.entries[&(4*DAY)].trader_points,20.);
        assert!(load(&p,"b").unwrap().entries.is_empty());
        for (week,points) in [(4*DAY+1,2.),(18*DAY,2.),(4*DAY,-1.),(4*DAY,f64::NAN)] {
            assert!(record(&p,Input {account:"a".into(),week_start:week,trader_points:points,referral_points:0.,final_allocation:false},11*DAY).is_err());
        }
        std::fs::write(file(&p,"a"),"broken").unwrap();
        assert!(record(&p,Input {account:"a".into(),week_start:4*DAY,trader_points:0.,referral_points:0.,final_allocation:false},11*DAY).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
