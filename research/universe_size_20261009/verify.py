import json, hashlib
from pathlib import Path
import numpy as np
import pandas as pd
import study as s
from analyze import sharpe,drawdown
O=s.OUT
checks={}
# Independently execute original uninstrumented run with only target swapped.
for c in s.configs:
 s.g['target']=s.Target(c)
 res=s.original_run(fee=.0007)
 p=O/'baseline_cap5_open_0.0007.npz';fresh=dict(np.load(p));p.unlink()
 saved=dict(np.load(O/f"{c['name']}_p0_f0.0007.npz"))
 assert all(np.array_equal(fresh[k],saved[k]) for k in ['r','t','turn'])
 df=pd.read_csv(O/f"{c['name']}_p0_f0.0007_daily.csv")
 expected=np.where(df.selected_n>=8,np.minimum(c['K'],df.selected_n//2),0) if c['K'] else df.kk.values
 assert np.array_equal(df.kk,expected)
 assert np.allclose(df.turn,saved['turn'],rtol=0,atol=1e-14)
 if c['K']:assert df.equity_cap.min()>c['K']
 if c['N']:assert (df.selected_n==np.minimum(df.universe,c['N'])).all()
 checks[c['name']]=dict(original_run_exact=True,min_equity_cap=int(df.equity_cap.min()),
                       top_n_binding_days=int((df.selected_n<df.universe).sum()),
                       no_missing_quotes=not res['missing_held_quotes'])
 for phase in range(3):
  for fee in s.fees:
   d=dict(np.load(O/f"{c['name']}_p{phase}_f{fee}.npz"))
   assert np.all(d['r']>-1)
   assert np.all(np.diff(d['t'])==86400000)
   eq=pd.Series(np.r_[1,np.cumprod(1+d['r'])]);mdd=float((eq/eq.cummax()-1).min())
   assert mdd==drawdown(d['r'],d['t'])['max_drawdown']
# Sufficient statistics bootstrap must equal direct nonlinear Sharpe calculation.
a=np.load(O/'aligned_returns.npz');r=a['r'];T=r.shape[-1]
for L in [10,20,40]:
 rng=np.random.default_rng(20261009+L);st=rng.integers(0,T,size=(100,int(np.ceil(T/L))))
 ix=((st[:,:,None]+np.arange(L))%T).reshape(100,-1)[:,:T][0]
 ss=np.array([[sharpe(p[ix]) for p in c] for c in r]).mean(1)
 saved=np.load(O/f'bootstrap_L{L}.npz')['delta'][0]
 assert np.allclose(saved,ss[1:]-ss[0],rtol=0,atol=1e-12)
 stats=json.loads((O/'bootstrap.json').read_text())[str(L)]
 assert np.all(np.array(stats['p_fwer'])>=np.array(stats['p']))
checks['bootstrap_sufficient_moments_vs_direct']=True
checks['drawdown_all_120_paths_independent_pandas_match']=True
checks['original_baseline_sha256']=hashlib.sha256(s.SRC.read_bytes()).hexdigest()
checks['source_unchanged']=checks['original_baseline_sha256']==json.loads((O/'manifest.json').read_text())[str(s.SRC)]
assert checks['source_unchanged']
(O/'verification.json').write_text(json.dumps(checks,indent=2))
print(json.dumps(checks,indent=2))
