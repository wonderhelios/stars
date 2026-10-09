"""Frozen-input baseline audit; no trailing TP candidates until reproduction passes."""
import json,hashlib
from pathlib import Path
import numpy as np,pandas as pd
D=Path(__file__).resolve().parent; req=json.loads((D/'retrieval/request.json').read_text()); end=req['end_ms']; names=sorted(req['coins'])
raw={c:[x for x in json.loads(Path('/tmp/hl-daily-full',c+'.json').read_text()) if x['T']<end and x['t']>=req['daily_start_ms']] for c in names}
times=sorted({x['t'] for rows in raw.values() for x in rows if float(x['c'])>0}); ti={t:i for i,t in enumerate(times)}; n=len(times); k=len(names)
p=np.full((n,k),np.nan);o=p.copy();v=p.copy()
for j,c in enumerate(names):
 for x in raw[c]:
  i=ti[x['t']];p[i,j]=float(x['c']);o[i,j]=float(x['o']);v[i,j]=float(x['v'])*p[i,j]
r=p/np.roll(p,1,axis=0)-1;r[0]=np.nan
V=pd.DataFrame(v).rolling(30,min_periods=5).mean().shift(1).values
SD=pd.DataFrame(r).rolling(20,min_periods=5).std(ddof=1).values
mom=p/np.roll(p,14,axis=0)-1; shock=v/V; T=np.zeros_like(p)
for i in range(32,n):
 idx=np.flatnonzero((V[i]>=5e6)&np.isfinite(p[i])&np.isfinite(p[i-14])&np.isfinite(SD[i])&np.array([':' not in c for c in names]))
 if len(idx)<8:continue
 kk=min(5,max(1,int(np.floor(len(idx)*.2+.5))),len(idx)//2)
 for s in [mom[i]/np.maximum(SD[i],1e-9),-SD[i],shock[i]]:
  order=idx[np.lexsort((np.array(names)[idx],s[idx]))];T[i,order[:kk]]-=.5/kk/3;T[i,order[-kk:]]+=.5/kk/3
 T[i,np.abs(T[i])<1e-12]=0
 if np.abs(T[i]).sum():T[i]/=np.abs(T[i]).sum()
checks=[]
for i in [45,180,365,550,750,1000,n-2]:
 acc={};vals=[]
 for j,c in enumerate(names):
  cm={x['t']:float(x['c']) for x in raw[c]};vm={x['t']:float(x['v'])*float(x['c']) for x in raw[c]};vv=[vm[t] for t in times[i-30:i] if t in vm]
  if ':' in c or len(vv)<5 or np.mean(vv)<5e6 or times[i] not in cm or times[i-14] not in cm:continue
  rr=[cm[times[z]]/cm[times[z-1]]-1 for z in range(i-19,i+1) if times[z] in cm and times[z-1] in cm]
  if len(rr)<5:continue
  sd=max(np.std(rr,ddof=1),1e-9);vals.append((c,(cm[times[i]]/cm[times[i-14]]-1)/sd,-sd,vm.get(times[i],0)/np.mean(vv)))
 if len(vals)>=8:
  kk=min(5,max(1,int(np.floor(len(vals)*.2+.5))),len(vals)//2)
  for f in [1,2,3]:
   order=sorted(vals,key=lambda z:(z[f],z[0]))
   for pos,z in enumerate(order):acc[z[0]]=acc.get(z[0],0)+(.5/kk/3 if pos>=len(vals)-kk else (-.5/kk/3 if pos<kk else 0))
 z=np.array([acc.get(c,0) for c in names]);z[np.abs(z)<1e-12]=0
 if np.abs(z).sum():z/=np.abs(z).sum()
 err=float(np.max(np.abs(z-T[i])));assert err<1e-10;checks.append(dict(t=times[i],error=err))
OP=pd.DataFrame(o).ffill().values
start=next(i for i in range(33,n) if np.abs(T[i-1]).sum());w=np.zeros(k);rs=[];turn=[];missing=[]
for i in range(start,n):
 ratio=np.nan_to_num(OP[i]/OP[i-1],nan=1);gain=0 if i==start else w@(ratio-1)
 if 1+gain<=0:raise ValueError('insolvency')
 w=w*ratio/(1+gain);target=T[i-1]*2.7;delta=target-w
 valid=np.isfinite(o[i]);missing.append(float(np.abs(w[~valid]).sum()))
 use=(np.abs(delta)>=.02*np.abs(target))|(target*w<=0);delta=np.where(use&valid,delta,0);delta[~valid]=-w[~valid]
 tr=np.abs(delta).sum();cost=tr*.0007;w=(w+delta)/(1-cost);rs.append((1+gain)*(1-cost)-1);turn.append(tr)
tr=np.abs(w).sum();rs[-1]=(1+rs[-1])*(1-.0007*tr)-1;turn[-1]+=tr
rs=np.array(rs);dates=pd.to_datetime(times[start:],unit='ms',utc=True)
def stats(m):
 q=rs[m];tt=np.array(turn)[m];eq=np.cumprod(1+q)
 return dict(sharpe=float(q.mean()/q.std(ddof=1)*np.sqrt(365)),days=len(q),annual_return=float(q.mean()*365),annual_cost=float(tt.mean()*.0007*365),daily_turnover=float(tt.mean()),max_drawdown=float(np.min(eq/np.maximum.accumulate(np.r_[1,eq])[1:]-1)))
result=dict(full=stats(np.ones(len(rs),bool)),sub_2023_24=stats(dates.year<=2024),sub_2025_26=stats(dates.year>=2025),start=str(dates[0]),end=str(dates[-1]),scalar_checks=checks,missing_events=int(np.count_nonzero(missing)),missing_max=float(max(missing)),configuration=dict(cap=5,top_frac=.2,gross=2.7,cost=.0007,band=.02,rebalance_slices=1,execution='prior closed daily signal -> next UTC open; large-account approximation; stale missing exit'),source_sha256=hashlib.sha256(Path('src/trader.rs').read_bytes()).hexdigest())
(D/'baseline_recheck.json').write_text(json.dumps(result,indent=2));np.savez_compressed(D/'baseline_recheck_returns.npz',r=rs,dates=np.array(dates.astype(str)),turn=turn);print(json.dumps(result,indent=2))
