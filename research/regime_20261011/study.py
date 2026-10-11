"""Frozen candidate definitions and exact execution replay; see PLAN.md."""
import json, math, hashlib
import numpy as np
import pandas as pd
from common import *
WINDOWS={'corr':[30,60,90],'return_disp':[10,20,40],'score_disp':[1,10,20],'raw_mom_disp':[1,10,20],'top5':[1,10,20],'hhi':[1,10,20],'universe':[1,10,20],'volume_count':[1,10,20],'factor_ac':[30,60,90],'neff':[1,10,20],'realized_vol':[20,60]}
DIRECTION={f:(1 if f in ['corr','top5','hhi','realized_vol'] else -1) for f in WINDOWS}
MINHIST=60

def rank_past(x):
    ans=np.full(len(x),np.nan); hist=[]
    for t,v in enumerate(x):
        if np.isfinite(v):
            if len(hist)>=MINHIST:
                h=np.asarray(hist);ans[t]=((h<v).sum()+.5*(h==v).sum())/len(h)
            hist.append(v)
    return ans

def thresholds(x,side):
    s=pd.Series(x);q=s.expanding(min_periods=MINHIST).quantile(.8 if side==1 else .2).shift(1).to_numpy()
    return q,np.isfinite(x)&np.isfinite(q)&((x>q) if side==1 else (x<q))

class NeffGate:
    def __init__(self,window):
        self.window=window;self.raw=[];self.series={w:[] for w in WINDOWS['neff']};self.composite=[];self.values=[];self.cutoffs=[];self.flags=[]
    def update(self,v):
        self.raw.append(v);ranks=[];values={}
        for w in self.series:
            x=float(np.mean(self.raw[-w:])) if len(self.raw)>=w and np.isfinite(self.raw[-w:]).all() else np.nan
            h=np.array(self.series[w]);h=h[np.isfinite(h)]
            ranks.append(((h<x).sum()+.5*(h==x).sum())/len(h) if len(h)>=MINHIST and np.isfinite(x) else np.nan)
            values[w]=(x,float(np.quantile(h,.2)) if len(h)>=MINHIST else np.nan)
            self.series[w].append(x)
        comp=float(np.mean(ranks));h=np.array(self.composite);h=h[np.isfinite(h)]
        qc=float(np.quantile(h,.2)) if len(h)>=MINHIST else np.nan;self.composite.append(comp)
        x,q=(comp,qc) if self.window=='avg' else values[self.window]
        flag=bool(np.isfinite(x) and np.isfinite(q) and x<q)
        self.values.append(x);self.cutoffs.append(q);self.flags.append(flag)
        return flag

class Engine:
    def __init__(self,g):
        self.g=g;self.n=g['n'];self.k=g['k'];self.ii=np.arange(33,self.n);self.N=len(self.ii);self.names=np.array(g['names'])
        self.elig=np.zeros((self.n,self.k),bool);self.w=np.zeros((self.n,self.k));self.mw=self.w.copy()
        for i in range(32,self.n):
            idx=np.flatnonzero((g['V'][i]>=5e6)&np.isfinite(g['P'][i])&np.isfinite(g['P'][i-14])&np.isfinite(g['SD'][i])&np.array([':' not in c for c in self.names]));self.elig[i,idx]=True
            self.w[i]=g['target'](i,1e6,5)
            if len(idx)>=8:
                kk=min(max(1,int(np.floor(len(idx)*.2+.5))),5,len(idx)//2)
                score=g['M'][i]/np.maximum(g['SD'][i],1e-9);order=idx[np.lexsort((self.names[idx],score[idx]))]
                self.mw[i,order[:kk]]=-.5/kk;self.mw[i,order[-kk:]]=.5/kk
        self.precision=np.array([10**g['meta'][c]['szDecimals'] for c in self.names])
    def run(self,fee=.0007,bad=None,neff_window=None,momentum=False):
        g=self.g;eq=1e6;qty=np.zeros(self.k);last=np.full(self.k,np.nan);rec=[];neffgate=NeffGate(neff_window) if neff_window is not None else None
        for t,i in enumerate(self.ii):
            px=g['a']['o'][i];valid=np.isfinite(px);mark=np.where(valid,px,last)
            assert not ((abs(qty)>1e-12)&~valid).any(),'Missing held quote'
            prevclose=np.where(np.isfinite(g['P'][i-1]),g['P'][i-1],last)
            absnot=np.nan_to_num(abs(qty)*prevclose);gross=absnot.sum();neff=gross*gross/(absnot@absnot) if gross>0 else np.nan
            flag=neffgate.update(neff) if neffgate else (bool(bad[t]) if bad is not None else False)
            leverage=1. if momentum or flag else 2.7
            gain=np.nansum(qty*(mark-last));before=eq;eq+=gain
            assert eq>0 and min(5,max(1,int(np.floor(eq*2.7/15))//4))==5
            w=(self.mw if momentum else self.w)[i-1]
            wanted=np.zeros(self.k);ok=valid&(abs(w)>=1e-12)
            size=np.floor(abs(w[ok])*eq*leverage/px[ok]*self.precision[ok])/self.precision[ok]
            wanted[ok]=np.where(size*px[ok]>=10,np.sign(w[ok])*size,0)
            d=wanted-qty;rounded=np.floor(abs(d)*self.precision)/self.precision
            delta=np.zeros(self.k)
            flip=valid&(qty*wanted<0);zero=valid&~flip&(wanted==0)&(abs(qty)*px>=10)
            trade=valid&~flip&(wanted!=0)&(rounded*px>=np.maximum(10,abs(wanted)*px*.02))
            delta[flip]=d[flip];delta[zero]=-qty[zero];delta[trade]=np.sign(d[trade])*rounded[trade]
            turnover=np.sum(abs(delta)*np.nan_to_num(px))/eq;cost=fee*np.sum(abs(delta)*np.nan_to_num(px))
            eq-=cost;qty+=delta;last=mark
            rec.append((eq/before-1,turnover,eq,neff,gain/before,leverage,int(flag),int(gross>0),float(np.sum(abs(qty)*np.nan_to_num(px))/eq)))
        out=pd.DataFrame(rec,columns=['r','turn','equity','neff','price_r','leverage','bad','earning_active','gross'])
        if neffgate:
            out['signal']=neffgate.values;out['threshold']=neffgate.cutoffs
        return out

def build_features(g,e,base,mom):
    N=e.N;daily={};di=e.ii-1
    raw={f:np.full(g['n'],np.nan) for f in ['return_disp','score_disp','raw_mom_disp','top5','hhi','universe','volume_count']}
    corr={w:np.full(g['n'],np.nan) for w in WINDOWS['corr']};coverage=[]
    for i in range(32,g['n']):
        idx=np.flatnonzero(e.elig[i]);raw['universe'][i]=len(idx);raw['volume_count'][i]=int((g['V'][i]>=5e6).sum())
        if len(idx)>=8:
            rr=g['R'][i,idx];rr=rr[np.isfinite(rr)];raw['return_disp'][i]=np.std(rr,ddof=1) if len(rr)>=8 else np.nan
            raw['score_disp'][i]=np.std(g['M'][i,idx]/np.maximum(g['SD'][i,idx],1e-9),ddof=1)
            raw['raw_mom_disp'][i]=np.std(g['M'][i,idx],ddof=1)
            for w in corr:
                c=pd.DataFrame(g['R'][max(1,i-w+1):i+1,idx]).corr(min_periods=max(20,math.ceil(.8*w))).to_numpy()
                vals=c[np.triu_indices(len(idx),1)];valid=np.isfinite(vals)
                corr[w][i]=float(np.mean(vals[valid])) if valid.any() else np.nan
                coverage.append((i,w,len(idx),int(valid.sum()),len(vals)))
        q=g['Q'][i];ok=np.isfinite(q)&(q>0)&np.array([':' not in c for c in e.names]);q=q[ok]
        if len(q):
            frac=q/q.sum();raw['top5'][i]=np.sort(frac)[-5:].sum();raw['hhi'][i]=frac@frac
    for f in raw:
        for w in WINDOWS[f]:daily[f'{f}_{w}']=pd.Series(raw[f]).rolling(w,min_periods=w).mean().to_numpy()[di]
    for w in corr:daily[f'corr_{w}']=corr[w][di]
    factor=mom.price_r.where(mom.earning_active.astype(bool))
    for w in WINDOWS['factor_ac']:
        daily[f'factor_ac_{w}']=factor.rolling(w,min_periods=w).apply(lambda x: np.corrcoef(x[:-1],x[1:])[0,1],raw=True).shift(1).to_numpy()
    for w in WINDOWS['neff']:daily[f'neff_{w}']=base.neff.rolling(w,min_periods=w).mean().to_numpy()
    for w in WINDOWS['realized_vol']:daily[f'realized_vol_{w}']=base.r.rolling(w,min_periods=w).std(ddof=1).shift(1).to_numpy()
    df=pd.DataFrame(daily)
    for f,ww in WINDOWS.items():
        ranks=np.column_stack([rank_past(df[f'{f}_{w}'].to_numpy()) for w in ww]);df[f'{f}_avg']=np.mean(ranks,axis=1)
    ranks=pd.DataFrame({c:rank_past(df[c].to_numpy()) for c in df})
    pd.DataFrame(coverage,columns=['i','window','universe','valid_pairs','all_pairs']).to_csv(OUT/'correlation_coverage.csv',index=False)
    return df,ranks

def main():
    g=load();e=Engine(g);base=e.run();ref=np.load(OUT/'baseline_cap5_open_0.0007.npz')
    diff=float(np.max(abs(base.r.to_numpy()-ref['r'])));tdiff=float(np.max(abs(base.turn.to_numpy()-ref['turn'])))
    assert diff<1e-12 and tdiff<1e-12,(diff,tdiff)
    mom=e.run(fee=0,momentum=True);features,ranks=build_features(g,e,base,mom)
    dates=pd.to_datetime(np.array(g['ts'])[e.ii],unit='ms').strftime('%Y-%m-%d');base.insert(0,'date',dates);mom.insert(0,'date',dates)
    base.to_csv(OUT/'baseline_daily.csv',index=False);mom.to_csv(OUT/'momentum_1x_daily.csv',index=False)
    features.insert(0,'date',dates);ranks.insert(0,'date',dates);features.to_csv(OUT/'features.csv',index=False);ranks.to_csv(OUT/'online_ranks.csv',index=False)
    uni=pd.DataFrame({'date':dates,'eligible':e.elig[e.ii-1].sum(1),'volume_only':(g['V'][e.ii-1]>=5e6).sum(1)})
    uni.to_csv(OUT/'universe_check.csv',index=False)
    policy=[];returns={};states={};details={};threshold_frame={}
    dd=(dates>'2024-03-05')&(dates<='2024-10-15');inc=(dates>='2024-03-05')&(dates<='2024-10-15');peak=int(np.flatnonzero(dates=='2024-03-05')[0])
    masks={'full':np.ones(e.N,bool),'2023-24':dates<'2025-01-01','2025-26':dates>='2025-01-01'}
    for fee in [.0007,.00075,.00045]:
        fb=base if fee==.0007 else e.run(fee=fee);returns[f'baseline@{fee}']=fb.r.to_numpy()
        for c in features.columns[1:]:
            f,w=c.rsplit('_',1);w=w if w=='avg' else int(w)
            q,bad=thresholds(features[c].to_numpy(),DIRECTION[f]);threshold_frame[c]=q
            out=e.run(fee=fee,bad=bad,neff_window=w if f=='neff' else None)
            key=f'{c}@{fee}';returns[key]=out.r.to_numpy();states[key]=out.bad.to_numpy()
            if fee==.0007:
                details[c]=out.to_numpy();
                if f=='neff':
                    # Endogenous portfolio concentration can differ from un-gated shadow holdings.
                    pd.DataFrame(dict(date=dates,signal=out.signal,threshold=out.threshold,bad=out.bad)).to_csv(OUT/f'neff_gate_{w}.csv',index=False)
            earning=np.r_[False,out.bad.to_numpy()[:-1].astype(bool)];pre20=out.bad.iloc[peak-20:peak].mean();pre60=out.bad.iloc[peak-60:peak].mean()
            first=np.flatnonzero(out.bad);warn=dict(coverage224=float(earning[dd].mean()),days224=int(earning[dd].sum()),coverage225=float(earning[inc].mean()),days225=int(earning[inc].sum()),pre20=float(pre20),pre60=float(pre60),peak_bad=int(out.bad.iloc[peak]),first20=float(earning[np.flatnonzero(dd)[:20]].mean()),first_bad=dates[first[0]] if len(first) else '',bad_fraction=float(out.bad.mean()),fixed_dd_return=stats(out.r.to_numpy()[dd])['total'],fixed_dd_mdd=stats(out.r.to_numpy()[dd])['mdd'])
            for period,mask in masks.items():
                st=stats(out.r.to_numpy()[mask]);bs=stats(fb.r.to_numpy()[mask]);policy.append(dict(candidate=c,fee=fee,period=period,**st,**warn,turn=float(out.turn.to_numpy()[mask].mean()),annual_cost=float(out.turn.to_numpy()[mask].mean()*fee*365),sharpe_delta=st['sharpe']-bs['sharpe'],terminal_retention=(1+st['total'])/(1+bs['total']),profit_loss=bs['total']-st['total'],daily_return_corr=float(np.corrcoef(out.r.to_numpy()[mask],fb.r.to_numpy()[mask])[0,1])))
            print('policy',key,stats(out.r)['sharpe'],flush=True)
    pd.DataFrame(policy).to_csv(OUT/'policy_metrics.csv',index=False);np.savez_compressed(OUT/'policy_returns.npz',**returns);np.savez_compressed(OUT/'policy_states.npz',**states)
    pd.DataFrame(threshold_frame).assign(date=dates).to_csv(OUT/'thresholds.csv',index=False)
    (OUT/'replay_verification.json').write_text(json.dumps(dict(max_return_difference=diff,max_turnover_difference=tdiff,n_features=features.shape[1]-1,n_price_files=len(g['files']),volume_count_diff_days=int((uni.eligible!=uni.volume_only).sum())),indent=2))
    print('PREPARATION DONE',flush=True)
if __name__=='__main__':main()
