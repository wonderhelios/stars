# 永续合约资金费率事件观察

本程序每 5 分钟扫描 OKX、Binance 和 Hyperliquid 永续合约，记录过去 24 小时价格涨跌超过 3%、预测资金费率高于 0.05% 的信号。每家交易所的同一合约在 24 小时观察期内只记录首个信号，即使价格变化让信号类别改变也不会重复入场。所有信号按做空方向观察 1、4、8、24 小时后的价格变化。网页和 OKX `paper report` 展示的净收益估算统一扣除 0.15% 往返交易成本，不包含实际资金费收入、真实成交滑点或强平影响，因此不是账户收益。

Hyperliquid 扫描每轮通过 `perpDexs` 发现主 DEX 和全部 HIP-3 DEX，不依赖静态部署者名单；若某个 DEX 请求失败，日志会记录失败和成功覆盖数。

## 五仓研究的数据采集

每轮扫描会把满足现有信号条件的**所有**候选写进各平台数据库的 `candidate_snapshots`，包括因 24 小时去重而没有新增到 `signals_v3` 的候选。`scan_runs` 只在本轮扫描正常结束时写入；做跨平台研究时应连接这张表，并优先使用 `failed_count = 0` 的轮次。两张表会随现有服务自动创建，不改变网页信号统计。

快照记录扫描开始时间、该候选实际观察时间、信号类别、参考价、费率及结算周期、下次结算时间、24 小时成交额、买卖盘价格和报价时间。OKX、Binance 的报价为最优买卖价；Hyperliquid 的 `quote_kind = impact` 是交易所提供的冲击买卖价，不能当成最优盘口，也不能直接视为 500 美元的可成交价。缺失的字段保持 `NULL`，不要用零填充。三个数据库仍分别使用 `OKX_QUANT_DB`、`OKX_QUANT_BINANCE_DB`、`OKX_QUANT_HL_DB` 指定的文件。

例如检查最近已完成的扫描及其快照数量：

```bash
sqlite3 -header -column /var/lib/okx-quant/hyperliquid.sqlite \
  'SELECT datetime(r.scan_started_at/1000,"unixepoch") AS scan_utc, r.market_count, r.prefiltered_count, r.recorded_count, r.failed_count, COUNT(c.inst_id) AS stored FROM scan_runs r LEFT JOIN candidate_snapshots c USING (scan_started_at) GROUP BY r.scan_started_at ORDER BY r.scan_started_at DESC LIMIT 5;'
```

三平台的扫描仍错开启动，因此复盘时须按候选的 `observed_at` 和报价时间对齐，并限制快照年龄；不能再按旧信号触发时间简单地“先到先得”。候选快照目前只记录决策时可见的信息，尚未记录所有候选的未来退出价格或实际资金费流水。

## 重新部署

### 研究页的实盘执行对照

Hyperliquid 研究页现在把旧的 T+24h **价格观察**与 Hyper Fly 的**实际执行汇总**分开显示。后者读取同服务器 Hyper Fly 看板的本机接口，展示已实现盈利、亏损、净额、持仓数量和最近一轮筛选。实盘成交经过机器人的入场、仓位、止损及风险规则；历史研究信号仍然不能据此补算为策略收益。

此连接默认关闭，避免把实盘财务汇总自动公开到 3000 端口。如果你愿意在研究页展示这些汇总，在 Edgeboard 服务环境中设置 `HYPER_FLY_RESEARCH_PORT=19527` 并重启 Edgeboard。该端口必须是同一台服务器上 Hyper Fly **实盘看板的本机端口**；程序只访问 `127.0.0.1`，不会返回账户地址、币种或逐笔交易。Hyper Fly 不可用时显示不可用，不沿用旧数据伪装成最新结果。不要把模拟看板端口填在这里。

从现在起，Hyperliquid 候选快照还会记录当时的最大杠杆和数量精度。旧快照没有这些字段，也没有盘中的标记价格和完整 L2 盘口，因此无法精确回放旧信号的 2% 止损及实际成交。

```bash
cargo build --release
```

默认服务监听 `0.0.0.0:3000`。数据库路径可用 `OKX_QUANT_DB`、`OKX_QUANT_BINANCE_DB`、`OKX_QUANT_HL_DB` 指定；默认分别位于 `/var/lib/okx-quant/` 下。请确保运行用户可写。三个数据库现在都使用统一的 `signals_v3` 表；旧表不会进入新统计。建议重新部署前备份旧数据库，并在需要全新数据文件时换用新路径。

记录的入场价是扫描时读取的行情价，不是实际成交价。到期价格取在目标时刻或之后最先收完的 1 分钟 K 线收盘价，最多比目标时刻晚 1 分钟。交易所接口暂时不可用时会重试；仍失败的价格留空，后续跟踪会再次尝试。

网页请求短暂失败时会保留浏览器中上次成功的数据，并明确显示断线状态和最后成功时间。HTTP 502 通常还需要检查网关与应用服务之间的连接，以及服务器上的服务日志；前端保留旧数据不代表行情仍在更新。

如果只有访问网页时偶发 502，先从访问网页的电脑对比普通请求和绕过本机代理的请求：

```bash
curl -sS -o /dev/null -w '%{http_code} %{time_total}\n' http://8.219.113.154:3000/api/snapshot
curl --noproxy '*' -sS -o /dev/null -w '%{http_code} %{time_total}\n' http://8.219.113.154:3000/api/snapshot
```

直连正常而普通请求失败时，应在本机代理规则中让服务器 IP 直连。服务器位于海外并不代表浏览器访问它时会自动绕过本机代理。OKX 行情响应启用了 gzip 压缩，以减少传输时断流的机会。

OKX WebSocket 若超过 90 秒没有收到任何有效行情，会主动断开并重连；若重连后仍无新行情，请检查服务日志里的 `okx ws` 错误以及服务器到 `ws.okx.com:8443` 的连接。

## 历史研究

```bash
cargo run --release -- research BTC-USDT-SWAP ETH-USDT-SWAP
cargo run --release -- research scan 200
```

历史研究优先使用**实际结算**资金费率，缺失时回退到历史预测费率，在结算后一小时的 K 线开盘价模拟入场。同一合约、同一费率方向的历史事件也按 24 小时去重。这只是探索性事件研究，与实时扫描预测资金费率并立即记录价格的规则不同。`scan` 根据**当前**报价金额选币种，不能消除历史币池偏差。其做空超额数字不能直接视为可交易 alpha；判断策略仍需按事先固定规则、独立后续数据、实际成交成本与同时间基准验证。不同交易所的同一底层币种仍可能同时出现，在跨交易所汇总时应按币种和时间聚类，而不是当作相互独立的交易。
