from pathlib import Path
import numpy as np,pandas as pd,json,math,sys
P=Path(__file__).resolve().parent;parent=P.parent
LONG="--long" in sys.argv;OUT=P/"long_2026" if LONG else P;OUT.mkdir(exist_ok=True)
ns={'__file__':str(parent/'run.py')}
source=(parent/'run.py').read_text().split('runs={};allmetrics=[]')[0]
source='\n'.join(l for l in source.splitlines() if not l.startswith('json.dump(attempts,'))
exec(source,ns)
names=ns['names'];K=len(names);dates=ns['dates'];steps=ns['steps'];DAY=86400000
start=pd.Timestamp('2026-01-01' if LONG else '2026-07-01',tz='UTC').value//10**6;end=pd.Timestamp('2026-10-01',tz='UTC').value//10**6
ix=np.flatnonzero((dates>=start)&(dates<end));LENGTH=len(ix);assert LENGTH==(273 if LONG else 92)
price={off:np.full((LENGTH,K),np.nan) for off in [0,300000,3600000]};lookup={dates[t]:i for i,t in enumerate(ix)}
for f in (P/'data').glob('*.json'):
 d=json.load(open(f))
 if d['coin'] not in names:continue
 k=names.index(d['coin'])
 assert len({r[0] for r in d['bars']})==len(d['bars'])
 for ts,v in d['bars']:
  assert np.isfinite(v) and v>0 and ts%300000==0
  day=ts//DAY*DAY;off=ts%DAY
  if day in lookup:price[off][lookup[day],k]=v
valid=np.isfinite(price[0])&np.isfinite(ns['P']['o'][ix]);diff=abs(price[0][valid]/ns['P']['o'][ix][valid]-1)
# Discrepancies are retained and reported, never silently rewritten.
bad_i,bad_j=np.where(valid & (abs(price[0]/ns['P']['o'][ix]-1)>1e-6))
(OUT/'price_discrepancies.json').write_text(json.dumps([{'date_ms':int(dates[ix[i]]),'coin':names[j],'intraday_open':float(price[0][i,j]),'daily_open':float(ns['P']['o'][ix][i,j])} for i,j in zip(bad_i,bad_j)],indent=2))
assert np.all(price[0][valid]>0)
lev=np.array([min(3,ns['meta'][n]['maxLeverage']) for n in names])
scenarios={'base':(.00075,0),'slip6':(.00105,0),'slip6_funding3':(.00105,.03)}
attempts=[{'id':f'{f}_{off}_{s}','factor':f,'offset_minutes':off//60000,'scenario':s} for f in ['MLV','MV'] for off in price for s in scenarios]
(OUT/'attempts.json').write_text(json.dumps(attempts,indent=2))
def rnd(q):return np.sign(q)*np.floor(abs(q)/steps)*steps
def simulate(factor,off,scenario,trace=None):
 fee,funding=scenarios[scenario];q=np.zeros(K);eq=3000.;prev=np.nan_to_num(price[off][0]);out=[];details=[]
 for d,t in enumerate(ix):
  valid=np.isfinite(price[off][d]);mark=np.where(valid,price[off][d],prev);before=eq
  if trace is not None:trace+=q*(mark-prev)
  eq+=q@(mark-prev)
  assert eq>0
  oldgross=abs(q*mark).sum()/eq
  # Reserve an upper bound for all old positions closed plus new gross opened.
  budget=eq/(1+fee*(oldgross+2.7)+2.7*funding/365)
  raw=rnd(np.divide(ns['targets'][factor][t-1]*2.7*budget,mark,out=np.zeros(K),where=valid))
  delta=abs((raw-q)*mark);change=valid&(delta>=max(10,.02*eq))&(delta<=np.nan_to_num(ns['V'][t-1])*.01)
  target=q.copy();target[change]=raw[change];initial=target.copy()
  for sign in [1,-1]:
   leg=target*sign>0;amount=abs(target[leg]*mark[leg]).sum()
   if amount>1.35*budget:target[leg]=rnd(target)[leg]*(1.35*budget/amount)
  target=rnd(target)
  net=(target*mark).sum()
  if abs(net)>.04*budget:
   leg=target*np.sign(net)>0;amount=abs(target[leg]*mark[leg]).sum();target[leg]*=max(0,1-(abs(net)-.04*budget)/amount);target=rnd(target)
  # Exchange-minimum-sized changes only; if dust prevents admission, flatten executable sizes.
  small=(abs((target-q)*mark)<10);target[small]=q[small];target[~valid]=q[~valid]
  def safe(x):
   after=eq-abs((x-q)*mark).sum()*fee-abs(x*mark).sum()*funding/365
   return after>0 and abs(x*mark).sum()<=2.7*after+1e-7 and abs((x*mark).sum())<=.05*after+1e-7 and (abs(x*mark)/lev).sum()<=.9*after+1e-7
  fallback=not safe(target)
  if fallback:target=q.copy();target[valid&(abs(q*mark)>=10)]=0
  repaired=not np.array_equal(initial,target)
  traded=abs((target-q)*mark).sum();cost=traded*fee
  if trace is not None:trace-=abs((target-q)*mark)*fee+abs(target*mark)*funding/365
  q=target;fund=abs(q*mark).sum()*funding/365;eq-=cost+fund
  g=abs(q*mark).sum()/eq;net=(q*mark).sum()/eq;mg=(abs(q*mark)/lev).sum()/eq;miss=abs(q[~valid]*mark[~valid]).sum()/eq
  if d==len(ix)-1:
   cost+=abs(q*mark).sum()*fee;traded+=abs(q*mark).sum();eq-=abs(q*mark).sum()*fee
   if trace is not None:trace-=abs(q*mark)*fee
  out.append([eq/before-1,traded/before,cost/before,fund/before,g,net,miss,np.count_nonzero(q)])
  details.append([repaired,fallback,mg,g>2.7+1e-7 or abs(net)>.05+1e-7 or mg>.9+1e-7,float(abs(ns['targets'][factor][t-1][~valid]).sum())]);prev=mark
 if trace is not None:assert np.isclose(trace.sum(),eq-3000,rtol=1e-10,atol=1e-8)
 return np.array(out),np.array(details)
runs={};rows=[]
attribution=[]
periods={'full':np.ones(LENGTH,bool)}
if LONG:
 periods['first_half']=dates[ix]<pd.Timestamp('2026-07-01',tz='UTC').value//10**6
 periods['third_quarter']=~periods['first_half']
for a in attempts:
 trace=np.zeros(K)
 x,d=simulate(a['factor'],a['offset_minutes']*60000,a['scenario'],trace);runs[a['id']]=x
 attribution.extend([dict(**a,coin=coin,net_pnl=float(v)) for coin,v in zip(names,trace)])
 for part,mask in periods.items():
  xx=x[mask];dd=d[mask]
  st=ns['stats'](x,mask);st.update(total_return=float(np.prod(1+xx[:,0])-1),cost_dollars_equity_fraction=float(xx[:,2].sum()),repairs=int(dd[:,0].sum()),fallbacks=int(dd[:,1].sum()),mean_margin=float(dd[:,2].mean()),max_margin=float(dd[:,2].max()),admission_violations=int(dd[:,3].sum()),missing_target_days=int((dd[:,4]>1e-9).sum()),max_missing_target_weight=float(dd[:,4].max()))
  rows.append(dict(**a,part=part,**st))
pd.DataFrame(attribution).to_csv(OUT/'attribution.csv',index=False)
pd.DataFrame(rows).to_csv(OUT/'metrics.csv',index=False);np.savez_compressed(OUT/'paths.npz',dates=dates[ix],**runs)
rng=np.random.default_rng(20261012);inference=[]
def sh(x):return x.mean()/x.std(ddof=1)*np.sqrt(365)
for part,mask in periods.items():
 for off in price:
  for scenario in scenarios:
   a=runs[f'MV_{off}_{scenario}'][mask,0];b=runs[f'MLV_{off}_{scenario}'][mask,0];obs=sh(a)-sh(b);size=len(a)
   for length in [7,15,30]:
    boot=[]
    for _ in range(1999):
     starts=rng.integers(size,size=math.ceil(size/length));ids=((starts[:,None]+np.arange(length))%size).ravel()[:size];boot.append(sh(a[ids])-sh(b[ids]))
    boot=np.array(boot);pval=(1+np.sum(boot-obs>=obs))/2000
    inference.append(dict(part=part,offset_minutes=off//60000,scenario=scenario,block=length,delta_sharpe=obs,ci_low=float(np.quantile(boot,.025)),ci_high=float(np.quantile(boot,.975)),p_one_sided=float(pval),p_bonf21=float(min(1,pval*21)),correlation=float(np.corrcoef(a,b)[0,1])))
pd.DataFrame(inference).to_csv(OUT/'inference.csv',index=False)
(OUT/'audit.json').write_text(json.dumps({'days':LENGTH,'markets':K,'matching_daily_intraday_observations':int(len(diff)),'max_daily_open_relative_difference':float(diff.max()),'available_intraday_points':{str(k):int(np.isfinite(v).sum()) for k,v in price.items()},'all_paths_finite':all(np.isfinite(v).all() for v in runs.values()),'held_missing_price_days':int(sum(r['unpriced_days'] for r in rows if r['part']=='full'))},indent=2))
print(pd.DataFrame(rows)[['factor','offset_minutes','scenario','part','total_return','sharpe','drawdown','repairs','fallbacks','admission_violations']].to_string(index=False))
