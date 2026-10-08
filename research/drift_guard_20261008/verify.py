"""Compile the actual FactorPanel excerpt, compare all available daily weights."""
from panel import *
import subprocess,tempfile,pathlib
src=pathlib.Path('/Users/wonder/Code/stars/src/trader.rs').read_text()
cfg=src[src.index('#[derive(Clone, Debug)]\npub struct TradeConfig'):src.index('#[derive(Clone, Debug)]\npub struct Order')]
fp=src[src.index('pub struct FactorPanel'):src.index('/// 目标权重（带符号')]
preamble='''use std::collections::{BTreeMap,BTreeSet,HashMap};
use std::io::{self,BufRead};
struct Candle {t:i64,c:f64,v:f64}
struct PanelEntry {coin:String,candles:Vec<Candle>}
'''
main='''fn main(){let mut map: BTreeMap<String,Vec<Candle>>=BTreeMap::new();
for line in io::stdin().lock().lines(){let s=line.unwrap();let p:Vec<_>=s.split_whitespace().collect();map.entry(p[0].into()).or_default().push(Candle{t:p[1].parse().unwrap(),c:p[2].parse().unwrap(),v:p[3].parse().unwrap()});}
let panel:Vec<_>=map.into_iter().map(|(coin,candles)|PanelEntry{coin,candles}).collect();let fp=FactorPanel::build(&panel);
let cfg=TradeConfig{target_positions:8,rebalance_slices:1,..Default::default()};
for i in 32..fp.ts.len()-1 {for (coin,w) in fp.weights_at(i,&cfg,570.) {println!("{} {} {:.17}",i,coin,w);}}}
'''
with tempfile.TemporaryDirectory(prefix='drift-parity-') as tmp:
 path=pathlib.Path(tmp);(path/'check.rs').write_text(preamble+cfg+fp+main)
 subprocess.run(['rustc','-O',str(path/'check.rs'),'-o',str(path/'check')],check=True,capture_output=True)
 inp=''.join(f'{coin} {int(x["t"])} {x["c"]} {x["v"]}\n' for coin,cs in raw.items() for x in cs)
 out=subprocess.run([str(path/'check')],input=inp,text=True,capture_output=True,check=True).stdout
 actual=np.zeros((n,k));ni={c:j for j,c in enumerate(names)}
 for row in out.splitlines():i,c,w=row.split();actual[int(i),ni[c]]=float(w)
 expected,_,_=targets(8);err=float(np.max(np.abs(actual-expected)));assert err<1e-12,err
 result=dict(rust_source_sha256=hashlib.sha256(src.encode()).hexdigest(),daily_weight_rows_compared=n-33,max_abs_weight_error=err,rust_equity_hint=570,cap=8,configs='daily slices1; source defaults otherwise; min_position15 does not shrink cap8 at570')
 json.dump(result,open(OUT+'/verification.json','w'),indent=2);print(result)
