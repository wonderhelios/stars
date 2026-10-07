"""Eight fixed daily hypotheses; paired block maxT. No tuning after results."""
import engine as e
import numpy as np,pandas as pd,json,hashlib
from pathlib import Path
OUT=Path(__file__).resolve().parent
# Daily snapshot has at least a full UTC-day allowance for archive publication.
# OI measured in base units, not USD, to avoid mechanically encoding price changes.
bn=pd.read_csv(OUT/'binance_daily_metrics.csv');bn['signal_date']=pd.to_datetime(bn.day,utc=True)+pd.Timedelta(days=1)
fields=['sum_open_interest','count_long_short_ratio','sum_toptrader_long_short_ratio','count_toptrader_long_short_ratio']
X={}
for f in fields:
 p=bn.pivot(index='signal_date',columns='coin',values=f).reindex(index=e.dates,columns=e.names)
 X[f]=p.apply(pd.to_numeric,errors='coerce').where(lambda x:x>0).values
logoi=np.log(X['sum_open_interest']);doi=logoi-np.roll(logoi,1,axis=0);doi[0]=np.nan
# volume is a flow, not a stock. "capacity" is a price impact proxy, NOT reconstructed OI.
logshock=np.log(np.maximum(e.shock,1e-12)); sign=np.sign(e.R)
amp=(e.a['h']-e.a['l'])/e.P
capacity=e.Q/np.maximum(amp,.001)
capshock=np.log(capacity/pd.DataFrame(capacity).rolling(30,min_periods=20).median().shift(1).values)
volpattern=sign*logshock
scores={
 'price_volume_confirm':volpattern,
 'price_volume_exhaust':-volpattern,
 'capacity_confirm':sign*capshock,
 'oi_confirm':np.sign(np.roll(e.R,1,axis=0))*doi,
 'oi_exhaust':-np.sign(np.roll(e.R,1,axis=0))*doi,
 'account_crowding_contra':-np.log(X['count_long_short_ratio']),
 'top_global_gap':np.log(X['sum_toptrader_long_short_ratio'])-np.log(X['count_long_short_ratio']),
 # Only crash+high-volume interaction. Pure reversal deliberately not retested.
 'cascade_rebound':np.where((e.R<-np.maximum(.05,2.5*e.SD))&(e.shock>2),-e.R*e.shock,0.)}
base=np.zeros((e.n,e.k));books={s:np.zeros_like(base) for s in scores};coverage={s:[] for s in scores}; scorecorr={s:[] for s in scores}
for i in range(32,e.n-1):
 idx=np.flatnonzero(np.array([':' not in c for c in e.names])&(e.V[i]>=5e6)&np.isfinite(e.P[i])&np.isfinite(e.P[i-14])&np.isfinite(e.SD[i]))
 if len(idx)<8:continue
 t=sum(e.book(idx,s,5) for s in [e.M[i]/np.maximum(e.SD[i],1e-9),-e.SD[i],e.shock[i]])/3
 if np.abs(t).sum()>1e-12:t/=np.abs(t).sum()
 base[i]=t
 for key,s in scores.items():
  good=idx[np.isfinite(s[i,idx])];coverage[key].append(len(good))
  if key=='cascade_rebound':
   event=good[s[i,good]>0]; other=good[s[i,good]==0]
   # Avoid arbitrary coin-name ties in a sparse event score.
   if len(event) and len(other):
    books[key][i,event]=.5/len(event);books[key][i,other]=-.5/len(other)
  else:books[key][i]=e.book(idx,s[i],5)
  if len(good)>=8 and np.std(s[i,good])>0:scorecorr[key].append(float(np.corrcoef(s[i,good],-e.M[i,good])[0,1]))
Ts={'baseline':base}
for key,b in books.items():
 Ts[key]=b
 t=.8*base+.2*b;g=np.abs(t).sum(axis=1)
 Ts['blend_'+key]=np.divide(t,g[:,None],out=np.zeros_like(t),where=g[:,None]>0)
runs={key:[e.run(t,offset=p) for p in range(3)] for key,t in Ts.items()}
summary={};rows=[]
for key,rr in runs.items():
 summary[key]={}
 for period,m in e.mask.items():
  ss=[e.stat(r['r'][m]) for r in rr];q={a:float(np.mean([s[a] for s in ss])) for a in ss[0]}
  q.update(phase_sharpes=[s['sharpe'] for s in ss],phase_sd=float(np.std([s['sharpe'] for s in ss])),correlation_baseline=float(np.mean([np.corrcoef(r['r'][m],runs['baseline'][p]['r'][m])[0,1] for p,r in enumerate(rr)])),daily_abs_traded_equity=float(np.mean([r['turn'][m].mean() for r in rr])))
  q['annual_cost']=q['daily_abs_traded_equity']*.00075*365
  summary[key][period]=q;rows.append(dict(candidate=key,period=period,**q))
# Centered bootstrap of Sharpe improvement. All candidates, phases and days paired.
keys=list(scores);b=np.array([v['r'] for v in runs['baseline']]);x=np.array([[v['r'] for v in runs['blend_'+s]] for s in keys])
observed=np.array([summary['blend_'+s]['full']['sharpe']-summary['baseline']['full']['sharpe'] for s in keys])
B=4999;rng=np.random.default_rng(610081);boot={}
for L in [15,30,60]:
 vals=[]
 for rep in range(B):
  start=rng.integers(len(e.ix),size=int(np.ceil(len(e.ix)/L)));inds=((start[:,None]+np.arange(L))%len(e.ix)).ravel()[:len(e.ix)]
  rb=b[:,inds];rx=x[:,:,inds]
  vals.append(np.mean(rx.mean(2)/rx.std(2,ddof=1)*np.sqrt(365)-rb.mean(1)/rb.std(1,ddof=1)*np.sqrt(365),axis=1))
 vals=np.array(vals);se=vals.std(0,ddof=1);maxt=((vals-observed)/se).max(1)
 boot[str(L)]={key:dict(delta_sharpe=float(observed[j]),ci95=np.quantile(vals[:,j],[.025,.975]).tolist(),p_daily_maxT=float((1+(maxt>=observed[j]/se[j]).sum())/(B+1)),p_fwer_all14=float(min(1,2*(1+(maxt>=observed[j]/se[j]).sum())/(B+1)))) for j,key in enumerate(keys)}
 print('block',L,flush=True)
stress={}
for key in ['baseline']+['blend_'+s for s in keys]:
 stress[key]={}
 for label,fee,pen in [('slippage_6bp',.00105,0),('missing_exit_5pct',.00075,.05),('maker_fee_optimistic',.00045,0)]:
  rr=[e.run(Ts[key],offset=p,fee=fee,missing_penalty=pen) for p in range(3)]
  stress[key][label]={period:float(np.mean([e.stat(r['r'][m])['sharpe'] for r in rr])) for period,m in e.mask.items()}
for key,T in Ts.items():
 assert np.max(np.abs(T.sum(1)))<1e-10
 assert np.isfinite(T).all()
meta={'hypotheses_daily':8,'hypotheses_hourly':6,'hypotheses_total':14,'bootstrap':B,'family_control':'Daily maxT(8) and hourly maxT(6), Bonferroni two families; require all block lengths','phase_policy':'all 3 offsets; arithmetic mean of phase Sharpes, not Sharpe of averaged PnL','execution':'next open taker .045% + slip .03%, 2.7x equity exposure; price/equity drift; 2% band; missing-price exit at stale mark flagged and stressed','funding':'Not fully available; excluded. No candidate can be declared investable on this backtest alone.','dates':[str(e.outdates[0]),str(e.outdates[-1])],'days':{p:int(m.sum()) for p,m in e.mask.items()},'bn_status':bn.status.value_counts().to_dict(),'coverage_mean':{s:float(np.mean(c)) for s,c in coverage.items()},'score_corr_14d_reversal_mean':{s:float(np.nanmean(c)) for s,c in scorecorr.items()},'missing_exposure_max':{key:float(max(v['missing'].max() for v in rr)) for key,rr in runs.items()},'input_sha256':{Path(f).name:hashlib.sha256(open(f,'rb').read()).hexdigest() for f in e.FILES}}
json.dump(dict(meta=meta,summary=summary,bootstrap=boot,stress=stress),open(OUT/'results_daily.json','w'),indent=2)
pd.DataFrame(rows).to_csv(OUT/'metrics_daily.csv',index=False)
out={'date':e.outdates.astype(str)}
for key,rr in runs.items():
 for p,v in enumerate(rr):out[key+f'_phase{p}']=v['r']
pd.DataFrame(out).to_csv(OUT/'daily_returns.csv',index=False)
for key in ['baseline']+['blend_'+s for s in keys]:print(key,[round(summary[key][p]['sharpe'],3) for p in e.mask],boot['30'].get(key.replace('blend_',''),{}),flush=True)
