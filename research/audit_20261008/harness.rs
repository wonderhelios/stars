#[path="src_snapshot/exchange.rs"] mod exchange;
#[path="src_snapshot/hl.rs"] mod hl;
#[path="src_snapshot/momentum.rs"] mod momentum;
#[path="src_snapshot/store.rs"] mod store;
#[path="src_snapshot/trader.rs"] mod trader;
#[path="src_snapshot/live.rs"] mod live;
#[path="src_snapshot/paper.rs"] mod paper;
fn main() {}
#[cfg(test)] mod audit {
use super::*;
#[test] fn rollback_config_is_now_propagated(){
 let c=live::LiveConfig {rebalance_slices:1,..Default::default()};
 assert_eq!(c.trade_config().rebalance_slices,1);
 println!("saved slices=1 actual slices={}",c.trade_config().rebalance_slices);
}
#[test] fn one_slice_reaches_every_coin_and_directions_are_correct(){
 let cfg=trader::TradeConfig {rebalance_slices:1,min_position_usd:0.0,..Default::default()};
 let names=["BTC","ETH","SOL","DOGE","ARB","HYPE"];
 let mids=names.iter().map(|s|(s.to_string(),100.0)).collect();
 let markets=names.iter().map(|s|(s.to_string(),exchange::MarketInfo{sz_decimals:3,max_leverage:25})).collect();
 let a=exchange::Acct{equity:1000.0,..Default::default()};
 let w=names.iter().enumerate().map(|(i,s)|(s.to_string(),if i%2==0{1.0/6.0}else{-1.0/6.0})).collect::<Vec<_>>();
 let p=trader::build_plan(&w,&a,&markets,&mids,&cfg,None);assert_eq!(p.orders.len(),6);
 for o in p.orders {assert_eq!(o.buy,o.target>0.0);assert!(!o.reduce_only);}
 for (cur,targetw,buy,reduce) in [(10.,0.5,true,false),(20.,0.5,false,true),(-10.,-0.5,false,false),(-20.,-0.5,true,true)] {
  let a=exchange::Acct{equity:1000.0,positions:[("BTC".into(),exchange::Pos{size:cur,..Default::default()})].into()};
  let p=trader::build_plan(&[("BTC".into(),targetw)],&a,&markets,&mids,&cfg,None);
  assert_eq!(p.orders.len(),1);assert_eq!(p.orders[0].buy,buy);assert_eq!(p.orders[0].reduce_only,reduce);
 }
 println!("one slice: all six names; long/short add/reduce PASS");
}
}

#[cfg(test)] mod reconcile_audit {
use crate::live::{LiveState,LiveRecord}; use anyhow::Result;
struct Exec {fills:serde_json::Value}
impl Exec { async fn user_fills(&self,_:usize)->Result<serde_json::Value>{Ok(self.fills.clone())} }
fn now_ms_pub()->i64 {10000}
pub async fn reconcile_fills(exec: &Exec, state: &mut LiveState) -> Result<usize> {
    let Some(v) = exec.user_fills(500).await.ok() else {
        return Ok(0);
    };
    let Some(arr) = v.as_array() else {
        return Ok(0);
    };
    // 首次对账绝不能回溯：水位为 0 时直接对齐到「现在」，导入 0 笔。
    //
    // 否则 userFills 会把账户有史以来的成交全倒进来 —— 包括这个策略上线前的
    // 几个月、上一套策略的、甚至别的品种的，已实现盈亏会瞬间变成 −623 这种
    // 荒谬数字。这个错误真实发生过。
    if state.reconciled_to == 0 {
        state.reconciled_to = now_ms_pub();
        return Ok(0);
    }
    let known: std::collections::HashSet<u64> =
        state.records.iter().filter_map(|r| r.tid).collect();
    let watermark = state.reconciled_to;
    let mut added = 0usize;
    let mut max_ts = watermark;
    for f in arr {
        let ts = f["time"].as_i64().unwrap_or(0);
        if ts <= watermark {
            continue;
        }
        let Some(tid) = f["tid"].as_u64() else { continue };
        if known.contains(&tid) {
            continue;
        }
        let coin = f["coin"].as_str().unwrap_or("").to_string();
        // 只认主 DEX 的币：HIP-3 的名字带 ':'，我们从不交易它们。
        if coin.is_empty() || coin.contains(':') {
            continue;
        }
        let sz: f64 = f["sz"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        let px: f64 = f["px"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        let dir = f["dir"].as_str().unwrap_or("");
        let pnl: f64 = f["closedPnl"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        // 只补「不是我们主动发单」的成交；我们自己发的单在执行时已经记过了。
        // 判断依据：成交方向是平仓、且当前记录里没有同一时刻的这笔。
        let is_close = dir.contains("Close");
        if !is_close {
            continue;
        }
        state.records.push(LiveRecord {
            ts,
            coin: coin.clone(),
            side: if f["side"].as_str() == Some("B") { "买".into() } else { "卖".into() },
            action: "止盈/被动成交".into(),
            size: sz,
            price: px,
            notional: sz * px,
            result: format!("被动成交 · 已实现 {:+.2}", pnl),
            live: true,
            pnl: Some(pnl),
            tid: Some(tid),
            entry_px: None,
            reduce_only: true,
        });
        added += 1;
        if ts > max_ts {
            max_ts = ts;
        }
    }
    if added > 0 || max_ts > watermark {
        state.reconciled_to = max_ts;
    }
    Ok(added)
}





#[tokio::test] async fn active_close_is_imported_again(){
 let mut st=LiveState::default();st.reconciled_to=100;
 st.records.push(LiveRecord{ts:150,coin:"BTC".into(),side:"卖".into(),action:"平仓".into(),size:1.,price:110.,notional:110.,result:"成交 1@110".into(),live:true,pnl:None,tid:None,entry_px:Some(100.),reduce_only:true});
 let e=Exec{fills:serde_json::json!([{"time":151,"tid":99,"coin":"BTC","sz":"1","px":"110","dir":"Close Long","closedPnl":"10","side":"A"}])};
 assert_eq!(reconcile_fills(&e,&mut st).await.unwrap(),1);assert_eq!(st.records.len(),2);
 println!("same active close now has two PnL records");
}
#[tokio::test] async fn timestamp_watermark_loses_late_equal_time_fill(){
 let mut st=LiveState::default();st.reconciled_to=100;
 let fill=serde_json::json!({"time":101,"tid":1,"coin":"BTC","sz":"1","px":"110","dir":"Close Long","closedPnl":"10","side":"A"});
 let e=Exec{fills:serde_json::json!([fill])};assert_eq!(reconcile_fills(&e,&mut st).await.unwrap(),1);
 let mut f=e.fills[0].clone();f["tid"]=serde_json::json!(2);let e=Exec{fills:serde_json::json!([f])};
 assert_eq!(reconcile_fills(&e,&mut st).await.unwrap(),0);
 println!("new tid at watermark timestamp is lost");
}
}
