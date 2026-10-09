"""Verify selected SEC records and independently reconstruct strategy equity."""
import json, hashlib
import numpy as np
import pandas as pd
from quality import OUT
from screen import ROOT, DATES, load

def main():
    manifest=json.loads((ROOT/'fundamentals/manifest.json').read_text())
    docs={x['symbol']:json.loads((ROOT/'fundamentals'/f'{x["symbol"]}.json').read_text()) for x in manifest if x.get('name_verified')}
    for x in manifest:
        if x.get('name_verified'):
            assert hashlib.sha256((ROOT/'fundamentals'/f'{x["symbol"]}.json').read_bytes()).hexdigest()==x['sha256']
    es=pd.read_csv(OUT/'financial_events.csv')
    for e in es.itertuples():
        assert pd.Timestamp(e.available)>pd.Timestamp(e.filed)
        assert e.available_idx == DATES.searchsorted(pd.Timestamp(e.filed),side='right')+1
        assert max(pd.Timestamp(e.value_filed),pd.Timestamp(e.op_filed),pd.Timestamp(e.comparison_filed))<=pd.Timestamp(e.filed)
        if pd.notna(e.previous_growth_filed): assert pd.Timestamp(e.previous_growth_filed)<pd.Timestamp(e.filed)
        facts=docs[e.symbol]['facts']['us-gaap']
        rs=[v for v in facts[e.tag]['units']['USD'] if v.get('accn')==e.accn and v.get('start')==e.start and v.get('end')==e.end and v.get('filed')==e.value_filed]
        assert any(abs(v['val']-e.revenue)<1e-6 for v in rs)
        ops=[v for v in facts['OperatingIncomeLoss']['units']['USD'] if v.get('accn')==e.op_accn and v.get('start')==e.start and v.get('end')==e.end and v.get('filed')==e.op_filed]
        assert any(abs(v['val']-e.operating_income)<1e-6 for v in ops)
        comps=[v for v in facts[e.comparison_tag]['units']['USD'] if v.get('accn')==e.comparison_accn and v.get('start')==e.comparison_start and v.get('end')==e.comparison_end and v.get('filed')==e.comparison_filed]
        assert any(abs(v['val']-e.previous_revenue)<1e-6 for v in comps)
        assert 350<=(pd.Timestamp(e.end)-pd.Timestamp(e.comparison_end)).days<=380
        assert abs((pd.Timestamp(e.end)-pd.Timestamp(e.start)).days-(pd.Timestamp(e.comparison_end)-pd.Timestamp(e.comparison_start)).days)<=14
        assert abs(e.growth-(e.revenue/e.previous_revenue-1))<1e-12
        assert abs(e.margin-e.operating_income/e.revenue)<1e-12
        if pd.notna(e.previous_growth): assert abs(e.acceleration-(e.growth-e.previous_growth))<1e-12
    data,_=load();t=pd.read_csv(OUT/'trades.csv');t['entry']=pd.to_datetime(t.entry,utc=True);t['exit']=pd.to_datetime(t.exit,utc=True)
    assert (t.exit>t.entry).all() and (t.hours<=48).all()
    for _,g in t.groupby(['strategy','symbol']):
        g=g.sort_values('entry')
        assert (g.entry.iloc[1:].reset_index(drop=True)>g.exit.iloc[:-1].reset_index(drop=True)).all()
    results=json.loads((OUT/'results.json').read_text());errors={}
    for name,g in t.groupby('strategy'):
        diag=results[name]['diagnostics'];assert diag['open_positions']==0
        div=0.
        for row in g.itertuples():
            start=row.entry.tz_convert('America/New_York').tz_localize(None).normalize();end=row.exit.tz_convert('America/New_York').tz_localize(None).normalize()
            d=data[row.symbol];div+=row.qty*d.loc[(d.index>start)&(d.index<=end),'div'].sum()
        nav=100000+g.price_pnl.sum()+div-g.fees.sum()-diag['finance']-diag['borrow']
        errors[name]=abs(nav-diag['nav']);assert errors[name]<1e-5
    check=json.loads((OUT/'prefix_checks.json').read_text());assert all(check.values())
    audit=dict(financial_records_verified=len(es),max_trade_reconciliation_error=max(errors.values()),prefix_checks=check,max_holding_hours=float(t.hours.max()),no_overlap=True,source_manifest_sha256=hashlib.sha256((ROOT/'fundamentals/manifest.json').read_bytes()).hexdigest(),code_sha256={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in ROOT.glob('*.py')})
    (OUT/'audit.json').write_text(json.dumps(audit,indent=2));print(json.dumps(audit,indent=2))

if __name__=='__main__':main()
