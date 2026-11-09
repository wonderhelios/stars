"""方向2: 信号强度加权（等权 -> 按动量分/量冲击分加权）

当前: 每个因子选 top-kk/bottom-kk, 腿内【等权】。
新: 腿内按【排名】或【z-score】加权（信号越强, 权重越大）。

保持其余不变: 两因子各选 top-kk/bottom-kk, 两个带符号向量求和, 归一化 Σ|w|=1。
"""
import json, glob, warnings
import numpy as np, pandas as pd
from pathlib import Path
warnings.filterwarnings('ignore', category=RuntimeWarning)

RET = Path('/Users/wonder/Code/stars/research/trailing_tp_20261008/retrieval')
req = json.load(open(RET/'request.json'))
end = req['end_ms']
meta = {x['name']: x for x in json.load(open(RET/'meta.json'))['universe']}
files = sorted(glob.glob('/tmp/hl-daily-v2/*.json'))
names = [Path(f).stem for f in files]
raw = {Path(f).stem: [x for x in json.load(open(f)) if req['daily_start_ms'] <= x['t'] and x['T'] < end and float(x['c']) > 0] for f in files}
ts = sorted({x['t'] for d in raw.values() for x in d})
ti = {t: i for i, t in enumerate(ts)}
n = len(ts); k = len(names)

O = np.full((n, k), np.nan); P = np.full((n, k), np.nan); Q = np.full((n, k), np.nan)
for j, c in enumerate(names):
    for x in raw[c]:
        i = ti[x['t']]; O[i, j] = float(x['o']); P[i, j] = float(x['c']); Q[i, j] = float(x['c']) * float(x['v'])

R = P / np.roll(P, 1, axis=0) - 1
V30 = pd.DataFrame(Q).rolling(30, min_periods=5).mean().shift(1).values
SD20 = pd.DataFrame(R).rolling(20, min_periods=5).std(ddof=1).values
M14 = P / np.roll(P, 14, axis=0) - 1
NOTCOL = np.array([':' not in c for c in names])
MINV = 1e7; CAP = 10; TF = 0.2; FEE = 0.0007; GROSS = 2.7; BAND = 0.02

def target(i, mode):
    w = np.zeros(k)
    ok = (V30[i - 1] >= MINV) & np.isfinite(P[i - 1]) & np.isfinite(P[i - 15]) & np.isfinite(SD20[i - 1]) & NOTCOL
    idx = np.flatnonzero(ok)
    if len(idx) < 8:
        return w
    c2 = min(CAP, 10); kk = min(max(1, int(np.floor(len(idx) * TF + .5))), c2, len(idx) // 2)
    for sc in [M14[i - 1] / np.maximum(SD20[i - 1], 1e-9), Q[i - 1] / V30[i - 1]]:
        o = idx[np.lexsort((np.array(names)[idx], sc[idx]))]   # 升序
        short = o[:kk]; long = o[-kk:]
        if mode == 'equal':
            w[short] -= .5 / kk / 2
            w[long] += .5 / kk / 2
        elif mode == 'rank':
            # 排名线性: 腿内第 p 名(p=1..kk, 最差=1, 最好=kk) 权重 ∝ p
            denom = kk * (kk + 1) / 2
            p = np.arange(1, kk + 1)  # 升序里越靠后越强
            w[short] -= (p / denom) * .5 / 2          # 空头腿: 最差(=1)权重最大? 不, 空头也是"越极端越大"
            w[long] += (p[::-1] / denom) * .5 / 2     # 多头腿: 最强最大
        elif mode == 'zscore':
            s = sc[idx]
            m = np.nanmean(s); sd = np.nanstd(s)
            z = (s - m) / sd if sd > 0 else np.zeros_like(s)
            z_long = np.clip(z[-kk:], 0, None); z_short = np.clip(-z[:kk], 0, None)
            s_long = z_long.sum(); s_short = z_short.sum()
            if s_long > 0:
                w[long] += (z_long / s_long) * .5 / 2
            if s_short > 0:
                w[short] -= (z_short / s_short) * .5 / 2
    g = np.abs(w).sum()
    return w / g if g > 0 else w

def run(mode, start=40):
    eq = 1e6; qty = np.zeros(k); last = np.full(k, np.nan)
    series = []; turns = []
    for i in range(start, n):
        o = O[i]; mo = np.where(np.isfinite(o), o, last)
        eq += np.nansum(qty * (mo - last))
        if eq <= 0:
            return None
        w = target(i, mode)
        d = np.zeros(k)
        for j in range(k):
            if not np.isfinite(o[j]):
                continue
            f = 10 ** meta[names[j]]['szDecimals']
            if abs(w[j]) < 1e-12:
                if abs(qty[j]) * o[j] >= 10:
                    d[j] = -qty[j]
            else:
                tgt = np.sign(w[j]) * np.floor((abs(w[j]) * eq * GROSS / o[j]) * f) / f
                dd = tgt - qty[j]
                if abs(dd) * o[j] >= max(10, abs(tgt) * o[j] * BAND):
                    d[j] = dd
        ntl = float(np.sum(np.abs(d) * np.nan_to_num(o)))
        eq -= FEE * ntl
        turns.append(ntl / max(eq, 1))
        qty += d
        last = np.where(np.isfinite(P[i]), P[i], last)
        eq += np.nansum(qty * (last - mo))
        series.append(eq)
    s = np.array(series)
    r = np.diff(s) / s[:-1]
    cv = s / s[0]
    SH = float(np.mean(r) / np.std(r, ddof=1) * np.sqrt(365))
    return dict(sharpe=SH, annual=float(np.mean(r) * 365),
                mdd=float((cv / np.maximum.accumulate(cv) - 1).min()),
                turn=float(np.mean(turns)), cost=float(np.mean(turns) * FEE * 365),
                rr=r, n=len(r))

print('=== 方向2: 信号强度加权（2023-06 → 2026-10）===\n')
print(f'{"方案":<26}{"Sharpe":>9}{"年化":>9}{"回撤":>9}{"换手":>8}{"成本/年":>9}')
print('-' * 74)
base = run('equal')
print(f'{"等权（现役）":<26}{base["sharpe"]:>9.3f}{base["annual"]*100:>8.1f}%{base["mdd"]*100:>8.1f}%{base["turn"]:>8.3f}{base["cost"]*100:>8.1f}%')
res = {'base': base}
for mode, lab in [('rank', '排名加权（线性）'), ('zscore', 'z-score 加权')]:
    o = run(mode)
    if not o:
        print(f'{lab:<26} 失败')
        continue
    res[mode] = o
    print(f'{lab:<26}{o["sharpe"]:>9.3f}{o["annual"]*100:>8.1f}%{o["mdd"]*100:>8.1f}%{o["turn"]:>8.3f}{o["cost"]*100:>8.1f}%')

print()
print('=== 相对基线 ===')
for mode in res:
    if mode == 'base':
        continue
    o = res[mode]
    print(f'  {mode:<8} Sharpe {o["sharpe"]-base["sharpe"]:+.3f} · 年化 {(o["annual"]-base["annual"])*100:+.1f}pp · 回撤 {(o["mdd"]-base["mdd"])*100:+.1f}pp · 换手 {o["turn"]/base["turn"]:.2f}x')

# 子区间
print()
print('=== 子区间 Sharpe ===')
yrs = pd.to_datetime(ts[41:41 + base['n']], unit='ms').year.values
for lab, m in [('2023-24', yrs <= 2024), ('2025-26', yrs >= 2025)]:
    row = f'  {lab}: '
    for mode in res:
        if m.sum() >= 60:
            row += f'{mode} {np.mean(res[mode]["rr"][m])/np.std(res[mode]["rr"][m],ddof=1)*np.sqrt(365):.3f}   '
    print(row)
