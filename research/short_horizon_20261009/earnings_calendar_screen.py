"""Three preregistered windows from an evolving, ex-ante issuer calendar."""
import hashlib
import json
from collections import Counter
import numpy as np
import pandas as pd
from screen import ROOT,DATES,SCHED,Signal,load,simulate,stat
from fetch import STOCKS

OUT=ROOT/'earnings_calendar'
RULES={'pre1':(0,1),'pre2':(-1,2),'event2':(0,2)}


def make_signals(data,notices,cutoff=None):
    n=len(next(iter(data.values())))
    syms=sorted(set(r['symbol'] for r in notices) & set(data))
    liquidity={s:((data[s].close*data[s].volume).rolling(20,min_periods=20).mean().shift(1)>=5e6).to_numpy() for s in syms}
    signals={rule:Signal({s:np.zeros(n,dtype=bool) for s in syms}) for rule in RULES}
    for rule,s in signals.items():s.name=rule
    known=sorted([r for r in notices if cutoff is None or pd.Timestamp(r['available_after'])<=cutoff],key=lambda r:pd.Timestamp(r['available_after']))
    cursor=0;plans={};used={r:set() for r in RULES};trace=[]
    for i in range(n):
        op=SCHED['open'].iloc[i]
        while cursor<len(known) and pd.Timestamp(known[cursor]['available_after'])<=op:
            row=known[cursor];cursor+=1
            plans[(row['symbol'],row['fiscal_year'],row['quarter'])]=row
        if DATES[i]<pd.Timestamp('2021-01-01'):continue
        for identity,row in plans.items():
            sym=row['symbol'];day=pd.Timestamp(row['planned_date'])
            if sym not in liquidity or day not in DATES:continue
            last=DATES.get_loc(day)-(row['phase']=='before_open')
            for rule,(offset,hold) in RULES.items():
                if i!=last+offset or identity in used[rule]:continue
                used[rule].add(identity)
                exitidx=i+hold-1
                hours=(SCHED['close'].iloc[exitidx]-op).total_seconds()/3600 if exitidx<len(SCHED) else None
                eligible=liquidity[sym][i] and hours is not None and hours<=48
                trace.append(dict(rule=rule,symbol=sym,fiscal_year=row['fiscal_year'],quarter=row['quarter'],
                    source=row['source'],available_after=row['available_after'],planned_date=row['planned_date'],phase=row['phase'],
                    intended_entry=str(op),intended_exit=str(SCHED['close'].iloc[exitidx]) if hours is not None else None,
                    hours=hours,signal=bool(eligible),reason='eligible' if eligible else 'illiquid_or_holding_over_48h_or_end_of_window'))
                if eligible:signals[rule][sym][i]=True
    return signals,trace


def main():
    notices=json.loads((OUT/'parsed_notices.json').read_text())
    assert len(notices)>0
    # Source review is a separate read-only stage and must precede market results.
    source_audit=json.loads((OUT/'source_audit.json').read_text())
    assert source_audit['verified_notices']==len(notices) and source_audit['ready_for_restricted_exploration']
    data,price_issues=load();sigs,trace=make_signals(data,notices)
    (OUT/'signal_trace.json').write_text(json.dumps(trace,indent=2))
    results={};returns={};trades=[]
    for rule,sig in sigs.items():
        hold=RULES[rule][1]
        for fee in [.0005,.0015]:
            key=f'{rule}_fee{int(fee*10000)}'
            r,diag,ts,ledger=simulate(data,sig,hold,fee=fee,record=True,phase_filter=False)
            r=r.loc['2021-01-01':]
            parts={'full':r,'2021-22':r.loc[:'2022-12-31'],'2023-24':r.loc['2023-01-01':'2024-12-31'],'2025-26':r.loc['2025-01-01':]}
            metrics={k:stat(v) for k,v in parts.items()}
            for m in metrics.values():
                if m['vol']==0:m['sharpe']=None
            results[key]=dict(metrics=metrics,diagnostics=diag)
            returns[key]=r.to_numpy()
            for t in ts:t['strategy']=key
            trades.extend(ts)
            frame=pd.DataFrame(ledger);frame[frame.date>='2021-01-01'].to_csv(OUT/f'{key}_ledger.csv',index=False)
            print(key,metrics['full']['annual_return'],metrics['full']['sharpe'],diag['trades'],flush=True)
    pd.DataFrame(trades).to_csv(OUT/'trades.csv',index=False)
    np.savez_compressed(OUT/'returns.npz',dates=r.index.to_numpy(dtype='datetime64[D]'),**returns)
    (OUT/'results.json').write_text(json.dumps(results,indent=2,allow_nan=False))
    prefix={}
    for cutdate in ['2023-12-31','2025-06-30']:
        n=DATES.searchsorted(pd.Timestamp(cutdate),side='right');cutoff=SCHED['close'].iloc[n-1]
        sub={s:d.iloc[:n].copy() for s,d in data.items()}
        partial,_=make_signals(sub,notices,cutoff)
        errors={}
        for rule in RULES:
            for s in sigs[rule]:
                candidate=partial[rule].get(s,np.zeros(n,dtype=bool))
                assert np.array_equal(candidate,sigs[rule][s][:n]),(cutdate,rule,s)
            rr,*_=simulate(data,partial[rule],RULES[rule][1],until=n,phase_filter=False)
            rr=rr.loc['2021-01-01':]
            err=float(np.max(np.abs(rr.to_numpy()-returns[rule+'_fee5'][:len(rr)])))
            assert err<1e-12,(cutdate,rule,err)
            errors[rule]=err
        prefix[cutdate]=errors
    (OUT/'prefix_checks.json').write_text(json.dumps(prefix,indent=2))
    coverage=[dict(symbol=s,parsed_notices=sum(r['symbol']==s for r in notices),
        price_available=s in data,status='restricted_notice_coverage' if any(r['symbol']==s for r in notices) else 'no_verified_notices_not_zero_events') for s in STOCKS]
    (OUT/'coverage.json').write_text(json.dumps(coverage,indent=2))
    (OUT/'manifest.json').write_text(json.dumps(dict(start='2021-01-01',end=str(r.index[-1].date()),
        rules=RULES,notice_count=len(notices),signal_counts=dict(Counter(t['rule'] for t in trace if t['signal'])),
        spec_sha256=hashlib.sha256((ROOT/'EARNINGS_CALENDAR_SPEC.md').read_bytes()).hexdigest(),
        code_sha256={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in [ROOT/'earnings_calendar_parse.py',ROOT/'earnings_calendar_screen.py',ROOT/'screen.py']},
        data_sha256={s:hashlib.sha256((ROOT/'raw'/f'{s}.json').read_bytes()).hexdigest() for s in sigs['pre1']},
        limitations=['Fixed present-day surviving universe','Incomplete original scheduling notices','Auction and bank-settlement proxies','Not a clean holdout','No synchronous crypto combination yet']),indent=2))


if __name__=='__main__':main()
