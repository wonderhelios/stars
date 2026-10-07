"""Mechanism diagnostics only: no independent hypothesis acceptance or naive t tests."""
import engine as e
import numpy as np,pandas as pd,json
from pathlib import Path
OUT=Path(__file__).resolve().parent
bn=pd.read_csv(OUT/'binance_daily_metrics.csv');bn['signal_date']=pd.to_datetime(bn.day,utc=True)+pd.Timedelta(days=1)
X=bn.pivot(index='signal_date',columns='coin',values='sum_open_interest').reindex(index=e.dates,columns=e.names).apply(pd.to_numeric,errors='coerce').values
X=np.where(X>0,X,np.nan);doi=np.log(X/np.roll(X,1,axis=0));doi[0]=np.nan
amp=(e.a['h']-e.a['l'])/e.P;capacity=e.Q/np.maximum(amp,.001)
dcap=np.log(capacity/np.roll(capacity,1,axis=0))
cor=[]
for j,c in enumerate(e.names):
 a=dcap[:-1,j];b=doi[1:,j];m=np.isfinite(a)&np.isfinite(b)
 if m.sum()>=100:cor.append({'coin':c,'samples':int(m.sum()),'correlation_capacity_change_bn_oi_change':float(np.corrcoef(a[m],b[m])[0,1])})
# Next open-to-open returns after completed daily signal, market-relative.
fwd=np.roll(e.OP,-2,axis=0)/np.roll(e.OP,-1,axis=0)-1
eligible=(e.V>=5e6)&np.isfinite(e.P)&np.isfinite(e.SD)
fwd[-2:]=np.nan
market=np.nanmean(np.where(eligible,fwd,np.nan),axis=1);adj=fwd-market[:,None]
rows=[]
for period,m in [('2023-24',e.dates.year<=2024),('2025-26',e.dates.year>=2025)]:
 for kind,price,vv in [('volume',e.R,e.shock-1),('actual_bn_oi',np.roll(e.R,1,axis=0),doi)]:
  for expanding in [False,True]:
   good=eligible&m[:,None]&(price>0)&((vv>0) if expanding else (vv<=0))&np.isfinite(adj)&np.isfinite(vv)
   vals=np.nanmean(np.where(good,adj,np.nan),axis=1);d=vals[np.isfinite(vals)]
   rows.append({'period':period,'variable':kind,'expanding':expanding,'coin_events_overlapping':int(good.sum()),'active_calendar_days':len(d),'mean_day_equal_weight_market_relative_next_return':float(np.mean(d)) if len(d) else None,'net_spread_cost_if_long_vs_equal_market':.003,'inference':'Descriptive only; overlapping coin events NOT independent samples; no t test or significance claim'})
json.dump({'proxy_oi_correlations':cor,'conditional_upprice_groups':rows},open(OUT/'diagnostics.json','w'),indent=2)
print('median proxy/OI corr',np.median([v['correlation_capacity_change_bn_oi_change'] for v in cor]),flush=True)
for r in rows:print(r,flush=True)
