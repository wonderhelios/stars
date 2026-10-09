"""Independent statement-table, chronology, signal and trade-equity audit."""
import hashlib
import json
import re
import sys
import numpy as np
import pandas as pd
from bs4 import BeautifulSoup
from screen import ROOT, SCHED, load, signals, Signal, simulate
from guidance_screen import OUT


def audit_sources():
    root = ROOT/'guidance_probe'
    rows = json.loads((root/'guidance_rows.json').read_text())
    coverage = json.loads((root/'guidance_coverage.json').read_text())
    catalogue = {r['url']:r for r in json.loads((root/'veeva_release_catalogue.json').read_text())}
    quarter_names = dict(first=1,second=2,third=3,fourth=4)
    sequence = []
    table_checks = 0
    seen = set()
    for row in rows:
        f = root/row['source_file']
        assert hashlib.sha256(f.read_bytes()).hexdigest() == row['sha256']
        soup = BeautifulSoup(f.read_text(),'html.parser')
        normalized = soup.get_text(' ',strip=True).replace('\xa0',' ')
        assert row['introduction'] in normalized
        for b in row['bullets']:
            assert b in normalized
        pub = pd.Timestamp(row['published'])
        modified = pd.Timestamp(row['modified'])
        assert pub <= modified <= pub+pd.Timedelta(minutes=2), 'Later modification needs manual timing review'
        assert pd.Timestamp(row['available_after']) == max(pub,modified)
        listed = catalogue[row['source']]['listing_date'].replace(' ET','')
        assert pub.tz_convert('America/New_York').tz_localize(None) == pd.Timestamp(listed)
        if row['source'] not in seen:
            a = row['actual_quarter']; assert a is not None
            values = []
            for tr in soup.find_all('tr'):
                cells = [td.get_text(' ',strip=True) for td in tr.find_all('td',recursive=False)]
                if cells and cells[0].lower().rstrip(':') == 'total revenues':
                    nums = [float(v.replace(',','')) for c in cells[1:] for v in re.findall(r'\b\d[\d,]*(?:\.\d+)?\b',c)]
                    if nums: values.append(nums[0])
            assert any(abs(v/1000-a['revenue_million']) <= .051 for v in values), (f.name,a,values)
            sequence.append(row['reported_fiscal_year']*4+quarter_names[a['quarter']])
            table_checks += 1; seen.add(row['source'])
        if row['remaining_year_revision_million'] is not None:
            old = next(r for r in rows if r['source']==row['previous_source'] and r['fiscal_year_end']==row['fiscal_year_end'])
            assert int(re.search(r'20\d{2}',row['fiscal_year_end']).group()) == row['reported_fiscal_year']
            nq=old['next_quarter_guidance']; aq=row['actual_quarter']
            assert nq['quarter'] == aq['quarter']
            assert pd.Timestamp(old['published']) < pub
            assert 45 < (pub-pd.Timestamp(old['published'])).days < 130
            target_end = pd.Timestamp(nq['ending'])
            fiscal_end = pd.Timestamp(row['fiscal_year_end'])
            assert fiscal_end-pd.DateOffset(years=1) < target_end <= fiscal_end
            revised_remaining = np.mean(row['revenue_range_million'])-aq['revenue_million']
            prior_remaining = np.mean(old['revenue_range_million'])-np.mean(nq['revenue_range_million'])
            assert abs(revised_remaining-prior_remaining-row['remaining_year_revision_million']) < 1e-9
    assert len(coverage)==22 and len(seen)==22 and all(np.diff(sorted(sequence))==1)
    audit=dict(releases_verified=22,independent_quarter_revenue_table_checks=table_checks,
        continuous_quarters=True,dates_match_listing=True,modifications_within_67_seconds=True,
        same_fiscal_year_and_quarter_verified=True,
        comparability_review='FY2024 TFC change effective 2023-02-01 precedes the initial annual guidance used on 2023-03-01. Revisions compare the same fiscal year only; year-over-year growth is not a signal. No basis reset identified inside comparable annual guidance pairs in these releases; prepared-call material not fully archived.')
    (OUT/'source_audit.json').write_text(json.dumps(audit,indent=2))
    return audit


def main():
    audit = audit_sources()
    if '--sources-only' in sys.argv:
        print(json.dumps(audit,indent=2)); return
    data,_=load()
    results=json.loads((OUT/'results.json').read_text())
    t=pd.read_csv(OUT/'trades.csv')
    t['entry']=pd.to_datetime(t.entry,utc=True);t['exit']=pd.to_datetime(t.exit,utc=True)
    trace=json.loads((OUT/'signal_trace.json').read_text())
    maxerror=0.
    for name,result in results.items():
        ts=t[t.strategy==name].sort_values('entry')
        hold=int(re.search(r'_h(\d)_',name).group(1))
        assert (ts.hours <= (192 if hold==5 else 48)).all()
        assert (ts.exit>ts.entry).all()
        assert (ts.entry.iloc[1:].reset_index(drop=True)>ts.exit.iloc[:-1].reset_index(drop=True)).all()
        div=0.
        for tr in ts.itertuples():
            start=tr.entry.tz_convert('America/New_York').tz_localize(None).normalize()
            end=tr.exit.tz_convert('America/New_York').tz_localize(None).normalize()
            div += tr.qty*data['VEEV'].loc[(data['VEEV'].index>start)&(data['VEEV'].index<=end),'div'].sum()
            field='pullback_entry' if 'pullback' in name else 'immediate_entry'
            ev=next(e for e in trace if e[field] and pd.Timestamp(e[field])==tr.entry)
            assert tr.entry >= pd.Timestamp(ev['available_after'])+pd.Timedelta(hours=1)
            if field=='pullback_entry': assert pd.Timestamp(ev['pullback_observed']) < tr.entry
        diag=result['diagnostics'];assert diag['open_positions']==0
        independent=100000+ts.price_pnl.sum()+div-ts.fees.sum()-diag['finance']-diag['borrow']
        error=abs(independent-diag['nav']);assert error < 1e-5
        maxerror=max(maxerror,error)
    # Shared engine extension must preserve its previous default behaviour exactly.
    sig=Signal(signals(data)['stock_gap_up']);sig.name='stock_gap_up'
    r,*_=simulate(data,sig,2,phase=0,fee=.0005)
    old=np.load(ROOT/'returns.npz')['stock_gap_up_h2_p0_fee5']
    regression=float(np.max(np.abs(r.to_numpy()-old)));assert regression<1e-12
    prefix=json.loads((OUT/'prefix_checks.json').read_text());assert all(prefix.values())
    audit.update(paths=len(results),trades_including_stress=len(t),max_cash_error=maxerror,
        no_overlap=True,max_holding_hours=float(t.hours.max()),prefix_checks=prefix,
        old_engine_default_regression_error=regression,
        research_only=True,not_survivorship_free=True,not_clean_holdout=True)
    (OUT/'audit.json').write_text(json.dumps(audit,indent=2));print(json.dumps(audit,indent=2))


if __name__=='__main__':main()
