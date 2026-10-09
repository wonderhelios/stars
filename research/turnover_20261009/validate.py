import json
import numpy as np
import study as s
checks={};b=np.load(s.OUT/'baseline_p0_f0.0007.npz');original=np.load(s.SRC.parent/'baseline_cap5_open_0.0007.npz')
checks['original_baseline_arrays_equal']=bool(all(np.array_equal(b[k],original[k]) for k in ['r','turn','t']))
checks['target_invariants']={}
for c in s.configs:
 target=s.Target(c);maxerr=0.;gate=0;finite=True
 for i in range(32,s.n-1):
  w=target(i,1e6,5);gross=abs(w).sum();maxerr=max(maxerr,min(abs(gross),abs(gross-1)));gate+=int(s.elig[i].sum()<8 and gross>1e-12);finite &=bool(np.isfinite(w).all())
 checks['target_invariants'][c['name']]=dict(max_gross_normalization_error=maxerr,universe_gate_breaches=gate,all_finite=finite)
 assert maxerr<1e-12 and gate==0 and finite
res,data=s.engine(dict(name='identity',kind='smooth',alpha=1.),0,.0007)
checks['alpha1_matches_baseline_returns']=bool(np.allclose(data['r'],b['r'],atol=1e-12,rtol=0))
checks['alpha1_matches_baseline_turn']=bool(np.allclose(data['turn'],b['turn'],atol=1e-12,rtol=0))
assert all(checks[k] for k in ['original_baseline_arrays_equal','alpha1_matches_baseline_returns','alpha1_matches_baseline_turn'])
(s.OUT/'engine_checks.json').write_text(json.dumps(checks,indent=2));print(json.dumps(checks,indent=2))
