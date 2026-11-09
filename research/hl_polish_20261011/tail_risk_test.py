"""单币尾部风险的各种限制（现役: 两因子 + cap=10 + minvol=10M）。

A 单币权重上限
B 空头只用更高流动性的币（逼空只发生在薄币上 ✗）
C 空头单币权重单独收紧（非对称 ✗）
D 空头额外加"成交额下限"（$30M ✗）
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
P=np.full((n,k),np.nan); Vv=np.full((n,k),np.nan)
for j,c in enumerate(names):
    for x in raw[c]:
        i=ti[x['t']]; P[i,j]=float(x['c']); Vv[i,j]=float(x['v'])
Q=P*Vv; R=P/np.roll(P,1,axis=0)-1; R[0]=np.nan
V=pd.DataFrame(Q).rolling(30,min_periods=5).mean().shift(1).values
SD=pd.DataFrame(R).rolling(20,min_periods=5).std(ddof=1).values
M=P/np.roll(P,14,axis=0)-1
meta={x['name']:x for x in json.load(open(RET/'meta.json'))['universe']}
NOTCOL=np.array([':' not in c for c in names])

def cap_weights(w,capL,capS,coin_is_long):
    """对多空分别设权重上限，用迭代 water-filling 保证严格生效。"""
    a=np.abs(w).copy(); s0=a.sum()
    if s0<=0: return w
    a/=s0
    for side in (True,False):
        m=(coin_is_long==side)&(a>0)
        lim = capL if side else capS
        if lim is None or not m.any(): continue
        fixed=np.zeros(len(a),bool)
        for _ in range(200):
            free=m&~fixed
            if not free.any(): break
            budget=(1.0-a[~m].sum()-a[fixed].sum())
            s=a[free].sum()
            if s<=0: break
            a[free]=a[free]/s*budget
            viol=free&(a>lim*(1+1e-12))
            if not viol.any(): break
            a[viol]=lim; fixed|=viol
    return np.sign(w)*a

def target(i,eq,minvolL,minvolS,capL,capS,cap=10,tf=0.2):
    c=min(cap,10)
    w=np.zeros(k)
    if i<32: return w,0
    okL=(V[i]>=minvolL)&np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])&NOTCOL
    okS=(V[i]>=minvolS)&okL
    mom=M[i]/np.maximum(SD[i],1e-9); vol=Q[i]/V[i]
    islong=np.zeros(k,bool)
    for s in [mom,vol]:
        idxL=np.flatnonzero(okL); idxS=np.flatnonzero(okS)
        if len(idxL)<8 or len(idxS)<8: continue
        kkL=min(max(1,int(np.floor(len(idxL)*tf+.5))),c,len(idxL)//2)
        kkS=min(max(1,int(np.floor(len(idxS)*tf+.5))),c,len(idxS)//2)
        oL=idxL[np.lexsort((np.array(names)[idxL],s[idxL]))]
        oS=idxS[np.lexsort((np.array(names)[idxS],s[idxS]))]
        for j in oL[-kkL:]: w[j]+=.5/kkL/2; islong[j]=True
        for j in oS[:kkS]:  w[j]-=.5/kkS/2   # 空头
    g=np.abs(w).sum()
    w=w/g if g>0 else w
    islong = islong & (w>0)
    if capL is not None or capS is not None:
        w=cap_weights(w,capL,capS,islong)
    return w, len(np.flatnonzero(okL))

def sh(r): return float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365)) if len(r)>1 else None

def run(minvolL=1e7,minvolS=1e7,capL=None,capS=None,fee=.0007):
    eq=1e6; qty=np.zeros(k); last=np.full(k,np.nan)
    rets=[];turn=[];curve=[]
    for i in range(33,n):
        px=a['o'][i] if False else None
        px=P[i]  # 用收盘价简化（相对比较用 ✗）
        mark=np.where(np.isfinite(px),px,last)
        gain=np.nansum(qty*(mark-last)); before=eq; eq+=gain
        if eq<=0: raise ValueError('insolvent')
        w,_=target(i-1,eq,minvolL,minvolS,capL,capS)
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
    worst=np.sort(rr)[:5]
    return dict(sharpe=sh(rr),annual=float(rr.mean()*365),turn=float(np.mean(turn)),
                cost=float(np.mean(turn)*fee*365),mdd=float((cv/np.maximum.accumulate(cv)-1).min()),
                w1=float(worst[0]),w5=float(worst[:5].mean()),
                d10=int((rr<=-0.10).sum()),d15=int((rr<=-0.15).sum()))

print("注: 本表用【收盘价】成交（简化 ✗）所以绝对数字与之前不可比 ✓ 只看方案之间的相对差异 ✗\n")
print(f"{'方案':<34}{'Sharpe':>8}{'年化':>8}{'回撤':>9}{'最差1日':>9}{'最差5日均':>10}{'>10%日':>8}{'>15%日':>8}")
print("-"*96)
CASES=[('基线（现役 ✗）',            dict()),
       ('A 单币上限 0.06',            dict(capL=0.06,capS=0.06)),
       ('A 单币上限 0.05',            dict(capL=0.05,capS=0.05)),
       ('B 空头门槛 $30M（多头$10M ✗）', dict(minvolS=3e7)),
       ('B 空头门槛 $50M',            dict(minvolS=5e7)),
       ('C 空头上限 0.05（多头不限 ✗）', dict(capS=0.05)),
       ('C 空头上限 0.04',            dict(capS=0.04)),
       ('D 空头 $30M + 上限 0.05',     dict(minvolS=3e7,capS=0.05)),
       ('D 空头 $50M + 上限 0.04',     dict(minvolS=5e7,capS=0.04))]
for lab,kw in CASES:
    o=run(**kw)
    print(f"{lab:<34}{o['sharpe']:>8.3f}{o['annual']*100:>7.0f}%{o['mdd']*100:>8.1f}%{o['w1']*100:>8.1f}%{o['w5']*100:>9.1f}%{o['d10']:>8}{o['d15']:>8}")
