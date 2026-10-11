"""Independent timing, execution, and statistical implementation checks."""
import ast,json,hashlib
import numpy as np
import pandas as pd
from common import *
from study import Engine,build_features,thresholds,DIRECTION
from inference import correlation,sharp,bootstrap

def main():
    g=load();e=Engine(g);N=e.N;checks={}
    # Original function statements with only leverage and output interception changed.
    tree=ast.parse(SRC.read_text());node=next(x for x in tree.body if isinstance(x,ast.FunctionDef) and x.name=='run')
    source=ast.get_source_segment(SRC.read_text(),node).replace('eq*2.7/px[j]','eq*LEV[i-33]/px[j]')
    source=source.replace("np.savez_compressed(OUT/f'baseline_cap{cap}_{clock}_{fee}.npz',r=rr,t=dates,turn=turn)","REC.update(r=rr,turn=np.array(turn))")
    (OUT/'extracted_audit_run.py').write_text(source+'\n')
    exec(compile(source,'extracted_audit_run.py','exec'),g)
    masks=[np.arange(N)%7<3,pd.read_csv(OUT/'neff_gate_avg.csv').bad.to_numpy().astype(bool)]
    for j,bad in enumerate(masks):
        g.update(LEV=np.where(bad,1.,2.7),REC={});g['run'](fee=.0007);fast=e.run(bad=bad)
        rd=float(np.max(abs(g['REC']['r']-fast.r)));td=float(np.max(abs(g['REC']['turn']-fast.turn)));assert rd<1e-12 and td<1e-12
        checks[f'original_dynamic_replay_{j}']=dict(return_max_abs=rd,turn_max_abs=td)
    features=pd.read_csv(OUT/'features.csv');base=pd.read_csv(OUT/'baseline_daily.csv');ranks=pd.read_csv(OUT/'online_ranks.csv');future=pd.read_csv(OUT/'future_returns.csv');states=np.load(OUT/'policy_states.npz')
    # Recompute from physically truncated price/volume arrays at three historical cutoffs.
    for date in ['2024-03-06','2024-07-17','2025-07-01']:
        h=load();cut=int(np.flatnonzero(pd.to_datetime(h['ts'],unit='ms').strftime('%Y-%m-%d')==date)[0])+1
        h['n']=cut;h['ts']=h['ts'][:cut]
        for z in ['P','Q','R','V','SD','M']:h[z]=h[z][:cut].copy()
        h['a']={z:x[:cut].copy() for z,x in h['a'].items()};ee=Engine(h);bb=ee.run();mm=ee.run(fee=0,momentum=True);ff,rr=build_features(h,ee,bb,mm)
        np.testing.assert_allclose(ff.to_numpy(),features.drop(columns='date').iloc[:ee.N].to_numpy(),atol=1e-12,rtol=1e-12,equal_nan=True)
        np.testing.assert_allclose(rr.to_numpy(),ranks.drop(columns='date').iloc[:ee.N].to_numpy(),atol=1e-12,rtol=1e-12,equal_nan=True)
        ng=ee.run(neff_window='avg');assert np.array_equal(ng.bad.to_numpy(),states['neff_avg@0.0007'][:ee.N])
        checks['prefix_'+date]=dict(days=ee.N,all_43_features_equal=True,all_online_ranks_equal=True,endogenous_gate_equal=True)
    # restore diagnostic overwritten by prefix checks
    build_features(load(),e,e.run(),e.run(fee=0,momentum=True))
    for c in features.columns[1:]:
        x=features[c].to_numpy();f=c.rsplit('_',1)[0];q,bad=thresholds(x,DIRECTION[f])
        for t in [60,200,245,470,900,N-1]:
            hist=x[:t];hist=hist[np.isfinite(hist)]
            if len(hist)>=60:assert abs(q[t]-np.quantile(hist,.8 if DIRECTION[f]>0 else .2))<1e-10
            else:assert not np.isfinite(q[t])
    checks['thresholds_exclude_current_and_future']=True
    r=base.r.to_numpy()
    for horizon in [1,20,60]:
        y=future[f'future{horizon}'].to_numpy()
        for t in [0,245,470,N-horizon-1]:assert abs(y[t]-(np.prod(1+r[t+1:t+horizon+1])-1))<1e-12
        assert np.isnan(y[-horizon:]).all()
    checks['forward_returns_start_next_day']=True
    # Delta influence numerical derivative under a common reweighting perturbation.
    rng=np.random.default_rng(718);x=rng.normal(size=150);y=.4*x+rng.normal(size=150);mask=np.ones(150,bool);a=rng.normal(size=150);a-=a.mean();eps=1e-6;w=(1+eps*a)/150
    rho,phi,_=correlation(x,y,mask);mx=w@x;my=w@y;weighted=(w@((x-mx)*(y-my)))/np.sqrt((w@((x-mx)**2))*(w@((y-my)**2)))
    assert abs((weighted-rho)/eps-np.mean(phi*a))<1e-5
    s,phi,_=sharp(x,mask);mu=w@x;sig=np.sqrt(w@((x-mu)**2));ws=np.sqrt(365*149/150)*mu/sig
    assert abs((ws-s)/eps-np.mean(phi*a))<1e-5
    checks['influence_finite_difference']=True
    inf=pd.read_csv(OUT/'inference.csv');assert len(inf)==516
    for L in [10,20,40,80,120]:
        assert (inf[f'p_fwer_{L}']+1e-12>=inf[f'p_raw_{L}']).all();assert (inf[f'p_fwer_{L}']>=.0001).all()
    checks['FWER_ge_raw_and_9999_resolution']=True
    manifest=json.loads((OUT/'input_manifest.json').read_text())
    assert all(hashlib.sha256(Path(p).read_bytes()).hexdigest()==v for p,v in manifest.items())
    checks['all_input_hashes_unchanged']=True
    (OUT/'audit.json').write_text(json.dumps(checks,indent=2));print(json.dumps(checks,indent=2),flush=True)
if __name__=='__main__':main()
