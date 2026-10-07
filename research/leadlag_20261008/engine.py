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
scores={}  # Baseline engine only; no previously excluded factors are tested.
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
