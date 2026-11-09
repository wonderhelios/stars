"""测「掉出名单就平仓」这个退出规则: 那些被剔除的币，后来反弹还是继续跌？"""
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
Q=P*Vv
R=P/np.roll(P,1,axis=0)-1
V=pd.DataFrame(Q).rolling(30,min_periods=5).mean().shift(1).values
SD=pd.DataFrame(R).rolling(20,min_periods=5).std(ddof=1).values
M=P/np.roll(P,14,axis=0)-1
NOTCOL=np.array([':' not in c for c in names])
MINV=5e6

def legs(i,cap=10,tf=0.2):
    """返回 (多头集合, 空头集合)，与现役一致（两因子 ✗）"""
    c2=min(cap,10)
    ok=(V[i]>=MINV)&np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])&NOTCOL
    idx=np.flatnonzero(ok)
    if len(idx)<8: return set(),set()
    kk=min(max(1,int(np.floor(len(idx)*tf+.5))),c2,len(idx)//2)
    long=set(); short=set()
    for s in [M[i]/np.maximum(SD[i],1e-9), Q[i]/V[i]]:
        order=idx[np.lexsort((np.array(names)[idx],s[idx]))]
        short|=set(np.array(names)[order[:kk]])
        long|=set(np.array(names)[order[-kk:]])
    return long,short

def fwd(i,coin,h):
    j=names.index(coin)
    if i+h>=n: return np.nan
    a,b=P[i,j],P[i+h,j]
    if not (np.isfinite(a) and np.isfinite(b) and a>0): return np.nan
    return b/a-1

res={'long_exit':{5:[],10:[],20:[]},'long_stay':{5:[],10:[],20:[]},
     'short_exit':{5:[],10:[],20:[]},'short_stay':{5:[],10:[],20:[]}}
for i in range(35,n-21):
    L0,S0=legs(i-1); L1,S1=legs(i)
    if not L1: continue
    for c in L0-L1:                      # 多头腿掉出去
        for h in (5,10,20): v=fwd(i,c,h); res['long_exit'][h].append(v) if np.isfinite(v) else None
    for c in L1:                          # 留在多头腿
        for h in (5,10,20): v=fwd(i,c,h); res['long_stay'][h].append(v) if np.isfinite(v) else None
    for c in S0-S1:                       # 空头腿掉出去
        for h in (5,10,20): v=fwd(i,c,h); res['short_exit'][h].append(v) if np.isfinite(v) else None
    for c in S1:
        for h in (5,10,20): v=fwd(i,c,h); res['short_stay'][h].append(v) if np.isfinite(v) else None

def st(v):
    a=np.array([x for x in v if np.isfinite(x)])
    if len(a)<30: return None
    return a.mean(), np.median(a), (a>0).mean(), len(a)
print("=== 被剔除 vs 留下 的未来收益（平均 ✗ 中位 ✓ 上涨占比 ✗）===")
print(f"{'组':<26}{'未来5日':>22}{'未来10日':>22}{'未来20日':>22}")
for grp,lab in [('long_exit','多头腿【掉出去】（我们平掉的 ✗）'),
                ('long_stay','多头腿【留下来】（我们保留的 ✓）'),
                ('short_exit','空头腿【掉出去】（我们平掉的 ✗）'),
                ('short_stay','空头腿【留下来】（我们保留的 ✓）')]:
    cells=[]
    for h in (5,10,20):
        s=st(res[grp][h])
        cells.append(f"{s[0]*100:>+6.2f}% / {s[1]*100:>+6.2f}% / {s[2]*100:>3.0f}%" if s else "—")
    print(f"{lab:<26}"+"".join(f"{c:>22}" for c in cells))
print()
print("★ 多头: 如果【掉出去的】后来涨得比【留下的】多 → 我们卖早了 ✗")
a=st(res['long_exit'][10]); b=st(res['long_stay'][10])
if a and b: print(f"  未来10日: 掉出去 {a[0]*100:+.2f}%  vs  留下 {b[0]*100:+.2f}%  →  差 {(a[0]-b[0])*100:+.2f}pp")
print("★ 空头: 如果【掉出去的】后来跌得比【留下的】多 → 我们平早了（空头平仓=买回 ✗）")
a=st(res['short_exit'][10]); b=st(res['short_stay'][10])
if a and b: print(f"  未来10日: 掉出去 {a[0]*100:+.2f}%  vs  留下 {b[0]*100:+.2f}%  →  差 {(a[0]-b[0])*100:+.2f}pp")
