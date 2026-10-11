from pathlib import Path
import hashlib,json,subprocess
P=Path(__file__).resolve().parent;ROOT=P.parents[2];src=(ROOT/'src/trader.rs').read_text();ex=(ROOT/'src/exchange.rs').read_text()
# Extract pure planning verbatim; no signer/network/executor code included.
chunk=src[src.index('#[derive(Clone, Debug)]'):src.index('/// 共享的因子面板')]+src[src.index('/// Keep a typed failure'):src.index('/// 执行结果')]
plan=src[src.index('/// 某个币属于哪一档'):src.index('/// Keep a typed failure')]
rounding=ex[ex.index('pub fn round_size('):ex.index('/// SDK 调用重试')]
(P/'harness/src').mkdir(parents=True,exist_ok=True)
(P/'harness/Cargo.toml').write_text('[package]\nname="stars-planner-replay"\nversion="0.1.0"\nedition="2021"\n[dependencies]\nanyhow="1"\nserde={version="1",features=["derive"]}\nserde_json="1"\n')
(P/'harness/src/planner.rs').write_text('use anyhow::{Context,Result};\nuse std::collections::{HashMap,BTreeSet};\nuse crate::exchange::{Acct,MarketInfo,round_size};\n'+chunk+plan)
(P/'harness/src/exchange.rs').write_text('''use std::collections::HashMap;
#[derive(Clone,Debug)] pub struct MarketInfo{pub sz_decimals:i32,pub max_leverage:u32}
#[derive(Clone,Debug,Default)] pub struct Pos{pub size:f64,pub leverage:u32,pub is_cross:bool}
#[derive(Clone,Debug,Default)] pub struct Acct{pub equity:f64,pub positions:HashMap<String,Pos>}
pub struct OpenOrder{pub coin:String,pub sz:f64,pub px:f64,pub side:String,pub reduce_only:bool}
'''+rounding)
factor=src[src.index('pub struct FactorPanel'):src.index('/// 目标权重（带符号') ]
(P/'harness/src/factor.rs').write_text('use std::collections::{HashMap,BTreeMap,BTreeSet};\nuse crate::planner::TradeConfig;\n#[derive(Clone,serde::Deserialize)] pub struct Candle{pub t:i64,pub c:f64,pub v:f64}\n#[derive(Clone,serde::Deserialize)] pub struct PanelEntry{pub coin:String,pub candles:Vec<Candle>}\n'+factor)
mv=factor[factor.index('    pub fn weights_at'):].rstrip()[:-1].replace('pub fn weights_at','pub fn weights_mv_at').replace('[&mom_adj, &low_vol, &shock_v]','[&mom_adj, &shock_v]').replace('w / 3.0','w / 2.0')
fpath=P/'harness/src/factor.rs';fpath.write_text(fpath.read_text().rstrip()[:-1]+mv+'\n}\n')
(P/'source_manifest.json').write_text(json.dumps({'git_commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),'trader_sha256':hashlib.sha256(src.encode()).hexdigest(),'exchange_sha256':hashlib.sha256(ex.encode()).hexdigest(),'pure_planner_sha256':hashlib.sha256((chunk+plan).encode()).hexdigest(),'network_or_signer_in_harness':False},indent=2))
