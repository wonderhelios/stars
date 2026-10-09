"""Frozen baseline execution with only explicit slot/universe targets changed."""
import ast, hashlib, json, sys
from pathlib import Path
import numpy as np
import pandas as pd
ROOT=Path.cwd()
assert ROOT==Path('/Users/wonder/Code/stars')
OUT=ROOT/'research/universe_size_20261009'
SRC=ROOT/'research/trailing_tp_20261008/baseline.py'
source=SRC.read_text(); tree=ast.parse(source)
cutoff=next(x.lineno for x in tree.body if isinstance(x,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='out' for t in x.targets))
g={'__file__':str(SRC),'__name__':'frozen_engine'}
exec(compile('\n'.join(source.splitlines()[:cutoff-1]),str(SRC),'exec'),g)
g['OUT']=OUT
original_target=g['target']; original_run=g['run']
n,k=g['n'],g['k']; names=np.array(g['names']); P,V,SD,M,Q=[g[z] for z in ['P','V','SD','M','Q']]
configs=[dict(name='baseline',K=None,N=None)]+[dict(name=f'K{K}',K=K,N=None) for K in [3,5,8,12,20]]+[dict(name=f'N{N}_K{K}',K=K,N=N) for N,K in [(30,5),(50,8),(50,12),(80,15)]]
fees=[.0007,.00045,.00015,.00075]
run_node=next(x for x in tree.body if isinstance(x,ast.FunctionDef) and x.name=='run')
runsource=ast.get_source_segment(source,run_node)
elig=[]
for i in range(n):
 elig.append(np.flatnonzero((V[i]>=5e6)&np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])&np.array([':' not in c for c in names])) if i>=32 else np.array([],int))
class Target:
 def __init__(self,c):self.c=c;self.info={}
 def __call__(self,i,equity,cap=5):
  c=self.c;idx=elig[i];universe=len(idx)
  if c['N'] is not None:idx=idx[np.lexsort((names[idx],-V[i,idx]))][:c['N']]
  kk=0;w=np.zeros(k);normalizer=0.
  eqcap=max(1,int(np.floor(equity*2.7/15))//4)
  if len(idx)>=8:
   kk=min(max(1,int(np.floor(len(idx)*.2+.5))),cap,eqcap,len(idx)//2) if c['K'] is None else min(c['K'],len(idx)//2)
   for s in [M[i]/np.maximum(SD[i],1e-9),-SD[i],Q[i]/V[i]]:
    order=idx[np.lexsort((names[idx],s[idx]))];w[order[:kk]]-=.5/kk/3;w[order[-kk:]]+=.5/kk/3
   normalizer=float(abs(w).sum());w=w/normalizer if normalizer>0 else w
  if c['name']=='baseline':
   exact=original_target(i,equity,cap)
   assert np.array_equal(w,exact)
   w=exact
  assert np.isfinite(w).all()
  assert normalizer==0 or np.isclose(abs(w).sum(),1)
  assert abs(w.sum())<1e-12
  self.info=dict(universe=universe,selected_n=len(idx),kk=kk,normalizer=normalizer,equity_cap=eqcap,
                 constrained=int(kk>0 and c['K'] is not None and kk<c['K']),
                 target_long=int((w>1e-12).sum()),target_short=int((w< -1e-12).sum()))
  return w

def engine(c,phase,fee):
 target=Target(c);rows=[]
 def record(d):
  q=d['qty']+d['delta'];px=np.nan_to_num(d['px']);eq=d['eq']
  rows.append(dict(date=pd.to_datetime(g['ts'][d['i']],unit='ms',utc=True).strftime('%Y-%m-%d'),
                   **target.info,actual_long=int((q>1e-12).sum()),actual_short=int((q< -1e-12).sum()),
                   turn=float(np.sum(abs(d['delta'])*px)/eq)))
 g['target']=target;g['record']=record
 s=runsource.replace('range(33,n)',f'range({33+phase},n)').replace('  cost=fee*','  record(locals())\n  cost=fee*')
 exec(compile(s,'<baseline_run_with_readonly_diagnostics>','exec'),g)
 res=g['run'](fee=fee)
 p=OUT/f'baseline_cap5_open_{fee}.npz';data=dict(np.load(p));p.unlink()
 return res,data,pd.DataFrame(rows)

def main():
 allres={}
 for c in configs:
  for phase in range(3):
   for fee in fees:
    key=f"{c['name']}_p{phase}_f{fee}";path=OUT/f'{key}.npz';jp=OUT/f'{key}.json'
    if path.exists() and jp.exists():res=json.loads(jp.read_text())
    else:
     res,data,diag=engine(c,phase,fee)
     np.savez_compressed(path,**data);diag.to_csv(OUT/f'{key}_daily.csv',index=False)
     jp.write_text(json.dumps(res,indent=2))
    if c['name']=='baseline' and phase==0 and fee==.0007:
     a=np.load(OUT/'original_baseline.npz');b=np.load(path)
     assert abs(res['full']['sharpe']-1.1293)<.00005
     assert all(np.array_equal(a[x],b[x]) for x in ['r','t','turn'])
     (OUT/'baseline_gate.json').write_text(json.dumps(dict(sharpe=res['full']['sharpe'],exact_daily_match=True,days=len(b['r'])),indent=2))
    assert not res['missing_held_quotes'],key
    allres[key]=res
   (OUT/'grid_results.json').write_text(json.dumps(allres,indent=2))
   print(c['name'],phase,'checkpoint',flush=True)
 paths=[SRC,ROOT/'docs/validation-protocol.md',OUT/'SPEC.md',Path(__file__),SRC.parent/'retrieval/request.json',SRC.parent/'retrieval/meta.json']+sorted(Path('/tmp/hl-daily-v2').glob('*.json'))
 (OUT/'manifest.json').write_text(json.dumps({str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in paths},indent=2))
 (OUT/'data_scope.json').write_text(json.dumps(dict(python=sys.executable,files=k,loaded_days=n,loaded_start=str(pd.to_datetime(g['ts'][0],unit='ms',utc=True)),loaded_end=str(pd.to_datetime(g['ts'][-1],unit='ms',utc=True)),return_start=str(pd.to_datetime(g['ts'][33],unit='ms',utc=True)),return_end=str(pd.to_datetime(g['ts'][-1],unit='ms',utc=True))),indent=2))
if __name__=='__main__':main()
