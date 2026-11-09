import os,sys,json
from pathlib import Path
import numpy as np
import pandas as pd
os.environ['INCLUDE_RECENT']='1'
ROOT=Path(__file__).resolve().parent
sys.path.insert(0,str(ROOT.parent))
import backtest as b
MODES=['baseline','momentum_only','volume_only','momentum75_volume25','momentum_plus_signed_volume','momentum_plus_reverse_volume','momentum_plus_direction_confirmed_volume','raw_momentum_plus_volume']
original=b.target

def components(i,eq,gross):
 cap=min(10,max(1,int(np.floor(eq*gross/15))//4))
 idx=np.flatnonzero((b.V[i]>=1e7)&np.isfinite(b.C[i])&np.isfinite(b.C[i-14])&np.isfinite(b.SD[i])&b.NOTCOL)
 if len(idx)<8:return {},idx,0
 kk=min(max(1,int(np.floor(len(idx)*.2+.5))),cap,len(idx)//2)
 def book(score):
  w=np.zeros(b.K);order=idx[np.lexsort((b.names[idx],score[idx]))];w[order[:kk]]=-.5/kk;w[order[-kk:]]=.5/kk;return w
 mom=b.M[i]/np.maximum(b.SD[i],1e-9);vol=b.Q[i]/b.V[i]
 confirmed=np.zeros(b.K)
 long=idx[b.M[i,idx]>0];short=idx[b.M[i,idx]<0]
 if len(long)>=kk and len(short)>=kk:
  for pool,sgn in [(long,1),(short,-1)]:
   order=pool[np.lexsort((b.names[pool],vol[pool]))];confirmed[order[-kk:]]=sgn*.5/kk
 return {'mom':book(mom),'vol':book(vol),'signed':book(np.sign(b.R[i])*vol),'rawmom':book(b.M[i]),'confirmed':confirmed},idx,kk

def weight(i,eq,gross,mode):
 c,_,_=components(i,eq,gross)
 if not c:return np.zeros(b.K)
 m=c['mom'];v=c['vol']
 w={'baseline':(m+v)/2,'momentum_only':m,'volume_only':v,'momentum75_volume25':.75*m+.25*v,'momentum_plus_signed_volume':(m+c['signed'])/2,'momentum_plus_reverse_volume':(m-v)/2,'momentum_plus_direction_confirmed_volume':(m+c['confirmed'])/2,'raw_momentum_plus_volume':(c['rawmom']+v)/2}[mode]
 g=np.abs(w).sum();return w/g if g else w

def main():
 # Match the original engine for current and historical configurations.
 for i in range(33,b.N,17):
  assert np.allclose(original(i,5739.2678625,2.7,'baseline'),weight(i,5739.2678625,2.7,'baseline'),atol=1e-12)
 print('BASELINE WEIGHT PARITY PASS',flush=True)
 windows=json.loads((ROOT/'spec.json').read_text())['windows'];all_results={};series={}
 for label,(lo,hi) in windows.items():
  start=int(np.searchsorted(b.ts,pd.Timestamp(lo,tz='UTC').value//1000000));end=int(np.searchsorted(b.ts,pd.Timestamp(hi,tz='UTC').value//1000000));all_results[label]={};series[label]={}
  for mode in MODES:
   b.target=lambda i,e,g,unused:weight(i,e,g,mode)
   try:
    a=b.simulate(start_idx=start,end_idx=end);z=b.metrics(a);z['missing']=a['missing_coins'];z['eligible_price_coverage']=not a['missing_coins'];z['annual_cost']=float(a['turn'].mean()*.00075*365)
    stress=b.simulate(start_idx=start,end_idx=end,fee=.00125);z['stress']=b.metrics(stress);z['stress_missing']=stress['missing_coins'];series[label][mode]=a['r'];all_results[label][mode]=z
    print(label,mode,round(z['sharpe'],3),round(z['total']*100,2),z['missing'],flush=True)
   except RuntimeError as e:all_results[label][mode]={'error':str(e)}
 # Latest factor membership and one-day return for real positions.
 i=b.N-1;c,idx,kk=components(i,5739.2678625,2.7)
 actual=json.loads((ROOT.parent/'evidence/live_latest.json').read_text())
 latest=[]
 for pos in actual['positions']:
  j=int(np.flatnonzero(b.names==pos['coin'])[0]);latest.append({'coin':pos['coin'],'side':pos['side'],'momentum14':float(b.M[i,j]),'return1d':float(b.R[i,j]),'volume_shock':float(b.Q[i,j]/b.V[i,j]),'momentum_book':float(c['mom'][j]),'volume_book':float(c['vol'][j]),'unrealized':pos['unrealized']})
 # Family-wise centered block bootstrap of mean-return differences on pooled validation windows.
 # Affected missing-price candidates are flagged, never promoted based on p-values.
 vals=['validation_2025','validation_2026'];base=np.concatenate([series[w]['baseline'] for w in vals]);keys=MODES[1:]
 D=np.column_stack([np.concatenate([series[w][k]-series[w]['baseline'] for w in vals]) for k in keys]);obs=D.mean(0);null=D-obs;rng=np.random.default_rng(20261017);boot={}
 # Sample blocks within each validation window, not across artificial reset boundaries.
 sizes=[len(series[w]['baseline']) for w in vals]
 for bl in [15,30,60]:
  samples=[]
  for rep in range(1999):
   inds=[];offset=0
   for n in sizes:
    starts=rng.integers(0,n,size=int(np.ceil(n/bl)));inds.extend((((starts[:,None]+np.arange(bl))%n).ravel()[:n]+offset).tolist());offset+=n
   samples.append(null[inds].mean(0))
  samples=np.array(samples);se=samples.std(0,ddof=1);mx=np.max(samples/np.maximum(se,1e-15),axis=1)
  boot[str(bl)]={k:{'mean_difference':float(obs[j]),'p_fwer':float((1+(mx>=obs[j]/max(se[j],1e-15)).sum())/2000)} for j,k in enumerate(keys)}
 (ROOT/'results.json').write_text(json.dumps({'windows':all_results,'latest_factor_membership':latest,'validation_mean_return_maxT':boot},indent=2))
 np.savez_compressed(ROOT/'series.npz',**{w+'__'+m:r for w,a in series.items() for m,r in a.items()})
 print('DONE',flush=True)

if __name__ == "__main__":
 main()
