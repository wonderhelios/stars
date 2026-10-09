# TxFlow 币安信号源切换（2026-10-09）

## 输入核实与真实导入结果

- `mapping_verified.json` 是 JSON 数组，82 条映射，以 `txflow_name` 作为库内币名（含 `-USDC`）。实际字段包含 `tx_unit`、`binance_unit`、价量倍数；导入器不使用这些倍数，因为 CSV 已经换算。
- `daily/` 实际有 **114 个 CSV**，全部表头为 `ts,open,high,low,close,volume`。按 82 条映射逐个读文件，其余 32 个候选不读。
- `ts` 是 UTC 日开盘 Unix 毫秒，映射内输入均按 86,400,000 毫秒连续；检查正有限 OHLC、非负有限基础币 volume、每日 UTC 对齐。价格按输入原值 USDT 存储，volume 按输入原值存储，不再换算。
- Store 实际表为 `candles(coin TEXT,t INTEGER,o REAL,h REAL,l REAL,c REAL,v REAL)`，主键 `(coin,t)`。复用 `Store::upsert_candles`；`trader::load_panel` 经 `all_panels` 读取这七列，按 coin、t 排序生成 PanelEntry。
- **实际导入 81 币、70,267 根日线**。唯一跳过：`HYPE-USDC`，仅 14 根，要求至少 33 根（现有默认 `max(lookback,30)+3`）。映射内无缺失文件。
- 本地输出库：`research/txflow_binance_20261009/binance-candles.sqlite`。两次实际命令输出分别保存于 `import-first.json`、`import-second.json`，完全一致；第二次后 SQLite 仍为 81 币 / 70,267 行。
- 所有映射内输入最新开盘为 `1791331200000`，即 **2026-10-07 UTC**。因此 10 月 9 日的 TxFlow 面板检查会拒绝这批旧数据，不能声称已具备当天实盘信号。

## 改动与对应测试

| 文件 | 改动 | 对应测试 |
|---|---|---|
| `src/binance.rs`（新增） | 离线导入器；映射筛选、CSV 校验、短/缺失跳过、幂等 upsert；拒绝目的库含未映射或本次跳过的旧币；HL/TxFlow 库路径隔离，含相对路径、符号链接和硬链接 | `binance::tests::import_is_idempotent_filters_mapping_preserves_units_and_skips_short_missing`；`binance::tests::import_command_rejects_hl_and_tx_database_aliases` |
| `src/main.rs` | `import-binance` 子命令在建立客户端/启动后台任务之前返回；HL AppState 无币安库，TxFlow AppState 使用独立币安库，保留 hl_store | 上述导入命令隔离测试；实际两次 cargo/二进制导入验证；`web::txflow_routes_tests::txflow_page_and_configuration_are_isolated_from_hyperliquid` 验证状态数据源选择 |
| `src/web.rs` | 新增 binance_store、统一 signal_store accessor；手动调仓与全仓重建改用 accessor | `web::txflow_routes_tests::txflow_page_and_configuration_are_isolated_from_hyperliquid`（新增 Arc 身份断言，币安与 HL 库明确不同）；现有 unavailable/route 测试 |
| `src/txflow.rs` | 自动调仓改用同一 signal_store accessor；两个测试构造器补字段 | 上述 accessor 身份断言；现有 `txflow::fix_regressions::execution_partial_fill_requires_final_account_verification` 等完整套件；自动调仓成功下单未访问真实账户验证 |
| `src/txflow_agent.rs` | 仅测试 AppState 构造器补 None 字段 | 现有 `txflow_agent::tests::*` 全套 |
| `src/live.rs` | TxFlow 从币安库直接读 TxFlow 币名，排名前不按交易市场交集过滤；保留新鲜连续检查；权重后筛选且不归一化；显示币安独立组合与真实面板/市场数量 | `live::txflow_fix_tests::binance_universe_differs_from_hl_and_markets_without_changing_weights`；`live::txflow_fix_tests::txflow_signal_requires_binance_source_and_yesterdays_contiguous_bars`（原测试改名/适配）；原有权重测试 |

差异场景测试建立 24 个币安币、1 个完全不同的 HL 币、只有 1 个可交易 TxFlow 市场；TxFlow 面板仍有全部 24 币，HL 路径仍只读取自己的 HL_ONLY。权重后删掉不支持的腿，剩余 0.2 保持原值。显示测试明确断言 `可排名 82 币 / TxFlow 共 227 市场`；生产显示动态真实数量，此批数据最多 81 币，映射总数单独显示 82，绝不将 82 条映射冒充 82 个可用排名币。

`src/trader.rs`、`src/momentum.rs`、`src/store.rs` 均未修改，权重算法一行未改。HL 分支仍执行 `trader::load_panel(store)`，忽略传入的其他信号库；HL 后台调用、交易客户端和鉴权/绑定均未改。仓库开始时 `src/live.rs` 已有用户的排名后过滤改动，本次在其基础上保留顺序和不归一化语义。

“丢弃不能交易的币”与“依据换成币安宇宙”同时处理为：权重后必须属于币安面板，并且有 TxFlow 市场才能保留。仅检查币安成员资格不能发现真实缺失的交易市场，因此保留执行资格检查；该检查不参与排名。

## 完整测试结果

命令：`cargo test --release --offline`，退出码 **0**。

```text
test result: ok. 66 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 77.84s
```

最终为 **66 passed / 0 failed / 1 ignored**，共 67 个测试（基线 63 passed，新增 3 个测试，原有数据源测试适配改名）。

最终完整输出保存于 `cargo-test-release-offline-final.log`。本地 mock 使用现有 `no_proxy()`；未访问真实账户或 TxFlow explorer。测试日志中的编译警告不当作失败，也不隐瞒。

## 服务器部署步骤（仅说明，没有执行）

先将此次源码以及映射表和已经完成单位换算的 CSV 同步到服务器项目目录。更新 CSV 至最近完整 UTC 日；本批截至 10 月 7 日，10 月 9 日直接使用会被拒绝。没有实现新的币安下载器，不重做既有面板研究。

在服务器项目根目录，以能写入目标目录的服务用户执行（已有缓存依赖时用 offline；否则先正常构建）：

```sh
cargo build --release --offline
STARS_DB=/var/lib/stars/candles.sqlite STARS_TXFLOW_DB=/var/lib/stars/txflow-candles.sqlite cargo run --release --offline -- import-binance /var/lib/stars/binance-candles.sqlite research/bn_panel_20261008/data/mapping_verified.json research/bn_panel_20261008/data/daily
```

同一条导入命令可重复执行。也可使用已编译的 `target/release/stars import-binance ...` 避免重编译。应替换上面的 HL/TxFlow 路径为服务器**实际配置**；导入器会拒绝写入这两个库及其别名。若旧币安目的库包含本次映射外/跳过的币，命令报错，使用新的独立币安库路径导入，不会默默混入旧宇宙。

服务环境设置：

```sh
STARS_BINANCE_DB=/var/lib/stars/binance-candles.sqlite
```

保留原 `STARS_DB`、`STARS_TXFLOW_DB` 和所有鉴权/绑定环境；如未设置 STARS_BINANCE_DB，默认使用 TxFlow 库同目录的 `binance-candles.sqlite`。导入不会触发网络、服务器启动或下单。由操作者审核数据新鲜度、导入输出及本报告后，安排现有服务更新/重启流程；本次没有执行部署或重启。

## 未做与限制

- 未 commit、push、部署、重启、访问真实账户、TxFlow explorer；按明确限制执行。
- 未伪造“82 个可排名币”：HYPE 太短，实际 81；没有补造历史。
- 未下载新数据；现有输入已过期，需外部更新 CSV 并重跑导入。导入器是离线缓存入口，不是日线持续采集器。
- hl_store 保留作对照入口，没有自动回退到 HL；自动回退会在失败时悄悄改变策略宇宙，缺币安库时拒绝 TxFlow 计划，HL 不受影响。
- 未做真实成交验证。测试证明离线数据路径/状态隔离与权重过滤行为，不代表服务器上的环境路径已设置或真实交易已切换。
