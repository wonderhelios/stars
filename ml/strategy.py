"""季度扩展 close1/close3/close5/close7 等权集成、buffer13 选币与因果持仓波动率目标。"""
from __future__ import annotations

from pathlib import Path
import datetime
import json
import math

import numpy as np

BASE = Path(__file__).resolve().parent
DEFAULT_CONFIG_PATH = BASE / "strategy_config.json"
DEFAULT_MODEL_PATH = BASE / "model.json"


def latest_quarter_end(today: datetime.date | None = None) -> datetime.date:
    """返回严格早于 today 的最近一个自然季度末（用于季度扩展重训判断）。"""
    today = today or datetime.datetime.now(datetime.timezone.utc).date()
    if today.month <= 3:
        return datetime.date(today.year - 1, 12, 31)
    if today.month <= 6:
        return datetime.date(today.year, 3, 31)
    if today.month <= 9:
        return datetime.date(today.year, 6, 30)
    return datetime.date(today.year, 9, 30)


def load_config(path=None):
    with open(path or DEFAULT_CONFIG_PATH, encoding="utf-8") as f:
        config = json.load(f)
    if config.get("format_version") != 5:
        raise ValueError("不支持的 ML 策略配置版本（需要 format_version=5）")
    retrain_freq = config.get("retrain_frequency")
    if retrain_freq not in ("quarterly", "frozen"):
        raise ValueError(f"不支持的 retrain_frequency: {retrain_freq}")
    components = config.get("ensemble_components")
    if not isinstance(components, list) or not components:
        raise ValueError("ensemble_components 必须为非空列表")
    comp_horizons = [int(c.get("horizon", 0)) for c in components]
    if any(h <= 0 for h in comp_horizons) or len(set(comp_horizons)) != len(comp_horizons):
        raise ValueError("集成组件周期必须为互异正整数")
    horizons = config.get("horizons")
    if horizons is not None and [int(h) for h in horizons] != comp_horizons:
        raise ValueError("horizons 与 ensemble_components 周期不一致")
    expected_w = 1.0 / len(components)
    if any(abs(float(c.get("weight", 0.0)) - expected_w) > 1e-12 for c in components):
        raise ValueError("集成组件必须等权")
    long_n = int(config.get("long_n", 0))
    short_n = int(config.get("short_n", 0))
    exit_rank = int(config.get("exit_rank", 0))
    if long_n <= 0 or short_n <= 0 or exit_rank < max(long_n, short_n):
        raise ValueError("long_n/short_n/exit_rank 配置无效")
    drift_band = float(config.get("drift_band", 0.0))
    if not math.isfinite(drift_band) or drift_band < 0.0 or drift_band >= 1.0:
        raise ValueError("drift_band 必须在 [0.0, 1.0) 内")
    gross = float(config.get("gross_leverage", 0.0))
    max_lev = float(config.get("max_leverage", gross))
    long_frac = float(config.get("long_fraction", 0.5))
    if not math.isfinite(gross) or gross <= 0.0 or not math.isfinite(max_lev) or max_lev <= 0.0:
        raise ValueError("gross_leverage/max_leverage 必须为正数")
    if not (0.0 < long_frac < 1.0):
        raise ValueError("long_fraction 必须在 (0, 1) 内")
    vol_target = float(config.get("vol_target") or 0.0)
    if not math.isfinite(vol_target) or vol_target < 0.0:
        raise ValueError("vol_target 必须为非负数（0.0 表示关闭）")
    vol_lb = int(config.get("vol_lookback_days", 20))
    if vol_lb < 2:
        raise ValueError("vol_lookback_days 至少为 2")
    min_scale = float(config.get("min_lev_scale", 0.35))
    max_scale = float(config.get("max_lev_scale", 1.0))
    if not (0.0 < min_scale <= max_scale):
        raise ValueError("min_lev_scale / max_lev_scale 配置无效")
    return config


def load_model(path=None):
    with open(path or DEFAULT_MODEL_PATH, encoding="utf-8") as f:
        return json.load(f)


def export_component(model, name="close3", horizon=3, weight=1.0):
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
    return {"name": name, "horizon": int(horizon), "weight": float(weight),
            "best_iteration": best_iteration, "trees": trees}


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
    values = np.zeros(len(x), dtype=float)
    for component in model["components"]:
        values += float(component["weight"]) * predict_component_rows(component, x)
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
    """保留 exit_rank 缓冲内最强的目标仓，再按 score 排名补齐 long_n 多 / short_n 空。

    缓冲内旧仓超过目标数量时，只保留排名最强的部分，其余照常进入平仓目标。
    低于交易所最小订单额的旧仓可能暂时平不掉，但不能因此卡死后续调仓。
    """
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
                    if float(size) > 0 and rank.get(c, n + 1) <= exit_rank),
                   key=rank.get)[:long_n]
    shorts = sorted((c for c, size in current_positions.items()
                     if float(size) < 0 and rank.get(c, 0) >= n - exit_rank + 1),
                    key=rank.get, reverse=True)[:short_n]
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
        raise ValueError("无法生成互不重叠的多空目标")
    gross = float(config["gross_leverage"])
    long_fraction = float(config["long_fraction"])
    weights = {c: gross * long_fraction / long_n for c in longs}
    weights.update({c: -gross * (1.0 - long_fraction) / short_n for c in shorts})
    return {"long": longs, "short": shorts, "weights": weights}


def compute_causal_holdings_vol_scale(
    weights: dict[str, float],
    close_prices: np.ndarray,
    coins: np.ndarray | list[str],
    dates: np.ndarray,
    signal_date,
    config: dict,
) -> dict:
    """严格因果持仓波动率目标：仅用 signal_date 及之前的已闭合币种日收益估算组合 rv20。

    说明（对应 Part B 审计 Caveat 4）：
      - 绝不使用可能受出入金污染的 ml_history 账户净值曲线；
      - 严格截取 dates <= signal_date（对应执行日 t 的 t-20..t-1 已闭合日收益）；
      - 目标权重 w 与币种日收益矩阵 R 相乘得到组合日收益序列 p = R @ w，
        rv20 = std(p, ddof=1) * sqrt(365)（代数等价于 sqrt(w^T Sigma w) * sqrt(365)）；
      - 若 vol_target == 0.0 则关闭波动率缩放（lev_scale=1.0）；
      - 若币种历史不足 lookback 天则安全回退 lev_scale=1.0；
      - 最终总杠杆严格上限为 max_leverage（默认 2.0x）。
    """
    vol_target = float(config.get("vol_target") or 0.0)
    lookback = int(config.get("vol_lookback_days", 20))
    min_scale = float(config.get("min_lev_scale", 0.35))
    max_scale = float(config.get("max_lev_scale", 1.0))
    max_lev = float(config.get("max_leverage", config.get("gross_leverage", 2.0)))

    base_gross = sum(abs(float(w)) for w in weights.values())
    hard_max_scale = (max_lev / base_gross) if base_gross > 0.0 else 1.0
    default_scale = min(1.0, hard_max_scale)
    eff_max_scale = min(max_scale, hard_max_scale)
    eff_min_scale = min(min_scale, eff_max_scale)

    def _result(rv20, lev_scale, valid_days, active, reason=None):
        return {
            "rv20": rv20,
            "lev_scale": float(lev_scale),
            "effective_gross": float(base_gross * lev_scale),
            "vol_target": vol_target,
            "vol_lookback_days": lookback,
            "valid_days": int(valid_days),
            "vol_target_active": bool(active),
            "fallback_reason": reason,
        }

    if base_gross <= 0.0 or close_prices is None or dates is None:
        return _result(None, default_scale, 0, False, "missing_price_history")

    dates_arr = np.asarray(dates)
    if dates_arr.dtype.kind in ("i", "u"):
        dates_d = dates_arr.astype("datetime64[ms]").astype("datetime64[D]")
    else:
        dates_d = dates_arr.astype("datetime64[D]")
    sig_d = np.datetime64(str(signal_date), "D")

    # 严格因果截断：只保留 <= signal_date 的已闭合日线，绝不接触未来 bar
    causal_idx = np.flatnonzero(dates_d <= sig_d)
    if len(causal_idx) < lookback + 1:
        return _result(None, default_scale, max(0, len(causal_idx) - 1), False, "insufficient_history")

    order = causal_idx[np.argsort(dates_d[causal_idx], kind="stable")]
    # 仅需最近 lookback + 1 根闭合日线即可算出 lookback 个闭合日收益
    window_idx = order[-(lookback + 1):]
    closes = np.asarray(close_prices, dtype=float)[window_idx]
    coin_to_col = {str(c): j for j, c in enumerate(coins)}

    long_items = [(coin_to_col[c], float(w)) for c, w in weights.items() if float(w) > 0.0 and c in coin_to_col]
    short_items = [(coin_to_col[c], float(w)) for c, w in weights.items() if float(w) < 0.0 and c in coin_to_col]
    target_long_gross = sum(w for _, w in weights.items() if float(w) > 0.0)
    target_short_gross = sum(-float(w) for _, w in weights.items() if float(w) < 0.0)

    if not long_items and not short_items:
        return _result(None, default_scale, 0, False, "no_matching_coins")

    # 计算 (lookback, k) 的闭合日收益 r[d] = close[d] / close[d-1] - 1
    prev_c = closes[:-1]
    curr_c = closes[1:]
    with np.errstate(divide="ignore", invalid="ignore"):
        rets = np.where((prev_c > 0.0) & (curr_c > 0.0), curr_c / prev_c - 1.0, np.nan)

    port_rets = []
    for d in range(lookback):
        row = rets[d]
        day_ret = 0.0
        if target_long_gross > 0.0:
            l_valid = [(col, w) for col, w in long_items if math.isfinite(row[col])]
            l_wsum = sum(w for _, w in l_valid)
            if l_wsum < 0.5 * target_long_gross:
                continue
            l_scale = target_long_gross / l_wsum
            day_ret += sum((w * l_scale) * row[col] for col, w in l_valid)
        if target_short_gross > 0.0:
            s_valid = [(col, w) for col, w in short_items if math.isfinite(row[col])]
            s_wsum = sum(-w for _, w in s_valid)
            if s_wsum < 0.5 * target_short_gross:
                continue
            s_scale = target_short_gross / s_wsum
            day_ret += sum((w * s_scale) * row[col] for col, w in s_valid)
        if math.isfinite(day_ret):
            port_rets.append(day_ret)

    valid_days = len(port_rets)
    if valid_days < lookback:
        return _result(None, default_scale, valid_days, False, "insufficient_history")

    rv20 = float(np.std(np.asarray(port_rets, dtype=float), ddof=1)) * math.sqrt(365.0)
    if not math.isfinite(rv20) or rv20 <= 1e-6:
        return _result(None, default_scale, valid_days, False, "zero_or_invalid_vol")

    if vol_target <= 0.0:
        return _result(rv20, default_scale, valid_days, False, "disabled")

    raw_scale = vol_target / rv20
    lev_scale = float(np.clip(raw_scale, eff_min_scale, eff_max_scale))
    return _result(rv20, lev_scale, valid_days, True, None)


def apply_causal_vol_targeting(
    selected: dict,
    close_prices: np.ndarray,
    coins: np.ndarray | list[str],
    dates: np.ndarray,
    signal_date,
    config: dict,
) -> tuple[dict[str, float], dict]:
    """对目标权重应用因果持仓波动率目标缩放，返回 (scaled_weights, vol_info)。"""
    base_weights = selected["weights"]
    vol_info = compute_causal_holdings_vol_scale(
        base_weights, close_prices, coins, dates, signal_date, config,
    )
    lev_scale = vol_info["lev_scale"]
    scaled_weights = {coin: float(w) * lev_scale for coin, w in base_weights.items()}
    return scaled_weights, vol_info


def validate_model(model, config):
    if model.get("format_version") != config.get("format_version"):
        raise ValueError("模型版本与当前策略配置不匹配")
    if model.get("strategy_id") != config.get("strategy_id"):
        raise ValueError("模型 strategy_id 与当前策略配置不匹配")
    if model.get("retrain_frequency") != config.get("retrain_frequency"):
        raise ValueError("模型重训频率与当前策略配置不匹配")
    if model.get("train_start") != config.get("train_start"):
        raise ValueError("模型训练起点与当前策略配置不匹配")
    train_end = model.get("train_end")
    try:
        train_start_d = datetime.date.fromisoformat(str(config["train_start"]))
        train_end_d = datetime.date.fromisoformat(str(train_end))
    except Exception as exc:
        raise ValueError(f"模型训练日期格式无效: {train_end}") from exc
    if train_end_d < train_start_d:
        raise ValueError("模型 train_end 早于 train_start")
    if config.get("retrain_frequency") == "frozen" and train_end != config.get("train_end"):
        raise ValueError("冻结模型 train_end 与配置不一致")
    if model.get("feature_names") != ordered_feature_names():
        raise ValueError("模型必须包含平台顺序的 151 个特征")
    components = model.get("components")
    if not isinstance(components, list) or len(components) != len(config["ensemble_components"]):
        raise ValueError("模型缺少完整的集成组件")
    for component, spec in zip(components, config["ensemble_components"]):
        if (component.get("name"), int(component.get("horizon", 0))) != (spec["name"], int(spec["horizon"])):
            raise ValueError("模型组件名称或周期与配置不一致")
        if abs(float(component.get("weight", 0.0)) - float(spec["weight"])) > 1e-12:
            raise ValueError("模型组件权重与配置不一致")
        if not component.get("trees"):
            raise ValueError("模型组件缺少可用树")
        if component.get("best_iteration") != len(component["trees"]):
            raise ValueError("模型树数与 best_iteration 不一致")
        if "trees" in spec and len(component["trees"]) != int(spec["trees"]):
            raise ValueError("模型树数与配置不一致")
