"""All eight main hypotheses; moving-block uncertainty, no winner selection."""
import json
import numpy as np
import pandas as pd
from macro_calendar_screen import OUT, CONFIGS


def main():
    z=np.load(OUT/'returns.npz');dates=pd.DatetimeIndex(z['dates']);names=list(CONFIGS);results={}
    for part,mask in [('full',np.ones(len(dates),bool)),('2021-22',dates.year<=2022),
                      ('2023-24',(dates.year>=2023)&(dates.year<=2024)),('2025-26',dates.year>=2025)]:
        y=np.stack([z[n+'_g1_fee5'][mask] for n in names],axis=1)
        mu=y.mean(axis=0);sd=y.std(axis=0,ddof=1);assert (sd>0).all()
        obs=mu/sd*np.sqrt(365)
        f=((y-mu)/sd-mu/(2*sd**3)*((y-mu)**2-sd**2))*np.sqrt(365)
        f-=f.mean(axis=0);n=len(f);cs=np.vstack([np.zeros((1,len(names))),np.cumsum(np.r_[f,f],axis=0)])
        results[part]={}
        for block in [15,30,60]:
            rng=np.random.default_rng(20261009+block);draws=[];B=9999
            for start in range(0,B,128):
                starts=rng.integers(0,n,size=(min(128,B-start),(n+block-1)//block))
                lengths=np.full(starts.shape,block);lengths[:,-1]=n-block*(starts.shape[1]-1)
                draws.append((cs[starts+lengths]-cs[starts]).sum(axis=1)/n)
            boot=np.vstack(draws);se=boot.std(axis=0,ddof=1);scaled=boot/se;threshold=obs/se
            p=((scaled>=threshold).sum(axis=0)+1)/(B+1)
            pmax=((scaled.max(axis=1)[:,None]>=threshold).sum(axis=0)+1)/(B+1)
            results[part][str(block)]={name:dict(sharpe=float(obs[j]),p_one_sided=float(p[j]),
                p_maxT_eight=float(pmax[j]),p_bonferroni_43=float(min(1,p[j]*43))) for j,name in enumerate(names)}
    payload=dict(hypotheses_this_batch=8,registered_hypotheses_this_series=43,bootstrap=9999,
        method='Sharpe influence-function circular moving-block, one-sided vs zero; maxT across eight main paths; conservative Bonferroni over 43 registered hypotheses',
        limitations=['Approximate asymptotic bootstrap with sparse events','Does not control the entire project history or prove portfolio improvement',
        'Gross 2.7 and stress are sensitivities, not separately selected candidates','Previously examined history is not clean out-of-sample'],results=results)
    (OUT/'inference.json').write_text(json.dumps(payload,indent=2))
    print({name:max(results['full'][str(b)][name]['p_bonferroni_43'] for b in [15,30,60]) for name in names})


if __name__=='__main__':main()
