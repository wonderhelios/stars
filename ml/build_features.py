"""v3: polars 版（和 alphapilot 同口径 ✗ Rust 实现 ✗ 内存小 ✗ 快）
全 235 币 + 动态 head(30天成交额前20% ✗ 18/22 缓冲 ✗ D 日成分用 D-2 数据) + 标签 pct_cs(forward_3d) 在 head 内
"""
import json, glob, datetime, warnings, os
import numpy as np
import pandas as pd
import polars as pl
from pathlib import Path
warnings.filterwarnings('ignore')
BASE = Path(__file__).resolve().parent
def log(m): print(m, flush=True)
WINDOWS = (5, 10, 20, 30, 60)

# ===== 1. 加载 hl2 长表（polars）=====
files = glob.glob(str(BASE / 'data' / 'hl2' / '*.json'))
univ = sorted(Path(f).stem for f in files)
raw = {Path(f).stem: json.load(open(f)) for f in files}
log(f"全宇宙 {len(univ)} 币")
rows = []
for c in univ:
    bars = raw[c]
    start = None
    for bi, x in enumerate(bars):
        if float(x['v']) > 0:
            start = bi; break
    if start is None: continue
    for x in bars[start:]:
        t = datetime.datetime.utcfromtimestamp(int(x['t']) / 1000)
        rows.append((t, c, float(x['o']), float(x['h']), float(x['l']), float(x['c']), float(x['v']), float(x['n'])))
df = pl.DataFrame(rows, schema={'datetime': pl.Datetime, 'coin': pl.String, 'open': pl.Float64, 'high': pl.Float64, 'low': pl.Float64, 'close': pl.Float64, 'volume': pl.Float64, 'trade_count': pl.Float64}, orient='row').sort('coin', 'datetime')
log(f"长表 {df.height} 行")

# ===== 2. funding + OI（转成列）=====
fr_maps, prem_maps = {}, {}
for c in univ:
    fp = BASE / 'data' / 'hl_funding' / f'{c}.json'
    if not fp.exists(): continue
    fd = json.load(open(fp)); fr, pm = {}, {}
    for dd, v in fd.items():
        if isinstance(v, dict):
            if v.get('fr') is not None: fr[dd] = v['fr']
            if v.get('prem') is not None: pm[dd] = v['prem']
        else: fr[dd] = v
    fr_maps[c] = fr; prem_maps[c] = pm
oi_map = {}
oi_files = glob.glob(str(BASE / 'data' / 'hl_oi' / '*.parquet'))
if oi_files:
    oidf = pd.concat([pd.read_parquet(f) for f in oi_files])
    oidf['date'] = pd.to_datetime(oidf['datetime']).dt.strftime('%Y-%m-%d')
    for _, r in oidf.groupby(['date','coin'])['open_interest'].last().reset_index().iterrows():
        oi_map[(r['coin'], r['date'])] = float(r['open_interest'])
df = df.with_columns(pl.col('datetime').dt.strftime('%Y-%m-%d').alias('ds'))
fr_l, prem_l, oi_l = [], [], []
for c, ds in zip(df['coin'].to_list(), df['ds'].to_list()):
    fr_l.append(fr_maps.get(c, {}).get(ds)); prem_l.append(prem_maps.get(c, {}).get(ds)); oi_l.append(oi_map.get((c, ds)))
df = df.with_columns(pl.Series('fr', fr_l, dtype=pl.Float64), pl.Series('prem', prem_l, dtype=pl.Float64), pl.Series('oi', oi_l, dtype=pl.Float64))
log(f"funding {int(df['fr'].is_not_null().sum())} 行, OI {int(df['oi'].is_not_null().sum())} 行")

# ===== 3. adv + head（polars 分组 + 递推）=====
df = df.with_columns((pl.col('close') * pl.col('volume')).rolling_mean(30).over('coin').alias('adv'))
all_ts = sorted(df['datetime'].unique().to_list())
ts_to_i = {t: i for i, t in enumerate(all_ts)}
coin_to_j = {c: j for j, c in enumerate(univ)}
k = len(univ); N = len(all_ts)
N_tmp = 0
prev = None; members = np.zeros((N, k), bool)
for dt0 in all_ts:
    day = df.filter((pl.col('datetime') == dt0) & pl.col('adv').is_finite())
    if day.height == 0 and prev is None:
        continue
    s = day.sort(['adv', 'coin'], descending=[True, False])
    coins_sorted = s['coin'].to_list()
    eff = dt0 + datetime.timedelta(days=2)
    if eff not in ts_to_i:
        continue
    if not coins_sorted and prev is None:
        continue
    Nc = len(coins_sorted); cur = set()
    for pos, coin in enumerate(coins_sorted):
        upper = 20.0 if prev is None else (22.0 if coin in prev else 18.0)
        if (pos + 1) * 100 <= upper * Nc:
            cur.add(coin)
    prev = cur
    i = ts_to_i[eff]
    for c in cur:
        if c in coin_to_j:
            members[i, coin_to_j[c]] = True
log(f"head 成员: 每天平均 {members.sum(1).mean():.1f} 币, 历史 {int((members.sum(0)>0).sum())} 币进过")

# ===== 4. 151 特征（polars rolling ✗ 和 alphapilot operators.py 同口径）=====
def ts(e): return e.over('coin', order_by='datetime')
o, h, l, c, v, nc = pl.col('open'), pl.col('high'), pl.col('low'), pl.col('close'), pl.col('volume'), pl.col('trade_count')
feats = {}
feats['kmid'] = (c - o) / o; feats['klen'] = (h - l) / o; feats['kmid_2'] = (c - o) / (h - l)
feats['kup'] = (h - pl.max_horizontal(o, c)) / o; feats['kup_2'] = (h - pl.max_horizontal(o, c)) / (h - l)
feats['klow'] = (pl.min_horizontal(o, c) - l) / o; feats['klow_2'] = (pl.min_horizontal(o, c) - l) / (h - l)
feats['ksft'] = (c * 2 - h - l) / o; feats['ksft_2'] = (c * 2 - h - l) / (h - l)
feats['open_0'] = o / c; feats['high_0'] = h / c; feats['low_0'] = l / c
feats['vwap_0'] = pl.lit(None, pl.Float64)
for w in WINDOWS:
    feats[f'roc_{w}'] = ts(c.shift(w)) / c
    feats[f'ma_{w}'] = ts(c.rolling_mean(w, min_samples=1)) / c
    feats[f'cntp_{w}'] = ts((c > ts(c.shift(1))).cast(pl.Float64).rolling_mean(w, min_samples=1))
    feats[f'cntn_{w}'] = ts((c < ts(c.shift(1))).cast(pl.Float64).rolling_mean(w, min_samples=1))
    feats[f'cntd_{w}'] = feats[f'cntp_{w}'] - feats[f'cntn_{w}']
    feats[f'std_{w}'] = ts(c.rolling_std(w, min_samples=1, ddof=0)) / c
    feats[f'max_{w}'] = ts(h.rolling_max(w, min_samples=1)) / c
    feats[f'min_{w}'] = ts(l.rolling_min(w, min_samples=1)) / c
    feats[f'qtlu_{w}'] = ts(c.rolling_quantile(0.8, interpolation='linear', window_size=w)) / c
    feats[f'qtld_{w}'] = ts(c.rolling_quantile(0.2, interpolation='linear', window_size=w)) / c
    imax = ts(h.rolling_map(lambda s: s.arg_max() + 1, window_size=w, min_samples=w))
    imin = ts(l.rolling_map(lambda s: s.arg_min() + 1, window_size=w, min_samples=w))
    feats[f'imax_{w}'] = imax / w; feats[f'imin_{w}'] = imin / w; feats[f'imxd_{w}'] = (imax - imin) / w
    feats[f'rank_{w}'] = ts(c.rolling_map(lambda s: (s <= s[-1]).sum() / len(s), window_size=w, min_samples=w))
    mnL = ts(l.rolling_min(w, min_samples=1)); mxH = ts(h.rolling_max(w, min_samples=1))
    feats[f'rsv_{w}'] = (c - mnL) / (mxH - mnL)
    feats[f'corr_{w}'] = ts(pl.rolling_corr(c, v.log(), window_size=w, min_samples=w))
    feats[f'cord_{w}'] = ts(pl.rolling_corr(c / ts(c.shift(1)), (v / ts(v.shift(1)) + 1).log(), window_size=w, min_samples=w))
    feats[f'vma_{w}'] = ts(v.rolling_mean(w, min_samples=1)) / v
    feats[f'vstd_{w}'] = ts(v.rolling_std(w, min_samples=1, ddof=0)) / v
    wv = (c / ts(c.shift(1)) - 1).abs() * v
    feats[f'wvma_{w}'] = ts(wv.rolling_std(w, min_samples=1, ddof=0)) / ts(wv.rolling_mean(w, min_samples=1))
    dlt = c - ts(c.shift(1))
    feats[f'sump_{w}'] = ts(pl.max_horizontal(dlt, 0).rolling_sum(w, min_samples=w)) / ts(dlt.abs().rolling_sum(w, min_samples=w))
    feats[f'sumn_{w}'] = ts(pl.max_horizontal(-dlt, 0).rolling_sum(w, min_samples=w)) / ts(dlt.abs().rolling_sum(w, min_samples=w))
    feats[f'sumd_{w}'] = feats[f'sump_{w}'] - feats[f'sumn_{w}']
    dvlt = v - ts(v.shift(1))
    feats[f'vsump_{w}'] = ts(pl.max_horizontal(dvlt, 0).rolling_sum(w, min_samples=w)) / ts(dvlt.abs().rolling_sum(w, min_samples=w))
    feats[f'vsumn_{w}'] = ts(pl.max_horizontal(-dvlt, 0).rolling_sum(w, min_samples=w)) / ts(dvlt.abs().rolling_sum(w, min_samples=w))
    feats[f'vsumd_{w}'] = feats[f'vsump_{w}'] - feats[f'vsumn_{w}']
    log(f"  特征窗口 {w} 完成（累计 {len(feats)} 个）")
feats['fr_1d'] = ts(pl.col('fr').rolling_mean(1, min_samples=1))
feats['fr_7d'] = ts(pl.col('fr').rolling_mean(7, min_samples=1))
feats['prem_1d'] = ts(pl.col('prem').rolling_mean(1, min_samples=1))
feats['oi_chg_1d'] = pl.col('oi') / ts(pl.col('oi').shift(1)) - 1
feats['oi_chg_7d'] = pl.col('oi') / ts(pl.col('oi').shift(7)) - 1
feats['tc_ratio'] = nc / ts(nc.rolling_mean(7, min_samples=1))
feats['log_dv'] = ts((c * v).rolling_mean(7, min_samples=1)).log()
feats['log_price'] = c.log()
log(f"特征 {len(feats)} 个")

# ===== 5. 一次算完（polars）→ 转 (n,k) 矩阵 =====
log("计算 151 特征（分币 ✗ 单币 pandas rolling ✗ 内存 < 100MB）…")
import gc
feat_names = list(feats.keys())
X = np.full((N, k, len(feat_names)), np.nan, dtype=np.float32)
fi_map = {name: i for i, name in enumerate(feat_names)}

# 按币分块：每个币取出该币的 OHLCV 序列，用 pandas rolling 算 151 特征
coin_vals = {}
for _coin in univ:
    sub = df.filter(pl.col('coin') == _coin).sort('datetime')
    if sub.height == 0:
        continue
    coin_vals[_coin] = {
        'o': sub['open'].to_numpy(), 'h': sub['high'].to_numpy(), 'l': sub['low'].to_numpy(),
        'c': sub['close'].to_numpy(), 'v': sub['volume'].to_numpy(), 'n': sub['trade_count'].to_numpy(),
        'fr': sub['fr'].to_numpy(), 'prem': sub['prem'].to_numpy(), 'oi': sub['oi'].to_numpy(),
        'ts': sub['datetime'].to_list(),
    }
log(f"已按币拆分 {len(coin_vals)} 币")

def _p(s): return pd.Series(s)
def _sh(arr, n):
    out = np.full_like(arr, np.nan, dtype=np.float64)
    if n > 0: out[n:] = arr[:-n]
    elif n < 0: out[:n] = arr[-n:]
    else: out = arr.astype(np.float64).copy()
    return out

for ci, coin in enumerate(univ):
    if coin not in coin_vals:
        continue
    cv = coin_vals[coin]
    o, h, l, cl, v, nc, fr, prem, oi = cv['o'], cv['h'], cv['l'], cv['c'], cv['v'], cv['n'], cv['fr'], cv['prem'], cv['oi']
    res = {}
    # K 线
    res['kmid'] = (cl - o) / o; res['klen'] = (h - l) / o; res['kmid_2'] = (cl - o) / (h - l)
    res['kup'] = (h - np.maximum(o, cl)) / o; res['kup_2'] = (h - np.maximum(o, cl)) / (h - l)
    res['klow'] = (np.minimum(o, cl) - l) / o; res['klow_2'] = (np.minimum(o, cl) - l) / (h - l)
    res['ksft'] = (cl * 2 - h - l) / o; res['ksft_2'] = (cl * 2 - h - l) / (h - l)
    res['open_0'] = o / cl; res['high_0'] = h / cl; res['low_0'] = l / cl
    res['vwap_0'] = np.full(len(cl), np.nan)
    # 滚动模板（pandas rolling ✗ 单币内存极小）
    sc, sh, sl, sv = _p(cl), _p(h), _p(l), _p(v)
    snc = _p(nc)
    logv = _p(np.log(np.where(v > 0, v, np.nan)))
    _c1 = _sh(cl, 1)
    ret = cl / _c1 - 1
    wv = np.abs(ret) * v
    for w in WINDOWS:
        res[f'roc_{w}'] = _sh(cl, w) / cl
        res[f'ma_{w}'] = sc.rolling(w, min_periods=1).mean().values / cl
        up = np.where(np.isfinite(_c1), (cl > _c1).astype(float), np.nan)
        dn = np.where(np.isfinite(_c1), (cl < _c1).astype(float), np.nan)
        res[f'cntp_{w}'] = _p(up).rolling(w, min_periods=1).mean().values
        res[f'cntn_{w}'] = _p(dn).rolling(w, min_periods=1).mean().values
        res[f'cntd_{w}'] = res[f'cntp_{w}'] - res[f'cntn_{w}']
        res[f'std_{w}'] = sc.rolling(w, min_periods=1).std(ddof=0).values / cl
        res[f'max_{w}'] = sh.rolling(w, min_periods=1).max().values / cl
        res[f'min_{w}'] = sl.rolling(w, min_periods=1).min().values / cl
        res[f'qtlu_{w}'] = sc.rolling(w, min_periods=w).quantile(0.8).values / cl
        res[f'qtld_{w}'] = sc.rolling(w, min_periods=w).quantile(0.2).values / cl
        imax = sh.rolling(w, min_periods=w).apply(lambda a: float(np.nanargmax(a)) + 1.0, raw=True).values
        imin = sl.rolling(w, min_periods=w).apply(lambda a: float(np.nanargmin(a)) + 1.0, raw=True).values
        res[f'imax_{w}'] = imax / w; res[f'imin_{w}'] = imin / w; res[f'imxd_{w}'] = (imax - imin) / w
        res[f'rank_{w}'] = sc.rolling(w, min_periods=w).rank(pct=True, method='max').values
        mnL = sl.rolling(w, min_periods=1).min().values; mxH = sh.rolling(w, min_periods=1).max().values
        res[f'rsv_{w}'] = (cl - mnL) / (mxH - mnL)
        res[f'corr_{w}'] = sc.rolling(w, min_periods=w).corr(logv).values
        res[f'cord_{w}'] = _p(cl / _c1).rolling(w, min_periods=w).corr(_p(np.log(v / _sh(v, 1) + 1))).values
        res[f'vma_{w}'] = sv.rolling(w, min_periods=1).mean().values / v
        res[f'vstd_{w}'] = sv.rolling(w, min_periods=1).std(ddof=0).values / v
        res[f'wvma_{w}'] = _p(wv).rolling(w, min_periods=1).std(ddof=0).values / _p(wv).rolling(w, min_periods=1).mean().values
        dlt = cl - _c1
        pos = np.maximum(dlt, 0); neg = np.maximum(-dlt, 0); tot = np.abs(dlt)
        res[f'sump_{w}'] = _p(pos).rolling(w, min_periods=w).sum().values / _p(tot).rolling(w, min_periods=w).sum().values
        res[f'sumn_{w}'] = _p(neg).rolling(w, min_periods=w).sum().values / _p(tot).rolling(w, min_periods=w).sum().values
        res[f'sumd_{w}'] = res[f'sump_{w}'] - res[f'sumn_{w}']
        dvlt = v - _sh(v, 1)
        vpos = np.maximum(dvlt, 0); vneg = np.maximum(-dvlt, 0); vtot = np.abs(dvlt)
        res[f'vsump_{w}'] = _p(vpos).rolling(w, min_periods=w).sum().values / _p(vtot).rolling(w, min_periods=w).sum().values
        res[f'vsumn_{w}'] = _p(vneg).rolling(w, min_periods=w).sum().values / _p(vtot).rolling(w, min_periods=w).sum().values
        res[f'vsumd_{w}'] = res[f'vsump_{w}'] - res[f'vsumn_{w}']
    res['fr_1d'] = _p(fr).rolling(1, min_periods=1).mean().values
    res['fr_7d'] = _p(fr).rolling(7, min_periods=1).mean().values
    res['prem_1d'] = _p(prem).rolling(1, min_periods=1).mean().values
    res['oi_chg_1d'] = oi / _sh(oi, 1) - 1
    res['oi_chg_7d'] = oi / _sh(oi, 7) - 1
    res['tc_ratio'] = nc / snc.rolling(7, min_periods=1).mean().values
    res['log_dv'] = np.log(_p(cl * v).rolling(7, min_periods=1).mean().values)
    res['log_price'] = np.log(cl)
    # 存回矩阵（按该币的时间索引）
    j = coin_to_j[coin]
    ts_list = cv['ts']
    idx_arr = np.array([ts_to_i[t] for t in ts_list if t in ts_to_i])
    valid_ts = [t for t in ts_list if t in ts_to_i]
    for name, arr in res.items():
        if name not in fi_map:
            continue
        fi = fi_map[name]
        a = np.asarray(arr, dtype=np.float64)
        a = np.where(np.isfinite(a), a, np.nan)
        X[idx_arr, j, fi] = a
    if (ci + 1) % 20 == 0:
        log(f"  币 {ci+1}/{len(univ)}")
    del res
    gc.collect()
log(f"矩阵 {X.shape}")

# ===== 6. 标签 pct_cs（polars）=====
df2 = df.with_columns((ts(c.shift(-3)) / c - 1).alias('fwd'))
df2 = df2.with_columns(pl.when(pl.col('fwd').is_finite()).then(pl.col('fwd')).otherwise(None).alias('fwd_m'))
memb_l = []
for dt0, coin in zip(df2['datetime'].to_list(), df2['coin'].to_list()):
    i = ts_to_i.get(dt0); j = coin_to_j.get(coin)
    memb_l.append(bool(members[i, j]) if (i is not None and j is not None) else False)
df2 = df2.with_columns(pl.Series('member', memb_l))
df2 = df2.with_columns((pl.col('fwd_m').rank().over('datetime') / pl.col('fwd_m').count().over('datetime')).alias('label_raw'))
df2 = df2.with_columns(pl.when(pl.col('member')).then(pl.col('label_raw')).otherwise(None).alias('label'))
labels = np.full((N, k), np.nan, dtype=np.float64)
_dts2 = df2['datetime'].to_list(); _coins2 = df2['coin'].to_list()
_i_arr = np.array([ts_to_i[t] for t in _dts2])
_j_arr = np.array([coin_to_j[c_] for c_ in _coins2])
lab_arr = df2['label'].to_numpy().astype(np.float64)
labels[_i_arr, _j_arr] = np.where(np.isfinite(lab_arr), lab_arr, np.nan)
log(f"标签覆盖 {int((np.isfinite(labels).sum(0)>0).sum())}/{k} 币")

# ===== 7. 保存 =====
ts_arr = np.array([int(t.replace(tzinfo=datetime.timezone.utc).timestamp() * 1000) for t in all_ts])
np.savez_compressed(str(BASE / 'features.npz'), ts=ts_arr, coins=np.array(univ), labels=labels, members=members, **{f'F_{name}': X[:, :, fi] for fi, name in enumerate(feat_names)})
log("已保存 features.npz")
