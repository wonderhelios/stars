"""Joint centered circular-block maxT bootstrap of delta-method influence functions."""
import json
import numpy as np
import pandas as pd
from common import OUT,stats
B=9999;BLOCKS=[10,20,40,80,120];SEED=20261011

def correlation(x,y,mask):
    good=mask&np.isfinite(x)&np.isfinite(y);n=good.sum();phi=np.zeros(len(x))
    if n<30 or np.std(x[good])<=1e-14 or np.std(y[good])<=1e-14:return np.nan,phi,n
    zx=(x[good]-np.mean(x[good]))/np.std(x[good]);zy=(y[good]-np.mean(y[good]))/np.std(y[good]);rho=float(np.mean(zx*zy))
    phi[good]=(zx*zy-rho*.5*(zx*zx+zy*zy))*len(x)/n
    return rho,phi,int(n)

def sharp(x,mask):
    good=mask&np.isfinite(x);n=good.sum();phi=np.zeros(len(x))
    if n<20 or np.std(x[good])<=1e-14:return np.nan,phi,int(n)
    v=x[good];mu=np.mean(v);sd=np.std(v);adj=np.sqrt(365*(n-1)/n)
    theta=adj*mu/sd
    phi[good]=adj*((v-mu)/sd-mu/(2*sd**3)*((v-mu)**2-sd**2))*len(x)/n
    return float(theta),phi,int(n)

def bootstrap(phi,theta,L):
    N,M=phi.shape;rng=np.random.default_rng(SEED+L)
    # Exact N-length concatenated circular blocks, including the last partial block.
    ext=np.concatenate([phi,phi[:L]],axis=0);cs=np.vstack([np.zeros(M),np.cumsum(ext,axis=0)])
    starts=np.arange(N);sums=cs[starts+L]-cs[starts]
    q,rem=divmod(N,L);tail=cs[starts+rem]-cs[starts] if rem else None
    dist=np.empty((B,M))
    for b in range(0,B,64):
        size=min(64,B-b);ind=rng.integers(0,N,size=(size,q+(rem>0)))
        boot=sums[ind[:,:q]].sum(axis=1)
        if rem:boot+=tail[ind[:,-1]]
        dist[b:b+size]=boot/N
    se=dist.std(axis=0,ddof=1);valid=np.isfinite(theta)&(se>1e-14)
    t=np.divide(abs(theta),se,out=np.zeros(M),where=valid)
    ts=np.divide(abs(dist),se[None,:],out=np.zeros_like(dist),where=valid[None,:])
    maxima=ts.max(axis=1);pf=(1+(maxima[:,None]>=t[None,:]).sum(0))/(B+1);p=(1+(ts>=t[None,:]).sum(0))/(B+1)
    pf[~valid]=1;p[~valid]=1
    return se,p,pf,maxima

def main():
    feat=pd.read_csv(OUT/'features.csv');rank=pd.read_csv(OUT/'online_ranks.csv');base=pd.read_csv(OUT/'baseline_daily.csv');pr=np.load(OUT/'policy_returns.npz')
    dates=feat.date.to_numpy();N=len(dates);r=base.r.to_numpy();idx=np.arange(N)
    masks={'full':np.ones(N,bool),'2023-24':dates<'2025-01-01','2025-26':dates>='2025-01-01'}
    future={}
    for h in [1,20,60]:
        y=np.full(N,np.nan)
        for t in range(N-h):y[t]=np.prod(1+r[t+1:t+h+1])-1
        future[h]=y
    rows=[];ifs=[];groups=[];phases=[]
    for c in feat.columns[1:]:
        x=feat[c].to_numpy();rk=rank[c].to_numpy()
        for period,mask in masks.items():
            for h in [20,60]:
                m=mask&(idx+h<N);m[m]=mask[(idx+h)[m]]
                rho,phi,n=correlation(x,future[h],m)
                active=base.gross.to_numpy()>0;ra,_,na=correlation(x,future[h],m&active)
                values=[]
                for phase in range(h):
                    pm=m&(idx%h==phase)&np.isfinite(x)&np.isfinite(future[h]);pn=int(pm.sum())
                    v=float(np.corrcoef(x[pm],future[h][pm])[0,1]) if pn>=5 and np.std(x[pm])>1e-14 and np.std(future[h][pm])>1e-14 else np.nan
                    phases.append(dict(candidate=c,period=period,horizon=h,phase=phase,n=pn,rho=v));values.append(v)
                vv=np.array(values);vv=vv[np.isfinite(vv)]
                rows.append(dict(candidate=c,period=period,test=f'corr{h}',effect=rho,n=n,active_rho=ra,active_n=na,phase_mean=float(vv.mean()) if len(vv) else np.nan,phase_sd=float(vv.std()) if len(vv) else np.nan,phase_min=float(vv.min()) if len(vv) else np.nan,phase_max=float(vv.max()) if len(vv) else np.nan,phase_n=len(vv)))
                ifs.append(phi)
            m=mask&(idx+1<N);m[m]=mask[(idx+1)[m]]
            lo=m&(rk<.2);hi=m&(rk>=.8)
            sl,pl,nl=sharp(future[1],lo);sh,ph,nh=sharp(future[1],hi)
            rows.append(dict(candidate=c,period=period,test='online_group_sharpe_diff',effect=sh-sl,n=nl+nh,low_sharpe=sl,high_sharpe=sh,low_n=nl,high_n=nh));ifs.append(ph-pl)
            sr,ps,ns=sharp(pr[f'{c}@0.0007'],mask);sb,pb,nb=sharp(r,mask)
            rows.append(dict(candidate=c,period=period,test='policy_sharpe_gain',effect=sr-sb,n=ns));ifs.append(ps-pb)
            # Ex-post rank groups are descriptive only, not a trading rule.
            post=pd.Series(x).where(m).rank(method='average',pct=True).to_numpy()
            for mode,rr in [('online',rk),('ex_post',post)]:
                for bucket in range(5):
                    mm=m&np.isfinite(rr)&(rr>=bucket/5)&((rr<(bucket+1)/5) if bucket<4 else (rr<=1))
                    groups.append(dict(candidate=c,period=period,mode=mode,quintile=bucket+1,**stats(future[1][mm]),mean_future20=float(np.nanmean(future[20][mm & (idx+20<N) & np.r_[mask[20:],np.zeros(20,bool)]])) if mm.any() else np.nan))
    result=pd.DataFrame(rows);phi=np.column_stack(ifs);theta=result.effect.to_numpy()
    assert len(result)==516 and np.max(abs(phi.mean(0)))<1e-10
    np.savez_compressed(OUT/'influence_functions.npz',phi=phi,theta=theta)
    for L in BLOCKS:
        se,p,pf,mx=bootstrap(phi,theta,L);result[f'se_{L}']=se;result[f'p_raw_{L}']=p;result[f'p_fwer_{L}']=pf
        np.save(OUT/f'bootstrap_maxT_{L}.npy',mx);result.to_csv(OUT/'inference.csv',index=False)
        print('BOOTSTRAP',L,'B',B,'M',len(result),'significant',int((pf<.05).sum()),'minimum',float(pf.min()),flush=True)
    result['p_fwer_required_max']=result[[f'p_fwer_{L}' for L in [10,20,40]]].max(axis=1)
    result['p_fwer_all_max']=result[[f'p_fwer_{L}' for L in BLOCKS]].max(axis=1)
    result.to_csv(OUT/'inference.csv',index=False);pd.DataFrame(groups).to_csv(OUT/'quintiles.csv',index=False);pd.DataFrame(phases).to_csv(OUT/'phases.csv',index=False)
    pd.DataFrame({'date':dates,**{f'future{h}':y for h,y in future.items()}}).to_csv(OUT/'future_returns.csv',index=False)
    (OUT/'inference_config.json').write_text(json.dumps(dict(B=B,blocks=BLOCKS,seed_base=SEED,hypotheses=len(result),max_influence_mean=float(np.max(abs(phi.mean(0)))),method='single-step centered circular-block delta influence maxT, two-sided'),indent=2))
if __name__=='__main__':main()
