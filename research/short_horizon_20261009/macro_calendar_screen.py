"""Frozen macro calendars, three asset sleeves and their fixed equal-weight mix."""
import hashlib
import json
import numpy as np
import pandas as pd
from screen import ROOT,DATES,SCHED,Signal,load,simulate,stat

OUT=ROOT/'macro_calendar'
CONFIGS={f'{asset}_{window}':dict(assets=['SPY','GLD','TLT'] if asset=='mix' else [asset],hold=hold)
    for asset in ['SPY','GLD','TLT','mix'] for window,hold in [('pre1',1),('event2',2)]}


def make_signals(data,events,cutoff=None):
    n=len(next(iter(data.values())))
    liq={s:((data[s].close*data[s].volume).rolling(20,min_periods=20).mean().shift(1)>=5e6).to_numpy() for s in ['SPY','GLD','TLT']}
    known=sorted([r for r in events if cutoff is None or pd.Timestamp(r['available_after'])<=cutoff],key=lambda r:pd.Timestamp(r['available_after']))
    sigs={name:Signal({s:np.zeros(n,dtype=bool) for s in c['assets']}) for name,c in CONFIGS.items()}
    for name,sig in sigs.items():sig.name=name
    cursor=0;plans={};trace=[]
    for i in range(n):
        op=SCHED['open'].iloc[i]
        while cursor<len(known) and pd.Timestamp(known[cursor]['available_after'])<=op:
            r=known[cursor];cursor+=1;plans[r['planned_date']]=r
        if DATES[i]<pd.Timestamp('2021-01-01'):continue
        for planned,r in plans.items():
            day=pd.Timestamp(planned)
            if day not in DATES or i!=DATES.get_loc(day)-1:continue
            for name,c in CONFIGS.items():
                end=i+c['hold']-1
                hours=(SCHED['close'].iloc[end]-op).total_seconds()/3600
                eligible=hours<=48 and all(liq[s][i] for s in c['assets'])
                trace.append(dict(configuration=name,source=r['source'],available_after=r['available_after'],planned_date=planned,
                    entry=str(op),exit=str(SCHED['close'].iloc[end]),hours=hours,eligible=bool(eligible)))
                if eligible:
                    for s in c['assets']:sigs[name][s][i]=True
    return sigs,trace


def main():
    events=json.loads((OUT/'planned_events.json').read_text());source=json.loads((OUT/'source_audit.json').read_text())
    assert source['verified_events']==len(events) and source['ready_for_exploration']
    data,_=load();sigs,trace=make_signals(data,events);(OUT/'signal_trace.json').write_text(json.dumps(trace,indent=2))
    results={};returns={};trades=[]
    for name,c in CONFIGS.items():
        for gross in [1.,2.7]:
            for fee in [.0005,.0015]:
                key=f'{name}_g{gross:g}_fee{int(fee*10000)}'
                r,diag,ts,ledger=simulate(data,sigs[name],c['hold'],gross=gross,fee=fee,record=True,phase_filter=False,per_name_cap=gross)
                r=r.loc['2021-01-01':]
                parts={'full':r,'2021-22':r.loc[:'2022-12-31'],'2023-24':r.loc['2023-01-01':'2024-12-31'],'2025-26':r.loc['2025-01-01':]}
                metrics={p:stat(v) for p,v in parts.items()}
                for m in metrics.values():
                    if m['vol']==0:m['sharpe']=None
                diag['mean_open_gross_in_research_window']=diag['mean_open_gross']*sum(DATES>=pd.Timestamp('2015-01-01'))/sum(DATES>=pd.Timestamp('2021-01-01'))
                results[key]=dict(configuration=name,gross=gross,fee=fee,metrics=metrics,diagnostics=diag)
                returns[key]=r.to_numpy()
                for t in ts:t['strategy']=key
                trades.extend(ts)
                df=pd.DataFrame(ledger);df[df.date>='2021-01-01'].to_csv(OUT/f'{key}_ledger.csv',index=False)
                print(key,metrics['full']['annual_return'],metrics['full']['sharpe'],diag['trades'],flush=True)
    pd.DataFrame(trades).to_csv(OUT/'trades.csv',index=False)
    np.savez_compressed(OUT/'returns.npz',dates=r.index.to_numpy(dtype='datetime64[D]'),**returns)
    (OUT/'results.json').write_text(json.dumps(results,indent=2,allow_nan=False))
    prefix={}
    for day in ['2023-12-31','2025-06-30']:
        n=DATES.searchsorted(pd.Timestamp(day),side='right');cut=SCHED['close'].iloc[n-1]
        partial,_=make_signals({s:d.iloc[:n].copy() for s,d in data.items()},events,cut)
        errors={}
        for name,c in CONFIGS.items():
            assert all(np.array_equal(partial[name][s],sigs[name][s][:n]) for s in c['assets'])
            for gross in [1.,2.7]:
                for fee in [.0005,.0015]:
                    key=f'{name}_g{gross:g}_fee{int(fee*10000)}'
                    rr,*_=simulate(data,partial[name],c['hold'],gross=gross,fee=fee,until=n,phase_filter=False,per_name_cap=gross)
                    rr=rr.loc['2021-01-01':]
                    err=float(np.max(np.abs(rr.to_numpy()-returns[key][:len(rr)])))
                    assert err<1e-12;errors[key]=err
        prefix[day]=errors
    (OUT/'prefix_checks.json').write_text(json.dumps(prefix,indent=2))
    (OUT/'manifest.json').write_text(json.dumps(dict(start='2021-01-01',end=str(r.index[-1].date()),events=len(events),configurations=CONFIGS,
        spec_sha256=hashlib.sha256((ROOT/'MACRO_CALENDAR_SPEC.md').read_bytes()).hexdigest(),
        code_sha256={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in [ROOT/'macro_calendar_parse.py',ROOT/'macro_calendar_screen.py',ROOT/'screen.py']},
        data_sha256={s:hashlib.sha256((ROOT/'raw'/f'{s}.json').read_bytes()).hexdigest() for s in ['SPY','GLD','TLT']}),indent=2))


if __name__=='__main__':main()
