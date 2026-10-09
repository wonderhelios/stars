"""Point-in-time SEC features. No financial observation precedes its filing."""
import json, hashlib
from collections import defaultdict
import numpy as np
import pandas as pd
from screen import ROOT, DATES, DAILY, load, signals, Signal, simulate, stat

OUT=ROOT/'quality'
TAGS=['RevenueFromContractWithCustomerExcludingAssessedTax','Revenues','SalesRevenueNet']

def records(doc, stop=None):
    facts=doc['facts'].get('us-gaap',{})
    grouped=defaultdict(list)
    for tag in TAGS+['OperatingIncomeLoss']:
        for row in facts.get(tag,{}).get('units',{}).get('USD',[]):
            if row.get('form') not in ('10-Q','10-K') or 'start' not in row:continue
            start,end,filed=map(pd.Timestamp,[row['start'],row['end'],row['filed']])
            if stop is not None and filed>stop:continue
            length=(end-start).days
            kind='quarter' if 70<=length<=110 else ('annual' if 330<=length<=400 else None)
            if kind is None or end>filed:continue
            grouped[filed].append(dict(tag=tag,start=start,end=end,filed=filed,kind=kind,val=float(row['val']),accn=row['accn']))
    return grouped

def financial_events(doc, stop=None):
    grouped=records(doc,stop)
    book={};prior={};events=[];last_new_end=None;last_new_avail=None
    for filed, rows in sorted(grouped.items()):
        if filed>pd.Timestamp('2026-10-07'):continue
        for row in sorted(rows,key=lambda x:x['accn']):
            book[row['tag'],row['start'],row['end']]=row
        available=sorted({(v['start'],v['end'],v['kind']) for v in book.values() if v['tag'] in TAGS and v['val']>0},key=lambda x:(x[1],x[0]))
        if not available:continue
        # Most recent period; if both quarter and annual exist at that end, use quarter.
        start,end,kind=available[-1]
        if (filed-end).days>210:continue
        def get_value(tags,st,en):
            return next((book[tag,st,en] for tag in tags if (tag,st,en) in book),None)
        rev=get_value(TAGS,start,end)
        op=get_value(['OperatingIncomeLoss'],start,end)
        comps=[x for x in available if x[2]==kind and 350<=(end-x[1]).days<=380 and abs((end-start).days-(x[1]-x[0]).days)<=14]
        if not comps or op is None:continue
        st0,en0,_=min(comps,key=lambda x:abs((end-x[1]).days-365))
        prevrev=get_value(TAGS,st0,en0)
        growth=rev['val']/prevrev['val']-1
        margin=op['val']/rev['val']
        prev_periods=[x for x in prior.get(kind,[]) if x['end']<end]
        prev=max(prev_periods,key=lambda x:x['end']) if prev_periods else None
        acceleration=growth-prev['growth'] if prev else np.nan
        i=DATES.searchsorted(filed,side='right')+1
        if i>=len(DATES):continue
        new=end!=last_new_end
        if new:last_new_avail=i;last_new_end=end
        ev=dict(filed=str(filed.date()),available=str(DATES[i].date()),available_idx=int(i),start=str(start.date()),end=str(end.date()),kind=kind,revenue=rev['val'],previous_revenue=prevrev['val'],operating_income=op['val'],growth=growth,margin=margin,acceleration=acceleration,tag=rev['tag'],accn=rev['accn'],comparison_accn=prevrev['accn'],op_accn=op['accn'],comparison_filed=str(prevrev['filed'].date()),value_filed=str(rev['filed'].date()),op_filed=str(op['filed'].date()),previous_growth=prev['growth'] if prev else None,previous_growth_filed=prev['filed'] if prev else None,new_period=new,new_period_idx=int(last_new_avail),period_id=str(end.date()))
        ev.update(comparison_tag=prevrev['tag'],comparison_start=str(st0.date()),comparison_end=str(en0.date()))
        events.append(ev)
        entry=dict(end=end,growth=growth,filed=str(filed.date()))
        old=prior.setdefault(kind,[])
        old[:]=[p for p in old if p['end']!=end]
        old.append(entry)
    return events

def features(events):
    f=pd.DataFrame(index=DATES,columns=['growth','margin','acceleration','age','period_id','period_end'],dtype=object)
    # Propagate a complete observation, preserving unknown fields as unknown.
    # Per-column ffill would incorrectly carry a quarterly acceleration into an
    # annual observation whose annual comparison is unavailable.
    known={}
    for e in events:known[e['available_idx']]=e
    last=None
    for i in range(len(DATES)):
        if i in known:last=known[i]
        if last is not None:
            if (DATES[i]-pd.Timestamp(last['end'])).days<=210:
                f.iloc[i]=[last['growth'],last['margin'],last['acceleration'],i-last['new_period_idx'],last['period_id'],last['end']]
    for k in ['growth','margin','acceleration','age']:f[k]=pd.to_numeric(f[k],errors='coerce')
    return f

def build(data, docs, cutoff=None):
    base=signals(data)['stock_trend_dip']
    result={k:{} for k in ['quality_dip','quality_accelerating_dip','quality_new_filing']}
    evs=[];coverage=[]
    for s,doc in sorted(docs.items()):
        if s not in data:continue
        es=financial_events(doc,cutoff);f=features(es)
        for e in es:e['symbol']=s
        evs+=es
        valid=f.growth.notna() & f.margin.notna()
        quality=(f.growth>.1)&(f.margin>0)
        accelerated=quality&(f.acceleration>0)
        result['quality_dip'][s]=base[s]&quality.to_numpy()
        result['quality_accelerating_dip'][s]=base[s]&accelerated.to_numpy()
        d=data[s]
        relative=(d.ret-data['QQQ'].ret).rolling(20).sum().shift(1)
        liquid=(d.close*d.volume).rolling(20).mean().shift(2)>=5e6
        possible=accelerated&(f.age>=0)&(f.age<5)&(relative<.05)&liquid
        event_signal=np.zeros(len(DATES),bool);seen=set()
        for i in np.flatnonzero(possible.to_numpy()):
            pid=f.iloc[i].period_id
            if pid not in seen:event_signal[i]=True;seen.add(pid)
        result['quality_new_filing'][s]=event_signal
        coverage.append(dict(symbol=s,events=len(es),first_available=es[0]['available'] if es else None,last_available=es[-1]['available'] if es else None,last_period=es[-1]['end'] if es else None,covered_sessions=int(valid.sum()),quality_sessions=int(quality.sum()),accelerating_sessions=int(accelerated.sum())))
    return result,evs,coverage

def main():
    OUT.mkdir(exist_ok=True)
    manifest=json.loads((ROOT/'fundamentals/manifest.json').read_text())
    # Do not silently run against a partially downloaded universe.
    assert len(manifest)==30, f'Fundamental fetch incomplete: {len(manifest)}/30'
    docs={x['symbol']:json.loads((ROOT/'fundamentals'/f'{x["symbol"]}.json').read_text()) for x in manifest if x.get('name_verified')}
    data,_=load();sigs,events,coverage=build(data,docs)
    pd.DataFrame(events).to_csv(OUT/'financial_events.csv',index=False)
    pd.DataFrame(coverage).to_csv(OUT/'coverage.csv',index=False)
    results={};rr={};trades=[]
    for name,s in sigs.items():
        signal=Signal(s);signal.name=name
        for hold in [1,2]:
            for phase in range(hold):
                for fee in [.0005,.0015]:
                    key=f'{name}_h{hold}_p{phase}_fee{int(fee*10000)}'
                    r,diag,tt,ledger=simulate(data,signal,hold,phase,fee,record=fee==.0005)
                    parts={'full':r,'2015-22':r.loc[:'2022-12-31'],'2023-24':r.loc['2023-01-01':'2024-12-31'],'2025-26':r.loc['2025-01-01':]}
                    results[key]=dict(metrics={k:stat(v) for k,v in parts.items()},diagnostics=diag)
                    rr[key]=r.to_numpy()
                    for t in tt:t['strategy']=key
                    trades+=tt
                    if ledger:pd.DataFrame(ledger).to_csv(OUT/f'{key}_ledger.csv',index=False)
                    print(key,round(results[key]['metrics']['full']['sharpe'],3),round(results[key]['metrics']['2025-26']['annual_return'],4),diag['trades'],flush=True)
    pd.DataFrame(trades).to_csv(OUT/'trades.csv',index=False)
    np.savez_compressed(OUT/'returns.npz',dates=DAILY.to_numpy(dtype='datetime64[D]'),**rr)
    (OUT/'results.json').write_text(json.dumps(results,indent=2))
    # Build a combined universe without changing previous batch outputs.
    old=np.load(ROOT/'returns.npz')
    np.savez_compressed(OUT/'combined_returns.npz',**{k:old[k] for k in old.files},**rr)
    prev=json.loads((ROOT/'results.json').read_text())
    (OUT/'combined_results.json').write_text(json.dumps({**prev,**results},indent=2))
    checks={}
    for cutoff in [pd.Timestamp('2018-12-31'),pd.Timestamp('2024-12-31')]:
        short,_,_=build(data,docs,cutoff)
        # Future filings cannot affect returns on or before their date.
        m=DATES<=cutoff
        checks[str(cutoff.date())]=all(np.array_equal(a[m],short[k][s][m]) for k,b in sigs.items() for s,a in b.items())
        assert checks[str(cutoff.date())]
    (OUT/'prefix_checks.json').write_text(json.dumps(checks,indent=2))

if __name__=='__main__':main()
