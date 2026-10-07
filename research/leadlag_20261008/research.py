"""Run python research.py. Fixed family; no tuning after results. Seed 20261008."""
import engine as e
import numpy as np,pandas as pd,json,glob,os,hashlib,warnings
from scipy.cluster.hierarchy import linkage,fcluster
from scipy.spatial.distance import squareform
warnings.filterwarnings('ignore',category=RuntimeWarning)
OUT=e.OUT; rng=np.random.default_rng(20261008)
base,_,univ=e.targets(); names=np.array(e.names); N=e.n; K=e.k
eligible=(e.V>=5e6)&np.isfinite(e.P)&np.isfinite(e.SD)&np.isfinite(e.M)
# Remove contemporary BTC component with causal trailing estimates; regress next residual on lagged BTC.
b=e.names.index('BTC'); btc=e.R[:,b]
C={}; diagnostics={}; pair_screen=[]
def roll(x,w,minp=None):return pd.DataFrame(x).rolling(w,min_periods=minp or w).mean().values
for win in [60,120]:
 x=np.broadcast_to(btc[:,None],e.R.shape)
 cov=roll(e.R*x,win)-roll(e.R,win)*roll(x,win)
 var=roll(x*x,win)-roll(x,win)**2
 beta=cov/var
 residual=e.R-beta*np.broadcast_to(btc[:,None],e.R.shape)
 for lag in [1,3]:
  past=pd.Series(btc).rolling(lag).sum().shift(1).values
  xx=np.broadcast_to(past[:,None],e.R.shape)
  cov2=roll(residual*xx,win)-roll(residual,win)*roll(xx,win)
  var2=roll(xx*xx,win)-roll(xx,win)**2
  pred=cov2/var2*np.broadcast_to(pd.Series(btc).rolling(lag).sum().values[:,None],e.R.shape)
  T=np.zeros_like(base)
  for i in range(max(32,win+lag),N-1):
   idx=np.flatnonzero(eligible[i]);idx=idx[idx!=b];T[i]=e.book(idx,pred[i],5)
  C[f'btc_daily_w{win}_lag{lag}']=T
# Daily causal correlation clusters. Every day refit: no monthly origin to select.
for win in [60,120]:
 groups={g:np.full((N,K),-1,int) for g in [4,8]}
 for i in range(win+1,N-1):
  idx=np.flatnonzero(eligible[i]&np.all(np.isfinite(e.R[i-win+1:i+1]),axis=0))
  if len(idx)<8:continue
  rr=e.R[i-win+1:i+1,idx];corr=np.corrcoef(rr,rowvar=False)
  Z=linkage(squareform(np.maximum(0,1-np.clip(corr,-1,1)),checks=False),method='average')
  for g in groups:groups[g][i,idx]=fcluster(Z,g,criterion='maxclust')
 for g,labels in groups.items():
  for horizon in [3,14]:
   mom=e.P/np.roll(e.P,horizon,axis=0)-1
   for mode in ['momentum','rotation']:
    T=np.zeros_like(base)
    for i in range(win+1,N-1):
     idx=np.flatnonzero(labels[i]>=0);s=np.full(K,np.nan)
     for lab in np.unique(labels[i,idx]):
      jj=idx[labels[i,idx]==lab]
      if len(jj)<2:continue
      val=np.nanmean(mom[i,jj]/np.maximum(e.SD[i,jj],1e-9))
      s[jj]=val if mode=='momentum' else -val
     T[i]=e.book(idx,s,5)
    C[f'cluster_w{win}_g{g}_h{horizon}_{mode}']=T
# All pairs screened on a fixed, past-only 180d formation period, BEFORE evaluation.
formation_end=230; form=e.R[formation_end-180:formation_end]
idx=np.flatnonzero(eligible[formation_end]&np.all(np.isfinite(form),axis=0))
corr=np.corrcoef(form[:,idx],rowvar=False)
pairs=[]
for a in range(len(idx)):
 for bb in range(a+1,len(idx)):
  pair_screen.append((e.names[idx[a]],e.names[idx[bb]],float(corr[a,bb])))
  if corr[a,bb]>=.7:pairs.append((idx[a],idx[bb]))
# Ratio spread and OLS hedge spread are both hypotheses. No full-history cointegration selection.
logp=np.log(e.P)
for a,bb in pairs:
 for win in [30,60]:
  for hedge in ['ratio','ols']:
   x=logp[:,bb]; y=logp[:,a]
   h=np.ones(N)
   if hedge=='ols':
    mx=roll(x[:,None],win)[:,0];my=roll(y[:,None],win)[:,0]
    h=(roll((x*y)[:,None],win)[:,0]-mx*my)/(roll((x*x)[:,None],win)[:,0]-mx*mx)
   # At time i use one fitted hedge for entire trailing spread window.
   T=np.zeros_like(base)
   for i in range(formation_end+1,N-1):
    if not(eligible[i,a] and eligible[i,bb]) or not np.isfinite(h[i]) or h[i]<=0:continue
    sp=y[i-win+1:i+1]-h[i]*x[i-win+1:i+1]
    if not np.isfinite(sp).all() or sp.std(ddof=1)<1e-8:continue
    z=(sp[-1]-sp.mean())/sp.std(ddof=1)
    if abs(z)<1:continue
    # Dollar neutral implementation; OLS only models signal, not beta hedge book.
    T[i,a]=-.5*np.sign(z);T[i,bb]=.5*np.sign(z)
   C[f'pair_{e.names[a]}_{e.names[bb]}_w{win}_{hedge}']=T
print('family',len(C),'pairs',len(pairs),'screened',len(pair_screen),flush=True)
# Common evaluation includes 2023; pairs stay inactive until after formation. Blends trade one account, net before cost.
e.START=45;e.ix=np.arange(e.START,e.END);e.outdates=e.dates[e.ix+1]
masks={'full':np.ones(len(e.ix),bool),'2023-24':e.outdates.year<=2024,'2025-26':e.outdates.year>=2025}
br=[e.run(base,o) for o in range(3)];B=np.array([v['r'] for v in br])
rows=[];returns=[];stand=[];keys=list(C)
def summarize(key,rr,kind):
 row={'candidate':key,'kind':kind}
 for period,m in masks.items():
  ss=[e.stat(v['r'][m]) for v in rr];prefix=period+'_'
  row[prefix+'sharpe']=np.mean([s['sharpe'] for s in ss]);row[prefix+'phase_sd']=np.std([s['sharpe'] for s in ss]);row[prefix+'phase_min']=min(s['sharpe'] for s in ss);row[prefix+'phase_max']=max(s['sharpe'] for s in ss)
  row[prefix+'net_annual']=np.mean([s['annual_arithmetic'] for s in ss]);row[prefix+'cagr']=np.mean([s['cagr'] for s in ss]);row[prefix+'turnover']=np.mean([v['turn'][m].mean() for v in rr]);row[prefix+'annual_cost']=row[prefix+'turnover']*.00075*365
  row[prefix+'corr']=np.mean([np.corrcoef(v['r'][m],B[o,m])[0,1] for o,v in enumerate(rr)])
 row['missing_max']=max(v['missing'].max() for v in rr)
 return row
baseline=summarize('baseline',br,'baseline')
for j,(key,T) in enumerate(C.items()):
 sr=[e.run(T,o) for o in range(3)];stand.append(summarize(key,sr,'standalone'))
 mix=.8*base+.2*T;gross=np.abs(mix).sum(axis=1);mix=np.divide(mix,gross[:,None],out=np.zeros_like(mix),where=gross[:,None]>0)
 rr=[e.run(mix,o) for o in range(3)];rows.append(summarize(key,rr,'blend20'));returns.append(np.array([v['r'] for v in rr]))
 if j%50==0:print('backtest',j,flush=True)
X=np.array(returns);np.savez_compressed(OUT+'/returns.npz',baseline=B,candidates=X,dates=e.outdates.astype(str),names=keys)
# maxT approximate centered, studentized paired Sharpe difference; same blocks all hypotheses/phases.
obs=X.mean(axis=2)/X.std(axis=2,ddof=1)*np.sqrt(365)-B.mean(axis=1)/B.std(axis=1,ddof=1)*np.sqrt(365)
obs=obs.mean(axis=1);boots={}
for L in [15,30,60]:
 draws=[]
 for rep in range(1999):
  starts=rng.integers(len(e.ix),size=int(np.ceil(len(e.ix)/L)));ii=((starts[:,None]+np.arange(L))%len(e.ix)).ravel()[:len(e.ix)]
  xb=X[:,:,ii];bb=B[:,ii]
  ds=(xb.mean(axis=2)/xb.std(axis=2,ddof=1)-bb.mean(axis=1)/bb.std(axis=1,ddof=1)).mean(axis=1)*np.sqrt(365);draws.append(ds)
 draws=np.array(draws);se=draws.std(axis=0,ddof=1);null=(draws-obs)/se;maximum=np.nanmax(null,axis=1)
 for j,row in enumerate(rows):row[f'p_fwer_L{L}']=(1+np.sum(maximum>=obs[j]/se[j]))/2000;row[f'delta_ci_low_L{L}']=np.quantile(draws[:,j],.025);row[f'delta_ci_high_L{L}']=np.quantile(draws[:,j],.975)
 print('bootstrap',L,flush=True)
for row in rows:
 row['p_fwer_global']=min(1,3*max(row[f'p_fwer_L{L}'] for L in [15,30,60]))
 row['passes']=row['p_fwer_global']<.05 and all(row[p+'_sharpe']>baseline[p+'_sharpe'] for p in ['2023-24','2025-26']) and row['full_corr']<.9
pd.DataFrame(rows+stand+[baseline]).to_csv(OUT+'/metrics.csv',index=False)
pd.DataFrame(pair_screen,columns=['a','b','formation_corr']).to_csv(OUT+'/pair_screen.csv',index=False)
meta={'family_count':len(C),'screened_pairs':len(pair_screen),'eligible_pairs':len(pairs),'formation_end':str(e.dates[formation_end]),'evaluation_start':str(e.outdates[0]),'evaluation_end':str(e.outdates[-1]),'period_days':{p:int(m.sum()) for p,m in masks.items()},'baseline':baseline,'passing': [r for r in rows if r['passes']], 'fee_taker':.00045,'slippage':.0003,'gross':2.7,'bootstrap_draws':1999,'seed':20261008,'manifest':{os.path.basename(f):hashlib.sha256(open(f,'rb').read()).hexdigest() for f in e.FILES}}
json.dump(meta,open(OUT+'/results.json','w'),indent=2)
print('BASELINE',baseline,flush=True);print('PASS',meta['passing'],flush=True)
