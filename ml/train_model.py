"""按季度扩展窗口训练四周期 LightGBM，并导出 model_ensemble.json。"""
from pathlib import Path
import argparse
import datetime
import json

import lightgbm as lgb
import numpy as np

try:
    from .ensemble import (
        BASE, component_labels, component_maturity, export_component, load_config,
        predict_component_rows, quarter_for_date, quarter_start, utc_now_iso, validate_model,
    )
except ImportError:
    from ensemble import (
        BASE, component_labels, component_maturity, export_component, load_config,
        predict_component_rows, quarter_for_date, quarter_start, utc_now_iso, validate_model,
    )


def aligned_prices(features_path, ts, coins):
    """从原始日线读取 O/C，避免用特征反推价格造成标签漂移。"""
    n, k = len(ts), len(coins)
    open_prices = np.full((n, k), np.nan)
    close_prices = np.full((n, k), np.nan)
    time_index = {int(t): i for i, t in enumerate(ts)}
    base = Path(features_path).resolve().parent
    for j, coin in enumerate(coins):
        path = base / "data" / "hl2" / (str(coin) + ".json")
        if not path.exists():
            continue
        with open(path, encoding="utf-8") as f:
            bars = json.load(f)
        for bar in bars:
            i = time_index.get(int(bar["t"]))
            if i is None:
                continue
            open_prices[i, j] = float(bar["o"])
            close_prices[i, j] = float(bar["c"])
    return open_prices, close_prices


def train_ensemble(features_path, output_path, config, as_of=None):
    as_of = as_of or datetime.datetime.now(datetime.timezone.utc).date().isoformat()
    start = quarter_start(as_of)
    quarter = quarter_for_date(as_of)
    with np.load(str(features_path)) as d:
        ts = d["ts"].copy()
        coins = d["coins"].astype(str)
        members = d["members"].astype(bool)
        feature_keys = [key for key in d.files if key.startswith("F_")]
        feature_names = [key[2:] for key in feature_keys]
        n, k = members.shape
        rows = np.flatnonzero(members.reshape(-1))
        raw = np.column_stack([d[key].reshape(-1)[rows] for key in feature_keys])
        open_prices, close_prices = aligned_prices(features_path, ts, coins)

    dates = ts.astype("int64").astype("datetime64[ms]").astype("datetime64[D]")
    row_days = rows // k
    row_dates = dates[row_days]
    lgb_params = {key: value for key, value in config["lightgbm"].items()
                  if key not in ("num_boost_round", "early_stopping_rounds")}
    components = []
    training_log = []
    for name in config["ensemble_components"]:
        labels_2d = component_labels(name, open_prices, close_prices, members)
        labels = labels_2d.reshape(-1)[rows]
        # open3/open5 分别多等待一天；内部验证标签也必须在季度开始前成熟。
        maturity = component_maturity(name)
        mature_dates = dates[np.minimum(row_days + maturity, n - 1)]
        eligible = (
            np.isfinite(labels)
            & (row_dates >= np.datetime64(config["train_start"], "D"))
            & (mature_dates < start)
        )
        eligible_dates = np.unique(row_dates[eligible])
        if len(eligible_dates) < 100:
            raise ValueError("%s 在 %s 前只有 %d 个训练日，拒绝训练" % (name, start, len(eligible_dates)))
        split = eligible_dates[len(eligible_dates) * 4 // 5]
        train_mask = eligible & (mature_dates < split)
        valid_mask = eligible & (row_dates >= split)
        if not train_mask.any() or not valid_mask.any():
            raise ValueError("%s 的训练集或内部验证集为空" % name)
        train = lgb.Dataset(raw[train_mask], label=labels[train_mask], feature_name=feature_names)
        valid = lgb.Dataset(raw[valid_mask], label=labels[valid_mask], reference=train)
        booster = lgb.train(
            lgb_params, train,
            num_boost_round=int(config["lightgbm"]["num_boost_round"]),
            valid_sets=[valid],
            callbacks=[lgb.early_stopping(int(config["lightgbm"]["early_stopping_rounds"]), verbose=False)],
        )
        component = export_component(booster, name)
        sample = raw[valid_mask][-100:]
        expected = booster.predict(sample, num_iteration=booster.best_iteration)
        actual = predict_component_rows(component, sample)
        error = float(np.max(np.abs(expected - actual))) if len(sample) else 0.0
        if error > 1e-10:
            raise ValueError("%s 的 JSON 推理误差过大: %.3g" % (name, error))
        info = {
            "model": name,
            "maturity_days": maturity,
            "inner_valid_start": str(split),
            "train_rows": int(train_mask.sum()),
            "valid_rows": int(valid_mask.sum()),
            "best_iteration": int(booster.best_iteration),
            "json_max_error": error,
        }
        training_log.append(info)
        components.append(component)
        print(json.dumps(info, ensure_ascii=False), flush=True)
        del train, valid, booster

    model = {
        "format_version": 2,
        "strategy_id": config["strategy_id"],
        "quarter": quarter,
        "train_before": str(start),
        "trained_at": utc_now_iso(),
        "feature_names": feature_names,
        "components": components,
        "training": training_log,
        "gross_leverage": float(config["gross_leverage"]),
        "volatility_target": None,
    }
    validate_model(model, config)
    output_path = Path(output_path)
    temporary = output_path.with_suffix(output_path.suffix + ".tmp")
    with open(temporary, "w", encoding="utf-8") as f:
        json.dump(model, f, ensure_ascii=False, separators=(",", ":"))
    temporary.replace(output_path)
    print("导出 %s：%s，%d 个组件，固定总杠杆 %.1f 倍" % (
        output_path, quarter, len(components), config["gross_leverage"]), flush=True)
    return model


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--features", default=str(BASE / "features.npz"))
    parser.add_argument("--output", default=str(BASE / "model_ensemble.json"))
    parser.add_argument("--config", default=str(BASE / "strategy_config.json"))
    parser.add_argument("--as-of", help="训练所属成交日 YYYY-MM-DD；默认当前 UTC 日期")
    args = parser.parse_args()
    train_ensemble(args.features, args.output, load_config(args.config), args.as_of)


if __name__ == "__main__":
    main()
