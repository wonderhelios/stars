"""Independent source and account replay for the frozen macro-calendar batch."""
import hashlib
import json
import re
import sys
import numpy as np
import pandas as pd
from bs4 import BeautifulSoup
from screen import ROOT, DATES, CAL, load, signals, Signal, simulate
from macro_calendar_screen import OUT, CONFIGS
from macro_calendar_parse import normalize
from earnings_calendar_audit import replay


def sources():
    events=json.loads((OUT/'planned_events.json').read_text())
    catalog=json.loads((OUT/'minutes_catalogue.json').read_text())
    calendar=BeautifulSoup((ROOT/'options_events/raw/fomccalendars.html').read_text(),'html.parser')
    actual={}
    for block in calendar.select('.fomc-meeting'):
        if 'notation vote' in block.get_text(' ',strip=True).lower():continue
        for a in block.find_all('a',href=True):
            m=re.fullmatch(r'/monetarypolicy/fomcminutes(\d{8})\.htm',a['href'])
            if m:actual[str(pd.Timestamp(m[1]).date())]=normalize(block.get_text(' ',strip=True))
    dec=normalize(BeautifulSoup((OUT/'dec2020_publication.html').read_text(),'html.parser').get_text(' ',strip=True))
    assert 'Released January 06, 2021' in dec
    comparisons=[]
    for r in events:
        p=OUT/r['source_file'];h=hashlib.sha256(p.read_bytes()).hexdigest()
        assert h==r['sha256']==json.loads(p.with_suffix('.meta.json').read_text())['sha256']
        text=normalize(BeautifulSoup(p.read_text(),'html.parser').get_text(' ',strip=True))
        assert r['schedule_evidence'] in text and r['update_evidence'] in text
        item=next(v for v in catalog if v['url']==r['source'])
        if item['meeting_id']!='20201216':
            ev=actual[r['source_meeting_date']]
            assert 'Released '+item['published_date'] in ev
        pub=pd.Timestamp(item['published_date']);mod=pd.Timestamp(r['last_update_date'])
        expected=(max(pub,mod)+pd.Timedelta(days=1,hours=1)).tz_localize('America/New_York')
        assert expected==pd.Timestamp(r['available_after'])
        within=r['planned_date']<='2026-10-07'
        if within:assert r['planned_date'] in actual
        comparisons.append(dict(planned_date=r['planned_date'],within_research_window=within,
            matches_observed_meeting=r['planned_date'] in actual,late_update=r['late_update_beyond_planned_event']))
    collection=json.loads((OUT/'collection_status.json').read_text())
    assert collection['finished'] and collection['catalogued']==collection['downloaded']==len(events)
    assert not collection['errors'] and not json.loads((OUT/'parse_issues.json').read_text())
    result=dict(verified_events=len(events),within_window=sum(r['within_research_window'] for r in comparisons),
        late_updates=sum(r['late_update'] for r in comparisons),comparisons=comparisons,
        ready_for_exploration=True,source_hash_quote_and_publication_checks=True,
        actual_calendar_audit_only=True,limitations=['Current source versions; late modifications conservatively delay availability',
        'No proof all extraordinary rescheduling announcements are archived'])
    (OUT/'source_audit.json').write_text(json.dumps(result,indent=2));return result


def main():
    source=sources()
    if '--sources-only' in sys.argv:print(json.dumps(source,indent=2));return
    data,_=load();results=json.loads((OUT/'results.json').read_text());trace=json.loads((OUT/'signal_trace.json').read_text())
    trades=pd.read_csv(OUT/'trades.csv');trades['entry']=pd.to_datetime(trades.entry,utc=True);trades['exit']=pd.to_datetime(trades.exit,utc=True)
    records={};maxerror=0.
    for key,result in results.items():
        ts=trades[trades.strategy==key];name=result['configuration'];gross=result['gross'];settlements=[]
        for sym,g in ts.groupby('symbol'):
            g=g.sort_values('entry');assert (g.entry.iloc[1:].reset_index(drop=True)>g.exit.iloc[:-1].reset_index(drop=True)).all()
        assert (ts.hours<=48).all()
        for t in ts.itertuples():
            match=[r for r in trace if r['configuration']==name and r['eligible'] and pd.Timestamp(r['entry'])==t.entry]
            assert len(match)==1 and t.symbol in CONFIGS[name]['assets']
            ev=match[0];assert pd.Timestamp(ev['available_after'])<=t.entry and pd.Timestamp(ev['exit'])==t.exit
            day=t.exit.tz_convert('America/New_York').tz_localize(None).normalize()
            lag=1 if day>=pd.Timestamp('2024-05-28') else 2
            settled=CAL.schedule.iloc[CAL.sessions.get_loc(day)+lag]['open']
            settlements.append((settled-t.entry).total_seconds()/3600)
        expected,flows=replay(ts,data,result['fee'],per_name_cap=gross)
        ledger=pd.read_csv(OUT/f'{key}_ledger.csv').set_index('date')
        error=float(np.max(np.abs(expected.reindex(ledger.index).to_numpy()-ledger.nav.to_numpy())))
        assert error<1e-5,(key,error)
        for f in ['finance','fees','nav']:assert abs(flows[f]-result['diagnostics'][f])<1e-5
        assert result['diagnostics']['borrow']==0
        for entry,g in ts.groupby('entry'):
            vals=g.qty*g.entry_price
            assert len(g)==len(CONFIGS[name]['assets']) and vals.max()-vals.min()<1e-6
        maxerror=max(maxerror,error)
        records[key]=dict(max_daily_nav_error=error,flows=flows,entry_to_sale_settlement_hours_max=max(settlements),
            entry_to_sale_settlement_hours_median=float(np.median(settlements)))
    # Shared default must preserve an already frozen unrelated strategy exactly.
    old=Signal(signals(data)['stock_gap_up']);old.name='stock_gap_up'
    r,*_=simulate(data,old,2)
    frozen=np.load(ROOT/'returns.npz',allow_pickle=False)
    err=float(np.max(np.abs(r.to_numpy()-frozen['stock_gap_up_h2_p0_fee5'])))
    assert err==0
    prefix=json.loads((OUT/'prefix_checks.json').read_text());assert all(e==0 for checks in prefix.values() for e in checks.values())
    audit=dict(source=source,paths=len(results),trades_including_stress_and_gross=len(trades),
        max_daily_nav_replay_error=maxerror,max_holding_hours=float(trades.hours.max()),
        default_engine_regression_error=err,prefix_checks=prefix,per_path=records,
        all_entries_after_source_available=True,no_same_name_overlap=True,equal_notional_mix_verified=True)
    (OUT/'audit.json').write_text(json.dumps(audit,indent=2));print(json.dumps({k:v for k,v in audit.items() if k not in ['per_path','source','prefix_checks']},indent=2))


if __name__=='__main__':main()
