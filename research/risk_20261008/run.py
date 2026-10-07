"""Fixed family, portfolio risk controls; execute python run.py. No live mutations."""
from pathlib import Path
import json,hashlib,glob,warnings
import numpy as np,pandas as pd
warnings.filterwarnings('ignore',category=RuntimeWarning)
OUT=Path(__file__).resolve().parent
source=OUT/'panel.py'
# Reuse audited panel loader and exact three-factor target builder, not its experiments.
exec(source.read_text().split('# Executions at next UTC open')[0])
OUT=Path(__file__).resolve().parent
base,_,univ=targets()
OP=pd.DataFrame(a['o']).ffill().values
START=45;END=n-2;ix=np.arange(START,END);N=len(ix);outdates=dates[ix+1]
mask={'full':np.ones(N,bool),'2023-24':outdates.year<=2024,'2025-26':outdates.year>=2025}

def stat(r):
 e=np.r_[1,np.cumprod(1+r)]
 return dict(sharpe=float(r.mean()/r.std(ddof=1)*np.sqrt(365)),annual_return=float(r.mean()*365),annual_vol=float(r.std(ddof=1)*np.sqrt(365)),cagr=float(e[-1]**(365/len(r))-1),mdd=float(np.min(e/np.maximum.accumulate(e)-1)))

# Risk changes execute ALL names immediately; ordinary signals remain staggered.
# Fees are paid in account equity; the risk book is physically traded, not scaled PnL.
def run(offset=0,scale=None,hedge=None,lev=2.7,fee=.00075,missing_penalty=0,dd=None):
 w=np.zeros(k);rrs=[];turns=[];gross=[];net=[];missing=[];weights=[];prevscale=1.;equity=1.;peak=1.;on=False
 for j,i in enumerate(ix):
  rr=np.nan_to_num(OP[i+1]/OP[i]-1) if j else np.zeros(k)
  gain=w@rr;assert 1+gain>0
  w=w*(1+rr)/(1+gain)
  equity*=1+gain;peak=max(peak,equity)
  s=1. if scale is None else scale[j]
  if dd is not None:
   drawdown=1-equity/peak
   if drawdown>=dd["threshold"]:on=True
   if drawdown<=dd["threshold"]/2:on=False
   s=dd["floor"] if on else 1.
  # Portfolio emergency control: resize whole existing book before today's signal updates.
  adjusted=w*s/prevscale if s!=prevscale else w.copy()
  target=base[i]*lev*s
  slot=((times[i]//DAY+1+offset+slots)%3)==0
  delta=target-adjusted
  use=slot&((np.abs(delta)>=.02*np.abs(target))|(target*adjusted<=0))
  adjusted=np.where(use,target,adjusted)
  if hedge is not None:
   # Hedge book separate for accounting, then net into one account. Adjust total risk daily.
   # Rebuild using stored unhedged physical baseline positions: no hidden hedge accumulation.
   adjusted=hedge[j]
  valid=np.isfinite(a['o'][i+1]);miss=np.abs(w[~valid]).sum()
  adjusted[~valid]=0
  turn=np.abs(adjusted-w).sum();cost=turn*fee+miss*missing_penalty
  assert cost<1
  equity*=1-cost
  w=adjusted/(1-cost)
  rrs.append((1+gain)*(1-cost)-1);turns.append(turn);gross.append(np.abs(w).sum());net.append(w.sum());missing.append(miss);weights.append(w.copy());prevscale=s
 terminal=np.abs(w).sum();rrs[-1]=(1+rrs[-1])*(1-terminal*fee)-1;turns[-1]+=terminal
 return dict(r=np.array(rrs),turn=np.array(turns),gross=np.array(gross),net=np.array(net),missing=np.array(missing),weights=np.array(weights))

baselines=[run(o) for o in range(3)]
# Observable mean pairwise correlation, complete 20/60 day history, current PIT liquid set.
corr={}
for win in [20,60]:
 v=np.full(N,np.nan)
 for j,i in enumerate(ix):
  idx=np.flatnonzero((V[i]>=5e6)&np.all(np.isfinite(R[i-win+1:i+1]),axis=0))
  if len(idx)<8:continue
  z=R[i-win+1:i+1,idx];sd=z.std(axis=0,ddof=1);z=z[:,sd>1e-9];sd=sd[sd>1e-9]
  z=(z-z.mean(axis=0))/sd
  q=z.shape[1];v[j]=((z.sum(axis=1)**2).sum()/(win-1)-q)/(q*(q-1))
 corr[win]=v
# Fixed high-beta hedge: rolling 60 daily regression beta to equal-weight liquid market.
high=np.zeros((N,k))
for j,i in enumerate(ix):
 if i<60:continue
 idx=np.flatnonzero((V[i]>=5e6)&np.all(np.isfinite(R[i-59:i+1]),axis=0))
 if len(idx)<8:continue
 z=R[i-59:i+1,idx];market=z.mean(axis=1);mc=market-market.mean();b=((z-z.mean(axis=0))*mc[:,None]).sum(axis=0)/(mc@mc)
 order=idx[np.lexsort((np.array(names)[idx],b))];high[j,order[-5:]]=-1/5

configs=[]
for threshold in [.10,.20,.30]:
 for floor in [1/3,2/3,.85]:configs.append(dict(name=f'dd_{threshold}_{floor:.3f}',kind='dd',threshold=threshold,floor=floor))
for win in [20,60]:
 for floor in [1/3,2/3]:configs.append(dict(name=f'corr_{win}_{floor:.3f}',kind='corr',win=win,threshold=.65,floor=floor))
for win in [10,20]:
 for trend in [False,True]:
  for floor in [1/3,2/3]:configs.append(dict(name=f'vol_{win}_{trend}_{floor:.3f}',kind='vol',win=win,trend=trend,floor=floor))
for budget in [.05,.10,.20]:configs.append(dict(name=f'tail_beta_{budget}',kind='tail',budget=budget))
for threshold in [.10,.20,.30]:
 for floor in [1/3,2/3,.85]:configs.append(dict(name=f"actual_dd_{threshold}_{floor:.3f}",kind="actual_dd",threshold=threshold,floor=floor))
K=len(configs)
assert K==33
json.dump(configs,open(OUT/'candidates.json','w'),indent=2)

def control(c,o):
 # At execution i+1 we know prior opening-to-opening PnL through i+1, including current mark.
 # baselines.r[j] contains today's close-execution costs, hence use only r[:j] (extra conservative lag).
 b=baselines[o];past=pd.Series(b['r']).shift(1);scale=np.ones(N)
 if c['kind']=='dd':
  eq=np.r_[1,np.cumprod(1+b['r'][:-1])];dd=1-eq/np.maximum.accumulate(eq);on=False
  for j,d in enumerate(dd):
   if d>=c['threshold']:on=True
   if d<=c['threshold']/2:on=False
   scale[j]=c['floor'] if on else 1
 elif c['kind']=='corr':scale=np.where(corr[c['win']]>.65,c['floor'],1)
 elif c['kind']=='vol':
  fast=past.rolling(c['win'],min_periods=c['win']).std(ddof=1).values
  slow=past.rolling(60,min_periods=60).std(ddof=1).values
  forecast=fast*np.clip(fast/slow,.5,2) if c['trend'] else fast
  # Entirely causal target: expanding median of previously realized 60d portfolio vol.
  target=pd.Series(slow).expanding(min_periods=20).median().shift(1).values
  scale=np.where(np.isfinite(target/forecast),np.clip(target/forecast,c['floor'],1),1)
 if c['kind']=='tail':
  # Fixed total gross budget, short high-beta replacing a fraction of existing core.
  h=(1-c['budget'])*b['weights']+2.7*c['budget']*high
  return scale,h
 return scale,None

runs={'baseline':baselines};controls={};rows=[];scale_store={}
for c in configs:
 name=c['name'];rr=[];cc=[];ss=[]
 for o in range(3):
  s,h=control(c,o);v=run(o,s,h,dd=c if c["kind"]=="actual_dd" else None);rr.append(v);ss.append(s)
  # Match realized mean gross, not merely mean requested multiplier. Solve causal constant book.
  lev=2.7*v['gross'].mean()/baselines[o]['gross'].mean()
  for _ in range(4):
   cv=run(o,lev=lev);lev*=v['gross'].mean()/cv['gross'].mean()
  cv=run(o,lev=lev);cc.append(cv)
 runs[name]=rr;controls[name]=cc;scale_store[name]=ss
 print('RUN',name,flush=True)

def summarize(rr,compare=None):
 out={}
 for period,m in mask.items():
  ss=[stat(x['r'][m]) for x in rr];q={key:float(np.mean([s[key] for s in ss])) for key in ss[0]}
  q.update(phase_sharpes=[s['sharpe'] for s in ss],phase_sharpe_sd=float(np.std([s['sharpe'] for s in ss],ddof=1)),turnover=float(np.mean([x['turn'][m].mean() for x in rr])),annual_cost=float(np.mean([x['turn'][m].mean() for x in rr]))*.00075*365,mean_gross=float(np.mean([x['gross'][m].mean() for x in rr])),mean_abs_net=float(np.mean([np.abs(x['net'][m]).mean() for x in rr])),correlation_baseline=float(np.mean([np.corrcoef(x['r'][m],baselines[o]['r'][m])[0,1] for o,x in enumerate(rr)])))
  if compare:q['delta_vs_matched']=q['sharpe']-summarize(compare)[period]['sharpe']
  out[period]=q
 return out
summary={z:summarize(rr,controls.get(z)) for z,rr in runs.items()};matched={z:summarize(rr) for z,rr in controls.items()}
for z,ss in summary.items():
 for p,q in ss.items():rows.append(dict(candidate=z,period=p,**q))
pd.DataFrame(rows).to_csv(OUT/'metrics.csv',index=False)
np.savez_compressed(OUT/'paths.npz',dates=outdates.astype(str),**{z:np.array([v['r'] for v in rr]) for z,rr in runs.items()},**{'matched_'+z:np.array([v['r'] for v in rr]) for z,rr in controls.items()})
# Paired circular moving-block bootstrap of Sharpe DIFFERENCES, 24 candidates x 2 comparators.
# Joint maxT over 66 comparisons includes baseline and equal-exposure control in same family.
X=np.array([[v['r'] for v in runs[c['name']]] for c in configs]);C=np.array([[v['r'] for v in controls[c['name']]] for c in configs]);B0=np.array([v['r'] for v in baselines])
def sharpe(x):return x.mean(axis=-1)/x.std(axis=-1,ddof=1)*np.sqrt(365)
obs=np.stack([(sharpe(X)-sharpe(B0)).mean(axis=1),(sharpe(X)-sharpe(C)).mean(axis=1)],axis=1)
boot={};rng=np.random.default_rng(20261008);REPS=2500
for L in [15,30,60]:
 sims=np.empty((REPS,K,2))
 for rep in range(REPS):
  starts=rng.integers(N,size=int(np.ceil(N/L)));inds=((starts[:,None]+np.arange(L))%N).ravel()[:N]
  sx=sharpe(X[:,:,inds]);sb=sharpe(B0[:,inds]);sc=sharpe(C[:,:,inds])
  sims[rep,:,0]=(sx-sb).mean(axis=1);sims[rep,:,1]=(sx-sc).mean(axis=1)
 se=sims.std(axis=0,ddof=1);null=(sims-obs)/se;maxnull=null.reshape(REPS,-1).max(axis=1)
 boot[str(L)]={c['name']:{kind:dict(delta=float(obs[j,t]),ci95=np.quantile(sims[:,j,t],[.025,.975]).tolist(),p_fwer=float((1+(maxnull>=obs[j,t]/se[j,t]).sum())/(REPS+1))) for t,kind in enumerate(['vs_baseline','vs_matched'])} for j,c in enumerate(configs)}
 print('BOOT',L,flush=True)
# Execution stress: no hypothetical maker fills. Maker cost is lower bound only.
stress={}
for c in configs:
 z=c['name'];stress[z]={}
 for label,fee,pen in [('maker_lower_bound',.00045,0),('slip_6bp',.00105,0),('missing_5pct',.00075,.05)]:
  vv=[run(o,*control(c,o),fee=fee,missing_penalty=pen,dd=c if c["kind"]=="actual_dd" else None) for o in range(3)]
  stress[z][label]={p:float(np.mean([stat(v['r'][m])['sharpe'] for v in vv])) for p,m in mask.items()}
meta=dict(candidates=K,comparisons=2*K,bootstrap_reps=REPS,periods={p:int(m.sum()) for p,m in mask.items()},start=str(outdates[0]),end=str(outdates[-1]),files=len(FILES),manifest={Path(f).name:hashlib.sha256(Path(f).read_bytes()).hexdigest() for f in FILES},baseline_source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),funding='Excluded: incomplete full-history coverage; disqualifies any live-ready claim',missing_events=[int((v['missing']>0).sum()) for v in baselines],maximum_missing_exposure=[float(v['missing'].max()) for v in baselines],account='Large-account approximation; no minimum order/rounding/liquidation model',selection='Exploratory: 24 fixed configs evaluated first; 9 actual-account DD configs added after initial results using the same threshold grid to complete scope. All 33 rerun and jointly corrected. No post-result parameter tuning.',vix='No local VIX candles; inspect api_evidence.json',drawdown='Both actual account equity and shadow baseline triggers; half-threshold recovery. No restart/reset/cooldown tuning.')
qualified=[]
for c in configs:
 z=c['name'];sub=all(summary[z][p]['sharpe']>summary['baseline'][p]['sharpe'] and summary[z][p]['delta_vs_matched']>0 for p in ['2023-24','2025-26'])
 sig=all(boot[str(L)][z][kind]['p_fwer']<.05 for L in [15,30,60] for kind in ['vs_baseline','vs_matched'])
 if sub and sig:qualified.append(z)
json.dump(dict(meta=meta,summary=summary,matched=matched,bootstrap=boot,stress=stress,statistical_survivors=qualified,live_qualified=[]),open(OUT/'results.json','w'),indent=2)
print('SURVIVORS',qualified)
print('BASELINE',summary['baseline'])
