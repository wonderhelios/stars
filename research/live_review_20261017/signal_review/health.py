import json
import numpy as np
import pandas as pd
import run as r
b=r.b;ROOT=r.ROOT
x=json.loads((ROOT/'extended_results.json').read_text());series=dict(np.load(ROOT/'extended_series.npz'));windows=json.loads((ROOT/'spec.json').read_text())['windows']
MODES=['volume_health90','both_factor_health90'];diagnostics={}
for window,(lo,hi) in windows.items():
 start=int(np.searchsorted(b.ts,pd.Timestamp(lo,tz='UTC').value//1000000));end=int(np.searchsorted(b.ts,pd.Timestamp(hi,tz='UTC').value//1000000))
 mom=series[window+'__momentum_only'];vol=series[window+'__volume_only']
 for mode in MODES:
  status=[]
  def weight(i,eq,gross,unused):
   done=i+1-start
   if done<90:return r.weight(i,eq,gross,'baseline')
   c,_,_=r.components(i,eq,gross)
   if not c:return np.zeros(b.K)
   # All source returns end on the signal's closing date; never use trade-day return.
   mh=np.mean(mom[done-90:done])>0;vh=np.mean(vol[done-90:done])>0
   active_m=mh if mode=='both_factor_health90' else True
   w=(c['mom'] if active_m else np.zeros(b.K))+(c['vol'] if vh else 0)
   status.append((i,bool(active_m),bool(vh)))
   g=np.abs(w).sum();return w/g if g else w
  b.target=weight;a=b.simulate(start_idx=start,end_idx=end);z=b.metrics(a);z['missing']=a['missing_coins'];z['eligible_price_coverage']=not a['missing_coins'];z['annual_cost']=float(a['turn'].mean()*.00075*365);diagnostics[window+'__'+mode]=status.copy()
  # Cost stress changes the sidecar health decisions too; otherwise the gate would see optimistic costs.
  oldmom=mom.copy();oldvol=vol.copy()
  for name in ['momentum_only','volume_only']:
   b.target=lambda i,eq,g,unused:r.weight(i,eq,g,name)
   rr=b.simulate(start_idx=start,end_idx=end,fee=.00125)['r']
   if name=='momentum_only':mom=rr
   else:vol=rr
  b.target=weight;stress=b.simulate(start_idx=start,end_idx=end,fee=.00125);mom=oldmom;vol=oldvol
  z['stress']=b.metrics(stress);z['stress_missing']=stress['missing_coins'];series[window+'__'+mode]=a['r'];x['windows'][window][mode]=z
  print(window,mode,'S',round(z['sharpe'],3),'total',round(z['total']*100,2),'DD',round(z['mdd']*100,2),z['missing'],flush=True)
keys=[k for k in x['windows']['train_2023_24'] if k!='baseline'];vals=['validation_2025','validation_2026'];D=np.column_stack([np.concatenate([series[w+'__'+k]-series[w+'__baseline'] for w in vals]) for k in keys]);obs=D.mean(0);null=D-obs;rng=np.random.default_rng(20261019);sizes=[len(series[w+'__baseline']) for w in vals];boot={}
for bl in [15,30,60]:
 samples=[]
 for rep in range(1999):
  inds=[];offset=0
  for n in sizes:
   starts=rng.integers(0,n,size=int(np.ceil(n/bl)));inds.extend((((starts[:,None]+np.arange(bl))%n).ravel()[:n]+offset).tolist());offset+=n
  samples.append(null[inds].mean(0))
 samples=np.array(samples);se=samples.std(0,ddof=1);mx=np.max(samples/np.maximum(se,1e-15),axis=1)
 boot[str(bl)]={k:{'mean_difference':float(obs[j]),'p_fwer':float((1+(mx>=obs[j]/max(se[j],1e-15)).sum())/2000)} for j,k in enumerate(keys)}
x['all_12_candidates_validation_maxT']=boot
(ROOT/'final_results.json').write_text(json.dumps(x,indent=2));np.savez_compressed(ROOT/'final_series.npz',**series);(ROOT/'health_decisions.json').write_text(json.dumps(diagnostics))
print('DONE',flush=True)
