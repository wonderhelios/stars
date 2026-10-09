import numpy as np, glob, os, json, datetime

# ---------- 载入面板 ----------
rows={}
for f in glob.glob('/tmp/hl-daily-full/*.json'):
    c=os.path.basename(f)[:-5]
    if ':' in c: continue
    try: d=sorted(json.load(open(f)),key=lambda x:int(x['t']))
    except Exception: continue
    d=[x for x in d if float(x['c'])>0]
    if len(d)<100: continue
    rows[c]=[(int(x['t'])//86400000*86400000,float(x['c']),float(x['v'])*float(x['c'])) for x in d]
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
SD=np.zeros((n,k))
for i in range(30,n): SD[i]=R[i-20:i].std(axis=0)

# 每个币固定一个 0..slices-1 的档位（用下标取模，不用 hash，保证可复现）
SLOT=np.arange(k)

def target(i):
    ok=(VOL[i]>=5e6)&np.isfinite(P[i])&(P[i]>0)&np.isfinite(P[i-14])&(P[i-14]>0)
    idx=np.where(ok)[0]
    if len(idx)<8: return None
    m=np.array([(P[i,x]/P[i-14,x]-1.0)/max(SD[i,x],1e-9) for x in idx])
    lv=np.array([-SD[i,x] for x in idx])
    sh=np.array([Q[i,x]/VOL[i,x] if VOL[i,x]>0 else 1.0 for x in idx])
    t=np.zeros(k)
    for sc in (m,lv,sh):
        o=np.argsort(sc); kk=min(max(1,int(round(len(idx)*0.2))),8,len(idx)//2)
        for pos,x in enumerate(o):
            if pos>=len(o)-kk: t[idx[x]]+=0.5/kk/3
            elif pos<kk: t[idx[x]]-=0.5/kk/3
    s=np.abs(t).sum()
    return t/s if s>0 else None

START=45
def run(slices=1, offset=0, fee=0.00045, slip=0.0003):
    w=np.zeros(k); prev=None; rets=[]; turn=0.0
    for i in range(START, n-1):
        r=0.0
        if prev is not None:
            rr=np.nan_to_num(np.where((P[i]>0)&(P[prev]>0),P[i]/P[prev]-1.0,0.0))
            r=float(np.nansum(w*rr))
        prev=i
        t=target(i)
        if t is not None:
            new=w.copy(); changed=False
            for j in range(k):
                # 轮转：i 每天变，所以每个币每 slices 天轮到一次
                if ((i + offset + SLOT[j]) % slices)==0:
                    if abs(t[j]-w[j])>1e-12:
                        new[j]=t[j]; changed=True
            if changed:
                to=float(np.abs(new-w).sum())/2.0
                r-=2*to*(fee+slip); turn+=to; w=new
        rets.append(r)
    return np.array(rets), turn

def stat(x):
    sd=x.std(ddof=1)
    eq=np.cumprod(1+x); pk=np.maximum.accumulate(eq)
    return x.mean()*365*100, (x.mean()/sd*np.sqrt(365)) if sd>0 else 0, ((eq-pk)/pk).min()*100

days=np.array([datetime.datetime.fromtimestamp(t/1000,datetime.UTC).year for t in allts[START:n-1]])
assert len(days)==len(run(1)[0]), f"长度不一致 {len(days)} vs {len(run(1)[0])}"

print("=== 全期（相位平均）===")
print(f"{'方案':<20}{'年化':>9}{'Sharpe':>8}{'回撤':>9}{'日均换手':>10}")
print("-"*56)
res={}
for sl in (1,2,3,4):
    xs=[run(sl,offset=o) for o in range(sl)]
    arrs=[x[0] for x in xs]; turns=[x[1] for x in xs]
    a,s,dd=[np.mean([stat(y)[q] for y in arrs]) for q in range(3)]
    tv=np.mean(turns)/len(days)*100
    res[sl]=arrs
    print(f"{('每日全量' if sl==1 else f'错峰 {sl} 档'):<20}{a:>8.1f}%{s:>8.2f}{dd:>8.1f}%{tv:>9.1f}%")
print()
print("=== 子区间（相位平均）===")
print(f"{'区间':<12}{'每日全量':>18}{'错峰 3 档':>18}")
print(f"{'':<12}{'Sharpe':>9}{'年化':>9}{'Sharpe':>9}{'年化':>9}")
print("-"*48)
for lo,hi in ((2023,2024),(2025,2026),(2023,2026)):
    m=(days>=lo)&(days<=hi)
    if m.sum()<150: continue
    s1=np.mean([stat(y[m])[1] for y in res[1]]); a1=np.mean([stat(y[m])[0] for y in res[1]])
    s3=np.mean([stat(y[m])[1] for y in res[3]]); a3=np.mean([stat(y[m])[0] for y in res[3]])
    print(f"{lo}-{hi:<7}{s1:>9.2f}{a1:>8.1f}%{s3:>9.2f}{a3:>8.1f}%")
