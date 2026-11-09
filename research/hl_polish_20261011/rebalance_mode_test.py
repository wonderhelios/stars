"""测"每天补回目标权重"是否帮忙。

A  每日全量再平衡（现役 ✗）: 每天把每个持仓补/减到目标名义
B1 建仓后不动（纯持有到掉出 ✗）: 只在【进名单】时开仓、【掉出】时平仓
B2 只在权重变化时调（信号驱动 ✗）: 目标权重没变就不因为价格漂移而交易
执行: 第 i-1 日收盘信号 → 第 i 日开盘成交 · fee 0.07% · gross 2.7
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
NOTCOL=np.array([':' not in c for c in names]); MINV=1e7; CAP=10; TF=0.2; FEE=0.0007; GROSS=2.7
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
def run(mode,band=0.02):
    eq=1e6; qty=np.zeros(k); last=np.full(k,np.nan); series=[]; turns=[]
    prevw=np.zeros(k)
    for i in range(40,n):
        o=O[i]; mo=np.where(np.isfinite(o),o,last)
        eq+=np.nansum(qty*(mo-last))
        if eq<=0: return None
        w=target(i)
        want_size=np.zeros(k); d=np.zeros(k)
        for j in range(k):
            if not np.isfinite(o[j]): continue
            f=10**meta[names[j]]['szDecimals']
            tgt=(abs(w[j])*eq*GROSS/o[j]) if abs(w[j])>1e-12 else 0.0
            tgt=np.sign(w[j])*np.floor(tgt*f)/f   # ★ 空头必须是负数量
            if mode=='A':
                if abs(w[j])<1e-12:
                    if abs(qty[j])*o[j]>=10: d[j]=-qty[j]
                else:
                    dd=tgt-qty[j]
                    if abs(dd)*o[j]>=max(10,tgt*o[j]*band): d[j]=dd
            elif mode=='B1':
                was=np.abs(prevw[j])>1e-12; now=abs(w[j])>1e-12
                if not was and now:
                    if tgt*o[j]>=10: d[j]=np.sign(w[j])*tgt
                elif was and not now:
                    if abs(qty[j])*o[j]>=10: d[j]=-qty[j]
            elif mode=='B2':
                chg=abs(w[j]-prevw[j])>1e-12
                if abs(w[j])<1e-12:
                    if abs(qty[j])*o[j]>=10: d[j]=-qty[j]
                elif chg:
                    dd=tgt-qty[j]
                    if abs(dd)*o[j]>=max(10,tgt*o[j]*band): d[j]=dd
        ntl=float(np.sum(np.abs(d)*np.nan_to_num(o))); eq-=FEE*ntl
        turns.append(ntl/max(eq,1)); qty+=d; prevw=w.copy()
        last=np.where(np.isfinite(P[i]),P[i],last)
        eq+=np.nansum(qty*(last-mo))
        series.append(eq)
    s=np.array(series); r=np.diff(s)/s[:-1]
    cv=s/s[0]
    return dict(sharpe=float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365)),annual=float(np.mean(r)*365),
                mdd=float((cv/np.maximum.accumulate(cv)-1).min()),turn=float(np.mean(turns)),
                cost=float(np.mean(turns)*FEE*365),n=len(r),rr=r)
print('=== 权重维护方式对比（2023-06 → 2026-10 ✗）===\n')
print(f"{'模式':<34}{'Sharpe':>9}{'年化':>9}{'回撤':>9}{'换手':>8}{'成本/年':>9}")
print('-'*80)
res={}
for mode,lab in [('A','A 每日全量再平衡（现役 ✗）'),
                 ('B1','B1 建仓后不动（持到掉出 ✗）'),
                 ('B2','B2 只在权重变化时调 ✗')]:
    o=run(mode)
    if not o: print(f'{lab:<34} 失败'); continue
    res[mode]=o
    print(f"{lab:<34}{o['sharpe']:>9.3f}{o['annual']*100:>8.1f}%{o['mdd']*100:>8.1f}%{o['turn']:>8.3f}{o['cost']*100:>8.1f}%")
if 'A' in res:
    print()
    print('=== 相对 A 的差异 ===')
    for m in ('B1','B2'):
        if m in res:
            print(f"  {m}: Sharpe {res[m]['sharpe']-res['A']['sharpe']:+.3f} · "
                  f"年化 {(res[m]['annual']-res['A']['annual'])*100:+.1f}pp · "
                  f"回撤 {(res[m]['mdd']-res['A']['mdd'])*100:+.1f}pp · "
                  f"换手 {res[m]['turn']/res['A']['turn']:.2f}x")
    # 子区间
    if 'B1' in res and 'A' in res:
        print()
        print('=== 子区间 Sharpe ===')
        yrs=pd.to_datetime(ts[41:41+len(res['A']['rr'])],unit='ms').year.values
        for lab,m in [('2023-24',yrs<=2024),('2025-26',yrs>=2025)]:
            print(f"  {lab}: A {np.mean(res['A']['rr'][m])/np.std(res['A']['rr'][m],ddof=1)*np.sqrt(365):.3f}"
                  f"   B1 {np.mean(res['B1']['rr'][m])/np.std(res['B1']['rr'][m],ddof=1)*np.sqrt(365):.3f}"
                  f"   B2 {np.mean(res['B2']['rr'][m])/np.std(res['B2']['rr'][m],ddof=1)*np.sqrt(365):.3f}")
