"""浮动流动性门槛的四种实现。

① 固定 N：每天取成交额最大的前 N 个币（宇宙大小恒定）
② 相对中位数：门槛 = c × 当日全市场中位成交额
③ 相对总量：门槛 = c × 当日全市场总成交额
④ 混合：门槛 = max($5M, c × 中位数)   —— 有底 + 自适应
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
NOTCOL=np.array([':' not in c for c in names])

def universe(i, mode, param):
    """返回当日合格下标。"""
    ok=np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])&np.isfinite(V[i])&NOTCOL
    if mode=='fixed':      return np.flatnonzero(ok & (V[i]>=param))
    if mode=='topn':
        cand=np.flatnonzero(ok)
        if len(cand)<=param: return cand
        return cand[np.argsort(-V[i][cand])[:param]]
    if mode=='median':
        cand=np.flatnonzero(ok)
        if not len(cand): return cand
        return cand[V[i][cand] >= param*np.median(V[i][cand])]
    if mode=='total':
        cand=np.flatnonzero(ok)
        if not len(cand): return cand
        return cand[V[i][cand] >= param*np.nansum(V[i][cand])]
    if mode=='hybrid':
        cand=np.flatnonzero(ok)
        if not len(cand): return cand
        return cand[V[i][cand] >= max(5e6, param*np.median(V[i][cand]))]
    raise ValueError(mode)

def target(i,eq,mode,param,cap=10,tf=0.2):
    cap=min(cap,max(1,int(np.floor(eq*2.7/15))//4))
    w=np.zeros(k)
    if i<32: return w,0
    idx=universe(i,mode,param)
    if len(idx)<8: return w,len(idx)
    kk=min(max(1,int(np.floor(len(idx)*tf+.5))),cap,len(idx)//2)
    for s in [M[i]/np.maximum(SD[i],1e-9), Q[i]/V[i]]:
        order=idx[np.lexsort((np.array(names)[idx],s[idx]))]
        w[order[:kk]]-=.5/kk/2; w[order[-kk:]]+=.5/kk/2
    g=np.abs(w).sum()
    return (w/g if g>0 else w), len(idx)

def sh(r): return float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365)) if len(r)>1 else None

def run(mode,param,fee=.0007):
    eq=1e6; qty=np.zeros(k); last=np.full(k,np.nan)
    rets=[];turn=[];curve=[];uni=[];dates=[]
    for i in range(33,n):
        px=a['o'][i]; mark=np.where(np.isfinite(px),px,last)
        gain=np.nansum(qty*(mark-last)); before=eq; eq+=gain
        if eq<=0: raise ValueError('insolvent')
        w,u=target(i-1,eq,mode,param); uni.append(u)
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
        eq-=cost; qty+=d; last=mark; rets.append(eq/before-1); turn.append(tr); curve.append(eq); dates.append(ts[i])
    rr=np.array(rets); cv=np.array(curve); dd=pd.to_datetime(dates,unit='ms',utc=True)
    out={'sharpe':sh(rr),'annual':float(rr.mean()*365),'turn':float(np.mean(turn)),
         'cost':float(np.mean(turn)*fee*365),'mdd':float((cv/np.maximum.accumulate(cv)-1).min()),
         'uni':float(np.mean(uni)),'cash':int((np.array(uni)<8).sum()),
         'uni23':float(np.mean([u for u,d_ in zip(uni,dd) if d_.year<=2024])),
         'uni25':float(np.mean([u for u,d_ in zip(uni,dd) if d_.year>=2025])),
         's2324':sh(rr[np.asarray(dd.year<=2024)]),'s2526':sh(rr[np.asarray(dd.year>=2025)])}
    return out,rr,dd

print(f"{'方案':<22}{'平均宇宙':>9}{'23-24':>7}{'25-26':>7}{'空仓日':>8}{'Sharpe':>9}{'23-24S':>8}{'25-26S':>8}{'最大回撤':>10}")
print("-"*92)
rows=[('固定 $5M（现在）',      'fixed',5e6),
      ('固定 $10M',             'fixed',1e7),
      ('① 前 30 大',            'topn',30),
      ('① 前 25 大',            'topn',25),
      ('① 前 20 大',            'topn',20),
      ('② 中位 ×2',             'median',2.0),
      ('② 中位 ×3',             'median',3.0),
      ('② 中位 ×5',             'median',5.0),
      ('③ 总量 ×1%',            'total',0.01),
      ('③ 总量 ×2%',            'total',0.02),
      ('④ max(5M, 中位×2)',     'hybrid',2.0),
      ('④ max(5M, 中位×3)',     'hybrid',3.0)]
keep={}
for lab,mode,param in rows:
    o,rr,dd=run(mode,param); keep[lab]=(o,rr,dd)
    print(f"{lab:<22}{o['uni']:>9.1f}{o['uni23']:>7.1f}{o['uni25']:>7.1f}{o['cash']:>8}{o['sharpe']:>9.4f}{o['s2324']:>8.2f}{o['s2526']:>8.2f}{o['mdd']*100:>9.1f}%")
print()
print("★ 自检: 固定 $5M 应复现 Sharpe 1.4529")
import pickle; pickle.dump({l:(v[0],v[1],v[2]) for l,v in keep.items()}, open('/tmp/dyn.pkl','wb'))
