"""验证 B2（只在目标权重变化时交易，不因价格漂移交易）是否真的优于 A。

协议五项:
 1 相位（起始日偏移 0/1/2）稳健性
 2 配对环形移动块 bootstrap 9999 次 · 块长 15/30/60
 3 次日开盘成交 · 费 4.5bp + 滑点 ... 这里用 7bp 总费（与基线一致）
 4 多重检验: 本次共比较 2 个变体（B1/B2）→ Bonferroni ×2
 5 报告两段 Sharpe / 年化 / 换手 / 成本
"""
import json,glob,warnings
import numpy as np, pandas as pd
from pathlib import Path
warnings.filterwarnings('ignore',category=RuntimeWarning)
RET=Path('/Users/wonder/Code/stars/research/trailing_tp_20261008/retrieval')
req=json.load(open(RET/'request.json')); end=req['end_ms']
meta={x['name']:x for x in json.load(open(RET/'meta.json'))['universe']}
files=sorted(glob.glob('/tmp/hl-daily-v2/*.json')); names=[Path(f).stem for f in files]
raw={Path(f).stem:[x for x in json.load(open(f)) if req['daily_start_ms']<=x['t'] and x['T']<end and float(x['c'])>0] for f in files}
ts=sorted({x['t'] for d in raw.values() for x in d}); ti={t:i for i,t in enumerate(ts)}; n=len(ts); k=len(names)
O=np.full((n,k),np.nan); P=np.full((n,k),np.nan); Q=np.full((n,k),np.nan)
for j,c in enumerate(names):
    for x in raw[c]:
        i=ti[x['t']]; O[i,j]=float(x['o']); P[i,j]=float(x['c']); Q[i,j]=float(x['c'])*float(x['v'])
R=P/np.roll(P,1,axis=0)-1
V30=pd.DataFrame(Q).rolling(30,min_periods=5).mean().shift(1).values
SD20=pd.DataFrame(R).rolling(20,min_periods=5).std(ddof=1).values
M14=P/np.roll(P,14,axis=0)-1
NOTCOL=np.array([':' not in c for c in names]); MINV=1e7; CAP=10; TF=0.2; FEE=0.0007; GROSS=2.7; BAND=0.02
def target(i):
    w=np.zeros(k)
    ok=(V30[i-1]>=MINV)&np.isfinite(P[i-1])&np.isfinite(P[i-15])&np.isfinite(SD20[i-1])&NOTCOL
    idx=np.flatnonzero(ok)
    if len(idx)<8: return w
    c2=min(CAP,10); kk=min(max(1,int(np.floor(len(idx)*TF+.5))),c2,len(idx)//2)
    for sc in [M14[i-1]/np.maximum(SD20[i-1],1e-9), Q[i-1]/V30[i-1]]:
        o=idx[np.lexsort((np.array(names)[idx],sc[idx]))]
        w[o[:kk]]-=.5/kk/2; w[o[-kk:]]+=.5/kk/2
    g=np.abs(w).sum()
    return w/g if g>0 else w
def run(mode,start=40):
    eq=1e6; qty=np.zeros(k); last=np.full(k,np.nan); series=[]; turns=[]; prevw=np.zeros(k)
    for i in range(start,n):
        o=O[i]; mo=np.where(np.isfinite(o),o,last)
        eq+=np.nansum(qty*(mo-last))
        if eq<=0: return None
        w=target(i); d=np.zeros(k)
        for j in range(k):
            if not np.isfinite(o[j]): continue
            f=10**meta[names[j]]['szDecimals']
            tgt=np.sign(w[j])*np.floor((abs(w[j])*eq*GROSS/o[j])*f)/f if abs(w[j])>1e-12 else 0.0
            if mode=='A':
                if abs(w[j])<1e-12:
                    if abs(qty[j])*o[j]>=10: d[j]=-qty[j]
                else:
                    dd=tgt-qty[j]
                    if abs(dd)*o[j]>=max(10,abs(tgt)*o[j]*BAND): d[j]=dd
            else:
                if abs(w[j])<1e-12:
                    if abs(qty[j])*o[j]>=10: d[j]=-qty[j]
                elif abs(w[j]-prevw[j])>1e-12:
                    dd=tgt-qty[j]
                    if abs(dd)*o[j]>=max(10,abs(tgt)*o[j]*BAND): d[j]=dd
        ntl=float(np.sum(np.abs(d)*np.nan_to_num(o))); eq-=FEE*ntl
        turns.append(ntl/max(eq,1)); qty+=d; prevw=w.copy()
        last=np.where(np.isfinite(P[i]),P[i],last)
        eq+=np.nansum(qty*(last-mo)); series.append(eq)
    s=np.array(series); return np.diff(s)/s[:-1], np.mean(turns), s
SH=lambda x: float(np.mean(x)/np.std(x,ddof=1)*np.sqrt(365)) if len(x)>2 and np.std(x)>0 else np.nan
print('=== 主检验: A vs B2（起始日 40 ✗）===')
ra,ta,sa=run('A'); rb,tb,sb=run('B2')
La=float(sa[-1]/sa[0]-1); Lb=float(sb[-1]/sb[0]-1)
print(f"  A : Sharpe {SH(ra):.4f}  累计 {La*100:.0f}%  换手 {ta:.3f}  成本 {ta*FEE*365*100:.1f}%/年")
print(f"  B2: Sharpe {SH(rb):.4f}  累计 {Lb*100:.0f}%  换手 {tb:.3f}  成本 {tb*FEE*365*100:.1f}%/年")
d=rb-ra
print(f"  差值 ΔSharpe = {SH(rb)-SH(ra):+.4f} · 日收益差均值 {np.mean(d)*100:+.4f}%/日")
print()
print('=== ① 配对环形移动块 bootstrap（9999 次 ✗）===')
rng=np.random.default_rng(20261017); N=len(ra)
for bl in (15,30,60):
    ds=[]
    for _ in range(9999):
        idx=[]
        while len(idx)<N:
            s0=rng.integers(0,N); idx.extend((np.arange(s0,s0+bl)%N).tolist())
        idx=np.array(idx[:N])
        ds.append(SH(rb[idx])-SH(ra[idx]))
    ds=np.array(ds); lo,hi=np.percentile(ds,[2.5,97.5])
    p=float(np.mean(ds<=0))     # 单边: B2 不优于 A 的概率 ✗
    print(f"  块长 {bl:>2}: ΔSharpe 95%CI [{lo:+.3f}, {hi:+.3f}] · 单边 p = {p:.4f}"
          + ('  ✓ 显著' if p<0.05 else '  ✗ 不显著'))
    if bl==30: p30=p; ci30=(lo,hi)
print(f"  Bonferroni ×2（本次比较了 B1/B2 两个变体 ✗）: p_corrected = {min(1,p30*2):.4f}"
      + ('  ✓ 通过' if p30*2<0.05 else '  ✗ 不通过'))
print()
print('=== ② 子区间 ===')
yrs=pd.to_datetime(ts[41:41+len(ra)],unit='ms').year.values
for lab,m in [('2023-24',yrs<=2024),('2025-26',yrs>=2025),('全期',np.ones(len(ra),bool))]:
    if m.sum()<60: continue
    print(f"  {lab:<8} A {SH(ra[m]):>7.3f}   B2 {SH(rb[m]):>7.3f}   Δ {SH(rb[m])-SH(ra[m]):+.3f}")
print()
print('=== ③ 相位（起始日偏移）稳健性 ===')
for off in (38,39,40,41,42):
    xa,_ ,_= run('A',off); xb,_,_=run('B2',off)
    if xa is None or xb is None: continue
    print(f"  起始第 {off} 天: A {SH(xa):.3f}  B2 {SH(xb):.3f}  Δ {SH(xb)-SH(xa):+.3f}")
