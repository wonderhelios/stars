# ML 策略：冻结 51 树 close3 + buffer9

这套生产策略复刻已验证的 AlphaPilot 参考模型，替代此前的四模型季度集成。研究回测不构成未来收益保证；本地修改在部署前仍需服务器 dry-run 核对。

## 当前规则

- **模型**：单个 LightGBM，最佳迭代 51 棵树，151 个特征；标签为未来 3 根日 K 收益的当期币池截面百分位。
- **冻结窗口**：2023-06-01 至 2026-04-30。每天只校验模型，不随季度滚动重训；模型缺失或损坏时仍按同一冻结窗口重建。
- **训练复现**：训练/内部验证按唯一时间戳 4/5 切分；训练标签必须在切分日前成熟。Arrow 保留缺失值，特征每 16 列分批构造 Dataset，验证集共用训练分箱。
- **币池**：30 天成交额前 20%，18%/22% 缓冲，成分滞后 2 天。
- **选币**：目标 6 多 6 空。旧多仓仍排名前 9 就保留，旧空仓仍排名后 9 就保留，再按 score 补齐；不是每期全部重建。
- **仓位**：固定总杠杆 2 倍，多空各 1 倍，单仓绝对权重 1/6，无波动目标。
- **执行**：信号日收盘计算，次日 UTC 00:00 开盘执行；从 2026-05-04 起每 3 天调仓。
- **资金费**：保留原始小时记录，日聚合采用 `(t, t+1d]`，所以 UTC 00:00 记录属于前一日。

## 文件

```text
ml/
  strategy_config.json  冻结训练、51 树、buffer9 和固定 2 倍配置
  strategy.py           JSON 推理、buffer9 选币及模型校验
  model.json            当前冻结单模型（训练生成）
  train_model.py        精确训练切分、指标核对及 JSON 导出
  build_features.py     151 特征、动态币池及 close3 标签
  fetch_funding.py      原始小时资金费缓存及正确日聚合
  predict.py            standalone 无旧仓预览，或传入真实持仓生成目标
  place.py              先读真实持仓，再应用 buffer9 并生成订单
  daily.py              更新数据、校验冻结模型、按周期调仓
  test_strategy.py      模型/选币/订单离线测试
```

## 本地训练、预测和测试

```bash
# 默认使用 ml/features.npz 和固定训练窗口
.venv/bin/python ml/train_model.py

# 无交易所持仓的 standalone 预览（不应用 buffer9）
.venv/bin/python ml/predict.py --json

# 离线测试
.venv/bin/python -m unittest discover -s ml -p 'test_*.py' -v
```

`predict.py` 单独运行时明确标记 `preview_without_current_positions=true`。真实计划由 `place.py` 先调用 `clearinghouseState` 获取持仓，再调用 `latest_positions(current_positions=...)` 生成；目标外旧仓归零，执行方式仍为 `next_open`。
