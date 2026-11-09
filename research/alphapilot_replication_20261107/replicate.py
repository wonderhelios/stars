import json,glob,datetime,warnings
from pathlib import Path
import numpy as np,pandas as pd
import lightgbm as lgb
warnings.filterwarnings('ignore')
LABELS=r'''2026-05-04|-APE,-ARB,-BIO,+BTC,+DOGE,+ETH,+HYPE,-LDO,-ORDI,+PUMP,-TRUMP,-VVV,+XMR,+kPEPE
2026-05-07|-BIO,+BNB,-MEGA,+TAO,+VVV,-WLFI,-XPL,+ZEC
2026-05-10|+ADA,+DOGE,-ENA,+ETH,-FARTCOIN,-MON,+PUMP,+SOL,-XPL
2026-05-13|-APE,+ASTER,+BNB,+LINK,-LIT,-NEAR,+PUMP,+SOL,-SUI,-TON,-XPL,+ZEC
2026-05-16|+AAVE,-APE,-BIO,+BNB,+BTC,-ENA,-LIT,-ORDI,+PENGU,+SOL,-TAO,-XPL,+ZEC,+kPEPE
2026-05-19|-APE,-ASTER,+BIO,+BNB,-ENA,+HYPE,+MEGA,-NEAR,+PUMP,-TAO,-VVV,-XPL,+kPEPE
2026-05-22|+BIO,+BNB,-FARTCOIN,-LDO,+MEGA,+MON,-PAXG,-PENDLE,+SOL,-SUI,-TON,+WLD,+kPEPE
2026-05-25|-APE,-CHIP,-CRV,+ETH,+HYPE,-SUI,+VVV,-XRP,+ZEC
2026-05-28|-AAVE,-ASTER,-DOGE,+ENA,+HYPE,+MEGA,-NEAR,+ONDO,+PENGU,-TAO,-XMR,+kPEPE
2026-05-31|+BIO,-CHIP,-LIT,+ONDO,+PENGU,+PUMP,-TAO,-TON,-VVV,-WLD,-XLM,+ZEC,+kPEPE
2026-06-03|-ASTER,-CHIP,+LIT,+MEGA,+MON,-ONDO,-PAXG,+PENGU,+PUMP,-XLM,+kPEPE
2026-06-06|+ASTER,-CRV,+FARTCOIN,+HYPE,-INJ,+LIT,-MON,-PAXG,+PENGU,+PUMP,-TON,-VIRTUAL,-XLM,+kPEPE
2026-06-09|+AVAX,+BTC,-FARTCOIN,+HYPE,-INJ,-LIT,-NEAR,+PENGU,+PUMP,-WLD,+XLM,-XPL,-ZEC,+kPEPE
2026-06-12|+ADA,-ASTER,+AVAX,-BNB,-CRV,-MON,+NEAR,+PAXG,+PENGU,+PUMP,-TON,-VIRTUAL,-XMR,+kPEPE
2026-06-15|+AVAX,+BTC,+ETH,-FARTCOIN,-INJ,-NEAR,+ONDO,-WLD,-XLM,-XPL
2026-06-18|+ADA,+AVAX,-BNB,-ENA,+HYPE,-MON,-NEAR,+XRP
2026-06-21|+ADA,+AVAX,+ETH,+HYPE,-JTO,-LIT,-MON,-PAXG,+SOL,-TRUMP,-UNI,-XPL
2026-06-24|+ADA,+AVAX,-JTO,-LINK,-LIT,-PAXG,+PUMP,+SOL,-TRUMP,-UNI,+XLM,+kPEPE
2026-06-27|+AVAX,+BCH,-CRV,-FARTCOIN,+HYPE,-INJ,-PAXG,-TAO,-XPL,+kPEPE
2026-06-30|+AVAX,+HYPE,-JTO,-PAXG,-TAO,-TRUMP,-UNI,+WLD,-XLM,+kPEPE
2026-07-03|+AVAX,+BTC,+ETH,-FARTCOIN,-NEAR,-VVV,-WLD,-XPL
2026-07-06|-ADA,+BTC,+ETH,-FARTCOIN,-JTO,-TAO,-UNI,+XRP
2026-07-09|-ASTER,+AVAX,+BTC,+ETH,-FARTCOIN,+PUMP,+SOL,-TRUMP,-VVV,-XLM,-XPL,+kPEPE
2026-07-12|+BCH,+BTC,-CRV,+ETH,-FARTCOIN,+HYPE,+PUMP,-UNI,-VVV,-WLD,-XPL,+kPEPE
2026-07-15|-ASTER,-ENA,-FARTCOIN,+HYPE,-JTO,+SOL,+XRP
2026-07-18|-AAVE,-ENA,-FARTCOIN,+LIT,-XLM,+XMR,-XPL,+kBONK
2026-07-21|+BNB,-EIGEN,-FARTCOIN,-JTO,+SOL,-TRUMP,-XPL
2026-07-24|-ARB,-BNB,+ENA,-JTO,+KAITO,+LIT,-ONDO,-SUI,-TRUMP
2026-07-27|+ADA,+AVAX,-BNB,+ETH,-JTO,-KAITO,+LINK,-TRUMP,-VVV,+XMR,-XPL,+kBONK
2026-07-30|+BNB,+BTC,+ENA,+ETH,-FARTCOIN,-JTO,+KAITO,-TRUMP,-UNI,-VVV,-ZRO,+kBONK
2026-08-02|+ETH,-FARTCOIN,-GRAM,-JTO,+LINK,+SOL,-UNI,-VVV,+XMR,-ZRO,+kBONK,+kPEPE
2026-08-05|+BNB,+BTC,+ENA,+ETH,-GRAM,-LIT,-TRUMP,-UNI,-VVV,+XMR,+kBONK
2026-08-08|+BNB,+BTC,-DOGE,-GRAM,-JTO,+PAXG,-UNI,-VVV,+ZEC
2026-08-11|-AAVE,-CASHCAT,+FARTCOIN,+KAITO,+LTC,+SOL,-WLD,+kBONK,+kPEPE
2026-08-14|-AAVE,-CASHCAT,-ENA,+FARTCOIN,-GRAM,-KAITO,+LINK,+PUMP,+TAO,-WLD,+ZEC,+kBONK,+kSHIB
2026-08-17|-AAVE,-ADA,-CASHCAT,-ENA,+ETH,-GRAM,+HYPE,+TAO,-UNI,+XMR,-XRP,+ZEC,+kBONK,+kPEPE
2026-08-20|+BTC,-CRV,-FARTCOIN,-GRAM,-LIT,+LTC,-ONDO,+PUMP,+SOL,+XMR
2026-08-23|-ADA,+BNB,+BTC,+ETH,-ONDO,+PAXG,-VVV,-WLD,+XMR
2026-08-26|-AAVE,+LINK,-NEAR,+SOL,-TRUMP,-XPL,+ZEC
2026-08-29|-CASHCAT,-DOGE,-ENA,+HYPE,+LINK,-MON,-PAXG,+SOL,+ZEC,-ZRO
2026-09-01|+BNB,-CASHCAT,-ENA,+ETH,+HYPE,-PAXG,+PUMP,-SUI,-WLD,-XPL,+ZEC,-ZRO,+kPEPE
2026-09-04|+BNB,+BTC,-CASHCAT,-DOGE,-ENA,+ETH,+HYPE,-NEAR,+SOL,-TRUMP,-VVV,+XMR
2026-09-07|+ARB,-CASHCAT,-LINK,-MON,-UNI
2026-09-10|+ASTER,-CRV,-HYPE,-LINK,+ONDO,+PENGU,+PUMP,-TAO,+UNI,-WLD,-XPL,+kPEPE
2026-09-13|+BNB,-CASHCAT,-CRV,-NEAR,+PENGU,+PUMP,-TAO,-WLD,+XMR,-XPL,+ZEC,+kPEPE
2026-09-16|+AVAX,-CRV,-LIT,+ONDO,+PENGU,+PUMP,-VVV,+XMR,-XPL,+kPEPE
2026-09-19|+BNB,+BTC,-CASHCAT,+ETH,-FARTCOIN,+HYPE,-LIT,+SOL,+TAO,-TRUMP,-WLD
2026-09-22|-ARB,-CASHCAT,-MON,-ONDO,-UNI,+kPEPE
2026-09-25|+ENA,-NEAR,-PENGU,-SUI,+VVV,-XPL,+XRP
2026-09-28|+AAVE,+BTC,-CASHCAT,-DOGE,+ETH,+HYPE,+LTC,-MON,-ONDO,+SOL,-SUI,-XPL
2026-10-01|+BTC,-CASHCAT,-DOGE,+ETH,+HYPE,+LTC,-ONDO,-PONS,+SOL,+WLD,-XPL
2026-10-04|+ARB,+BTC,-CASHCAT,+ETH,+HYPE,-LIT,+LTC,-MON,+SOL,-SUI,-WLD,-XPL
2026-10-07|+AAVE,+ADA,+BNB,+BTC,-FARTCOIN,-LIT,-NEAR,-ONDO,+PUMP,+SOL,-XPL,-ZRO'''
labels=[]
for line in LABELS.splitlines():
    d, ss=line.split('|'); labels.append((d,{x[1:]:1 if x[0]=='+' else -1 for x in ss.split(',')}))
# Load daily data exactly as provided; construct a common UTC date panel.
raw={}
for f in glob.glob('/tmp/hl2/*.json'):
    c=Path(f).stem; rows=json.load(open(f)); rows=[x for x in rows if float(x['c'])>0]
    if len(rows)>=120: raw[c]=rows
names=sorted(raw); dates=sorted({x['t'] for v in raw.values() for x in v}); ti={t:i for i,t in enumerate(dates)}; n=len(dates); k=len(names)
C=np.full((n,k),np.nan); O=C.copy(); H=C.copy(); L=C.copy(); Q=C.copy()
for j,c in enumerate(names):
    for x in raw[c]:
        i=ti[x['t']]; C[i,j]=float(x['c']); O[i,j]=float(x['o']); H[i,j]=float(x['h']); L[i,j]=float(x['l']); Q[i,j]=float(x['c'])*float(x['v'])
R=pd.DataFrame(C).pct_change().values
V30=pd.DataFrame(Q).rolling(30,min_periods=20).mean().shift(1).values
# Feature panel: only information available at the previous daily close.
feat={}
for w in [1,3,7,14,30,60]: feat[f'ret{w}']=C/np.roll(C,w,axis=0)-1
for w in [5,10,20,30,60]: feat[f'vol{w}']=pd.DataFrame(R).rolling(w,min_periods=max(4,w//2)).std(ddof=1).values
for w in [20,60]:
    hi=pd.DataFrame(C).rolling(w,min_periods=max(10,w//2)).max().values; lo=pd.DataFrame(C).rolling(w,min_periods=max(10,w//2)).min().values
    feat[f'fromhi{w}']=C/hi-1; feat[f'fromlo{w}']=C/lo-1
logq=np.log(np.maximum(Q,1)); feat['logdv30']=pd.DataFrame(logq).rolling(30,min_periods=20).mean().values; feat['volsurp']=logq-pd.DataFrame(logq).rolling(30,min_periods=20).mean().values
feat['range']=(H-L)/np.maximum(O,1e-12); feat['intraday']=C/np.maximum(O,1e-12)-1; feat['closepos']=(C-L)/np.maximum(H-L,1e-12)
# Funding data as trailing means, timestamp-bounded at the prior close.
f=json.load(open('/tmp/hl_funding_full.json')); fby={c:sorted((int(t),float(v)) for t,v in d.items()) for c,d in f.items()}
fr7=np.full_like(C,np.nan); fr30=np.full_like(C,np.nan)
for j,c in enumerate(names):
    if c not in fby: continue
    tt=np.array([z[0] for z in fby[c]]); vv=np.array([z[1] for z in fby[c]])
    for i,t in enumerate(dates):
        stop=np.searchsorted(tt,t,side='left'); a=np.searchsorted(tt,t-7*86400000,side='left'); b=np.searchsorted(tt,t-30*86400000,side='left')
        if stop-a>=100: fr7[i,j]=vv[a:stop].mean()
        if stop-b>=400: fr30[i,j]=vv[b:stop].mean()
feat['fund7']=fr7; feat['fund30']=fr30
fn=list(feat); X=np.stack([feat[f] for f in fn],axis=2)
# Build labelled cross-sections on the previous close; website holdings are the classes (short=0, neutral=1, long=2).
train_rows=[]; train_y=[]; train_groups=[]; train_days=[]; label_map={}
for d, pos in labels:
    # trades timestamped at UTC 00:00: use the prior candle close as signal and timestamp day's open as execution.
    idx=next((i for i,t in enumerate(dates) if datetime.datetime.fromtimestamp(t/1000,datetime.timezone.utc).strftime('%Y-%m-%d')==d),None)
    if idx is None or idx<61: continue
    s=idx-1
    eligible=np.isfinite(V30[s])&(np.sum(np.isfinite(X[s]),axis=1)>=12)
    liquid=np.flatnonzero(eligible)
    js=liquid[np.argsort(V30[s,liquid])[-30:]]
    if len(js)<12: continue
    # grade selected long/short vs neutral for pairwise ranking
    y=np.ones(len(js),dtype=int)
    for q,j in enumerate(js):
        if names[j] in pos: y[q]=2 if pos[names[j]]>0 else 0
    train_rows.append(X[s,js]); train_y.extend(y.tolist()); train_groups.append(len(js)); train_days.append((d,idx,js,y,pos)); label_map[d]=pos
# chronological holdout: fit through Aug 05, then unseen rebalance snapshots Aug 08-Oct 07.
cut='2026-08-05'; tridx=[i for i,x in enumerate(train_days) if x[0]<=cut]; teidx=[i for i,x in enumerate(train_days) if x[0]>cut]
flatX=np.concatenate(train_rows,axis=0); flatY=np.array(train_y,dtype=int)
def flatten(ids):
    xx=[]; yy=[]; gg=[]; meta=[]
    for ii in ids:
        d,ix,js,y,pos=train_days[ii]; xx.append(X[ix-1,js]); yy.extend(y); gg.append(len(js)); meta.append(train_days[ii])
    return np.concatenate(xx),np.array(yy),gg,meta
xtr,ytr,gtr,mtr=flatten(tridx); xte,yte,gte,mte=flatten(teidx)
params={'objective':'lambdarank','metric':'ndcg','learning_rate':0.025,'num_leaves':5,'max_depth':2,'min_data_in_leaf':40,'feature_fraction':0.85,'lambda_l2':25,'verbosity':-1,'num_threads':4,'seed':2718,'deterministic':True,'force_col_wise':True}
model=lgb.train(params,lgb.Dataset(xtr,label=ytr,group=gtr,feature_name=fn),num_boost_round=100)
# Backtest on held-out dates; choose 6 each side, 2x gross, hold between rebalance opens. Include actual position path comparator.
def volscale(w, s, win=20):
    js=np.flatnonzero(w)
    if len(js)<4:return 1.
    rr=[]
    for j in js:
        z=R[max(0,s-win+1):s+1,j]
        rr.append(z)
    mat=np.array(rr).T
    port=np.nansum(mat*w[js],axis=1)
    vol=np.nanstd(port,ddof=1)*2.7
    return float(np.clip(.02/vol,.2,3.)) if np.isfinite(vol) and vol>1e-9 else 1.
def run(indices, use_model=True, placebo=False, seed=5, lev=5.4):
    # rebalance selected label dates only; fill at open. Scores formed from prior close.
    schedule=[]
    rng=np.random.default_rng(seed)
    for ii in indices:
        d,ix,js,y,pos=train_days[ii]
        if use_model:
            sc=model.predict(X[ix-1,js]); order=np.argsort(sc)
            longs=js[order[-min(6,len(js)//2):]]; shorts=js[order[:min(6,len(js)//2)]]
            if placebo:
                perm=rng.permutation(js); longs=perm[-min(6,len(js)//2):]; shorts=perm[:min(6,len(js)//2)]
            w=np.zeros(k); w[longs]=0.5/len(longs); w[shorts]=-0.5/len(shorts)
        else:
            w=np.zeros(k)
            longs=[names.index(c) for c,s in pos.items() if s>0 and c in names]; shorts=[names.index(c) for c,s in pos.items() if s<0 and c in names]
            if longs:w[longs]=0.5/len(longs)
            if shorts:w[shorts]=-0.5/len(shorts)
        w*=lev if lev != 5.4 else 2.7*volscale(w,ix-1)
        schedule.append((ix,w,d))
    if not schedule:return None
    ix0=schedule[0][0]; ix1=schedule[-1][0]
    # Daily open-to-open mark, retain weights between dated rebalances.
    eq=1.; curve=[]; dret=[]; curr=np.zeros(k); costs=[]; datesout=[]; p=0
    for i in range(ix0,ix1):
        if p<len(schedule) and i==schedule[p][0]:
            nw=schedule[p][1]; turn=np.nansum(np.abs(nw-curr)); eq*=max(0,1-.00073*turn); costs.append(turn); curr=nw.copy(); p+=1
        good=np.isfinite(O[i])&np.isfinite(O[i+1]); rr=np.zeros(k); rr[good]=O[i+1,good]/O[i,good]-1
        r=float(np.nansum(curr*rr)); eq*=1+r; curve.append(eq); dret.append(r); datesout.append(datetime.datetime.fromtimestamp(dates[i]/1000,datetime.timezone.utc).strftime('%Y-%m-%d'))
    r=np.array(dret); e=np.array(curve)
    if len(r)<5:return None
    sh=r.mean()/r.std(ddof=1)*np.sqrt(365) if np.std(r,ddof=1)>0 else np.nan
    ann=(e[-1]**(365/len(r))-1)*100 if e[-1]>0 else -100
    dd=np.max(1-e/np.maximum.accumulate(e))*100
    win=np.mean(r>0)*100; conc=np.sort(r)[-5:].sum()/r.sum()*100 if r.sum()>0 else np.nan
    return {'N':len(r),'sh':sh,'ann':ann,'dd':dd,'win':win,'conc5':conc,'equity':e[-1],'daily':r,'dts':datesout,'turn_avg':np.mean(costs) if costs else 0}
actual=run(teidx,use_model=False); pred=run(teidx,use_model=True)
actual2=run(teidx,use_model=False,lev=2.0); pred2=run(teidx,use_model=True,lev=2.0)
print('DATA',len(names),'assets; date range',datetime.datetime.fromtimestamp(dates[0]/1000,datetime.timezone.utc).date(),datetime.datetime.fromtimestamp(dates[-1]/1000,datetime.timezone.utc).date())
print('LABELS',len(train_days),'train',len(tridx),'test',len(teidx),'train window',train_days[tridx[0]][0],train_days[tridx[-1]][0],'test window',train_days[teidx[0]][0],train_days[teidx[-1]][0],'features',len(fn),'rows',len(xtr))
for label,z in [('AP holdings (vol target)',actual),('Imitation model (vol target)',pred),('AP holdings (2x)',actual2),('Imitation model (2x)',pred2)]:print(label,{k:round(v,3) for k,v in z.items() if k not in ['daily','dts']})
# Selected-side overlap and direction hit rate, by held-out rebalance snapshot.
prec=[]; rec=[]; corr=[]
for ii in teidx:
    d,ix,js,y,pos=train_days[ii]; sc=model.predict(X[ix-1,js]); ord=np.argsort(sc); q=min(6,len(js)//2); top=set(js[ord[-q:]]); bot=set(js[ord[:q]])
    al=set(names.index(c) for c,s in pos.items() if c in names and s>0); ask=set(names.index(c) for c,s in pos.items() if c in names and s<0)
    tp=len(top&al)+len(bot&ask); predn=len(top)+len(bot); actn=len(al)+len(ask)
    prec.append(tp/predn if predn else np.nan); rec.append(tp/actn if actn else np.nan)
    corr.append(np.corrcoef(sc,y)[0,1] if len(set(y))>1 else np.nan)
print('HOLDOUT imitation precision',np.nanmean(prec),'recall',np.nanmean(rec),'score-label corr',np.nanmean(corr))
# randomized control with identical eligible universe, gross exposure and volatility wrapper.
places=[run(teidx,use_model=True,placebo=True,seed=seed,lev=2.0) for seed in range(20)]
print('RANDOM20 median',{key:round(float(np.nanmedian([p[key] for p in places])),3) for key in ['sh','ann','dd','win','conc5','equity']})
print('RANDOM20 10-90 Sharpe',np.nanpercentile([p['sh'] for p in places],[10,90]).round(3).tolist())
print('HIST top5 positive daily contribution %',pred2['conc5'],'top5 raw',float(np.sort(pred2['daily'])[-5:].sum()*100))
print('MODEL gain importance',sorted(zip(fn,model.feature_importance(importance_type='gain')),key=lambda z:-z[1])[:12])
print('DISPERSION predicted daily std',np.std(pred2['daily'],ddof=1),'actual',np.std(actual2['daily'],ddof=1))
# Candidate 2: same LightGBM ranking family, but train on the stated objective (future 3-bar cross-sectional rank).
# This is distinct from Candidate 1's imitation of selected holdings.
cutdate='2026-08-05'; trainX=[]; trainY=[]; trainG=[]; train_date_groups=[]; train_sig=[]
for s in range(61,n-4):
    d=datetime.datetime.fromtimestamp(dates[s+1]/1000,datetime.timezone.utc).strftime('%Y-%m-%d')
    if d>cutdate: break
    elig=np.isfinite(V30[s])&(np.sum(np.isfinite(X[s]),axis=1)>=12); cand=np.flatnonzero(elig)
    if len(cand)<12:continue
    js=cand[np.argsort(V30[s,cand])[-30:]]
    yret=C[s+4,js]/C[s+1,js]-1
    good=np.isfinite(yret)&(yret>-.95)
    js=js[good]; yret=yret[good]
    if len(js)<12:continue
    rk=pd.Series(yret).rank(pct=True).values
    y=np.minimum(4,np.floor(rk*5)).astype(int)
    trainX.append(X[s,js]); trainY.extend(y.tolist());trainG.append(len(js));train_date_groups.append(d)
ret_params={'objective':'lambdarank','metric':'ndcg','learning_rate':0.025,'num_leaves':7,'max_depth':3,'min_data_in_leaf':80,'feature_fraction':0.85,'lambda_l2':30,'verbosity':-1,'num_threads':4,'seed':2718,'deterministic':True,'force_col_wise':True}
model2=lgb.train(ret_params,lgb.Dataset(np.concatenate(trainX),label=np.array(trainY),group=trainG,feature_name=fn),num_boost_round=120)
model_save=model;model=model2
actual2=run(teidx,use_model=False,lev=2.0); predret=run(teidx,use_model=True,lev=2.0)
print('CANDIDATE2 return-ranker train_days',len(trainG),'rows',len(trainY),'date',train_date_groups[0],train_date_groups[-1])
print('CANDIDATE2 AP holdings (2x)',{k:round(float(v),3) for k,v in actual2.items() if k not in ['daily','dts']})
print('CANDIDATE2 LightGBM future-rank (2x)',{k:round(float(v),3) for k,v in predret.items() if k not in ['daily','dts']})
pr=[run(teidx,use_model=True,placebo=True,seed=seed,lev=2.0) for seed in range(20)]
print('CANDIDATE2 RANDOM20 median',{key:round(float(np.nanmedian([p[key] for p in pr])),3) for key in ['sh','ann','dd','win','equity']})
print('CANDIDATE2 gain importance',sorted(zip(fn,model2.feature_importance(importance_type='gain')),key=lambda z:-z[1])[:12])
model=model_save
# Candidate 3: fixed 50/50 blend of the site-holdings ranker and forward-return ranker (no weight search).
def run_blend(indices, lev=2.0, vol_target=False):
    schedule=[]
    for ii in indices:
        d,ix,js,y,pos=train_days[ii]
        a=model_save.predict(X[ix-1,js]); b=model2.predict(X[ix-1,js])
        ra=pd.Series(a).rank(pct=True).values; rb=pd.Series(b).rank(pct=True).values
        sc=.5*ra+.5*rb; order=np.argsort(sc); q=min(6,len(js)//2)
        longs=js[order[-q:]]; shorts=js[order[:q]]
        w=np.zeros(k); w[longs]=.5/len(longs);w[shorts]=-.5/len(shorts);w*= (2.7*volscale(w,ix-1) if vol_target else lev)
        schedule.append((ix,w,d))
    ix0=schedule[0][0];ix1=schedule[-1][0];eq=1.;curr=np.zeros(k);curve=[];rrs=[];costs=[]
    for i in range(ix0,ix1):
        if any(x[0]==i for x in schedule):
            _,nw,_=next(x for x in schedule if x[0]==i);turn=np.nansum(np.abs(nw-curr));eq*=max(0,1-.00073*turn);costs.append(turn);curr=nw.copy()
        good=np.isfinite(O[i])&np.isfinite(O[i+1]);rvec=np.zeros(k);rvec[good]=O[i+1,good]/O[i,good]-1
        r=float(np.nansum(curr*rvec));eq*=1+r;curve.append(eq);rrs.append(r)
    rr=np.array(rrs);ee=np.array(curve);sh=rr.mean()/rr.std(ddof=1)*np.sqrt(365)
    return {'N':len(rr),'sh':sh,'ann':(ee[-1]**(365/len(rr))-1)*100,'dd':np.max(1-ee/np.maximum.accumulate(ee))*100,'win':np.mean(rr>0)*100,'equity':ee[-1],'conc5':np.sort(rr)[-5:].sum()/rr.sum()*100 if rr.sum()>0 else np.nan,'turn_avg':np.mean(costs)}
blend=run_blend(teidx)
blend_vt=run_blend(teidx,vol_target=True)
print('CANDIDATE3 50/50 blend (2x)',{k:round(float(v),3) for k,v in blend.items()})
print('CANDIDATE3 50/50 blend (book_vol_estimate x2.7 / 2% target)',{k:round(float(v),3) for k,v in blend_vt.items()})
print('CANDIDATE3 random Sharpe >= model count / 20',sum(p['sh']>=blend['sh'] for p in places),'random median metrics', {key:round(float(np.nanmedian([p[key] for p in places])),3) for key in ['sh','ann','dd','win','equity']})
places_vt=[run(teidx,use_model=True,placebo=True,seed=seed,lev=5.4) for seed in range(20)]
print('CANDIDATE3 risk-target RANDOM20 median',{key:round(float(np.nanmedian([p[key] for p in places_vt])),3) for key in ['sh','ann','dd','win','equity']},'Sharpe>=model count',sum(p['sh']>=blend_vt['sh'] for p in places_vt))
# Funding-aware re-mark of the fixed 50/50 candidate; missing coin funding observations are set to zero, so this is a lower-bound cost estimate.
fraw=json.load(open('/tmp/hl_funding_full.json')); fseries={c:(np.array(sorted(int(t) for t in d)),np.array([float(d[t]) for t in sorted(d,key=lambda z:int(z))])) for c,d in fraw.items()}
fund_daily=np.zeros((n,k)); fund_cover=np.zeros((n,k),dtype=bool)
for j,c in enumerate(names):
    if c not in fseries: continue
    tt,vv=fseries[c]
    for i in range(n-1):
        lo=np.searchsorted(tt,dates[i],side='left'); hi=np.searchsorted(tt,dates[i+1],side='left')
        if hi>lo: fund_daily[i,j]=vv[lo:hi].sum();fund_cover[i,j]=True

def run_blend_funding(lev=2.0,vol_target=False):
    schedule=[]
    for ii in teidx:
        d,ix,js,y,pos=train_days[ii];a=model_save.predict(X[ix-1,js]);b=model2.predict(X[ix-1,js]);sc=.5*pd.Series(a).rank(pct=True).values+.5*pd.Series(b).rank(pct=True).values;order=np.argsort(sc);q=min(6,len(js)//2);lg=js[order[-q:]];sh=js[order[:q]]
        w=np.zeros(k);w[lg]=.5/len(lg);w[sh]=-.5/len(sh);w*=2.7*volscale(w,ix-1) if vol_target else lev;schedule.append((ix,w))
    eq=1.;curr=np.zeros(k);curve=[];dr=[];charged=[];exposure=[];p=0;ix0=schedule[0][0];ix1=schedule[-1][0]
    for i in range(ix0,ix1):
        if p<len(schedule) and i==schedule[p][0]:
            nw=schedule[p][1];turn=np.abs(nw-curr).sum();eq*=max(0,1-.00073*turn);curr=nw.copy();p+=1
        px=np.isfinite(O[i])&np.isfinite(O[i+1]);rr=np.zeros(k);rr[px]=O[i+1,px]/O[i,px]-1
        fr=-float(np.nansum(curr*fund_daily[i]));cov=np.sum(np.abs(curr)*fund_cover[i])/max(np.sum(np.abs(curr)),1e-12);r=float(np.nansum(curr*rr))+fr;eq*=1+r;curve.append(eq);dr.append(r);charged.append(fr);exposure.append(cov)
    rr=np.array(dr);ee=np.array(curve);return {'N':len(rr),'sh':rr.mean()/rr.std(ddof=1)*np.sqrt(365),'ann':(ee[-1]**(365/len(rr))-1)*100,'dd':np.max(1-ee/np.maximum.accumulate(ee))*100,'win':np.mean(rr>0)*100,'equity':ee[-1],'conc5':np.sort(rr)[-5:].sum()/rr.sum()*100 if rr.sum()>0 else np.nan,'funding_pct_equity':sum(charged)*100,'funding_coverage':float(np.mean(exposure))}
print('FUNDING INCLUDED, 28 coin coverage only; 2x',{k:round(float(v),3) for k,v in run_blend_funding().items()})
print('FUNDING INCLUDED, 28 coin coverage only; book vol',{k:round(float(v),3) for k,v in run_blend_funding(vol_target=True).items()})
def run_random_funding(seed,lev=2.0,vol_target=False):
    rng=np.random.default_rng(seed);sch=[]
    for ii in teidx:
        d,ix,js,y,pos=train_days[ii];perm=rng.permutation(js);q=min(6,len(js)//2);lg=perm[-q:];sh=perm[:q];w=np.zeros(k);w[lg]=.5/q;w[sh]=-.5/q;w*=2.7*volscale(w,ix-1) if vol_target else lev;sch.append((ix,w))
    eq=1.;curr=np.zeros(k);e=[];rrs=[];p=0
    for i in range(sch[0][0],sch[-1][0]):
        if p<len(sch) and i==sch[p][0]:nw=sch[p][1];eq*=max(0,1-.00073*np.abs(nw-curr).sum());curr=nw.copy();p+=1
        good=np.isfinite(O[i])&np.isfinite(O[i+1]);rv=np.zeros(k);rv[good]=O[i+1,good]/O[i,good]-1;fund=-float(np.nansum(curr*fund_daily[i]));r=float(np.nansum(curr*rv))+fund;eq*=1+r;e.append(eq);rrs.append(r)
    rr=np.array(rrs);ee=np.array(e);return {'sh':rr.mean()/rr.std(ddof=1)*np.sqrt(365),'ann':(ee[-1]**(365/len(rr))-1)*100,'dd':np.max(1-ee/np.maximum.accumulate(ee))*100,'win':np.mean(rr>0)*100,'equity':ee[-1]}
rv2=[run_random_funding(s,lev=2.0) for s in range(100,120)];rvt=[run_random_funding(s,vol_target=True) for s in range(100,120)]
model2x=run_blend_funding();modelvt=run_blend_funding(vol_target=True)
print('MATCHED FUNDING random20, 2x median',{k:round(float(np.median([x[k] for x in rv2])),3) for k in rv2[0]},'Sharpe>=model',sum(x['sh']>=model2x['sh'] for x in rv2))
print('MATCHED FUNDING random20, bookvol median',{k:round(float(np.median([x[k] for x in rvt])),3) for k in rvt[0]},'Sharpe>=model',sum(x['sh']>=modelvt['sh'] for x in rvt))
def run_rank_funding(which,lev=2.,vol_target=False):
    mdl=model_save if which=='imit' else model2;sch=[]
    for ii in teidx:
        d,ix,js,y,pos=train_days[ii];sc=mdl.predict(X[ix-1,js]);order=np.argsort(sc);q=min(6,len(js)//2);lg=js[order[-q:]];sh=js[order[:q]];w=np.zeros(k);w[lg]=.5/q;w[sh]=-.5/q;w*=2.7*volscale(w,ix-1) if vol_target else lev;sch.append((ix,w))
    eq=1.;curr=np.zeros(k);curve=[];dr=[];p=0
    for i in range(sch[0][0],sch[-1][0]):
        if p<len(sch) and i==sch[p][0]:nw=sch[p][1];eq*=max(0,1-.00073*np.abs(nw-curr).sum());curr=nw.copy();p+=1
        good=np.isfinite(O[i])&np.isfinite(O[i+1]);rv=np.zeros(k);rv[good]=O[i+1,good]/O[i,good]-1;r=float(np.nansum(curr*rv))-float(np.nansum(curr*fund_daily[i]));eq*=1+r;curve.append(eq);dr.append(r)
    rr=np.array(dr);ee=np.array(curve);return {'sh':rr.mean()/rr.std(ddof=1)*np.sqrt(365),'ann':(ee[-1]**(365/len(rr))-1)*100,'dd':np.max(1-ee/np.maximum.accumulate(ee))*100,'win':np.mean(rr>0)*100,'equity':ee[-1],'conc5':np.sort(rr)[-5:].sum()/rr.sum()*100 if rr.sum()>0 else np.nan}
for which in ['imit','ret']:
    for vt in [False,True]:print('SINGLE MODEL FUNDING',which,'risk-target' if vt else '2x',{k:round(float(v),3) for k,v in run_rank_funding(which,vol_target=vt).items()})
