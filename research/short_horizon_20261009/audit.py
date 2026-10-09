"""Independent completed-trade accounting and chronology checks."""
import json, hashlib
import numpy as np
import pandas as pd
from screen import ROOT, load, signals

def main():
 data,_=load();full=signals(data)
 # Rebuild the features from truncated price history, not merely truncate trades.
 short={k:v.iloc[:2300].copy() for k,v in data.items()};prefix=signals(short)
 assert all(np.array_equal(a[:2300],prefix[n][s]) for n,x in full.items() for s,a in x.items())
 t=pd.read_csv(ROOT/'trades.csv');t['entry']=pd.to_datetime(t.entry,utc=True);t['exit']=pd.to_datetime(t.exit,utc=True)
 assert (t.exit>t.entry).all() and (t.hours<=48).all()
 overlap=0
 for _,g in t.groupby(['strategy','symbol']):
  g=g.sort_values('entry');overlap+=int((g.entry.iloc[1:].reset_index(drop=True)<=g.exit.iloc[:-1].reset_index(drop=True)).sum())
 assert overlap==0
 independent={};allres=json.loads((ROOT/'results.json').read_text())
 for name,g in t.groupby('strategy'):
  diag=allres[name]['diagnostics'];assert diag['open_positions']==0
  div=0.
  for row in g.itertuples():
   start=row.entry.tz_convert('America/New_York').tz_localize(None).normalize()
   end=row.exit.tz_convert('America/New_York').tz_localize(None).normalize()
   d=data[row.symbol];div+=row.qty*d.loc[(d.index>start)&(d.index<=end),'div'].sum()
  nav=100000+g.price_pnl.sum()+div-g.fees.sum()-diag['finance']-diag['borrow']
  err=abs(nav-diag['nav']);assert err<1e-5
  independent[name]=err
 sources={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in ROOT.glob('*.py')}
 result=dict(no_future_feature_prefix=True,no_overlapping_trades=True,max_hold_hours=float(t.hours.max()),independent_trade_reconciliation_max_dollars=max(independent.values()),per_strategy_errors=independent,code_sha256=sources,limitations=['Selected present-day stock universe, including missing ANSS history, is not survivorship-free.','Yahoo split-adjusted prices and fractional adjusted shares are proxies; historical auction fills not replayed.','Cash settlement uses exchange trading sessions; bank-only holidays can differ, so cash recycling remains a proxy.','3% borrow/6% financing/5bp trading friction are scenario assumptions, not verified historical executable quotes.','Short availability not verified; no short candidate can be promoted.','Dividend payment uses conservative 30-calendar-day proxy; true pay dates missing.','Cash P&L over holidays is booked at next session; daily correlation with UTC crypto is not synchronized.'])
 (ROOT/'audit.json').write_text(json.dumps(result,indent=2));print(json.dumps(result,indent=2))

if __name__=='__main__':main()
