"""24 UTC execution hours. Same closed UTC daily signals; complete observed-hour subset.
No fallback marks, no re-anchored candles, all3 schedule offsets, restricted universe.
"""
from engine import *
import ast,math
source=open(OUT+'/run.py').read();tree=ast.parse(source)
funcs={}
for node in tree.body:
 if isinstance(node,ast.FunctionDef) and node.name in ['sif','maxT']:
  exec(ast.get_source_segment(source,node),globals(),funcs)
globals().update(funcs)
print('functions',list(funcs), 'source',OUT,flush=True)
H=np.full((24,n,k),np.nan)
for f in glob.glob('/tmp/hl-hist/*.json'):
 name=os.path.basename(f)[:-5]
 if name not in names:continue
 j=names.index(name)
 for x in json.load(open(f)):
  t=int(x['t']);day=t//DAY*DAY;h=(t-day)//3600000
  if day in ti:H[h,ti[day],j]=float(x['o'])
start=pd.Timestamp('2026-03-11',tz='UTC').value//1000000;end=pd.Timestamp('2026-10-02',tz='UTC').value//1000000
obsidx=np.flatnonzero((np.array(times)>=start)&(np.array(times)<=end));keep=np.isfinite(H[:,obsidx]).all(axis=(0,1));restricted=[names[j] for j in np.flatnonzero(keep)]
assert keep.sum()>=8
V[:,~keep]=np.nan;base,_,univ=targets()
# Override engine globals: start zero, each hour enters at observed opening after midnight.
START=obsidx[0]-1;END=obsidx[-1];ix=np.arange(START,END);outdates=dates[ix+1];OPorig=OP.copy();Oorig=a['o'].copy()
# run's globals belong to engine module, not this importing module.
import engine as e
e.START=START;e.END=END;e.ix=ix
runs={}
for hour in range(24):
 e.OP=np.where(np.isfinite(H[hour]),H[hour],OPorig);e.a['o']=H[hour]
 rr=[run(base,o) for o in range(3)];runs['hour_'+str(hour)]=rr
 # Selected universe is fully observed at every execution and mark in sample.
 assert max(v['missing'].max() for v in rr)==0
runs['baseline']=runs['hour_0']
mask=np.ones(len(ix),bool);boot=maxT(runs,'baseline',mask)
res={}
bavg=np.mean([v['r'] for v in runs['baseline']],axis=0)
for z,rr in runs.items():
 sh=[stat(v['r'])['sharpe'] for v in rr];to=float(np.mean([v['turn'].mean() for v in rr]));res[z]=dict(sharpe=float(np.mean(sh)),phase_sharpes=sh,phase_sd=float(np.std(sh)),phase_range=float(np.ptp(sh)),annual_net_arithmetic=float(np.mean([v['r'].mean()*365 for v in rr])),daily_turnover_equity=to,annual_cost=to*.00075*365,correlation_baseline=float(np.corrcoef(np.mean([v['r'] for v in rr],axis=0),bavg)[0,1]),correlation_same_phase=float(np.mean([np.corrcoef(v['r'],runs['baseline'][o]['r'])[0,1] for o,v in enumerate(rr)])))
meta=dict(start=str(outdates[0]),end=str(outdates[-1]),days=len(ix),restricted_universe=restricted,coins=int(keep.sum()),candidates=24,caveat='2026 only, historical currently-surviving hourly-covered universe subset; cannot pass 2023-24 gate. Same UTC daily signal delayed to hour h; does not test rolling daily candle origins. Complete observed hourly prices, no proxy or touched-maker fills.')
json.dump(dict(meta=meta,summary=res,bootstrap=boot),open(OUT+'/hourly_results.json','w'),indent=2)
np.savez_compressed(OUT+'/hourly_returns.npz',dates=np.asarray(outdates.astype(str),dtype='U'),**{z:np.array([v['r'] for v in rr]) for z,rr in runs.items()})
for z,v in res.items():print(z,round(v['sharpe'],3),boot['30'].get(z,{}).get('p_fwer_global'))
