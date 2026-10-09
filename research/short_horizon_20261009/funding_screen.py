"""Frozen four-hypothesis cross-sectional funding screen; no trading APIs."""
from pathlib import Path
from collections import Counter
import json
import numpy as np
import pandas as pd
from fetch_funding_panel import OUT,COINS,START,END,save

DAY=86400000
HOUR=3600000
EVAL=int(pd.Timestamp('2023-06-01',tz='UTC').timestamp()*1000)


def inputs():
    times=np.arange(START,END+1,DAY,dtype=np.int64)
    n,m=len(times),len(COINS)
    d={k:np.full((n,m),np.nan) for k in ['o','h','l','c','quote']}
    funding=[]
    for j,coin in enumerate(COINS):
        rows=json.loads((OUT/(coin+'_daily.json')).read_text())
        for r in rows:
            i=(int(r[0])-START)//DAY
            for k,col in [('o',1),('h',2),('l',3),('c',4),('quote',7)]:d[k][i,j]=float(r[col])
        # Final mark is the last complete day's closing price, not a future open.
        d['o'][-1,j]=d['c'][-2,j]
        funding.append(json.loads((OUT/(coin+'_funding_with_marks.json')).read_text()))
    d.update(times=times,names=np.array(COINS))
    return d,funding


def features(d,funding):
    n,m=d['o'].shape
    ret=pd.DataFrame(d['c']).pct_change(fill_method=None)
    momentum=(pd.DataFrame(d['c'])/pd.DataFrame(d['c']).shift(14)-1).shift().values
    vol=ret.rolling(20,min_periods=20).std().shift().values
    liquid=pd.DataFrame(d['quote']).rolling(30,min_periods=30).mean().shift().values>=5e6
    enough=pd.DataFrame(d['c']).rolling(60,min_periods=60).count().shift().values==60
    scores=np.full((n,m),np.nan)
    for j,rows in enumerate(funding):
        ts=np.array([r['fundingTime'] for r in rows],dtype=np.int64)
        rates=np.array([float(r['fundingRate']) for r in rows])
        for i,t in enumerate(d['times']):
            cutoff=t-HOUR
            lo,hi=np.searchsorted(ts,[cutoff-7*DAY,cutoff],side='right')
            window=ts[lo:hi]
            if len(window)<2:continue
            if window[0]-(cutoff-7*DAY)>8*HOUR+60000 or cutoff-window[-1]>8*HOUR+60000:continue
            if np.max(np.diff(window))>8*HOUR+60000:continue
            scores[i,j]=rates[lo:hi].sum()/7
    eligible=liquid&enough&np.isfinite(momentum)&np.isfinite(vol)&np.isfinite(scores)&np.isfinite(d['o'])
    targets={k:np.zeros((n,m)) for k in ['raw','residual']}
    for i in range(n-1):
        ids=np.flatnonzero(eligible[i])
        if len(ids)<12:continue
        x=np.column_stack([momentum[i,ids],vol[i,ids]])
        x=(x-x.mean(axis=0))/np.maximum(x.std(axis=0),1e-12)
        x=np.column_stack([np.ones(len(ids)),x])
        residual=scores[i,ids]-x@np.linalg.lstsq(x,scores[i,ids],rcond=None)[0]
        for name,score in [('raw',scores[i,ids]),('residual',residual)]:
            order=ids[np.lexsort((d['names'][ids],score))]
            targets[name][i,order[:3]]=1/6
            targets[name][i,order[-3:]]=-1/6
    return targets,scores,eligible


def fund_intervals(d,funding):
    shape=d['o'].shape
    payment=np.zeros(shape);proxy_abs=np.zeros(shape);positive=np.zeros(shape);negative=np.zeros(shape)
    counts=np.zeros(shape,dtype=np.int64)
    for j,rows in enumerate(funding):
        for r in rows:
            t=int(r['fundingTime'])//HOUR*HOUR
            # Payments at midnight belong to positions before that rebalance.
            i=int(np.ceil((t-START)/DAY))
            if i<=0 or i>=shape[0]:continue
            money=float(r['fundingRate'])*r['payment_mark']
            payment[i,j]+=money;positive[i,j]+=max(money,0);negative[i,j]+=min(money,0)
            if r['mark_source']=='official_mark_bar_open_proxy':proxy_abs[i,j]+=abs(money)
            counts[i,j]+=1
    return payment,proxy_abs,positive,negative,counts


def stats(r):
    r=np.asarray(r);nav=np.r_[1,np.cumprod(1+r)];sd=r.std(ddof=1)
    return dict(sharpe=float(r.mean()/sd*np.sqrt(365)) if sd>1e-14 else None,annual_arithmetic=float(r.mean()*365),cagr=float(nav[-1]**(365/len(r))-1),max_drawdown=float((nav/np.maximum.accumulate(nav)-1).min()))


def simulate(d,target,scores,funds,hold,phase,fee=.0008,lev=2.7,proxy_error=0):
    payment,proxy_abs,positive,negative,counts=funds
    ids=np.flatnonzero(d['times']>=EVAL)
    q=np.zeros(len(COINS));equity=100000.;exit_i=None;last_exit=None
    returns=[];daily=[];events=[];risk=[];positions=[]
    total_funding=0.;total_price=0.;total_fees=0.
    for step,i in enumerate(ids):
        before=equity;gain=0.;fund=0.;cost=0.
        if np.any(q):
            assert step>0 and np.isfinite(d['o'][i,q!=0]).all()
            gain=float(q@(d['o'][i]-d['o'][i-1]))
            # Dot products must not allow 0 * NaN in not-yet-listed assets.
            if not np.isfinite(gain):gain=float(np.sum(q[q!=0]*(d['o'][i,q!=0]-d['o'][i-1,q!=0])))
            fund=float(q@payment[i]+proxy_error*np.abs(q)@proxy_abs[i])
            adverse=np.where(q>=0,d['l'][i-1],d['h'][i-1])
            active=q!=0
            loss_price=float(np.sum(q[active]*(adverse[active]-d['o'][i-1,active])))
            fund_worst=float(np.maximum(q,0)@positive[i]+np.minimum(q,0)@negative[i]+proxy_error*np.abs(q)@proxy_abs[i])
            gross_upper=float(np.sum(np.abs(q[active])*d['h'][i-1,active]))
            envelope=before+loss_price-fund_worst
            if envelope<.1*gross_upper:
                risk.append(dict(time=int(d['times'][i]),equity_lower=envelope,maintenance_upper=.1*gross_upper))
            equity+=gain-fund
            assert equity>0, 'Bankruptcy: reject this model path'
        total_funding+=fund;total_price+=gain
        if exit_i==i:
            active=q!=0
            closefee=float(np.sum(np.abs(q[active])*d['o'][i,active]))*fee
            equity-=closefee;cost+=closefee
            events[-1].update(exit_time=int(d['times'][i]),exit_prices=d['o'][i,active].tolist(),equity_after=equity,exit_fee=closefee)
            q[:]=0;exit_i=None;last_exit=i
        forecast=0.
        if i<len(d['times'])-1 and not np.any(q) and (d['times'][i]//DAY)%(hold+1)==phase and i+hold<len(d['times']):
            w=target[i]
            forecast=float(-w@np.nan_to_num(scores[i])*hold)
            if np.abs(w).sum()>.99 and forecast>2*.0008:
                assert last_exit is None or i-last_exit>=1
                active=w!=0
                after=equity/(1+fee*lev)
                q[active]=w[active]*lev*after/d['o'][i,active]
                entryfee=equity-after;equity=after;cost+=entryfee;exit_i=i+hold
                events.append(dict(entry_time=int(d['times'][i]),expected_exit_time=int(d['times'][exit_i]),names=d['names'][active].tolist(),quantities=q[active].tolist(),entry_prices=d['o'][i,active].tolist(),equity_after_entry=equity,entry_fee=entryfee,forecast_per_gross=forecast))
        total_fees+=cost
        returns.append(equity/before-1)
        positions.append(q.copy())
        daily.append(dict(time=int(d['times'][i]),equity_before=before,price_pnl=gain,funding_paid=fund,fees=cost,equity_after=equity,gross=float(np.sum(np.abs(q[q!=0])*d['o'][i,q!=0]))))
    assert not np.any(q) and all('exit_time' in e for e in events)
    summary=dict(status='margin_envelope_failed' if risk else 'research_proxy',trades=len(events),funding_income=-total_funding,price_pnl=total_price,fees=total_fees,final_equity=equity,risk_flags=risk,metrics={})
    dates=pd.to_datetime(d['times'][ids],unit='ms')
    for part,mask in [('full',np.ones(len(ids),bool)),('2023-24',dates.year<=2024),('2025-26',dates.year>=2025)]:summary['metrics'][part]=stats(np.array(returns)[mask])
    assert abs(equity-(100000+total_price-total_funding-total_fees))<1e-6
    return np.array(returns),np.array(positions),daily,events,summary


def main():
    d,funding=inputs();target,scores,eligible=features(d,funding);funds=fund_intervals(d,funding)
    evalmask=d['times']>=EVAL
    arrays={'dates':pd.to_datetime(d['times'][evalmask],unit='ms').to_numpy()};positions={};results={};events={}
    for name in target:
        for hold in [1,2]:
            for phase in range(hold+1):
                for label,fee,lev,error in [('base',.0008,2.7,0),('stress',.0015,2.7,0),('markstress',.0008,2.7,.01),('unit',.0008,1,0)]:
                    key=f'{name}_h{hold}_p{phase}_{label}'
                    r,q,daily,ev,summary=simulate(d,target[name],scores,funds,hold,phase,fee,lev,error)
                    arrays[key]=r;positions[key]=q;results[key]=summary;events[key]=ev
                    pd.DataFrame(daily).to_csv(OUT/(key+'_ledger.csv'),index=False)
    np.savez_compressed(OUT/'returns.npz',**arrays)
    np.savez_compressed(OUT/'positions.npz',**positions)
    np.savez_compressed(OUT/'inputs.npz',**d,scores=scores,eligible=eligible,target_raw=target['raw'],target_residual=target['residual'])
    save(OUT/'results.json',results);save(OUT/'events.json',events)
    for name,result in results.items():
        if name.endswith('_base'):print(name,result,flush=True)


if __name__=='__main__':main()
