"""Fixed four hypotheses. No parameter/direction selection. Run: python test_candidates.py"""
import os,json,glob,hashlib,warnings
import numpy as np,pandas as pd
warnings.filterwarnings('ignore',category=RuntimeWarning)
OUT=os.path.dirname(os.path.abspath(__file__)); DAY=86400000
FILES=sorted(glob.glob('/tmp/hl-daily-full/*.json'))
raw={os.path.basename(f)[:-5]:json.load(open(f)) for f in FILES}
names=sorted(raw); times=sorted({int(x['t']) for d in raw.values() for x in d if float(x['c'])>0})
dates=pd.to_datetime(times,unit='ms',utc=True); ti={t:i for i,t in enumerate(times)}; n=len(times); k=len(names)
a={z:np.full((n,k),np.nan) for z in ['o','c','h','l','v','n']}
for j,name in enumerate(names):
 for x in raw[name]:
  if int(x['t']) not in ti or float(x['c'])<=0:continue
  i=ti[int(x['t'])]
  for z in a:a[z][i,j]=float(x.get(z,np.nan))
P=a['c']; Q=P*a['v']; R=P/np.roll(P,1,axis=0)-1;R[0]=np.nan
# Exactly excludes today from liquidity mean, missing observations excluded.
V=pd.DataFrame(Q).rolling(30,min_periods=5).mean().shift(1).values
SD=pd.DataFrame(R).rolling(20,min_periods=5).std(ddof=1).values
M=P/np.roll(P,14,axis=0)-1
shock=Q/V
ill=pd.DataFrame(np.abs(R)/np.where(Q>0,Q,np.nan)).rolling(30,min_periods=20).mean().values
# Downside share: holds total volatility approximately separate from downside asymmetry.
down=pd.DataFrame(np.minimum(R,0)**2).rolling(30,min_periods=20).sum().values
sq=pd.DataFrame(R**2).rolling(30,min_periods=20).sum().values
down=down/sq
clv=np.where(a['h']>a['l'],(2*P-a['h']-a['l'])/(a['h']-a['l']),0)
flow=pd.DataFrame(clv*Q).rolling(30,min_periods=20).sum().values/pd.DataFrame(Q).rolling(30,min_periods=20).sum().values
# Size conditional on volume: avg trade $ / its own previous 30d mean.
size=Q/np.where(a['n']>0,a['n'],np.nan)
large=pd.DataFrame(size).rolling(30,min_periods=20).mean().values
# Use cross-sectional average dollar trade size, not volume shock or price momentum.
scores={'illiquidity':ill,'downside_share':down,'close_pressure':flow,'trade_size':large}
slots=np.array([sum(c.encode())%3 for c in names])

def book(idx,s,cap):
 t=np.zeros(k); idx=idx[np.isfinite(s[idx])]
 if len(idx)<8:return t
 kk=min(max(1,int(np.floor(len(idx)*.2+.5))),cap,len(idx)//2)
 order=idx[np.lexsort((np.array(names)[idx],s[idx]))]
 t[order[:kk]]=-.5/kk;t[order[-kk:]]=.5/kk
 return t

def targets(cap=5):
 base=np.zeros((n,k)); books={z:np.zeros((n,k)) for z in scores}; univ=[]
 for i in range(32,n-1):
  ok=(V[i]>=5e6)&np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])
  idx=np.flatnonzero(ok);univ.append(len(idx))
  if len(idx)<8:continue
  t=sum(book(idx,s,cap) for s in [M[i]/np.maximum(SD[i],1e-9),-SD[i],shock[i]])/3
  if np.abs(t).sum()>1e-12:t/=np.abs(t).sum()
  base[i]=t
  for z,s in scores.items():books[z][i]=book(idx,s[i],cap)
 return base,books,univ

# Executions at next UTC open, after prior candle has closed. Positions drift in price and equity.
# Missing price: carry last observable mark; report exposures rather than silently dropping position.
OP=pd.DataFrame(a['o']).ffill().values
START=45; END=n-2; ix=np.arange(START,END); outdates=dates[ix+1]

def run(T,offset=0,lev=2.7,fee=.00075,band=.02,missing_penalty=0):
 w=np.zeros(k);rs=[];ts=[];missing=[];gross=[];nets=[]
 for i in ix:
  # w is notional / equity at previous opening.
  if i>START:
   rr=np.nan_to_num(OP[i+1]/OP[i]-1)
   gain=w@rr; equity_ratio=1+gain
   if equity_ratio<=0:raise ValueError('insolvency')
   w=w*(1+rr)/equity_ratio
  else:gain=0
  tgt=T[i]*lev
  slot=((times[i]//DAY+1+offset+slots)%3)==0
  valid=np.isfinite(a['o'][i+1])
  delta=tgt-w
  # Explicit hypothetical exit at stale mark, plus adverse penalty stress.
  # Not a claim that such a fill was available; unresolved delisting uncertainty.
  missing_exposure=np.abs(w[~valid]).sum()
  use=slot&valid&((np.abs(delta)>=band*np.abs(tgt)) | (tgt*w<=0))
  dw=np.where(use,delta,0);dw[~valid]=-w[~valid]
  turn=np.abs(dw).sum();cost=turn*fee+missing_exposure*missing_penalty
  # Accounting fees reduces equity and increases all remaining notional weights.
  w=(w+dw)/(1-cost)
  rs.append((1+gain)*(1-cost)-1);ts.append(turn)
  missing.append(missing_exposure);gross.append(np.abs(w).sum());nets.append(w.sum())
 # Terminal liquidation at final observed open; all books treated consistently.
 terminal=np.abs(w).sum();rs[-1]=(1+rs[-1])*(1-terminal*fee)-1;ts[-1]+=terminal
 return {'r':np.array(rs),'turn':np.array(ts),'missing':np.array(missing),'gross':np.array(gross),'net':np.array(nets)}

def stat(r):
 eq=np.r_[1,np.cumprod(1+r)]; dd=np.min(eq/np.maximum.accumulate(eq)-1)
 return dict(sharpe=float(r.mean()/r.std(ddof=1)*np.sqrt(365)),annual_arithmetic=float(r.mean()*365),cagr=float(eq[-1]**(365/len(r))-1),max_drawdown=float(dd))
mask={'full':np.ones(len(ix),bool),'2023-24':outdates.year<=2024,'2025-26':outdates.year>=2025}
base,books,univ=targets()
Ts={'baseline':base}
for z,t in books.items():
 Ts[z]=t
 mix=.8*base+.2*t
 # Same target gross budget; netting happens BEFORE costs, in one account.
 gross=np.abs(mix).sum(axis=1);mix=np.divide(mix,gross[:,None],out=np.zeros_like(mix),where=gross[:,None]>0)
 Ts['blend_'+z]=mix
runs={z:[run(t,o) for o in range(3)] for z,t in Ts.items()}
summary={}; csv=[]
for z,rr in runs.items():
 summary[z]={}
 for period,m in mask.items():
  ss=[stat(v['r'][m]) for v in rr]
  q={key:float(np.mean([s[key] for s in ss])) for key in ss[0]}
  q['phase_sharpes']=[s['sharpe'] for s in ss]
  q['daily_oneway_turnover_equity']=float(np.mean([v['turn'][m].mean() for v in rr]))
  q['annual_cost']=q['daily_oneway_turnover_equity']*.00075*365
  q['correlation_baseline']=float(np.mean([np.corrcoef(rr[o]['r'][m],runs['baseline'][o]['r'][m])[0,1] for o in range(3)]))
  summary[z][period]=q;csv.append(dict(strategy=z,period=period,**q))
# Basic implementation checks: long/short targets, gross budget, return accounting.
for z,T in Ts.items():
 assert np.max(np.abs(T.sum(axis=1)))<1e-10, z
 g=np.abs(T).sum(axis=1)
 assert np.all((g<1e-10)|(np.abs(g-1)<1e-10)), z
for z,rr in runs.items():
 for v in rr:
  assert np.isfinite(v['r']).all() and (v['r']>-1).all(), z
  assert np.all(v['turn']>=0), z
assert np.all(np.diff(times)==DAY), 'daily timeline gaps'
# Paired circular moving block bootstrap: same day indices for all phases and hypotheses.
# Family of 4 blended improvements. Max studentized mean differential, centered under null.
rng=np.random.default_rng(20261008); B=3000
b=np.array([x['r'] for x in runs['baseline']]); mixes=[z for z in runs if z.startswith('blend_')]
x=np.array([[v['r'] for v in runs[z]] for z in mixes]); diff=(x-b).mean(axis=1)
boot={}
for L in [15,30,60]:
 bs=[];means=[]
 for rep in range(B):
  starts=rng.integers(len(ix),size=int(np.ceil(len(ix)/L)))
  inds=((starts[:,None]+np.arange(L))%len(ix)).ravel()[:len(ix)]
  rb=b[:,inds];rx=x[:,:,inds]
  sb=rb.mean(axis=1)/rb.std(axis=1,ddof=1)*np.sqrt(365)
  sx=rx.mean(axis=2)/rx.std(axis=2,ddof=1)*np.sqrt(365)
  bs.append((sx-sb).mean(axis=1));means.append(diff[:,inds].mean(axis=1))
 bs=np.array(bs);means=np.array(means);se=means.std(axis=0,ddof=1)
 observed=np.array([summary[z]['full']['sharpe']-summary['baseline']['full']['sharpe'] for z in mixes])
 se=bs.std(axis=0,ddof=1);obs=observed/se;null=(bs-observed)/se
 maxnull=null.max(axis=1)
 boot[str(L)]={z:{'delta_sharpe_ci95':np.quantile(bs[:,j],[.025,.975]).tolist(),'delta_mean_annual':float(diff[j].mean()*365),'p_fwer_maxT':float((1+np.sum(maxnull>=obs[j]))/(B+1))} for j,z in enumerate(mixes)}
# Slippage stress (additional 3bps), exact same hypotheses. Also unit-gross diagnostic.
stress={}
for z in Ts:
 stress[z]={}
 for label,lev,fee in [('one_x',1,.00075),('slip_6bp',2.7,.00105),('missing_5pct',2.7,.00075)]:
  rr=[run(Ts[z],o,lev=lev,fee=fee,missing_penalty=.05 if label=='missing_5pct' else 0) for o in range(3)]
  stress[z][label]={p:{'sharpe':float(np.mean([stat(v['r'][m])['sharpe'] for v in rr]))} for p,m in mask.items()}
# Cap=8 reproduction is diagnostic only, not candidate selection.
b8,_,_=targets(8);rr8=[run(b8,o,lev=1) for o in range(3)]
meta={'daily_files':len(FILES),'daily_start':str(dates[0]),'daily_end':str(dates[-1]),'evaluation_start':str(outdates[0]),'evaluation_end':str(outdates[-1]),'days_by_period':{z:int(m.sum()) for z,m in mask.items()},'universe_mean':float(np.mean(univ)),'universe_median':float(np.median(univ)),'cap8_baseline_one_x_sharpe':float(np.mean([stat(v['r'])['sharpe'] for v in rr8])),'missing_held_max_equity':{z:float(max(v['missing'].max() for v in rr)) for z,rr in runs.items()},'missing_exit_events':{z:[int(np.sum(v['missing']>0)) for v in rr] for z,rr in runs.items()},'baseline_gross_mean':float(np.mean([v['gross'].mean() for v in runs['baseline']])), 'baseline_net_abs_mean':float(np.mean([np.abs(v['net']).mean() for v in runs['baseline']])), 'manifest':{os.path.basename(f):hashlib.sha256(open(f,'rb').read()).hexdigest() for f in FILES},'funding':'NOT included: full historical data unavailable','minimum_order':'Large-account approximation: ignores $10 order minimum and size rounding; uses 2% relative band','selection':'4 fixed directions, 30d windows, 20% blends, cap5, all 3 offsets; no grid search'}
json.dump(dict(meta=meta,summary=summary,bootstrap=boot,stress=stress),open(OUT+'/results.json','w'),indent=2)
pd.DataFrame(csv).to_csv(OUT+'/metrics.csv',index=False)
rows={'date':outdates.astype(str)}
for z,rr in runs.items():
 for o,v in enumerate(rr):rows[z+'_phase'+str(o)]=v['r']
pd.DataFrame(rows).to_csv(OUT+'/daily_returns.csv',index=False)
print(json.dumps({k:v for k,v in meta.items() if k!='manifest'},indent=2))
for row in csv:print(row['strategy'],row['period'],'S',round(row['sharpe'],3),'ann',round(row['annual_arithmetic']*100,2),'DD',round(row['max_drawdown']*100,2),'TO',round(row['daily_oneway_turnover_equity']*100,2),'corr',round(row['correlation_baseline'],3))
print('BOOT',json.dumps(boot['30'],indent=2))
