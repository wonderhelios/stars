"""2026 hourly price audit, all fixed candidates; missing hourly prices use daily-open proxy.
This is a sensitivity check, not an independent backtest or historical-universe claim.
"""
import test_candidates as t
import glob,json,os,numpy as np,pandas as pd
original=t.OP.copy(); origraw=t.a['o'].copy(); hourly={}
for f in glob.glob('/tmp/hl-hist/*.json'):
 name=os.path.basename(f)[:-5]
 hourly[name]={int(x['t']):float(x['o']) for x in json.load(open(f)) if float(x['o'])>0}
m=(t.outdates>=pd.Timestamp('2026-03-11',tz='UTC'))&(t.outdates<=pd.Timestamp('2026-10-02',tz='UTC'))
res={}
for hour in [0,1,4]:
 prices=original.copy();known=np.zeros_like(prices,dtype=bool)
 for j,name in enumerate(t.names):
  h=hourly.get(name,{})
  for i,stamp in enumerate(t.times):
   px=h.get(stamp+hour*3600000)
   if px is not None:prices[i,j]=px;known[i,j]=True
 t.OP=prices
 res[str(hour)]={}
 for z,T in t.Ts.items():
  rr=[t.run(T,o) for o in range(3)]
  res[str(hour)][z]={'sharpe':float(np.mean([t.stat(v['r'][m])['sharpe'] for v in rr])), 'annual_arithmetic':float(np.mean([t.stat(v['r'][m])['annual_arithmetic'] for v in rr])), 'target_hourly_gross_coverage':float(np.sum(np.abs(T[t.ix[m]])*known[t.ix[m]+1])/np.maximum(np.abs(T[t.ix[m]]).sum(),1e-9))}
t.OP=original
json.dump({'days':int(m.sum()),'caveat':'Hybrid hourly/daily marks, sparse hourly coverage; not proof of realizable full-book fills. No maker fills assumed. Same daily signals and 3 offsets.','results':res},open(t.OUT+'/hourly_execution.json','w'),indent=2)
print('HOURLY',json.dumps(res,indent=2))
