import json,itertools,math
from pathlib import Path
import numpy as np,pandas as pd
import study as s
O=s.OUT
D=pd.read_csv(O/'diagnostics.csv');D['period']=np.where(D.date<'2024-03-05','pre',np.where(D.date<='2024-10-15','drawdown','post'));N=len(D)
b0=dict(np.load(O/'baseline_p0_f0.0007.npz'));D['portfolio_vol20']=pd.Series(b0['r']).rolling(20,min_periods=20).std().shift(1)
D['xs_vol_eligible_signal']=D.xs_vol
D['xs_vol']=np.nanstd(s.R[33:],axis=1,ddof=1)
D['xs_vol_all_signal']=np.nanstd(s.R[32:-1],axis=1,ddof=1)
# exact Shapley allocations of ideal L1 target changes among three sleeves, including net normalization
factor=[];normal=[]
for i in range(32,s.n-1):
 old=s.sleeves[i-1];new=s.sleeves[i];go=s.norm[i-1] or 1;gn=s.norm[i] or 1;dx=new/gn-old/go
 def val(sub):return np.abs(dx[list(sub)].sum(0)).sum()*2.7 if sub else 0.
 alloc=[]
 for j in range(3):
  others=[x for x in range(3) if x!=j];v=0
  for size in range(3):
   for sub in itertools.combinations(others,size):v+=math.factorial(size)*math.factorial(2-size)/6*(val(sub+(j,))-val(sub))
  alloc.append(v)
 factor.append(alloc)
 u0=old.sum(0);u1=new.sum(0);a=(u1-u0)*.5*(1/go+1/gn);b=(1/gn-1/go)*.5*(u1+u0);total=abs(a+b).sum()*2.7;na=(abs(a).sum()+abs(a+b).sum()-abs(b).sum())*1.35;normal.append([na,total-na])
D[['factor_momentum','factor_lowvol','factor_volume']]=factor;D[['raw_change_shapley','normalization_shapley']]=normal
assert np.allclose(D[['entry','exit','target_flip','same_weight_change','target_unchanged']].sum(axis=1),D.turn)
assert np.allclose(D[['actual_entry','actual_exit','actual_flip','actual_same']].sum(axis=1),D.turn)
assert np.allclose(D[['factor_momentum','factor_lowvol','factor_volume']].sum(axis=1),D.ideal_signal_turn)
assert np.allclose(D[['signal_shapley','drift_shapley']].sum(axis=1),D.turn)
D.to_csv(O/'daily_analysis.csv',index=False)
cols=['turn','xs_vol','market_vol20','portfolio_vol20','rank_distance','universe']
for label,mask in [('full',np.ones(N,bool)),('drawdown',D.period.eq('drawdown')),('active',D.kk.gt(0))]:
 for method in ['pearson','spearman']:D.loc[mask,cols].corr(method=method).to_csv(O/f'corr_{label}_{method}.csv')
summary={}
for label,mask in [('pre',D.period.eq('pre')),('drawdown',D.period.eq('drawdown')),('post',D.period.eq('post')),('pre_active',D.period.eq('pre')&D.kk.gt(0))]:
 x=D.loc[mask];summary[label]={'days':len(x),'active_days':int(x.kk.gt(0).sum()),'universe_min':int(x.universe.min()),'universe_max':int(x.universe.max()),**x.mean(numeric_only=True).to_dict(),'skip_min_count':int(x.skip_min_n.sum()),'skip_min_usd':float((x.skip_min_turn*x.equity).sum())}
(O/'mechanism_summary.json').write_text(json.dumps(summary,indent=2))
common=b0['t'][2:]; dates=pd.to_datetime(common,unit='ms',utc=True);dd=(dates>=pd.Timestamp('2024-03-05',tz='UTC'))&(dates<=pd.Timestamp('2024-10-15',tz='UTC'))
def get(c,fee=.0007):
 datas=[dict(np.load(O/f"{c}_p{p}_f{fee}.npz")) for p in range(3)];return np.array([d['r'][np.isin(d['t'],common)] for d in datas]),np.array([d['turn'][np.isin(d['t'],common)] for d in datas])
def metrics(r,t,fee):
 return dict(sharpe=float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365)),turn=float(t.mean()),cost=float(t.mean()*365*fee),annual=float(r.mean()*365),cagr=float(np.expm1(np.log1p(r).mean()*365)),ddturn=float(t[dd].mean()),s1=s.g['sh'](r[dates.year<=2024]),s2=s.g['sh'](r[dates.year>=2025]))
results=[];ravg=[]
for c in s.configs:
 name=c['name'];r,t=get(name);base,bt=get('baseline');ms=[metrics(r[p],t[p],.0007) for p in range(3)];bs=[metrics(base[p],bt[p],.0007) for p in range(3)];rec={'name':name,**{k:float(np.mean([m[k] for m in ms])) for k in ms[0]}};delta=np.array([m['sharpe']-b['sharpe'] for m,b in zip(ms,bs)])
 rec.update(delta=float(delta.mean()),phase_sd=float(np.std([m['sharpe'] for m in ms])),phase_range=float(np.ptp(delta)),phase_sharpes=[m['sharpe'] for m in ms],d1=rec['s1']-np.mean([b['s1'] for b in bs]),d2=rec['s2']-np.mean([b['s2'] for b in bs]),corr=float(np.corrcoef(r.mean(0),base.mean(0))[0,1]),dd_reduction=float(1-rec['ddturn']/np.mean([b['ddturn'] for b in bs])))
 rec['cost_cases']={}
 for fee in [.00015,.00045,.00075]:
  rr,tt=get(name,fee);br,btt=get('baseline',fee);mm=[metrics(rr[p],tt[p],fee) for p in range(3)];bm=[metrics(br[p],btt[p],fee) for p in range(3)];m={k:float(np.mean([x[k] for x in mm])) for k in mm[0]};m.update(d1=m['s1']-np.mean([x['s1'] for x in bm]),d2=m['s2']-np.mean([x['s2'] for x in bm]));rec['cost_cases'][str(fee)]=m
 results.append(rec);ravg.append(r)
# Paired Sharpe influence-function moving-block maxT (studentization via block resampling SD)
ravg=np.array(ravg);mu=ravg.mean(2);sd=ravg.std(2);influence=(np.sqrt(365)*((ravg-mu[:,:,None])/sd[:,:,None]-mu[:,:,None]/(2*sd[:,:,None]**3)*((ravg-mu[:,:,None])**2-sd[:,:,None]**2))).mean(1);diff=influence[1:]-influence[0];diff-=diff.mean(1)[:,None];observed=np.array([x['delta'] for x in results[1:]])
B=4999;stats={};T=len(common)
for L in [10,20,40]:
 rng=np.random.default_rng(20261009+L);boot=np.zeros((B,len(diff)))
 for start in range(0,B,100):
  nb=min(100,B-start);st=rng.integers(0,T,size=(nb,int(np.ceil(T/L))));ix=((st[:,:,None]+np.arange(L))%T).reshape(nb,-1)[:,:T];boot[start:start+nb]=diff[:,ix].mean(2).T
 se=boot.std(0,ddof=1);z=observed/se;null=boot/se;maxnull=null.max(1);p=(1+(null>=z).sum(0))/(B+1);pf=(1+(maxnull[:,None]>=z).sum(0))/(B+1)
 stats[str(L)]={'se':se.tolist(),'p':p.tolist(),'p_fwer':pf.tolist()};np.savez_compressed(O/f'bootstrap_L{L}.npz',null=null,maxnull=maxnull,observed=z)
 for j,rec in enumerate(results[1:]):rec[f'p_L{L}']=float(p[j]);rec[f'pfwer_L{L}']=float(pf[j])
 print('bootstrap',L,'checkpoint',flush=True)
(O/'bootstrap.json').write_text(json.dumps(stats,indent=2))
for rec in results:
 rec['low_cost']=rec['name']!='baseline' and rec['delta']>=-.10 and rec['dd_reduction']>=1/3
 rec['usable']=rec['name']!='baseline' and max(rec.get(f'pfwer_L{x}',1) for x in [10,20,40])<.05 and rec['s1']>0 and rec['s2']>0 and rec['d1']>0 and rec['d2']>0 and rec['delta']>rec['phase_range'] and rec['cost_cases']['0.00075']['d1']>0 and rec['cost_cases']['0.00075']['d2']>0
(O/'results.json').write_text(json.dumps(results,indent=2));pd.DataFrame([{k:v for k,v in x.items() if not isinstance(v,(dict,list))} for x in results]).to_csv(O/'results.csv',index=False)
print(pd.read_csv(O/'results.csv').to_string(index=False))
