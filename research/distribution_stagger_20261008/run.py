from engine import *
import math
mask={'full':np.ones(len(ix),bool),'2023-24':outdates.year<=2024,'2025-26':outdates.year>=2025}
base,books,univ=targets();Ts={'baseline':base}
for z,t in books.items():
 Ts[z]=t;mix=.8*base+.2*t;g=np.abs(mix).sum(axis=1);Ts['blend_'+z]=np.divide(mix,g[:,None],out=np.zeros_like(mix),where=g[:,None]>0)
schedules={}
contrib=pd.DataFrame(np.abs(base-np.roll(base,1,axis=0))).rolling(60,min_periods=20).mean().shift().values
for metric,label in [(SD,'vol'),(contrib,'turn')]:
 for cycles in [(2,3,4),(1,3,6)]:
  per=np.full((n,k),3,dtype=int)
  for i in range(32,n):
   idx=np.flatnonzero((V[i]>=5e6)&np.isfinite(metric[i])&np.isfinite(P[i]))
   order=idx[np.lexsort((np.array(names)[idx],metric[i,idx]))]
   for ids,c in zip(np.array_split(order,3),cycles[::-1]):per[i,ids]=c
   if i>32:
    missing=~((V[i]>=5e6)&np.isfinite(P[i]));per[i,missing]=per[i-1,missing]
  schedules[f'{label}_{cycles[0]}_{cycles[1]}_{cycles[2]}']=per
runs={z:[run(t,o) for o in range(3)] for z,t in Ts.items()}
for p in [2,4,5,6]:runs[f'uniform_{p}']=[run(base,o,period=p) for o in range(p)]
for z,p in schedules.items():runs[z]=[run(base,o,period=p) for o in range(math.lcm(*np.unique(p).tolist()))]
bavg=np.mean([v['r'] for v in runs['baseline']],axis=0)
def summarize(rr,m,fee=.00075):
 ss=[stat(v['r'][m]) for v in rr];q={key:float(np.mean([s[key] for s in ss])) for key in ss[0]};sh=[s['sharpe'] for s in ss]
 q.update(phase_sharpes=sh,phase_sd=float(np.std(sh)),phase_range=float(np.ptp(sh)),daily_turnover_equity=float(np.mean([v['turn'][m].mean() for v in rr])),correlation_baseline=float(np.corrcoef(np.mean([v['r'][m] for v in rr],axis=0),bavg[m])[0,1]),mean_gross=float(np.mean([v['gross'][m].mean() for v in rr])),mean_abs_net=float(np.mean([np.abs(v['net'][m]).mean() for v in rr])))
 if len(rr)==3:q['correlation_same_phase']=float(np.mean([np.corrcoef(v['r'][m],runs['baseline'][o]['r'][m])[0,1] for o,v in enumerate(rr)]))
 q['daily_turnover_unit_gross']=q['daily_turnover_equity']/2.7;q['daily_half_turnover_unit_gross']=q['daily_turnover_equity']/5.4;q['annual_cost_unit_gross']=q['daily_turnover_equity']/2.7*fee*365;q['annual_cost']=q['daily_turnover_equity']*fee*365;return q
summary={z:{p:summarize(rr,m) for p,m in mask.items()} for z,rr in runs.items()}
upper={}
for label,to in [('5pct_half_gross',.27),('5pct_unit_gross',.135),('5pct_equity',.05),('zero_cost',0)]:
 rr=[]
 for v in runs['baseline']:
  gross=(1+v['r'])/(1-v['turn']*.00075)-1;r=(1+gross)*(1-to*.00075)-1
  rr.append(dict(v,r=r,turn=np.full(len(ix),to)))
 upper[label]={p:summarize(rr,m) for p,m in mask.items()}
def sif(r):
 mu=r.mean();sd=r.std(ddof=1);return np.sqrt(365)*((r-mu)/sd-mu*((r-mu)**2-sd**2)/(2*sd**3))
def maxT(allruns,baseline,m,B=9999):
 zs=[z for z in allruns if z!=baseline];b=allruns[baseline];baseS=np.mean([stat(v['r'][m])['sharpe'] for v in b]);bi=np.mean([sif(v['r'][m]) for v in b],axis=0)
 D=np.array([np.mean([sif(v['r'][m]) for v in allruns[z]],axis=0)-bi for z in zs]).T;N=len(D)
 obs=np.array([np.mean([stat(v['r'][m])['sharpe'] for v in allruns[z]])-baseS for z in zs]);res={};rng=np.random.default_rng(20261008)
 for L in [15,30,60]:
  vals=[]
  for start in range(0,B,250):
   batch=min(250,B-start);starts=rng.integers(N,size=(batch,math.ceil(N/L)));inds=((starts[:,:,None]+np.arange(L))%N).reshape(batch,-1)[:,:N];counts=np.zeros((batch,N))
   for j in range(batch):counts[j]=np.bincount(inds[j],minlength=N)
   vals.append(counts@(D-D.mean(axis=0))/N)
  vals=np.concatenate(vals);se=np.maximum(vals.std(axis=0,ddof=1),1e-12);mx=(vals/se).max(axis=1)
  res[str(L)]={z:dict(delta_sharpe=float(obs[j]),se=float(se[j]),ci95=(obs[j]+np.quantile(vals[:,j],[.025,.975])).tolist(),p_fwer_within=float((1+(mx>=obs[j]/se[j]).sum())/(B+1)),p_fwer_global=float(min(1,2*(1+(mx>=obs[j]/se[j]).sum())/(B+1)))) for j,z in enumerate(zs)}
 return res
print('DAILY RUNS COMPLETE',flush=True)
bootstrap={p:maxT(runs,'baseline',m) for p,m in mask.items()}
stress={}
for z in runs:
 T=Ts.get(z,base);p=schedules.get(z,int(z.split('_')[1]) if z.startswith('uniform_') else 3);stress[z]={}
 for label,fee,pen in [('slippage6bp',.00105,0),('missing5pct',.00075,.05),('maker_optimistic',.00045,0)]:
  rr=[run(T,o,period=p,fee=fee,missing_penalty=pen) for o in range(len(runs[z]))];stress[z][label]={part:summarize(rr,m,fee) for part,m in mask.items()}
meta=dict(daily_files=len(FILES),start=str(outdates[0]),end=str(outdates[-1]),days={p:int(m.sum()) for p,m in mask.items()},daily_candidates=len(runs)-1,hourly_candidates=24,total_candidates=len(runs)-1+24,universe_mean=float(np.mean(univ)),universe_median=float(np.median(univ)),baseline_reference=1.79,baseline_reproduced=summary['baseline']['full']['sharpe'],bootstrap='9999 circular paired moving blocks; mean phase Sharpe influence maxT; Bonferroni x2 across daily and hourly families; blocks15/30/60; three periods separately',funding='Excluded: full 2023-26 funding unavailable; no candidate deployable until reconciled',minimum_orders='Large-account approximation, ignores $10 minimum and rounding, applies 2% band',missing={z:dict(max_equity_exposure=float(max(v['missing'].max() for v in rr)),events=[int((v['missing']>0).sum()) for v in rr]) for z,rr in runs.items()},manifest={os.path.basename(f):hashlib.sha256(open(f,'rb').read()).hexdigest() for f in FILES})
for z,T in Ts.items():
 assert np.max(np.abs(T.sum(axis=1)))<1e-10
for rr in runs.values():
 for v in rr:assert np.isfinite(v['r']).all() and (v['r']>-1).all()
assert np.all(np.diff(times)==DAY)
json.dump(dict(meta=meta,summary=summary,upper=upper,bootstrap=bootstrap,stress=stress),open(OUT+'/results.json','w'),indent=2)
np.savez_compressed(OUT+'/returns.npz',dates=np.asarray(outdates.astype(str),dtype='U'),**{z:np.array([v['r'] for v in rr]) for z,rr in runs.items()})
pd.DataFrame([dict(strategy=z,period=p,**v) for z,q in summary.items() for p,v in q.items()]).to_csv(OUT+'/metrics.csv',index=False)
np.savez_compressed(OUT+'/panel.npz',base=base,times=times,names=names,O=a['o'],OP=OP)
for z in summary:print(z,[round(summary[z][p]['sharpe'],3) for p in mask],bootstrap['full']['30'].get(z,{}).get('p_fwer_global'),flush=True)
print('UPPER',[(z,[round(upper[z][p]['sharpe'],3) for p in mask]) for z in upper],flush=True)
