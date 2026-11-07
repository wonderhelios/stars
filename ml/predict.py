"""ML 策略预测 + 选币。

加载 model.json（LightGBM 树结构）+ features.npz（特征矩阵），
取最新一天的 head 成分币，预测 score，输出前 6 做多、后 6 做空的目标持仓。

用法：
    python predict.py            # 打印最新一天的目标持仓
    python predict.py --json     # 输出 JSON（给下单/监控用）
"""
from pathlib import Path
BASE = Path(__file__).resolve().parent
import json
import numpy as np
import pandas as pd
import datetime

LONG_N = 6   # 做多数量
SHORT_N = 6  # 做空数量
LEVERAGE = 2.0


def load_model(path=None):
    """加载导出的模型（树结构）。"""
    path = Path(path or BASE / 'model.json')
    return json.load(open(path))


def predict_score(model, x):
    """纯 Python 推理：遍历每棵树，累加叶子值。缺失用 NaN。"""
    s = 0.0
    for nodes in model['trees']:
        idx = 0
        while 'leaf' not in nodes[idx]:
            n = nodes[idx]
            fv = x[n['feat']]
            if np.isnan(fv):
                idx = n['right'] if n['miss'] == 0 else n['left']
            else:
                idx = n['left'] if fv <= n['thr'] else n['right']
        s += nodes[idx]['leaf']
    return s


def latest_positions():
    """返回最新一天的目标持仓：{'long': [...], 'short': [...], 'date': 'YYYY-MM-DD'}。"""
    model = load_model()
    d = np.load(str(BASE / 'features.npz'))
    ts = d['ts']
    coins = d['coins']
    members = d['members']
    feat_names = [f for f in d.files if f.startswith('F_')]

    # 最新一天
    i = len(ts) - 1
    hm = members[i].astype(bool)

    # 算最新一天每个币的 151 个特征
    feats = np.column_stack([d[f][i, :] for f in feat_names])

    # 预测 head 成分币的 score
    scores = np.full(len(coins), np.nan)
    for j in range(len(coins)):
        if not hm[j]:
            continue
        x = feats[j]
        # 特征全 NaN 的币跳过
        scores[j] = predict_score(model, x)

    # 排序：score 高 → 做多，score 低 → 做空
    idx = np.flatnonzero(np.isfinite(scores))
    order = idx[np.lexsort((coins[idx], np.nan_to_num(scores[idx])))]

    long_coins = [str(coins[j]) for j in order[-LONG_N:]]
    short_coins = [str(coins[j]) for j in order[:SHORT_N]]

    date = datetime.datetime.utcfromtimestamp(ts[i] / 1000).strftime('%Y-%m-%d')
    return {
        'date': date,
        'long': long_coins,
        'short': short_coins,
        'n_universe': int(hm.sum()),
    }


def main():
    import sys
    r = latest_positions()
    if '--json' in sys.argv:
        print(json.dumps(r, ensure_ascii=False))
    else:
        print(f"=== ML 策略目标持仓（{r['date']} ✗ head {r['n_universe']} 币）===")
        print(f"做多 {LONG_N}: {', '.join(r['long'])}")
        print(f"做空 {SHORT_N}: {', '.join(r['short'])}")


if __name__ == '__main__':
    main()
