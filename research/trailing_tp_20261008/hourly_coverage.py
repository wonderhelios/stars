"""Coverage audit only. Does not bypass failed baseline by testing trailing rules."""
from pathlib import Path
import json,numpy as np,datetime
p=Path(__file__).resolve().parent;ns={'__file__':str(p/'baseline.py')};s=(p/'baseline.py').read_text();exec(s[:s.index('out=dict(config=')],ns)
req=ns['req'];names=ns['names'];ts=ns['ts'];end=req['end_ms'];hr={}
for c in names:
 d=json.load(open('/tmp/hl-hist/'+c+'.json'));hr[c]={x['t'] for x in d if x['T']<end and float(x['c'])>0}
W=np.array([ns['target'](i,1e6) for i in range(ns['n'])]);rows=[]
for phase in range(24):
 eligible=0;good=0;bad=[]
 for i,t in enumerate(ts):
  start=t+phase*3600000;finish=start+86400000
  if i<33 or start<req['hourly_start_ms'] or finish+3600000>end:continue
  wanted=np.flatnonzero(abs(W[i-1])+abs(W[i-2])>1e-12)
  if len(wanted)==0:continue
  eligible+=1;check=range(start,finish+1,3600000);miss=[names[j] for j in wanted if not all(h in hr[names[j]] for h in check)]
  if miss:bad.append(dict(day=t,missing=miss))
  else:good+=1
 rows.append(dict(rebalance_hour_utc=phase,eligible_days=eligible,fully_synchronous_days=good,failed_days=bad))
out=dict(status='coverage audit only; trailing execution count zero',phases=rows,mean_full_days=float(np.mean([r['fully_synchronous_days'] for r in rows])),min_full_days=min(r['fully_synchronous_days'] for r in rows),max_full_days=max(r['fully_synchronous_days'] for r in rows),definition='for each scheduled day and phase, all 25 completed hourly candles exist for union of that day and prior day source target names; sufficient price availability only, not proof of actual fills or continuous intrabar order')
(p/'hourly_coverage.json').write_text(json.dumps(out,indent=2));print({k:v for k,v in out.items() if k!='phases'})
