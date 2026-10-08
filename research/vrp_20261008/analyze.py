"""Descriptive VRP tests; NO option P&L backtest. Seed and test family fixed."""
import json,glob,math
from pathlib import Path
import numpy as np,pandas as pd
O=Path(__file__).resolve().parent; rng=np.random.default_rng(20261008)
DAY=86400000;server=json.load(open(O/'raw/metadata.json'))['server_ms'];end=pd.to_datetime(server,unit='ms',utc=True).floor('D')
frames={};results={};tests=[]
for coin in ['BTC','ETH']:
 d=pd.DataFrame(json.load(open(O/f'{coin}_dvol.json')),columns=['t','o','h','l','c']);d.index=pd.to_datetime(d.t,unit='ms',utc=True)+pd.Timedelta(days=1)
 # Both DVOL and HL daily CLOSE become observable at next midnight.
 iv=(d.c/100).loc[d.index<=end];iv=iv[~iv.index.duplicated(keep='last')]
 p=pd.DataFrame(json.load(open(f'/tmp/hl-daily-full/{coin}.json')));p.index=pd.to_datetime(p.t,unit='ms',utc=True)+pd.Timedelta(days=1)
 close=p.c.astype(float).loc[p.index<=end];close=close[~close.index.duplicated(keep='last')].asfreq('D')
 r=np.log(close/close.shift());rv2=r.pow(2).rolling(30,min_periods=30).sum()*365/30
 fv2=r.pow(2).rolling(30,min_periods=30).sum().shift(-30)*365/30
 f=pd.DataFrame({'iv':iv,'r':r,'pastvar':rv2,'futurevar':fv2});f['vol_gap']=f.iv-np.sqrt(f.futurevar);f['var_gap']=f.iv**2-f.futurevar
 f['past_gap']=f.iv**2-f.pastvar;f['future_gap']=f.past_gap.shift(-30)
 frames[coin]=f
 for typ in ['vol_gap','var_gap']:tests.append((coin+'_'+typ,'mean',f[[typ]].dropna().iloc[:,0]))
 # Partial covariance: IV-RV gap predicts future RV beyond lagged RV.
 for label,yname in [('predict_futurevar','futurevar'),('mean_reversion','future_gap')]:
  z=f[['past_gap','pastvar',yname]].dropna();control=np.c_[np.ones(len(z)),z.pastvar.values]
  x=z.past_gap.values-control@np.linalg.lstsq(control,z.past_gap.values,rcond=None)[0]
  y=z[yname].values if label=='predict_futurevar' else (z[yname]-z.past_gap).values
  y=y-control@np.linalg.lstsq(control,y,rcond=None)[0]
  # positive sign for prediction, negative sign for reversion, pre-specified
  sign=1 if label=='predict_futurevar' else -1
  tests.append((coin+'_'+label,'moment',pd.Series(sign*x*y,index=z.index)))
  results[coin+'_'+label]={'partial_slope':float(np.dot(x,y)/np.dot(x,x)),'n':len(z),'warning':'In-sample association; no out-of-sample strategy evidence. Reversion includes mechanical RV rolling-window effects.'}
 # Daily equal-weight rolling variance-cohort accrual: accounting diagnostic only.
 f['daily_variance_proxy']=f.iv.pow(2).shift(1).rolling(30,min_periods=30).mean()/365-f.r.pow(2)
 f.to_csv(O/f'{coin}_aligned.csv',index_label='timestamp_observable')
 def summary(z):
  phases=[]
  for phase in range(30):
   a=z.loc[((z.index.view('i8')//(DAY*1000000))%30)==phase]
   phases.append({'phase':phase,'n':len(a),'vol_gap_pp':float(a.vol_gap.mean()*100),'var_gap':float(a.var_gap.mean())})
  return {'n':len(z),'start':str(z.index.min()),'last':str(z.index.max()),'iv_pct':float(z.iv.mean()*100),'future_rv_pct':float(np.sqrt(z.futurevar).mean()*100),'vol_gap_pp':float(z.vol_gap.mean()*100),'variance_gap':float(z.var_gap.mean()),'positive_vol_gap_fraction':float((z.vol_gap>0).mean()),'positive_variance_gap_fraction':float((z.var_gap>0).mean()),'phase_vol_gap_mean_pp':float(np.mean([a['vol_gap_pp'] for a in phases])),'phase_vol_gap_sd_pp':float(np.std([a['vol_gap_pp'] for a in phases],ddof=1)),'phase_vol_gap_minmax_pp':[float(min(a['vol_gap_pp'] for a in phases)),float(max(a['vol_gap_pp'] for a in phases))],'phases':phases}
 z=f.dropna(subset=['vol_gap','var_gap']);results[coin]={period:summary(a) for period,a in [('full',z),('2023-24',z[(z.index+pd.Timedelta(days=30)).year<=2024]),('2025-26',z[z.index.year>=2025])]}
# Joint circular moving-block bootstrap, common dates and synchronized resampling.
A=pd.concat({name:s for name,_,s in tests},axis=1).dropna();X=A.values;n=len(X);B=4000
bootout={}
for L in [60,90,120]:
 means=np.empty((B,X.shape[1]));nb=math.ceil(n/L)
 for b in range(B):
  starts=rng.integers(0,n,nb);ix=((starts[:,None]+np.arange(L))%n).ravel()[:n];means[b]=X[ix].mean(axis=0)
 se=means.std(axis=0,ddof=1);obs=X.mean(axis=0)/se
 null=(means-X.mean(axis=0))/se;maxT=null.max(axis=1)
 bootout[str(L)]={name:{'n_common':n,'statistic':float(obs[j]),'p_fwer':float((1+(maxT>=obs[j]).sum())/(B+1)),'mean':float(X[:,j].mean()),'block_percentile_95_ci':np.quantile(means[:,j],[.025,.975]).tolist()} for j,name in enumerate(A.columns)}
results['multiple_testing']={'family_size':len(tests),'replicates':B,'seed':20261008,'method':'one-sided centered joint moving-block maxT; sample-SE from block bootstrap; all 8 moments jointly, shared dates; sensitivity 60/90/120 days','tests':bootout}
# Baseline exact implementation diagnostic, all 3 phases; never substitute forward proxy for real P&L.
b=pd.read_csv(O.parent/'liq_oi_20261008/baseline_returns.csv',index_col=0,parse_dates=True);b.index=pd.to_datetime(b.index,utc=True)
corr={}
for coin,f in frames.items():
 corr[coin]={}
 for period,mask in [('full',lambda idx:np.ones(len(idx),bool)),('2023-24',lambda idx:idx.year<=2024),('2025-26',lambda idx:idx.year>=2025)]:
  entries=[]
  for col in b:
   # baseline date denotes beginning of day whose realized return ends next midnight.
   bs=b[col].copy();bs.index=bs.index+pd.Timedelta(days=1)
   v=pd.concat([f.daily_variance_proxy.rename('proxy'),bs.rename('baseline')],axis=1).dropna();v=v.loc[mask(v.index)]
   rho=v.corr().iloc[0,1];entries.append({'baseline_phase':col,'n':len(v),'daily_proxy_correlation':float(rho),'baseline_sharpe':float(v.baseline.mean()/v.baseline.std()*np.sqrt(365))})
  corr[coin][period]=entries
results['correlation_DIAGNOSTIC_ONLY']=corr
# Current snapshot ATM term structure; no historical prediction test.
chain={};costs={}
for coin in frames:
 inst=json.load(open(O/f'raw/{coin}_instruments.json'))['result'];summ=json.load(open(O/f'raw/{coin}_summary.json'))['result'];lookup={x['instrument_name']:x for x in summ};rows=[]
 for expiry in sorted({x['expiration_timestamp'] for x in inst}):
  days=(expiry-server)/DAY
  if days<=0:continue
  a=[x for x in inst if x['expiration_timestamp']==expiry and x['instrument_name'] in lookup]
  if not a:continue
  spot=lookup[a[0]['instrument_name']].get('underlying_price');a=sorted(a,key=lambda x:abs(x['strike']-spot))
  near=[x for x in a if x['strike']==a[0]['strike']];ivs=[lookup[x['instrument_name']].get('mark_iv') for x in near];ivs=[v for v in ivs if v is not None]
  rows.append({'days':days,'strike':a[0]['strike'],'atm_mark_iv_pct':float(np.mean(ivs)) if ivs else None})
 chain[coin]=rows
 books=[json.load(open(p))['result'] for p in glob.glob(str(O/f'raw/{coin}-*_book.json'))]
 cc=[]
 for x in books:
  bid=x.get('best_bid_price');ask=x.get('best_ask_price');mark=x.get('mark_price');vega=x.get('greeks',{}).get('vega');spot=x['index_price']
  if not bid or not ask:continue
  half=(ask-bid)/2;fee=min(.0003,.125*bid)
  # Vega is USD per 1 vol point. Inverse option price is BTC/ETH per unit.
  cc.append({'instrument':x['instrument_name'],'bid':bid,'ask':ask,'index_price':spot,'half_spread_underlying':half,'entry_fee_underlying':fee,'entry_cost_vol_points':(half+fee)*spot/vega if vega else None,'roundtrip_cost_vol_points':(ask-bid+fee+min(.0003,.125*ask))*spot/vega if vega else None,'bid_depth_first20':sum(v[1] for v in x['bids']),'ask_depth_first20':sum(v[1] for v in x['asks']),'snapshot_only':True})
 costs[coin]=cc
results['current_ATM_term_structure']=chain;results['current_ATM_cost_snapshot']=costs
json.dump(results,open(O/'results.json','w'),indent=2,allow_nan=False)
print(json.dumps({c:{p:{k:v for k,v in results[c][p].items() if k!='phases'} for p in results[c]} for c in frames},indent=2));print('FWER',json.dumps(bootout['90'],indent=2));print('COST',costs);print('CORR',corr)
