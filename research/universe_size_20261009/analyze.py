"""Paired nonlinear block bootstrap; drawdown paths and complete metric tables."""
import json, sys, hashlib
from pathlib import Path
import numpy as np
import pandas as pd
O=Path('research/universe_size_20261009')
NAMES=['baseline','K3','K5','K8','K12','K20','N30_K5','N50_K8','N50_K12','N80_K15']
FEES=[.0007,.00045,.00015,.00075]
def save(name,obj): (O/name).write_text(json.dumps(obj,indent=2,ensure_ascii=False))
def load(name,p=0,fee=.0007):return dict(np.load(O/f'{name}_p{p}_f{fee}.npz'))
def sharpe(r):return float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365))
def drawdown(r,t):
 dates=pd.to_datetime(t,unit='ms',utc=True);ds=pd.DatetimeIndex([dates[0]-pd.Timedelta(days=1)]+list(dates))
 e=np.r_[1.,np.cumprod(1+r)];hi=np.maximum.accumulate(e);dd=e/hi-1
 valley=int(dd.argmin());peak=int(np.flatnonzero(e[:valley+1]==hi[valley])[-1])
 rec=np.flatnonzero(e[valley:]>=hi[valley]);recovery=int(valley+rec[0]) if len(rec) else None
 end=len(e)-1 if recovery is None else recovery
 # Longest underwater episode, including right-censored current episode.
 lastpeak=0;longest=dict(days=0,peak=None,end=None,recovered=True)
 for i in range(1,len(e)):
  reached=e[i]>=hi[i-1]
  if reached or i==len(e)-1:
   days=(ds[i]-ds[lastpeak]).days if i>lastpeak+1 or not reached else 0
   if days>longest['days']:longest=dict(days=int(days),peak=str(ds[lastpeak].date()),end=str(ds[i].date()),recovered=bool(reached))
  if reached:lastpeak=i
 return dict(max_drawdown=float(dd[valley]),peak=str(ds[peak].date()),trough=str(ds[valley].date()),
             recovery=str(ds[recovery].date()) if recovery is not None else None,recovered=recovery is not None,
             decline_days=int((ds[valley]-ds[peak]).days),recovery_days=int((ds[end]-ds[valley]).days),
             underwater_days=int((ds[end]-ds[peak]).days),longest_underwater=longest)

def metrics(r,tr,t,fee):
 return dict(days=len(r),sharpe=sharpe(r),daily_turnover=float(tr.mean()),annual_cost=float(tr.mean()*fee*365),
             annual_arithmetic=float(r.mean()*365),cagr=float(np.expm1(np.log1p(r).mean()*365)),
             total_return=float(np.prod(1+r)-1),**drawdown(r,t))
def arrays(name,fee,common):
 d=[load(name,p,fee) for p in range(3)]
 return np.array([x['r'][np.isin(x['t'],common)] for x in d]),np.array([x['turn'][np.isin(x['t'],common)] for x in d])
def collect():
 common=load('baseline',2)['t'];dates=pd.to_datetime(common,unit='ms',utc=True)
 br,bt=arrays('baseline',.0007,common);base=[metrics(br[p],bt[p],common,.0007) for p in range(3)]
 records=[];allarrays=[];periods={}
 for name in NAMES:
  r,tr=arrays(name,.0007,common);allarrays.append(r)
  mm=[metrics(r[p],tr[p],common,.0007) for p in range(3)]
  keys=['sharpe','daily_turnover','annual_cost','annual_arithmetic','cagr','total_return','max_drawdown','decline_days','recovery_days','underwater_days']
  rec=dict(name=name,days=len(common),**{k:float(np.mean([m[k] for m in mm])) for k in keys},phase_metrics=mm)
  delta=np.array([m['sharpe']-b['sharpe'] for m,b in zip(mm,base)])
  rec.update(delta=float(delta.mean()),phase_sd=float(np.std([m['sharpe'] for m in mm])),phase_delta_range=float(np.ptp(delta)),
             corr=float(np.corrcoef(r.mean(0),br.mean(0))[0,1]),subperiods={})
  for label,mask in [('2023-24',dates.year<=2024),('2025-26',dates.year>=2025)]:
   ms=[metrics(r[p,mask],tr[p,mask],common[mask],.0007) for p in range(3)]
   bs=[metrics(br[p,mask],bt[p,mask],common[mask],.0007) for p in range(3)]
   rec['subperiods'][label]={k:float(np.mean([m[k] for m in ms])) for k in keys}
   rec['subperiods'][label]['days']=int(mask.sum())
   rec['subperiods'][label]['delta']=float(np.mean([m['sharpe']-b['sharpe'] for m,b in zip(ms,bs)]))
  d=load(name);rec['original_full']=metrics(d['r'],d['turn'],d['t'],.0007)
  rec['cost_cases']={}
  for fee in FEES:
   rr,tt=arrays(name,fee,common);bb,btt=arrays('baseline',fee,common)
   ms=[metrics(rr[p],tt[p],common,fee) for p in range(3)]
   cm={k:float(np.mean([m[k] for m in ms])) for k in keys}
   cm['subperiods']={}
   for label,mask in [('2023-24',dates.year<=2024),('2025-26',dates.year>=2025)]:
    ss=np.mean([sharpe(rr[p,mask]) for p in range(3)]);bss=np.mean([sharpe(bb[p,mask]) for p in range(3)])
    cm['subperiods'][label]=dict(sharpe=float(ss),delta=float(ss-bss),annual_arithmetic=float(rr[:,mask].mean()*365))
   rec['cost_cases'][str(fee)]=cm
  df=pd.read_csv(O/f'{name}_p0_f0.0007_daily.csv');df['actual_names']=df.actual_long+df.actual_short;df['target_names']=df.target_long+df.target_short
  periods[name]={}
  for label,mask in [('pre',df.date<'2024-03-05'),('drawdown',(df.date>='2024-03-05')&(df.date<='2024-10-15')),('post',df.date>'2024-10-15')]:
   x=df.loc[mask];a=x[x.kk>0]
   periods[name][label]=dict(days=len(x),active_days=len(a),**{c:float(x[c].mean()) for c in ['universe','selected_n','kk','turn','actual_names','actual_long','actual_short','target_names','normalizer']},
      active_kk_mean=float(a.kk.mean()),active_kk_min=int(a.kk.min()),active_kk_max=int(a.kk.max()),
      constrained_active_fraction=float(a.constrained.mean()),active_actual_names=float(a.actual_names.mean()),
      active_actual_long=float(a.actual_long.mean()),active_actual_short=float(a.actual_short.mean()),
      active_actual_names_min=int(a.actual_names.min()),active_actual_names_max=int(a.actual_names.max()),
      top_n_binding_days=int((x.selected_n<x.universe).sum()),zero_target_days=int((x.kk==0).sum()))
  records.append(rec)
 save('metrics.json',records);save('periods.json',periods)
 np.savez_compressed(O/'aligned_returns.npz',r=np.array(allarrays),t=common,names=NAMES)
 print('metrics checkpoint',flush=True)

def bootstrap():
 a=np.load(O/'aligned_returns.npz');r=a['r'];C,P,T=r.shape;flat=r.reshape(C*P,T);sq=flat*flat
 obs=(flat.mean(1)/flat.std(1,ddof=1)*np.sqrt(365)).reshape(C,P).mean(1);delta=obs[1:]-obs[0]
 stats={}
 for L,B in [(20,9999),(10,4999),(40,4999)]:
  rng=np.random.default_rng(20261009+L);path=O/f'bootstrap_L{L}_checkpoint.npz';boot=np.empty((B,C-1));done=0
  if path.exists():
   saved=np.load(path);done=int(saved['done']);boot[:done]=saved['delta'];rng.bit_generator.state=json.loads(str(saved['rng_state']))
  for start in range(done,B,100):
   nb=min(100,B-start);st=rng.integers(0,T,size=(nb,int(np.ceil(T/L))))
   ix=((st[:,:,None]+np.arange(L))%T).reshape(nb,-1)[:,:T]
   counts=np.zeros((nb,T))
   for j in range(nb):counts[j]=np.bincount(ix[j],minlength=T)
   mu=counts@flat.T/T;var=(counts@sq.T-T*mu*mu)/(T-1)
   sh=(mu/np.sqrt(np.maximum(var,1e-30))*np.sqrt(365)).reshape(nb,C,P).mean(2)
   boot[start:start+nb]=sh[:,1:]-sh[:,[0]]
   if (start+nb)%500==0 or start+nb==B:
    np.savez_compressed(path,done=start+nb,delta=boot[:start+nb],rng_state=json.dumps(rng.bit_generator.state))
  se=boot.std(0,ddof=1);null=(boot-delta)/se;z=delta/se;maximum=null.max(1)
  p=(1+(null>=z).sum(0))/(B+1);pf=(1+(maximum[:,None]>=z).sum(0))/(B+1)
  stats[str(L)]=dict(B=B,block_days=L,names=NAMES[1:],observed_delta=delta.tolist(),se=se.tolist(),p=p.tolist(),p_fwer=pf.tolist(),
                    basic_ci95_low=(2*delta-np.quantile(boot,.975,axis=0)).tolist(),basic_ci95_high=(2*delta-np.quantile(boot,.025,axis=0)).tolist())
  np.savez_compressed(O/f'bootstrap_L{L}.npz',delta=boot,centered_z=null,observed_z=z,max_z=maximum)
  save('bootstrap.json',stats)
  print('bootstrap',L,B,'checkpoint',flush=True)
 records=json.loads((O/'metrics.json').read_text());b=records[0]
 for j,rec in enumerate(records):
  if j:
   rec['bootstrap']={L:{k:s[k][j-1] for k in ['se','p','p_fwer','basic_ci95_low','basic_ci95_high']} for L,s in stats.items()}
   rec['checks']=dict(fwer=all(s['p_fwer'][j-1]<.05 for s in stats.values()),
      positive_subperiods=all(s['sharpe']>0 for s in rec['subperiods'].values()),
      improvement_gt_phase_noise=rec['delta']>rec['phase_delta_range'],
      turnover_le_baseline=rec['daily_turnover']<=b['daily_turnover'])
   rec['usable']=all(rec['checks'].values())
   rec['protocol_both_improved']=all(s['delta']>0 for s in rec['subperiods'].values())
   rec['cost_075_both_improved']=all(s['delta']>0 for s in rec['cost_cases']['0.00075']['subperiods'].values())
   rec['drawdown_improvement']=rec['max_drawdown']-b['max_drawdown']
   rec['risk_reduced']=rec['drawdown_improvement']>=.10 and rec['delta']>=-.15
   rec['risk_all_starts']=all(m['max_drawdown']-bm['max_drawdown']>=.10 and m['sharpe']-bm['sharpe']>=-.15 for m,bm in zip(rec['phase_metrics'],b['phase_metrics']))
 save('results.json',records)
 print(pd.DataFrame([{k:x.get(k) for k in ['name','sharpe','delta','daily_turnover','annual_cost','cagr','max_drawdown','underwater_days','usable','risk_reduced']} for x in records]).to_string(index=False))

if __name__=='__main__':
 if len(sys.argv)==1 or sys.argv[1]=='metrics':collect()
 if len(sys.argv)==1 or sys.argv[1]=='bootstrap':bootstrap()
