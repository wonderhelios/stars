"""单币权重上限（正确实现）。

关键：截断后【不再】强行归一化到 1（那会把上限推回去）。
用迭代 water-filling：超限的钉在上限，剩余预算只在【未超限】的币之间按比例分配，
直到没人超限。如果所有币都被钉住，gross 就低于 1 —— 这是上限的诚实代价。
每个组合都【断言并回报】实际最大权重，证明上限真的生效。
"""
import json,glob,warnings
import numpy as np, pandas as pd
from pathlib import Path
warnings.filterwarnings('ignore',category=RuntimeWarning)
RET=Path('/Users/wonder/Code/stars/research/trailing_tp_20261008/retrieval')
req=json.load(open(RET/'request.json')); end=req['end_ms']
files=sorted(glob.glob('/tmp/hl-daily-v2/*.json')); names=[Path(f).stem for f in files]
raw={Path(f).stem:[x for x in json.load(open(f)) if req['daily_start_ms']<=x['t'] and x['T']<end and float(x['c'])>0] for f in files}
ts=sorted({x['t'] for d in raw.values() for x in d}); ti={t:i for i,t in enumerate(ts)}; n=len(ts); k=len(names)
a={z:np.full((n,k),np.nan) for z in ['o','c','v']}
for j,c in enumerate(names):
    for x in raw[c]:
        for z in a: a[z][ti[x['t']],j]=float(x[z])
P=a['c']; Q=P*a['v']; R=P/np.roll(P,1,axis=0)-1; R[0]=np.nan
V=pd.DataFrame(Q).rolling(30,min_periods=5).mean().shift(1).values
SD=pd.DataFrame(R).rolling(20,min_periods=5).std(ddof=1).values
M=P/np.roll(P,14,axis=0)-1
meta={x['name']:x for x in json.load(open(RET/'meta.json'))['universe']}

def apply_cap(w, cap):
    """迭代 water-filling：严格保证 max|w| <= cap。返回 (w_new, violated_flag)。"""
    a=np.abs(w); s0=a.sum()
    if cap is None or s0<=0: return w, False
    a=a/s0
    fixed=np.zeros(len(a),bool)
    for _ in range(200):
        free=~fixed
        if not free.any(): break
        budget=1.0-a[fixed].sum()
        s=a[free].sum()
        if s<=0: break
        a[free]=a[free]/s*budget
        viol=free&(a>cap*(1+1e-12))
        if not viol.any(): break
        a[viol]=cap; fixed|=viol
    bad=bool((a>cap*(1+1e-9)).any())
    return np.sign(w)*a, bad

def target(i,equity,cap=5,wcap=None):
    cap=min(cap,max(1,int(np.floor(equity*2.7/15))//4))
    w=np.zeros(k)
    if i<32: return w
    idx=np.flatnonzero((V[i]>=5e6)&np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])&np.array([':' not in c for c in names]))
    if len(idx)<8: return w
    kk=min(max(1,int(np.floor(len(idx)*.2+.5))),cap,len(idx)//2)
    for s in [M[i]/np.maximum(SD[i],1e-9), Q[i]/V[i]]:   # 现役：两因子
        order=idx[np.lexsort((np.array(names)[idx],s[idx]))]
        w[order[:kk]]-=.5/kk/2; w[order[-kk:]]+=.5/kk/2
    g=np.abs(w).sum()
    w=w/g if g>0 else w
    w,bad=apply_cap(w,wcap)
    if bad: raise AssertionError('上限未生效！')
    return w

def sh(r): return float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365)) if len(r)>1 else None

def run(wcap=None,cap=5,clock='open',fee=.0007):
    prices=a['o'] if clock=='open' else P
    eq=1e6; qty=np.zeros(k); last=np.full(k,np.nan); rets=[]; turn=[]; curve=[]; maxw=[]; gross=[]
    for i in range(33,n):
        px=prices[i]; mark=np.where(np.isfinite(px),px,last)
        gain=np.nansum(qty*(mark-last)); before=eq; eq+=gain   # before 必须在加收益之前
        if eq<=0: raise ValueError('insolvent')
        w=target(i-1 if clock=='open' else i,eq,cap,wcap)
        maxw.append(np.abs(w).max() if np.abs(w).max()>0 else 0.0); gross.append(np.abs(w).sum())
        wanted=np.zeros(k)
        for j,c in enumerate(names):
            if not np.isfinite(px[j]) or abs(w[j])<1e-12: continue
            f=10**meta[c]['szDecimals']; size=np.floor(abs(w[j])*eq*2.7/px[j]*f)/f
            if size*px[j]>=10: wanted[j]=np.sign(w[j])*size
        delta=np.zeros(k)
        for j in range(k):
            if not np.isfinite(px[j]): continue
            f=10**meta[names[j]]['szDecimals']; d=wanted[j]-qty[j]
            if qty[j]*wanted[j]<0: delta[j]=d
            elif wanted[j]==0:
                if abs(qty[j])*px[j]>=10: delta[j]=-qty[j]
            elif np.floor(abs(d)*f)/f*px[j]>=max(10,abs(wanted[j])*px[j]*.02):
                delta[j]=np.sign(d)*np.floor(abs(d)*f)/f
        cost=fee*np.sum(abs(delta)*np.nan_to_num(px)); tr=np.sum(abs(delta)*np.nan_to_num(px))/eq
        eq-=cost; qty+=delta; last=mark; rets.append(eq/before-1); turn.append(tr); curve.append(eq)
    rr=np.array(rets); cv=np.array(curve)
    return dict(sharpe=sh(rr),annual=float(rr.mean()*365),turnover=float(np.mean(turn)),
                cost=float(np.mean(turn)*fee*365),mdd=float((cv/np.maximum.accumulate(cv)-1).min()),
                ending=float(eq),maxw=float(np.mean(maxw)),gross=float(np.mean(gross))), rr

for CAP in (5,8):
    print(f"\n═══ 回测参数 cap={CAP}（你线上是 target_positions=8）═══")
    print(f"{'单币上限':<12}{'Sharpe':>9}{'年化':>9}{'换手':>8}{'年成本':>8}{'最大回撤':>10}{'实际最大权重':>13}{'平均gross':>11}")
    print("-"*84)
    res={}
    for wc,label in [(None,'无（现在）'),(0.09,'0.09'),(0.08,'0.08'),(0.07,'0.07'),(0.06,'0.06'),(0.05,'0.05'),(0.04,'0.04')]:
        o=run(wc,CAP)[0]; res[label]=o
        flag='' if wc is None else (' ✓上限生效' if o['maxw']<=wc*1.001 else ' ✗上限失效!')
        print(f"{label:<12}{o['sharpe']:>9.4f}{o['annual']*100:>8.1f}%{o['turnover']:>8.3f}{o['cost']*100:>7.1f}%{o['mdd']*100:>9.1f}%{o['maxw']:>13.4f}{o['gross']:>11.4f}{flag}")
    base=res['无（现在）']['sharpe']
    print(f"  自检: 无上限 Sharpe = {base:.4f}（cap5 应为 1.2194）")
