"""Paired block-bootstrap Sharpe difference; all 12 exploratory variants.
Centered max-T is an approximate dependence-aware family-wise diagnostic.
It cannot repair prior search, funding/model gaps, or prove future profitability.
"""
import json
from pathlib import Path
import numpy as np
R=Path(__file__).resolve().parent;x=json.loads((R/'final_results.json').read_text());a=np.load(R/'final_series.npz');keys=list(x['windows']['validation_2025']);windows=['validation_2025','validation_2026'];mat=np.concatenate([np.column_stack([a[w+'__'+k] for k in keys]) for w in windows]);sizes=[len(a[w+'__baseline']) for w in windows]
def sh(m):return m.mean(0)/m.std(0,ddof=1)*np.sqrt(365)
obs=sh(mat)[1:]-sh(mat)[0];out={};rng=np.random.default_rng(20261020)
for bl in [15,30,60]:
 samples=[]
 for rep in range(1999):
  inds=[];offset=0
  for n in sizes:
   starts=rng.integers(0,n,int(np.ceil(n/bl)));inds.extend((((starts[:,None]+np.arange(bl))%n).ravel()[:n]+offset).tolist());offset+=n
  v=sh(mat[inds]);samples.append(v[1:]-v[0])
 samples=np.array(samples);se=samples.std(0,ddof=1);mx=((samples-obs)/se).max(1)
 out[str(bl)]={k:{'delta_sharpe':float(obs[j]),'paired_block_percentile_ci95':np.percentile(samples[:,j],[2.5,97.5]).tolist(),'p_fwer_approx':float((1+np.sum(mx>=obs[j]/se[j]))/2000)} for j,k in enumerate(keys[1:])}
(R/'sharpe_validation.json').write_text(json.dumps(out,indent=2));print(json.dumps(out['30'],indent=2))
