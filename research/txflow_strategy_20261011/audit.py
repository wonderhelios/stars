"""Recheck saved paths and causality without repeating the candidate search."""
from pathlib import Path
import json
import numpy as np
import pandas as pd
p=Path(__file__).resolve().parent
ns={'__file__':str(p/'run.py')}
exec((p/'run.py').read_text().split('runs={};allmetrics=[]')[0],ns)
z=np.load(p/'paths.npz'); attempts=json.loads((p/'attempts.json').read_text())
assert len(z.files)==281
assert all(np.isfinite(z[a['id']]).all() for a in attempts)
assert all((z[a['id']][:,0]>-1).all() for a in attempts)
assert all(np.allclose(np.cumprod(1+z[a['id']][:,0])[-1]**(365/966)-1,ns['stats'](z[a['id']],ns['masks']['full'])['cagr']) for a in attempts)
# Prefix recomputation: future closes/volume cannot affect earlier factor scores.
bound=ns['N']//2
cl=ns['P']['c'][:bound];vol=ns['P']['v'][:bound]
sd=pd.DataFrame(cl).pct_change(fill_method=None).rolling(20,min_periods=5).std().values
vv=pd.DataFrame(cl*vol).rolling(30,min_periods=5).mean().shift().values
mm=cl/pd.DataFrame(cl).shift(14).values-1
for k,v in {'M':mm/np.maximum(sd,1e-9),'L':-sd,'V':cl*vol/vv}.items():
 assert np.allclose(v,ns['scores'][k][:bound],equal_nan=True)
# Execution causality: modifying all future prices and signals leaves prefix ledger intact.
before=ns['run'](ns['targets']['MLV'],1,0,'base')
target=ns['targets']['MLV'].copy();target[bound:]*=-1
ns['P']['o'][bound:]*=2;ns['lastprice'][bound:]*=2
changed=ns['run'](target,1,0,'base')
assert np.array_equal(before[ns['indices']<bound],changed[ns['indices']<bound])
# Independent two-market, constant-price ledger: $8100 initial gross, 7.5bp each side.
ns.update(K=2,indices=np.array([2,3]),dates=np.arange(4)*86400000,steps=np.array([1.,1.]),hashes=np.array([0,0]),P={'o':np.ones((4,2))},lastprice=np.ones((4,2)),V=np.full((4,2),1e8))
r=ns['run'](np.tile([.5,-.5],(4,1)),1,0,'base')
assert np.isclose(3000*np.prod(1+r[:,0]),3000-8100*.00075*2)
rows=[]
for a in attempts:
 for part,mask in ns['masks'].items():
  b=z[f"MLV_1_0_{a['scenario']}"][mask,0];v=z[a['id']][mask,0]
  rows.append({**a,'part':part,'correlation_to_same_scenario_baseline':float(np.corrcoef(v,b)[0,1])})
pd.DataFrame(rows).to_csv(p/'correlations.csv',index=False)
result={'all_paths_finite':True,'bankrupt_paths':0,'factor_prefix_invariant':True,'ledger_future_perturbation_invariant':True,'independent_constant_price_fee_ledger':True,'paths':280,'unpriced_paths':sum(bool((z[a['id']][:,6]>1e-8).any()) for a in attempts)}
(p/'audit.json').write_text(json.dumps(result,indent=2));print(json.dumps(result))
