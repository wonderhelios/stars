import json,pandas as pd,numpy as np
from pathlib import Path
O=Path(__file__).resolve().parent;r=json.load(open(O/'results.json'));checks=[]
for c in ['BTC','ETH']:
 a=json.load(open(O/f'{c}_dvol.json'));ts=np.array([x[0] for x in a]);assert len(set(ts))==len(ts);assert np.all(np.diff(ts)==86400000)
 f=pd.read_csv(O/f'{c}_aligned.csv',index_col=0,parse_dates=True);f.index=pd.to_datetime(f.index,utc=True)
 tested=0
 for i in np.flatnonzero(np.isfinite(f.futurevar.values))[::83]:
  window=f.r.iloc[i+1:i+31];assert len(window)==30 and window.notna().all()
  manual=(window**2).sum()*365/30;assert np.isclose(manual,f.futurevar.iloc[i]);tested+=1
 assert tested>10
 for p,v in r[c].items():assert len(v['phases'])==30;assert sum(x['n'] for x in v['phases'])==v['n']
 checks.append({'currency':c,'dvol_daily_contiguous':True,'future_label_manual_checks':tested,'all_30_phases_count_reconciled':True})
assert r['multiple_testing']['family_size']==8
for L,vs in r['multiple_testing']['tests'].items():assert len(vs)==8;assert all(0<=v['p_fwer']<=1 for v in vs.values())
json.dump({'checks':checks,'maxT_family_and_pvalues_valid':True,'note':'Data/accounting integrity checks only; no executable-strategy validation.'},open(O/'audit.json','w'),indent=2)
print(checks)
