"""只改因子组合，其他一切不动。自检：all 必须复现 Sharpe 1.1293。"""
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

def factors(i,which):
    mom=M[i]/np.maximum(SD[i],1e-9); low=-SD[i]; vol=Q[i]/V[i]
    return {'all':[mom,low,vol],'nolow':[mom,vol],'revlow':[mom,-low,vol],
            'onlymom':[mom],'onlyvol':[vol],'onlylow':[low]}[which]

def target(i,equity,which,cap=5):
    cap=min(cap,max(1,int(np.floor(equity*2.7/15))//4))
    w=np.zeros(k)
    if i<32: return w
    idx=np.flatnonzero((V[i]>=5e6)&np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])&np.array([':' not in c for c in names]))
    if len(idx)<8: return w
    kk=min(max(1,int(np.floor(len(idx)*.2+.5))),cap,len(idx)//2)
    ss=factors(i,which); nf=len(ss)
    for s in ss:
        order=idx[np.lexsort((np.array(names)[idx],s[idx]))]
        w[order[:kk]]-=.5/kk/nf; w[order[-kk:]]+=.5/kk/nf
    g=np.abs(w).sum()
    return w/g if g>0 else w

def sh(r): return float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365)) if len(r)>1 else None

def run(which,cap=5,clock='open',fee=.0007):
    prices=a['o'] if clock=='open' else P
    eq=1e6; qty=np.zeros(k); last=np.full(k,np.nan); rets=[]; turn=[]; dates=[]; curve=[]
    for i in range(33,n):
        px=prices[i]; mark=np.where(np.isfinite(px),px,last)
        gain=np.nansum(qty*(mark-last)); before=eq; eq+=gain   # before 必须在加收益【之前】
        if eq<=0: raise ValueError('insolvent')
        w=target(i-1 if clock=='open' else i,eq,which,cap)
        wanted=np.zeros(k)
        for j,c in enumerate(names):
            if not np.isfinite(px[j]) or abs(w[j])<1e-12: continue
            f=10**meta[c]['szDecimals']; size=np.floor(abs(w[j])*eq*2.7/px[j]*f)/f
            if size*px[j]>=10: wanted[j]=np.sign(w[j])*size
        delta=np.zeros(k)
        for j in range(k):
            if not np.isfinite(px[j]): continue
            f=10**meta[names[j]]['szDecimals']; d=wanted[j]-qty[j]
            if qty[j]*wanted[j]<0: delta[j]=d
            elif wanted[j]==0:
                if abs(qty[j])*px[j]>=10: delta[j]=-qty[j]
            elif np.floor(abs(d)*f)/f*px[j]>=max(10,abs(wanted[j])*px[j]*.02):
                delta[j]=np.sign(d)*np.floor(abs(d)*f)/f
        cost=fee*np.sum(abs(delta)*np.nan_to_num(px)); tr=np.sum(abs(delta)*np.nan_to_num(px))/eq
        eq-=cost; qty+=delta; last=mark; rets.append(eq/before-1); turn.append(tr); dates.append(ts[i]); curve.append(eq)
    rr=np.array(rets); dd=pd.to_datetime(dates,unit='ms',utc=True); cv=np.array(curve)
    out={}
    for label,mask in [('full',np.ones(len(rr),bool)),('2023-24',dd.year<=2024),('2025-26',dd.year>=2025)]:
        r=rr[mask]; t=np.array(turn)[mask]
        out[label]=dict(sharpe=sh(r),annual=float(r.mean()*365),cost=float(t.mean()*fee*365),turnover=float(t.mean()))
    out['mdd']=float((cv/np.maximum.accumulate(cv)-1).min()); out['ending']=float(eq)
    return out,rr

print(f"{'变体':<26}{'全期S':>8}{'23-24':>8}{'25-26':>8}{'年化':>8}{'换手':>7}{'年成本':>8}{'最大回撤':>10}")
print("-"*84)
store={}
for which,label in [('all','三因子（基线/自检）'),('nolow','★ 删掉低波动'),('revlow','反转低波动(+SD)'),
                    ('onlymom','只剩动量'),('onlyvol','只剩量冲击'),('onlylow','只剩低波动')]:
    o=run(which)[0]; store[which]=o
    f=o['full']
    print(f"{label:<26}{f['sharpe']:>8.4f}{o['2023-24']['sharpe']:>8.2f}{o['2025-26']['sharpe']:>8.2f}"
          f"{f['annual']*100:>7.1f}%{f['turnover']:>7.3f}{f['cost']*100:>7.1f}%{o['mdd']*100:>9.1f}%")
print()
base=store['all']['full']['sharpe']
print(f"自检: 基线 S = {base:.4f}  （记录值 1.1293）  → {'✓ 复现' if abs(base-1.1293)<0.002 else '✗ 不一致，结果不可用'}")
nl=store['nolow']['full']['sharpe']
print(f"★ 删掉低波动: S {base:.4f} → {nl:.4f}  ({nl-base:+.4f})   回撤 {store['all']['mdd']*100:.1f}% → {store['nolow']['mdd']*100:.1f}%")

# ===== 显著性：配对块 bootstrap（决定能不能真动钱）=====
rng=np.random.default_rng(20261011)
_,ra=run('all'); _,rb=run('nolow'); _,rm=run('onlymom')
def S(r): return np.mean(r)/np.std(r,ddof=1)*np.sqrt(365)
def block_boot(ra,rb,bl,B=9999,seed=20261011):
    rng=np.random.default_rng(seed); nn=len(ra); obs=S(rb)-S(ra); ds=[]
    for _ in range(B):
        idx=[]
        while len(idx)<nn:
            s=rng.integers(0,nn); idx.extend((np.arange(s,s+bl)%nn).tolist())
        idx=np.array(idx[:nn])
        ds.append(S(rb[idx])-S(ra[idx]))
    ds=np.array(ds)
    return obs, ds.std(ddof=1), float((ds<=0).mean()), float(2*min((ds<=0).mean(),(ds>=0).mean()))
print()
print("=== 配对块 bootstrap（H0: ΔSharpe ≤ 0）===")
print(f"{'比较':<22}{'ΔSharpe':>10}{'SE(L20)':>10}{'p(单边)':>10}{'p(双边)':>10}")
for label,rb_ in [('删低波动 − 基线',rb),('只剩动量 − 基线',rm)]:
    for bl in (20,):
        obs,se,p1,p2=block_boot(ra,rb_,bl)
        print(f"{label:<22}{obs:>+10.4f}{se:>10.4f}{p1:>10.3f}{p2:>10.3f}")

print()
print("="*78)
print("当前策略 vs 旧策略（精确指标，cap=5、2.7x、含成本）")
print("="*78)
for lbl,which in [('旧: 三因子','all'),('现在: 两因子(删低波动)','nolow')]:
    o=store[which]; f=o['full']
    cagr=(o['ending']/1e6)**(365/len(ra))-1
    vol=f['annual']/f['sharpe']
    print(f"{lbl}")
    print(f"   Sharpe        {f['sharpe']:.4f}")
    print(f"   算术年化       {f['annual']*100:+.1f}%")
    print(f"   复利 CAGR      {cagr*100:+.1f}%")
    print(f"   年化波动       {vol*100:.1f}%")
    print(f"   日换手         {f['turnover']:.3f}")
    print(f"   年成本         {f['cost']*100:.1f}%")
    print(f"   最大回撤       {o['mdd']*100:.1f}%")
    print(f"   子区间 Sharpe  2023-24 {o['2023-24']['sharpe']:.2f} | 2025-26 {o['2025-26']['sharpe']:.2f}")
    print()
