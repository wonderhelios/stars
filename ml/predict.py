"""四周期 LightGBM 集成预测，输出 6 多 6 空及固定 2 倍目标权重。

用法：
    python predict.py
    python predict.py --json
"""
from pathlib import Path
import json
import sys

import numpy as np

try:
    from .ensemble import (
        BASE, ensemble_scores, fixed_weight_selection, load_config, load_model,
        validate_execution_quarter, validate_model,
    )
except ImportError:
    from ensemble import (
        BASE, ensemble_scores, fixed_weight_selection, load_config, load_model,
        validate_execution_quarter, validate_model,
    )


def latest_positions(features_path=None, model_path=None, config_path=None):
    config = load_config(config_path)
    model = load_model(model_path)
    validate_model(model, config)
    with np.load(str(features_path or BASE / "features.npz")) as d:
        ts = d["ts"].copy()
        coins = d["coins"].astype(str)
        members = d["members"].astype(bool)
        feature_keys = [key for key in d.files if key.startswith("F_")]
        feature_names = [key[2:] for key in feature_keys]
        if model["feature_names"] != feature_names:
            raise ValueError("模型与 features.npz 的特征列顺序不一致")
        i = len(ts) - 1
        while i >= 0 and members[i].sum() < int(config["min_universe"]):
            i -= 1
        if i < 0:
            raise ValueError("features.npz 中没有成员数足够的 head 日期")
        idx = np.flatnonzero(members[i])
        x = np.column_stack([d[key][i, idx] for key in feature_keys])
        signal_date = str(ts[i].astype("int64").astype("datetime64[ms]").astype("datetime64[D]"))

    model_quarter = validate_execution_quarter(model, signal_date)
    scores = ensemble_scores(model, x)
    selected = fixed_weight_selection(scores, coins[idx], config)
    result = {
        "date": signal_date,
        "execution": "next_open",
        "model_quarter": model_quarter,
        "n_universe": int(len(idx)),
        "gross_leverage": float(config["gross_leverage"]),
        "long": selected["long"],
        "short": selected["short"],
        "weights": selected["weights"],
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
        print("=== ML 四周期集成目标持仓（%s，%s，head %d 币）===" % (
            result["date"], result["model_quarter"], result["n_universe"]))
        print("做多 %d: %s" % (len(result["long"]), ", ".join(result["long"])))
        print("做空 %d: %s" % (len(result["short"]), ", ".join(result["short"])))
        print("固定总杠杆 %.1f 倍（无波动目标）" % result["gross_leverage"])


if __name__ == "__main__":
    main()
