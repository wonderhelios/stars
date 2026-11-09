"""测试: 因子分与未来收益是否单调？极端桶会不会反转？
（回答"能不能不用人工通知解锁，只靠价格/量数据规避暴涨尾部" ✗）"""
import json,glob,warnings
import numpy as np, pandas as pd
from pathlib import Path
warnings.filterwarnings('ignore',category=RuntimeWarning)
RET=Path('/Users/wonder/Code/stars/research/trailing_tp_20261008/retrieval')
req=json.load(open(RET/'request.json')); end=req['end_ms']
files=sorted(glob.glob('/tmp/hl-daily-v2/*.json')); names=[Path(f).stem for f in files]
raw={Path(f).stem:[x for x in json.load(open(f)) if req['daily_start_ms']<=x['t'] and x['T']<end and float(x['c'])>0] for f in files}
ts=sorted({x['t'] for d in raw.values() for x in d}); ti={t:i for i,t in enumerate(ts)}; n=len(ts); k=len(names)
P=np.full((n,k),np.nan); Q=np.full((n,k),np.nan)
for j,c in enumerate(names):
    for x in raw[c]:
        i=ti[x['t']]; P[i,j]=float(x['c']); Q[i,j]=float(x['c'])*float(x['v'])
R=P/np.roll(P,1,axis=0)-1
V30=pd.DataFrame(Q).rolling(30,min_periods=5).mean().shift(1).values
SD20=pd.DataFrame(R).rolling(20,min_periods=5).std(ddof=1).values
M14=P/np.roll(P,14,axis=0)-1
NOTCOL=np.array([':' not in c for c in names]); MINV=1e7
MOM=M14/np.maximum(SD20,1e-9); VSH=Q/np.maximum(V30,1e-9)
def fwd(i,j,h):
    if i+h>=n: return np.nan
    a,b=P[i,j],P[i+h,j]
    return b/a-1 if (np.isfinite(a) and np.isfinite(b) and a>0) else np.nan
print(f"宇宙 {k} · 天数 {n} · 流动性门槛 $10M ✗\n")
for fname,F in [('动量÷波动',MOM),('量冲击',VSH)]:
    # 建立 (i,j,forward) 样本
    recs=[]
    for i in range(300,n-21,3):     # 每 3 天取一次，降低重叠 ✗
        ok=(V30[i]>=MINV)&np.isfinite(P[i])&np.isfinite(SD20[i])&np.isfinite(F[i])&NOTCOL
        idx=np.flatnonzero(ok)
        if len(idx)<15: continue
        s=F[i][idx]
        q=np.argsort(np.argsort(s))/ (len(idx)-1)   # 分位 0~1 ✗
        for r,(j,qq) in enumerate(zip(idx,q)):
            recs.append((qq, fwd(i,j,5), fwd(i,j,10), fwd(i,j,20)))
    a=np.array([[x[0],x[1],x[2],x[3]] for x in recs],dtype=float)
    print(f"═══ {fname}（样本 {len(a):,} ✗）═══")
    print(f"{'分位桶':<12}{'样本':>8}{'未来5日':>11}{'未来10日':>11}{'未来20日':>11}")
    for lo,hi,lab in [(0,.1,'最低 10%'),(.1,.3,'10-30%'),(.3,.7,'中间 40%'),(.7,.9,'70-90%'),(.9,1.01,'最高 10%')]:
        m=(a[:,0]>=lo)&(a[:,0]<hi)
        if m.sum()<50: continue
        print(f"{lab:<12}{int(m.sum()):>8}{np.nanmean(a[m,1])*100:>10.2f}%{np.nanmean(a[m,2])*100:>10.2f}%{np.nanmean(a[m,3])*100:>10.2f}%")
    # 极端分位（越高越好 → 做多）
    top10=a[a[:,0]>=.9]; top1=a[a[:,0]>=.99]
    print(f"  【最高 1%（暴涨币 ✗）】样本 {len(top1)} · 未来10日 {np.nanmean(top1[:,2])*100:+.2f}%")
    print(f"  【最高 10%】未来10日 {np.nanmean(top10[:,2])*100:+.2f}%")
    print(f"  【90-95%】未来10日 {np.nanmean(a[(a[:,0]>=.90)&(a[:,0]<.95),2])*100:+.2f}%")
    print(f"  【95-99%】未来10日 {np.nanmean(a[(a[:,0]>=.95)&(a[:,0]<.99),2])*100:+.2f}%")
    print()
