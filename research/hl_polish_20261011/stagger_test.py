"""错峰调仓在新配置上重测：slices=1/2/3/4/5/6，相位平均。
配置: 两因子(动量÷波动,量冲击) · cap=10 · top_frac=0.2 · 门槛$10M · gross 2.7 · fee 0.07%
执行: 第 i 日用第 i-1 日收盘信号，第 i 日【开盘价】成交 ✗
错峰: 第 N 日只对 (N + hash(coin)) % slices == 0 的币下单 ✓ 其余保持现状 ✗
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
R=P/np.roll(P,1,axis=0)-1; R[0]=np.nan
V30=pd.DataFrame(Q).rolling(30,min_periods=5).mean().shift(1).values
SD20=pd.DataFrame(R).rolling(20,min_periods=5).std(ddof=1).values
M14=P/np.roll(P,14,axis=0)-1
NOTCOL=np.array([':' not in c for c in names]); MINV=1e7; CAP=10; TF=0.2; FEE=0.0007; GROSS=2.7
print(f"宇宙 {k} 个 · {n} 天（{pd.to_datetime(ts[0],unit='ms').date()} → {pd.to_datetime(ts[-1],unit='ms').date()} ✗）")
def _hash(c):
    h=0
    for ch in c: h=(h*131+ord(ch))%1000003
    return h
def target(i):
    w=np.zeros(k)
    ok=(V30[i-1]>=MINV)&np.isfinite(P[i-1])&np.isfinite(P[i-15])&np.isfinite(SD20[i-1])&NOTCOL
    idx=np.flatnonzero(ok)
    if len(idx)<8: return w,0
    c2=min(CAP,10); kk=min(max(1,int(np.floor(len(idx)*TF+.5))),c2,len(idx)//2)
    for sc in [M14[i-1]/np.maximum(SD20[i-1],1e-9), Q[i-1]/V30[i-1]]:
        o=idx[np.lexsort((np.array(names)[idx],sc[idx]))]
        w[o[:kk]]-=.5/kk/2; w[o[-kk:]]+=.5/kk/2
    g=np.abs(w).sum()
    return (w/g if g>0 else w), kk
def run_series(slices, phase):
    eq=1e6; qty=np.zeros(k); last=np.full(k,np.nan); series=[]; turns=[]
    for i in range(40,n):
        o=O[i]; mo=np.where(np.isfinite(o),o,last)
        eq+=np.nansum(qty*(mo-last))
        if eq<=0: return None
        w,kk=target(i)
        dayno=ts[i]//86400000+phase
        inslot=(np.ones(k,bool) if slices<=1 else
                np.array([(dayno+_hash(c))%slices==0 for c in names]))
        wanted=np.zeros(k)
        for j in range(k):
            if not np.isfinite(o[j]) or abs(w[j])<1e-12 or not inslot[j]: continue
            f=10**meta[names[j]]['szDecimals']; sz=np.floor(abs(w[j])*eq*GROSS/o[j]*f)/f
            if sz*o[j]>=10: wanted[j]=np.sign(w[j])*sz
        d=np.zeros(k)
        for j in range(k):
            if not np.isfinite(o[j]) or not inslot[j]: continue
            if abs(w[j])<1e-12:
                if abs(qty[j])*o[j]>=10: d[j]=-qty[j]
                continue
            f=10**meta[names[j]]['szDecimals']; dd=wanted[j]-qty[j]
            if qty[j]*wanted[j]<0: d[j]=dd
            elif np.floor(abs(dd)*f)/f*o[j]>=max(10,abs(wanted[j])*o[j]*.02): d[j]=np.sign(dd)*np.floor(abs(dd)*f)/f
        ntl=float(np.sum(np.abs(d)*np.nan_to_num(o))); eq-=FEE*ntl
        turns.append(ntl/max(eq,1)); qty+=d
        last=np.where(np.isfinite(P[i]),P[i],last)
        eq+=np.nansum(qty*(last-mo))     # ★ 日内（开→收）—— 之前漏了这段
        series.append(eq)
    s=np.array(series); r=np.diff(s)/s[:-1]
    cv=s/s[0]; mdd=float((cv/np.maximum.accumulate(cv)-1).min())
    return dict(sharpe=float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365)),annual=float(np.mean(r)*365),
                mdd=mdd,turn=float(np.mean(turns)),
                cost=float(np.mean(turns)*FEE*365),n=len(r))
base=run_series(1,0)
print(f"\n自检 基线(slices=1): Sharpe {base['sharpe']:.4f}  年化 {base['annual']*100:.1f}%  "
      f"回撤 {base['mdd']*100:.1f}%  换手 {base['turn']:.3f}  成本 {base['cost']*100:.1f}%/年")
print(f"  （期望 ~2.08 Sharpe ✓ 若接近则数据与口径正确 ✗）\n")
print(f"{'slices':<7}{'相位平均':>10}{'各相位范围':>17}{'年化':>9}{'回撤':>9}{'换手':>8}{'成本/年':>9}{'vs基线':>9}")
print("-"*80)
res={1:base}
for s in [2,3,4,5,6]:
    outs=[run_series(s,ph) for ph in range(s)]; outs=[o for o in outs if o]
    if not outs: continue
    res[s]=outs
    sh=[o['sharpe'] for o in outs]
    mlam=float(np.mean([o['annual'] for o in outs])); mmdd=float(np.mean([o['mdd'] for o in outs]))
    mtr=float(np.mean([o['turn'] for o in outs])); mc=float(np.mean([o['cost'] for o in outs]))
    print(f"{s:<7}{np.mean(sh):>10.4f}{f'{min(sh):.2f}~{max(sh):.2f}':>17}{mlam*100:>8.1f}%{mmdd*100:>8.1f}%"
          f"{mtr:>8.3f}{mc*100:>8.1f}%{np.mean(sh)-base['sharpe']:>+9.4f}")
# 子区间（用 slices=3 ✓ 取相位平均 ✓）
print("\n=== 子区间（slices=3 相位平均 ✓）===")
if 3 in res:
    print(f"  基线 slices=1:  Sharpe {base['sharpe']:.3f}  换手 {base['turn']:.3f}")
    print(f"  slices=3:       Sharpe {np.mean([o['sharpe'] for o in res[3]]):.3f}  换手 {np.mean([o['turn'] for o in res[3]]):.3f}")
