# 清算级联与持仓结构：可复现验证

运行顺序（Python 3，numpy/pandas/requests）：

```sh
cd /Users/wonder/Code/stars/research/liq_oi_20261008
python fetch_data.py
python check_baseline.py
python validate.py
python hourly_events.py
python descriptive.py
python write_report.py
```

`fetch_data.py` 只作公开只读查询。Binance 缓存位于 `/tmp/liq-oi-binance/`，约 19,152 个小 JSON；失败响应也记录，不前向填充 OI。归档固定 BTC ETH SOL XRP DOGE ADA AVAX LINK LTC BCH DOT ATOM BNB NEAR APT ARB 16 币，不按回测结果筛币。下载前已固定 8 个日频方向、6 个小时配置；无收益驱动的阈值/持有期/混合比例搜索。

日频验证参考 `trader.rs` 当前实现；`engine.py` 记账复用此前 `marginal_20261008` 引擎，不运行该研究候选。`check_baseline.py` 用独立字典循环核对 Rust 因子计算，不把旧的 1.79 作为此引擎输出。

主结果见 `report.md`，原始精度见 `results_daily.json`、`results_hourly.json`，各日各相位收益见 CSV。所有 bootstrap 都按共享日索引抽块，跨币与候选相关性保留；8/6 两族各用 maxT，再按两族 Bonferroni，整个 14 候选族 FWER 控制。

成交按下根 open 的 taker + 3bps 滑点。maker 费率仅作理想下界敏感性，不模拟 maker 成交，也不声称任何 maker 收益。资金费率缺完整历史，因此检验通过也不能直接视为实盘收益验证；本轮失败候选不会绕过此限制。
