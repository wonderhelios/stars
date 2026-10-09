import trailing_grid_full as g
import json,hashlib
from pathlib import Path
import numpy as np
S=g.SAVE;hour={};manifest={};coverage={}
for f in sorted(Path('/tmp/hl-hist').glob('*.json')):
 blob=f.read_bytes();manifest[str(f)]=hashlib.sha256(blob).hexdigest();d=[x for x in json.loads(blob) if x['T']<g.end]
 coverage[f.stem]=set(int(x['t']) for x in d)
 if f.stem not in g.names:continue
 j=g.names.index(f.stem)
 for x in d:hour.setdefault(int(x['t']),[]).append((j,x))
btc=coverage['BTC'];first=(min(btc)//86400000+1)*86400000;last=max(btc)//86400000*86400000;hd={}
for t,b in sorted(hour.items()):
 day=t//86400000*86400000
 if first<=day<last:hd.setdefault(day,[]).append((t,b))
for day in g.ts:
 if day not in hd:
  i=g.ti[day];hd[day]=[(day,[(j,dict(o=g.a['o'][i,j],h=g.H[i,j],l=g.L[i,j],c=g.P[i,j])) for j in range(g.k) if np.isfinite(g.P[i,j])])]
window=dict(first=first,last_exclusive=last,days=(last-first)//86400000,full_hourly_symbols=sum(all(t in times for t in range(first,last,3600000)) for times in coverage.values()),symbols_with_any_hourly=sum(bool(v) for v in coverage.values()),outside='daily fallback; only window metrics interpreted',caveat='hourly OHLC remains ambiguous within hour; no ticks/orderbook/fills')
(S/'hour_window.json').write_text(json.dumps(window,indent=2));(S/'hour_manifest.json').write_text(json.dumps(manifest,indent=2))
for act in [.1,.2,.3]:
 for dist in [.05,.1,.15]:
  for mode in ['coin','portfolio']:
   for path in ['pess','opt']:
    key=f'hour_{act}_{dist}_{mode}_{path}'
    if not (S/(key+'.npz')).exists():g.persist(key,g.simulate(act,dist,mode,path,hourly=hd))
    print(key,flush=True)
