"""方向1: 成本优先的择时门 —— 回测

核心想法: 每笔调仓的期望收益 ≈ 截面离散度 × IC − 成本。
离散度低的日子, 期望价差可能覆盖不了 7bp 成本, 却仍然付成本进去交易。

实现:
- 离散度 disp[i] = 滚动 N 日的「每日截面收益标准差」的均值（截面 = 过 $10M 门槛的币）
  daily_cs_std[i] = std over coins( return[i] )
  disp[i] = mean(daily_cs_std[i-N+1 .. i])
- 门控 gate: disp[i] < 分位阈值(尾部/扩展窗口) 时, 当天不下单（保留持仓, 省换手）
- 缩放 scale: gross 按 disp/median(disp) 缩放（0.5~2 倍, clamp）

只允许用 i-1 及之前的信息（disp 用 i-1 时刻的值, 避免未来函数）。
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

# ---- 离散度序列（截面 = 过 $10M 门槛的币, 用前一日值, 无未来函数）----
def universe_mask(i):
    return (V30[i] >= MINV) & np.isfinite(P[i]) & np.isfinite(P[i - 1]) & NOTCOL

daily_cs_std = np.full(n, np.nan)
for i in range(1, n):
    m = universe_mask(i)
    if m.sum() >= 8:
        daily_cs_std[i] = np.nanstd(R[i][m])

disp = np.full(n, np.nan)
for i in range(1, n):
    w = daily_cs_std[max(0, i - 9):i + 1]
    w = w[np.isfinite(w)]
    if len(w) >= 5:
        disp[i] = np.mean(w)

disp_median = np.nanmedian(disp)

# ---- 目标权重（两因子, 与现役一致）----
def target(i):
    w = np.zeros(k)
    ok = (V30[i - 1] >= MINV) & np.isfinite(P[i - 1]) & np.isfinite(P[i - 15]) & np.isfinite(SD20[i - 1]) & NOTCOL
    idx = np.flatnonzero(ok)
    if len(idx) < 8:
        return w
    c2 = min(CAP, 10); kk = min(max(1, int(np.floor(len(idx) * TF + .5))), c2, len(idx) // 2)
    for sc in [M14[i - 1] / np.maximum(SD20[i - 1], 1e-9), Q[i - 1] / V30[i - 1]]:
        o = idx[np.lexsort((np.array(names)[idx], sc[idx]))]
        w[o[:kk]] -= .5 / kk / 2
        w[o[-kk:]] += .5 / kk / 2
    g = np.abs(w).sum()
    return w / g if g > 0 else w

# ---- 引擎（mode='A' 每日全量）----
def run(gate_quantile=None, scale=False, start=40):
    eq = 1e6; qty = np.zeros(k); last = np.full(k, np.nan)
    series = []; turns = []
    # 尾部扩展分位（只用过去数据）
    hist = []
    for i in range(start, n):
        o = O[i]; mo = np.where(np.isfinite(o), o, last)
        eq += np.nansum(qty * (mo - last))
        if eq <= 0:
            return None
        w = target(i)
        # 判断今天要不要交易
        trade_today = True
        if gate_quantile is not None:
            if len(hist) >= 60 and np.isfinite(disp[i - 1]):
                thresh = np.percentile(hist, gate_quantile * 100)
                trade_today = disp[i - 1] >= thresh
            elif len(hist) >= 60:
                trade_today = False
        mult = 1.0
        if scale and np.isfinite(disp[i - 1]) and disp_median > 0:
            mult = float(np.clip(disp[i - 1] / disp_median, 0.5, 2.0))
        d = np.zeros(k)
        if trade_today:
            for j in range(k):
                if not np.isfinite(o[j]):
                    continue
                f = 10 ** meta[names[j]]['szDecimals']
                if abs(w[j]) < 1e-12:
                    if abs(qty[j]) * o[j] >= 10:
                        d[j] = -qty[j]
                else:
                    tgt = np.sign(w[j]) * np.floor((abs(w[j]) * eq * GROSS * mult / o[j]) * f) / f
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
        if np.isfinite(disp[i - 1]):
            hist.append(disp[i - 1])
    s = np.array(series)
    r = np.diff(s) / s[:-1]
    cv = s / s[0]
    SH = float(np.mean(r) / np.std(r, ddof=1) * np.sqrt(365))
    return dict(sharpe=SH, annual=float(np.mean(r) * 365),
                mdd=float((cv / np.maximum.accumulate(cv) - 1).min()),
                turn=float(np.mean(turns)), cost=float(np.mean(turns) * FEE * 365),
                rr=r, n=len(r))

print('=== 方向1: 成本门控 / 离散度缩放（2023-06 → 2026-10）===')
print(f'离散度 disp: 中位 {disp_median * 100:.2f}%  范围 [{np.nanpercentile(disp,5)*100:.2f}%, {np.nanpercentile(disp,95)*100:.2f}%]\n')

base = run()
print(f'{"方案":<30}{"Sharpe":>9}{"年化":>9}{"回撤":>9}{"换手":>8}{"成本/年":>9}')
print('-' * 76)
print(f'{"基线（每日全量）":<30}{base["sharpe"]:>9.3f}{base["annual"]*100:>8.1f}%{base["mdd"]*100:>8.1f}%{base["turn"]:>8.3f}{base["cost"]*100:>8.1f}%')

res = {'base': base}
for q in [0.1, 0.2, 0.3, 0.5]:
    o = run(gate_quantile=q)
    if not o:
        print(f'{"门控 分位 " + str(q):<30} 失败')
        continue
    res[f'gate{q}'] = o
    print(f'{"门控 分位 " + str(q):<30}{o["sharpe"]:>9.3f}{o["annual"]*100:>8.1f}%{o["mdd"]*100:>8.1f}%{o["turn"]:>8.3f}{o["cost"]*100:>8.1f}%')

o = run(scale=True)
if o:
    res['scale'] = o
    print(f'{"缩放 gross(0.5~2x)":<30}{o["sharpe"]:>9.3f}{o["annual"]*100:>8.1f}%{o["mdd"]*100:>8.1f}%{o["turn"]:>8.3f}{o["cost"]*100:>8.1f}%')

print()
print('=== 相对基线 ===')
for key in res:
    if key == 'base':
        continue
    o = res[key]
    print(f'  {key:<10} Sharpe {o["sharpe"]-base["sharpe"]:+.3f} · 年化 {(o["annual"]-base["annual"])*100:+.1f}pp · 回撤 {(o["mdd"]-base["mdd"])*100:+.1f}pp · 换手 {o["turn"]/base["turn"]:.2f}x')
