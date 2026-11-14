"""复现平台冻结的 close3 单模型：精确切分、分批分箱和 JSON 树导出。"""
from pathlib import Path
import argparse
import datetime
import json

import lightgbm as lgb
import numpy as np
import pyarrow as pa
from scipy.stats import pearsonr, spearmanr

try:
    from .strategy import (
        BASE, export_component, load_config, ordered_feature_names,
        predict_component_rows, validate_model,
    )
except ImportError:
    from strategy import (
        BASE, export_component, load_config, ordered_feature_names,
        predict_component_rows, validate_model,
    )


def utc_now_iso():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def validation_metrics(pred, forward, days):
    rank_ic, pearson_ic = [], []
    for day in np.unique(days):
        take = days == day
        if take.sum() > 1 and len(np.unique(pred[take])) > 1:
            rank_ic.append(spearmanr(pred[take], forward[take]).statistic)
            pearson_ic.append(pearsonr(pred[take], forward[take]).statistic)
    rank_ic = np.asarray(rank_ic, dtype=float)
    pearson_ic = np.asarray(pearson_ic, dtype=float)
    return {
        "rank_ic": float(np.nanmean(rank_ic)),
        "rank_icir": float(np.nanmean(rank_ic) / np.nanstd(rank_ic, ddof=1)),
        "pearson_ic": float(np.nanmean(pearson_ic)),
        "pearson_icir": float(np.nanmean(pearson_ic) / np.nanstd(pearson_ic, ddof=1)),
        "validation_days": int(len(rank_ic)),
    }


def arrow_block(x, row_ids, names, offset, stop):
    # from_pandas=True 保留 Arrow null，与平台 Dataset 的缺失值分箱一致。
    return pa.table({names[col]: pa.array(x[row_ids, col], from_pandas=True)
                     for col in range(offset, stop)})


def train_model(features_path, output_path, config):
    names = ordered_feature_names()
    keys = ["F_" + name for name in names]
    with np.load(str(features_path)) as d:
        missing = [key for key in keys if key not in d.files]
        if missing:
            raise ValueError("features.npz 缺少特征: %s" % ", ".join(missing[:5]))
        ts = d["ts"].astype("datetime64[ms]").astype("datetime64[D]")
        members = d["members"].astype(bool)
        labels = d["labels"].copy()
        forward = d["fwd"].copy()
        n, k = members.shape
        rows = np.flatnonzero(members.reshape(-1))
        x = np.column_stack([d[key].reshape(-1)[rows] for key in keys])

    row_days = rows // k
    row_dates = ts[row_days]
    row_labels = labels.reshape(-1)[rows]
    eligible = ((row_dates >= np.datetime64(config["train_start"], "D"))
                & (row_dates <= np.datetime64(config["train_end"], "D"))
                & np.isfinite(row_labels))
    unique = np.unique(row_dates[eligible])
    if len(unique) < 100:
        raise ValueError("冻结窗口内有效训练日不足")
    split = unique[len(unique) * 4 // 5]
    label_end = ts[np.minimum(row_days + 3, n - 1)]
    train_mask = eligible & (label_end < split)
    valid_mask = eligible & (row_dates >= split)
    train_rows, valid_rows = np.flatnonzero(train_mask), np.flatnonzero(valid_mask)
    if not len(train_rows) or not len(valid_rows):
        raise ValueError("训练集或验证集为空")

    params = {key: value for key, value in config["lightgbm"].items()
              if key not in ("num_boost_round", "early_stopping_rounds")}
    train = None
    for offset in range(0, len(names), 16):
        stop = min(offset + 16, len(names))
        block = lgb.Dataset(
            arrow_block(x, train_rows, names, offset, stop), row_labels[train_rows],
            feature_name=names[offset:stop], params=params,
        ).construct()
        if train is None:
            train = block
        else:
            train.add_features_from(block)
    valid = lgb.Dataset(
        arrow_block(x, valid_rows, names, 0, len(names)), row_labels[valid_rows],
        reference=train, feature_name=names, params=params,
    ).construct()
    booster = lgb.train(
        params, train, num_boost_round=int(config["lightgbm"]["num_boost_round"]),
        valid_sets=[valid], valid_names=["validation"],
        callbacks=[lgb.early_stopping(
            int(config["lightgbm"]["early_stopping_rounds"]), verbose=False)],
    )
    component = export_component(booster)
    expected = booster.predict(x[valid_rows], num_iteration=booster.best_iteration)
    actual = predict_component_rows(component, x[valid_rows])
    error = float(np.max(np.abs(expected - actual)))
    if error > 1e-10:
        raise ValueError("JSON 推理与 LightGBM 不一致: %.3g" % error)

    report = {
        "split": str(split), "train_rows": int(train_mask.sum()),
        "validation_rows": int(valid_mask.sum()), "trees": int(booster.best_iteration),
        "feature_count": len(names), "json_max_error": error,
        **validation_metrics(expected, forward.reshape(-1)[rows][valid_mask], row_dates[valid_mask]),
    }
    model = {
        "format_version": 3,
        "strategy_id": config["strategy_id"],
        "model": "alphapilot51_close3",
        "train_start": config["train_start"],
        "train_end": config["train_end"],
        "retrain_frequency": "frozen",
        "trained_at": utc_now_iso(),
        "feature_names": names,
        "component": component,
        "training": report,
        "gross_leverage": float(config["gross_leverage"]),
        "volatility_target": None,
    }
    validate_model(model, config)
    output_path = Path(output_path)
    temporary = output_path.with_suffix(output_path.suffix + ".tmp")
    with open(temporary, "w", encoding="utf-8") as f:
        json.dump(model, f, ensure_ascii=False, separators=(",", ":"))
    temporary.replace(output_path)
    print(json.dumps(report, ensure_ascii=False), flush=True)
    print("导出冻结单模型 %s（%d 棵树）" % (output_path, booster.best_iteration), flush=True)
    return model


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--features", default=str(BASE / "features.npz"))
    parser.add_argument("--output", default=str(BASE / "model.json"))
    parser.add_argument("--config", default=str(BASE / "strategy_config.json"))
    args = parser.parse_args()
    train_model(args.features, args.output, load_config(args.config))


if __name__ == "__main__":
    main()
