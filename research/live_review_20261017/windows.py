"""Explicit cold-start windows: avoids inheriting unverifiable retired positions.
Not a spliceable full-history curve, and not untouched out-of-sample.
"""
import json
import numpy as np
import pandas as pd
import backtest as b
modes=[('baseline',0),('half',0),('third',0),('cap5',0),('band10',0),('signal_only',0),('slices3',0),('slices3',1),('slices3',2)]
def index(date):return int(np.searchsorted(b.ts,pd.Timestamp(date,tz='UTC').value//1000000))
windows={'2023-24':(33,index('2025-01-01')),'2025_after_FTM_gap':(index('2025-01-14'),index('2026-01-01')),'2026':(index('2026-01-01'),len(b.ts)),'recent_Oct08_Oct16':(index('2026-10-08'),len(b.ts))}
res={}
for name,(lo,hi) in windows.items():
 if hi<=lo:continue
 res[name]={}
 for mode,phase in modes:
  a=b.simulate(mode,phase=phase,start_idx=lo,end_idx=hi)
  z=b.metrics(a);z['missing_coins']=a['missing_coins'];z['price_coverage_pass']=not a['missing_coins']
  stress=b.simulate(mode,phase=phase,start_idx=lo,end_idx=hi,fee=.00125)
  z['cost_stress_total']=b.metrics(stress)['total'];res[name][mode+'_p'+str(phase)]=z
 print(name,[(k,round(v['sharpe'],2),round(v['total']*100,2),v['price_coverage_pass']) for k,v in res[name].items()],flush=True)
w=b.target(len(b.ts)-1,5739.2678625,2.7,'baseline')
res['latest_targets']={str(b.names[j]):float(w[j]) for j in np.flatnonzero(abs(w)>1e-12)}
(b.ROOT/'window_results.json').write_text(json.dumps(res,indent=2))
