"""v2: 全 234 币 + 动态 head(30天成交额前20%, 含18%/22%缓冲) + 标签在head内算pct_cs"""
import json,glob,datetime,os,warnings
import numpy as np,pandas as pd
from pathlib import Path
BASE = Path(__file__).resolve().parent
warnings.filterwarnings('ignore')
def log(m): print(m, flush=True)
# 1. 加载全 234 币
files=glob.glob(str(BASE/'data'/'hl2'/'*.json'))
univ=sorted(Path(f).stem for f in files)
raw={Path(f).stem:json.load(open(f)) for f in files}
k=len(univ); log(f"全宇宙 {k} 币")
ts=sorted({int(x['t']) for d in raw.values() for x in d if float(x['c'])>0})
ti={t:i for i,t in enumerate(ts)}
n=len(ts); log(f"时间 {n} 天")
O=np.full((n,k),np.nan);C=np.full((n,k),np.nan);H=np.full((n,k),np.nan);L=np.full((n,k),np.nan);V=np.full((n,k),np.nan);N=np.full((n,k),np.nan)
for j,c in enumerate(univ):
    for x in raw[c]:
        i=ti[int(x['t'])]
        O[i,j]=float(x['o']);C[i,j]=float(x['c']);H[i,j]=float(x['h']);L[i,j]=float(x['l']);V[i,j]=float(x['v']);N[i,j]=float(x['n'])
# 2. funding(33币) + OI(parquet全币)
FR=np.full((n,k),np.nan);PREM=np.full((n,k),np.nan)
for j,c in enumerate(univ):
    for fp in (str(BASE/'data'/'hl_funding'/f'{c}.json'),):
        if not Path(fp).exists(): continue
        fd=json.load(open(fp))
        for i in range(n):
            d=datetime.datetime.utcfromtimestamp(ts[i]/1000).strftime('%Y-%m-%d')
            if d not in fd: continue
            v=fd[d]
            if isinstance(v,dict):
                if v.get('fr') is not None and np.isnan(FR[i,j]): FR[i,j]=v['fr']
                if v.get('prem') is not None: PREM[i,j]=v['prem']
            else:
                if np.isnan(FR[i,j]): FR[i,j]=v
        break
OI=np.full((n,k),np.nan)
oi_files=glob.glob(str(BASE/'data'/'hl_oi'/'*.parquet'))
if oi_files:
    oidf=pd.concat([pd.read_parquet(f) for f in oi_files])
    oidf['datetime']=pd.to_datetime(oidf['datetime'])
    oidf['date']=oidf['datetime'].dt.floor('D')
    oi_day=oidf.groupby(['date','coin'])['open_interest'].last().reset_index()
    for j,c in enumerate(univ):
        sub=oi_day[oi_day['coin']==c]
        if len(sub)==0: continue
        sub=sub.set_index('date')
        for i in range(n):
            d=pd.Timestamp(ts[i],unit='ms').floor('D')
            if d in sub.index: OI[i,j]=sub.loc[d,'open_interest']
log(f"funding {int((np.isfinite(FR).sum(0)>0).sum())}/{k}, OI {int((np.isfinite(OI).sum(0)>0).sum())}/{k}")
# 3. 动态 head(含缓冲)
dv30=pd.DataFrame(C*V).rolling(30,min_periods=30).mean().values
def head_members():
    members=np.zeros((n,k),bool); prev=None
    for i in range(n):
        row=dv30[i]; valid=np.isfinite(row)
        if valid.sum()<10: prev=set(); continue
        idx=np.flatnonzero(valid)
        order=idx[np.argsort(-row[valid],kind='stable')]
        N=valid.sum(); cur=set()
        for pos,j in enumerate(order):
            pct=(pos+1)/N*100
            upper = 20.0 if prev is None else (22.0 if j in prev else 18.0)
            if pct<=upper: cur.add(int(j))
        members[i]=np.array([j in cur for j in range(k)])
        prev=cur
    return members
members=head_members()
log(f"head 成员: 每天平均 {members.sum(1).mean():.1f} 币, 历史 {int((members.sum(0)>0).sum())} 币进过")
# 4. 构造 151 特征(全币)
feats={}
feats['log_price']=np.log(C)
feats['kmid']=(C-O)/O; feats['klen']=(H-L)/O; feats['kmid_2']=(C-O)/(H-L)
feats['kup']=(H-np.maximum(O,C))/O; feats['kup_2']=(H-np.maximum(O,C))/(H-L)
feats['klow']=(np.minimum(O,C)-L)/O; feats['klow_2']=(np.minimum(O,C)-L)/(H-L)
feats['ksft']=(2*C-H-L)/O; feats['ksft_2']=(2*C-H-L)/(H-L)
feats['open_0']=O/C; feats['high_0']=H/C; feats['low_0']=L/C; feats['vwap_0']=np.full((n,k),np.nan)
WINDOWS=(5,10,20,30,60)
dC=pd.DataFrame(C);dH=pd.DataFrame(H);dL=pd.DataFrame(L);dV=pd.DataFrame(V)
logV=np.log(np.maximum(V,1e-12)); dlogV=pd.DataFrame(logV)
ret=C/np.roll(C,1,axis=0)-1; ret[0]=np.nan
for w in WINDOWS:
    feats[f'roc_{w}']=np.roll(C,w,axis=0)/C
    feats[f'ma_{w}']=dC.rolling(w,min_periods=w).mean().values/C
    up=(C>np.roll(C,1,axis=0)).astype(float); dn=(C<np.roll(C,1,axis=0)).astype(float)
    feats[f'cntp_{w}']=pd.DataFrame(up).rolling(w,min_periods=w).mean().values
    feats[f'cntn_{w}']=pd.DataFrame(dn).rolling(w,min_periods=w).mean().values
    feats[f'cntd_{w}']=feats[f'cntp_{w}']-feats[f'cntn_{w}']
    feats[f'std_{w}']=dC.rolling(w,min_periods=w).std(ddof=1).values/C
    feats[f'max_{w}']=dH.rolling(w,min_periods=w).max().values/C
    feats[f'min_{w}']=dL.rolling(w,min_periods=w).min().values/C
    feats[f'qtlu_{w}']=dC.rolling(w,min_periods=w).quantile(0.8).values/C
    feats[f'qtld_{w}']=dC.rolling(w,min_periods=w).quantile(0.2).values/C
    imax=dH.rolling(w,min_periods=w).apply(lambda a:float(np.nanargmax(a)),raw=True).values
    imin=dL.rolling(w,min_periods=w).apply(lambda a:float(np.nanargmin(a)),raw=True).values
    feats[f'imax_{w}']=imax/w; feats[f'imin_{w}']=imin/w; feats[f'imxd_{w}']=(imax-imin)/w
    feats[f'rank_{w}']=dC.rolling(w,min_periods=w).rank(pct=True).values
    feats[f'rsv_{w}']=(C-dL.rolling(w,min_periods=w).min().values)/(dH.rolling(w,min_periods=w).max().values-dL.rolling(w,min_periods=w).min().values)
    feats[f'corr_{w}']=dC.rolling(w,min_periods=w).corr(dlogV).values
    feats[f'cord_{w}']=pd.DataFrame(C/np.roll(C,1,axis=0)).rolling(w,min_periods=w).corr(pd.DataFrame(np.log(V/np.roll(V,1,axis=0)+1))).values
    feats[f'vma_{w}']=dV.rolling(w,min_periods=w).mean().values/V
    feats[f'vstd_{w}']=dV.rolling(w,min_periods=w).std(ddof=1).values/V
    wv=pd.DataFrame(np.abs(ret)*V)
    feats[f'wvma_{w}']=wv.rolling(w,min_periods=w).std(ddof=1).values/wv.rolling(w,min_periods=w).mean().values
    pos=np.maximum(C-np.roll(C,1,axis=0),0); neg=np.maximum(np.roll(C,1,axis=0)-C,0); tot=np.abs(C-np.roll(C,1,axis=0))
    dpos=pd.DataFrame(pos);dneg=pd.DataFrame(neg);dtot=pd.DataFrame(tot)
    feats[f'sump_{w}']=dpos.rolling(w,min_periods=w).sum().values/dtot.rolling(w,min_periods=w).sum().values
    feats[f'sumn_{w}']=dneg.rolling(w,min_periods=w).sum().values/dtot.rolling(w,min_periods=w).sum().values
    feats[f'sumd_{w}']=feats[f'sump_{w}']-feats[f'sumn_{w}']
    vpos=np.maximum(V-np.roll(V,1,axis=0),0); vneg=np.maximum(np.roll(V,1,axis=0)-V,0); vtot=np.abs(V-np.roll(V,1,axis=0))
    dvpos=pd.DataFrame(vpos);dvneg=pd.DataFrame(vneg);dvtot=pd.DataFrame(vtot)
    feats[f'vsump_{w}']=dvpos.rolling(w,min_periods=w).sum().values/dvtot.rolling(w,min_periods=w).sum().values
    feats[f'vsumn_{w}']=dvneg.rolling(w,min_periods=w).sum().values/dvtot.rolling(w,min_periods=w).sum().values
    feats[f'vsumd_{w}']=feats[f'vsump_{w}']-feats[f'vsumn_{w}']
feats['fr_1d']=pd.DataFrame(FR).rolling(1,min_periods=1).mean().values
feats['fr_7d']=pd.DataFrame(FR).rolling(7,min_periods=7).mean().values
feats['prem_1d']=pd.DataFrame(PREM).rolling(1,min_periods=1).mean().values
feats['oi_chg_1d']=OI/np.roll(OI,1,axis=0)-1
feats['oi_chg_7d']=OI/np.roll(OI,7,axis=0)-1
feats['tc_ratio']=N/pd.DataFrame(N).rolling(7,min_periods=7).mean().values
feats['log_dv']=np.log(pd.DataFrame(C*V).rolling(7,min_periods=7).mean().values)
log(f"特征 {len(feats)} 个")
# 5. 标签 pct_cs(forward) 在 head 成员内
fwd=np.roll(C,-3,axis=0)/C-1
labels=np.full((n,k),np.nan)
for i in range(n):
    hm=members[i]
    if hm.sum()<5: continue
    row=fwd[i].copy(); row[~hm]=np.nan
    valid=np.isfinite(row)
    if valid.sum()<3: continue
    ranks=pd.Series(row[valid]).rank()
    labels[i,valid]=ranks.values/valid.sum()
log(f"标签覆盖 {int((np.isfinite(labels).sum(0)>0).sum())}/{k} 币")
# 6. 保存
np.savez_compressed(str(BASE/'features.npz'), ts=np.array(ts), coins=np.array(univ),
    **{f'F_{name}':v for name,v in feats.items()}, labels=labels, fwd=fwd, members=members)
log("已保存 features.npz")
