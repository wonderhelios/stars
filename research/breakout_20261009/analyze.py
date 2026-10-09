"""Paired circular block bootstrap; each return series is independently checkpointed."""
import json
from pathlib import Path
import numpy as np
import pandas as pd

HERE=Path(__file__).resolve().parent
RUNS=HERE/'runs'; CACHE=HERE/'bootstrap'; CACHE.mkdir(exist_ok=True)
REPS=49999; BLOCK=30; FAMILY=484; SEED=20261009
ref=np.load(RUNS/'baseline_mix_k1_p0.npz')
N=len(ref['r']); D=pd.to_datetime(ref['t'],unit='ms',utc=True)
starts=np.random.default_rng(SEED).integers(0,N,size=(REPS,(N+BLOCK-1)//BLOCK),dtype=np.int32)

def dump(path,obj):path.write_text(json.dumps(obj,indent=2,allow_nan=False))
def sharpe(r):return float(r.mean()/r.std(ddof=1)*np.sqrt(365))
def boot(key):
    path=CACHE/(key+'.npz')
    if path.exists():
        z=np.load(path);return z['sharpe'],z['turn']
    z=np.load(RUNS/(key+'.npz'))
    assert np.array_equal(z['t'],ref['t'])
    r=z['r'];t=z['turn']
    x=np.stack([r,r*r,t],axis=1);xx=np.concatenate([x,x],axis=0)
    cs=np.concatenate([np.zeros((1,3)),np.cumsum(xx,axis=0)])
    full=cs[np.arange(N)+BLOCK]-cs[np.arange(N)]
    rem=N%BLOCK;nb=N//BLOCK
    tail=cs[np.arange(N)+rem]-cs[np.arange(N)]
    means=[];turn=[]
    for i in range(0,REPS,1000):
        s=starts[i:i+1000]
        totals=full[s[:,:nb]].sum(axis=1)
        if rem:totals+=tail[s[:,nb]]
        avg=totals[:,0]/N
        sd=np.sqrt(np.maximum(0,(totals[:,1]-N*avg*avg)/(N-1)))
        means.append(avg/sd*np.sqrt(365));turn.append(totals[:,2]/N)
    bs=np.concatenate(means);bt=np.concatenate(turn)
    np.savez_compressed(path,sharpe=bs,turn=bt)
    return bs,bt

def pair(key,control):
    meta=json.loads((RUNS/(control+'.json')).read_text())
    if meta.get('invalid'):return dict(invalid=True,control=control,reason=meta['reason'])
    c=np.load(RUNS/(key+'.npz'));b=np.load(RUNS/(control+'.npz'))
    cs,ct=boot(key);bs,bt=boot(control)
    delta=sharpe(c['r'])-sharpe(b['r']); db=cs-bs
    p=float((1+np.count_nonzero(db-delta>=delta))/(REPS+1)) if delta>0 else 1.
    td=float(b['turn'].mean()-c['turn'].mean());tb=bt-ct
    tp=float((1+np.count_nonzero(tb-td>=td))/(REPS+1)) if td>0 else 1.
    return dict(control=control,delta=delta,p=p,p_fwer=min(1.,p*FAMILY),correlation=float(np.corrcoef(c['r'],b['r'])[0,1]),
                delta_ci95=np.quantile(db,[.025,.975]).tolist(),turnover_reduction=td,turnover_reduction_ci95=np.quantile(tb,[.025,.975]).tolist(),
                turnover_p=tp,turnover_p_fwer=min(1.,tp*FAMILY),subdelta=[sharpe(c['r'][m])-sharpe(b['r'][m]) for m in [D.year<=2024,D.year>=2025]])

def getrow(key):
    r=json.loads((RUNS/(key+'.json')).read_text())
    if r.get('invalid'):return r
    r['vs_daily']=pair(key,'baseline_mix_k1_p0')
    if r['period']>1:r['vs_matched']=pair(key,f"baseline_mix_k{r['period']}_p{r['phase']}")
    if r['period']==1 and r['mode']!='universe':r['vs_universe']=pair(key,f"{r['factor']}_universe_k1_p0")
    return r

def aggregate(rows):
    if any(r.get('invalid') for r in rows):
        return dict(invalid=True,failed_phases=[r['phase'] for r in rows if r.get('invalid')],phase_count=len(rows),reason='insolvent phases retained; no survivor-only average')
    matched_ok=all(not r['vs_matched'].get('invalid') for r in rows)
    s=np.array([r['full']['sharpe'] for r in rows]); d=np.array([r['vs_daily']['delta'] for r in rows]); ds=np.array([r['vs_matched']['delta'] for r in rows if not r['vs_matched'].get('invalid')])
    sub=np.array([[r[label]['sharpe'] for label in ['2023-24','2025-26']] for r in rows])
    p=max(r['vs_daily']['p'] for r in rows);pm=max(r['vs_matched']['p'] for r in rows) if matched_ok else None
    shb=np.stack([boot(r['key'])[0] for r in rows]).mean(axis=0)-boot('baseline_mix_k1_p0')[0]
    tb=boot('baseline_mix_k1_p0')[1]-np.stack([boot(r['key'])[1] for r in rows]).mean(axis=0)
    turn=float(np.mean([r['full']['daily_turnover'] for r in rows]))
    td=float(ref['turn'].mean()-turn)
    tp=float((1+np.count_nonzero(tb-td>=td))/(REPS+1)) if td>0 else 1.
    return dict(sharpe=float(s.mean()),sharpe_sd=float(s.std(ddof=1)),sharpe_range=[float(s.min()),float(s.max())],
        delta=float(d.mean()),delta_sd=float(d.std(ddof=1)),delta_min=float(d.min()),matched_delta=float(ds.mean()) if matched_ok else None,matched_delta_sd=float(ds.std(ddof=1)) if matched_ok else None,matched_delta_min=float(ds.min()) if matched_ok else None,
        turnover=turn,annual_cost=float(np.mean([r['full']['annual_cost'] for r in rows])),subsharpe=sub.mean(axis=0).tolist(),sub_min=sub.min(axis=0).tolist(),
        subdelta=np.mean([r['vs_daily']['subdelta'] for r in rows],axis=0).tolist(),matched_subdelta_min=np.min([r['vs_matched']['subdelta'] for r in rows],axis=0).tolist() if matched_ok else None,
        p=p,p_fwer=min(1,p*FAMILY),matched_p=pm,matched_p_fwer=min(1,pm*FAMILY) if matched_ok else None,matched_available=matched_ok,correlation=float(np.mean([r['vs_daily']['correlation'] for r in rows])),
        delta_ci95=np.quantile(shb,[.025,.975]).tolist(),turnover_reduction_ci95=np.quantile(tb,[.025,.975]).tolist(),turnover_p_fwer=min(1,tp*FAMILY))

def main():
    screening=json.loads((HERE/'correlations.json').read_text());factors=[r['factor'] for r in screening if not r['stopped']]
    best=json.loads((HERE/'selection.json').read_text())['selected']
    daily=[];holds=[]
    for f in factors:
        for mode in ['mix','single']:
            key=f'{f}_{mode}_k1_p0';r=getrow(key)
            r['phases']=[getrow(f'{f}_{mode}_k3_p{p}') for p in range(3)]
            r['phase_summary']=aggregate(r['phases'])
            a=r['phase_summary']
            gates=dict(fwer=r['vs_daily']['p_fwer']<.05 and a['matched_p_fwer']<.05,
                subperiods=all(r[label]['sharpe']>0 for label in ['2023-24','2025-26']) and min(r['vs_daily']['subdelta'])>0 and min(a['matched_subdelta_min'])>0,
                phase=a['matched_delta']>a['matched_delta_sd'] and a['matched_delta_min']>0,
                turnover=r['full']['daily_turnover']<1.427)
            r['gates']=gates;r['usable']=all(gates.values());daily.append(r)
            dump(HERE/'daily_analysis.json',daily)
            print('daily',key,round(r['vs_daily']['delta'],3),r['vs_daily']['p_fwer'],flush=True)
    for mode in ['mix','single']:
        for period in [3,5,10]:
            rows=[getrow(f'{best}_{mode}_k{period}_p{p}') for p in range(period)]
            a=aggregate(rows)
            if a.get('invalid'):
                holds.append(dict(factor=best,mode=mode,period=period,phases=rows,summary=a,gates=dict(insolvency=False),usable=False,descriptive_cost_value=False,noninferiority_unadjusted=False))
                dump(HERE/'hold_analysis.json',holds)
                print('hold',mode,period,'INSOLVENT',flush=True)
                continue
            gates=dict(fwer=a['p_fwer']<.05 and a['matched_available'] and a['matched_p_fwer']<.05,
                subperiods=min(a['sub_min'])>0 and min(a['subdelta'])>0 and a['matched_available'] and min(a['matched_subdelta_min'])>0,
                phase=a['delta']>a['delta_sd'] and a['delta_min']>0 and a['matched_available'] and a['matched_delta']>a['matched_delta_sd'] and a['matched_delta_min']>0,
                turnover=a['turnover']<1.427)
            h=dict(factor=best,mode=mode,period=period,phases=rows,summary=a,gates=gates,usable=all(gates.values()),
                descriptive_cost_value=a['delta']>=-.10 and a['turnover']<=float(ref['turn'].mean())*.8,
                noninferiority_unadjusted=a['delta_ci95'][0]>-.10)
            holds.append(h);dump(HERE/'hold_analysis.json',holds)
            print('hold',mode,period,round(a['sharpe'],3),round(a['turnover'],3),a['p_fwer'],flush=True)
    dump(HERE/'statistics_config.json',dict(repetitions=REPS,block=BLOCK,family=FAMILY,seed=SEED,unique_runs=len(list(RUNS.glob('*.json')))))

if __name__=='__main__':main()
