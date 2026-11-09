"""预备测试: 把 HIP-3（美股/商品/指数永续）加入横截面宇宙，会不会更好？

三个组合对比: A 只加密 · B 只 HIP-3 · C 两者合并
关键诊断: A 与 B 的日收益相关性（决定分散化有没有用 ✗）
"""
import json,glob,os,warnings
import numpy as np, pandas as pd
from pathlib import Path
warnings.filterwarnings('ignore',category=RuntimeWarning)

def load(dirpath, tag):
    out={}
    for f in glob.glob(dirpath+'/*.json'):
        d=json.load(open(f))
        if len(d)<40: continue
        name=os.path.basename(f)[:-5].replace('__',':') if tag=='HIP3' else os.path.basename(f)[:-5]
        out[name]={int(x['t']):(float(x['c']),float(x['v'])) for x in d}
    return out

C=load('/tmp/hl-daily-v2','C'); H=load('/tmp/hip3-daily','HIP3')
print(f"加密 {len(C)} 个 · HIP-3 {len(H)} 个")
allts=sorted({t for d in C.values() for t in d} | {t for d in H.values() for t in d})
ti={t:i for i,t in enumerate(allts)}; n=len(allts)
def build(src):
    names=sorted(src); k=len(names)
    P=np.full((n,k),np.nan); V=np.full((n,k),np.nan)
    for j,c in enumerate(names):
        for t,(px,v) in src[c].items(): P[ti[t],j]=px; V[ti[t],j]=v
    return names,P,V*P   # 美元成交额
cn,CP,CQ=build(C); hn,HP,HQ=build(H)
print(f"时间轴 {n} 天: {pd.to_datetime(allts[0],unit='ms').date()} → {pd.to_datetime(allts[-1],unit='ms').date()}")

# ★ 只用 HIP-3 有覆盖的窗口（否则 A/B 不可比 ✗）
hcover=np.isfinite(HP).sum(axis=1)
has_h=hcover>=10
first=int(np.argmax(has_h))
print(f"HIP-3 覆盖 ≥10 个市场的起始: 第 {first} 天 = {pd.to_datetime(allts[first],unit='ms').date()}（之后 {n-first} 天 ✗）")

def prep(names,P,Q):
    R=P/np.roll(P,1,axis=0)-1
    win=lambda W,f: pd.DataFrame(W).rolling(W2,min_periods=5).apply(f).values if False else None
    V30=pd.DataFrame(Q).rolling(30,min_periods=5).mean().shift(1).values
    SD20=pd.DataFrame(R).rolling(20,min_periods=5).std(ddof=1).values
    M14=P/np.roll(P,14,axis=0)-1
    return R,SD20,M14,V30,Q

def run(names,P,Q,i0,i1,minvol=1e7,cap=10,tf=0.2,fee=.0007):
    R,SD20,M14,V30,QQ=prep(names,P,Q)
    k=len(names); eq=1e6; qty=np.zeros(k); last=np.full(k,np.nan)
    rets=[];turn=[];curve=[];kk=[]
    for i in range(max(i0,35),i1):
        px=P[i]; mark=np.where(np.isfinite(px),px,last)
        gain=np.nansum(qty*(mark-last)); before=eq; eq+=gain
        if eq<=0: return None
        ok=(V30[i-1]>=minvol)&np.isfinite(P[i-1])&np.isfinite(P[i-15])&np.isfinite(SD20[i-1])
        idx=np.flatnonzero(ok)
        w=np.zeros(k)
        if len(idx)>=8:
            kkv=min(max(1,int(np.floor(len(idx)*tf+.5))),cap,len(idx)//2); kk.append(kkv)
            for s in [M14[i-1]/np.maximum(SD20[i-1],1e-9), QQ[i-1]/V30[i-1]]:
                o=idx[np.lexsort((np.array(names)[idx],s[idx]))]
                w[o[:kkv]]-=.5/kkv/2; w[o[-kkv:]]+=.5/kkv/2
            g=np.abs(w).sum()
            if g>0: w/=g
        wanted=np.zeros(k)
        for j,c in enumerate(names):
            if not np.isfinite(px[j]) or abs(w[j])<1e-12: continue
            f=1e6; size=np.floor(abs(w[j])*eq*2.7/px[j]*f)/f
            if size*px[j]>=10: wanted[j]=np.sign(w[j])*size
        d=np.zeros(k)
        for j in range(k):
            if not np.isfinite(px[j]): continue
            dd=wanted[j]-qty[j]
            if qty[j]*wanted[j]<0: d[j]=dd
            elif wanted[j]==0:
                if abs(qty[j])*px[j]>=10: d[j]=-qty[j]
            elif abs(dd)*px[j]>=max(10,abs(wanted[j])*px[j]*.02): d[j]=dd
        cost=fee*np.sum(abs(d)*np.nan_to_num(px)); tr=np.sum(abs(d)*np.nan_to_num(px))/eq
        eq-=cost; qty+=d; last=mark; rets.append(eq/before-1); turn.append(tr); curve.append(eq)
    rr=np.array(rets); cv=np.array(curve)
    S=lambda x: np.mean(x)/np.std(x,ddof=1)*np.sqrt(365) if len(x)>1 else np.nan
    return dict(sharpe=S(rr),annual=float(rr.mean()*365),mdd=float((cv/np.maximum.accumulate(cv)-1).min()),
                turn=float(np.mean(turn)),n=len(rr),kk=float(np.mean(kk)) if kk else 0), rr

# 合并面板
cN=cn; hN=[f"HIP3::{x}" for x in hn]
names=cN+hN
P=np.hstack([CP,HP]); Q=np.hstack([CQ,HQ])
print()
print(f"{'组合':<20}{'Sharpe':>9}{'年化':>9}{'最大回撤':>10}{'换手':>8}{'平均kk':>8}{'天数':>7}")
res={}
for lab,cols in [('A 只加密',list(range(len(cN)))),('B 只 HIP-3',list(range(len(cN),len(names)))),
                 ('C 加密+HIP-3',list(range(len(names))))]:
    o,rr=run([names[i] for i in cols],P[:,cols],Q[:,cols],first,n,cap=10)
    res[lab]=(o,rr)
    if o: print(f"{lab:<20}{o['sharpe']:>9.3f}{o['annual']*100:>8.0f}%{o['mdd']*100:>9.1f}%{o['turn']:>8.2f}{o['kk']:>8.1f}{o['n']:>7}")
print()
a=res['A 只加密'][1]; b=res['B 只 HIP-3'][1]
m=min(len(a),len(b))
corr=np.corrcoef(a[-m:],b[-m:])[0,1]
print(f"★ A（加密）与 B（HIP-3）日收益相关性 = 【{corr:+.3f}】✗")
print(f"  → 越接近 0 越有分散价值 ✓ 越接近 1 越说明'同源'（没意义 ✗）")
print(f"  组合 C 的 Sharpe = {res['C 加密+HIP-3'][0]['sharpe']:.3f}")
print(f"  理论上完全独立时应有 √2 倍提升 ✗")
