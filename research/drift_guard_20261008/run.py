"""No intraday proxies. Daily baseline + explicitly limited observed hourly windows."""
from panel import *
import math
FEE=[.00075,.00105,.00145]
CAND=[('net'+str(int(t*100)),t,False) for t in [.02,.03,.05,.08]]+ [('net'+str(int(t*100))+'_lev85',t,True) for t in [.02,.03,.05,.08]]
T,_,univ=targets(8)
assert np.max(np.abs(T.sum(1)))<1e-10
assert np.max(np.abs(T).sum(1))<=1+1e-10

def sh(r):
 return float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365))
def stats(r):
 e=np.r_[1,np.cumprod(1+r)]
 return dict(sharpe=sh(r),annual_arithmetic=float(np.mean(r)*365),cagr=float(e[-1]**(365/len(r))-1),max_drawdown=float(np.min(e/np.maximum.accumulate(e)-1)))
def drift(w):
 g=np.abs(w).sum();return abs(w.sum())/g if g>0 else 0

def daily(fee):
 OP=pd.DataFrame(a['o']).ffill().values
 # All completed daily bars, prior signal -> following observed daily open.
 start=next(i for i in range(33,n-1) if np.abs(T[i-1]).sum()>0)
 w=np.zeros(k);r=[];turn=[];pre=[];post=[];ne=[];missing=[];events=[]
 for i in range(start,n):
  gain=0 if i==start else w@np.nan_to_num(OP[i]/OP[i-1]-1)
  if 1+gain<=0:raise ValueError('insolvency')
  if i>start:w=w*np.nan_to_num(OP[i]/OP[i-1],nan=1)/(1+gain)
  pre.append(drift(w));ne.append(abs(w.sum()));missing.append(float(np.abs(w[~np.isfinite(a['o'][i])]).sum()))
  target=T[i-1]*2.7
  # Retains live 2% resize band, all positions in daily slot. Large-account fills.
  delta=target-w;use=(np.abs(delta)>=.02*np.abs(target))|(target*w<=0)
  valid=np.isfinite(a['o'][i]);delta=np.where(use&valid,delta,0);delta[~valid]=-w[~valid]
  # Missing held marks: explicitly hypothetical stale exit, never open at stale price.
  tr=np.abs(delta).sum();cost=fee*tr
  w=(w+delta)/(1-cost)
  r.append((1+gain)*(1-cost)-1);turn.append(tr);post.append(drift(w));events.append(int(tr>1e-12))
 terminal=np.abs(w).sum();r[-1]=(1+r[-1])*(1-fee*terminal)-1;turn[-1]+=terminal
 dd=dates[start:];rr=np.array(r);tt=np.array(turn)
 out={}
 for label,m in [('full',np.ones(len(rr),bool)),('2023-24',dd.year<=2024),('2025-26',dd.year>=2025)]:
  pv=np.array(pre)[m];out[label]=dict(**stats(rr[m]),days=int(m.sum()),rebalance_events=int(np.array(events)[m].sum()),turnover_equity_sum=float(tt[m].sum()),daily_turnover_equity=float(tt[m].mean()),annual_cost=float(tt[m].mean()*fee*365),net_gross_pre=dict(mean=float(pv.mean()),p90=float(np.quantile(pv,.9)),max=float(pv.max())),net_gross_post=dict(mean=float(np.array(post)[m].mean()),p90=float(np.quantile(np.array(post)[m],.9)),max=float(np.array(post)[m].max())),net_equity_pre_max=float(np.array(ne)[m].max()),missing_mark_events=int((np.array(missing)[m]>1e-12).sum()),missing_mark_exposure_max=float(np.array(missing)[m].max()))
 np.savez_compressed(OUT+f'/daily_{fee}.npz',dates=np.asarray(dd.astype(str),dtype='U'),r=rr,turn=tt,pre=pre,post=post)
 return out
D={str(f):daily(f) for f in FEE}
print('DAILY',D,flush=True)
# Only two >=30-day contiguous full-target hourly coverage windows.
# Coverage selection is based on prices, never P&L. Restart flat at each boundary.
windows=[('2026-03-15','2026-05-06'),('2026-06-17','2026-07-28')]
hr={os.path.basename(f)[:-5]:{int(x['t']):float(x['o']) for x in json.load(open(f)) if float(x['o'])>0} for f in glob.glob('/tmp/hl-hist/*.json')}
segments=[]
for st,en in windows:
 lo=ti[int(pd.Timestamp(st,tz='UTC').value//1000000)];hi=ti[int(pd.Timestamp(en,tz='UTC').value//1000000)]
 ts=np.arange(times[lo],times[hi]+24*3600000,3600000,dtype=np.int64)
 Phr=np.array([[hr.get(name,{}).get(int(t),np.nan) for name in names] for t in ts])
 segments.append((lo,hi,ts,Phr))

def hourly(mode,threshold,lowlev,phase,fee):
 rs=[];turn=[];events=[];extra=[];dr=[];after=[];costs=[]
 for lo,hi,ts,Phr in segments:
  w=np.zeros(k);last=None
  for day in range(lo,hi):
   daygain=1.;dayturn=0.;de=0;ex=0;dc=0
   for h in range(24):
    q=(day-lo)*24+phase+h;price=Phr[q];absolute_hour=phase+h;current_day=day+absolute_hour//24
    held=np.abs(w)>1e-12
    assert np.isfinite(price[held]).all(),'missing held hourly price'
    gain=0 if last is None else w[held]@(price[held]/last[held]-1)
    assert 1+gain>0
    if last is not None:
     ratio=np.ones(k);ratio[held]=price[held]/last[held];w=w*ratio/(1+gain)
    daygain*=1+gain;dr.append(drift(w))
    scheduled=(h==0)
    trigger=not scheduled and ((mode=='always' and h==12) or (mode=='guard' and (abs(w.sum())>threshold or (lowlev and np.abs(w).sum()<2.7*.85))))
    if scheduled or trigger:
     target=T[current_day-1]*2.7;ids=np.abs(target)>1e-12
     assert np.isfinite(price[ids]).all(),'missing target hourly price'
     delta=target-w;use=(np.abs(delta)>=.02*np.abs(target))|(target*w<=0);delta=np.where(use,delta,0)
     tr=np.abs(delta).sum();c=tr*fee;assert c<1
     w=(w+delta)/(1-c);daygain*=1-c;dayturn+=tr;dc+=c
     if tr>1e-12:de+=1;ex+=int(trigger)
    after.append(drift(w));last=price
   # Mark to next daily phase; subsequent loop must not count interval twice.
   p=Phr[(day+1-lo)*24+phase];held=np.abs(w)>1e-12;assert np.isfinite(p[held]).all()
   gain=w[held]@(p[held]/last[held]-1);ratio=np.ones(k);ratio[held]=p[held]/last[held];w=w*ratio/(1+gain);last=p;daygain*=1+gain
   if day==hi-1:
    tr=np.abs(w).sum();c=fee*tr;daygain*=1-c;dayturn+=tr;dc+=c
   rs.append(daygain-1);turn.append(dayturn);events.append(de);extra.append(ex);costs.append(dc)
 return dict(r=np.array(rs),turn=np.array(turn),events=np.array(events),extra=np.array(extra),pre=np.array(dr),post=np.array(after),cost=np.array(costs))
H={}
for f in FEE:
 H[str(f)]={}
 for name,mode,t,ll in [('baseline','base',0,False),('always_twice','always',0,False)]+[(name,'guard',t,ll) for name,t,ll in CAND]:
  H[str(f)][name]=[hourly(mode,t,ll,o,f) for o in range(24)]
 print('HOURLY',f,[(name,round(np.mean([sh(x['r']) for x in runs]),4)) for name,runs in H[str(f)].items()],flush=True)

def summarize(runs):
 ss=np.array([sh(v['r']) for v in runs]);pre=np.concatenate([v['pre'] for v in runs]);post=np.concatenate([v['post'] for v in runs]);avg=np.mean([v['r'] for v in runs],axis=0)
 return dict(sharpe_phase_mean=float(ss.mean()),phase_sd=float(ss.std()),phase_min=float(ss.min()),phase_max=float(ss.max()),phase_sharpes=ss.tolist(),sharpe_average_return=sh(avg),annual_arithmetic=float(avg.mean()*365),rebalance_events_mean=float(np.mean([v['events'].sum() for v in runs])),extra_events_mean=float(np.mean([v['extra'].sum() for v in runs])),turnover_equity_sum=float(np.mean([v['turn'].sum() for v in runs])),daily_turnover_equity=float(np.mean([v['turn'].mean() for v in runs])),annual_cost=float(np.mean([v['cost'].mean() for v in runs])*365),net_gross_pre=dict(mean=float(pre.mean()),p90=float(np.quantile(pre,.9)),max=float(pre.max())),net_gross_post=dict(mean=float(post.mean()),p90=float(np.quantile(post,.9)),max=float(post.max())),correlation_baseline=0)
S={f:{name:summarize(rr) for name,rr in runs.items()} for f,runs in H.items()}
for f,runs in H.items():
 b=np.mean([v['r'] for v in runs['baseline']],axis=0)
 for name,rr in runs.items():S[f][name]['correlation_baseline']=float(np.corrcoef(b,np.mean([v['r'] for v in rr],axis=0))[0,1])
# One family: 8 guards x 3 costs x two comparators + always vs baseline x3 =51 hypotheses.
# Sharpe influence-function paired moving-block maxT, phase-average statistic.
def sif(r):
 mu=r.mean();sd=r.std(ddof=1);return np.sqrt(365)*((r-mu)/sd-mu*((r-mu)**2-sd**2)/(2*sd**3))
labels=[];IF=[];obs=[]
for f,runs in H.items():
 for name in [x[0] for x in CAND]+['always_twice']:
  for comp in (['baseline','always_twice'] if name!='always_twice' else ['baseline']):
   labels.append(f+'|'+name+'|'+comp)
   IF.append(np.mean([sif(v['r']) for v in runs[name]],axis=0)-np.mean([sif(v['r']) for v in runs[comp]],axis=0))
   obs.append(S[f][name]['sharpe_phase_mean']-S[f][comp]['sharpe_phase_mean'])
IF=np.array(IF).T;obs=np.array(obs);N=len(IF);B=9999;boot={};lengths=[hi-lo for lo,hi,_,_ in segments]
for L in [5,10,20]:
 rng=np.random.default_rng(20261008+L);vals=[]
 for start in range(0,B,250):
  b=min(250,B-start);counts=np.zeros((b,N));pos=0
  for nn in lengths:
   starts=rng.integers(nn,size=(b,math.ceil(nn/L)));ind=((starts[:,:,None]+np.arange(L))%nn).reshape(b,-1)[:,:nn]+pos
   for z in range(b):counts[z]+=np.bincount(ind[z],minlength=N)
   pos+=nn
  vals.append(counts@(IF-IF.mean(axis=0))/N)
 vals=np.concatenate(vals);se=vals.std(0,ddof=1);maxstat=(vals/np.maximum(se,1e-12)).max(1)
 boot[str(L)]={label:dict(delta_sharpe=float(obs[j]),se=float(se[j]),ci95=(obs[j]+np.quantile(vals[:,j],[.025,.975])).tolist(),p_fwer=float((1+(maxstat>=obs[j]/max(se[j],1e-12)).sum())/(B+1))) for j,label in enumerate(labels)}
 print('BOOT',L,min(v['p_fwer'] for v in boot[str(L)].values()),flush=True)
meta=dict(files=len(FILES),daily_start=str(dates[next(i for i in range(33,n-1) if np.abs(T[i-1]).sum()>0)]),daily_end=str(dates[-1]),hourly_windows=windows,hourly_days=N,phase_count=24,hypotheses=len(labels),daily_guard='not identifiable intraday; daily sampled coalesces with mandatory rebalance',hourly_universe='full daily universe and full weights, no restricted universe or proxy marks',target_cap='8 per factor leg, per live weights_at; merged composite may exceed 8 per side',execution='2% live resize band; no minimum order, rounding or funding; large-account approximation',hourly_liquidations=2)
json.dump(dict(meta=meta,daily=D,hourly=S,bootstrap=boot),open(OUT+'/results.json','w'),indent=2)
np.savez_compressed(OUT+'/hourly_returns.npz',**{f+'|'+name:np.array([v['r'] for v in rr]) for f,runs in H.items() for name,rr in runs.items()})
manifest={os.path.basename(f):hashlib.sha256(open(f,'rb').read()).hexdigest() for f in FILES}
json.dump(manifest,open(OUT+'/manifest.json','w'),indent=2)
print('COMPLETE',meta,flush=True)
