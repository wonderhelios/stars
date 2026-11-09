import json
import numpy as np
import test_lifecycle as m
R=m.ROOT;x=json.loads((R/'results.json').read_text());series=dict(np.load(R/'series.npz'))
variants=['long_tp8_cool24','short_stop5_cool24','long_tp8_short_stop5','no_loser_add'];windows={'first_half':(0,(m.N//48)*24),'second_half':((m.N//48)*24,m.N),'recent':(int((1791417600000-m.start)//m.HOUR),m.N)}
for window,(lo,hi) in windows.items():
 for mode in variants:
  a=m.sim(mode,lo=lo,hi=hi);z=m.b.metrics(a);z.update({k:a[k] for k in ['missing','risk_exits','closed_episode_median_hours','open_positions']});z['cost_stress']=m.b.metrics(m.sim(mode,fee=.00125,lo=lo,hi=hi));x[window][mode]=z;series[window+'__'+mode]=a['r']
  print(window,mode,round(z['sharpe'],3),round(z['total']*100,2),round(z['mdd']*100,2),flush=True)
(R/'all_results.json').write_text(json.dumps(x,indent=2));np.savez_compressed(R/'all_series.npz',**series)
keys=[k for k in x['first_half'] if k!='baseline'];vals=['first_half','second_half'];mat=np.concatenate([np.column_stack([series[w+'__baseline']]+[series[w+'__'+k] for k in keys]) for w in vals]);sizes=[len(series[w+'__baseline']) for w in vals]
def sh(a):return a.mean(0)/a.std(0,ddof=1)*np.sqrt(365)
obs=sh(mat)[1:]-sh(mat)[0];rng=np.random.default_rng(20261021);boot={}
for bl in [7,15,30]:
 samples=[]
 for rep in range(1999):
  inds=[];offset=0
  for n in sizes:
   st=rng.integers(0,n,int(np.ceil(n/bl)));inds.extend((((st[:,None]+np.arange(bl))%n).ravel()[:n]+offset).tolist());offset+=n
  v=sh(mat[inds]);samples.append(v[1:]-v[0])
 samples=np.array(samples);se=samples.std(0,ddof=1);mx=((samples-obs)/se).max(1)
 boot[str(bl)]={k:{'delta_sharpe':float(obs[j]),'ci95':np.percentile(samples[:,j],[2.5,97.5]).tolist(),'p_fwer':float((1+(mx>=obs[j]/se[j]).sum())/2000)} for j,k in enumerate(keys)}
(R/'multiple_testing.json').write_text(json.dumps(boot,indent=2));print('DONE',flush=True)
