"""冻结 51 树 close3 模型预测，应用 buffer9，输出固定 2 倍目标权重。

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
        BASE, fixed_weight_selection, load_config, load_model, model_scores,
        ordered_feature_names, validate_model,
    )
except ImportError:
    from strategy import (
        BASE, fixed_weight_selection, load_config, load_model, model_scores,
        ordered_feature_names, validate_model,
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
        signal_date = str(ts[i].astype("int64").astype("datetime64[ms]").astype("datetime64[D]"))

    scores = model_scores(model, x)
    selected = fixed_weight_selection(scores, coins[idx], config, current_positions)
    has_current = current_positions is not None
    selected_set = set(selected["long"] + selected["short"])
    result = {
        "date": signal_date,
        "execution": "next_open",
        "model": "alphapilot51_close3",
        "trees": len(model["component"]["trees"]),
        "train_end": model["train_end"],
        "n_universe": int(len(idx)),
        "gross_leverage": float(config["gross_leverage"]),
        "buffer": 9 if has_current else None,
        "preview_without_current_positions": not has_current,
        "long": selected["long"],
        "short": selected["short"],
        "weights": selected["weights"],
        "selection": {
            "retained": sorted(selected_set & set(current_positions or {})),
            "closed": sorted(set(current_positions or {}) - selected_set),
        },
    }
    gross = sum(abs(value) for value in result["weights"].values())
    if abs(gross - float(config["gross_leverage"])) > 1e-12:
        raise ValueError("目标权重总杠杆校验失败")
    return result


def main():
    result = latest_positions()
    if "--json" in sys.argv:
        print(json.dumps(result, ensure_ascii=False))
    else:
        print("=== ML 51树 close3 + buffer9（%s，head %d 币）===" % (
            result["date"], result["n_universe"]))
        print("做多 %d: %s" % (len(result["long"]), ", ".join(result["long"])))
        print("做空 %d: %s" % (len(result["short"]), ", ".join(result["short"])))
        print("固定总杠杆 %.1f 倍；standalone 输出未应用真实持仓缓冲" % result["gross_leverage"])


if __name__ == "__main__":
    main()
