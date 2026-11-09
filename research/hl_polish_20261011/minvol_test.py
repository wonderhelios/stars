"""收严流动性门槛（min_vol_usd）—— 现役配置: 两因子 + cap=10 + top_frac 0.2。"""
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

def target(i,eq,cap,tf,minvol):
    cap=min(cap,max(1,int(np.floor(eq*2.7/15))//4))
    w=np.zeros(k)
    if i<32: return w
    idx=np.flatnonzero((V[i]>=minvol)&np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])&np.array([':' not in c for c in names]))
    if len(idx)<8: return w
    kk=min(max(1,int(np.floor(len(idx)*tf+.5))),cap,len(idx)//2)
    for s in [M[i]/np.maximum(SD[i],1e-9), Q[i]/V[i]]:
        order=idx[np.lexsort((np.array(names)[idx],s[idx]))]
        w[order[:kk]]-=.5/kk/2; w[order[-kk:]]+=.5/kk/2
    g=np.abs(w).sum()
    return w/g if g>0 else w

def sh(r): return float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365)) if len(r)>1 else None

def run(minvol,cap=10,tf=0.2,fee=.0007):
    eq=1e6; qty=np.zeros(k); last=np.full(k,np.nan)
    rets=[];turn=[];curve=[];uni=[];pos=[];cash=0
    for i in range(33,n):
        px=a['o'][i]; mark=np.where(np.isfinite(px),px,last)
        gain=np.nansum(qty*(mark-last)); before=eq; eq+=gain
        if eq<=0: raise ValueError('insolvent')
        idx=np.flatnonzero((V[i-1]>=minvol)&np.isfinite(P[i-1])&np.isfinite(P[i-15])&np.isfinite(SD[i-1])&np.array([':' not in c for c in names]))
        uni.append(len(idx))
        if len(idx)<8: cash+=1
        w=target(i-1,eq,cap,tf,minvol)
        pos.append(int((np.abs(w)>1e-12).sum()))
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
                uni=float(np.mean(uni)),pos=float(np.mean(pos)),cash=cash)

print("═══ 收严流动性门槛（现役: 两因子 + cap=10）═══")
print(f"{'门槛':<10}{'平均宇宙':>9}{'平均持仓':>9}{'空仓日':>8}{'Sharpe':>9}{'年化':>9}{'年成本':>8}{'最大回撤':>10}")
print("-"*74)
for mv,lab in [(5e6,'$5M(现在)'),(7.5e6,'$7.5M'),(1e7,'$10M'),(1.5e7,'$15M'),(2e7,'$20M'),(3e7,'$30M')]:
    o=run(mv)
    print(f"{lab:<10}{o['uni']:>9.1f}{o['pos']:>9.1f}{o['cash']:>8}{o['sharpe']:>9.4f}{o['annual']*100:>8.1f}%{o['cost']*100:>7.1f}%{o['mdd']*100:>9.1f}%")
print()
print("★ 自检: $5M 应复现 Sharpe 1.4529（cap=10 两因子）")

# ===== 显著性 + 分段 =====
print()
print("═══ 分段年化 Sharpe（看改善来自哪一段）═══")
import datetime as _dt
def run_full(minvol):
    eq=1e6; qty=np.zeros(k); last=np.full(k,np.nan); rets=[]; dates=[]
    for i in range(33,n):
        px=a['o'][i]; mark=np.where(np.isfinite(px),px,last)
        gain=np.nansum(qty*(mark-last)); before=eq; eq+=gain
        if eq<=0: raise ValueError('insolvent')
        w=target(i-1,eq,10,0.2,minvol)
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
        cost=.0007*np.sum(abs(d)*np.nan_to_num(px)); tr=np.sum(abs(d)*np.nan_to_num(px))/eq
        eq-=cost; qty+=d; last=mark; rets.append(eq/before-1); dates.append(ts[i])
    return np.array(rets), pd.to_datetime(dates,unit='ms',utc=True)

r5,d5=run_full(5e6); r10,d10=run_full(1e7)
for lab,mask in [('全期',np.ones(len(r5),bool)),('2023-24',d5.year<=2024),('2025-26',d5.year>=2025)]:
    print(f"  {lab:<10} $5M {sh(r5[mask]):>7.3f}   $10M {sh(r10[mask]):>7.3f}   Δ {sh(r10[mask])-sh(r5[mask]):>+7.3f}")
print()
print("═══ 配对块 bootstrap（H0: ΔSharpe ≤ 0）═══")
rng=np.random.default_rng(20261012)
def S(r): return np.mean(r)/np.std(r,ddof=1)*np.sqrt(365)
obs=S(r10)-S(r5); nn=len(r5)
print(f"  ΔSharpe = {obs:+.4f}")
for bl in (10,20,40):
    ds=[]
    for _ in range(9999):
        idx=[]
        while len(idx)<nn:
            s=rng.integers(0,nn); idx.extend((np.arange(s,s+bl)%nn).tolist())
        idx=np.array(idx[:nn]); ds.append(S(r10[idx])-S(r5[idx]))
    ds=np.array(ds)
    print(f"  块长 {bl:>3}: SE {ds.std(ddof=1):.4f}   p(单边) {(ds<=0).mean():.4f}")
