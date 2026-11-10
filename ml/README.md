# ML 策略：四周期 LightGBM 集成

151 个价量与加密原生特征，经过 `close3 / close5 / open3 / open5` 四个 LightGBM 回归模型，预测动态 head 内的横截面收益分位。每日先将四个模型的预测分别转成 head 内截面分位，再等权平均。

## 当前策略口径

- 币池：30 天成交额前 20%，18%/22% 缓冲，成分滞后 2 天。
- 选币：集成 score 前 6 做多、后 6 做空，共 12 个仓位。
- 仓位：**固定总杠杆 2 倍**，多空各 1 倍，单仓绝对权重 `1/6`；没有波动目标或动态杠杆。
- 执行：信号日收盘计算特征，下一日开盘成交，每 3 天调仓。
- 训练：每季度扩展窗口重训；四个标签的训练和内部验证数据都必须在季度开始前成熟。
- 标签成熟期：`close3=3`、`close5=5`、`open3=4`、`open5=6` 天。open 标签从下一日开盘持有到相应未来开盘。
- 手续费研究口径：单边名义成交额 0.03%。

## 文件

```text
ml/
  strategy_config.json   唯一策略参数来源（模型、6/6、固定 2 倍等）
  ensemble.py           共享标签、JSON 推理、截面集成和选币逻辑
  model_ensemble.json   当前季度四模型文件（训练生成）
  model.json            历史单模型，仅保留作研究对照，不作为回退
  train_model.py        季度扩展窗口训练与 JSON 导出
  build_features.py     生成 151 个特征、动态 head 和历史 close3 标签
  predict.py            输出 long / short / 带符号 weights
  place.py              按 weights × equity / price 生成订单，目标外旧仓归零
  daily.py              更新数据、重算特征、检查季度模型、按周期调仓
  test_ensemble.py      不访问网络的离线单元测试
```

## 本地训练与 dry-run

```bash
# 使用当前 features.npz，训练当前 UTC 季度模型
.venv/bin/python ml/train_model.py

# 也可明确指定成交日（季度首日训练只使用此前成熟标签）
.venv/bin/python ml/train_model.py --as-of 2026-11-11

# 只生成目标持仓，不访问交易接口
.venv/bin/python ml/predict.py --json

# 离线测试
.venv/bin/python -m unittest discover -s ml -p 'test_*.py' -v
```

`daily.py` 在调仓日检查模型季度。模型缺失、损坏、训练失败或季度不匹配都会中止，**不会回退 `model.json`，也不会沿用旧模型下单**。`place.py` 只接受新预测结果中的带符号 `weights`，不再自行假定每仓 `equity/6`。

## 研究边界

四模型候选来自 `research/ml_autoresearch_20261110/` 的滚动研究。研究中的收益与回撤不构成上线保证；固定 2 倍版本也不等同于研究里曾评估的 2.1 倍加波动目标版本。生产改动需要先完成本地训练、JSON 推理一致性、离线订单 dry-run 和小额验证，再另行决定是否部署。
