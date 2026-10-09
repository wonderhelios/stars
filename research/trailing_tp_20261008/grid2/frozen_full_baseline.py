import json,hashlib,glob,os,datetime,warnings
import numpy as np,pandas as pd
from pathlib import Path
warnings.filterwarnings('ignore',category=RuntimeWarning)
OUT=Path(__file__).resolve().parent
req=json.load(open(OUT/'retrieval/request.json'));end=req['end_ms']
files=sorted(glob.glob('/tmp/hl-daily-full/*.json'));names=[Path(f).stem for f in files]
raw={Path(f).stem:[x for x in json.load(open(f)) if x['T']<end and float(x['c'])>0] for f in files}
ts=sorted({x['t'] for d in raw.values() for x in d});ti={t:i for i,t in enumerate(ts)};n=len(ts);k=len(names)
a={z:np.full((n,k),np.nan) for z in ['o','c','v']}
for j,c in enumerate(names):
 for x in raw[c]:
  for z in a:a[z][ti[x['t']],j]=float(x[z])
P=a['c'];Q=P*a['v'];R=P/np.roll(P,1,axis=0)-1;R[0]=np.nan
V=pd.DataFrame(Q).rolling(30,min_periods=5).mean().shift(1).values
SD=pd.DataFrame(R).rolling(20,min_periods=5).std(ddof=1).values
M=P/np.roll(P,14,axis=0)-1
meta={x['name']:x for x in json.load(open(OUT/'retrieval/meta.json'))['universe']}

def target(i,equity,cap=5):
 cap=min(cap,max(1,int(np.floor(equity*2.7/15))//4))
 w=np.zeros(k)
 if i<32:return w
 idx=np.flatnonzero((V[i]>=5e6)&np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])&np.array([':' not in c for c in names]))
 if len(idx)<8:return w
 kk=min(max(1,int(np.floor(len(idx)*.2+.5))),cap,len(idx)//2)
 for s in [M[i]/np.maximum(SD[i],1e-9),-SD[i],Q[i]/V[i]]:
  order=idx[np.lexsort((np.array(names)[idx],s[idx]))];w[order[:kk]]-=.5/kk/3;w[order[-kk:]]+=.5/kk/3
 g=np.abs(w).sum()
 return w/g if g>0 else w

def sh(r):return float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365)) if len(r)>1 else None

def run(cap=5,clock='open',fee=.0007):
 # large initial capital; exchange size decimals/minimum/band applied at every order
 prices=a['o'] if clock=='open' else P
 eq=1e6;qty=np.zeros(k);last=np.full(k,np.nan);rets=[];turn=[];missing=[];dates=[]
 for i in range(33,n):
  px=prices[i];held=np.abs(qty)>1e-12
  miss=held&~np.isfinite(px);missing.append(dict(t=ts[i],coins=np.array(names)[miss].tolist(),stale_notional_equity=float(np.nansum(abs(qty[miss])*last[miss])/eq))) if miss.any() else None
  mark=np.where(np.isfinite(px),px,last)
  gain=np.nansum(qty*(mark-last));before=eq;eq+=gain
  if eq<=0:raise ValueError('insolvent')
  w=target(i-1 if clock=='open' else i,eq,cap)
  wanted=np.zeros(k)
  for j,c in enumerate(names):
   if not np.isfinite(px[j]) or abs(w[j])<1e-12:continue
   f=10**meta[c]['szDecimals'];size=np.floor(abs(w[j])*eq*2.7/px[j]*f)/f
   if size*px[j]>=10:wanted[j]=np.sign(w[j])*size
  delta=np.zeros(k)
  for j in range(k):
   if not np.isfinite(px[j]):continue  # cannot liquidate stale missing quote
   f=10**meta[names[j]]['szDecimals'];d=wanted[j]-qty[j]
   if qty[j]*wanted[j]<0: # closing plus reopening charged both sides, equivalent abs delta
    delta[j]=d
   elif wanted[j]==0:
    if abs(qty[j])*px[j]>=10:delta[j]=-qty[j]
   elif np.floor(abs(d)*f)/f*px[j]>=max(10,abs(wanted[j])*px[j]*.02):
    delta[j]=np.sign(d)*np.floor(abs(d)*f)/f
  cost=fee*np.sum(abs(delta)*np.nan_to_num(px));tr=np.sum(abs(delta)*np.nan_to_num(px))/eq
  eq-=cost;qty+=delta;last=mark;rets.append(eq/before-1);turn.append(tr);dates.append(ts[i])
 rr=np.array(rets);dd=pd.to_datetime(dates,unit='ms',utc=True)
 results={}
 for label,mask in [('full',np.ones(len(rr),bool)),('2023-24',dd.year<=2024),('2025-26',dd.year>=2025)]:
  r=rr[mask];t=np.array(turn)[mask]
  results[label]=dict(days=len(r),sharpe=sh(r),annual_arithmetic=float(r.mean()*365) if len(r) else None,annual_cost=float(t.mean()*fee*365) if len(t) else None,daily_turnover=float(t.mean()) if len(t) else None)
 results['missing_held_quotes']=missing;results['ending_equity']=eq
 np.savez_compressed(OUT/f'baseline_cap{cap}_{clock}_{fee}.npz',r=rr,t=dates,turn=turn)
 return results
