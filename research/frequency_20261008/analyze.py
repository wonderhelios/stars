"""Short-sample diagnostic ONLY: closed UTC daily signals, hourly execution.
No live code imports or edits. All distinct hourly phases; cost-dependent equity.
"""
import pathlib,json,datetime,hashlib
import numpy as np,pandas as pd
O=pathlib.Path(__file__).resolve().parent;H=3600000;D=24*H
coverage=json.loads((O/'coverage.json').read_text());names=sorted(x['coin'] for x in coverage if x['liquid']);K=len(names)
# Current liquid selection is intentional download universe; survivor bias disclosed.
start=int(pd.Timestamp('2026-03-11',tz='UTC').timestamp()*1000);end=int(pd.Timestamp('2026-10-03',tz='UTC').timestamp()*1000)
ts=np.arange(start,end+H,H);N=len(ts)-1;prices=np.full((len(ts),K),np.nan)
inputs={}
for j,c in enumerate(names):
 f=pathlib.Path('/tmp/hl-hist')/(c+'.json');inputs[str(f)]=hashlib.sha256(f.read_bytes()).hexdigest()
 for x in json.loads(f.read_text()):
  t=int(x['t']);i=(t-start)//H
  if start<=t<=end and (t-start)%H==0:prices[i,j]=float(x['o'])
# Daily factors reproduce trader.rs, including dvol=close*volume, prior-30 liquidity.
raw={c:json.loads((pathlib.Path('/tmp/hl-daily-full')/(c+'.json')).read_text()) for c in names}
days=np.array(sorted({int(x['t']) for v in raw.values() for x in v}));di={t:i for i,t in enumerate(days)}
P=np.full((len(days),K),np.nan);Q=P.copy()
for j,c in enumerate(names):
 f=pathlib.Path('/tmp/hl-daily-full')/(c+'.json');inputs[str(f)]=hashlib.sha256(f.read_bytes()).hexdigest()
 for x in raw[c]:
  i=di[int(x['t'])];P[i,j]=float(x['c']);Q[i,j]=float(x['v'])*float(x['c'])
R=P/np.roll(P,1,axis=0)-1;R[0]=np.nan
V=pd.DataFrame(Q).rolling(30,min_periods=5).mean().shift(1).to_numpy();SD=pd.DataFrame(R).rolling(20,min_periods=5).std(ddof=1).to_numpy();M=P/np.roll(P,14,axis=0)-1
W=np.zeros_like(P);eligible=[]
for i in range(32,len(days)):
 ok=np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])&(V[i]>=5e6);idx=np.flatnonzero(ok)
 if len(idx)<8:continue
 k=min(8,max(1,int(np.floor(len(idx)*.2+.5))),len(idx)//2);acc=np.zeros(K)
 for s in [M[i]/np.maximum(SD[i],1e-9),-SD[i],Q[i]/V[i]]:
  order=idx[np.lexsort((np.array(names)[idx],s[idx]))];acc[order[:k]]-=.5/k/3;acc[order[-k:]]+=.5/k/3
 if np.abs(acc).sum()>1e-12:W[i]=acc/np.abs(acc).sum()
assert np.max(np.abs(W.sum(axis=1)))<1e-10
weights=np.array([W[di[(int(t)//D-1)*D]] for t in ts[:-1]])
# Any missing needed target or held mark is an error; never forward fill.
assert not np.any((np.abs(weights)>1e-12)&~np.isfinite(prices[:-1])),'missing selected target price'
returns=np.nan_to_num(prices[1:]/prices[:-1]-1,nan=0.0)
SLIPS=[.0002,.0003,.0005,.0008,.0012,.002];PERIODS={'1x':24,'1.5x':16,'2x':12,'3x':8,'4x':6}
def sharpe(r):
 r=np.asarray(r);return np.mean(r,axis=-1)/np.std(r,axis=-1,ddof=1)*np.sqrt(365)
def sim(period,slip):
 phases=np.arange(period);pos=np.zeros((period,K));eq=np.ones(period);hr=np.zeros((period,N));gross=hr.copy();turn=hr.copy();ex=hr.copy()
 for h in range(N):
  assert not np.any((np.abs(pos)>1e-12)&~np.isfinite(prices[h])),'missing held price'
  g=np.abs(pos).sum(axis=1);ex[:,h]=np.divide(np.abs(pos.sum(axis=1)),g,out=np.zeros(period),where=g>0)
  pre=eq.copy();trade=(int(ts[h]//H)%period)==phases
  target=2.7*eq[:,None]*weights[h];delta=target-pos
  # Existing live rebalance band; no position-size dollar thresholds without account equity.
  mask=(np.abs(delta)>=.02*np.abs(target))|((target==0)&(pos!=0))|(pos*target<0)
  delta=np.where(mask&trade[:,None],delta,0);pos+=delta
  turn[:,h]=np.abs(delta).sum(axis=1)/pre;cost=(.00045+slip)*turn[:,h]*pre
  assert not np.any((np.abs(pos)>1e-12)&~np.isfinite(prices[h+1])),'missing next held price'
  pnl=(pos*returns[h]).sum(axis=1);eq+=pnl-cost
  assert np.min(eq)>0,'bankrupt'
  hr[:,h]=(pnl-cost)/pre;gross[:,h]=pnl/pre;pos*=1+returns[h]
 # Uniform terminal taker exit; do not count uncharged final inventory.
 final=np.abs(pos).sum(axis=1)/pre;hr[:,-1]-=(.00045+slip)*final;turn[:,-1]+=final
 eq-=(.00045+slip)*final*pre
 assert np.allclose(np.prod(1+hr,axis=1),eq,rtol=1e-10,atol=1e-10),'equity reconciliation'
 assert np.isfinite(hr).all()
 return {'r':np.prod(1+hr.reshape(period,-1,24),axis=2)-1,'gross':gross.reshape(period,-1,24).sum(axis=2),'turn':turn.reshape(period,-1,24).sum(axis=2),'ex':ex}
assert N%24==0
runs={};summary={};daily_dates=pd.to_datetime(ts[:-1:24],unit='ms',utc=True).astype(str).tolist();L=N//24
splits={'full':np.arange(L),'segment_1':np.arange(0,L//3),'segment_2':np.arange(L//3,2*L//3),'segment_3':np.arange(2*L//3,L)}
for name,period in PERIODS.items():
 for s in SLIPS:
  key=f'{name}_{s:.4f}';v=sim(period,s);runs[key]=v;ss={}
  for label,ix in splits.items():
   sh=sharpe(v['r'][:,ix]);to=v['turn'][:,ix].mean()*365;gr=v['gross'][:,ix].mean()*365;cost=to*(.00045+s)
   ss[label]=dict(sharpe=float(sh.mean()),phase_sharpes=sh.tolist(),phase_sd=float(sh.std()),phase_range=float(np.ptp(sh)),annual_turnover=float(to),annual_cost=float(cost),annual_gross_arithmetic=float(gr),cost_over_gross=float(cost/gr) if gr>0 else None,annual_net_arithmetic=float(v['r'][:,ix].mean()*365))
  e=v['ex'].flatten();ss['exposure']=dict(mean=float(e.mean()),p90=float(np.quantile(e,.9)),max=float(e.max()))
  summary[key]=ss;print(key,ss['full']['sharpe'],flush=True)
# Daily Sharpe influence function, phase-mean statistic, paired centred circular block maxT.
# One family: 4 alternatives * 6 slips * (full+3 segments) =96 hypotheses.
def influence(r):
 mu=r.mean(axis=1,keepdims=True);sd=r.std(axis=1,ddof=0,keepdims=True)
 return (np.sqrt(365)*((r-mu)/sd-mu/(2*sd**3)*((r-mu)**2-sd**2))).mean(axis=0)
hyp=[]
for label,ix in splits.items():
 for name in list(PERIODS)[1:]:
  for s in SLIPS:
   key=f'{name}_{s:.4f}';base=f'1x_{s:.4f}';x=influence(runs[key]['r'][:,ix])-influence(runs[base]['r'][:,ix]);obs=summary[key][label]['sharpe']-summary[base][label]['sharpe']
   hyp.append((key,label,x-x.mean(),obs))
boot={};B=4999
for block in [7,14,30]:
 rng=np.random.default_rng(20261008+block);draws=[]
 # Common pseudo-time draws across all hypotheses, including segments.
 seeds=rng.integers(0,L,size=(B,int(np.ceil(L/block))))
 for key,label,x,obs in hyp:
  n=len(x);indices=((seeds[:,:int(np.ceil(n/block))]%n)[:,:,None]+np.arange(block))%n;indices=indices.reshape(B,-1)[:,:n]
  noise=x[indices].mean(axis=1);se=noise.std(ddof=1);draws.append(noise/max(se,1e-12))
 maxnull={label:np.max([z for item,z in zip(hyp,draws) if item[1]==label],axis=0) for label in splits};rows=[]
 for (key,label,x,obs),z in zip(hyp,draws):
  se=np.std(z,ddof=1) # z already standardized; recover bootstrap SE from exact draw variance separately
  n=len(x);indices=((seeds[:,:int(np.ceil(n/block))]%n)[:,:,None]+np.arange(block))%n;noise=x[indices.reshape(B,-1)[:,:n]].mean(axis=1);sd=noise.std(ddof=1)
  rows.append(dict(key=key,segment=label,delta_sharpe=obs,bootstrap_se=float(sd),p_fwer=float(min(1,4*(1+(maxnull[label]>=obs/max(sd,1e-12)).sum())/(B+1))),ci95=[float(obs-np.quantile(noise,.975)),float(obs-np.quantile(noise,.025))]))
 boot[str(block)]=rows
meta=dict(status='SHORT_SAMPLE_DIAGNOSTIC_NOT_VALIDATION',days=L,start=daily_dates[0],last_day=daily_dates[-1],universe=names,phase_counts={k:v for k,v in PERIODS.items()},cost_frequency_configurations=len(runs),phase_simulations=sum(PERIODS.values())*len(SLIPS),hypotheses=len(hyp),bootstrap_replicates=B,inputs_sha256=inputs,segments={k:[daily_dates[v[0]],daily_dates[v[-1]],len(v)] for k,v in splits.items()})
(O/'results.json').write_text(json.dumps(dict(meta=meta,summary=summary,bootstrap=boot),indent=2));np.savez_compressed(O/'daily_returns.npz',**{k:v['r'] for k,v in runs.items()})
print('done',meta['hypotheses'],flush=True)
