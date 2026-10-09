"""Economic accounting, no-lookahead, size and adverse execution checks."""
import hashlib
import json
import numpy as np
import pandas as pd
from carry import OUT, inputs, run, stats
from combine import trend, portfolio

d=inputs()
dates=pd.to_datetime(d['times'],unit='ms',utc=True)
checks={}
# An inverse short funded with one BTC must offset a doubling AND a halving of spot.
for s in [10000.,40000.]:
    equity=(1.+20000.*(1/s-1/20000.))*s
    assert abs(equity-20000.)<1e-10
checks['inverse_contract_hedge_doubling_halving']=True
# Cash and basis engines independently reconcile every daily equity change inside run().
whole,_=run(d,gated=True)
short={k:(v[:700] if isinstance(v,np.ndarray) else v) for k,v in d.items()}
prefix,_=run(short,gated=True)
err=float(np.max(np.abs(whole['r'][:699]-prefix['r'][:-1]))) # Last prefix day liquidates, so exclude only that day.
assert err==0
checks['gated_carry_prefix_error']=err
size={}
for capital in [1000.,10000.,100000.]:
    try:
        v,audit=run(d,gated=True,initial_equity=capital)
        size[str(int(capital))]=dict(full=stats(v['r']),late=stats(v['r'][dates.year>=2025]),audit=audit)
    except (ValueError,AssertionError) as e:
        size[str(int(capital))]=dict(error='Ledger or solvency assertion: '+str(e))
stress_carry,audit=run(d,gated=True,stress=True,slippage=.0006,transfer_delay=3)
stress_trend,_=trend(d,fee_rate=.00105)
dated=dict(np.load(OUT/'dated_returns.npz'))
saved=dict(np.load(OUT/'portfolio_returns.npz'))
start_time=int(saved['times'][0]);first=int(np.flatnonzero(d['times']==start_time)[0]);common_start=first-126
R=np.array([stress_carry['r'][common_start:],dated['dated_equal_slip6bp'][common_start:],stress_trend[common_start:]]).T
out=[]
for phase in range(30):
    rr,ww,cc=portfolio(R,'iv',phase,126,fee=.004,entry_costs=np.array([.0021,.00465,.00105]))
    out.append(rr)
rr=np.array(out);dd=pd.to_datetime(saved['times'],unit='ms',utc=True)
stress={}
for label,m in [('full',np.ones(len(dd),bool)),('2023-24',dd.year<=2024),('2025-26',dd.year>=2025)]:
    ss=[stats(r[m]) for r in rr]
    stress[label]={k:float(np.mean([s[k] for s in ss])) for k in ss[0]}
    stress[label]['phase_range']=[min(s['sharpe'] for s in ss),max(s['sharpe'] for s in ss)]
    rf=saved['rf'][m]
    stress[label]['sharpe_sofr']=float(np.mean([(r[m]-rf).mean()/(r[m]-rf).std(ddof=1)*np.sqrt(365) for r in rr]))
summary=json.loads((OUT/'portfolio_results.json').read_text())
checks.update(summary['checks'])
result=dict(checks=checks,size_sensitivity=size,combined_stress=stress,stress_carry_audit=audit,
            stress_definition='Adverse signed funding oracle, missing-hour penalty, 6bp slippage, 3-day transfers, doubled inter-sleeve allocation cost')
(OUT/'verification.json').write_text(json.dumps(result,indent=2))
# Capture inputs and implementation after completion. Bad/empty API probes are retained as evidence, not used as data.
files=list(OUT.glob('*.py'))+list(OUT.glob('*.json'))+list((OUT/'raw').glob('*'))+[OUT/'SPEC.md',OUT.parent/'portfolio_20261009/input.npz',OUT.parent/'portfolio_20261009/returns.npz']
manifest={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in files if p.is_file() and p.name!='manifest.json'}
(OUT/'manifest.json').write_text(json.dumps(manifest,indent=2))
print(json.dumps(result,indent=2),flush=True)
