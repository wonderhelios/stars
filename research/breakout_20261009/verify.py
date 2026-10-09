"""Data integrity and structural execution checks; no parameter search."""
import hashlib,json
from pathlib import Path
import numpy as np
from run import B, F, ELIG, HERE, SAVE, SOURCE

manifest=json.loads((HERE/'baseline_manifest.json').read_text())
changed=[p for p,h in manifest.items() if hashlib.sha256(Path(p).read_bytes()).hexdigest()!=h]
assert not changed,changed
checks=[]
for path in sorted(SAVE.glob('*.json')):
    r=json.loads(path.read_text())
    if r.get('invalid'):
        checks.append(dict(key=r['key'],invalid=True,failure_t=r['failure_t']));continue
    z=np.load(path.with_suffix('.npz'));scheduled=(z['t']//86400000+r['phase'])%r['period']==0
    assert np.all(z['turn'][~scheduled]==0),r['key']
    assert len(z['r'])==1192 and np.isfinite(z['r']).all(),r['key']
    assert np.isclose(z['turn'].mean()*.0007*365,r['full']['annual_cost']),r['key']
    checks.append(dict(key=r['key'],missing_held_quotes=len(r['missing_held_quotes']),off_schedule_turnover_zero=True))

# Exact 252-day affine identity and causal prefix invariance for rolling formulas.
import pandas as pd
hi=pd.DataFrame(B.P).rolling(253,min_periods=253).max().values
assert np.allclose(B.P/hi-1,F['distance252'],equal_nan=True)
prefix_checks=[]
for stop in [400,800,1100]:
    sub=B.P[:stop]
    for N in [20,60,120,252]:
        h=pd.DataFrame(sub).rolling(N+1,min_periods=N+1).max().values
        assert np.allclose(sub/h-1,F[f'distance{N}'][:stop],equal_nan=True)
        prev=pd.DataFrame(sub).rolling(N,min_periods=N).max().shift(1).values
        f=np.where(np.isfinite(prev)&np.isfinite(sub),(sub>prev).astype(float),np.nan)
        assert np.allclose(f,F[f'binary{N}'][:stop],equal_nan=True)
    for N in [20,120]:
        h=pd.DataFrame(sub).rolling(N+1,min_periods=N+1).max().values
        l=pd.DataFrame(sub).rolling(N+1,min_periods=N+1).min().values
        f=np.divide(sub-l,h-l,out=np.full_like(sub,np.nan),where=h>l)
        assert np.allclose(f,F[f'channel{N}'][:stop],equal_nan=True)
    h=pd.DataFrame(sub).rolling(365,min_periods=365).max().values
    assert np.allclose(sub/h,F['high52'][:stop],equal_nan=True)
    prefix_checks.append(stop)

# Compare the fast block sum implementation with explicit resampled daily arrays.
from analyze import starts, BLOCK, N, boot, sharpe
for key in ['baseline_mix_k1_p0','channel20_single_k5_p0']:
    z=np.load(SAVE/(key+'.npz'));bs,bt=boot(key)
    for row in range(3):
        idx=((starts[row,:,None]+np.arange(BLOCK)[None,:])%N).ravel()[:N]
        assert np.isclose(sharpe(z['r'][idx]),bs[row],atol=1e-10)
        assert np.isclose(z['turn'][idx].mean(),bt[row],atol=1e-10)

coverage=[]
binary_diagnostics=[]
for name,f in F.items():
    size=(ELIG&np.isfinite(f)).sum(axis=1)
    inds=np.flatnonzero(size[32:-1]>=8)+32
    coverage.append(dict(factor=name,first_signal_date=str(pd.to_datetime(B.ts[inds[0]],unit='ms',utc=True)),active_days=len(inds),zero_universe_days=int(np.count_nonzero(size[32:-1]<8))))
    if name.startswith('binary'):
        longs=[];shorts=[];constant=[]
        for i in inds:
            idx=np.flatnonzero(ELIG[i]&np.isfinite(f[i])); kk=min(max(1,int(np.floor(len(idx)*.2+.5))),5,len(idx)//2)
            order=idx[np.lexsort((np.array(B.names)[idx],f[i,idx]))]
            longs.append(float(f[i,order[-kk:]].mean()));shorts.append(float(f[i,order[:kk]].mean()));constant.append(bool(np.ptp(f[i,idx])==0))
        binary_diagnostics.append(dict(factor=name,constant_signal_day_fraction=float(np.mean(constant)),long_newhigh_fraction=float(np.mean(longs)),short_newhigh_fraction=float(np.mean(shorts))))
out=dict(input_hashes_unchanged=True,files_checked=len(manifest),exact_replay=json.loads((HERE/'engine_verification.json').read_text()),
         runs=checks,causal_prefix_checks=prefix_checks,bootstrap_explicit_resample_verified=True,coverage=coverage,binary_diagnostics=binary_diagnostics,failed_runs=[x for x in checks if x.get('invalid')])
(HERE/'verification.json').write_text(json.dumps(out,indent=2))
print(json.dumps(dict(input_hashes_unchanged=True,runs=len(checks),failed_runs=out['failed_runs'],missing_quote_events=sum(x.get('missing_held_quotes',0) for x in checks)),indent=2))
