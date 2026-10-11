"""在 cap=8（线上参数）上重跑之前的实验 + cap 上扫。"""
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

def apply_cap(w,cap):
    aa=np.abs(w); s0=aa.sum()
    if cap is None or s0<=0: return w
    aa=aa/s0; fixed=np.zeros(len(aa),bool)
    for _ in range(200):
        free=~fixed
        if not free.any(): break
        budget=1.0-aa[fixed].sum(); s=aa[free].sum()
        if s<=0: break
        aa[free]=aa[free]/s*budget
        viol=free&(aa>cap*(1+1e-12))
        if not viol.any(): break
        aa[viol]=cap; fixed|=viol
    assert not (aa>cap*(1+1e-9)).any(), '上限未生效'
    return np.sign(w)*aa

def target(i,eq,cap,facs,normalize=True,wcap=None,fixed_kk=None):
    cap=min(cap,max(1,int(np.floor(eq*2.7/15))//4))
    w=np.zeros(k)
    if i<32: return w
    idx=np.flatnonzero((V[i]>=5e6)&np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])&np.array([':' not in c for c in names]))
    if len(idx)<8: return w
    kk=min(fixed_kk,len(idx)//2) if fixed_kk else min(max(1,int(np.floor(len(idx)*.2+.5))),cap,len(idx)//2)
    mom=M[i]/np.maximum(SD[i],1e-9); low=-SD[i]; vol=Q[i]/V[i]
    ss={'all':[mom,low,vol],'nolow':[mom,vol]}[facs]
    for s in ss:
        order=idx[np.lexsort((np.array(names)[idx],s[idx]))]
        w[order[:kk]]-=.5/kk/len(ss); w[order[-kk:]]+=.5/kk/len(ss)
    if normalize:
        g=np.abs(w).sum()
        if g>0: w=w/g
    return apply_cap(w,wcap)

def sh(r): return float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365)) if len(r)>1 else None

def run(cap=5,facs='all',normalize=True,wcap=None,fixed_kk=None,fee=.0007):
    prices=a['o']; eq=1e6; qty=np.zeros(k); last=np.full(k,np.nan)
    rets=[];turn=[];curve=[];gross=[]
    for i in range(33,n):
        px=prices[i]; mark=np.where(np.isfinite(px),px,last)
        gain=np.nansum(qty*(mark-last)); before=eq; eq+=gain
        if eq<=0: raise ValueError('insolvent')
        w=target(i-1,eq,cap,facs,normalize,wcap,fixed_kk)
        gross.append(np.abs(w).sum())
        wanted=np.zeros(k)
        for j,c in enumerate(names):
            if not np.isfinite(px[j]) or abs(w[j])<1e-12: continue
            f=10**meta[c]['szDecimals']; size=np.floor(abs(w[j])*eq*2.7/px[j]*f)/f
            if size*px[j]>=10: wanted[j]=np.sign(w[j])*size
        d=np.zeros(k)
        for j in range(k):
            if not np.isfinite(px[j]): continue
            f=10**meta[names[j]]['szDecimals']; dd=wanted[j]-qty[j]
            if qty[j]*wanted[j]<0: d[j]=dd
            elif wanted[j]==0:
                if abs(qty[j])*px[j]>=10: d[j]=-qty[j]
            elif np.floor(abs(dd)*f)/f*px[j]>=max(10,abs(wanted[j])*px[j]*.02):
                d[j]=np.sign(dd)*np.floor(abs(dd)*f)/f
        cost=fee*np.sum(abs(d)*np.nan_to_num(px)); tr=np.sum(abs(d)*np.nan_to_num(px))/eq
        eq-=cost; qty+=d; last=mark; rets.append(eq/before-1); turn.append(tr); curve.append(eq)
    rr=np.array(rets); cv=np.array(curve)
    return dict(sharpe=sh(rr),annual=float(rr.mean()*365),turn=float(np.mean(turn)),
                cost=float(np.mean(turn)*fee*365),mdd=float((cv/np.maximum.accumulate(cv)-1).min()),
                gross=float(np.mean(gross)))

print("═══ ① cap 上扫（现役两因子策略）═══")
print(f"{'cap':<6}{'Sharpe':>9}{'年化':>9}{'换手':>8}{'年成本':>8}{'最大回撤':>10}{'平均gross':>11}")
print("-"*64)
base=None
for cp in [5,8,10,12,15,20,25,30]:
    o=run(cap=cp,facs='nolow')
    if cp==5: base=o
    print(f"{cp:<6}{o['sharpe']:>9.4f}{o['annual']*100:>8.1f}%{o['turn']:>8.3f}{o['cost']*100:>7.1f}%{o['mdd']*100:>9.1f}%{o['gross']:>11.4f}")
print(f"  （自检: cap5 两因子 Sharpe = {base['sharpe']:.4f}，应为 1.2194）")

print("\n═══ ② 之前的实验在 cap=8 上重跑 ═══")
print(f"{'实验':<34}{'Sharpe':>9}{'年化':>9}{'最大回撤':>10}{'换手':>8}")
print("-"*72)
rows=[
 ('三因子（原始基线）',        dict(cap=8,facs='all')),
 ('两因子（删低波动，现役）',   dict(cap=8,facs='nolow')),
 ('两因子 + 不归一化(A实验)',   dict(cap=8,facs='nolow',normalize=False)),
 ('K12 固定每腿12币（三因子）', dict(cap=8,facs='all',fixed_kk=12)),
 ('K12 固定每腿12币（两因子）', dict(cap=8,facs='nolow',fixed_kk=12)),
 ('两因子 + 单币上限 0.06',     dict(cap=8,facs='nolow',wcap=0.06)),
 ('两因子 + 单币上限 0.05',     dict(cap=8,facs='nolow',wcap=0.05)),
]
for lbl,kw in rows:
    o=run(**kw)
    print(f"{lbl:<34}{o['sharpe']:>9.4f}{o['annual']*100:>8.1f}%{o['mdd']*100:>9.1f}%{o['turn']:>8.3f}")

print("\n═══ ③ 对照: 同样几个实验在 cap=5（旧口径）═══")
print(f"{'实验':<34}{'Sharpe':>9}{'年化':>9}{'最大回撤':>10}{'换手':>8}")
print("-"*72)
for lbl,kw in [('三因子',dict(cap=5,facs='all')),('两因子',dict(cap=5,facs='nolow')),
               ('K12（三因子）',dict(cap=5,facs='all',fixed_kk=12))]:
    o=run(**kw)
    print(f"{lbl:<34}{o['sharpe']:>9.4f}{o['annual']*100:>8.1f}%{o['mdd']*100:>9.1f}%{o['turn']:>8.3f}")
