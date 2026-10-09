"""Derived directly from frozen baseline.py; does not execute its baseline runs."""
import json, hashlib, glob, datetime
from pathlib import Path
import numpy as np
import pandas as pd
ROOT=Path(__file__).resolve().parent
exec((ROOT/'grid2/frozen_baseline.py').read_text().split('out=dict(config=')[0],globals())
H=np.full_like(P,np.nan);L=H.copy()
for j,c in enumerate(names):
 for x in raw[c]: H[ti[x['t']],j]=float(x['h']);L[ti[x['t']],j]=float(x['l'])
F=np.array([10**meta[c]['szDecimals'] for c in names]);slot=np.array([sum(c.encode())%3 for c in names])
D=pd.to_datetime(ts[33:],unit='ms',utc=True)
SAVE=ROOT/'grid2';SAVE.mkdir(exist_ok=True)
# Written before candidate computation; stricter family includes both path scenarios.
CONFIG=dict(activation=[.1,.2,.3],distance=[.05,.1,.15],modes=['coin','portfolio'],paths=['pess','opt'],primary_slices=1,sensitivity_slices=3,phases=[0,1,2],family=144,bootstrap=3999,block=30,seed=20261009,fee=.0007,gate='FWER<.05; both subperiod Sharpe and deltas positive; delta > phase SD; pessimistic positive',short='favorable minimum price, adverse maximum; return on entry notional',portfolio='synchronous open/close equity; peaks only close; pess checks open then close, opt close only')
(SAVE/'predeclared.json').write_text(json.dumps(CONFIG,indent=2))
def simulate(act,dist,mode,path,slices=1,phase=0,hourly=None):
 eq=1e6;anchor=eq;qty=np.zeros(k);last=np.full(k,np.nan);entry=last.copy();peak=np.zeros(k);armed=np.zeros(k,bool);locked=np.zeros(k,bool)
 gp=eq;ga=False;gentry=eq;rr=[];turn=[];costs=[];events=[];missing=0;bankrupt=False;bankruptcy_t=None
 def exits(i,op,hi,lo,cl,t):
  nonlocal eq,qty,peak,armed,gp,ga,locked
  held=(abs(qty)>1e-12)&np.isfinite(cl)&np.isfinite(entry)
  if mode=='coin':
   sign=np.sign(qty);fav=np.where(sign>0,hi,lo);bad=np.where(sign>0,lo,hi)
   favorable=sign*(fav/entry-1);peak=np.where(held,np.maximum(peak,favorable),peak);armed|=held&(peak>=act)
   stop=entry*(1+sign*(peak-dist));probe=bad if path=='pess' else cl
   hit=held&armed&(sign*(probe-stop)<=0)
   # Pess gap uses open only when previously armed stop (handled by peak may be new); cap execution at adverse open.
   fill=bad if path=='pess' else cl
  else:
   # Mark every leg at same timestamp; never combine asynchronous extrema.
   def nav(px):return eq+np.nansum(qty*(np.where(np.isfinite(px),px,last)-last))
   nc=nav(cl);no=nav(op);oldgp=gp;oldga=ga
   gp=max(gp,nc);ga=ga or gp/gentry-1>=act
   trigger=(oldga and no<=oldgp*(1-dist)) if path=='pess' else False
   closehit=ga and nc<=gp*(1-dist)
   hit=held if trigger or closehit else np.zeros(k,bool);fill=op if trigger else cl
  idx=np.flatnonzero(hit&np.isfinite(fill))
  if len(idx):
   fee=.0007*np.sum(abs(qty[idx])*fill[idx]);eq+=np.sum(qty[idx]*(fill[idx]-last[idx]))-fee
   costs[-1]+=fee/anchor;turn[-1]+=np.sum(abs(qty[idx])*fill[idx])/anchor
   for j in idx:events.append([int(t),names[j],float(qty[j]),float(fill[j]),float(entry[j]),float(peak[j])])
   qty[idx]=0;locked[idx]=True;armed[idx]=False;peak[idx]=0
   if mode=='portfolio':ga=False;gp=eq
 for i in range(33,n):
  if bankrupt:
   rr.append(-1.0 if eq<=0 and anchor>0 else 0.0);turn.append(0.0);costs.append(0.0);eq=0.;anchor=0.;continue
  before=anchor;px=a['o'][i];held=abs(qty)>1e-12;missing+=int(np.sum(held&~np.isfinite(px)))
  mark=np.where(np.isfinite(px),px,last);eq+=np.nansum(qty*(mark-last))
  eligible=np.ones(k,bool) if slices==1 else ((ts[i]//86400000+slot+phase)%3==0)
  locked[eligible]=False
  if mode=='portfolio' and not np.any(held):gentry=eq;gp=eq;ga=False
  w=target(i-1,eq,5);wanted=np.zeros(k);ok=np.isfinite(px)&(abs(w)>1e-12)
  size=np.floor(abs(w)*eq*2.7/np.where(ok,px,1)*F)/F;wanted=np.where(ok&(size*px>=10),np.sign(w)*size,0)
  delta=np.zeros(k)
  for j in np.flatnonzero(eligible&np.isfinite(px)):
   d=wanted[j]-qty[j]
   if qty[j]*wanted[j]<0:delta[j]=d
   elif wanted[j]==0:
    if abs(qty[j])*px[j]>=10:delta[j]=-qty[j]
   elif np.floor(abs(d)*F[j])/F[j]*px[j]>=max(10,abs(wanted[j])*px[j]*.02):delta[j]=np.sign(d)*np.floor(abs(d)*F[j])/F[j]
  old=qty.copy();oldentry=entry.copy();oldpeak=peak.copy();fee=.0007*np.sum(abs(delta)*np.nan_to_num(px));tr=np.sum(abs(delta)*np.nan_to_num(px))/eq;eq-=fee;qty+=delta
  new=(old*qty<=0)&(abs(qty)>1e-12);add=(old*qty>0)&(abs(qty)>abs(old))
  entry[new]=px[new];peak[new]=0;armed[new]=False
  entry[add]=(abs(old[add])*entry[add]+abs(delta[add])*px[add])/abs(qty[add])
  # Peak is expressed in entry-return units; rebase after additions to preserve peak price.
  peak[add]=np.maximum(0,np.sign(qty[add])*(oldentry[add]*(1+np.sign(qty[add])*oldpeak[add])/entry[add]-1))
  armed[add]=peak[add]>=act if act is not None else False
  last=mark;rr.append(eq/before-1);turn.append(tr);costs.append(fee/before);anchor=eq
  if act is not None:
   if hourly is None:exits(i,px,H[i],L[i],P[i],ts[i])
   else:
    for t,bars in hourly.get(ts[i],[]):
     oo=np.full(k,np.nan);hh=oo.copy();ll=oo.copy();cc=oo.copy()
     for j,b in bars:oo[j]=float(b['o']);hh[j]=float(b['h']);ll[j]=float(b['l']);cc[j]=float(b['c'])
     exits(i,oo,hh,ll,cc,t)
  if eq<=0:bankrupt=True;bankruptcy_t=int(ts[i]);qty[:]=0
 return dict(r=np.array(rr),turn=np.array(turn),cost=np.array(costs),events=events,missing=missing,bankruptcy_t=bankruptcy_t)
def persist(key,v):
 np.savez_compressed(SAVE/(key+'.npz'),r=v['r'],t=ts[33:],turn=v['turn'],cost=v['cost'])
 (SAVE/(key+'.json')).write_text(json.dumps(dict(events=v['events'],missing=v['missing'],bankruptcy_t=v['bankruptcy_t'])))
if __name__=='__main__':
 for slices,phases in [(1,[0]),(3,range(3))]:
  for phase in phases:
   for act in [.1,.2,.3]:
    for dist in [.05,.1,.15]:
     for mode in ['coin','portfolio']:
      for path in ['pess','opt']:
       key=f's{slices}_p{phase}_{act}_{dist}_{mode}_{path}'
       if (SAVE/(key+'.npz')).exists():continue
       persist(key,simulate(act,dist,mode,path,slices,phase));print(key,flush=True)
   if slices==3:
    key=f'control_s3_p{phase}'
    if not (SAVE/(key+'.npz')).exists():persist(key,simulate(None,0,'coin','pess',slices,phase))
