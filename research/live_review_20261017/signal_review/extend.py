import json
import numpy as np
import pandas as pd
import run as r
b=r.b;ROOT=r.ROOT
MODES=['absolute14_veto','short_trend3_veto','no_post_cancellation_reinflation']
R3=b.C/np.roll(b.C,3,axis=0)-1

def weight(i,eq,gross,mode):
 c,_,_=r.components(i,eq,gross)
 if not c:return np.zeros(b.K)
 w=(c['mom']+c['vol'])/2
 if mode=='no_post_cancellation_reinflation':return w
 g=abs(w).sum()
 if g:w/=g
 signal=b.M[i] if mode=='absolute14_veto' else R3[i]
 w[(w*signal<=0)|~np.isfinite(signal)]=0
 long=w[w>0].sum();short=-w[w<0].sum();leg=min(long,short)
 if long:w[w>0]*=leg/long
 if short:w[w<0]*=leg/short
 return w

x=json.loads((ROOT/'results.json').read_text());series=dict(np.load(ROOT/'series.npz'));windows=json.loads((ROOT/'spec.json').read_text())['windows']
for window,(lo,hi) in windows.items():
 start=int(np.searchsorted(b.ts,pd.Timestamp(lo,tz='UTC').value//1000000));end=int(np.searchsorted(b.ts,pd.Timestamp(hi,tz='UTC').value//1000000))
 for mode in MODES:
  b.target=lambda i,e,g,unused:weight(i,e,g,mode)
  a=b.simulate(start_idx=start,end_idx=end);z=b.metrics(a);z['missing']=a['missing_coins'];z['eligible_price_coverage']=not a['missing_coins'];z['annual_cost']=float(a['turn'].mean()*.00075*365)
  stress=b.simulate(start_idx=start,end_idx=end,fee=.00125);z['stress']=b.metrics(stress);z['stress_missing']=stress['missing_coins'];series[window+'__'+mode]=a['r'];x['windows'][window][mode]=z
  print(window,mode,round(z['sharpe'],3),round(z['total']*100,2),'DD',round(z['mdd']*100,2),'gross',round(z['avg_gross'],2),z['missing'],flush=True)
# Retain separate reset windows; no invented full-history equity curve.
keys=r.MODES[1:]+MODES;vals=['validation_2025','validation_2026'];D=np.column_stack([np.concatenate([series[w+'__'+k]-series[w+'__baseline'] for w in vals]) for k in keys]);obs=D.mean(0);null=D-obs;rng=np.random.default_rng(20261018);sizes=[len(series[w+'__baseline']) for w in vals];boot={}
for bl in [15,30,60]:
 samples=[]
 for rep in range(1999):
  inds=[];offset=0
  for n in sizes:
   starts=rng.integers(0,n,size=int(np.ceil(n/bl)));inds.extend((((starts[:,None]+np.arange(bl))%n).ravel()[:n]+offset).tolist());offset+=n
  samples.append(null[inds].mean(0))
 samples=np.array(samples);se=samples.std(0,ddof=1);mx=np.max(samples/np.maximum(se,1e-15),axis=1)
 boot[str(bl)]={k:{'mean_difference':float(obs[j]),'p_fwer':float((1+(mx>=obs[j]/max(se[j],1e-15)).sum())/2000)} for j,k in enumerate(keys)}
x['all_candidates_validation_maxT']=boot
(ROOT/'extended_results.json').write_text(json.dumps(x,indent=2));np.savez_compressed(ROOT/'extended_series.npz',**series)
print('DONE',flush=True)
