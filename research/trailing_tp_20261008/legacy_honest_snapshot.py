import json,os,datetime
import numpy as np
D="/tmp/hl-daily-full"
rows={}
for f in sorted(os.listdir(D)):
    if not f.endswith('.json'): continue
    c=f[:-5]
    try: d=sorted(json.load(open(os.path.join(D,f))),key=lambda x:int(x['t']))
    except: continue
    d=[x for x in d if float(x['c'])>0]
    if len(d)<100: continue
    rows[c]=[(int(x['t']),float(x['c']),float(x['v'])*float(x['c'])) for x in d]
allts=sorted(set(t for v in rows.values() for t,_,_ in v))
ti={t:i for i,t in enumerate(allts)}
names=sorted(rows); n,k=len(allts),len(names)
P=np.full((n,k),np.nan); Q=np.zeros((n,k))
for j,c in enumerate(names):
    for t,p,q in rows[c]:
        i=ti.get(t)
        if i is not None: P[i,j]=p; Q[i,j]=q
R=np.zeros((n,k))
for i in range(1,n):
    with np.errstate(invalid='ignore',divide='ignore'):
        R[i]=np.where((P[i]>0)&(P[i-1]>0),P[i]/P[i-1]-1.0,0.0)
VOL=np.zeros((n,k)); cs=np.cumsum(Q,axis=0)
for i in range(n):
    lo=max(0,i-30); VOL[i]=(cs[i]-(cs[lo-1] if lo>0 else 0))/max(1,i-lo)
SD20=np.zeros((n,k))
for i in range(30,n):
    SD20[i]=R[i-20:i].std(axis=0)

def weights(i, top=0.2, cap=8):
    ok=(VOL[i]>=5e6)&np.isfinite(P[i])&(P[i]>0)&np.isfinite(P[i-14])&(P[i-14]>0)
    idx=np.where(ok)[0]
    if len(idx)<8: return None
    m=np.array([(P[i,j]/P[i-14,j]-1.0)/max(SD20[i,j],1e-9) for j in idx])
    lv=np.array([-SD20[i,j] for j in idx])
    sh=np.array([Q[i,j]/VOL[i,j] if VOL[i,j]>0 else 1.0 for j in idx])
    tgt=np.zeros(k)
    for sc in (m,lv,sh):
        o=np.argsort(sc); kk=min(max(1,int(round(len(idx)*top))),cap, len(idx)//2)
        for pos,j in enumerate(o):
            if pos>=len(o)-kk: tgt[idx[j]]+=0.5/kk/3
            elif pos<kk: tgt[idx[j]]-=0.5/kk/3
    s=np.abs(tgt).sum()
    if s>0: tgt/=s
    return tgt

def run(rebal=1, slip=0.0, fee=0.00045, hold=None):
    w=np.zeros(k); prev=None; rets=[]; days=[]
    for i in range(45,n-1):
        r=0.0
        if prev is not None:
            rr=np.nan_to_num(np.where((P[i]>0)&(P[prev]>0),P[i]/P[prev]-1.0,0.0))
            r=float(np.nansum(w*rr))
        prev=i
        do = (hold is None) or ((i % rebal)==0)
        if do:
            t=weights(i)
            if t is None: rets.append(0.0); days.append(allts[i]); w=np.zeros(k); continue
            to=float(np.abs(t-w).sum())/2.0
            # 费率 + 额外滑点（每次换仓双向）
            r -= 2*to*(fee+slip); w=t
        rets.append(r); days.append(allts[i])
    return np.array(rets), days

def st(x):
    x=np.asarray(x); m,sd=x.mean(),x.std(ddof=1)
    eq=np.cumprod(1+x); pk=np.maximum.accumulate(eq)
    return m*365*100, sd*np.sqrt(365)*100, (m/sd*np.sqrt(365)) if sd>0 else 0, ((eq-pk)/pk).min()*100

base,days=run()
print("=== 3.2 年全期（含 taker 费 0.045%）===")
a,v,s,dd=st(base); print(f"  年化 {a:+.1f}%  波动 {v:.1f}%  Sharpe {s:.2f}  最大回撤 {dd:.1f}%")
print()
print("=== 逐年稳定性（这才是真实可信度）===")
yrs=np.array([datetime.datetime.fromtimestamp(t/1000,datetime.UTC).year for t in days])
for y in sorted(set(yrs)):
    sel=yrs==y
    if sel.sum()<60: continue
    a,v,s,dd=st(base[sel])
    print(f"  {y}: 年化 {a:+7.1f}%  波动 {v:5.1f}%  Sharpe {s:5.2f}  回撤 {dd:6.1f}%  ({sel.sum()} 天)")
print()
print("=== 加入滑点后的影响 ===")
for slip in (0.0, 0.0005, 0.001, 0.002):
    x,_=run(slip=slip); a,v,s,dd=st(x)
    print(f"  滑点 {slip*100:.2f}%/次:  年化 {a:+6.1f}%  Sharpe {s:5.2f}  回撤 {dd:6.1f}%")
print()
print("=== 换仓频率（降成本）===")
for rb,label in ((1,'每日'),(2,'每2日'),(3,'每3日'),(5,'每5日')):
    x,_=run(rebal=rb, hold=True); a,v,s,dd=st(x)
    print(f"  {label}:  年化 {a:+6.1f}%  Sharpe {s:5.2f}  回撤 {dd:6.1f}%")
