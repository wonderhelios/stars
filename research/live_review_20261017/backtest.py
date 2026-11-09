"""Independent daily cash/position ledger. No production imports or writes.
Run using bundled Python (numpy, pandas). Results are research, not deployable validation.
"""
import json, hashlib, warnings, os
from pathlib import Path
import numpy as np
import pandas as pd
warnings.filterwarnings('ignore',category=RuntimeWarning)
ROOT=Path(__file__).resolve().parent
REQ=Path('research/trailing_tp_20261008/retrieval')
req=json.loads((REQ/'request.json').read_text())
meta={x['name']:x for x in json.loads((REQ/'meta.json').read_text())['universe']}
files=sorted(Path('/tmp/hl-daily-v2').glob('*.json'))
names=np.array([f.stem for f in files]); raw={f.stem:[r for r in json.loads(f.read_text()) if req['daily_start_ms']<=r['t'] and r['T']<req['end_ms'] and float(r['c'])>0] for f in files}
if os.environ.get('INCLUDE_RECENT') == '1':
 for c in names:
  p=ROOT/'recent'/(c+'.json')
  if p.exists():
   merged={r['t']:r for r in raw[c]};merged.update({r['t']:r for r in json.loads(p.read_text())});raw[c]=list(merged.values())
ts=sorted({r['t'] for rows in raw.values() for r in rows}); ti={t:i for i,t in enumerate(ts)}; N=len(ts); K=len(names)
O=np.full((N,K),np.nan); C=O.copy(); Q=O.copy()
for j,c in enumerate(names):
 for r in raw[c]:
  i=ti[r['t']];O[i,j]=float(r['o']);C[i,j]=float(r['c']);Q[i,j]=float(r['c'])*float(r['v'])
R=C/np.roll(C,1,axis=0)-1;R[0]=np.nan
V=pd.DataFrame(Q).rolling(30,min_periods=5).mean().shift(1).to_numpy()
SD=pd.DataFrame(R).rolling(20,min_periods=5).std(ddof=1).to_numpy()
M=C/np.roll(C,14,axis=0)-1
F=np.array([10**meta[c]['szDecimals'] for c in names]); HASH=np.array([sum(c.encode()) for c in names]);NOTCOL=np.array([':' not in c for c in names])
DATES=pd.to_datetime(ts[33:],unit='ms',utc=True)
MANIFEST={'start':str(pd.to_datetime(ts[0],unit='ms',utc=True)),'end':str(pd.to_datetime(ts[-1],unit='ms',utc=True)),'coins':K,'days':N,'sha256':{f.name:hashlib.sha256(f.read_bytes()).hexdigest() for f in files}}
MANIFEST['includes_recent']=os.environ.get('INCLUDE_RECENT') == '1'
if MANIFEST['includes_recent']:
 MANIFEST['recent_sha256']={f.name:hashlib.sha256(f.read_bytes()).hexdigest() for f in (ROOT/'recent').glob('*.json')}
(ROOT/('data_manifest_recent.json' if MANIFEST['includes_recent'] else 'data_manifest_historical.json')).write_text(json.dumps(MANIFEST,indent=2))
# Base rankings fixed from past closed bars, no future returns involved.
BOOKS=np.zeros((N,K)); COUNTS=np.zeros(N,int)
for i in range(32,N):
 idx=np.flatnonzero((V[i]>=1e7)&np.isfinite(C[i])&np.isfinite(C[i-14])&np.isfinite(SD[i])&NOTCOL);COUNTS[i]=len(idx)
 if len(idx)<8:continue
 kk=min(max(1,int(np.floor(len(idx)*.2+.5))),10,len(idx)//2)
 for sc in [M[i]/np.maximum(SD[i],1e-9),Q[i]/V[i]]:
  order=idx[np.lexsort((names[idx],sc[idx]))];BOOKS[i,order[:kk]]-=.25/kk;BOOKS[i,order[-kk:]]+=.25/kk
 g=np.abs(BOOKS[i]).sum()
 if g:BOOKS[i]/=g

def target(i,equity,gross,mode):
 # Recompute only when the account's minimum position constraint reduces cap.
 cap=min(10,max(1,int(np.floor(equity*gross/15))//4));w=BOOKS[i].copy()
 if cap<10:
  w[:]=0;idx=np.flatnonzero((V[i]>=1e7)&np.isfinite(C[i])&np.isfinite(C[i-14])&np.isfinite(SD[i])&NOTCOL)
  if len(idx)>=8:
   kk=min(max(1,int(np.floor(len(idx)*.2+.5))),cap,len(idx)//2)
   for sc in [M[i]/np.maximum(SD[i],1e-9),Q[i]/V[i]]:
    order=idx[np.lexsort((names[idx],sc[idx]))];w[order[:kk]]-=.25/kk;w[order[-kk:]]+=.25/kk
   if np.abs(w).sum():w/=np.abs(w).sum()
 if mode=='cap5':
  w=np.clip(w,-.05,.05)
  # Keep dollar neutrality, do not refill freed risk into remaining names.
  l=w[w>0].sum();s=-w[w<0].sum();leg=min(l,s)
  if l:w[w>0]*=leg/l
  if s:w[w<0]*=leg/s
 return w

def simulate(mode='baseline',fee=.00075,capital=5739.2678625,phase=0,drag=0,start_idx=33,end_idx=None):
 gross={'half':1.35,'third':.9}.get(mode,2.7);eq=capital;qty=np.zeros(K);last=np.full(K,np.nan);prevw=np.zeros(K)
 returns=[];turns=[];exposures=[];curve=[eq];missing=0;missing_coins={};missing_max_pct=0.0
 for i in range(start_idx,N if end_idx is None else end_idx):
  before=eq;o=O[i];valid=np.isfinite(o)&(o>0);op=np.where(valid,o,last)
  eq+=np.nansum(qty*(op-last))
  if eq<=0:raise RuntimeError('insolvent at open')
  missing_mask=(np.abs(qty)>1e-12)&~valid
  missing+=int(np.sum(missing_mask))
  missing_max_pct=max(missing_max_pct,float(np.nansum(np.abs(qty[missing_mask])*last[missing_mask])/eq))
  for j in np.flatnonzero(missing_mask):missing_coins[str(names[j])]=missing_coins.get(str(names[j]),0)+1
  w=target(i-1,eq,gross,mode);threshold=max(10,eq*.017)
  wanted=np.sign(w)*np.floor(np.nan_to_num(np.abs(w)*eq*gross/o)*F)/F
  wanted[(np.abs(wanted)*np.nan_to_num(o)<threshold)|~valid]=0
  delta=wanted-qty;rounded=np.sign(delta)*np.floor(np.abs(delta)*F+1e-9)/F
  close=(wanted==0)&(np.abs(qty)*np.nan_to_num(o)>=10)
  flip=(wanted*qty<0);fresh=(np.abs(qty)<1e-12)&(wanted!=0)
  band=.10 if mode=='band10' else .02
  resize=np.abs(rounded)*np.nan_to_num(o)>=np.maximum(threshold,np.abs(wanted)*np.nan_to_num(o)*band)
  if mode=='signal_only':
   over=np.nansum(np.abs(qty)*op)>eq*gross*1.1
   resize &= (np.abs(w-prevw)>1e-12)|over
  trade=(close|flip|fresh|resize)&valid
  if mode=='slices3':trade &= ((ts[i]//86400000+HASH+phase)%3==0)
  delta=np.where(trade,rounded,0);delta[trade&close]=-qty[trade&close]
  notional=np.sum(np.abs(delta)*np.nan_to_num(o));eq-=fee*notional;qty+=delta;prevw=w
  cp=np.where(np.isfinite(C[i]),C[i],op);eq+=np.nansum(qty*(cp-op));last=cp
  exposure=np.nansum(np.abs(qty)*cp)/max(eq,1e-12)
  eq-=drag/365*np.nansum(np.abs(qty)*cp)
  if eq<=0:raise RuntimeError('insolvent at close')
  returns.append(eq/before-1);turns.append(notional/before);exposures.append(exposure);curve.append(eq)
 return {'r':np.array(returns),'turn':np.array(turns),'gross':np.array(exposures),'curve':np.array(curve),'missing_mark_days_positions':missing,'missing_coins':missing_coins,'missing_max_equity_fraction':missing_max_pct}

def metrics(a,mask=None):
 mask=np.ones(len(a['r']),bool) if mask is None else mask;r=a['r'][mask];t=a['turn'][mask];cv=np.r_[1,np.cumprod(1+r)]
 return {'days':len(r),'sharpe':float(r.mean()/r.std(ddof=1)*np.sqrt(365)),'arithmetic_annual':float(r.mean()*365),'cagr':float(cv[-1]**(365/len(r))-1),'mdd':float((cv/np.maximum.accumulate(cv)-1).min()),'annual_vol':float(r.std(ddof=1)*np.sqrt(365)),'daily_turnover':float(t.mean()),'avg_gross':float(a['gross'][mask].mean()),'max_gross':float(a['gross'][mask].max()),'worst_day':float(r.min()),'total':float(cv[-1]-1)}

if __name__=='__main__':
 modes=[('baseline',0),('half',0),('third',0),('cap5',0),('band10',0),('signal_only',0),('slices3',0),('slices3',1),('slices3',2)]
 masks={'full':np.ones(len(DATES),bool),'2023-24':DATES.year<=2024,'2025-H1_2026':(DATES.year>=2025)&(DATES<pd.Timestamp('2026-07-01',tz='UTC')),'late_2026':DATES>=pd.Timestamp('2026-07-01',tz='UTC')}
 results={};series={}
 for mode,phase in modes:
  key=f'{mode}_p{phase}';a=simulate(mode,phase=phase);series[key]=a
  out={s:metrics(a,m) for s,m in masks.items()};out['missing_mark_days_positions']=a['missing_mark_days_positions'];out['missing_coins']=a['missing_coins'];out['missing_max_equity_fraction']=a['missing_max_equity_fraction'];out['correlation_baseline']=float(np.corrcoef(a['r'],series['baseline_p0']['r'])[0,1])
  for label,kwargs in [('cost12.5bp',{'fee':.00125}),('capital1m',{'capital':1e6}),('funding_drag5pct_notional',{'drag':.05})]:out[label]=metrics(simulate(mode,phase=phase,**kwargs))
  results[key]=out;print(key,json.dumps(out['full']),flush=True)
 # Paired centered moving blocks, family-wise max-T over all variants.
 base=series['baseline_p0']['r'];keys=list(series)[1:];D=np.column_stack([series[k]['r']-base for k in keys]);obs=D.mean(0);null=D-obs; rng=np.random.default_rng(20261017);boot={}
 for bl in [15,30,60]:
  samples=[]
  for j in range(1999):
   starts=rng.integers(0,len(base),size=int(np.ceil(len(base)/bl)));idx=((starts[:,None]+np.arange(bl))%len(base)).ravel()[:len(base)];samples.append(null[idx].mean(0))
  samples=np.array(samples);se=samples.std(0,ddof=1);mx=np.max(samples/np.maximum(se,1e-15),axis=1)
  boot[str(bl)]={k:{'mean_daily_difference':float(obs[i]),'p_fwer':float((1+np.sum(mx>=obs[i]/max(se[i],1e-15)))/2000)} for i,k in enumerate(keys)}
 (ROOT/'results.json').write_text(json.dumps({'validation_status':'DIAGNOSTIC_ONLY_INVALID_FOR_PROMOTION: missing held-position prices, funding/settlements incomplete','results':results,'paired_mean_return_maxT':boot},indent=2))
 np.savez_compressed(ROOT/'daily_series.npz',ts=np.array(ts[33:]),**{k:v['r'] for k,v in series.items()})
 print('COMPLETE',flush=True)
