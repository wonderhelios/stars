"""Independent sleeve allocation with lagged estimation, drift, and allocation costs."""
import json
import math
import numpy as np
import pandas as pd
from carry import OUT, inputs, stats


def trend(d,fee_rate=.00075):
    qty = np.zeros(2)
    eq, rr = 100000., []
    audits = {'funding_total': 0., 'fees_total': 0.}
    for i in range(len(d['times'])):
        before = eq
        p = d['perp_o'][i,:2]
        if i:
            eq += qty@(p-d['perp_o'][i-1,:2])
            oracle = (d['spot_o'][i-1,:2]/d['fx_o'][i-1]+d['spot_o'][i,:2]/d['fx_o'][i])/2
            funding = -np.sum(qty*oracle*d['rates'][i,:2])
            eq += funding
            audits['funding_total'] += funding
        target = np.zeros(2)
        if i >= 64:
            signal = np.sign(d['perp_c'][i-1,:2]/d['perp_c'][i-64,:2]-1)
            target = .5*eq*signal/p
        delta = target-qty
        delta[np.abs(delta)<.02*np.maximum(abs(target),1e-12)]=0.
        fee = np.abs(delta)@p*fee_rate
        eq -= fee
        audits['fees_total'] += fee
        qty += delta
        if i == len(d['times'])-1:
            fee = np.abs(qty)@p*fee_rate
            eq -= fee
            audits['fees_total'] += fee
        assert eq > 0
        rr.append(eq/before-1)
    return np.array(rr), audits


def capped(w):
    w=np.maximum(w,0.)
    if w.sum()<=1e-12:
        return np.zeros(len(w))
    w/=w.sum()
    # At most 95% in one sleeve; leave any unallocated amount as unremunerated cash.
    return np.minimum(w,.95)


def allocation(history, method, rf=None):
    if method=='equal':
        return np.ones(history.shape[1])/history.shape[1]
    sd=np.maximum(history.std(axis=0,ddof=1),1e-5)
    if method=='iv':
        w=1/sd
        w/=w.sum()
        # Redistribute the >95% excess to the other sleeves; two sleeves therefore have a 5% floor.
        if w.max()>.95:
            j=w.argmax();others=np.arange(len(w))!=j
            w[others]*=.05/w[others].sum();w[j]=.95
        return w
    mu=history.mean(axis=0)
    # Non-negative active-set heuristic, no full-period expected-return fitting.
    cov=np.cov(history,rowvar=False)
    cov=.5*cov+.5*np.diag(np.diag(cov))
    allowed=mu>0
    w=np.zeros(len(mu))
    while allowed.any():
        ids=np.flatnonzero(allowed)
        sol=np.linalg.solve(cov[np.ix_(ids,ids)]+np.eye(len(ids))*1e-12,mu[ids])
        if np.all(sol>=0):
            w[ids]=sol;break
        allowed[ids[sol<0]]=False
    return capped(w)


def portfolio(R, method, phase, start, fee=.002, period=30, entry_costs=None):
    weights=np.zeros(R.shape[1]);out=[];ws=[];costs=[]
    for i in range(start,len(R)):
        # Allocate before earning today's return, from the 126 completed preceding dates only.
        cost=0.
        if i==start or (i-start+phase)%period==0:
            desired=allocation(R[i-126:i],method)
            cost=np.abs(desired-weights).sum()*fee
            if i==start and entry_costs is not None:
                cost=float(desired@entry_costs)
            weights=desired
        gain=weights@R[i]
        out.append((1-cost)*(1+gain)-1)
        ws.append(weights.copy());costs.append(cost)
        weights=weights*(1+R[i])/(1+gain)
    return np.array(out), np.array(ws), np.array(costs)


def sharpe(r):
    return float(r.mean()/r.std(ddof=1)*np.sqrt(365))


def influence(r):
    mu,sd=r.mean(),r.std(ddof=1)
    return np.sqrt(365)*((r-mu)/sd-mu*((r-mu)**2-sd**2)/(2*sd**3))


def uncertainty(runs,rf,mask,B=4999):
    keys=list(runs)
    result={}
    for rf_label,cash in [('zero',np.zeros_like(rf)),('sofr',rf)]:
        D=np.array([np.mean([influence(r[mask]-cash[mask]) for r in runs[z]],axis=0) for z in keys]).T
        obs=np.array([np.mean([sharpe(r[mask]-cash[mask]) for r in runs[z]]) for z in keys])
        D-=D.mean(axis=0)
        N=len(D);rng=np.random.default_rng(20261009)
        result[rf_label]={}
        for L in [15,30,60]:
            draws=[]
            for b in range(0,B,200):
                nn=min(200,B-b)
                st=rng.integers(N,size=(nn,math.ceil(N/L)))
                ix=((st[:,:,None]+np.arange(L))%N).reshape(nn,-1)[:,:N]
                count=np.array([np.bincount(v,minlength=N) for v in ix])
                draws.append(count@D/N)
            draws=np.concatenate(draws)
            se=np.maximum(draws.std(axis=0,ddof=1),1e-12)
            critical=np.quantile(np.max(draws/se,axis=1),.95)
            bidx=keys.index('core')
            delta=obs-obs[bidx]
            paired=draws-draws[:,bidx,None]
            se_d=np.maximum(paired.std(axis=0,ddof=1),1e-12)
            max_d=np.max(paired/se_d,axis=1)
            result[rf_label][str(L)]={z:dict(sharpe=float(obs[j]),ci95=(obs[j]+np.quantile(draws[:,j],[.025,.975])).tolist(),
                         simultaneous_lower95=float(obs[j]-critical*se[j]),
                         delta_core=float(delta[j]),p_fwer_improvement=float((1+np.sum(max_d>=delta[j]/se_d[j]))/(B+1))) for j,z in enumerate(keys)}
    return result


def main():
    d=inputs();cash=dict(np.load(OUT/'carry_returns.npz'));dated=dict(np.load(OUT/'dated_returns.npz'))
    tr,tr_audit=trend(d)
    # Local artifact generated by our previous research script; its dates are stored as object strings.
    core=dict(np.load(OUT.parent/'portfolio_20261009/returns.npz',allow_pickle=True))
    coretimes=pd.to_datetime(core['dates'],utc=True).astype('int64')//1000000
    common=np.array(sorted(set(d['times'])&set(coretimes)))
    ix=np.searchsorted(d['times'],common);ci=np.searchsorted(coretimes,common)
    # Common formation and evaluation dates for all comparisons.
    start=126
    dates=pd.to_datetime(common[start:],unit='ms',utc=True)
    legs={'carry':cash['carry_gated'][ix],'dated':dated['dated_equal'][ix],'trend':tr[ix]}
    core_r=core['baseline'][:,ci]
    f=pd.read_csv(OUT/'raw/sofr.csv',parse_dates=['observation_date']).set_index('observation_date')['SOFR']
    # Prior calendar-date SOFR observation, weekend forward fill, used ONLY for ex-post cash comparison.
    # This does not reconstruct the NY publication timestamp and never enters signal/allocation decisions.
    f.index=f.index.tz_localize('UTC')
    all_dates=pd.to_datetime(common,unit='ms',utc=True)
    rf=f.reindex(pd.date_range(f.index.min(),all_dates.max(),freq='D')).ffill().shift(1).reindex(all_dates).values/100/360
    assert np.isfinite(rf).all()
    rf=rf[start:]
    runs={'core':core_r[:,start:], 'carry_gated':legs['carry'][None,start:],
          'carry_always':cash['carry_always'][ix][None,start:],
          'dated':legs['dated'][None,start:], 'trend':tr[ix][None,start:]}
    configs={
        'core_carry_equal':(['core','carry'],'equal',1),
        'core_carry_iv':(['core','carry'],'iv',30),
        'core_carry_trend_iv':(['core','carry','trend'],'iv',30),
        'core_carry_trend_mv':(['core','carry','trend'],'mv',30),
        'new_three_iv':(['carry','dated','trend'],'iv',30),
        'new_three_mv':(['carry','dated','trend'],'mv',30),
        'core_carry_dated_iv':(['core','carry','dated'],'iv',30),
        'carry_dated_equal':(['carry','dated'],'equal',1),
    }
    allocations={}
    for label,(names,method,period) in configs.items():
        ret,weights,costs=[],[],[]
        for cp in range(3 if 'core' in names else 1):
            R=np.array([core_r[cp] if z=='core' else legs[z] for z in names]).T
            ec=np.array([{'core':.002025,'carry':.0016225,'dated':.00345,'trend':.00075}[z] for z in names])
            for phase in range(period):
                r,w,c=portfolio(R,method,phase,start,period=period,entry_costs=ec)
                ret.append(r);weights.append(w);costs.append(c)
        runs[label]=np.array(ret)
        allocations[label]=dict(legs=names,mean_weights=dict(zip(names,np.mean(weights,axis=(0,1)).tolist())),
                                mean_cash=float(1-np.mean(np.sum(weights,axis=2))),
                                annual_allocation_cost=float(np.mean(costs)*365))
    masks={'full':np.ones(len(dates),bool),'2023-24':dates.year<=2024,'2025-26':dates.year>=2025}
    summary={}
    for z,rr in runs.items():
        summary[z]={}
        for part,m in masks.items():
            ss=[stats(r[m]) for r in rr]
            q={k:float(np.mean([s[k] for s in ss])) for k in ss[0]}
            q.update(sharpe_sofr=float(np.mean([sharpe(r[m]-rf[m]) for r in rr])),
                     cash_benchmark_annual=float(rf[m].mean()*365),
                     phase_range=[min(s['sharpe'] for s in ss),max(s['sharpe'] for s in ss)],
                     phase_sd=float(np.std([s['sharpe'] for s in ss])),
                     worst_drawdown=float(min(s['max_drawdown'] for s in ss)))
            summary[z][part]=q
        print(z,{p:(round(q['sharpe'],3),round(q['sharpe_sofr'],3),round(q['annual_return']*100,2)) for p,q in summary[z].items()},flush=True)
    unc={p:uncertainty(runs,rf,m) for p,m in masks.items()}
    corr=pd.DataFrame({'core':core_r[:,start:].mean(axis=0),**{z:r[start:] for z,r in legs.items()}},index=dates).corr()
    # Forward-prefix allocation invariance: future returns never change earlier allocations.
    test_R=np.column_stack([legs[z] for z in ['carry','dated','trend']])
    checks={}
    for method in ['iv','mv']:
        r,w,_=portfolio(test_R,method,0,start)
        p,pw,_=portfolio(test_R[:700],method,0,start)
        err=float(np.max(np.abs(w[:len(pw)]-pw)))
        assert err==0
        checks[method+'_prefix_error']=err
    np.savez_compressed(OUT/'portfolio_returns.npz',times=common[start:],rf=rf,**runs)
    result=dict(meta=dict(start=str(dates[0]),end=str(dates[-1]),days=len(dates),formation_days=126,
                          tests=len(runs)-1,bootstrap_replicates=4999,core_funding_incomplete=True),
                summary=summary,allocations=allocations,uncertainty=unc,correlations=corr.to_dict(),checks=checks,trend_audit=tr_audit)
    (OUT/'portfolio_results.json').write_text(json.dumps(result,indent=2))
    pd.DataFrame([dict(strategy=z,period=p,**q) for z,v in summary.items() for p,q in v.items()]).to_csv(OUT/'portfolio_metrics.csv',index=False)
    corr.to_csv(OUT/'correlations.csv')


if __name__=='__main__':
    main()
