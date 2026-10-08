"""Independent invariants over saved trades; does not rerun or search candidates."""
from pathlib import Path
import json,numpy as np,pandas as pd
P=Path(__file__).parent
E=pd.read_csv(P/'events.csv');L=pd.read_csv(P/'legs.csv');meta=json.load(open(P/'meta_context.json'))
md={x['name'].split(':')[-1]:x for x in meta[0]['universe']}
for z in ['entry','exit','signal_start','signal_end']:E[z]=pd.to_datetime(E[z],utc=True)
for z in ['entry','exit']:L[z]=pd.to_datetime(L[z],utc=True)
assert (E.signal_end<=E.entry).all();assert (E.entry<E.exit).all()
assert (E.exit<=pd.Timestamp('2026-10-04',tz='UTC')).all()
checks={}
for path,e in E.groupby('path'):
 e=e.sort_values('entry');assert (e.entry.iloc[1:].to_numpy()>=e.exit.iloc[:-1].to_numpy()).all(),path
checks['overlap_violations']=0
size=L.groupby(['path','entry']).weight.agg(lambda z:z.abs().sum());assert np.max(np.abs(size-1))<1e-9
net=L[~L.path.str.contains('_none_')].groupby(['path','entry']).weight.sum();assert np.max(np.abs(net))<1e-9
checks['gross_weight_max_error']=float(np.max(np.abs(size-1)));checks['hedged_net_weight_max_error']=float(np.max(np.abs(net)))
iso=[c for c in L[L.path.str.startswith('cross9')].coin.unique() if md[c].get('onlyIsolated',False) or md[c].get('marginMode','normal')!='normal'];assert not iso
checks['cross_universe_invalid_margin_assets']=iso
# Recompute fixed-quantity gross and exit turnover from individual price fields.
g=L.weight*(L.exit_px/L.entry_px-1);tr=L.weight.abs()*(1+L.exit_px/L.entry_px)
assert np.max(np.abs(g-L.gross))<1e-12;assert np.max(np.abs(tr-L.turn))<1e-12
checks['gross_accounting_max_error']=float(np.max(np.abs(g-L.gross)))
# Exact event calendar alignment incl America/New_York conversion.
a=E[E.path.str.contains('external_weekend')];op=a.signal_end.dt.tz_convert('America/New_York');pc=a.signal_start.dt.tz_convert('America/New_York')
assert (op.dt.weekday==6).all() and (op.dt.hour==20).all() and (op.dt.minute==0).all()
assert (pc.dt.weekday==4).all() and (pc.dt.hour==20).all()
checks['external_weekend_independent_dates']=int(a.signal_end.nunique())
a=E[E.path.str.contains('overnight')| (E.path.str.contains('_weekend_')&~E.path.str.contains('external_weekend'))];op=a.signal_end.dt.tz_convert('America/New_York');assert (op.dt.hour==9).all() and (op.dt.minute==30).all()
checks['cash_open_unique_utc_clock_times']=sorted(a.signal_end.dt.strftime('%H:%M').unique().tolist())
# Direct funding recomputation for both holding types, independently of cumulative cache.
errors=[]
for _,r in pd.concat([L[L.path.str.contains('daily')].iloc[::max(1,len(L[L.path.str.contains('daily')])//10)],L[L.path.str.contains('overnight')].iloc[::max(1,len(L[L.path.str.contains('overnight')])//10)]]).iterrows():
 intra='daily' not in r.path
 f=pd.DataFrame(json.load(open(P/'raw'/f'{r.coin}_funding.json')));f.index=pd.to_datetime(f.time,unit='ms',utc=True)
 raw=P/'raw'/f'{r.coin}_30m.json' if intra else Path('/tmp/xyz')/f'xyz_{r.coin}.json'
 d=pd.DataFrame(json.load(open(raw)));d.index=pd.to_datetime(d.t,unit='ms',utc=True);d=d.sort_index();step=pd.Timedelta(minutes=30) if intra else pd.Timedelta(days=1)
 total=0
 for t,x in f[(f.index>r.entry)&(f.index<=r.exit)].iterrows():
  v=d[d.index+step<=t]
  if len(v):total-=r.weight/r.entry_px*float(v.iloc[-1].c)*float(x.fundingRate)
 errors.append(abs(total-r.funding))
assert max(errors)<1e-10;checks['direct_funding_max_error']=max(errors)
checks['paths_observed']=int(E.path.nunique());checks['event_book_count']=len(E);checks['leg_count']=len(L)
json.dump(checks,open(P/'checks.json','w'),indent=2);print(json.dumps(checks,indent=2))
