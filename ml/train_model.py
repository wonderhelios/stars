"""训练冻结 close1/close3/close5 等权集成并导出无 LightGBM 依赖的 JSON。"""
from pathlib import Path
import argparse
import datetime
import json

import lightgbm as lgb
import numpy as np
import pyarrow as pa
from scipy.stats import pearsonr, rankdata, spearmanr

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


def pct_cs_labels(forward, members):
    """未来 h 日收益在当期动态币池内的截面百分位。"""
    labels = np.full(forward.shape, np.nan, dtype=float)
    for i in range(len(forward)):
        valid = members[i] & np.isfinite(forward[i])
        count = int(valid.sum())
        if count:
            labels[i, valid] = rankdata(forward[i, valid], method="average") / count
    return labels


def load_training_data(features_path, names):
    keys = ["F_" + name for name in names]
    with np.load(str(features_path)) as d:
        missing = [key for key in keys if key not in d.files]
        if missing:
            raise ValueError("features.npz 缺少特征: %s" % ", ".join(missing[:5]))
        ts_ms = d["ts"].copy()
        ts = ts_ms.astype("datetime64[ms]").astype("datetime64[D]")
        coins = d["coins"].astype(str)
        members = d["members"].astype(bool)
        cached_labels = d["labels"].copy()
        cached_forward = d["fwd"].copy()
        n, k = members.shape
        rows = np.flatnonzero(members.reshape(-1))
        x = np.column_stack([d[key].reshape(-1)[rows] for key in keys])

    close = np.full((n, k), np.nan, dtype=float)
    lookup = {int(t): i for i, t in enumerate(ts_ms)}
    for j, coin in enumerate(coins):
        path = BASE / "data" / "hl2" / f"{coin}.json"
        if not path.exists():
            raise FileNotFoundError(f"训练缺少 {coin} 日线缓存: {path}")
        for bar in json.loads(path.read_text()):
            i = lookup.get(int(bar["t"]))
            if i is not None and float(bar["c"]) > 0:
                close[i, j] = float(bar["c"])
    return ts, members, rows, x, close, cached_labels, cached_forward


def train_model(features_path, output_path, config):
    names = ordered_feature_names()
    ts, members, rows, x, close, cached_labels, cached_forward = load_training_data(features_path, names)
    n, k = members.shape
    row_days = rows // k
    row_dates = ts[row_days]
    params = {key: value for key, value in config["lightgbm"].items()
              if key not in ("num_boost_round", "early_stopping_rounds")}
    components, reports = [], {}

    for spec in config["ensemble_components"]:
        horizon = int(spec["horizon"])
        forward = np.full((n, k), np.nan, dtype=float)
        forward[:-horizon] = close[horizon:] / close[:-horizon] - 1
        labels = pct_cs_labels(forward, members)
        if horizon == 3:
            valid = np.isfinite(cached_labels) & np.isfinite(labels)
            if not np.allclose(cached_labels[valid], labels[valid]):
                raise ValueError("重建 close3 标签与 features.npz 缓存不一致")
            valid_forward = np.isfinite(cached_forward) & np.isfinite(forward)
            if not np.allclose(cached_forward[valid_forward], forward[valid_forward]):
                raise ValueError("重建 close3 前向收益与 features.npz 缓存不一致")

        row_labels = labels.reshape(-1)[rows]
        eligible = ((row_dates >= np.datetime64(config["train_start"], "D"))
                    & (row_dates <= np.datetime64(config["train_end"], "D"))
                    & np.isfinite(row_labels))
        unique = np.unique(row_dates[eligible])
        if len(unique) < 100:
            raise ValueError("冻结窗口内有效训练日不足")
        split = unique[len(unique) * 4 // 5]
        label_end = ts[np.minimum(row_days + horizon, n - 1)]
        train_mask = eligible & (label_end < split)
        valid_mask = eligible & (row_dates >= split)
        train_rows, valid_rows = np.flatnonzero(train_mask), np.flatnonzero(valid_mask)
        if not len(train_rows) or not len(valid_rows):
            raise ValueError("训练集或验证集为空")

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
        component = export_component(booster, spec["name"], horizon, spec["weight"])
        expected = booster.predict(x[valid_rows], num_iteration=booster.best_iteration)
        actual = predict_component_rows(component, x[valid_rows])
        error = float(np.max(np.abs(expected - actual)))
        if error > 1e-10:
            raise ValueError("JSON 推理与 LightGBM 不一致: %.3g" % error)

        report = {
            "horizon": horizon, "weight": float(spec["weight"]),
            "split": str(split), "train_rows": int(train_mask.sum()),
            "validation_rows": int(valid_mask.sum()), "trees": int(booster.best_iteration),
            "feature_count": len(names), "json_max_error": error,
            **validation_metrics(expected, forward.reshape(-1)[rows][valid_mask], row_dates[valid_mask]),
        }
        components.append(component)
        reports[spec["name"]] = report
        print(json.dumps(report, ensure_ascii=False), flush=True)

    model = {
        "format_version": 4,
        "strategy_id": config["strategy_id"],
        "model": "fixed_h135_equal",
        "train_start": config["train_start"],
        "train_end": config["train_end"],
        "retrain_frequency": "frozen",
        "trained_at": utc_now_iso(),
        "feature_names": names,
        "components": components,
        "training": reports,
        "gross_leverage": float(config["gross_leverage"]),
        "volatility_target": None,
    }
    validate_model(model, config)
    output_path = Path(output_path)
    temporary = output_path.with_suffix(output_path.suffix + ".tmp")
    with open(temporary, "w", encoding="utf-8") as f:
        json.dump(model, f, ensure_ascii=False, separators=(",", ":"))
    temporary.replace(output_path)
    print("导出冻结三周期集成 %s（%s）" % (
        output_path, ", ".join(f"{c['name']}={len(c['trees'])}棵" for c in components)), flush=True)
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
