"""Crash-volume proxies, not identified liquidation events. All hold-period phases."""
import json,glob,numpy as np,pandas as pd
from pathlib import Path
OUT=Path(__file__).resolve().parent;HOUR=3600000;DAY=86400000
raw={Path(f).stem:json.load(open(f)) for f in sorted(glob.glob('/tmp/hl-hist/*.json'))};names=sorted(raw)
times=sorted({int(x['t']) for d in raw.values() for x in d});ti={t:i for i,t in enumerate(times)}
a={k:np.full((len(times),len(names)),np.nan) for k in ['o','c','v']}
for j,name in enumerate(names):
 for c in raw[name]:
  for k in a:a[k][ti[int(c['t'])],j]=float(c[k])
C=a['c'];Q=C*a['v'];R=C/np.roll(C,1,axis=0)-1;R[0]=np.nan
sd=pd.DataFrame(R).rolling(168,min_periods=120).std().shift(1).values
vol=pd.DataFrame(Q).rolling(168,min_periods=120).mean().shift(1).values
liq=pd.DataFrame(Q).rolling(720,min_periods=120).mean().shift(1).values*24
shock=Q/vol; crash=R<-np.maximum(.03,2.5*sd)
OP=pd.DataFrame(a['o']).ffill().values
START=720;END=len(times)-26
hourdates=pd.to_datetime(times,unit='ms',utc=True);days=sorted(set(t//DAY*DAY for t in times[START+1:END+26]));di={d:i for i,d in enumerate(days)}

def stat(r):
 return float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365)) if np.std(r)>0 else 0.
records={};xs=[];summary={};labels=[]
for h in [6,12,24]:
 for high in [True,False]:
  key=('cascade' if high else 'ordinary_crash')+f'_{h}h';phase_r=[];phase_events=[];phase_returns=[];phase_turn=[];phase_missing=[];phase_mom=[];score_corr=[];event_beta=[]
  for phase in range(h):
   daily=np.zeros(len(days));turn=np.zeros(len(days));ev=[];mom=[];miss=0
   for i in range(START+phase,END,h):
    eligible=(liq[i]>=5e6)&np.isfinite(C[i])&np.isfinite(a['o'][i+1])
    event=eligible&crash[i]&((shock[i]>2) if high else (shock[i]<=2))
    peers=eligible&~event
    if not event.any() or not peers.any():continue
    w=np.zeros(len(names));w[event]=.5/event.sum();w[peers]=-.5/peers.sum()
    lag=i-336
    if lag>=0:
     mm=C[i]/C[lag]-1;good=eligible&np.isfinite(mm)
     if good.sum()>=8 and np.std(mm[good])>0:score_corr.append(float(np.corrcoef(event[good].astype(float),-mm[good])[0,1]))
    entry=OP[i+1];d0=di[times[i+1]//DAY*DAY];de=di[times[i+1+h]//DAY*DAY]
    # Unit initial gross; both market legs crossed at observable next opens.
    daily[d0]-=.00075;turn[d0]+=1
    for j in range(i+1,i+1+h):
     delta=np.nan_to_num((OP[j+1]-OP[j])/entry)
     daily[di[times[j+1]//DAY*DAY]]+=w@delta
    exitgross=np.nansum(np.abs(w)*OP[i+1+h]/entry)
    daily[de]-=exitgross*.00075;turn[de]+=exitgross
    ret=OP[i+1+h]/entry-1;hedge=np.nanmean(ret[peers]);ev.extend((ret[event]-hedge-.00075*(4+ret[event]+hedge)).tolist())
    miss+=int((~np.isfinite(a['o'][i+1+h,event])).sum())
    if lag>=0:mom.extend((C[i,event]/C[lag,event]-1).tolist())
   phase_r.append(daily);phase_events.append(len(ev));phase_returns.append(float(np.mean(ev)) if len(ev) else None);phase_turn.append(turn);phase_missing.append(miss);phase_mom.append(float(np.nanmean(mom)) if len(mom) else None)
  r=np.array(phase_r);xs.append(r.mean(0));labels.append(key)
  summary[key]={'phase_sharpes':[stat(v) for v in r],'sharpe_phase_mean':float(np.mean([stat(v) for v in r])),'sharpe_phase_sd':float(np.std([stat(v) for v in r])),'phase_event_counts':phase_events,'events_all_phases':sum(phase_events),'phase_mean_net_market_relative_trade':phase_returns,'phase_mean_prior14d_return':phase_mom,'net_market_relative_trade_all_phases':float(np.average([v or 0 for v in phase_returns],weights=phase_events)) if sum(phase_events) else None,'annual_cost':float(np.array(phase_turn).mean(0).mean()*.00075*365),'annual_abs_traded_equity':float(np.array(phase_turn).mean(0).mean()*365),'missing_event_exit_prices':sum(phase_missing),'score_correlation_14d_reversal_mean':float(np.nanmean(score_corr)) if score_corr else None,'2023-24':'UNAVAILABLE; cannot qualify'}
  records[key]=r
# Paired calendar-day bootstrap, averaged over ALL phases, centered net-PnL means.
x=np.array(xs);obs=x.mean(1);B=4999;rng=np.random.default_rng(610082);boot={}
for L in [15,30,60]:
 vals=[]
 for rep in range(B):
  start=rng.integers(len(days),size=int(np.ceil(len(days)/L)));inds=((start[:,None]+np.arange(L))%len(days)).ravel()[:len(days)]
  vals.append(x[:,inds].mean(1))
 vals=np.array(vals);se=vals.std(0,ddof=1);maxt=((vals-obs)/np.maximum(se,1e-12)).max(1)
 boot[str(L)]={key:{'p_hourly_maxT':float((1+(maxt>=obs[j]/max(se[j],1e-12)).sum())/(B+1)),'p_fwer_all14':float(min(1,2*(1+(maxt>=obs[j]/max(se[j],1e-12)).sum())/(B+1))),'annual_mean_ci95':(np.quantile(vals[:,j],[.025,.975])*365).tolist()} for j,key in enumerate(labels)}
# Align to same baseline dates; phase-average PnL only for correlation, never its Sharpe.
f=OUT/'baseline_returns.csv'
if f.exists():
 b=pd.read_csv(f);b.index=pd.to_datetime(b.date,utc=True);cols=[f'baseline_phase{i}' for i in range(3)]
 b=b[cols].mean(1);dateidx=pd.to_datetime(days,unit='ms',utc=True);bb=b.reindex(dateidx)
 for j,key in enumerate(labels):
  m=np.isfinite(bb);summary[key]['correlation_baseline']=float(np.corrcoef(x[j,m],bb[m])[0,1])
json.dump({'meta':{'files':len(raw),'start':str(hourdates[START+1]),'end':str(hourdates[END]),'test_days':len(days),'bootstrap':B,'hypotheses':6,'event_definition':'1h drop > max(3%,2.5*prior168h sigma); high-volume >2*prior168h mean; ordinary-crash <=2; prior720h daily volume>=5M','cost':'taker+slippage .075% per leg per side; long-short net event spread pays .30%','overlap':'Each H has all H integer-hour grid phases; within each phase holding windows cannot overlap; shared calendar-day blocks preserve cross-coin/candidate dependence','limitations':'Volume crash is NOT liquidation; 2026 only; funding omitted; exit marks forward-filled if missing and count reported; fixed unit initial notional not compounding equity'},'summary':summary,'bootstrap':boot},open(OUT/'results_hourly.json','w'),indent=2)
pd.DataFrame({'date':pd.to_datetime(days,unit='ms',utc=True).astype(str),**{k:x[j] for j,k in enumerate(labels)}}).to_csv(OUT/'hourly_daily_returns.csv',index=False)
for key in labels:print(key,summary[key],boot['30'][key],flush=True)
