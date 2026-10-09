# TxFlow / Binance daily panel

规范入口是本目录 `REPORT.md` 和 `data/mapping_all.csv/json`。`alternate_batch/` 保留另一批采集结果；其倍数字段约定不同，不要与本批混用。两批通过的 82 个币名单一致。

本批信号数据来源为 **币安 USDT 现货**，校验参照为 Hyperliquid。TxFlow 仅调用一次只读 `/info` 的 perpMeta；没有调用 explorer，也没有取其价量。代理是 `http://127.0.0.1:7897`；采集最多 2 并发，每次超时 40 秒，瞬态失败最多重试 2 次。永续探针返回 HTTP 418，出口 IP 临时封禁，因此未采用永续。

本目录日期是研究批次名。截止固定为 **2026-10-08 00:00 UTC**，最新完整日线起点是 **2026-10-07 00:00 UTC**；不会加入未收盘日线。

## 数据结构

所有日线 CSV 表头为 `ts,open,high,low,close,volume`。ts 是 UTC 开盘 Unix 毫秒；volume 是基础币数量而非成交额。不补值、不伪造，每币最多 1000 根完整日线。

- `data/txflow_markets.csv/json`：完整 227 市场。
- `data/binance_spot/<SYMBOL>.csv`：币安原始单位数据。
- `data/daily/<TxFlow名>.csv`：币安数据换算到 TxFlow 单位；**也包含未通过候选，禁止整目录无筛选读取**。
- `data/hyperliquid/`：独立校验参照。
- `data/validation/<TxFlow名>.json`：全部同 UTC 日的价格/成交量计算证据。
- `data/mapping_all.csv/json`：每市场状态与转换字段；`mapping_verified.json`：82 个映射校验通过项；`mapping_strict.json`：79 个无额外提示的候选。
- `data/raw/`：本批真实响应与历史失败记录；`requests.jsonl`：HTTP、请求时间、重试与耗时；`provenance.json`：初始探针及元数据请求证据。
- `data/checkpoints/`：逐币即时保存，`coverage.json`：覆盖统计，`run_config.json`：冻结日期与阈值，`manifest.json`：数据文件 SHA256。
- `verification.json`：离线验证结果；`src_integrity.json`：源码与目录内既有快照一致性结果。

## 单位

`tx_unit` / `binance_unit` / `hl_unit` 是每个市场报价单位包含的基础币枚数。价格换算为 `Binance OHLC × bn_price_to_tx_mult`；成交量为 `Binance volume × bn_volume_to_tx_mult`。两个乘数互为倒数。

1000BONK-USDC → BONKUSDT：价格 ×1000，量 ÷1000。PEPEUSDT/SHIBUSDT 为单币现货，映射无需换单位；其 HL kPEPE/kSHIB 对照要按 1000 币单位归一。实际 TxFlow 是 PEPE-USDC，清单中没有 PEPU-USDC。

表中原始价格比 = HL close / Binance close；原始量比 = Binance volume / HL volume。归一价格比 = `(Binance close / binance_unit) / (HL close / hl_unit)`；归一量比 = `(Binance volume × binance_unit) / (HL volume × hl_unit)`。

LIT-USDC 明确是 Lighter Protocol，而币安旧 LITUSDT 属于 Litentry，故剔除。无可靠 HL 对照和仅同 ticker 的 HL 现货也不会进入验证名单。量级差异同时受跨交易所活跃度影响，不能仅凭量比证明单位错误。

## 复现

```bash
python3 collect_validate.py
python3 verify_artifacts.py
```

重复运行复用成功原始响应与冻结日期；已失败项可补采。离线验证对照原始 K 线逐值检查 CSV、倍数转换、同日价量计算、已收盘截止、完整名单和哈希。两份映射候选都不代表策略已适配；本次没有修改 src、commit、push 或部署。
