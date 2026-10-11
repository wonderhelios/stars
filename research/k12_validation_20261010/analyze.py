"""Complete descriptive metrics and preregistered paired nonlinear maxT."""
import json,hashlib,sys
from pathlib import Path
import numpy as np
import pandas as pd
from engine import OUT,savejson,savenpz,key
NAMES=['B0','K12','A','A_const','B','B_sign','C','C_diag']
PAIRS=[('K12','B0'),('A','B0'),('A','K12'),('A','A_const'),('B','B0'),('B','K12'),('B','B_sign'),('C','B0'),('C','K12'),('C','C_diag')]
FEES=[.0007,.00075,.00105,.00045,.00015]
def load(name,p=0,fee=.0007,delay=0):return dict(np.load(OUT/f'{key(name,p,fee,delay)}.npz'))
def sh(r):return float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365))
def dd(r,t):
 dates=pd.to_datetime(t,unit='ms',utc=True);dates=pd.DatetimeIndex([dates[0]-pd.Timedelta(days=1)]+list(dates))
 e=np.r_[1.,np.cumprod(1+r)];hi=np.maximum.accumulate(e);d=e/hi-1;v=int(d.argmin());p=int(np.flatnonzero(e[:v+1]==hi[v])[-1]);re=np.flatnonzero(e[v:]>=hi[v]);re=int(v+re[0]) if len(re) else None;end=len(e)-1 if re is None else re
 last=0;longest=0
 for i in range(1,len(e)):
  if e[i]>=hi[i-1] or i==len(e)-1:
   duration=(dates[i]-dates[last]).days if i>last+1 or e[i]<hi[i-1] else 0;longest=max(longest,duration)
  if e[i]>=hi[i-1]:last=i
 return dict(max_drawdown=float(d[v]),peak=str(dates[p].date()),trough=str(dates[v].date()),recovery=None if re is None else str(dates[re].date()),decline_days=int((dates[v]-dates[p]).days),recovery_days=None if re is None else int((dates[re]-dates[v]).days),underwater_days=int((dates[end]-dates[p]).days),censored=re is None,longest_underwater_days=int(longest))
def met(d,mask,fee):
 r=d['r'][mask];t=d['t'][mask];nf=d['neff'][mask]
 return dict(days=len(r),start=str(pd.to_datetime(t[0],unit='ms',utc=True).date()),end=str(pd.to_datetime(t[-1],unit='ms',utc=True).date()),sharpe=sh(r),annual_arithmetic=float(r.mean()*365),cagr=float(np.expm1(np.log1p(r).mean()*365)),daily_vol=float(r.std(ddof=1)),daily_turnover=float(d['turn'][mask].mean()),annual_cost=float(d['turn'][mask].mean()*fee*365),dollar_cost=float(d['cost'][mask].sum()),gross_mean=float(d['gross'][mask].mean()),gross_peak=float(d['gross'][mask].max()),neff_active_mean=float(np.nanmean(nf)),neff_min=float(np.nanmin(nf)),neff_max=float(np.nanmax(nf)),cash_days=int(np.isnan(nf).sum()),**dd(r,t))
def collect():
 common=load('B0',2)['t'];dates=pd.to_datetime(common,unit='ms',utc=True);records={};aligned=[]
 for name in NAMES:
  records[name]={};ar=[]
  cases=[(0,fee) for fee in FEES]+[(1,fee) for fee in FEES[:3]] if name in NAMES[:4] else [(0,.0007)]
  for delay,fee in cases:
   phases=[]
   for p in range(3):
    d=load(name,p,fee,delay);dt=pd.to_datetime(d['t'],unit='ms',utc=True);inc=np.isin(d['t'],common)
    masks={'full':np.ones(len(dt),bool),'2023-24':dt.year<=2024,'2025-26':dt.year>=2025,'since2024':dt>=pd.Timestamp('2024-01-01',tz='UTC'),'common':inc,'common_2023-24':inc&(dt.year<=2024),'common_2025-26':inc&(dt.year>=2025)}
    phases.append({lab:met(d,mask,fee) for lab,mask in masks.items()})
    if delay==0 and fee==.0007:ar.append(d['r'][inc])
   records[name][f'f{fee}_d{delay}']=phases
  aligned.append(ar)
 savejson(OUT/'metrics.json',records);savenpz(OUT/'aligned_returns.npz',r=np.array(aligned),t=common,names=NAMES)
 contrasts={}
 for name,base in PAIRS:
  label=f'{name}-{base}';contrasts[label]={}
  for case in records[name]:
   if case not in records[base]:continue
   a=records[name][case];b=records[base][case];s={}
   for period in a[0]:
    delta=[aa[period]['sharpe']-bb[period]['sharpe'] for aa,bb in zip(a,b)]
    s[period]=dict(delta_mean=float(np.mean(delta)),delta_range=float(np.ptp(delta)),delta_by_start=delta,sharpe_mean=float(np.mean([aa[period]['sharpe'] for aa in a])),base_sharpe_mean=float(np.mean([bb[period]['sharpe'] for bb in b])))
   contrasts[label][case]=s
 savejson(OUT/'contrasts.json',contrasts)
 corr={}
 for period,mask in [('common',np.ones(len(common),bool)),('2023-24',dates.year<=2024),('2025-26',dates.year>=2025),('since2024',dates>=pd.Timestamp('2024-01-01',tz='UTC'))]:
  cor=np.corrcoef(np.array(aligned).mean(1)[:,mask]);pd.DataFrame(cor,index=NAMES,columns=NAMES).to_csv(OUT/f'correlation_{period}.csv');corr[period]=cor.tolist()
 savejson(OUT/'correlations.json',dict(names=NAMES,periods=corr))
 rows=[]
 for name,cases in records.items():
  for case,phases in cases.items():
   for p,periods in enumerate(phases):
    for period,m in periods.items():rows.append(dict(name=name,case=case,start_offset=p,period=period,**m))
 pd.DataFrame(rows).to_csv(OUT/'metrics.csv',index=False)
 print('metrics checkpoint',flush=True)

def bootstrap():
 a=np.load(OUT/'aligned_returns.npz');r=a['r'];C,P,T=r.shape;flat=r.reshape(C*P,T);sq=flat*flat;left=np.array([NAMES.index(x) for x,y in PAIRS]);right=np.array([NAMES.index(y) for x,y in PAIRS]);obs=(flat.mean(1)/flat.std(1,ddof=1)*np.sqrt(365)).reshape(C,P).mean(1);delta=obs[left]-obs[right];stats={}
 for L in [10,20,40]:
  B=9999;rng=np.random.default_rng(20261010);cp=OUT/f'bootstrap_L{L}_checkpoint.npz';boot=np.empty((B,len(PAIRS)));done=0;maxerr=0.
  if cp.exists():
   d=np.load(cp);done=int(d['done']);boot[:done]=d['delta'];rng.bit_generator.state=json.loads(str(d['rng_state']));maxerr=float(d['direct_check_max_error'])
  for start in range(done,B,100):
   nb=min(100,B-start);st=rng.integers(0,T,size=(nb,int(np.ceil(T/L))));ix=((st[:,:,None]+np.arange(L))%T).reshape(nb,-1)[:,:T];counts=np.zeros((nb,T))
   for j in range(nb):counts[j]=np.bincount(ix[j],minlength=T)
   mu=counts@flat.T/T;var=(counts@sq.T-T*mu*mu)/(T-1)
   assert (var>0).all(),'Zero variance bootstrap path'
   s=(mu/np.sqrt(var)*np.sqrt(365)).reshape(nb,C,P).mean(2);boot[start:start+nb]=s[:,left]-s[:,right]
   if start==0:
    for j in range(min(nb,10)):
     direct=r[:,:,ix[j]];ds=(direct.mean(2)/direct.std(2,ddof=1)*np.sqrt(365)).mean(1);maxerr=max(maxerr,float(abs(ds[left]-ds[right]-boot[j]).max()))
    assert maxerr<1e-10
   if (start+nb)%500==0 or start+nb==B:
    savenpz(cp,done=start+nb,delta=boot[:start+nb],rng_state=json.dumps(rng.bit_generator.state),direct_check_max_error=maxerr)
  se=boot.std(0,ddof=1);assert (se>0).all();null=(boot-delta)/se;z=delta/se;maximum=null.max(1);pf=(1+(maximum[:,None]>=z).sum(0))/(B+1)
  stats[str(L)]=dict(B=B,seed=20261010,days=T,comparisons=[f'{x}-{y}' for x,y in PAIRS],observed_delta=delta.tolist(),se=se.tolist(),p_fwer=pf.tolist(),basic_ci95_low=(2*delta-np.quantile(boot,.975,axis=0)).tolist(),basic_ci95_high=(2*delta-np.quantile(boot,.025,axis=0)).tolist(),direct_check_max_error=maxerr)
  savenpz(OUT/f'bootstrap_L{L}.npz',delta=boot,centered_z=null,observed_z=z,max_z=maximum);savejson(OUT/'bootstrap.json',stats);print('bootstrap checkpoint',L,pf[:4],flush=True)

def diagnostics():
 summary={};common=load('B0',2)['t']
 for name in NAMES[:4]:
  d=load(name);dt=pd.to_datetime(d['t'],unit='ms',utc=True);j=int(np.flatnonzero(dt==pd.Timestamp('2024-03-05',tz='UTC'))[0]);v=int(np.flatnonzero(dt==pd.Timestamp('2024-10-15',tz='UTC'))[0]);later=np.flatnonzero(d['equity'][v+1:]>=d['equity'][j]);rec=int(v+1+later[0]) if len(later) else len(dt)-1
  df=pd.DataFrame({x:d[x] for x in ['t','r','equity','price_pnl','cost','gross','gross_pre','target_gross','g','turn','neff']});df.insert(0,'date',dt.strftime('%Y-%m-%d'));df=df.iloc[j:rec+1].copy();df['cumulative_price_pnl']=df.price_pnl.cumsum();df['cumulative_cost']=df.cost.cumsum();df.to_csv(OUT/f'drawdown_recovery_{name}.csv',index=False)
  periods={}
  for label,lo,hi in [('fixed_drawdown',j,v),('recovery',v+1,rec)]:
   pp=float(d['price_pnl'][lo:hi+1].sum());cc=float(d['cost'][lo:hi+1].sum());initial=float(d['equity_before'][lo]);end=float(d['equity'][hi]);assert abs(initial+pp-cc-end)<1e-6*initial
   periods[label]=dict(start=str(dt[lo].date()),end=str(dt[hi].date()),days=hi-lo+1,gross_mean=float(d['gross'][lo:hi+1].mean()),price_pnl=pp,cost=cc,starting_equity=initial,ending_equity=end,price_pnl_per_start=pp/initial,cost_per_start=cc/initial,net_return=end/initial-1)
  periods['anchor_recovery']=str(dt[rec].date()) if len(later) else None;summary[name]=periods
 a=load('A');b=load('B0');g=a['g'][:-1];ar=a['price_pnl'][1:]/a['equity_before'][1:];br=b['price_pnl'][1:]/b['equity_before'][1:];valid=a['target_gross'][:-1]>0
 relation={}
 for label,mask in [('all_calendar',np.ones(len(g),bool)),('nonzero_g_descriptive',valid)]:
  relation[label]=dict(n=int(mask.sum()),g_mean=float(g[mask].mean()),g_std=float(g[mask].std(ddof=1)),corr_A_gross=float(np.corrcoef(g[mask],ar[mask])[0,1]),corr_A_squared=float(np.corrcoef(g[mask],ar[mask]**2)[0,1]),corr_B0_gross=float(np.corrcoef(g[mask],br[mask])[0,1]),corr_B0_squared=float(np.corrcoef(g[mask],br[mask]**2)[0,1]))
 pd.DataFrame(dict(execution_date=pd.to_datetime(a['t'][:-1],unit='ms',utc=True).strftime('%Y-%m-%d'),g=g,next_holding_A_gross_return=ar,next_holding_B0_gross_return=br)).to_csv(OUT/'g_next_return.csv',index=False)
 savejson(OUT/'drawdown_diagnostics.json',summary);savejson(OUT/'g_relation.json',relation)

def gates():
 metrics=json.loads((OUT/'metrics.json').read_text());contrasts=json.loads((OUT/'contrasts.json').read_text());stats=json.loads((OUT/'bootstrap.json').read_text());out={}
 for j,(name,base) in enumerate(PAIRS[:4]):
  c=contrasts[f'{name}-{base}'];main=c['f0.0007_d0'];cost=c['f0.00075_d0'];a=metrics[name]['f0.0007_d0'];b=metrics['B0']['f0.0007_d0'];ratio=np.mean([x['common']['daily_turnover'] for x in a])/np.mean([x['common']['daily_turnover'] for x in b])
  checks=dict(positive_delta=main['common']['delta_mean']>0,all_three_fwer=all(x['p_fwer'][j]<.05 for x in stats.values()),both_segments_positive_and_improved=all(main[p]['sharpe_mean']>0 and main[p]['delta_mean']>0 for p in ['common_2023-24','common_2025-26']),both_segments_improved_at_7_5bp=all(cost[p]['delta_mean']>0 for p in ['common_2023-24','common_2025-26']),startup_mean_exceeds_range=main['common']['delta_mean']>main['common']['delta_range'],turnover_budget=ratio<=1.1)
  sensitivity={case:{period:c[case][period]['delta_mean'] for period in ['common','common_2023-24','common_2025-26']} for case in ['f0.00105_d0','f0.0007_d1','f0.00075_d1','f0.00105_d1']}
  out[f'{name}-{base}']=dict(checks=checks,passed=all(checks.values()),turnover_ratio_B0=ratio,execution_sensitive=any(v<=0 for case in sensitivity.values() for v in case.values()),stress_deltas=sensitivity)
 savejson(OUT/'gates.json',out)
if __name__=='__main__':
 collect();diagnostics();bootstrap();gates()
