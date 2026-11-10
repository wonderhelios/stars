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
    bars = raw[c]
    # 模拟采集器: 删除 volume=0 的回填前缀（HL candleSnapshot 回填了上线前的 bar，采集器当时采集不到）
    start_bi = None
    for bi, x in enumerate(bars):
        if float(x['v']) > 0:
            start_bi = bi
            break
    if start_bi is None:
        continue  # 整个历史无成交量，跳过
    bars = bars[start_bi:]
    for x in bars:
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
        row=dv30[i]; valid=np.isfinite(row)  # is_finite（volume=0 的币 adv=0 是有限值，保留）
        idx=np.flatnonzero(valid)
        if len(idx)==0 and prev is None:
            continue  # 尚无合格候选且未建池，跳过（不能提前消耗首次建池）
        order=idx[np.argsort(-row[valid],kind='stable')]
        N=valid.sum(); cur=set()
        for pos,j in enumerate(order):
            upper = 20.0 if prev is None else (22.0 if j in prev else 18.0)
            # 1-based 整数比较（polars rank 是 1-based ✗ 避免浮点）
            if (pos+1)*100 <= upper*N: cur.add(int(j))
        # 关键: effective = datetime + 2 天（D 日成分用 D-2 日的数据 ✗ 2 天滞后）
        if i+2 < n:
            members[i+2]=np.array([j in cur for j in range(k)])
        prev=cur
    return members
members=head_members()
log(f"head 成员: 每天平均 {members.sum(1).mean():.1f} 币, 历史 {int((members.sum(0)>0).sum())} 币进过")
# 4. 构造 151 特征(全币)
feats={}
feats['kmid']=(C-O)/O; feats['klen']=(H-L)/O; feats['kmid_2']=(C-O)/(H-L)
feats['kup']=(H-np.maximum(O,C))/O; feats['kup_2']=(H-np.maximum(O,C))/(H-L)
feats['klow']=(np.minimum(O,C)-L)/O; feats['klow_2']=(np.minimum(O,C)-L)/(H-L)
feats['ksft']=(2*C-H-L)/O; feats['ksft_2']=(2*C-H-L)/(H-L)
feats['open_0']=O/C; feats['high_0']=H/C; feats['low_0']=L/C; feats['vwap_0']=np.full((n,k),np.nan)
WINDOWS=(5,10,20,30,60)
# numpy 手写滚动（对齐 alphapilot operators.py 的 _windowed 浮点累加顺序）
def _sw(arr, w, partial=False):
    ap = np.pad(arr, ((w-1,0),(0,0)), constant_values=np.nan) if partial else arr
    return np.lib.stride_tricks.sliding_window_view(ap, w, axis=0)
def _mean3(wins):
    valid = ~np.isnan(wins)
    counts = valid.sum(axis=2)
    totals = np.sum(wins, axis=2, where=valid)
    return np.divide(totals, counts, out=np.full(counts.shape, np.nan), where=counts>0)
def _std3(wins):
    valid = ~np.isnan(wins)
    first = valid.argmax(axis=2)
    fi, fj = np.ogrid[:wins.shape[0], :wins.shape[1]]
    shifted = wins - wins[fi, fj, first][:,:,None]
    centered = shifted - _mean3(shifted)[:,:,None]
    return np.sqrt(_mean3(centered*centered))
def _corr3(xw, yw):
    with np.errstate(divide='ignore', invalid='ignore'):
        cx = xw - xw.mean(axis=2, keepdims=True)
        cy = yw - yw.mean(axis=2, keepdims=True)
        return (cx*cy).sum(axis=2) / np.sqrt((cx*cx).sum(axis=2)*(cy*cy).sum(axis=2))
def _rank3(wins):
    return (wins <= wins[:,:,-1:]).sum(axis=2) / wins.shape[2]
def _part(arr, w, fn):
    return fn(_sw(arr, w, True))
def _strict1(arr, w, fn):
    wins = _sw(arr, w, False)
    res = fn(wins)
    complete = (~np.isnan(wins)).all(axis=2)
    res = np.where(complete, res, np.nan)
    out = np.full((arr.shape[0], arr.shape[1]), np.nan)
    out[w-1:] = res
    return out
def _strict2(a, b, w):
    aw = _sw(a, w, False); bw = _sw(b, w, False)
    res = _corr3(aw, bw)
    complete = (~np.isnan(aw)).all(axis=2) & (~np.isnan(bw)).all(axis=2)
    res = np.where(complete, res, np.nan)
    out = np.full((a.shape[0], a.shape[1]), np.nan)
    out[w-1:] = res
    return out
def _shift(arr, n):
    out = np.full_like(arr, np.nan)
    if n > 0: out[n:] = arr[:-n]
    elif n < 0: out[:n] = arr[-n:]
    else: out = arr.copy()
    return out
logV = np.log(np.where(V>0, V, np.nan))
_c1 = _shift(C, 1)
ret = C/_c1 - 1
for _wi, w in enumerate(WINDOWS):
    log(f"  特征窗口 {w} 开始（{_wi+1}/{len(WINDOWS)}）")
    feats[f'roc_{w}']=_shift(C, w)/C
    feats[f'ma_{w}']=_part(C, w, _mean3)/C
    up=np.where(np.isfinite(_c1),(C>_c1).astype(float),np.nan); dn=np.where(np.isfinite(_c1),(C<_c1).astype(float),np.nan)
    feats[f'cntp_{w}']=_part(up, w, _mean3)
    feats[f'cntn_{w}']=_part(dn, w, _mean3)
    feats[f'cntd_{w}']=feats[f'cntp_{w}']-feats[f'cntn_{w}']
    feats[f'std_{w}']=_part(C, w, _std3)/C
    feats[f'max_{w}']=_part(H, w, lambda x: np.nanmax(x, axis=2))/C
    feats[f'min_{w}']=_part(L, w, lambda x: np.nanmin(x, axis=2))/C
    feats[f'qtlu_{w}']=_strict1(C, w, lambda x: np.quantile(x, 0.8, axis=2, method='linear'))/C
    feats[f'qtld_{w}']=_strict1(C, w, lambda x: np.quantile(x, 0.2, axis=2, method='linear'))/C
    imax=_strict1(H, w, lambda x: x.argmax(axis=2)+1); imin=_strict1(L, w, lambda x: x.argmin(axis=2)+1)
    feats[f'imax_{w}']=imax/w; feats[f'imin_{w}']=imin/w; feats[f'imxd_{w}']=(imax-imin)/w
    feats[f'rank_{w}']=_strict1(C, w, _rank3)
    mnL=_part(L, w, lambda x: np.nanmin(x, axis=2)); mxH=_part(H, w, lambda x: np.nanmax(x, axis=2))
    feats[f'rsv_{w}']=(C-mnL)/(mxH-mnL)
    feats[f'corr_{w}']=_strict2(C, logV, w)
    feats[f'cord_{w}']=_strict2(C/_c1, np.log(V/_shift(V,1)+1), w)
    feats[f'vma_{w}']=_part(V, w, _mean3)/V
    feats[f'vstd_{w}']=_part(V, w, _std3)/V
    wv=np.abs(ret)*V
    feats[f'wvma_{w}']=_part(wv, w, _std3)/_part(wv, w, _mean3)
    pos=np.maximum(C-_c1,0); neg=np.maximum(_c1-C,0); tot=np.abs(C-_c1)
    feats[f'sump_{w}']=_strict1(pos, w, lambda x: x.sum(axis=2))/_strict1(tot, w, lambda x: x.sum(axis=2))
    feats[f'sumn_{w}']=_strict1(neg, w, lambda x: x.sum(axis=2))/_strict1(tot, w, lambda x: x.sum(axis=2))
    feats[f'sumd_{w}']=feats[f'sump_{w}']-feats[f'sumn_{w}']
    vpos=np.maximum(V-_shift(V,1),0); vneg=np.maximum(_shift(V,1)-V,0); vtot=np.abs(V-_shift(V,1))
    feats[f'vsump_{w}']=_strict1(vpos, w, lambda x: x.sum(axis=2))/_strict1(vtot, w, lambda x: x.sum(axis=2))
    feats[f'vsumn_{w}']=_strict1(vneg, w, lambda x: x.sum(axis=2))/_strict1(vtot, w, lambda x: x.sum(axis=2))
    feats[f'vsumd_{w}']=feats[f'vsump_{w}']-feats[f'vsumn_{w}']
    log(f"  特征窗口 {w} 完成（累计 {len(feats)} 个特征）")
feats['fr_1d']=_part(FR, 1, _mean3)
feats['fr_7d']=_part(FR, 7, _mean3)
feats['prem_1d']=_part(PREM, 1, _mean3)
feats['oi_chg_1d']=OI/_shift(OI,1)-1
feats['oi_chg_7d']=OI/_shift(OI,7)-1
feats['tc_ratio']=N/_part(N, 7, _mean3)
feats['log_dv']=np.log(_part(C*V, 7, _mean3))
feats['log_price']=np.log(C)  # 第 151 位（最后 ✗ 和 alphapilot 列顺序一致）
log(f"特征 {len(feats)} 个")
# 5. 标签 pct_cs(forward) 在 head 成员内
fwd=_shift(C, -3)/C-1
labels=np.full((n,k),np.nan)
for i in range(n):
    hm=members[i]
    row=fwd[i].copy(); row[~hm]=np.nan
    valid=np.isfinite(row)
    if valid.sum()==0: continue  # 没有 forward 非空的成员（pct_cs 分母为 0）
    ranks=pd.Series(row[valid]).rank()
    labels[i,valid]=ranks.values/valid.sum()
log(f"标签覆盖 {int((np.isfinite(labels).sum(0)>0).sum())}/{k} 币")
# 5.5 非有限 → NaN（DSL 把 inf/NaN 统一转 null ✗ 比如 log(0)、除零）
for _name in feats:
    _v = feats[_name]
    feats[_name] = np.where(np.isfinite(_v), _v, np.nan)
# 6. 保存
np.savez_compressed(str(BASE/'features.npz'), ts=np.array(ts), coins=np.array(univ),
    **{f'F_{name}':v for name,v in feats.items()}, labels=labels, fwd=fwd, members=members)
log("已保存 features.npz")
