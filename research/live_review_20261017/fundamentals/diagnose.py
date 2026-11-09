"""Signal diagnosis, not strategy selection or deployable return backtest."""
import os,sys,json
from pathlib import Path
os.environ['INCLUDE_RECENT']='1'
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
import backtest as b
import numpy as np
import pandas as pd
P=Path(__file__).resolve().parent
spec={'purpose':'First principles predictive ranking and return concentration diagnosis; no strategy optimization','factors':['risk_adjusted_momentum','volume_shock'],'horizons_days':[1,2,5],'periods':['2023-24','2025','2026'],'execution':'signal complete daily close; entry next open; exit horizon close; gross returns only','universe':'as-of lagged30day volume>=10m; same production history/SD requirements; no HIP3','limitations':['historical universe survivorship and historical metadata unresolved','missing future marks explicitly counted; complete-case results conditional on coverage','multi-day observations overlap; 30-day time-block confidence intervals descriptive, no significance or promotion claims','all historical periods previously inspected; no untouched OOS','quintiles diagnostic equal-count portfolios, not capped production books'],'family':'18 diagnostic cells; no candidate selected'}
(P/'spec.json').write_text(json.dumps(spec,indent=2))
rows=[]
for i in range(32,b.N-1):
 idx=np.flatnonzero((b.V[i]>=1e7)&np.isfinite(b.C[i])&np.isfinite(b.C[i-14])&np.isfinite(b.SD[i])&b.NOTCOL)
 if len(idx)<10:continue
 year=pd.Timestamp(b.ts[i],unit='ms',tz='UTC').year;period='2023-24' if year<=2024 else str(year)
 for h in [1,2,5]:
  if i+h>=b.N:continue
  # Do not straddle year split.
  if pd.Timestamp(b.ts[i+h],unit='ms',tz='UTC').year!=year:continue
  y=b.C[i+h,idx]/b.O[i+1,idx]-1
  valid=np.isfinite(y)&np.isfinite(b.O[i+1,idx])&(b.O[i+1,idx]>0)
  for name,sc in [('momentum',b.M[i]/np.maximum(b.SD[i],1e-9)),('volume',b.Q[i]/b.V[i])]:
   # Assign bins before observing missing outcomes. Never replace missing selected names.
   order=np.lexsort((b.names[idx],sc[idx]));bins=np.array_split(order,5)
   qs=[float(np.mean(y[z])) if np.isfinite(y[z]).all() else None for z in bins]
   ic=float(pd.Series(sc[idx][valid]).rank().corr(pd.Series(y[valid]).rank())) if valid.sum()>3 else None
   rows.append({'t':b.ts[i],'period':period,'h':h,'factor':name,'ic':ic,'q':qs,'missing':int((~valid).sum()),'n':len(idx)})
def ci(x):
 x=np.asarray(x);rng=np.random.default_rng(20261017);n=len(x)
 v=[]
 for _ in range(999):
  z=(rng.integers(n,size=(int(np.ceil(n/30)),1))+np.arange(30))%n
  v.append(np.mean(x[z.ravel()[:n]]))
 return [float(v) for v in np.quantile(v,[.025,.975])]
out=[]
for period in ['2023-24','2025','2026']:
 for f in ['momentum','volume']:
  for h in [1,2,5]:
   rr=[r for r in rows if (r['period'],r['factor'],r['h'])==(period,f,h)]
   clean=[r for r in rr if all(x is not None for x in r['q'])];q=np.array([r['q'] for r in clean]);ic=np.array([r['ic'] for r in rr if r['ic'] is not None]);spread=q[:,-1]-q[:,0]
   out.append({'period':period,'factor':f,'h':h,'days':len(rr),'complete_days':len(clean),'missing_observations':sum(r['missing'] for r in rr),'mean_ic':float(ic.mean()),'ic_ci30':ci(ic),'quintile_mean_bps':(q.mean(0)*1e4).tolist(),'top_bottom_spread_bps':float(spread.mean()*1e4),'spread_ci30_bps':(np.array(ci(spread))*1e4).tolist(),'positive_spread_days':float((spread>0).mean())})
# Production ledger concentration: independent cold starts to avoid inherited delisted positions.
def ix(s):return int(np.searchsorted(b.ts,pd.Timestamp(s,tz='UTC').value//1000000))
tails={}
for name,lo,hi in [('2023-24',33,ix('2025-01-01')),('2025',ix('2025-01-14'),ix('2026-01-01')),('2026',ix('2026-01-01'),b.N)]:
 a=b.simulate(start_idx=lo,end_idx=hi);r=a['r'];rank=np.argsort(r);full=b.metrics(a);summ=float(r.sum());top=float(r[rank[-5:]].sum())
 tails[name]={'metrics':full,'missing':a['missing_coins'],'sum_daily_returns':summ,'best5_sum':top,'best5_share_sum':top/summ if summ else None,'sum_without_best5':summ-top,'worst5_sum':float(r[rank[:5]].sum()),'positive_days':float((r>0).mean()),'median_daily_bps':float(np.median(r)*1e4),'annual_cost_approx':float(a['turn'].mean()*.00075*365),'best5_dates':[str(pd.Timestamp(b.ts[lo+int(j)],unit='ms',tz='UTC').date()) for j in rank[-5:]]}
(P/'results.json').write_text(json.dumps({'diagnostics':out,'concentration':tails},indent=2))
(P/'daily_observations.json').write_text(json.dumps(rows))
for x in out:print(x['period'],x['factor'],x['h'],'IC',round(x['mean_ic'],4),'Q',np.round(x['quintile_mean_bps'],1),'spread',round(x['top_bottom_spread_bps'],1),'missing',x['missing_observations'])
print('TAILS',json.dumps(tails))
