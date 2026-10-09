"""Source evidence and independent cash/settlement replay for calendar trades."""
import hashlib
import json
import re
import sys
from collections import Counter
from pathlib import Path
import numpy as np
import pandas as pd
from bs4 import BeautifulSoup
from screen import ROOT,DATES,SCHED,CAL,load
from earnings_calendar_screen import OUT,RULES
from guidance_extract import walk_json


def sources():
    rows=json.loads((OUT/'parsed_notices.json').read_text())
    actual={}
    for sym in ['MDB','ADSK','SNPS','DDOG']:
        for r in json.loads((ROOT/'guidance_expansion'/sym/'parsed_guidance.json').read_text()):
            key=(sym,r['reported_fiscal_year'],r['reported_quarter'])
            actual.setdefault(key,[]).append(dict(date=pd.Timestamp(r['published']).tz_convert('America/New_York').strftime('%Y-%m-%d'),source=r['source']))
    q={'first':1,'second':2,'third':3,'fourth':4}
    for r in json.loads((ROOT/'guidance_probe/guidance_rows.json').read_text()):
        if r.get('actual_quarter'):
            key=('VEEV',r['reported_fiscal_year'],q[r['actual_quarter']['quarter']])
            actual.setdefault(key,[]).append(dict(date=pd.Timestamp(r['published']).tz_convert('America/New_York').strftime('%Y-%m-%d'),source=r['source']))
    for item in json.loads((ROOT/'guidance_expansion/WDAY/downloaded.json').read_text()):
        s=BeautifulSoup((ROOT/'guidance_expansion/WDAY'/item['file']).read_text(),'html.parser');ld=[]
        for script in s.find_all('script',type='application/ld+json'):
            try:ld.extend(walk_json(json.loads(script.get_text())))
            except json.JSONDecodeError:continue
        for r in ld:
            if r.get('@type') not in ['NewsArticle','Article'] or not r.get('datePublished'):continue
            qm=re.search(r'(First|Second|Third|Fourth) Quarter',r['headline'],re.I)
            fy=re.search(r'Fiscal (20\d{2})',r['headline'],re.I)
            if qm and fy:
                actual.setdefault(('WDAY',int(fy[1]),q[qm[1].lower()]),[]).append(dict(date=pd.Timestamp(r['datePublished']).tz_convert('America/New_York').strftime('%Y-%m-%d'),source=item['url']))
    comparisons=[]
    for r in rows:
        path=OUT/r['source_file'];assert hashlib.sha256(path.read_bytes()).hexdigest()==r['sha256']
        soup=BeautifulSoup(path.read_text(),'html.parser');body=soup.select_one('.release-body') or soup
        t=body.get_text(' ',strip=True).replace('\xa0',' ')
        assert r['release_clause'] in t
        for d in r['planned_date_evidence']:assert d['text'] in r['release_clause']
        pub=pd.Timestamp(r['published']);mod=pd.Timestamp(r['modified']);known=pd.Timestamp(r['available_after'])
        assert known==max(pub,mod)+pd.Timedelta(hours=1)
        day=pd.Timestamp(r['planned_date']);assert 0<=(day-pub.tz_convert('America/New_York').tz_localize(None).normalize()).days<=120
        assert r['phase'] in ['before_open','after_close']
        found=actual.get((r['symbol'],r['fiscal_year'],r['quarter']),[])
        comparisons.append(dict(symbol=r['symbol'],fiscal_year=r['fiscal_year'],quarter=r['quarter'],
            notice=r['source'],planned_date=r['planned_date'],observed_releases=found,
            status='matches_observed_date' if any(v['date']==r['planned_date'] for v in found) else ('different_date' if found else 'actual_release_not_in_saved_coverage')))
    (OUT/'planned_actual_comparison.json').write_text(json.dumps(comparisons,indent=2))
    collection=json.loads((OUT/'collection_status.json').read_text())
    parse_summary=json.loads((OUT/'parse_summary.json').read_text())
    assert parse_summary['downloaded']==collection['downloaded']
    assert parse_summary['parsed']+parse_summary['issues']==collection['downloaded']
    assert collection['downloaded']+len(collection['errors'])==collection['catalogued'], 'source acquisition is not yet fully accounted for'
    result=dict(verified_notices=len(rows),source_hash_and_quote_checks=True,
        planned_actual_counts=dict(Counter(r['status'] for r in comparisons)),
        downloaded=collection['downloaded'],catalogued=collection['catalogued'],download_failures=len(collection['errors']),
        timing_rule='max publication/modification + 60 minutes; actual dates are audit-only and never replace plans',
        ready_for_restricted_exploration=len(rows)>0,
        limitations=['Not a complete 30-company calendar','No proof all historical rescheduling notices were archived','Ambiguous conference-only notices remain excluded','Actual-date differences retained, not retrospectively removed'])
    (OUT/'source_audit.json').write_text(json.dumps(result,indent=2));return result


def replay(trades,data,fee,per_name_cap=.2):
    """Rebuild all account cash flows from orders and raw OHLC, not engine totals."""
    cash=100000.;due=[];held={};interest=0.;prev=SCHED.loc['2014-12-31','close'];navs={};fees=0.;divs=0.
    entries={k:list(g.itertuples()) for k,g in trades.groupby('entry')}
    for day,sc in SCHED.loc['2015-01-01':].iterrows():
        op,cl=sc['open'],sc['close']
        charge=max(0.,-cash)*.06*(op-prev).total_seconds()/(365*86400);cash-=charge;interest+=charge
        cash+=sum(amount for when,amount in due if when<=op);due=[(when,a) for when,a in due if when>op]
        for sym,tr in held.items():
            dividend=tr.qty*data[sym].at[day,'div']
            if dividend:due.append((op+pd.Timedelta(days=30),dividend));divs+=dividend
        opening_nav=cash+sum(v for _,v in due)+sum(t.qty*data[s].at[day,'open'] for s,t in held.items())
        for tr in entries.get(op,[]):
            assert tr.symbol not in held and tr.qty>0
            price=data[tr.symbol].at[day,'open'];assert abs(price-tr.entry_price)<1e-8
            notional=tr.qty*price;assert notional<=per_name_cap*opening_nav+1e-6
            charge=notional*fee;fees+=charge;cash-=notional+charge;held[tr.symbol]=tr
        charge=max(0.,-cash)*.06*(cl-op).total_seconds()/(365*86400);cash-=charge;interest+=charge
        for sym,tr in list(held.items()):
            if tr.exit==cl:
                close=data[sym].at[day,'close'];assert abs(close-tr.exit_price)<1e-8
                value=tr.qty*close;charge=value*fee;fees+=charge;cash-=charge
                lag=1 if day>=pd.Timestamp('2024-05-28') else (2 if day>=pd.Timestamp('2017-09-05') else 3)
                settle=CAL.schedule.iloc[CAL.sessions.get_loc(day)+lag]['open'];due.append((settle,value));del held[sym]
        navs[str(day.date())]=cash+sum(v for _,v in due)+sum(t.qty*data[s].at[day,'close'] for s,t in held.items())
        prev=cl
    assert not held
    return pd.Series(navs),dict(finance=interest,fees=fees,dividends=divs,nav=navs[str(DATES[-1].date())])


def main():
    source=sources()
    if '--sources-only' in sys.argv:print(json.dumps(source,indent=2));return
    data,_=load();results=json.loads((OUT/'results.json').read_text());trace=json.loads((OUT/'signal_trace.json').read_text())
    t=pd.read_csv(OUT/'trades.csv');t['entry']=pd.to_datetime(t.entry,utc=True);t['exit']=pd.to_datetime(t.exit,utc=True)
    maxerror=0.;records={}
    for key,result in results.items():
        ts=t[t.strategy==key].copy();rule=key.split('_fee')[0];fee=int(key.split('_fee')[1])/10000
        for sym,g in ts.groupby('symbol'):
            g=g.sort_values('entry');assert (g.entry.iloc[1:].reset_index(drop=True)>g.exit.iloc[:-1].reset_index(drop=True)).all()
        assert (ts.hours<=48).all()
        for tr in ts.itertuples():
            matching=[x for x in trace if x['rule']==rule and x['symbol']==tr.symbol and pd.Timestamp(x['intended_entry'])==tr.entry and x['signal']]
            assert len(matching)==1
            ev=matching[0];assert pd.Timestamp(ev['available_after'])<=tr.entry
            assert pd.Timestamp(ev['intended_exit'])==tr.exit
        expected,flows=replay(ts,data,fee)
        ledger=pd.read_csv(OUT/f'{key}_ledger.csv').set_index('date')
        error=float(np.max(np.abs(expected.reindex(ledger.index).to_numpy()-ledger.nav.to_numpy())))
        assert error<1e-5,(key,error)
        for field in ['fees','finance','nav']:assert abs(flows[field]-result['diagnostics'][field])<1e-5
        maxerror=max(maxerror,error);records[key]=dict(max_daily_nav_error=error,independent_flows=flows)
    prefix=json.loads((OUT/'prefix_checks.json').read_text());assert all(e<1e-12 for x in prefix.values() for e in x.values())
    result=dict(source=source,paths=len(results),trades_including_stress=len(t),max_holding_hours=float(t.hours.max()),
        max_daily_nav_replay_error=maxerror,per_path=records,prefix_checks=prefix,
        no_same_name_overlap=True,all_entries_after_known_notice=True,actual_dates_never_used_for_trading=True)
    (OUT/'audit.json').write_text(json.dumps(result,indent=2));print(json.dumps(result,indent=2))


if __name__=='__main__':main()
