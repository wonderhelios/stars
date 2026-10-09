"""把 baseline / channel20 的净值曲线重跑出来并画图。不改任何策略逻辑，只调用 run.py 的引擎。"""
import numpy as np, json, sys
from pathlib import Path
HERE = Path(__file__).resolve().parent
src = (HERE/'run.py').read_text()
# 只取 main() 之前的定义，避免触发它的批处理
exec(compile(src.split('def main()')[0], 'run.py', 'exec'), globals())
B = globals()['B']; ORIGINAL_TARGET = globals()['ORIGINAL_TARGET']; target_factory = globals()['target_factory']

curves = {}
for factor, mode, k, p in [('baseline','mix',1,0), ('channel20','mix',3,0), ('channel20','mix',1,0)]:
    key = f'{factor}_{mode}_k{k}_p{p}'
    B.target = ORIGINAL_TARGET if factor == 'baseline' else target_factory(factor, mode)
    B.PERIOD = k; B.PHASE = p; B.RUN_ID = key + '_curve'
    run = B.run
    # 让引擎把 r/t/turn 交回来：它本来就会存 npz，这里换个目录
    res = run(5, 'open', .0007)
    d = np.load(globals()['SAVE']/ (key + '_curve.npz')) if (globals()['SAVE']/(key+'_curve.npz')).exists() else None
    if d is None:
        # 引擎存在 OUT（本目录）下
        cand = list(HERE.glob('*_curve.npz'))
        d = np.load(cand[0]) if cand else None
    if d is not None and 'r' in d:
        curves[key] = (d['r'], d['t'], d['turn'])
        r = d['r']; to = d['turn']
        eq = np.cumprod(1+r); peak = np.maximum.accumulate(eq)
        print(f"  {key:<26} Sharpe {r.mean()/r.std(ddof=1)*np.sqrt(365):.4f}  年化 {r.mean()*365*100:6.1f}%  "
              f"最终 {eq[-1]:5.2f}x  回撤 {(eq/peak-1).min()*100:6.1f}%  换手 {to.mean():.4f}", flush=True)
    else:
        print(f"  {key}: 拿不到曲线（引擎没存 r）", flush=True)
np.savez_compressed(HERE/'curves_for_plot.npz', **{f'{k}_{i}': v[i] for k,v in curves.items() for i in range(3)})
json.dump({k: len(v[0]) for k,v in curves.items()}, open(HERE/'curves_index.json','w'), indent=1)
