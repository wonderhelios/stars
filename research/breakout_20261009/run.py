"""Frozen-baseline execution, factor screen and checkpointed candidate runs."""
import argparse
import hashlib
import json
import types
from pathlib import Path
import numpy as np
import pandas as pd

HERE = Path(__file__).resolve().parent
SOURCE = HERE.parent / 'trailing_tp_20261008/baseline.py'
SAVE = HERE / 'runs'
SAVE.mkdir(exist_ok=True)
code = SOURCE.read_text().split('\nout=dict(config=')[0]
# Keep the exact execution loop; only the scheduled-order mask and output name change.
code = code.replace('if not np.isfinite(px[j]):continue  # cannot liquidate stale missing quote',
                    'if not np.isfinite(px[j]) or (ts[i]//86400000+PHASE)%PERIOD != 0:continue  # scheduled order mask')
code = code.replace("OUT/f'baseline_cap{cap}_{clock}_{fee}.npz'", "OUT/(RUN_ID+'.npz')")
B = types.ModuleType('frozen_baseline')
B.__file__ = str(SOURCE)
exec(compile(code, str(SOURCE), 'exec'), B.__dict__)
B.OUT = SAVE
ORIGINAL_TARGET = B.target
P, M, SD, V, Q, names = B.P, B.M, B.SD, B.V, B.Q, np.array(B.names)
ELIG = (V >= 5e6) & np.isfinite(P) & np.isfinite(np.roll(P,14,axis=0)) & np.isfinite(SD) & np.array([':' not in c for c in names])
ELIG[:32] = False
F = {}
for N in [20,60,120,252]:
    hi = pd.DataFrame(P).rolling(N+1,min_periods=N+1).max().values
    prev = pd.DataFrame(P).rolling(N,min_periods=N).max().shift(1).values
    F[f'distance{N}'] = P/hi-1
    F[f'binary{N}'] = np.where(np.isfinite(prev)&np.isfinite(P), (P>prev).astype(float), np.nan)
F['high52'] = P/pd.DataFrame(P).rolling(365,min_periods=365).max().values
for N in [20,120]:
    hi = pd.DataFrame(P).rolling(N+1,min_periods=N+1).max().values
    lo = pd.DataFrame(P).rolling(N+1,min_periods=N+1).min().values
    F[f'channel{N}'] = np.divide(P-lo,hi-lo,out=np.full_like(P,np.nan),where=hi>lo)

def dump(path, obj):
    path.write_text(json.dumps(obj, indent=2, allow_nan=False))

def corr(x,y):
    return float(np.corrcoef(x,y)[0,1]) if np.std(x)>0 and np.std(y)>0 else np.nan

def describe(values):
    a=np.array(values); a=a[np.isfinite(a)]
    return dict(mean=float(a.mean()),sd=float(a.std(ddof=1)),p10=float(np.quantile(a,.1)),p90=float(np.quantile(a,.9)),days=len(a)) if len(a)>1 else None

def screen():
    rows=[]
    for name,f in F.items():
        daily=[];ties=[];counts=[]
        for i in range(32,B.n-1):
            idx=ELIG[i]&np.isfinite(f[i])
            if idx.sum()<8:continue
            x=f[i,idx]; m=M[i,idx]; z=m/np.maximum(SD[i,idx],1e-9)
            xr=pd.Series(x).rank().values
            daily.append([B.ts[i],corr(x,m),corr(xr,pd.Series(m).rank().values),corr(x,z),corr(xr,pd.Series(z).rank().values)])
            ties.append(1-len(np.unique(x))/len(x));counts.append(int(idx.sum()))
        a=np.array(daily)
        stats={label:describe(a[:,j+1]) for j,label in enumerate(['M_pearson','M_spearman','MSD_pearson','MSD_spearman'])}
        gate=max(stats['M_pearson']['mean'],stats['M_spearman']['mean'])
        rows.append(dict(factor=name,stats=stats,screen_correlation=gate,stopped=bool(gate>.8),mean_tie_fraction=float(np.mean(ties)),mean_universe=float(np.mean(counts))))
        pd.DataFrame(daily,columns=['t','M_pearson','M_spearman','MSD_pearson','MSD_spearman']).to_csv(HERE/(name+'_correlation.csv'),index=False)
    dump(HERE/'correlations.json',rows)
    dates=pd.to_datetime(B.ts,unit='ms',utc=True)
    dump(HERE/'data_audit.json',dict(files=len(names),rows=B.n,start=str(dates[0]),end=str(dates[-1]),return_start=str(dates[33]),return_days=B.n-33,
        eligible_min=int(ELIG[32:].sum(axis=1).min()),eligible_max=int(ELIG[32:].sum(axis=1).max()),
        source_sha256=hashlib.sha256(SOURCE.read_bytes()).hexdigest(),numpy=np.__version__,pandas=pd.__version__))
    for r in rows: print(r['factor'],round(r['screen_correlation'],4),'STOP' if r['stopped'] else 'CONTINUE',flush=True)

def target_factory(factor, mode):
    f=F[factor]
    def target(i,equity,cap=5):
        cap=min(cap,max(1,int(np.floor(equity*2.7/15))//4))
        w=np.zeros(B.k)
        if i<32:return w
        idx=np.flatnonzero(ELIG[i]&np.isfinite(f[i]))
        if len(idx)<8:return w
        kk=min(max(1,int(np.floor(len(idx)*.2+.5))),cap,len(idx)//2)
        signals=[M[i]/np.maximum(SD[i],1e-9),-SD[i],Q[i]/V[i]] if mode=='universe' else ([f[i]] if mode=='single' else [f[i],-SD[i],Q[i]/V[i]])
        for s in signals:
            order=idx[np.lexsort((names[idx],s[idx]))]
            w[order[:kk]]-=.5/kk/len(signals);w[order[-kk:]]+=.5/kk/len(signals)
        g=np.abs(w).sum()
        return w/g if g>0 else w
    return target

def execute(factor, mode, period, phase):
    key=f'{factor}_{mode}_k{period}_p{phase}'
    if (SAVE/(key+'.json')).exists():return json.loads((SAVE/(key+'.json')).read_text())
    B.target=ORIGINAL_TARGET if factor=='baseline' else target_factory(factor,mode)
    B.PERIOD=period;B.PHASE=phase;B.RUN_ID=key
    try:
        result=B.run(5,'open',.0007)
    except ValueError as e:
        if str(e)!='insolvent':raise
        tb=e.__traceback__
        while tb.tb_next is not None:tb=tb.tb_next
        loc=tb.tb_frame.f_locals
        result=dict(invalid=True,reason='frozen baseline raised insolvent',failure_t=int(B.ts[loc['i']]),failure_equity=float(loc['eq']))
    result.update(factor=factor,mode=mode,period=period,phase=phase,key=key)
    dump(SAVE/(key+'.json'),result)
    print(key,('INSOLVENT '+str(result['failure_t'])) if result.get('invalid') else (round(result['full']['sharpe'],4),round(result['full']['daily_turnover'],4)),flush=True)
    return result

def main(stage):
    if stage=='screen':screen();return
    if stage=='verify':
        execute('baseline','mix',1,0)
        ref=np.load(HERE/'baseline_reference.npz'); got=np.load(SAVE/'baseline_mix_k1_p0.npz')
        assert all(np.array_equal(ref[z],got[z]) for z in ['r','t','turn']), 'engine replay differs'
        dump(HERE/'engine_verification.json',dict(exact_array_equality=True,fields=['r','t','turn'],source_sha256=hashlib.sha256(SOURCE.read_bytes()).hexdigest()))
        return
    rows=json.loads((HERE/'correlations.json').read_text()); factors=[r['factor'] for r in rows if not r['stopped']]
    if stage=='daily':
        for factor in factors:
            for mode in ['mix','single','universe']:execute(factor,mode,1,0)
    elif stage=='phases':
        for p in range(3):execute('baseline','mix',3,p)
        for factor in factors:
            for mode in ['mix','single']:
                for p in range(3):execute(factor,mode,3,p)
    elif stage=='hold':
        scores={f:float(np.mean([json.loads((SAVE/f'{f}_{mode}_k1_p0.json').read_text())['full']['sharpe'] for mode in ['mix','single']])) for f in factors}
        best=sorted(scores,key=lambda f:(-scores[f],f))[0]
        dump(HERE/'selection.json',dict(method='mean daily Sharpe across mix/single',scores=scores,selected=best))
        for period in [3,5,10]:
            for p in range(period):
                execute('baseline','mix',period,p)
                for mode in ['mix','single']:execute(best,mode,period,p)
    else:raise ValueError(stage)

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('stage',choices=['screen','verify','daily','phases','hold'])
    main(parser.parse_args().stage)
