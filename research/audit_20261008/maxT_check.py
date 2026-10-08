from pathlib import Path
import numpy as np,pandas as pd,json
root=Path(__file__).resolve().parents[2];out=Path(__file__).resolve().parent
paths={};baseline=None
for d in ['distribution_stagger_20261008','risk_20261008','leadlag_20261008']:
 a=np.load(root/'research'/d/('paths.npz' if d.startswith('risk') else 'returns.npz'))
 if baseline is None:baseline=a['baseline']
 assert np.max(np.abs(a['baseline']-baseline))<1e-12
 if d.startswith('leadlag'):
  for z,x in zip(a['names'],a['candidates']):paths[d+'/'+str(z)]=x
 else:
  for z in a.files:
   if z in ['dates','baseline'] or z.startswith('matched_') or z.startswith('low_') or z=='high_kurt':continue
   paths[d+'/'+z]=a[z]
for d,f in [('marginal_20261008','daily_returns.csv'),('liq_oi_20261008','daily_returns.csv')]:
 a=pd.read_csv(root/'research'/d/f)
 for z in sorted({c.split('_phase')[0] for c in a.columns if c.startswith('blend_')}):paths[d+'/'+z]=a[[z+f'_phase{i}' for i in range(3)]].values.T

a=np.load(root/'research/onchain_20261008/returns.npz')
assert np.max(np.abs(a['baseline']-baseline))<1e-12
for z in a.files:
 if z not in ['dates','baseline','market'] and not z.startswith('matched_'):paths['onchain_20261008/'+z]=a[z]

def s(x):return (x.mean(-1)/x.std(-1,ddof=1)*np.sqrt(365)).mean()
def influence(x):
 mu=x.mean(1)[:,None];sd=x.std(1,ddof=1)[:,None];return (np.sqrt(365)*((x-mu)/sd-mu*((x-mu)**2-sd**2)/(2*sd**3))).mean(0)
keys=list(paths);obs=np.array([s(paths[z])-s(baseline) for z in keys]);D=np.array([influence(paths[z])-influence(baseline) for z in keys]).T;D-=D.mean(0);N=len(D)
rng=np.random.default_rng(20261008);B=4999;res={}
for L in [15,30,60]:
 vals=[]
 for b in range(0,B,100):
  starts=rng.integers(N,size=(min(100,B-b),int(np.ceil(N/L))));idx=((starts[:,:,None]+np.arange(L))%N).reshape(len(starts),-1)[:,:N]
  counts=np.array([np.bincount(x,minlength=N) for x in idx]);vals.append(counts@D/N)
 vals=np.concatenate(vals);se=np.maximum(vals.std(0,ddof=1),1e-12);maxnull=(vals/se).max(1)
 res[str(L)]={z:{'delta':float(obs[j]),'p_fwer':float((1+(maxnull>=obs[j]/se[j]).sum())/(B+1))} for j,z in enumerate(keys)}
json.dump({'comparisons':len(keys),'B':B,'method':'Independent joint daily Sharpe influence-function maxT, conditional on supplied paths; unequal phase counts retained; no hourly or pair formation inference; exploratory counterfactuals are not deployment candidates','results':res},open(out/'maxT_check.json','w'),indent=2)
print('daily comparisons',len(keys),'minimum p',min(q['p_fwer'] for v in res.values() for q in v.values()))
