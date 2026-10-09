"""12 hypotheses, phase-average statistics; dependent calendar block maxT.

This controls this batch only, not selection across the entire research history.
"""
import json
import numpy as np
import pandas as pd
from screen import ROOT, stat

def main(returns_path=None, results_path=None, output=None):
    output=ROOT if output is None else output
    z=np.load(ROOT/'returns.npz' if returns_path is None else returns_path)
    dates=pd.DatetimeIndex(z['dates'])
    names=sorted(set(k.split('_p')[0] for k in z.files if k.endswith('fee5')))
    raw=json.loads((ROOT/'results.json' if results_path is None else results_path).read_text())
    results={}; summary=[]
    for part,mask in [('full',np.ones(len(dates),bool)),('2015-22',dates.year<=2022),('2023-24',(dates.year>=2023)&(dates.year<=2024)),('2025-26',dates.year>=2025)]:
        obs=[]; influences=[]
        for name in names:
            phases=[k for k in z.files if k.startswith(name+'_p') and k.endswith('fee5')]
            ys=np.stack([z[k][mask] for k in phases],axis=1)
            mu=ys.mean(axis=0);sd=np.maximum(ys.std(axis=0,ddof=1),1e-15)
            sr=mu/sd*np.sqrt(365)
            influence=((ys-mu)/sd-(mu/(2*sd**3))*((ys-mu)**2-sd**2))*np.sqrt(365)
            influences.append(influence.mean(axis=1)); obs.append(float(sr.mean()))
            row=dict(strategy=name,part=part,sharpe=float(sr.mean()),phase_min=float(sr.min()),phase_max=float(sr.max()),annual_return=float(mu.mean()*365),worst_drawdown=min(stat(z[k][mask])['max_drawdown'] for k in phases),trades=sum(raw[k]['diagnostics']['trades'] for k in phases)/len(phases))
            summary.append(row)
        obs=np.array(obs); f=np.stack(influences,axis=1);f-=f.mean(axis=0)
        n=len(f);result={}
        for block in [15,30,60]:
            rng=np.random.default_rng(20261009+block)
            # Moving circular block sums. Exact n observations, including final partial block.
            doubled=np.r_[f,f];cs=np.vstack([np.zeros((1,len(names))),np.cumsum(doubled,axis=0)])
            dist=[]
            for _ in range(0,4999,128):
                b=min(128,4999-len(dist)*128)
                starts=rng.integers(0,n,size=(b,(n+block-1)//block))
                lens=np.full(starts.shape,block);lens[:,-1]=n-block*(starts.shape[1]-1)
                dist.append((cs[starts+lens]-cs[starts]).sum(axis=1)/n)
            boot=np.vstack(dist)
            se=boot.std(axis=0,ddof=1)
            nullmax=(boot/np.maximum(se,1e-15)).max(axis=1)
            p=((nullmax[:,None]>=obs/np.maximum(se,1e-15)).sum(axis=0)+1)/(len(boot)+1)
            result[str(block)]={name:dict(p_fwer=float(p[j]),simultaneous_lower95=float(obs[j]-np.quantile(nullmax,.95)*se[j])) for j,name in enumerate(names)}
        results[part]=result
    table=pd.DataFrame(summary)
    table['p_fwer_worst_block']=[max(results[row.part][str(b)][row.strategy]['p_fwer'] for b in [15,30,60]) for row in table.itertuples()]
    table.to_csv(output/'phase_summary.csv',index=False)
    (output/'inference.json').write_text(json.dumps(dict(hypotheses=len(names),bootstrap=4999,method='phase-average Sharpe influence-function circular block maxT vs zero, one-sided; exploratory family only',results=results),indent=2))
    print(table.loc[table.part=='2025-26'].to_string(index=False))

if __name__=='__main__': main()
