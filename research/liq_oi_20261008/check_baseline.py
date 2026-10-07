"""Independent scalar translation of trader.rs to check baseline vectorization."""
import engine as e
import numpy as np,json,pandas as pd
from pathlib import Path
OUT=Path(__file__).resolve().parent

def scalar(i):
 vals=[]
 for name,d in e.raw.items():
  if ':' in name:continue
  cm={int(x['t']):float(x['c']) for x in d if float(x['c'])>0};vm={int(x['t']):float(x['v'])*float(x['c']) for x in d if float(x['c'])>0}
  vv=[vm[t] for t in e.times[i-30:i] if t in vm]
  if len(vv)<5 or np.mean(vv)<5e6 or e.times[i] not in cm or e.times[i-14] not in cm:continue
  rets=[cm[e.times[j]]/cm[e.times[j-1]]-1 for j in range(i-19,i+1) if e.times[j] in cm and e.times[j-1] in cm]
  if len(rets)<5:continue
  sd=max(np.std(rets,ddof=1),1e-9)
  vals.append((name,(cm[e.times[i]]/cm[e.times[i-14]]-1)/sd,-sd,vm[e.times[i]]/np.mean(vv)))
 w={}
 if len(vals)<8:return np.zeros(e.k)
 k=min(max(1,int(np.floor(len(vals)*.2+.5))),5,len(vals)//2)
 for f in [1,2,3]:
  order=sorted(vals,key=lambda v:(v[f],v[0]))
  for j,v in enumerate(order):
   a=.5/k if j>=len(order)-k else (-.5/k if j<k else 0)
   w[v[0]]=w.get(v[0],0)+a/3
 r=np.array([w.get(name,0) for name in e.names]);g=np.abs(r).sum()
 return r/g if g>0 else r
base=np.zeros((e.n,e.k));num=[]
for i in range(32,e.n-1):
 idx=np.flatnonzero(np.array([':' not in c for c in e.names])&(e.V[i]>=5e6)&np.isfinite(e.P[i])&np.isfinite(e.P[i-14])&np.isfinite(e.SD[i]));num.append(len(idx))
 if len(idx)<8:continue
 t=sum(e.book(idx,s,5) for s in [e.M[i]/np.maximum(e.SD[i],1e-9),-e.SD[i],e.shock[i]])/3;g=np.abs(t).sum();base[i]=t/g if g>0 else t
checks=[]
for i in [45,80,180,365,550,750,1000,e.n-3]:
 ref=scalar(i);err=float(np.max(np.abs(ref-base[i])));assert err<1e-10,(i,err);checks.append({'date':str(e.dates[i]),'max_weight_error':err})
rr=[e.run(base,offset=p) for p in range(3)]
summary={period:{'phase_sharpes':[e.stat(v['r'][m])['sharpe'] for v in rr],'sharpe':float(np.mean([e.stat(v['r'][m])['sharpe'] for v in rr]))} for period,m in e.mask.items()}
json.dump({'scalar_parity_checks':checks,'summary':summary,'liquid_universe_mean':float(np.mean(num)),'legacy_headline_comparability':'trader.rs cap5, vol includes current day ddof1, prior30d volume missing excluded, byte-sum slots. Legacy /tmp/stag2.py uses cap8, vol excludes current day ddof0, liquidity includes current Q divided by 30, index slots, fixed unit weights at same close. Thus headline 1.79 is not an exact matched baseline for this engine.'},open(OUT/'baseline_check.json','w'),indent=2)
out={'date':e.outdates.astype(str)}
for p,v in enumerate(rr):out['baseline_phase'+str(p)]=v['r']
pd.DataFrame(out).to_csv(OUT/'baseline_returns.csv',index=False)
np.save(OUT/'baseline_targets.npy',base)
print(summary,flush=True)
