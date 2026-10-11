import json,sqlite3,hashlib,itertools,math
from pathlib import Path
import numpy as np,pandas as pd
ROOT=Path(__file__).resolve().parents[2]; OUT=Path(__file__).resolve().parent
DB=ROOT/'research/txflow_binance_20261009/binance-candles.sqlite'
DAY=86400000
meta={m['name']:m for m in json.load(open(OUT/'market_snapshot.json'))['universe'] if not m.get('haltTrading') and not m.get('delisted') and not m.get('onlyIsolated')}
white={m['txflow_name'] for m in json.load(open(ROOT/'config/binance-mapping.json'))}
with sqlite3.connect(f'file:{DB}?mode=ro',uri=True) as c: frame=pd.read_sql_query('select * from candles order by t,coin',c)
frame=frame[frame.coin.isin(white&meta.keys())]; names=sorted(frame.coin.unique()); dates=np.arange(frame.t.min(),frame.t.max()+DAY,DAY);N=len(dates);K=len(names)
P={key:frame.pivot(index='t',columns='coin',values=key).reindex(index=dates,columns=names).values for key in ['o','c','v']}
assert np.all(np.isfinite(frame[['o','c','v']].values)) and (frame.o>0).all() and (frame.c>0).all()
assert not frame.duplicated(['coin','t']).any()
Q=P['c']*P['v'];R=pd.DataFrame(P['c']).pct_change(fill_method=None).values
V=pd.DataFrame(Q).rolling(30,min_periods=5).mean().shift().values
SD=pd.DataFrame(R).rolling(20,min_periods=5).std().values
M=P['c']/pd.DataFrame(P['c']).shift(14).values-1
scores={'M':M/np.maximum(SD,1e-9),'L':-SD,'V':Q/V}
books={key:np.zeros((N,K)) for key in scores}; universe=[]
for t in range(32,N):
 ok=(V[t]>=5e6)&np.isfinite(M[t])&np.isfinite(SD[t]);ix=np.flatnonzero(ok);universe.append(len(ix))
 if len(ix)<8:continue
 k=min(max(1,int(np.floor(len(ix)*.2+.5))),8,len(ix)//2)
 for key,A in scores.items():
  order=ix[np.lexsort((np.array(names)[ix],A[t,ix]))]
  books[key][t,order[:k]]=-.5/k;books[key][t,order[-k:]]=.5/k
combos=['MLV','M','L','V','ML','MV','LV'];targets={}
for combo in combos:
 a=sum(books[c] for c in combo)/len(combo);gross=abs(a).sum(1);a=np.divide(a,gross[:,None],out=np.zeros_like(a),where=gross[:,None]>1e-12);targets[combo]=a
 assert np.max(abs(a.sum(1)))<1e-10
steps=np.array([10.**(-int(meta[n]['szDecimals'])) for n in names]);hashes=np.array([sum(n.encode()) for n in names]);lastprice=pd.DataFrame(P['o']).ffill().values
indices=np.arange(34,N);outdates=pd.to_datetime(dates[indices],unit='ms',utc=True)
scenarios={'base':(.00075,0.,2),'slip6':(.00105,0.,2),'maker_upper':(.00045,0.,2),'funding3pct':(.00075,.03,2),'next_open_upper':(.00075,0.,1)}
attempts=[dict(id=f'{c}_{period}_{phase}_{case}',family=f'{c}_{period}',factor=c,period=period,phase=phase,scenario=case) for c in combos for period in [1,3,4] for phase in range(period) for case in scenarios]
json.dump(attempts,open(OUT/'attempts.json','w'),indent=2)

def run(T,period,phase,case):
 fee,funding,lag=scenarios[case];q=np.zeros(K);eq=3000.;previous=np.nan_to_num(lastprice[indices[0]-1]);rows=[]
 for t in indices:
  before=eq;mark=np.where(np.isfinite(lastprice[t]),lastprice[t],previous);eq+=q@(mark-previous)
  if eq<=0: rows.append([-1.,0.,0.,0.,0.,0.,0.,0.]);q[:]=0;break
  valid=np.isfinite(P['o'][t])&(P['o'][t]>0);target=np.divide(T[t-lag]*2.7*eq,mark,out=np.zeros(K),where=valid)
  target=np.sign(target)*np.floor(abs(target)/steps)*steps
  delta=target-q;dollars=abs(delta*mark)
  slot=(hashes%period)==((dates[t]//DAY+phase)%period)
  # All-at-once taker proxy. Actual sequential exchange admission is NOT simulated.
  change=valid&slot&(dollars>=max(10.,eq*.02))&(dollars<=np.nan_to_num(V[t-lag])*.01)
  traded=float(dollars[change].sum());q[change]=target[change];cost=traded*fee
  notional=q*mark;fund=abs(notional).sum()*funding/365;eq-=cost+fund
  if t==indices[-1]:
   close=float(abs(notional).sum());eq-=close*fee;cost+=close*fee;traded+=close
  gross=abs(notional).sum()/max(eq,1e-9);net=notional.sum()/max(eq,1e-9);missing=abs(notional[~valid]).sum()/max(eq,1e-9)
  rows.append([eq/before-1,traded/before,cost/before,fund/before,gross,net,missing,float(np.count_nonzero(q))]);previous=mark
  if eq<=0:break
 if len(rows)<len(indices):rows.extend([[0.]*8]*(len(indices)-len(rows)))
 return np.array(rows)

def stats(a,mask):
 a=a[mask];r=a[:,0];curve=np.cumprod(1+r);peak=np.maximum.accumulate(np.r_[1.,curve])[1:];sd=r.std(ddof=1)
 return dict(sharpe=float(r.mean()/sd*np.sqrt(365)) if sd>0 else 0.,annual=float(r.mean()*365),cagr=float(curve[-1]**(365/len(r))-1) if curve[-1]>0 else -1.,drawdown=float((curve/peak-1).min()),turnover=float(a[:,1].mean()*365),annual_cost=float(a[:,2].mean()*365),funding_cost=float(a[:,3].mean()*365),gross=float(a[:,4].mean()),risk_violation_days=int(((a[:,4]>3)|(abs(a[:,5])>.1)).sum()),unpriced_days=int((a[:,6]>1e-8).sum()))
masks={'full':np.ones(len(indices),bool),'train2024':outdates.year==2024,'valid2025':outdates.year==2025,'diagnostic2026':outdates.year==2026,'late2025_26':outdates.year>=2025}
runs={};allmetrics=[]
for n,a in enumerate(attempts):
 x=run(targets[a['factor']],a['period'],a['phase'],a['scenario']);runs[a['id']]=x
 for part,mask in masks.items():allmetrics.append(dict(**a,part=part,**stats(x,mask)))
 if n%40==0:print('completed',n,len(attempts),flush=True)
metrics=pd.DataFrame(allmetrics);metrics.to_csv(OUT/'metrics.csv',index=False)
summary=metrics.groupby(['family','scenario','part']).agg(sharpe=('sharpe','mean'),phase_min=('sharpe','min'),phase_max=('sharpe','max'),annual=('annual','mean'),cagr=('cagr','mean'),drawdown=('drawdown','mean'),turnover=('turnover','mean'),annual_cost=('annual_cost','mean'),gross=('gross','mean'),risk_days=('risk_violation_days','max'),unpriced_days=('unpriced_days','max')).reset_index();summary.to_csv(OUT/'summary.csv',index=False)
train=summary[(summary.scenario=='base')&(summary.part=='train2024')].sort_values('sharpe',ascending=False);selected=train.iloc[0].family
json.dump({'selection_only_2024':selected,'days':{k:int(v.sum()) for k,v in masks.items()},'markets':len(names),'rows':len(frame),'start':str(outdates[0]),'end':str(outdates[-1]),'universe_mean':float(np.mean(universe)),'initial_equity':3000,'attempts':len(attempts),'families':21,'phase_paths':56,'db_sha256':hashlib.sha256(DB.read_bytes()).hexdigest(),'metadata_sha256':hashlib.sha256((OUT/'market_snapshot.json').read_bytes()).hexdigest(),'mapping_sha256':hashlib.sha256((ROOT/'config/binance-mapping.json').read_bytes()).hexdigest()},open(OUT/'manifest.json','w'),indent=2)
np.savez_compressed(OUT/'paths.npz',dates=dates[indices],**runs)
print('Selected using 2024 only:',selected,flush=True)
print(summary[(summary.scenario=='base')&(summary.family.isin(['MLV_1',selected]))].to_string(index=False),flush=True)
# Influence-function paired circular block maxT for mean phase Sharpe differences.
def sif(r):
 mu=r.mean();sd=max(r.std(ddof=1),1e-12);return np.sqrt(365)*((r-mu)/sd-mu*((r-mu)**2-sd**2)/(2*sd**3))
families=sorted(set(a['family'] for a in attempts));results={};rng=np.random.default_rng(20261011)
for part,mask in masks.items():
 br=runs['MLV_1_0_base'][mask,0];bs=stats(runs['MLV_1_0_base'],mask)['sharpe'];bi=sif(br)
 others=[f for f in families if f!='MLV_1'];D=[];obs=[]
 for f in others:
  paths=[runs[a['id']] for a in attempts if a['family']==f and a['scenario']=='base'];D.append(np.mean([sif(x[mask,0]) for x in paths],axis=0)-bi);obs.append(np.mean([stats(x,mask)['sharpe'] for x in paths])-bs)
 D=np.array(D).T;obs=np.array(obs);T=len(D);D-=D.mean(0);result={}
 for length in [15,30,60]:
  boot=[]
  for _ in range(1999):
   starts=rng.integers(T,size=math.ceil(T/length));ids=((starts[:,None]+np.arange(length))%T).ravel()[:T];boot.append(D[ids].mean(0))
  boot=np.array(boot);se=np.maximum(boot.std(0,ddof=1),1e-12);mx=(boot/se).max(1);result[str(length)]={f:{'delta_sharpe':float(obs[j]),'p_fwer':float((1+(mx>=obs[j]/se[j]).sum())/2000)} for j,f in enumerate(others)}
 results[part]=result
 print('inference',part,flush=True)
json.dump(results,open(OUT/'inference.json','w'),indent=2)
