import os,sys,json,hashlib
from pathlib import Path
import numpy as np
import pandas as pd
os.environ['INCLUDE_RECENT']='1';ROOT=Path(__file__).resolve().parent;sys.path.insert(0,str(ROOT.parent));import backtest as b
DAY=86400000;HOUR=3600000
# Union raw hourly files by timestamp; extension supersedes overlapping cache.
raw={};hashes={}
for name in b.names:
 rows={}
 for folder in ['hourly','recent_hourly']:
  p=ROOT/folder/(name+'.json')
  if p.exists():
   hashes[str(p.relative_to(ROOT))]=hashlib.sha256(p.read_bytes()).hexdigest()
   for r in json.loads(p.read_text()):rows[r['t']]=r
 raw[name]=rows
btc=sorted(raw['BTC']);start=(min(btc)//DAY+1)*DAY;end=1792195200000
HT=np.arange(start,end,HOUR,dtype=np.int64);N=len(HT);K=b.K;A=np.full((N,K,4),np.nan)
for j,name in enumerate(b.names):
 for t,r in raw[name].items():
  if start<=t<end:
   i=(t-start)//HOUR
   for k,z in enumerate(['o','h','l','c']):A[i,j,k]=float(r[z])
DINDEX={int(t):i for i,t in enumerate(b.ts)};FIRST=np.array([min(x['t'] for x in b.raw[n]) if b.raw[n] else end for n in b.names])
(ROOT/'data_manifest.json').write_text(json.dumps({'start':int(start),'end':end,'hours':N,'source_hashes':hashes},indent=2))
VAR=json.loads((ROOT/'spec.json').read_text())['variants']

def daily_weights(i,eq,mode):
 w=b.target(i,eq,2.7,'baseline')
 if mode in ['confirm2','confirm2_stop2sigma_max48']:
  old=b.target(i-1,eq,2.7,'baseline');w[np.sign(old)!=np.sign(w)]=0
  l=w[w>0].sum();s=-w[w<0].sum()
  if l and s:w[w>0]*=.5/l;w[w<0]*=.5/s
  else:w[:]=0
 if mode=='age90_no_shock3sigma':
  recent=b.R[i-2:i+1];prior=b.SD[i-3:i];ok=np.all(np.abs(recent)<=3*np.maximum(prior,1e-9),axis=0)&((b.ts[i]-FIRST)>=90*DAY)
  idx=np.flatnonzero(ok&(b.V[i]>=1e7)&np.isfinite(b.C[i])&np.isfinite(b.C[i-14])&np.isfinite(b.SD[i])&b.NOTCOL);w[:]=0
  if len(idx)>=8:
   cap=min(10,max(1,int(eq*2.7/15)//4));kk=min(max(1,int(np.floor(len(idx)*.2+.5))),cap,len(idx)//2)
   for sc in [b.M[i]/np.maximum(b.SD[i],1e-9),b.Q[i]/b.V[i]]:
    order=idx[np.lexsort((b.names[idx],sc[idx]))];w[order[:kk]]-=.25/kk;w[order[-kk:]]+=.25/kk
   if abs(w).sum():w/=abs(w).sum()
 return w

def trigger(mode,qty,entry,peak,sigma,hi,lo):
 long=qty>0;held=qty!=0;out=np.zeros(K,bool)
 if mode in ['stop5_cool24','stop2sigma_cool24','confirm2_stop2sigma_max48']:
  stop=np.full(K,.05) if mode=='stop5_cool24' else np.maximum(.02,2*sigma)
  out=held&np.where(long,lo<=entry*(1-stop),hi>=entry*(1+stop))
 if mode=='tp8_cool24':out=held&np.where(long,hi>=entry*1.08,lo<=entry*.92)
 if mode=='trail8_4_cool24':
  armed=held&np.where(long,peak>=entry*1.08,peak<=entry*.92)
  out=armed&np.where(long,lo<=peak*.96,hi>=peak*1.04)
 if mode in ['long_tp8_cool24','long_tp8_short_stop5']:out|=held&long&(hi>=entry*1.08)
 if mode in ['short_stop5_cool24','long_tp8_short_stop5']:out|=held&~long&(hi>=entry*1.05)
 return out

def sim(mode,fee=.00075,lo=0,hi=None,exit_delay_hours=1,rebalance_hour=1,target_gross=2.7):
 hi=N if hi is None else hi;eq=5739.2678625;qty=np.zeros(K);last=np.full(K,np.nan);entry=np.zeros(K);peak=np.zeros(K);sigma=np.zeros(K);born=np.zeros(K);block=np.zeros(K);pending=np.zeros(K,bool);due=np.full(K,np.inf);days=[];daily=[];turns=[];gross=[];eqs=[eq];day_before=eq;day_turn=0;missing={};risk_exits=0;holding=[];ledger=[]
 for h in range(lo,hi):
  t=HT[h];o,high,low,close=A[h].T;valid=np.isfinite(o)&(o>0);op=np.where(valid,o,last)
  eq+=np.nansum(qty*(op-last))
  if eq<=0:raise ValueError('insolvent')
  for j in np.flatnonzero((qty!=0)&~valid):missing[str(b.names[j])]=missing.get(str(b.names[j]),0)+1
  age=(t-born)/HOUR;limit=48 if mode in ['max48_cool24','confirm2_stop2sigma_max48'] else 120 if mode=='max120_cool24' else np.inf
  exit_mask=((pending&(t>=due))|((qty!=0)&(age>=limit)))&valid&(np.abs(qty)*o>=10)
  if exit_mask.any():
   nt=np.sum(np.abs(qty[exit_mask])*o[exit_mask]);eq-=fee*nt;day_turn+=nt;risk_exits+=int(exit_mask.sum());holding.extend(age[exit_mask].tolist())
   for j in np.flatnonzero(exit_mask):ledger.append({'t':int(t),'coin':str(b.names[j]),'kind':'risk_exit','age_hours':float(age[j]),'price':float(o[j])})
   qty[exit_mask]=0;entry[exit_mask]=0;peak[exit_mask]=0;block[exit_mask]=t+24*HOUR;pending[exit_mask]=False
  # Rebalance after risk exits, so cooldown cannot be bypassed on same hour.
  di=DINDEX.get(int(t//DAY*DAY-DAY))
  if t%DAY==rebalance_hour*HOUR and di is not None:
   w=daily_weights(di,eq,mode);w[t<block]=0;threshold=max(10,eq*.017)
   wanted=np.sign(w)*np.floor(np.nan_to_num(abs(w)*eq*target_gross/o)*b.F)/b.F;wanted[(abs(wanted)*np.nan_to_num(o)<threshold)|~valid]=0
   delta=wanted-qty;delta=np.sign(delta)*np.floor(abs(delta)*b.F+1e-9)/b.F
   closeout=(wanted==0)&(abs(qty)*np.nan_to_num(o)>=10);flip=qty*wanted<0;fresh=(qty==0)&(wanted!=0);resize=abs(delta)*np.nan_to_num(o)>=np.maximum(threshold,abs(wanted)*np.nan_to_num(o)*.02)
   delta=np.where(valid&(closeout|flip|fresh|resize),delta,0);delta[valid&closeout]=-qty[valid&closeout]
   if mode=='no_loser_add':delta[(qty*delta>0)&(qty*(o-entry)<0)]=0
   new=qty+delta;changed=delta!=0
   for j in np.flatnonzero(changed):
    if new[j]==0:
     holding.append((t-born[j])/HOUR);entry[j]=0;peak[j]=0;pending[j]=False
    elif qty[j]==0 or qty[j]*new[j]<0:
     if qty[j]!=0:holding.append((t-born[j])/HOUR)
     born[j]=t;entry[j]=o[j];peak[j]=o[j];sigma[j]=b.SD[di,j];pending[j]=False
     ledger.append({'t':int(t),'coin':str(b.names[j]),'kind':'entry','price':float(o[j])})
    elif abs(new[j])>abs(qty[j]):entry[j]=(entry[j]*abs(qty[j])+o[j]*abs(delta[j]))/abs(new[j])
   nt=np.sum(abs(delta)*np.nan_to_num(o));eq-=fee*nt;day_turn+=nt;qty=new
  cp=np.where(np.isfinite(close),close,op);eq+=np.nansum(qty*(cp-op));last=cp
  if eq<=0:raise ValueError('insolvent')
  # Trigger from completed hour only. Exit is queued for next hourly open.
  fired=trigger(mode,qty,entry,peak,sigma,high,low)
  due[fired&~pending]=t+exit_delay_hours*HOUR
  pending|=fired
  good=np.isfinite(high)&np.isfinite(low)
  peak=np.where((qty>0)&good,np.maximum(peak,high),np.where((qty<0)&good,np.minimum(peak,low),peak))
  if t%DAY==23*HOUR:
   days.append(int(t//DAY*DAY));daily.append(eq/day_before-1);turns.append(day_turn/day_before);gross.append(float(np.nansum(abs(qty)*cp)/eq));eqs.append(eq);day_before=eq;day_turn=0
 return {'r':np.array(daily),'turn':np.array(turns),'gross':np.array(gross),'curve':np.array(eqs),'ts':days,'missing':missing,'risk_exits':risk_exits,'closed_episode_median_hours':float(np.median(holding)) if holding else None,'open_positions':int(np.sum(qty!=0)),'ledger':ledger}

if __name__=='__main__':
 windows={'first_half':(0,(N//48)*24),'second_half':((N//48)*24,N),'recent':(int((1791417600000-start)//HOUR),N)} # Oct8 UTC
 results={};series={}
 for window,(lo,hi) in windows.items():
  results[window]={}
  for mode in VAR:
   try:
    a=sim(mode,lo=lo,hi=hi);z=b.metrics(a);z.update({k:a[k] for k in ['missing','risk_exits','closed_episode_median_hours','open_positions']});z['cost_stress']=b.metrics(sim(mode,fee=.00125,lo=lo,hi=hi));results[window][mode]=z;series[window+'__'+mode]=a['r']
    if window=='recent':(ROOT/(mode+'_recent_ledger.json')).write_text(json.dumps(a['ledger']))
    print(window,mode,'S',round(z['sharpe'],3),'ret',round(z['total']*100,2),'DD',round(z['mdd']*100,2),'exits',z['risk_exits'],'missing',z['missing'],flush=True)
   except ValueError as e:results[window][mode]={'error':str(e)}
 (ROOT/'results.json').write_text(json.dumps(results,indent=2));np.savez_compressed(ROOT/'series.npz',**series);print('DONE',flush=True)
