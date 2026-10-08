"""Independent scalar implementation of trader.rs weights, checked against vector panel."""
from pathlib import Path
import json,numpy as np
import baseline_engine as e
P=Path(__file__).parent
checks=[]
for i in [50,150,450,750,900,1000,e.n-3]:
 names=[];scores=[]
 for j,c in enumerate(e.names):
  cm={int(x['t']):float(x['c']) for x in e.raw[c] if float(x['c'])>0};vm={int(x['t']):float(x['v'])*float(x['c']) for x in e.raw[c] if float(x['c'])>0}
  vs=[vm[e.times[k]] for k in range(i-30,i) if e.times[k] in vm]
  if len(vs)<5 or np.mean(vs)<5e6 or e.times[i] not in cm or e.times[i-14] not in cm:continue
  rr=[cm[e.times[k]]/cm[e.times[k-1]]-1 for k in range(i+1-20,i+1) if e.times[k] in cm and e.times[k-1] in cm]
  if len(rr)<5:continue
  sd=max(float(np.std(rr,ddof=1)),1e-9);names.append(c);scores.append([(cm[e.times[i]]/cm[e.times[i-14]]-1)/sd,-sd,vm.get(e.times[i],0)/np.mean(vs)])
 acc={c:0. for c in names};k=min(max(1,int(np.floor(len(names)*.2+.5))),5,len(names)//2)
 if len(names)>=8:
  for factor in range(3):
   order=sorted(range(len(names)),key=lambda j:(scores[j][factor],names[j]))
   for pos,j in enumerate(order):
    if pos>=len(names)-k:acc[names[j]]+=.5/k/3
    elif pos<k:acc[names[j]]-=.5/k/3
 norm=sum(abs(x) for x in acc.values());scalar=np.array([acc.get(c,0)/norm if norm else 0 for c in e.names])
 ok=(e.V[i]>=5e6)&np.isfinite(e.P[i])&np.isfinite(e.P[i-14])&np.isfinite(e.SD[i]);idx=np.flatnonzero(ok)
 vec=sum(e.book(idx,s,5) for s in [e.M[i]/np.maximum(e.SD[i],1e-9),-e.SD[i],e.shock[i]])/3
 if np.abs(vec).sum()>1e-12:vec/=np.abs(vec).sum()
 error=float(np.max(np.abs(scalar-vec)));assert error<1e-9
 checks.append({'date':str(e.dates[i]),'max_weight_error':error,'liquid_universe':len(names)})
json.dump(checks,open(P/'baseline_parity.json','w'),indent=2);print(json.dumps(checks,indent=2))
