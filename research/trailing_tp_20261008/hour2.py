import grid2 as g
import glob,json,hashlib
import numpy as np,pandas as pd
from pathlib import Path
loaded_hashes={}
files=sorted(glob.glob('/tmp/hl-hist/*.json'));data={}
for p in files:
 try:
  payload=Path(p).read_bytes();loaded_hashes[p]=hashlib.sha256(payload).hexdigest();rows=json.loads(payload)
  if isinstance(rows,list):data[Path(p).stem]=rows
 except Exception:pass
btc={x['t']//86400000*86400000 for x in data.get('BTC',[]) if x['T']<g.end}
hours={}
for j,c in enumerate(g.names):
 for x in data.get(c,[]):
  if x['T']>=g.end:continue
  t=x['t']
  if t not in hours:hours[t]=[np.full(g.k,np.nan) for _ in range(3)]
  for z,v in zip(['h','l','c'],hours[t]):v[j]=float(x[z])
g.hour_steps={}
for day in sorted(btc):
 ht=[t for t in sorted(hours) if day<=t<day+86400000]
 if len(ht)==24:g.hour_steps[day]=[hours[t] for t in ht]
np.savez_compressed(g.OUT/'hour2_candles_snapshot.npz',t=sorted(hours),ohl=np.array([hours[t] for t in sorted(hours)]),names=g.names)
mask=np.array([t in g.hour_steps for t in g.ts[33:]])
rows=[]
base=g.simulate()[0]
for mode in ['coin','portfolio']:
 for A in [.1,.2,.3]:
  for D in [.05,.1,.15]:
   ends={}
   for side in ['low','close']:
    r,t,c,ex,mi=g.simulate(A,D,mode,side,hourly=True)
    ends[side]=dict(sharpe=g.sh(r[mask]),delta=g.sh(r[mask])-g.sh(base[mask]),annual_cost=float(c[mask].mean()*365),full_path_exit_count=ex,missing_marks=mi)
    np.savez_compressed(g.OUT/f'hour2_{mode}_{A}_{D}_{side}.npz',r=r,t=g.ts[33:],sample=mask)
   rows.append(dict(mode=mode,activation=A,distance=D,ends=ends));(g.OUT/'hour2_results.json').write_text(json.dumps(rows,indent=2));print(mode,A,D,flush=True)
audit=dict(files=len(data),days=int(mask.sum()),hours=len(g.hour_steps)*24,start=str(pd.to_datetime(min(g.hour_steps),unit='ms',utc=True)),end=str(pd.to_datetime(max(g.hour_steps),unit='ms',utc=True)),min_rows=min(map(len,data.values())),max_rows=max(map(len,data.values())),missing_cells=sum(int(np.isnan(x[2]).sum()) for vals in g.hour_steps.values() for x in vals),hashes=loaded_hashes)
(g.OUT/'hour2_audit.json').write_text(json.dumps(audit,indent=2))
