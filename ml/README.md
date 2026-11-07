# ML 策略（复刻 alphapilot LightGBM）

复刻 alphapilot「ML Alpha Long-Short」的 LightGBM 模型，与实盘动量策略并行跑，吃「小币分化」行情。

## 目录

```
ml/
  model.json        训练好的 LightGBM 模型（42 棵树，151 特征）
  train_model.py    训练 + 导出模型（从 features.npz → model.json）
  build_features.py 算 151 个特征 + 动态 head + 标签（从 data/ → features.npz）
  predict.py        加载模型 + 预测 score + 输出前6多后6空
  fetch_ohlcv.py    拉 Hyperliquid 日线 OHLCV（2023-04 起）
  fetch_funding.py  拉资金费率 + premium（2023-05 起）
  fetch_oi.py       拉 OI 月度 parquet（2023-04 起）
  data/             原始数据（hl2 / hl_funding / hl_oi，.gitignore 不提交）
  features.npz      特征矩阵（161M，build_features.py 生成，.gitignore 不提交）
```

## 策略口径（与 alphapilot 对齐）

- 信号：151 个特征（Alpha158 价量 + 加密原生 fr/prem/oi/tc/log_dv）→ LightGBM → score
- 标签：`pct_cs(forward_3d)`（未来 3 天收益在当天 head 成分内的截面百分位）
- 币池：head = 30 天成交额前 20%（进出 18%/22% 缓冲）
- 选币：score 排序，前 6 做多、后 6 做空，等权 1/12，2x 杠杆
- 调仓：每 3 天（日线），无止损、无波动目标
- 手续费：0.03%

## 依赖

```bash
pip install lightgbm numpy pandas pyarrow
```

## 部署流程

```bash
# 1. 拉数据（首次或每日）
python ml/fetch_ohlcv.py      # 日线 OHLCV
python ml/fetch_funding.py    # 资金费率 + premium
python ml/fetch_oi.py         # OI parquet

# 2. 算特征（每次拉完数据后）
python ml/build_features.py   # → features.npz

# 3. 预测 + 输出持仓
python ml/predict.py          # 打印最新一天前6多后6空
python ml/predict.py --json   # 输出 JSON（给下单/监控）
```

## 训练（复现时用）

```bash
python ml/train_model.py      # 用 features.npz 训练 → model.json
```

训练参数与 alphapilot 一致：`lr=0.05, num_leaves=31, min_data_in_leaf=200,
feature_fraction=0.8, bagging_fraction=0.8, bagging_freq=1, lambda_l2=10,
seed=42, num_boost_round=1000, 早停 100`。训练区间 2023-06 ~ 2026-04-30，
切分 80/20，purge 3 天前瞻。

## 复现验证结果

- 验证 Rank IC：0.114（alphapilot 0.108）
- 25 天（10-09 ~ 11-05）回测：+23.9%（alphapilot 官方 +23%）
- 半年（05-04 ~ 11-05）回测：+81.4%（alphapilot 官方 +71.6%）
- head 历史币：139（alphapilot 124，我们宇宙 234 币 vs 它 165 永续）
