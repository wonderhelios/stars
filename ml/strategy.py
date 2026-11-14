"""AlphaPilot 复刻：单模型 JSON 推理、buffer9 选币和固定 2 倍权重。"""
from pathlib import Path
import json
import math

import numpy as np

BASE = Path(__file__).resolve().parent
DEFAULT_CONFIG_PATH = BASE / "strategy_config.json"
DEFAULT_MODEL_PATH = BASE / "model.json"


def load_config(path=None):
    with open(path or DEFAULT_CONFIG_PATH, encoding="utf-8") as f:
        config = json.load(f)
    if config.get("format_version") != 3:
        raise ValueError("不支持的 ML 策略配置版本")
    if config.get("label") != "close3" or config.get("ensemble_components"):
        raise ValueError("当前策略只允许单个 close3 模型")
    if config.get("gross_leverage") != 2.0 or config.get("long_fraction") != 0.5:
        raise ValueError("当前实现要求固定总杠杆 2 倍且多空各占一半")
    if config.get("exit_rank") != 9:
        raise ValueError("当前复刻策略要求 buffer9")
    return config


def load_model(path=None):
    with open(path or DEFAULT_MODEL_PATH, encoding="utf-8") as f:
        return json.load(f)


def export_component(model, name="close3"):
    """将最佳迭代轮数导出为不依赖 LightGBM 的 JSON 树。"""
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
                raise ValueError("暂不支持非数值 LightGBM 分裂")
            nodes.append({
                "feat": int(node["split_feature"]), "thr": float(node["threshold"]),
                "left": -1, "right": -1,
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
    return {"name": name, "best_iteration": best_iteration, "trees": trees}


def predict_component(component, x):
    total = 0.0
    for nodes in component["trees"]:
        idx = 0
        while "leaf" not in nodes[idx]:
            node = nodes[idx]
            value = float(x[int(node["feat"])])
            missing_type = node.get("missing_type", "NaN")
            if missing_type == "None" and math.isnan(value):
                value = 0.0
            is_missing = ((missing_type != "None" and math.isnan(value))
                          or (missing_type == "Zero" and value == 0.0))
            if is_missing:
                idx = node["left"] if int(node.get("miss", 1)) == 1 else node["right"]
            else:
                idx = node["left"] if value <= node["thr"] else node["right"]
        total += float(nodes[idx]["leaf"])
    return total


def predict_component_rows(component, x):
    return np.array([predict_component(component, row) for row in x], dtype=float)


def model_scores(model, x):
    values = predict_component_rows(model["component"], x)
    if not np.isfinite(values).all():
        raise ValueError("模型产生了非有限分数")
    return values


def ordered_feature_names():
    """平台顺序：基础列，特征优先滚动窗口，尾部加密原生列。"""
    base = ["kmid", "klen", "kmid_2", "kup", "kup_2", "klow", "klow_2",
            "ksft", "ksft_2", "open_0", "high_0", "low_0", "vwap_0"]
    rolling = ["roc", "ma", "cntp", "cntn", "cntd", "std", "max", "min",
               "qtlu", "qtld", "imax", "imin", "imxd", "rank", "rsv", "corr",
               "cord", "vma", "vstd", "wvma", "sump", "sumn", "sumd",
               "vsump", "vsumn", "vsumd"]
    tail = ["fr_1d", "fr_7d", "prem_1d", "oi_chg_1d", "oi_chg_7d",
            "tc_ratio", "log_dv", "log_price"]
    names = base + [f"{name}_{window}" for name in rolling
                    for window in (5, 10, 20, 30, 60)] + tail
    if len(names) != 151:
        raise AssertionError("特征定义必须为 151 列")
    return names


def fixed_weight_selection(scores, coins, config, current_positions=None):
    """保留排名仍在 buffer9 内的旧仓，再按 score 排名补齐 6 多 6 空。"""
    scores = np.asarray(scores, dtype=float)
    coins = np.asarray(coins).astype(str)
    if len(scores) < max(int(config["min_universe"]),
                         int(config["long_n"]) + int(config["short_n"])):
        raise ValueError("有效 head 币数不足，拒绝生成仓位")
    if not np.isfinite(scores).all():
        raise ValueError("截面分数含非有限值，拒绝生成仓位")
    order = np.lexsort((coins, -scores))
    ranked = coins[order].tolist()
    rank = {coin: i + 1 for i, coin in enumerate(ranked)}
    n = len(ranked)
    current_positions = current_positions or {}
    exit_rank = int(config["exit_rank"])
    long_n, short_n = int(config["long_n"]), int(config["short_n"])
    longs = sorted((c for c, size in current_positions.items()
                    if float(size) > 0 and rank.get(c, n + 1) <= exit_rank), key=rank.get)
    shorts = sorted((c for c, size in current_positions.items()
                     if float(size) < 0 and rank.get(c, 0) >= n - exit_rank + 1),
                    key=rank.get, reverse=True)
    if len(longs) > long_n or len(shorts) > short_n:
        raise ValueError("当前单边持仓超过目标数量，无法应用 buffer9")
    for coin in ranked:
        if len(longs) == long_n:
            break
        if coin not in longs and coin not in shorts:
            longs.append(coin)
    for coin in reversed(ranked):
        if len(shorts) == short_n:
            break
        if coin not in longs and coin not in shorts:
            shorts.append(coin)
    if len(longs) != long_n or len(shorts) != short_n or set(longs) & set(shorts):
        raise ValueError("无法生成互不重叠的 6 多 6 空目标")
    gross = float(config["gross_leverage"])
    long_fraction = float(config["long_fraction"])
    weights = {c: gross * long_fraction / long_n for c in longs}
    weights.update({c: -gross * (1.0 - long_fraction) / short_n for c in shorts})
    return {"long": longs, "short": shorts, "weights": weights}


def validate_model(model, config):
    if model.get("format_version") != 3 or model.get("strategy_id") != config.get("strategy_id"):
        raise ValueError("模型文件与当前策略配置不匹配")
    component = model.get("component", {})
    if component.get("name") != "close3" or not component.get("trees"):
        raise ValueError("模型缺少可用的 close3 树")
    if component.get("best_iteration") != len(component["trees"]):
        raise ValueError("模型树数与 best_iteration 不一致")
    if len(model.get("feature_names", [])) != 151:
        raise ValueError("模型必须包含 151 个特征")
    if model.get("train_end") != config.get("train_end"):
        raise ValueError("模型训练窗口与冻结配置不一致")
    if len(component["trees"]) != int(config.get("model_trees", 51)):
        raise ValueError("模型必须使用复现验证过的 51 棵最佳迭代树")
