"""AST extraction; no import of baseline and no execution of its final sweep."""
import ast, hashlib, json
from pathlib import Path
import numpy as np
import pandas as pd
ROOT=Path('/Users/wonder/Code/stars')
OUT=ROOT/'research/regime_20261011'
SRC=ROOT/'research/trailing_tp_20261008/baseline.py'
assert Path.cwd()==ROOT

def load():
    source=SRC.read_text(); tree=ast.parse(source)
    cutoff=next(x.lineno for x in tree.body if isinstance(x,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='out' for t in x.targets))
    g={'__file__':str(SRC),'__name__':'regime_ast_extract'}
    exec(compile('\n'.join(source.splitlines()[:cutoff-1]),str(SRC),'exec'),g)
    g['OUT']=OUT
    return g

def stats(r):
    r=np.asarray(r); r=r[np.isfinite(r)]
    if not len(r):return dict(days=0,sharpe=np.nan,total=np.nan,mdd=np.nan,cagr=np.nan,annual=np.nan)
    e=np.r_[1,np.cumprod(1+r)]
    return dict(days=len(r),sharpe=float(r.mean()/r.std(ddof=1)*np.sqrt(365)) if r.std(ddof=1)>0 else np.nan,total=float(e[-1]-1),mdd=float((e/np.maximum.accumulate(e)-1).min()),cagr=float(e[-1]**(365/len(r))-1),annual=float(r.mean()*365))

def gate():
    g=load(); result=g['run'](5,'open',.0007)
    r=np.load(OUT/'baseline_cap5_open_0.0007.npz')
    check=dict(**stats(r['r']),start=str(pd.to_datetime(r['t'][0],unit='ms').date()),end=str(pd.to_datetime(r['t'][-1],unit='ms').date()),baseline_result=result)
    (OUT/'baseline_gate.json').write_text(json.dumps(check,indent=2))
    assert round(result['full']['sharpe'],4)==1.1293,check
    ref=np.load(SRC.parent/'baseline_cap5_open_0.0007.npz')
    assert np.array_equal(ref['r'],r['r']) and np.array_equal(ref['t'],r['t']) and np.array_equal(ref['turn'],r['turn'])
    manifest={str(p):hashlib.sha256(Path(p).read_bytes()).hexdigest() for p in g['files']+[str(SRC),str(SRC.parent/'retrieval/request.json'),str(SRC.parent/'retrieval/meta.json'),'docs/validation-protocol.md','research/drawdown_attr_20261010/REPORT.md']}
    (OUT/'input_manifest.json').write_text(json.dumps(manifest,indent=2))
    print(json.dumps(check,indent=2),flush=True)
    return g
if __name__=='__main__':gate()
