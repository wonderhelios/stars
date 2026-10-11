"""测 min_order_pct（按净值比例的最小下单额）—— 线上现在设的是 1.7%。
这是唯一一个"线上参数与回测口径不一致"的地方，从没被测过。"""
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

def target(i,eq,cap=10,tf=0.2):
    cap=min(cap,max(1,int(np.floor(eq*2.7/15))//4))
    w=np.zeros(k)
    if i<32: return w
    idx=np.flatnonzero((V[i]>=5e6)&np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])&np.array([':' not in c for c in names]))
    if len(idx)<8: return w
    kk=min(max(1,int(np.floor(len(idx)*tf+.5))),cap,len(idx)//2)
    for s in [M[i]/np.maximum(SD[i],1e-9), Q[i]/V[i]]:
        order=idx[np.lexsort((np.array(names)[idx],s[idx]))]
        w[order[:kk]]-=.5/kk/2; w[order[-kk:]]+=.5/kk/2
    g=np.abs(w).sum()
    return w/g if g>0 else w

def sh(r): return float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365)) if len(r)>1 else None

def run(minpct=0.0,cap=10,tf=0.2,fee=.0007):
    """minpct: 调整门槛 = max($10, 仓位目标×2%, 净值×minpct)。平仓不受影响（只用绝对下限）。"""
    eq=1e6; qty=np.zeros(k); last=np.full(k,np.nan); rets=[];turn=[];curve=[];nadj=[]
    for i in range(33,n):
        px=a['o'][i]; mark=np.where(np.isfinite(px),px,last)
        gain=np.nansum(qty*(mark-last)); before=eq; eq+=gain
        if eq<=0: raise ValueError('insolvent')
        w=target(i-1,eq,cap,tf)
        wanted=np.zeros(k)
        for j,c in enumerate(names):
            if not np.isfinite(px[j]) or abs(w[j])<1e-12: continue
            f=10**meta[c]['szDecimals']; size=np.floor(abs(w[j])*eq*2.7/px[j]*f)/f
            if size*px[j]>=10: wanted[j]=np.sign(w[j])*size
        d=np.zeros(k); na=0
        for j in range(k):
            if not np.isfinite(px[j]): continue
            f=10**meta[names[j]]['szDecimals']; dd=wanted[j]-qty[j]
            if qty[j]*wanted[j]<0: d[j]=dd; na+=1
            elif wanted[j]==0:
                if abs(qty[j])*px[j]>=10: d[j]=-qty[j]; na+=1   # 平仓只用绝对下限
            else:
                thr=max(10.0, abs(wanted[j])*px[j]*0.02, eq*minpct)
                if np.floor(abs(dd)*f)/f*px[j]>=thr:
                    d[j]=np.sign(dd)*np.floor(abs(dd)*f)/f; na+=1
        cost=fee*np.sum(abs(d)*np.nan_to_num(px)); tr=np.sum(abs(d)*np.nan_to_num(px))/eq
        eq-=cost; qty+=d; last=mark; rets.append(eq/before-1); turn.append(tr); curve.append(eq); nadj.append(na)
    rr=np.array(rets); cv=np.array(curve)
    return dict(sharpe=sh(rr),annual=float(rr.mean()*365),turn=float(np.mean(turn)),
                cost=float(np.mean(turn)*fee*365),mdd=float((cv/np.maximum.accumulate(cv)-1).min()),
                orders=float(np.mean(nadj)))

print("═══ min_order_pct 扫描（cap=10，两因子，与线上同口径）═══")
print(f"{'min_order_pct':<15}{'门槛(净值$3000)':>16}{'日均调整笔数':>13}{'Sharpe':>9}{'年化':>9}{'年成本':>8}{'最大回撤':>10}{'换手':>8}")
print("-"*94)
for mp in [0.0,0.005,0.01,0.015,0.017,0.02,0.03,0.05]:
    o=run(minpct=mp)
    thr=3000*mp
    print(f"{mp:<15.3f}{('$'+format(thr,',.0f')) if thr>10 else '$10':>16}{o['orders']:>13.1f}{o['sharpe']:>9.4f}{o['annual']*100:>8.1f}%{o['cost']*100:>7.1f}%{o['mdd']*100:>9.1f}%{o['turn']:>8.3f}")
print()
print("★ 你线上现在是 0.017（门槛约 $52）")
