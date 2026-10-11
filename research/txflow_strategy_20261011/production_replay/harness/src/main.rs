#![allow(dead_code)]
mod exchange;mod planner;mod factor;
use std::collections::HashMap;
use serde::Deserialize;
use serde_json::json;
use exchange::{Acct,Pos,MarketInfo};
#[derive(Deserialize)] struct Input {panel:Vec<factor::PanelEntry>,names:Vec<String>,dates:Vec<i64>,markets:Vec<(i32,u32)>,prices:HashMap<String,Vec<Vec<Option<f64>>>>,targets:HashMap<String,Vec<Vec<f64>>>}
fn main()->anyhow::Result<()>{
 let args:Vec<_>=std::env::args().collect();let input:Input=serde_json::from_slice(&std::fs::read(&args[1])?)?;
 let markets:HashMap<_,_>=input.names.iter().zip(&input.markets).map(|(n,(d,l))|(n.clone(),MarketInfo{sz_decimals:*d,max_leverage:*l})).collect();
 let lookup:HashMap<_,_>=input.names.iter().enumerate().map(|(j,n)|(n.clone(),j)).collect();
 let cfg=planner::TradeConfig{target_positions:8,min_order_pct:0.02,rebalance_slices:1,..Default::default()};
 for (d,ts) in input.dates.iter().enumerate(){
  let panel:Vec<_>=input.panel.iter().filter_map(|e|{let cs:Vec<_>=e.candles.iter().filter(|c|c.t<=*ts-86400000).cloned().collect();if cs.len()<33{return None;}let recent=&cs[cs.len()-33..];if recent.last().unwrap().t!=*ts-86400000||!recent.windows(2).all(|w|w[1].t-w[0].t==86400000){return None;}Some(factor::PanelEntry{coin:e.coin.clone(),candles:cs})}).collect();
  anyhow::ensure!(panel.len()>=20,"fresh universe");let fp=factor::FactorPanel::build(&panel);let i=fp.last_closed_index(*ts);
  for name in ["MLV","MV"]{let rust=if name=="MLV"{fp.weights_at(i,&cfg,3000.)}else{fp.weights_mv_at(i,&cfg,3000.)};let w:HashMap<_,_>=rust.into_iter().collect();for (j,n) in input.names.iter().enumerate(){anyhow::ensure!((w.get(n).copied().unwrap_or(0.)-input.targets[name][d][j]).abs()<1e-10,"factor mismatch {name} {ts} {n}");}}
 }
 let mut results=Vec::new();
 for factor in ["MLV","MV"]{for offset in ["0","5","60"]{for (case,fee,funding) in [("base",0.00075,0.),("slip6",0.00105,0.),("slip6_funding3",0.00105,0.03)]{
  let mut eq=3000.;let mut q=vec![0.;input.names.len()];let mut prev=vec![0.;q.len()];let mut rows=Vec::new();
  for d in 0..input.dates.len(){
   let before=eq;let mut mark=prev.clone();let mut mids=HashMap::new();
   for j in 0..q.len(){if let Some(p)=input.prices[offset][d][j]{mark[j]=p;mids.insert(input.names[j].clone(),p);}eq+=q[j]*(mark[j]-prev[j]);}
   anyhow::ensure!(eq>0.,"bankrupt {factor} {offset} {case} {d}");
   let positions=input.names.iter().enumerate().filter(|(j,_)|q[*j]!=0.).map(|(j,n)|(n.clone(),Pos{size:q[j],is_cross:true,leverage:3})).collect();
   let acct=Acct{equity:eq,positions};let weights:Vec<_>=input.names.iter().zip(&input.targets[factor][d]).filter(|(_,w)|w.abs()>1e-12).map(|(n,w)|(n.clone(),*w)).collect();
   let result=planner::build_txflow_plan(&weights,&acct,&[],&markets,&mids,&cfg);
   let mut traded=0.;let mut error=String::new();let mut resized=false;let mut count=0;
   match result {Ok(plan)=>{resized=plan.notes.iter().any(|x|x.starts_with("预算内缩减"));count=plan.orders.len();
    for o in plan.orders{let j=lookup[&o.coin];let delta=if o.buy{o.size}else{-o.size};let actual=if o.reduce_only {if q[j]*delta<0.{delta.signum()*delta.abs().min(q[j].abs())}else{0.}}else{delta};q[j]+=actual;traded+=actual.abs()*mark[j];}
   },Err(e)=>{error=format!("{e:#}");}}
   let mut cost=traded*fee;let gross:f64=q.iter().zip(&mark).map(|(a,p)|(a*p).abs()).sum();let net:f64=q.iter().zip(&mark).map(|(a,p)|a*p).sum();let fund=gross*funding/365.;eq-=cost+fund;
   let live_eq=eq;let missing:f64=q.iter().enumerate().filter(|(j,_)|input.prices[offset][d][*j].is_none()).map(|(j,a)|(a*mark[j]).abs()).sum();
   if d==input.dates.len()-1{cost+=gross*fee;traded+=gross;eq-=gross*fee;}
   rows.push(json!({"r":eq/before-1.,"turnover":traded/before,"cost":cost/before,"funding":fund/before,"gross":gross/live_eq,"net":net/live_eq,"missing":missing/live_eq,"accepted":error.is_empty(),"error":error,"resized":resized,"orders":count,"positions":q.iter().filter(|x|x.abs()>1e-12).count(),"equity":eq}));prev=mark;
  }
  results.push(json!({"factor":factor,"offset":offset,"case":case,"rows":rows}));
 }}}
 std::fs::write(&args[2],serde_json::to_vec(&json!({"dates":input.dates,"results":results}))?)?;Ok(())
}
#[cfg(test)] mod tests {
 use super::*;
 fn fixture()->(Acct,HashMap<String,MarketInfo>,HashMap<String,f64>,Vec<(String,f64)>,planner::TradeConfig){
  let names:Vec<_>=(0..6).map(|i|format!("C{i}")).collect();
  (Acct{equity:3000.,..Default::default()},names.iter().map(|n|(n.clone(),MarketInfo{sz_decimals:0,max_leverage:3})).collect(),names.iter().map(|n|(n.clone(),1.)).collect(),names.iter().enumerate().map(|(i,n)|(n.clone(),if i<3{1./6.}else{-1./6.})).collect(),planner::TradeConfig{rebalance_slices:1,min_order_pct:0.02,..Default::default()})
 }
 #[test] fn accepted_initial_book_uses_one_account_budget(){let(a,m,p,w,c)=fixture();let plan=planner::build_txflow_plan(&w,&a,&[],&m,&p,&c).unwrap();assert_eq!(plan.orders.len(),6);let gross:f64=plan.orders.iter().map(|o|o.notional).sum();assert!((gross-8100.).abs()<1e-8);assert!(gross/3.<=a.equity*c.margin_buffer);}
 #[test] fn rejected_plan_never_flattens_positions(){let(mut a,m,p,_,c)=fixture();a.positions.insert("C0".into(),Pos{size:100.,is_cross:true,leverage:3});assert!(planner::build_txflow_plan(&[],&a,&[],&m,&p,&c).is_err());assert_eq!(a.positions["C0"].size,100.);}
 #[test] fn resting_orders_reserve_risk_budget(){let(a,m,p,w,c)=fixture();let orders=[exchange::OpenOrder{coin:"C0".into(),sz:2000.,px:1.,side:"B".into(),reduce_only:false}];assert!(planner::build_txflow_plan(&w,&a,&orders,&m,&p,&c).is_err());}
 #[test] fn pepe_lot_exact(){assert_eq!(exchange::round_size(59_300_001.,-5),59_300_000.);}
}
