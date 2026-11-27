"""季度扩展 close1/close3/close5/close7 等权集成 + buffer13 + 因果持仓波动率目标。

用法：
    python predict.py --json

单独运行 predict 时没有交易所当前持仓，只能给出无缓冲预览；place.py 会先读取
真实持仓再调用 latest_positions(current_positions=...)，生成实际调仓目标。
"""
import json
import sys

import numpy as np

try:
    from .strategy import (
        BASE, apply_causal_vol_targeting, fixed_weight_selection, load_config,
        load_model, model_scores, ordered_feature_names, validate_model,
    )
except ImportError:
    from strategy import (
        BASE, apply_causal_vol_targeting, fixed_weight_selection, load_config,
        load_model, model_scores, ordered_feature_names, validate_model,
    )


def latest_positions(features_path=None, model_path=None, config_path=None,
                     current_positions=None):
    config = load_config(config_path)
    model = load_model(model_path)
    validate_model(model, config)
    expected_names = ordered_feature_names()
    if model["feature_names"] != expected_names:
        raise ValueError("模型特征顺序不是平台 151 列定义")
    with np.load(str(features_path or BASE / "features.npz")) as d:
        ts = d["ts"].copy()
        coins = d["coins"].astype(str)
        members = d["members"].astype(bool)
        missing = [name for name in expected_names if "F_" + name not in d.files]
        if missing:
            raise ValueError("features.npz 缺少模型特征: %s" % ", ".join(missing[:5]))
        i = len(ts) - 1
        while i >= 0 and members[i].sum() < int(config["min_universe"]):
            i -= 1
        if i < 0:
            raise ValueError("features.npz 中没有成员数足够的 head 日期")
        idx = np.flatnonzero(members[i])
        x = np.column_stack([d["F_" + name][i, idx] for name in expected_names])
        dates_d = ts[:i + 1].astype("int64").astype("datetime64[ms]").astype("datetime64[D]")
        signal_date = str(dates_d[-1])
        # F_log_price = log(close)，仅截取 [:i+1] 严格因果已闭合日线
        close_prices = np.exp(np.asarray(d["F_log_price"][:i + 1], dtype=float))

    scores = model_scores(model, x)
    selected = fixed_weight_selection(scores, coins[idx], config, current_positions)
    scaled_weights, vol_info = apply_causal_vol_targeting(
        selected, close_prices, coins, dates_d, signal_date, config,
    )
    has_current = current_positions is not None
    selected_set = set(selected["long"] + selected["short"])
    effective_gross = float(vol_info["effective_gross"])
    max_lev = float(config.get("max_leverage", config["gross_leverage"]))
    result = {
        "date": signal_date,
        "execution": "next_open",
        "model": config.get("strategy_id", model.get("model")),
        "horizons": [int(c["horizon"]) for c in model["components"]],
        "retrain_frequency": model.get("retrain_frequency"),
        "trees": {c["name"]: len(c["trees"]) for c in model["components"]},
        "train_start": model.get("train_start"),
        "train_end": model["train_end"],
        "n_universe": int(len(idx)),
        "gross_leverage": effective_gross,
        "base_gross_leverage": float(config["gross_leverage"]),
        "max_leverage": max_lev,
        "vol_target": float(config.get("vol_target") or 0.0),
        "rv20": vol_info["rv20"],
        "lev_scale": float(vol_info["lev_scale"]),
        "vol_lookback_days": int(vol_info["vol_lookback_days"]),
        "vol_valid_days": int(vol_info["valid_days"]),
        "vol_target_active": bool(vol_info["vol_target_active"]),
        "drift_band": float(config.get("drift_band", 0.0)),
        "buffer": int(config["exit_rank"]) if has_current else None,
        "preview_without_current_positions": not has_current,
        "long": selected["long"],
        "short": selected["short"],
        "weights": scaled_weights,
        "selection": {
            "retained": sorted(selected_set & set(current_positions or {})),
            "closed": sorted(set(current_positions or {}) - selected_set),
        },
    }
    gross = sum(abs(value) for value in result["weights"].values())
    if gross > max_lev + 1e-9 or abs(gross - effective_gross) > 1e-9:
        raise ValueError("目标权重总杠杆校验失败")
    return result


def main():
    result = latest_positions()
    if "--json" in sys.argv:
        print(json.dumps(result, ensure_ascii=False))
    else:
        horizons_str = "/".join(str(h) for h in result["horizons"])
        rv_str = f"{result['rv20']:.2%}" if result["rv20"] is not None else "N/A"
        print("=== ML close%s 等权 + buffer%d + VT%.0f%%（%s，head %d 币）===" % (
            horizons_str, int(load_config()["exit_rank"]),
            result["vol_target"] * 100.0, result["date"], result["n_universe"]))
        print("训练窗口: %s .. %s (%s)" % (
            result["train_start"], result["train_end"], result["retrain_frequency"]))
        print("做多 %d: %s" % (len(result["long"]), ", ".join(result["long"])))
        print("做空 %d: %s" % (len(result["short"]), ", ".join(result["short"])))
        print("因果 rv20=%s | lev_scale=%.4f | 有效总杠杆=%.3fx (上限 %.1fx)" % (
            rv_str, result["lev_scale"], result["gross_leverage"], result["max_leverage"]))
        print("提示：standalone 输出未应用真实持仓缓冲")


if __name__ == "__main__":
    main()
