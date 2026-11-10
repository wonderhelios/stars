"""v4: 真正的 polars 版（和 alphapilot 同口径 ✗ pl.rolling_* Rust 实现 ✗ 内存小）
全 235 币 + 动态 head(30天成交额前20% ✗ 18/22 缓冲 ✗ D 日成分用 D-2 数据) + 标签 pct_cs(forward_3d) 在 head 内
"""
import json, glob, datetime, warnings, gc
import numpy as np
import pandas as pd
import polars as pl
from pathlib import Path
warnings.filterwarnings('ignore')
BASE = Path(__file__).resolve().parent
def log(m): print(m, flush=True)
WINDOWS = (5, 10, 20, 30, 60)

# ===== 1. 加载 hl2 长表 =====
files = glob.glob(str(BASE / 'data' / 'hl2' / '*.json'))
univ = sorted(Path(f).stem for f in files)
raw = {Path(f).stem: json.load(open(f)) for f in files}
log(f"全宇宙 {len(univ)} 币")
rows = []
for _coin in univ:
    bars = raw[_coin]
    start = None
    for bi, x in enumerate(bars):
        if float(x['v']) > 0:
            start = bi; break
    if start is None: continue
    for x in bars[start:]:
        t = datetime.datetime.utcfromtimestamp(int(x['t']) / 1000)
        rows.append((t, _coin, float(x['o']), float(x['h']), float(x['l']), float(x['c']), float(x['v']), float(x['n'])))
del raw; gc.collect()
df = pl.DataFrame(rows, schema={'datetime': pl.Datetime, 'coin': pl.String, 'open': pl.Float64, 'high': pl.Float64, 'low': pl.Float64, 'close': pl.Float64, 'volume': pl.Float64, 'trade_count': pl.Float64}, orient='row').sort('coin', 'datetime')
del rows; gc.collect()
log(f"长表 {df.height} 行")

# ===== 2. funding + OI =====
fr_maps, prem_maps = {}, {}
for _coin in univ:
    fp = BASE / 'data' / 'hl_funding' / f'{_coin}.json'
    if not fp.exists(): continue
    fd = json.load(open(fp)); fr, pm = {}, {}
    for dd, v in fd.items():
        if isinstance(v, dict):
            if v.get('fr') is not None: fr[dd] = v['fr']
            if v.get('prem') is not None: pm[dd] = v['prem']
        else: fr[dd] = v
    fr_maps[_coin] = fr; prem_maps[_coin] = pm
oi_map = {}
oi_files = glob.glob(str(BASE / 'data' / 'hl_oi' / '*.parquet'))
if oi_files:
    oidf = pd.concat([pd.read_parquet(f) for f in oi_files])
    oidf['date'] = pd.to_datetime(oidf['datetime']).dt.strftime('%Y-%m-%d')
    for _, r in oidf.groupby(['date','coin'])['open_interest'].last().reset_index().iterrows():
        oi_map[(r['coin'], r['date'])] = float(r['open_interest'])
del oidf; gc.collect()
df = df.with_columns(pl.col('datetime').dt.strftime('%Y-%m-%d').alias('ds'))
fr_l, prem_l, oi_l = [], [], []
for _c, ds in zip(df['coin'].to_list(), df['ds'].to_list()):
    fr_l.append(fr_maps.get(_c, {}).get(ds)); prem_l.append(prem_maps.get(_c, {}).get(ds)); oi_l.append(oi_map.get((_c, ds)))
del fr_maps, prem_maps, oi_map; gc.collect()
df = df.with_columns(pl.Series('fr', fr_l, dtype=pl.Float64), pl.Series('prem', prem_l, dtype=pl.Float64), pl.Series('oi', oi_l, dtype=pl.Float64))
del fr_l, prem_l, oi_l; gc.collect()
log(f"funding {int(df['fr'].is_not_null().sum())} 行, OI {int(df['oi'].is_not_null().sum())} 行")

# ===== 3. adv + head =====
df = df.with_columns((pl.col('close') * pl.col('volume')).rolling_mean(30).over('coin').alias('adv'))
all_ts = sorted(df['datetime'].unique().to_list())
ts_to_i = {t: i for i, t in enumerate(all_ts)}
coin_to_j = {c: j for j, c in enumerate(univ)}
k = len(univ); N = len(all_ts)
prev = None; members = np.zeros((N, k), bool)
for dt0 in all_ts:
    day = df.filter((pl.col('datetime') == dt0) & pl.col('adv').is_finite())
    if day.height == 0 and prev is None: continue
    s = day.sort(['adv', 'coin'], descending=[True, False])
    coins_sorted = s['coin'].to_list()
    eff = dt0 + datetime.timedelta(days=2)
    if eff not in ts_to_i: continue
    if not coins_sorted and prev is None: continue
    Nc = len(coins_sorted); cur = set()
    for pos, coin in enumerate(coins_sorted):
        upper = 20.0 if prev is None else (22.0 if coin in prev else 18.0)
        if (pos + 1) * 100 <= upper * Nc: cur.add(coin)
    prev = cur
    i = ts_to_i[eff]
    for coin in cur:
        if coin in coin_to_j: members[i, coin_to_j[coin]] = True
log(f"head 成员: 每天平均 {members.sum(1).mean():.1f} 币, 历史 {int((members.sum(0)>0).sum())} 币进过")

# ===== 4. 151 特征（polars 表达式 ✗ 和 alphapilot operators.py 同口径）=====
def ts_(e): return e.over('coin', order_by='datetime')
o, h, l, c, v, nc = pl.col('open'), pl.col('high'), pl.col('low'), pl.col('close'), pl.col('volume'), pl.col('trade_count')
feats = {}
feats['kmid'] = (c - o) / o; feats['klen'] = (h - l) / o; feats['kmid_2'] = (c - o) / (h - l)
feats['kup'] = (h - pl.max_horizontal(o, c)) / o; feats['kup_2'] = (h - pl.max_horizontal(o, c)) / (h - l)
feats['klow'] = (pl.min_horizontal(o, c) - l) / o; feats['klow_2'] = (pl.min_horizontal(o, c) - l) / (h - l)
feats['ksft'] = (c * 2 - h - l) / o; feats['ksft_2'] = (c * 2 - h - l) / (h - l)
feats['open_0'] = o / c; feats['high_0'] = h / c; feats['low_0'] = l / c
feats['vwap_0'] = pl.lit(None, pl.Float64)
for w in WINDOWS:
    feats[f'roc_{w}'] = ts_(c.shift(w)) / c
    feats[f'ma_{w}'] = ts_(c.rolling_mean(w, min_samples=1)) / c
    feats[f'cntp_{w}'] = ts_((c > ts_(c.shift(1))).cast(pl.Float64).rolling_mean(w, min_samples=1))
    feats[f'cntn_{w}'] = ts_((c < ts_(c.shift(1))).cast(pl.Float64).rolling_mean(w, min_samples=1))
    feats[f'cntd_{w}'] = feats[f'cntp_{w}'] - feats[f'cntn_{w}']
    feats[f'std_{w}'] = ts_(c.rolling_std(w, min_samples=1, ddof=0)) / c
    feats[f'max_{w}'] = ts_(h.rolling_max(w, min_samples=1)) / c
    feats[f'min_{w}'] = ts_(l.rolling_min(w, min_samples=1)) / c
    feats[f'qtlu_{w}'] = ts_(c.rolling_quantile(0.8, interpolation='linear', window_size=w)) / c
    feats[f'qtld_{w}'] = ts_(c.rolling_quantile(0.2, interpolation='linear', window_size=w)) / c
    imax = ts_(h.rolling_map(lambda s: s.arg_max() + 1, window_size=w, min_samples=w))
    imin = ts_(l.rolling_map(lambda s: s.arg_min() + 1, window_size=w, min_samples=w))
    feats[f'imax_{w}'] = imax / w; feats[f'imin_{w}'] = imin / w; feats[f'imxd_{w}'] = (imax - imin) / w
    feats[f'rank_{w}'] = ts_(c.rolling_map(lambda s: (s <= s[-1]).sum() / len(s), window_size=w, min_samples=w))
    mnL = ts_(l.rolling_min(w, min_samples=1)); mxH = ts_(h.rolling_max(w, min_samples=1))
    feats[f'rsv_{w}'] = (c - mnL) / (mxH - mnL)
    feats[f'corr_{w}'] = ts_(pl.rolling_corr(c, v.log(), window_size=w, min_samples=w))
    feats[f'cord_{w}'] = ts_(pl.rolling_corr(c / ts_(c.shift(1)), (v / ts_(v.shift(1)) + 1).log(), window_size=w, min_samples=w))
    feats[f'vma_{w}'] = ts_(v.rolling_mean(w, min_samples=1)) / v
    feats[f'vstd_{w}'] = ts_(v.rolling_std(w, min_samples=1, ddof=0)) / v
    wv = (c / ts_(c.shift(1)) - 1).abs() * v
    feats[f'wvma_{w}'] = ts_(wv.rolling_std(w, min_samples=1, ddof=0)) / ts_(wv.rolling_mean(w, min_samples=1))
    dlt = c - ts_(c.shift(1))
    feats[f'sump_{w}'] = ts_(pl.max_horizontal(dlt, 0).rolling_sum(w, min_samples=w)) / ts_(dlt.abs().rolling_sum(w, min_samples=w))
    feats[f'sumn_{w}'] = ts_(pl.max_horizontal(-dlt, 0).rolling_sum(w, min_samples=w)) / ts_(dlt.abs().rolling_sum(w, min_samples=w))
    feats[f'sumd_{w}'] = feats[f'sump_{w}'] - feats[f'sumn_{w}']
    dvlt = v - ts_(v.shift(1))
    feats[f'vsump_{w}'] = ts_(pl.max_horizontal(dvlt, 0).rolling_sum(w, min_samples=w)) / ts_(dvlt.abs().rolling_sum(w, min_samples=w))
    feats[f'vsumn_{w}'] = ts_(pl.max_horizontal(-dvlt, 0).rolling_sum(w, min_samples=w)) / ts_(dvlt.abs().rolling_sum(w, min_samples=w))
    feats[f'vsumd_{w}'] = feats[f'vsump_{w}'] - feats[f'vsumn_{w}']
    log(f"  特征窗口 {w} 完成（累计 {len(feats)} 个）")
feats['fr_1d'] = ts_(pl.col('fr').rolling_mean(1, min_samples=1))
feats['fr_7d'] = ts_(pl.col('fr').rolling_mean(7, min_samples=1))
feats['prem_1d'] = ts_(pl.col('prem').rolling_mean(1, min_samples=1))
feats['oi_chg_1d'] = pl.col('oi') / ts_(pl.col('oi').shift(1)) - 1
feats['oi_chg_7d'] = pl.col('oi') / ts_(pl.col('oi').shift(7)) - 1
feats['tc_ratio'] = nc / ts_(nc.rolling_mean(7, min_samples=1))
feats['log_dv'] = ts_((c * v).rolling_mean(7, min_samples=1)).log()
feats['log_price'] = c.log()
log(f"特征 {len(feats)} 个")

# ===== 5. 分批算（每批 20 个 ✗ 内存小）→ 转 (n,k) 矩阵 =====
log("计算 151 特征（polars ✗ 分批 20 个一批 ✗ 内存小）…")
feat_names = list(feats.keys())
X = np.full((N, k, len(feat_names)), np.nan, dtype=np.float32)
dts = df['datetime'].to_list(); coins = df['coin'].to_list()
_i_arr = np.array([ts_to_i[t] for t in dts])
_j_arr = np.array([coin_to_j[c_] for c_ in coins])
BATCH = 20
for start in range(0, len(feat_names), BATCH):
    batch = feat_names[start:start + BATCH]
    dfx = df.with_columns([feats[name].alias(name) for name in batch])
    for off, name in enumerate(batch):
        col = dfx[name].to_numpy().astype(np.float64)
        X[_i_arr, _j_arr, start + off] = np.where(np.isfinite(col), col, np.nan)
    del dfx; gc.collect()
    log(f"  特征批次 {start + len(batch)}/{len(feat_names)} 完成")
log(f"矩阵 {X.shape}")

# ===== 6. 标签 pct_cs =====
df2 = df.with_columns((ts_(c.shift(-3)) / c - 1).alias('fwd'))
df2 = df2.with_columns(pl.when(pl.col('fwd').is_finite()).then(pl.col('fwd')).otherwise(None).alias('fwd_m'))
memb_l = []
for dt0, coin in zip(df2['datetime'].to_list(), df2['coin'].to_list()):
    i = ts_to_i.get(dt0); j = coin_to_j.get(coin)
    memb_l.append(bool(members[i, j]) if (i is not None and j is not None) else False)
df2 = df2.with_columns(pl.Series('member', memb_l))
df2 = df2.with_columns((pl.col('fwd_m').rank().over('datetime') / pl.col('fwd_m').count().over('datetime')).alias('label_raw'))
df2 = df2.with_columns(pl.when(pl.col('member')).then(pl.col('label_raw')).otherwise(None).alias('label'))
labels = np.full((N, k), np.nan, dtype=np.float64)
lab_arr = df2['label'].to_numpy().astype(np.float64)
labels[_i_arr, _j_arr] = np.where(np.isfinite(lab_arr), lab_arr, np.nan)
log(f"标签覆盖 {int((np.isfinite(labels).sum(0)>0).sum())}/{k} 币")

# ===== 7. 保存 =====
ts_arr = np.array([int(t.replace(tzinfo=datetime.timezone.utc).timestamp() * 1000) for t in all_ts])
np.savez_compressed(str(BASE / 'features.npz'), ts=ts_arr, coins=np.array(univ), labels=labels, members=members, **{f'F_{name}': X[:, :, fi] for fi, name in enumerate(feat_names)})
log("已保存 features.npz")
