"""Exploratory revised-vintage tests. NOT point-in-time certified. Fixed family before outcomes."""
import engine as e
import numpy as np,pandas as pd,json,hashlib,math,warnings
from pathlib import Path
warnings.filterwarnings('ignore',category=RuntimeWarning)
OUT=Path(__file__).resolve().parent;D=OUT/'data'
N=len(e.ix);mask={'full':np.ones(N,bool),'2023-24':e.outdates.year<=2024,'2025-26':e.outdates.year>=2025}
# Fixed economic mapping, not selected on observed returns. Multichain tokens mostly omitted.
mapping={'Ethereum':['ETH','AAVE','UNI','LDO','CRV','ENS','MKR','COMP'], 'Solana':['SOL','JUP','JTO','RAY'], 'Arbitrum':['ARB','GMX'], 'Optimism':['OP'], 'Avalanche':['AVAX','JOE'], 'BSC':['BNB','CAKE','XVS'], 'Polygon':['MATIC','POL','QUICK'], 'Sui':['SUI','CETUS'], 'Aptos':['APT'], 'Near':['NEAR'], 'Cosmos':['ATOM','OSMO'], 'Cardano':['ADA'], 'Fantom':['FTM'], 'Tron':['TRX'], 'Injective':['INJ'], 'Sei':['SEI'], 'Mantle':['MNT'], 'Ton':['TON']}
json.dump(mapping,open(OUT/'mapping.json','w'),indent=2)
audit={}
def series(key,field,tokens=False):
 try:
  j=json.load(open(D/(key+'.json')));j=j['tokens'] if tokens else j
  if not isinstance(j,list):raise ValueError('not historical list')
  vals={pd.Timestamp(int(v['date']),unit='s',tz='UTC').normalize():float(v[field].get('peggedUSD',np.nan) if isinstance(v.get(field),dict) else v.get(field,np.nan)) for v in j}
  s=pd.Series(vals).sort_index();s=s.where(s>0)
  audit[key]={'first':str(s.first_valid_index()),'last':str(s.last_valid_index()),'observations':int(s.notna().sum()),'duplicate_dates':len(j)-len(vals),'historical_availability_known':False}
  # Forward fill maximum one day, never backfill; growth needs all dates spanned.
  return s.reindex(e.dates).ffill(limit=1)
 except Exception as ex:
  audit[key]={'error':str(ex),'historical_availability_known':False};return pd.Series(np.nan,index=e.dates)

supply={z:series('stable_'+z,'circulating',True) for z in ['usdt','usdc']}
supply['sum']=supply['usdt']+supply['usdc'];supply['all']=series('stable_all','totalCirculating')
tvl={c:series('tvl_'+c,'tvl') for c in mapping};stable={c:series('stable_'+c,'totalCirculating') for c in mapping}
base,_,univ=e.targets(); Ts={'baseline':base};scales={};ready={};meta_candidates={};scores={};coverage={};corr={}
# No >3x setting. High relative growth keeps current 3x, low halves to 1.5x.
# Causal 180-day expanding/rolling median of prior available observations, min60.
for lag in [2,7]:
 for h in [14,30]:
  for src,s in supply.items():
   g=np.log(s/s.shift(h)).shift(lag);threshold=g.rolling(180,min_periods=60).median().shift(1)
   scale=np.where(np.isfinite(g)&np.isfinite(threshold),np.where(g>=threshold,1.,.5),1.)
   z=f'timing_{src}_{h}_lag{lag}';scales[z]=scale[e.ix];ready[z]=(np.isfinite(g)&np.isfinite(threshold)).values[e.ix];Ts[z]=base
   meta_candidates[z]={'kind':'timing','horizon':h,'lag':lag,'source':src,'unavailable_signal_days':int((~np.isfinite(g.iloc[e.ix])).sum())}
  for kind in ['tvl','tvl_native_adjusted','stable_share']:
   score=np.full_like(base,np.nan)
   for c,coins in mapping.items():
    if kind=='stable_share':v=stable[c]/supply['all']
    else:v=tvl[c]
    g=np.log(v/v.shift(h))
    if kind=='tvl_native_adjusted':
     native=coins[0]
     if native not in e.names:continue
     price=pd.Series(e.P[:,e.names.index(native)],index=e.dates)
     g-=np.log(price/price.shift(h))
    g=g.shift(lag).values
    for coin in coins:
     if coin in e.names:score[:,e.names.index(coin)]=g
   z=f'{kind}_{h}_lag{lag}';scores[z]=score

def tiebook(idx,s):
 good=idx[np.isfinite(s[idx])];w=np.zeros(e.k)
 if len(good)<8 or len(np.unique(s[good]))<4:return w
 kk=min(5,max(1,int(np.floor(len(good)*.2+.5))),len(good)//2)
 # Fractionally allocate ties at leg boundary; never alphabetically prefer ecosystems.
 for sign in [-1,1]:
  remaining=float(kk)
  for level in sorted(set(s[good]),reverse=sign==1):
   group=good[s[good]==level];take=min(remaining,len(group));w[group]+=sign*.5/kk*take/len(group);remaining-=take
   if remaining<=0:break
 return w
for z,s in scores.items():
 b=np.zeros_like(base);cv=[];cr=[]
 for i in range(32,e.n-1):
  idx=np.flatnonzero((e.V[i]>=5e6)&np.isfinite(e.P[i])&np.isfinite(e.P[i-14])&np.isfinite(e.SD[i])&np.array([':' not in c for c in e.names]))
  b[i]=tiebook(idx,s[i]);good=idx[np.isfinite(s[i,idx])];cv.append(len(good))
  if len(good)>=8 and np.std(s[i,good])>0:
   cr.append(pd.Series(s[i,good]).rank().corr(pd.Series(e.M[i,good]/np.maximum(e.SD[i,good],1e-9)).rank()))
 Ts['solo_'+z]=b
 mix=.8*base+.2*b;gross=np.abs(mix).sum(1);Ts['blend_'+z]=np.divide(mix,gross[:,None],out=np.zeros_like(mix),where=gross[:,None]>0)
 coverage[z]={'mean_mapped_liquid_coins':float(np.mean(cv)),'active_fraction':float(np.mean(np.abs(b[e.ix]).sum(1)>0))};corr[z]=float(np.nanmean(cr))
 for prefix in ['solo_','blend_']:meta_candidates[prefix+z]={'kind':'cross_section','score':z}
assert len(meta_candidates)==40
json.dump(meta_candidates,open(OUT/'candidates.json','w'),indent=2)

# Full book resized on total-exposure change. Ordinary signal changes retain all three slice phases.
def run(T,phase=0,scale=None,lev=2.7,fee=.00075,penalty=0):
 w=np.zeros(e.k);prev=1.;out={z:[] for z in ['r','turn','gross','net','missing']}
 for j,i in enumerate(e.ix):
  rr=np.nan_to_num(e.OP[i+1]/e.OP[i]-1) if j else np.zeros(e.k);gain=w@rr
  assert 1+gain>0
  w=w*(1+rr)/(1+gain);s=1. if scale is None else scale[j]
  new=w*s/prev if s!=prev else w.copy();target=T[i]*lev*s
  slot=((e.times[i]//e.DAY+1+phase+e.hashes)%3)==0
  delta=target-new;use=slot&((np.abs(delta)>=.02*np.abs(target))|(target*new<=0));new=np.where(use,target,new)
  valid=np.isfinite(e.a['o'][i+1]);missing=np.abs(w[~valid]).sum();new[~valid]=0
  turn=np.abs(new-w).sum();cost=turn*fee+missing*penalty;assert cost<1
  w=new/(1-cost);out['r'].append((1+gain)*(1-cost)-1);out['turn'].append(turn);out['gross'].append(np.abs(w).sum());out['net'].append(w.sum());out['missing'].append(missing);prev=s
 terminal=np.abs(w).sum();out['r'][-1]=(1+out['r'][-1])*(1-terminal*fee)-1;out['turn'][-1]+=terminal
 return {z:np.array(v) for z,v in out.items()}
runs={z:[run(t,p,scales.get(z)) for p in range(3)] for z,t in Ts.items()};matched={};matched_lev={}
for z in scales:
 matched[z]=[];matched_lev[z]=[]
 for p in range(3):
  lev=2.7*runs[z][p]['gross'].mean()/runs['baseline'][p]['gross'].mean()
  for _ in range(4):
   q=run(base,p,lev=lev);lev*=runs[z][p]['gross'].mean()/q['gross'].mean()
  matched[z].append(run(base,p,lev=lev));matched_lev[z].append(lev)
# Matched constant controls use ex-post mean gross ONLY as diagnostic, not tradable inference.
def summarize(rr,fee=.00075):
 out={}
 for period,m in mask.items():
  ss=[e.stat(v['r'][m]) for v in rr];q={a:float(np.mean([s[a] for s in ss])) for a in ss[0]}
  q.update(phase_sharpes=[s['sharpe'] for s in ss],phase_sd=float(np.std([s['sharpe'] for s in ss])),phase_range=float(np.ptp([s['sharpe'] for s in ss])),turnover_equity=float(np.mean([v['turn'][m].mean() for v in rr])),mean_gross=float(np.mean([v['gross'][m].mean() for v in rr])),mean_abs_net=float(np.mean([np.abs(v['net'][m]).mean() for v in rr])),correlation_baseline=float(np.mean([np.corrcoef(v['r'][m],runs['baseline'][p]['r'][m])[0,1] for p,v in enumerate(rr)])))
  q['annual_cost']=q['turnover_equity']*fee*365;out[period]=q
 return out
summary={z:summarize(rr) for z,rr in runs.items()};control_summary={z:summarize(rr) for z,rr in matched.items()}
# No overlapping horizon events. Daily market conditional mean difference tested with calendar blocks.
market=np.zeros(N)
for j,i in enumerate(e.ix):
 ok=(e.V[i-1]>=5e6)&np.isfinite(e.a['o'][i])&np.isfinite(e.a['o'][i+1]);market[j]=np.mean((e.a['o'][i+1]/e.a['o'][i]-1)[ok]) if ok.sum() else 0
# scale at prior execution determines forward return ending today.
market_stats={};market_IF={}
for z,scale in scales.items():
 signal=np.r_[np.nan,np.where(ready[z],scale,np.nan)[:-1]];high=signal==1.;low=signal==.5
 def diff(m):
  hi=high&m;lo=low&m
  return {'high_days':int(hi.sum()),'low_days':int(lo.sum()),'daily_return_high':float(market[hi].mean()),'daily_return_low':float(market[lo].mean()),'daily_high_minus_low':float(market[hi].mean()-market[lo].mean())}
 market_stats[z]={p:diff(m) for p,m in mask.items()}
 # IF for unconditional calendar means ratio, preserving serial dependence/market exposure.
 market_IF[z]=high*(market-market[high].mean())/high.mean()-low*(market-market[low].mean())/low.mean()

def sif(r):
 mu=r.mean();sd=r.std(ddof=1)
 return np.sqrt(365)*((r-mu)/sd-mu*((r-mu)**2-sd**2)/(2*sd**3))
# 40 vs base +16 vs constant +16 market =72 comparisons in one family (market claims included).
B=4999;bootstrap={};rng=np.random.default_rng(20261008)
for period,m in mask.items():
 labels=[];obs=[];IF=[]
 bi=np.mean([sif(v['r'][m]) for v in runs['baseline']],axis=0)
 for z in meta_candidates:
  labels.append(z+'__baseline');obs.append(summary[z][period]['sharpe']-summary['baseline'][period]['sharpe']);zi=np.mean([sif(v['r'][m]) for v in runs[z]],axis=0);IF.append(zi-bi)
  if z in matched:
   labels.append(z+'__constant');obs.append(summary[z][period]['sharpe']-control_summary[z][period]['sharpe']);IF.append(zi-np.mean([sif(v['r'][m]) for v in matched[z]],axis=0))
 for z in scales:
  mm=market[m];sig=np.r_[np.nan,np.where(ready[z],scales[z],np.nan)[:-1]][m];hi=sig==1;lo=sig==.5
  labels.append(z+'__market');obs.append(market_stats[z][period]['daily_high_minus_low']);IF.append(hi*(mm-mm[hi].mean())/hi.mean()-lo*(mm-mm[lo].mean())/lo.mean())
 obs=np.array(obs);IF=np.array(IF).T;IF-=IF.mean(0);NN=len(IF);bootstrap[period]={}
 assert len(labels)==72
 for L in [15,30,60]:
  vals=[]
  for start in range(0,B,200):
   batch=min(200,B-start);starts=rng.integers(NN,size=(batch,math.ceil(NN/L)));inds=((starts[:,:,None]+np.arange(L))%NN).reshape(batch,-1)[:,:NN];counts=np.array([np.bincount(a,minlength=NN) for a in inds]);vals.append(counts@IF/NN)
  vals=np.concatenate(vals);se=np.maximum(vals.std(0,ddof=1),1e-12);mx=(vals/se).max(1)
  bootstrap[period][str(L)]={z:{'effect':float(obs[j]),'se':float(se[j]),'ci95':(obs[j]+np.quantile(vals[:,j],[.025,.975])).tolist(),'p_fwer':float((1+(mx>=obs[j]/se[j]).sum())/(B+1))} for j,z in enumerate(labels)}
 print('BOOT',period,flush=True)
stress={}
for label,fee,pen in [('maker_optimistic',.00045,0),('slip6bp',.00105,0),('missing5pct',.00075,.05)]:
 stress[label]={z:summarize([run(t,p,scales.get(z),fee=fee,penalty=pen) for p in range(3)],fee) for z,t in Ts.items()}
 print('STRESS',label,flush=True)
qualified=[]
for z in meta_candidates:
 comparators=['baseline']+(['constant'] if z in matched else [])
 good=all(summary[z][p]['sharpe']>summary['baseline'][p]['sharpe'] and (z not in matched or summary[z][p]['sharpe']>control_summary[z][p]['sharpe']) for p in ['2023-24','2025-26'])
 sig=all(bootstrap[p][str(L)][z+'__'+c]['p_fwer']<.05 for p in mask for L in [15,30,60] for c in comparators)
 if good and sig:qualified.append(z)
for T in Ts.values():assert np.max(np.abs(T.sum(1)))<1e-10 and np.isfinite(T).all()
meta={'candidate_count':40,'timing_candidates':16,'cross_section_blends':12,'cross_section_solo':12,'family_comparisons':72,'bootstrap_reps':B,'blocks':[15,30,60],'dates':[str(e.outdates[0]),str(e.outdates[-1])],'period_days':{p:int(m.sum()) for p,m in mask.items()},'reference_sharpe':1.79,'historical_vintage':'Current revised API snapshot. Data date+lag is assumed availability, NOT demonstrated availability. All candidates fail PIT certification.','funding':'Excluded, incomplete 2023-26 coverage; net denotes after execution costs only.','execution':'Next UTC open proxy; physical holdings/equity drift; 2% band; all three phases; large account ignores minimum orders/precision/liquidations; terminal taker close; missing price exits at stale marks.','matched_controls':'Constant exposure solved ex-post to match actual mean gross. Diagnostic only; no return optimization.','mapping':'Fixed current economically motivated mapping; not historical archived membership. Survivorship/protocol cross-chain exposure remain.','market':'PIT liquidity-filtered equal weight native HL perps, NOT global investable crypto index; high/low conditional next daily open return, no multi-day overlap. Missing warmup treated as high for allocation, excluded from predictive high/low test below.','input_hashes':{Path(f).name:hashlib.sha256(Path(f).read_bytes()).hexdigest() for f in e.FILES},'source_trader_sha256':hashlib.sha256(Path('/Users/wonder/Code/stars/src/trader.rs').read_bytes()).hexdigest(),'missing_exposure':{z:float(max(v['missing'].max() for v in rr)) for z,rr in runs.items()}}
# Availability is a HARD rejection regardless of nominal statistical survivors.
json.dump(dict(meta=meta,data_audit=audit,candidates=meta_candidates,summary=summary,matched=control_summary,matched_leverage=matched_lev,coverage=coverage,score_rank_correlation_momentum=corr,market_prediction=market_stats,bootstrap=bootstrap,stress=stress,statistical_survivors=qualified,live_qualified=[]),open(OUT/'results.json','w'),indent=2)
pd.DataFrame([dict(candidate=z,period=p,**s) for z,v in summary.items() for p,s in v.items()]).to_csv(OUT/'metrics.csv',index=False)
np.savez_compressed(OUT/'returns.npz',dates=np.asarray(e.outdates.astype(str),dtype='U'),market=market,**{z:np.array([v['r'] for v in rr]) for z,rr in runs.items()},**{'matched_'+z:np.array([v['r'] for v in rr]) for z,rr in matched.items()})
pd.DataFrame({'signal_day':e.dates[e.ix].astype(str),'execution_day':e.outdates.astype(str),**scales,**{'ready_'+z:v for z,v in ready.items()}}).to_csv(OUT/'timing_signals.csv',index=False)
print('BASELINE',[summary['baseline'][p]['sharpe'] for p in mask]);print('STATISTICAL_SURVIVORS',qualified)
