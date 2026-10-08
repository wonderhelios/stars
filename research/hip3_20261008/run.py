import os,json,glob,hashlib,warnings
from pathlib import Path
import numpy as np,pandas as pd,exchange_calendars as xc
from fetch import STOCKS,COINS,END
warnings.filterwarnings('ignore',category=RuntimeWarning)
ROOT=Path(__file__).parent;RAW=ROOT/'raw';DAY=pd.Timedelta(days=1);HALF=pd.Timedelta(minutes=30)
META=json.loads((ROOT/'meta_context.json').read_text());md={x['name'].split(':')[-1]:x for x in META[0]['universe']}

def load(f):
 x=json.loads(Path(f).read_text())
 if not x:return pd.DataFrame()
 d=pd.DataFrame(x);d.index=pd.to_datetime(d.t,unit='ms',utc=True);d=d.sort_index();d=d[~d.index.duplicated(keep='last')]
 for z in ['o','c','h','l','v','n']:d[z]=pd.to_numeric(d[z])
 d=d[(d.index+pd.to_timedelta(30 if str(f).endswith('30m.json') else 1440,unit='m')<=pd.to_datetime(END,unit='ms',utc=True)) & (d.c>0)]
 d['dv']=d.v*d.c;return d
D={Path(f).stem.replace('xyz_',''):load(f) for f in glob.glob('/tmp/xyz/*.json')}
I={c:load(RAW/f'{c}_30m.json') for c in COINS}
F={}
for c in COINS:
 x=json.loads((RAW/f'{c}_funding.json').read_text());df=pd.DataFrame(x)
 if len(df):
  df.index=pd.to_datetime(df.time,unit='ms',utc=True);df=df[~df.index.duplicated()];df['rate']=pd.to_numeric(df.fundingRate);F[c]=df.sort_index()
 else:F[c]=pd.DataFrame()
cal=xc.get_calendar('XNYS');sched=cal.schedule.loc['2025-10-01':'2026-10-08'];sched.to_csv(ROOT/'cash_calendar.csv')
# Baseline own copy: byte sum slots, cap5, vol includes signal bar, historical PIT volume.
import baseline_engine as be
# Include return ending at the final available next open (o is known even for a forming daily bar).
be.END=be.n-1;be.ix=np.arange(be.START,be.END);be.outdates=be.dates[be.ix+1]
be.mask={'full':np.ones(len(be.ix),bool),'2023-24':(be.outdates-DAY).year<=2024,'2025-26':(be.outdates-DAY).year>=2025}
T=np.zeros((be.n,be.k))
for i in range(32,be.n-1):
 ok=(be.V[i]>=5e6)&np.isfinite(be.P[i])&np.isfinite(be.P[i-14])&np.isfinite(be.SD[i]);idx=np.flatnonzero(ok)
 if len(idx)<8:continue
 t=sum(be.book(idx,s,5) for s in [be.M[i]/np.maximum(be.SD[i],1e-9),-be.SD[i],be.shock[i]])/3
 if np.abs(t).sum()>1e-12:t/=np.abs(t).sum()
 T[i]=t
base_runs=[be.run(T,p) for p in range(3)]
# be.run labels PnL at its ending opening; move to actual holding UTC day for alignment.
B=pd.DataFrame({f'p{p}':v['r'] for p,v in enumerate(base_runs)},index=be.outdates-DAY);B.to_csv(ROOT/'baseline_returns.csv')
fullbase={z:[be.stat(v['r'][m]) for v in base_runs] for z,m in be.mask.items()}
# Daily close return and matching BTC; no ffill across missing prices.
BTC=load('/tmp/hl-daily-full/BTC.json');btc=BTC.c.pct_change(fill_method=None)
btcrows=[]
for c,d in D.items():
 r=d.c.pct_change(fill_method=None);q=pd.concat([r.rename('x'),btc.rename('btc')],axis=1).dropna()
 if len(q)<40:continue
 for tag,m in [('full',np.ones(len(q),bool)),('first',np.arange(len(q))<len(q)//2),('second',np.arange(len(q))>=len(q)//2),('weekend',q.index.weekday>=5)]:
  a=q.loc[m];btcrows.append(dict(coin=c,part=tag,n=len(a),corr=a.x.corr(a.btc),beta=a.x.cov(a.btc)/a.btc.var(),r2=a.x.corr(a.btc)**2))
pd.DataFrame(btcrows).to_csv(ROOT/'btc_correlations.csv',index=False)
cov=[]
for c,d in D.items():
 cov.append(dict(coin=c,n=len(d),start=str(d.index.min()),end=str(d.index.max()),median_daily_usd=d.dv.median(),p10_daily_usd=d.dv.quantile(.1),min_daily_usd=d.dv.min(),zero_volume=int((d.dv==0).sum()),delisted=md.get(c,{}).get('isDelisted',False)))
pd.DataFrame(cov).to_csv(ROOT/'coverage.csv',index=False)

# Trade accounting: signed quantities fixed from entry, funded price at last known candle.
events=[];PATHS={};GROUPS={};DIAG={};PERLEG=[]
def mark(c,t,intra):
 d=I[c] if intra else D[c]
 return float(d.loc[t,'o']) if t in d.index else np.nan

def signal_price(c,t,intra):
 d=I[c] if intra else D[c];step=HALF if intra else DAY
 return float(d.loc[t-step,'c']) if t-step in d.index else np.nan

def liquidity(c,t,intra):
 d=I[c] if intra else D[c];step=HALF if intra else DAY
 a=d[(d.index>=t-DAY)&(d.index+step<=t)];v=a.dv.sum()
 if len(a)<(46 if intra else 1):return 0,0
 last=float(d.loc[t-step,'dv']) if t-step in d.index else 0
 return v,last

FUND={}
for c,f in F.items():
 for intra in [False,True]:
  d=I[c] if intra else D[c];step=HALF if intra else DAY
  if f.empty:FUND[c,intra]=(np.array([],dtype='int64'),np.array([0.]));continue
  # Price settled notional proxy; carry last completed mark. No use of future candle close.
  prices=pd.DataFrame({'time':d.index+step,'price':d.c.values})
  x=pd.merge_asof(pd.DataFrame({'time':f.index,'rate':f.rate.values}),prices,on='time',direction='backward')
  x['cash']=x.rate*x.price.fillna(0)
  FUND[c,intra]=(x.time.astype('int64').to_numpy(),np.r_[0,x.cash.cumsum().to_numpy()])
def funding(c,entry,exit,qty,intra):
 times,cum=FUND[c,intra]
 lo=np.searchsorted(times,entry.value,side='right');hi=np.searchsorted(times,exit.value,side='right')
 return -qty*(cum[hi]-cum[lo])

def trade(path,entry,exit,legs,intra,signal_start,signal_end,phase):
 # legs: gross weights in units of strategy NAV=1; allocation decided across eligible names earlier.
 rec={};caps=[]
 for c,w in legs.items():
  pe=mark(c,entry,intra);px=mark(c,exit,intra)
  if not np.isfinite(pe*px) or pe<=0 or px<=0:return False
  qty=w/pe;gross=qty*(px-pe);fu=funding(c,entry,exit,qty,intra)
  trn=abs(w)+abs(qty*px)
  gm=md[c].get('growthMode')=='enabled';scale=float(md[c].get('deployerFeeScale','1'));mult=(1+scale if scale<1 else 2*scale)*(.1 if gm else 1)
  dv,last=liquidity(c,entry,intra);cap=min(dv*.001,last*(.01 if intra else .001))/abs(w) if abs(w)>0 else np.inf
  caps.append(cap)
  rec[c]=(gross,fu,trn,trn*(.00045*mult+.0003))
  PERLEG.append(dict(path=path,entry=str(entry),exit=str(exit),coin=c,weight=w,entry_px=pe,exit_px=px,gross=gross,funding=fu,turn=trn,capacity_nav=cap))
 # Allocate daily MTM, funding, entry+exit cost on actual days (no exit-date lump of multiday trades).
 days=pd.date_range(entry.normalize(),exit.normalize(),freq='D')
 for c,w in legs.items():
  qty=w/mark(c,entry,intra);prev=entry;prevp=mark(c,entry,intra)
  for day in days:
   end=min(exit,day+DAY)
   if end<=prev:continue
   endp=mark(c,end,intra)
   if not np.isfinite(endp):raise ValueError('missing daily mark')
   gain=qty*(endp-prevp);fu=funding(c,prev,end,qty,intra)
   turn=(abs(w) if prev==entry else 0)+(abs(qty*endp) if end==exit else 0)
   gm=md[c].get('growthMode')=='enabled';scale=float(md[c].get('deployerFeeScale','1'));mult=(1+scale if scale<1 else 2*scale)*(.1 if gm else 1)
   key=prev.normalize()
   p=PATHS[path].setdefault(key,np.zeros(7));p+=np.array([gain,fu,turn,turn*.00075,turn*.00145,turn*.00245,turn*(.00045*mult+.0003)])
   prev=end;prevp=endp
 events.append(dict(path=path,entry=str(entry),exit=str(exit),signal_start=str(signal_start),signal_end=str(signal_end),phase=phase,nlegs=len(legs),gross=sum(v[0] for v in rec.values()),funding=sum(v[1] for v in rec.values()),turn=sum(v[2] for v in rec.values()),capacity_nav=min(caps)))
 return True

def eligible(c,t,intra,hedge=False):
 dv,last=liquidity(c,t,intra);return dv>=(5e6 if hedge else 1e6) and last>0

CROSS=[c for c in STOCKS if not md[c].get('onlyIsolated',False) and md[c].get('marginMode','normal')=='normal']
for variant,stocks in [('all16',STOCKS),('cross9',CROSS)]:
 # 12 daily hypotheses, complete hold phases, no overlapping book within each path.
 for hedge in ['XYZ100','SP500']:
  for lb in [1,3,5]:
   for hold in [1,3]:
    group=f'{variant}_daily_{hedge}_lb{lb}_h{hold}';GROUPS[group]=[]
    start=max(D[hedge].index.min(),min(D[c].index.min() for c in stocks))+pd.Timedelta(days=lb+1)
    lastday=min(D[hedge].index.max()-DAY,B.index.max())
    end=lastday+DAY-pd.Timedelta(days=hold)
    DIAG[group]={'start':str(start.normalize()),'end':str(lastday.normalize())}
    for phase in range(hold):
     path=f'{group}_p{phase}';GROUPS[group].append(path);PATHS[path]={}
     for entry in pd.date_range(start.normalize(),end.normalize(),freq='D'):
      if (entry.value//(86400*10**9))%hold!=phase or not eligible(hedge,entry,False,True):continue
      exit=entry+pd.Timedelta(days=hold);chosen=[]
      for c in stocks:
       if not eligible(c,entry,False):continue
       ps=signal_price(c,entry,False);pp=signal_price(c,entry-pd.Timedelta(days=lb),False);hs=signal_price(hedge,entry,False);hp=signal_price(hedge,entry-pd.Timedelta(days=lb),False)
       if not np.isfinite(ps*pp*hs*hp) or min(ps,pp,hs,hp)<=0:continue
       sig=ps/pp-hs/hp
       if abs(sig)<1e-10:continue
       chosen.append((c,-np.sign(sig)))
      if not chosen:continue
      legs={c:s*.5/len(chosen) for c,s in chosen};legs[hedge]=-sum(legs.values())
      # Keep offsetting pair hedge netting; normalize AFTER netting to fixed unit gross.
      gross=sum(abs(w) for w in legs.values());legs={c:w/gross for c,w in legs.items() if abs(w)>1e-12}
      trade(path,entry,exit,legs,False,entry-pd.Timedelta(days=lb),entry,phase)
 # 18 calendar mechanisms, all three execution latency sensitivities.
 for kind in ['overnight','weekend','external_weekend']:
  for hedge in ['none','XYZ100','SP500']:
   for hold in (['1h','3h'] if kind=='external_weekend' else ['1h','3h','close']):
    group=f'{variant}_{kind}_{hedge}_{hold}';GROUPS[group]=[]
    start=max(I[c].index.min() for c in COINS);end=min(min(I[c].index.max() for c in COINS),B.index.max()+DAY)
    DIAG[group]={'start':str(start.normalize()+DAY),'end':str(min(end.normalize()-DAY,B.index.max()))}
    for delay in [30,60,90]:
     path=f'{group}_d{delay}';GROUPS[group].append(path);PATHS[path]={}
     for j in range(1,len(sched)):
      row=sched.iloc[j];pr=sched.iloc[j-1];op=row['open'];close=row['close'];pc=pr['close']
      if kind=='external_weekend':
       if not (op.weekday()==0 and pc.weekday()==4):continue
       # Exact BOATS external-input resumption: Sunday 20:00 ET, previous Friday 20:00 ET.
       op=(row['open'].tz_convert('America/New_York').normalize()-DAY+pd.Timedelta(hours=20)).tz_convert('UTC')
       pc=(pr['close'].tz_convert('America/New_York').normalize()+pd.Timedelta(hours=20)).tz_convert('UTC')
       close=op+pd.Timedelta(hours=8)
      delta=(op.normalize()-pc.normalize()).days
      if kind=='overnight' and delta!=1:continue
      if kind=='weekend' and not (op.weekday()==0 and pc.weekday()==4 and delta==3):continue
      entry=op+pd.Timedelta(minutes=delay);exit=close if hold=='close' else entry+pd.Timedelta(hours=int(hold[0]))
      if entry<start+DAY or exit>end or exit>close:continue
      if hedge!='none' and not eligible(hedge,entry,True,True):continue
      chosen=[]
      for c in stocks:
       if not eligible(c,entry,True):continue
       ps=signal_price(c,op,True);pp=signal_price(c,pc,True)
       if not np.isfinite(ps*pp) or min(ps,pp)<=0:continue
       sig=ps/pp-1
       if hedge!='none':
        hs=signal_price(hedge,op,True);hp=signal_price(hedge,pc,True)
        if not np.isfinite(hs*hp) or min(hs,hp)<=0:continue
        sig-=hs/hp-1
       if abs(sig)<1e-10:continue
       if not np.isfinite(mark(c,entry,True)*mark(c,exit,True)):continue
       chosen.append((c,-np.sign(sig)))
      if not chosen:continue
      legs={c:s*(1 if hedge=='none' else .5)/len(chosen) for c,s in chosen}
      if hedge!='none':legs[hedge]=-sum(legs.values())
      gross=sum(abs(w) for w in legs.values());legs={c:w/gross for c,w in legs.items() if abs(w)>1e-12}
      trade(path,entry,exit,legs,True,pc,op,delay)

E=pd.DataFrame(events);E.to_csv(ROOT/'events.csv',index=False);pd.DataFrame(PERLEG).to_csv(ROOT/'legs.csv',index=False)
# Fill no-trade days with zero only within declared opportunity windows, preserving absent history.
all_dates=pd.date_range(min(pd.Timestamp(x['start']) for x in DIAG.values()),max(pd.Timestamp(x['end']) for x in DIAG.values()),freq='D')
arrays={};active={};metrics=[]
def sh(x):
 return float(np.mean(x)/np.std(x,ddof=1)*np.sqrt(365)) if len(x)>1 and np.std(x)>1e-12 else 0.
def safe_corr(x,y):return float(np.corrcoef(x,y)[0,1]) if np.std(x)>1e-12 and np.std(y)>1e-12 else np.nan
base=B.reindex(all_dates).to_numpy();assert np.isfinite(base).all()
for g,paths in GROUPS.items():
 mask=(all_dates>=pd.Timestamp(DIAG[g]['start']))&(all_dates<=pd.Timestamp(DIAG[g]['end']));active[g]=mask
 cube=[]
 for p in paths:
  a=np.array([PATHS[p].get(t,np.zeros(7)) for t in all_dates]);cube.append(a)
 cube=np.array(cube);avg=cube.mean(axis=0);arrays[g]=cube
 for part,pm in [('full',mask),('first',mask&(all_dates<=all_dates[mask][len(all_dates[mask])//2-1])),('second',mask&(all_dates>all_dates[mask][len(all_dates[mask])//2-1]))]:
  net=cube[:,:,0]+cube[:,:,1]-cube[:,:,3];net=net[:,pm];bt=base[pm]
  # reported phase mean is mean of phase Sharpes, distinct from Sharpe of averaged paths.
  phases=[sh(z) for z in net];phase_delta=[sh(.9*bt[:,b]+.1*2.7*net[p])-sh(bt[:,b]) for p in range(len(paths)) for b in range(3)]
  r=avg[pm,0]+avg[pm,1]-avg[pm,3];bm=bt.mean(axis=1);em=E[E.path.isin(paths)]
  metrics.append(dict(candidate=g,part=part,days=int(pm.sum()),events_per_path=float(len(em)/len(paths)) if part=='full' else np.nan,sharpe_phase_mean=np.mean(phases),sharpe_phase_sd=np.std(phases),sharpe_phase_min=min(phases),sharpe_phase_max=max(phases),sharpe_averaged_path=sh(r),baseline_phase_sharpe_mean=np.mean([sh(bt[:,b]) for b in range(3)]),blend_delta_sharpe_mean=np.mean(phase_delta),blend_delta_sharpe_min=min(phase_delta),corr_baseline=safe_corr(r,bm),net_ann=365*r.mean(),gross_ann=365*avg[pm,0].mean(),funding_ann=365*avg[pm,1].mean(),turn_ann=365*avg[pm,2].mean(),cost_ann=365*avg[pm,3].mean(),net_ann_slip10=365*(avg[pm,0]+avg[pm,1]-avg[pm,4]).mean(),net_ann_slip20=365*(avg[pm,0]+avg[pm,1]-avg[pm,5]).mean(),net_ann_current_fee=365*(avg[pm,0]+avg[pm,1]-avg[pm,6]).mean(),maker_fee_only_lower_bound_ann=365*(avg[pm,0]+avg[pm,1]-avg[pm,2]*.00045).mean(),capacity_min=float(em.capacity_nav.min()),capacity_p10=float(em.capacity_nav.quantile(.1))))
metrics=pd.DataFrame(metrics)
# Studentization from bootstrap mean dispersion; synchronized centered circular block bootstrap maxT.
# Include both all phase/latency paths and their economic averages in FWER family.
labels=[];cols=[];masks=[]
for g,cube in arrays.items():
 net=cube[:,:,0]+cube[:,:,1]-cube[:,:,3]
 for p in range(len(net)):
  labels.append(f'{g}:path{p}');cols.append(net[p]);masks.append(active[g])
 labels.append(g);cols.append(net.mean(axis=0));masks.append(active[g])
X=np.array(cols).T;M=np.array(masks).T.astype(float);N=M.sum(axis=0);mu=(X*M).sum(axis=0)/N;XC=(X-mu)*M
K=3999;RNG=np.random.default_rng(20261008);boot_results={};improve_results={};GS=list(GROUPS)
# Separate paired FWER test of fixed-blend Sharpe improvement, retains actual baseline phases.
obs_delta=np.array([metrics[(metrics.candidate==g)&(metrics.part=='full')].blend_delta_sharpe_mean.iloc[0] for g in GS])
for block in [7,14]:
 draws=np.empty((K,len(labels)));deltas=np.empty((K,len(GS)))
 for k in range(K):
  starts=RNG.integers(0,len(all_dates),size=(len(all_dates)+block-1)//block);idx=((starts[:,None]+np.arange(block))%len(all_dates)).ravel()[:len(all_dates)]
  den=M[idx].sum(axis=0);draws[k]=XC[idx].sum(axis=0)/np.maximum(den,1)
  for j,g in enumerate(GS):
   ix=idx[active[g][idx]]
   if len(ix)<4:deltas[k,j]=0;continue
   cube=arrays[g];net=cube[:,ix,0]+cube[:,ix,1]-cube[:,ix,3];bt=base[ix]
   deltas[k,j]=np.mean([sh(.9*bt[:,b]+.27*net[p])-sh(bt[:,b]) for p in range(len(net)) for b in range(3)])
 sd=draws.std(axis=0,ddof=1);obs=mu/np.maximum(sd,1e-12);mx=(draws/np.maximum(sd,1e-12)).max(axis=1)
 pf=np.array([(1+(mx>=z).sum())/(K+1) for z in obs]);boot_results[block]={label:dict(p_fwer=float(pf[j]),t_block=float(obs[j]),mean_ci_low=float(mu[j]+np.quantile(draws[:,j],.025)),mean_ci_high=float(mu[j]+np.quantile(draws[:,j],.975))) for j,label in enumerate(labels)}
 ds=deltas.std(axis=0,ddof=1);dmx=((deltas-obs_delta)/np.maximum(ds,1e-12)).max(axis=1)
 improve_results[block]={g:dict(p_fwer=float((1+(dmx>=obs_delta[j]/max(ds[j],1e-12)).sum())/(K+1)),delta=float(obs_delta[j]),ci_low=float(np.quantile(deltas[:,j],.025)),ci_high=float(np.quantile(deltas[:,j],.975))) for j,g in enumerate(GS)}
 print('bootstrap',block,'done',flush=True)
for block in [7,14]:
 metrics[f'p_fwer_net_{block}']=metrics.candidate.map({g:boot_results[block][g]['p_fwer'] for g in GS})
 metrics[f'p_fwer_blend_{block}']=metrics.candidate.map({g:improve_results[block][g]['p_fwer'] for g in GS})
metrics.to_csv(ROOT/'metrics.csv',index=False)
# Timestamp audit: require UTC offset transitions in full calendar, exact 30min boundaries in data.
audit={'economic_candidates':len(GS),'phase_latency_paths':sum(len(v) for v in GROUPS.values()),'fwer_mean_tests':len(labels),'bootstrap_replicates':K,'bootstrap_blocks':[7,14],'full_baseline':fullbase,'dates':{c:dict(n=len(d),start=str(d.index.min()),end=str(d.index.max()),non30m_boundaries=int((d.index.minute%30!=0).sum()),missing_30m=int((d.index.to_series().diff()>HALF).sum())) for c,d in I.items()},'funding':{c:dict(n=len(f),start=str(f.index.min()),end=str(f.index.max())) for c,f in F.items()},'calendar_open_utc_hours':sorted(set(sched['open'].dt.hour.tolist())),'vix_meta':md['VIX'],'input_sha256':{str(f):hashlib.sha256(Path(f).read_bytes()).hexdigest() for f in list(RAW.glob('*.json'))+list(Path('/tmp/xyz').glob('*.json'))+[ROOT/'DESIGN.md',Path('/Users/wonder/Code/stars/src/trader.rs'),Path('/Users/wonder/Code/stars/docs/validation-protocol.md')]}}
# current book diagnostic only; no extrapolation as historic available execution.
books=[]
for f in RAW.glob('*_book.json'):
 d=json.loads(f.read_text());bid=d['levels'][0];ask=d['levels'][1];bb=float(bid[0]['px']);ba=float(ask[0]['px']);mid=(bb+ba)/2
 books.append(dict(coin=d['coin'],time=d['time'],spread_bps=(ba/bb-1)*1e4,bid_within3bps_usd=sum(float(z['px'])*float(z['sz']) for z in bid if float(z['px'])>=mid*(1-.0003)),ask_within3bps_usd=sum(float(z['px'])*float(z['sz']) for z in ask if float(z['px'])<=mid*(1+.0003))))
pd.DataFrame(books).to_csv(ROOT/'current_books.csv',index=False)
json.dump(audit,open(ROOT/'audit.json','w'),indent=2);json.dump({'net':boot_results,'blend':improve_results},open(ROOT/'bootstrap.json','w'),indent=2)
out=pd.DataFrame(index=all_dates)
for g,a in arrays.items():out[g]=(a[:,:,0]+a[:,:,1]-a[:,:,3]).mean(axis=0)
out['baseline']=base.mean(axis=1);out.to_csv(ROOT/'daily_returns.csv')
print(metrics[metrics.part=='full'].sort_values('net_ann',ascending=False).to_string(index=False),flush=True)
