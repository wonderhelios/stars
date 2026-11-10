"""四周期 LightGBM 集成的共享标签、推理和固定杠杆选币逻辑。"""
from pathlib import Path
import datetime
import json
import math

import numpy as np
import pandas as pd

BASE = Path(__file__).resolve().parent
DEFAULT_CONFIG_PATH = BASE / "strategy_config.json"
DEFAULT_MODEL_PATH = BASE / "model_ensemble.json"


def load_config(path=None):
    with open(path or DEFAULT_CONFIG_PATH, encoding="utf-8") as f:
        config = json.load(f)
    if config.get("format_version") != 2:
        raise ValueError("不支持的 ML 策略配置版本")
    if config.get("gross_leverage") != 2.0:
        raise ValueError("当前实现要求固定总杠杆 2 倍")
    if config.get("long_fraction") != 0.5:
        raise ValueError("当前实现要求多空各占一半名义仓位")
    return config


def load_model(path=None):
    """只加载四模型文件；不回退到历史单模型 model.json。"""
    with open(path or DEFAULT_MODEL_PATH, encoding="utf-8") as f:
        return json.load(f)


def component_maturity(name):
    """标签从信号日到可结算日所需的自然日数。"""
    if name.startswith("close"):
        return int(name[5:])
    if name.startswith("open"):
        return int(name[4:]) + 1
    raise ValueError("未知标签周期: %s" % name)


def cross_sectional_percentiles(forward, members):
    """逐日在当日 head 内计算平均并列秩百分位。"""
    forward = np.asarray(forward, dtype=float)
    members = np.asarray(members, dtype=bool)
    labels = np.full(forward.shape, np.nan)
    for i in range(len(forward)):
        idx = np.flatnonzero(members[i] & np.isfinite(forward[i]))
        if len(idx):
            labels[i, idx] = pd.Series(forward[i, idx]).rank(pct=True).to_numpy()
    return labels


def component_labels(name, open_prices, close_prices, members):
    """close_h: C[i+h]/C[i]-1；open_h: O[i+h+1]/O[i+1]-1。"""
    close_prices = np.asarray(close_prices, dtype=float)
    open_prices = np.asarray(open_prices, dtype=float)
    n, k = members.shape
    horizon = int(name[5:] if name.startswith("close") else name[4:])
    maturity = component_maturity(name)
    forward = np.full((n, k), np.nan)
    if name.startswith("close"):
        forward[:-horizon] = close_prices[horizon:] / close_prices[:-horizon] - 1.0
    elif name.startswith("open"):
        forward[:-maturity] = open_prices[maturity:] / open_prices[1:-horizon] - 1.0
    else:
        raise ValueError("未知标签周期: %s" % name)
    return cross_sectional_percentiles(forward, members)


def export_component(model, name):
    """将 LightGBM 的最佳迭代轮数导出为无 LightGBM 依赖的 JSON 树。"""
    best_iteration = int(model.best_iteration)
    dump = model.dump_model(num_iteration=best_iteration)
    trees = []
    for tree_info in dump["tree_info"]:
        nodes = []

        def walk(node):
            if "leaf_value" in node:
                idx = len(nodes)
                nodes.append({"leaf": float(node["leaf_value"])})
                return idx
            idx = len(nodes)
            if node.get("decision_type", "<=") != "<=":
                raise ValueError("暂不支持非数值 LightGBM 分裂: %s" % node.get("decision_type"))
            nodes.append({
                "feat": int(node["split_feature"]),
                "thr": float(node["threshold"]),
                "left": -1,
                "right": -1,
                "miss": 1 if node.get("default_left", True) else 0,
                "missing_type": node.get("missing_type", "NaN"),
            })
            left = walk(node["left_child"])
            right = walk(node["right_child"])
            nodes[idx]["left"] = left
            nodes[idx]["right"] = right
            return idx

        walk(tree_info["tree_structure"])
        trees.append(nodes)
    return {
        "name": name,
        "maturity_days": component_maturity(name),
        "best_iteration": best_iteration,
        "trees": trees,
    }


def predict_component(component, x):
    """单行 JSON 树推理，非有限特征按缺失值处理。"""
    total = 0.0
    for nodes in component["trees"]:
        idx = 0
        while "leaf" not in nodes[idx]:
            node = nodes[idx]
            value = float(x[int(node["feat"])])
            missing_type = node.get("missing_type", "NaN")
            # None：LightGBM 把 NaN 当作 0 再做数值比较，并非固定走右支；
            # NaN/Zero：NaN 走默认支；Zero：精确零值也走默认支。
            if missing_type == "None" and math.isnan(value):
                value = 0.0
            is_missing = (
                (missing_type != "None" and math.isnan(value))
                or (missing_type == "Zero" and value == 0.0)
            )
            if is_missing:
                idx = node["left"] if int(node.get("miss", 1)) == 1 else node["right"]
            else:
                idx = node["left"] if value <= node["thr"] else node["right"]
        total += float(nodes[idx]["leaf"])
    return total


def predict_component_rows(component, x):
    return np.array([predict_component(component, row) for row in x], dtype=float)


def ensemble_scores(model, x):
    """各模型先做 head 内截面分位，再等权平均。"""
    ranked = []
    for component in model["components"]:
        values = predict_component_rows(component, x)
        if not np.isfinite(values).all():
            raise ValueError("模型 %s 产生了非有限分数" % component["name"])
        ranked.append(pd.Series(values).rank(pct=True).to_numpy())
    return np.mean(ranked, axis=0)


def fixed_weight_selection(scores, coins, config):
    """分数最高 6 个做多、最低 6 个做空；固定总杠杆 2 倍。"""
    scores = np.asarray(scores, dtype=float)
    coins = np.asarray(coins).astype(str)
    long_n = int(config["long_n"])
    short_n = int(config["short_n"])
    if len(scores) < max(int(config["min_universe"]), long_n + short_n):
        raise ValueError("有效 head 币数不足，拒绝生成仓位")
    if not np.isfinite(scores).all():
        raise ValueError("截面分数含非有限值，拒绝生成仓位")
    order = np.lexsort((coins, scores))
    longs = order[-long_n:][::-1]
    shorts = order[:short_n]
    gross = float(config["gross_leverage"])
    long_fraction = float(config["long_fraction"])
    weights = {}
    for j in longs:
        weights[str(coins[j])] = gross * long_fraction / long_n
    for j in shorts:
        weights[str(coins[j])] = -gross * (1.0 - long_fraction) / short_n
    if len(weights) != long_n + short_n:
        raise ValueError("多空选币发生重叠")
    return {
        "long": [str(coins[j]) for j in longs],
        "short": [str(coins[j]) for j in shorts],
        "weights": weights,
    }


def quarter_for_date(value):
    date = np.datetime64(value, "D")
    year = int(str(date)[:4])
    month = int(str(date)[5:7])
    return "%04d-Q%d" % (year, (month - 1) // 3 + 1)


def quarter_start(value):
    date = np.datetime64(value, "D")
    year = int(str(date)[:4])
    month = int(str(date)[5:7])
    return np.datetime64("%04d-%02d-01" % (year, 3 * ((month - 1) // 3) + 1), "D")


def utc_now_iso():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def validate_model(model, config):
    if model.get("format_version") != 2 or model.get("strategy_id") != config.get("strategy_id"):
        raise ValueError("模型文件与当前策略配置不匹配")
    expected = list(config["ensemble_components"])
    actual = [c.get("name") for c in model.get("components", [])]
    if actual != expected:
        raise ValueError("模型组件不完整或与配置顺序不一致: %s" % actual)
    if len(model.get("feature_names", [])) == 0:
        raise ValueError("模型缺少特征列表")
    for component in model["components"]:
        if component.get("maturity_days") != component_maturity(component["name"]):
            raise ValueError("模型 %s 的标签成熟期错误" % component["name"])
        if not component.get("trees"):
            raise ValueError("模型 %s 没有可用树" % component["name"])


def validate_execution_quarter(model, signal_date):
    """季度首日使用前一交易日信号，因此按下一日成交季度校验。"""
    execution_date = np.datetime64(signal_date, "D") + np.timedelta64(1, "D")
    expected = quarter_for_date(execution_date)
    if model.get("quarter") != expected:
        raise ValueError("模型季度 %s 与成交季度 %s 不一致，拒绝生成订单" % (model.get("quarter"), expected))
    return expected
