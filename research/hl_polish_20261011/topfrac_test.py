"""确认 cap>=9 无作用，并扫真正的旋钮 top_frac。"""
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

def target(i,eq,cap,tf,facs='nolow'):
    cap=min(cap,max(1,int(np.floor(eq*2.7/15))//4))
    w=np.zeros(k)
    if i<32: return w
    idx=np.flatnonzero((V[i]>=5e6)&np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])&np.array([':' not in c for c in names]))
    if len(idx)<8: return w
    kk=min(max(1,int(np.floor(len(idx)*tf+.5))),cap,len(idx)//2)
    mom=M[i]/np.maximum(SD[i],1e-9); low=-SD[i]; vol=Q[i]/V[i]
    ss={'all':[mom,low,vol],'nolow':[mom,vol]}[facs]
    for s in ss:
        order=idx[np.lexsort((np.array(names)[idx],s[idx]))]
        w[order[:kk]]-=.5/kk/len(ss); w[order[-kk:]]+=.5/kk/len(ss)
    g=np.abs(w).sum()
    return w/g if g>0 else w

def sh(r): return float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365)) if len(r)>1 else None

def run(cap=10,tf=0.2,facs='nolow',fee=.0007):
    eq=1e6; qty=np.zeros(k); last=np.full(k,np.nan); rets=[];turn=[];curve=[];kks=[]
    for i in range(33,n):
        px=a['o'][i]; mark=np.where(np.isfinite(px),px,last)
        gain=np.nansum(qty*(mark-last)); before=eq; eq+=gain
        if eq<=0: raise ValueError('insolvent')
        idx=np.flatnonzero((V[i-1]>=5e6)&np.isfinite(P[i-1])&np.isfinite(P[i-15])&np.isfinite(SD[i-1])&np.array([':' not in c for c in names]))
        if len(idx)>=8:
            c2=min(cap,max(1,int(np.floor(eq*2.7/15))//4))
            kks.append(min(max(1,int(np.floor(len(idx)*tf+.5))),c2,len(idx)//2))
        w=target(i-1,eq,cap,tf,facs)
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
                kk=float(np.mean(kks)) if kks else 0.0)

print("═══ ① 确认 cap>=9 完全无作用（top_frac=0.2 固定）═══")
print(f"{'cap':<6}{'平均kk':>8}{'Sharpe':>9}{'年化':>9}{'最大回撤':>10}{'换手':>8}")
print("-"*52)
for cp in [8,9,10,12,15,20,30,50]:
    o=run(cap=cp,tf=0.2)
    print(f"{cp:<6}{o['kk']:>8.2f}{o['sharpe']:>9.4f}{o['annual']*100:>8.1f}%{o['mdd']*100:>9.1f}%{o['turn']:>8.3f}")

print("\n═══ ② ★ 真正的旋钮: top_frac（cap=15 放开，让它不成为约束）═══")
print(f"{'top_frac':<10}{'平均kk':>8}{'Sharpe':>9}{'年化':>9}{'年成本':>8}{'最大回撤':>10}{'换手':>8}")
print("-"*62)
for tf in [0.20,0.22,0.25,0.27,0.30,0.35,0.40,0.45,0.50]:
    o=run(cap=15,tf=tf)
    print(f"{tf:<10}{o['kk']:>8.2f}{o['sharpe']:>9.4f}{o['annual']*100:>8.1f}%{o['cost']*100:>7.1f}%{o['mdd']*100:>9.1f}%{o['turn']:>8.3f}")

print("\n═══ ③ 同样扫一遍三因子（看低波动在不同广度下的作用）═══")
print(f"{'top_frac':<10}{'平均kk':>8}{'Sharpe':>9}{'年化':>9}{'最大回撤':>10}")
print("-"*48)
for tf in [0.20,0.25,0.27,0.30,0.35]:
    o=run(cap=15,tf=tf,facs='all')
    print(f"{tf:<10}{o['kk']:>8.2f}{o['sharpe']:>9.4f}{o['annual']*100:>8.1f}%{o['mdd']*100:>9.1f}%")
