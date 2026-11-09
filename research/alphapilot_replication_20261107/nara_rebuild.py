import json, glob, datetime, warnings, os
from pathlib import Path
import numpy as np
import pandas as pd
import lightgbm as lgb
warnings.filterwarnings('ignore')

ROOT=Path('/tmp/hl2'); OUT=Path(__file__).parent
COST=0.00073; VT=0.02; LEV=2.7; VOLWIN=20; MAX_UNI=33; N_SIDE=6

def utc_date(t): return datetime.datetime.fromtimestamp(t/1000,datetime.timezone.utc).date()
def load_panel():
    raw={}
    for f in glob.glob(str(ROOT/'*.json')):
        name=Path(f).stem
        with open(f) as h: rows=json.load(h)
        rows=[r for r in rows if float(r['c'])>0]
        if len(rows)>=120: raw[name]=rows
    names=sorted(raw); dates=sorted({r['t'] for rows in raw.values() for r in rows}); ti={t:i for i,t in enumerate(dates)}
    n,k=len(dates),len(names); mats={x:np.full((n,k),np.nan) for x in ['o','h','l','c','v','n']}
    for j,name in enumerate(names):
        for r in raw[name]:
            i=ti[r['t']]
            for x in mats:
                try: mats[x][i,j]=float(r[x])
                except: pass
    return names,dates,mats

def roll(a,w,op,minp=None):
    minp=minp or w
    return pd.DataFrame(a).rolling(w,min_periods=minp).agg(op).values

def make_features(names,dates,m):
    O,H,L,C,V,N=(m[x] for x in ['o','h','l','c','v','n']); eps=1e-12
    r=np.full_like(C,np.nan); r[1:]=C[1:]/C[:-1]-1
    logv=np.log(np.maximum(V,eps)); volchg=np.full_like(V,np.nan); volchg[1:]=V[1:]/np.maximum(V[:-1],eps)-1
    X={}
    def add(n,x): X[n]=x
    # 13 current bar features. Daily VWAP is not present in source bars; HLC3 proxy is explicitly flagged.
    rng=np.maximum(H-L,eps); oc=np.minimum(O,C); cc=np.maximum(O,C)
    add('kmid',(C-O)/np.maximum(O,eps)); add('klen',(H-L)/np.maximum(O,eps)); add('kmid_2',(C-O)/rng)
    add('kup',(H-cc)/np.maximum(O,eps)); add('kup_2',(H-cc)/rng); add('klow',(oc-L)/np.maximum(O,eps)); add('klow_2',(oc-L)/rng)
    add('ksft',(2*C-H-L)/np.maximum(O,eps)); add('ksft_2',(2*C-H-L)/rng)
    add('open_0',O/np.maximum(C,eps)); add('high_0',H/np.maximum(C,eps)); add('low_0',L/np.maximum(C,eps))
    add('vwap_0',((H+L+C)/3)/np.maximum(C,eps))
    for w in [5,10,20,30,60]:
        # approximate Alpha158/Nara daily rolling definitions; all use data through current close only.
        roc=np.full_like(C,np.nan); roc[w:]=C[:-w]/np.maximum(C[w:],eps) # delay(close,w)/close
        add(f'roc_{w}',roc); add(f'ma_{w}',roll(C,w,'mean',max(2,w//2))/np.maximum(C,eps))
        up=(r>0).astype(float); dn=(r<0).astype(float)
        cntp=roll(up,w,'mean',max(2,w//2)); cntn=roll(dn,w,'mean',max(2,w//2))
        add(f'cntp_{w}',cntp); add(f'cntn_{w}',cntn); add(f'cntd_{w}',cntp-cntn)
        add(f'std_{w}',roll(C,w,'std',max(2,w//2))/np.maximum(C,eps))
        add(f'max_{w}',roll(H,w,'max',max(2,w//2))/np.maximum(C,eps)); add(f'min_{w}',roll(L,w,'min',max(2,w//2))/np.maximum(C,eps))
        add(f'qtlu_{w}',roll(C,w,lambda z: np.quantile(z,.8),max(2,w//2))/np.maximum(C,eps)); add(f'qtld_{w}',roll(C,w,lambda z: np.quantile(z,.2),max(2,w//2))/np.maximum(C,eps))
        imax=roll(H,w,'max',max(2,w//2)); imin=roll(L,w,'min',max(2,w//2))
        ixmax=np.full_like(C,np.nan); ixmin=np.full_like(C,np.nan)
        for j in range(len(names)):
            hs=pd.Series(H[:,j]); ls=pd.Series(L[:,j])
            ixmax[:,j]=hs.rolling(w,min_periods=max(2,w//2)).apply(lambda z: int(np.argmax(z)),raw=True).values
            ixmin[:,j]=ls.rolling(w,min_periods=max(2,w//2)).apply(lambda z: int(np.argmin(z)),raw=True).values
        ixmax/=w; ixmin/=w; add(f'imax_{w}',ixmax); add(f'imin_{w}',ixmin); add(f'imxd_{w}',ixmax-ixmin)
        # time-series rank of current close in trailing window (empirical percentile)
        rank=np.full_like(C,np.nan)
        for j in range(len(names)):
            rank[:,j]=pd.Series(C[:,j]).rolling(w,min_periods=max(2,w//2)).apply(lambda z: np.mean(z<=z[-1]),raw=True).values
        add(f'rank_{w}',rank)
        hi=roll(H,w,'max',max(2,w//2)); lo=roll(L,w,'min',max(2,w//2)); add(f'rsv_{w}',(C-lo)/np.maximum(hi-lo,eps))
        corr=np.full_like(C,np.nan); cord=np.full_like(C,np.nan)
        for j in range(len(names)):
            corr[:,j]=pd.Series(C[:,j]).rolling(w,min_periods=max(3,w//2)).corr(pd.Series(V[:,j])).values
            cord[:,j]=pd.Series(r[:,j]).rolling(w,min_periods=max(3,w//2)).corr(pd.Series(volchg[:,j])).values
        add(f'corr_{w}',corr); add(f'cord_{w}',cord)
        add(f'vma_{w}',roll(V,w,'mean',max(2,w//2))/np.maximum(V,eps)); add(f'vstd_{w}',roll(V,w,'std',max(2,w//2))/np.maximum(V,eps))
        # Volume-weighted return volatility, interpreted as sqrt(sum(v*r^2)/sum(v)).
        wv=np.full_like(C,np.nan)
        for j in range(len(names)):
            num=pd.Series(V[:,j]*r[:,j]**2).rolling(w,min_periods=max(2,w//2)).sum()
            den=pd.Series(V[:,j]).rolling(w,min_periods=max(2,w//2)).sum()
            wv[:,j]=np.sqrt(num/np.maximum(den,eps))
        add(f'wvma_{w}',wv)
        pos=np.maximum(r,0); neg=np.maximum(-r,0)
        sump=roll(pos,w,'sum',max(2,w//2)); sumn=roll(neg,w,'sum',max(2,w//2))
        add(f'sump_{w}',sump); add(f'sumn_{w}',sumn); add(f'sumd_{w}',sump-sumn)
        # volume portions allocated to positive/negative return days
        vp=roll(V*(r>0),w,'sum',max(2,w//2)); vn=roll(V*(r<0),w,'sum',max(2,w//2)); vd=roll(V,w,'sum',max(2,w//2))
        vsp=vp/np.maximum(vd,eps); vsn=vn/np.maximum(vd,eps); add(f'vsump_{w}',vsp); add(f'vsumn_{w}',vsn); add(f'vsumd_{w}',vsp-vsn)
    # crypto-native fields; unavailable OI and premium remain NaN
    funding={}
    try:
        obj=json.load(open('/tmp/hl_funding_full.json'))
        for c,vals in obj.items(): funding[c]=sorted((int(t),float(v)) for t,v in vals.items())
    except Exception: pass
    fr1=np.full_like(C,np.nan); fr7=np.full_like(C,np.nan)
    for j,c in enumerate(names):
        if c not in funding: continue
        tt=np.array([x[0] for x in funding[c]],dtype=np.int64); vv=np.array([x[1] for x in funding[c]])
        for i,t in enumerate(dates):
            # funding hourly bars through daily close timestamp; rolling means count hours, then daily means.
            end=np.searchsorted(tt,t+86400000,side='left'); start=max(0,end-24); st7=max(0,end-168)
            if end>start: fr1[i,j]=np.mean(vv[start:end])
            if end>st7: fr7[i,j]=np.mean(vv[st7:end])
    add('fr_1d',fr1); add('fr_7d',fr7); add('prem_1d',np.full_like(C,np.nan)); add('oi_chg_1d',np.full_like(C,np.nan)); add('oi_chg_7d',np.full_like(C,np.nan))
    tc= N/np.maximum(roll(N,7,'mean',3),eps); add('tc_ratio',tc)
    dv=C*V; add('log_dv',np.log(np.maximum(roll(dv,7,'mean',4),eps))); add('log_price',np.log(np.maximum(C,eps)))
    assert len(X)==151, f'Feature count {len(X)}'
    return X, dv

def build_dataset(names,dates,m,X,dv):
    C=m['c']; O=m['o']; n,k=C.shape; dts=[utc_date(t) for t in dates]
    # Universe at signal t: top 33 by trailing 30d dollar volume available by t-2.
    dvol=pd.DataFrame(dv).rolling(30,min_periods=15).mean().shift(2).values
    # target is cross-sectional percentile of 3-day open-to-open return, enter next open.
    feats=list(X); Xm=np.stack([X[f] for f in feats],axis=2)
    rows=[]; y=[]; meta=[]
    for s in range(65,n-4):
        if dts[s] < datetime.date(2024,6,15): continue
        eligible=np.isfinite(dvol[s]) & (dvol[s]>0) & np.isfinite(O[s+1]) & np.isfinite(O[s+4])
        ids=np.flatnonzero(eligible)
        if len(ids)<12: continue
        if len(ids)>MAX_UNI:
            ids=ids[np.argsort(dvol[s,ids])[-MAX_UNI:]]
        fwd=O[s+4,ids]/O[s+1,ids]-1
        labels=pd.Series(fwd).rank(pct=True,method='average').values
        rows.append(Xm[s,ids,:]); y.extend(labels.tolist()); meta.extend([(s,j,float(fwd[q])) for q,j in enumerate(ids)])
    return feats, rows, np.asarray(y), meta,dvol

def book_vol_estimate(w,s,R):
    # Target position 20-day weighted covariance, then multiply 2.7 exactly as requested.
    js=np.flatnonzero(np.abs(w)>1e-12)
    if len(js)<4 or s<20:return np.nan
    mat=R[s-19:s+1,js]
    if np.any(np.sum(np.isfinite(mat),axis=0)<16):return np.nan
    # Covariance based on complete aligned rows; this is the weighted covariance of the current target book.
    good=np.all(np.isfinite(mat),axis=1)
    if good.sum()<12:return np.nan
    cov=np.cov(mat[good],rowvar=False,ddof=1)
    ww=w[js]
    pvar=float(ww@cov@ww)
    return np.sqrt(max(pvar,0))*LEV if pvar>0 else np.nan

def backtest(names,dates,m,scorer,first,last,label='model',seed=42,risk_target=True):
    C=m['c']; O=m['o']; V=m['v']; R=np.full_like(C,np.nan); R[1:]=C[1:]/C[:-1]-1
    dts=[utc_date(t) for t in dates]; dv=C*V; liq=pd.DataFrame(dv).rolling(30,min_periods=15).mean().shift(2).values
    # hourly funding accrued by daily close to approximate daily funding debits/credits.
    fund={}
    obj=json.load(open('/tmp/hl_funding_full.json'))
    for c,vals in obj.items(): fund[c]=sorted((int(t),float(v)) for t,v in vals.items())
    frdaily=np.zeros_like(C)
    for j,c in enumerate(names):
        if c not in fund:continue
        tt=np.array([x[0] for x in fund[c]],dtype=np.int64); vv=np.array([x[1] for x in fund[c]])
        for i,t in enumerate(dates):
            e=np.searchsorted(tt,t+86400000,side='left'); b=np.searchsorted(tt,t,side='left')
            if e>b: frdaily[i,j]=vv[b:e].sum()
    start=next(i for i,d in enumerate(dts) if d>=first); end=max(i for i,d in enumerate(dts) if d<=last)
    eq=1.; qty=np.zeros(len(names)); weights=np.zeros(len(names)); curve=[]; daily=[]; datesout=[]; turnover=[]; perasset=np.zeros(len(names)); allret=[]
    rng=np.random.default_rng(seed)
    for i in range(start,end):
        eqprev=eq
        # Execute at today's open; mark open-to-open until the next rebalance bar.
        if (i-start)%3==0:
            s=i-1
            eligible=np.isfinite(liq[s])&(liq[s]>0)&np.isfinite(O[i])
            ids=np.flatnonzero(eligible)
            if len(ids)>MAX_UNI: ids=ids[np.argsort(liq[s,ids])[-MAX_UNI:]]
            if len(ids)>=2*N_SIDE:
                score=scorer(s,ids)
                if label=='random': score=rng.standard_normal(len(ids))
                order=np.argsort(score); longids=ids[order[-N_SIDE:]]; shortids=ids[order[:N_SIDE]]
                w=np.zeros(len(names)); w[longids]=.5/N_SIDE; w[shortids]=-.5/N_SIDE
                v=book_vol_estimate(w,s,R)
                mult=(np.clip(VT/v,.2,3.) if np.isfinite(v) and v>0 else .45) if risk_target else 2.0
                w*=mult
            else:w=np.zeros(len(names))
            turn=float(np.abs(w-weights).sum()); turnover.append(turn)
            eq*=max(1-COST*turn,1e-6); weights=w
        # funding at daily rates; + funding means longs pay shorts
        eq*=max(1-float(np.sum(weights*frdaily[i])),1e-6)
        if i<end:
            rr=np.zeros(len(names)); good=np.isfinite(O[i])&np.isfinite(O[i+1]); rr[good]=O[i+1,good]/O[i,good]-1
            d=float(np.sum(weights*rr)); eq*=max(1+d,1e-6); dnet=eq/eqprev-1; daily.append(dnet); allret.append(dnet)
            curve.append(eq); datesout.append(dts[i].isoformat())
    rr=np.array(daily); cc=np.array(curve)
    if not len(rr):return {}
    sh=rr.mean()/rr.std(ddof=1)*np.sqrt(365) if rr.std(ddof=1)>0 else np.nan
    ann=(cc[-1]**(365/len(rr))-1)*100 if cc[-1]>0 else -100
    dd=np.max(1-cc/np.maximum.accumulate(np.r_[1.,cc])[1:])*100
    top5=np.sort(rr)[-5:].sum()/rr.sum()*100 if rr.sum()>0 else np.nan
    return dict(label=label,start=datesout[0],end=datesout[-1],days=len(rr),sharpe=float(sh),annual_pct=float(ann),maxdd_pct=float(dd),win_pct=float((rr>0).mean()*100),top5_gain_share_pct=float(top5),equity=float(cc[-1]),mean_turnover_pct=float(np.mean(turnover)*100),daily=rr,curve=cc,dates=datesout)

def main():
    names,dates,m=load_panel(); print(f'data={len(names)} assets {utc_date(dates[0])}..{utc_date(dates[-1])}',flush=True)
    X,dv=make_features(names,dates,m); feats,blocks,y,meta,dvol=build_dataset(names,dates,m,X,dv)
    n=len(dates); val_end=next((i for i,d in enumerate([utc_date(t) for t in dates]) if d>datetime.date(2026,4,30)),n-4); cut=next(i for i,d in enumerate([utc_date(t) for t in dates]) if d>=datetime.date(2025,9,30)); splitdate=utc_date(dates[cut]); print(f'features={len(feats)}, site_validation_start={splitdate}, rows={len(y)}',flush=True)
    # Reconstruct the site's reported chronological boundary (validation begins 2025-09-30); local data starts later than AlphaPilot's.
    # Purge train outcomes whose realization crosses into validation.
    si=np.array([x[0] for x in meta]); jj=np.array([x[1] for x in meta]);
    Xall=np.concatenate(blocks,axis=0)
    trainmask=si+4<cut
    valmask=(si>=cut)&(si+4<val_end)
    testmask=(si>=val_end)&(si+4<n-1)
    print(f'train date range {utc_date(dates[si[trainmask].min()])}..{utc_date(dates[si[trainmask].max()])} rows {trainmask.sum()}; val {utc_date(dates[si[valmask].min()])}..{utc_date(dates[si[valmask].max()])} rows {valmask.sum()}; test from {utc_date(dates[val_end])} rows {testmask.sum()}',flush=True)
    params=dict(objective='regression',metric='rmse',learning_rate=.05,num_leaves=31,min_data_in_leaf=200,feature_fraction=.8,bagging_fraction=.8,bagging_freq=1,lambda_l2=10,seed=42,verbosity=-1,num_threads=6)
    dtr=lgb.Dataset(Xall[trainmask],label=y[trainmask],feature_name=feats); dva=lgb.Dataset(Xall[valmask],label=y[valmask],feature_name=feats,reference=dtr)
    model=lgb.train(params,dtr,num_boost_round=1000,valid_sets=[dva],callbacks=[lgb.early_stopping(100,verbose=True),lgb.log_evaluation(50)])
    print(f'best_iteration={model.best_iteration}',flush=True)
    # create a fast prediction cache for each signal day; model sees only signal-day features.
    scorecache={}
    for s in range(cut,n-4):
        ids=np.flatnonzero(np.isfinite(dvol[s])&(dvol[s]>0))
        if len(ids)>MAX_UNI:ids=ids[np.argsort(dvol[s,ids])[-MAX_UNI:]]
        scorecache[s]=(ids,model.predict(np.stack([X[f][s,ids] for f in feats],axis=1),num_iteration=model.best_iteration))
    def score(s,ids):
        # use cached day prediction in same id order as backtest liquidity universe
        a,b=scorecache.get(s,(None,None))
        if a is None:return np.zeros(len(ids))
        mp={j:v for j,v in zip(a,b)}
        return np.array([mp.get(j,0.) for j in ids])
    # Determine test interval exactly to local dataset end; also report site-window comparison.
    dlist=[utc_date(t) for t in dates]
    # Validate Nara model over the distinct out-of-sample interval and matched website statistic dates.
    results=[]
    test=backtest(names,dates,m,score,datetime.date(2026,5,4),dlist[-2],label='nara_model')
    apwin=backtest(names,dates,m,score,datetime.date(2026,5,4),datetime.date(2026,10,7),label='nara_model_site_window')
    apwin2x=backtest(names,dates,m,score,datetime.date(2026,5,4),datetime.date(2026,10,7),label='nara_model_site_window_2x',risk_target=False)
    rand=backtest(names,dates,m,score,datetime.date(2026,5,4),dlist[-2],label='random',seed=42)
    rand_many=[backtest(names,dates,m,score,datetime.date(2026,5,4),dlist[-2],label='random',seed=z) for z in range(20)]
    # early-stop validation strategy path is diagnostic, not independent.
    val=backtest(names,dates,m,score,splitdate,datetime.date(2026,4,30),label='nara_validation')
    for r in [val,apwin,apwin2x,test,rand]:
        print({k:(round(v,4) if isinstance(v,(float,np.floating)) else v) for k,v in r.items() if k not in ('daily','curve','dates')},flush=True)
    fi=sorted(zip(feats,model.feature_importance(importance_type='gain')),key=lambda z:z[1],reverse=True)
    # IC on truly held-out test interval: within-date Spearman prediction vs 3d forward returns.
    ic=[]
    for s in range(val_end,n-4):
        ids=np.flatnonzero(np.isfinite(dvol[s])&(dvol[s]>0))
        if len(ids)>MAX_UNI:ids=ids[np.argsort(dvol[s,ids])[-MAX_UNI:]]
        if len(ids)<12:continue
        sc=score(s,ids); fwd=m['o'][s+4,ids]/m['o'][s+1,ids]-1
        ic.append(pd.Series(sc).corr(pd.Series(fwd),method='spearman'))
    ic=np.asarray(ic); ic=ic[np.isfinite(ic)]
    metrics={k:v for k,v in test.items() if k not in ('daily','curve','dates')}; metrics['site_display_window']={k:v for k,v in apwin.items() if k not in ('daily','curve','dates')}; metrics['site_display_window_fixed_2x']={k:v for k,v in apwin2x.items() if k not in ('daily','curve','dates')}; metrics['validation']={k:v for k,v in val.items() if k not in ('daily','curve','dates')}; metrics['random_control']={k:v for k,v in rand.items() if k not in ('daily','curve','dates')}; metrics['random_20_seed_median']={k:float(np.nanmedian([z[k] for z in rand_many])) for k in ['sharpe','annual_pct','maxdd_pct','win_pct','top5_gain_share_pct']}
    metrics['data']={'assets':len(names),'start':str(utc_date(dates[0])),'end':str(utc_date(dates[-1])),'features':len(feats),'available_feature_groups':['OHLCV','trade_count','funding_rate'],'unavailable':['open_interest','premium','true_vwap'],'split_date':str(splitdate),'best_iteration':model.best_iteration,'train_rows':int(trainmask.sum()),'validation_rows':int(valmask.sum()),'test_rows':int(testmask.sum()),'test_ic_mean':float(ic.mean()) if len(ic) else None,'test_ic_positive_fraction':float((ic>0).mean()) if len(ic) else None,'feature_gain_top20':fi[:20]}
    for key,r in [('validation',val),('site_window',apwin),('site_window_2x',apwin2x),('test',test),('random',rand)]:
        pd.DataFrame({'date':r['dates'],'daily_return':r['daily'],'equity':r['curve']}).to_csv(OUT/f'nara_{key}_daily.csv',index=False)
    json.dump(metrics,open(OUT/'nara_metrics.json','w'),indent=2,ensure_ascii=False,default=lambda x:float(x) if isinstance(x,np.floating) else x)
    print('top features',fi[:20],flush=True)
if __name__=='__main__':main()
