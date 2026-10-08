# REPORT5 — TxFlow 总敞口与损坏状态 fail closed

本轮只修复指定两个漏洞。工作区开始时已有 exchange/live/store/trader/txflow/web 等未提交修改，未回滚或覆盖这些既有工作。参考验证目录的 attacks.log、B.log、extend_harness.py 及 snapshot/src/txflow.rs 的攻击实现。

## 1. 最终核验缺少总敞口与计划外逐币上限

### 修复前真实失败

先写回归测试，再修改生产核验。证据：`REPORT5-gross-before.log`，命令：

```text
cargo test --release --offline report5_unplanned -- --nocapture
prelim=["TxFlow 最终账户已核验逐币目标及净敞口"]
orders=["卖 C1-USDC 开仓 39.150000 · 成交 39.15@100 · 开空", "买 C0-USDC 开仓 39.150000 · 成交 39.15@100 · 开多"]
extra +/-$10000 must abort despite zero net
... report5_unplanned_balanced_gross_must_abort ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 61 filtered out
```

Mock 正常成交计划两腿，再加入 C6-USDC +100、C7-USDC -100（价格 $100，额外名义各 $10,000）。旧核验成功，新测试要求 aborted，故失败。

首次 harness 使用不受 client 识别的 EXTRA-LONG/EXTRA-SHORT，账户解析已报错，不能证明漏洞；该探索结果保留于 REPORT5-before-all.log。改为验证 agent 使用的 C6/C7-USDC 并扩充 mock 市场，才得到上述真正失败；没有改断言或放宽阈值。

### 改动

- `trader::execute` 的 `live && exec.is_txflow()` 最终核验同时检查总名义 `Σ abs(position_value)` 和逐币数量、逐币名义上限，均为目标的 1.01 倍。
- 原有逐腿达标（一个数量精度单位且金额不超过 min_order_usd）、净敞口检查继续保留。1% 是新增上限，不能被最小下单金额放宽；零目标只有浮点 epsilon（数量 1e-10、美元 1e-8），计划外 $10,000 不能通过。
- 检查非有限数据；目标名义用计划中间价计算。价格变化超过 1% 时保守中止，进入恢复，不能静默视为成功。
- `build_txflow_plan` 保存经过风险准入的最终完整目标，包括错峰/调仓带保留仓位；执行单覆盖同币最终目标。该元数据只在 TxFlow 准入函数填充、TxFlow 最终核验读取。HL 下单顺序、金额、执行分支和状态加载未改。
- 超限置 aborted，现有 complete_txflow_execution 保留 pending_recovery，不更新成功时间。

### 修复后

`REPORT5-after-final.log`：五项漏洞回归 5 passed / 0 failed。计划外平衡仓位被中止。

用户指定的精确场景另有 `report5_balanced_1000x_must_abort_and_recover`：明确设置两个目标 +10/-10，mock 实际成交 +10000/-10000，断言 aborted、恢复记录保留、成功时间未更新。诚实说明：放大“计划腿本身”在原代码已被逐腿差值拒绝（REPORT5-before.log 中该测试已通过），不能伪称它修复前失败。真正修复前失败的是验证 agent 的“计划腿达标＋额外平衡仓位”，上面已保存红灯证据。

`report5_realistic_rounding_is_accepted` 检查目标各 10、实际各 10.009，名义误差各 $0.90 仍可接受，未放宽既有达标门槛。

## 2. 状态文件损坏静默丢失恢复记录

### 修复前真实失败

新加载入口最初仅代理旧 load，先运行测试并保存 `REPORT5-before.log`：

```text
... report5_required_fields_and_unknown_fields ... FAILED
missing required account must be explicit error
... report5_corrupt_state_fails_closed ... FAILED
corrupt state must return explicit error instead of losing recovery
test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 58 filtered out
```

测试先保存 pending_recovery 并确认可恢复，再截断为 `{truncated`；旧加载器返回默认状态而非错误。另一测试先确认顶层/配置未知字段可以加载，再删除必填 account；旧加载器仍静默接受。

### 改动

- 增加 TxFlow 专用 `LiveState::load_txflow -> Result`。只有不存在文件表示首次启动；读失败、JSON 解析失败、必填 config/account/key_path/armed 缺失、类型错误、armed 状态缺失签名配置均明确报错。
- 错误打印包含状态文件路径和具体解析/字段原因的警告；不自动使用 .bak、不覆盖损坏文件、不丢弃其待人工修复的恢复证据。
- 保持 serde 对未知字段的兼容；可选配置字段仍用 LiveConfig 的既有合理默认值。未开启实盘的新状态可以有空 account/key_path。
- 启动时错误只禁用 TxFlow：不启动其 background，不安装正常交易/配置/授权 handler。`/txflow` 和 `/api/txflow/*` 返回带具体错误的 HTTP 503；HL router 和后台任务正常启动。内部占位默认状态没有可用 handler 或任务，不能拿它继续下单。
- 增加 `report5_corrupt_txflow_routes_reject_every_operation`，覆盖页面、状态读取、配置、下单、授权、reset 全部 503 且包含显式错误，防止网页显示默认 $0 并误认为正常。

### 修复后

`REPORT5-after-final.log` 中两个加载回归均通过，包含真实警告：

```text
警告: TxFlow 状态文件损坏或不可读取 .../state.json；拒绝启动实盘，请修复状态文件（恢复记录不得丢弃）: 缺失必填 config.account
警告: ...: key must be a string at line 1 column 2
```

## 完整验证

最终代码执行 `cargo test --release --offline`，退出码 **0**。完整原始输出：`REPORT5-cargo-final.log`。

```text
running 64 tests
... approval_receipt_persists_configuration_and_does_not_resubmit ... ok
... report5_corrupt_txflow_routes_reject_every_operation ... ok
test result: ok. 63 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 144.28s
```

原有 57 项实际通过的测试继续通过（覆盖用户要求的 56 项）；新增 6 项通过。唯一 ignored 为既有 `txflow::public_tests::mainnet_public_reads`，未添加任何 ignore。新增回归单独最终执行见 `REPORT5-regressions-final.log`：**6 passed / 0 failed / 0 ignored / 58 filtered out**。`git diff --check` 通过；仍有既有 unused variable `cfg` 编译警告。

已知历史失败项 approval_receipt 本次最终实际 **ok**；这一结果只如实记录，不声称审批业务问题已被定位或修复。

中途隔离路由测试发现 Axum 0.7 不支持 `{*path}` 写法，运行失败（`REPORT5-isolation-before.log`）：`Invalid route ... catch-all parameters are only allowed at the end of a route`。已改为该版本的 `*path`，没有更改断言，重新跑完整测试。中途日志也保留，不能当最终通过证据。

既有失败项：`txflow_agent::tests::approval_receipt_persists_configuration_and_does_not_resubmit`。用户提供的历史基线称它失败，本轮没有修改它的断言或审批实现；只按本机代理要求对 mock client 加 no_proxy()。本轮中途完整运行实际显示该项 ok（REPORT5-full.log，62 passed / 0 failed / 1 ignored），不能捏造仍失败，也不将其宣称为本轮审批逻辑修复；最终日志也确认为 ok。

## 未做及边界

- 未访问真实账户、TxFlow explorer；新增交易场景只访问本地 127.0.0.1 mock，HTTP client 使用 no_proxy()。
- 既有 approval mock 的测试 client 也改为 no_proxy()，仅测试构造器，未改授权行为或断言。
- 未改鉴权、网络监听地址或绑定；未 commit、push、部署、重启现有真钱服务。
- 未自动恢复损坏状态或擅自清掉恢复记录；需人工修复文件后重新启动 TxFlow 功能。
- 未把既有其他漏洞/测试的修复归功于本轮；未删测试、加 ignore 或弱化断言。已有只读 mainnet 集成测试的 ignore 保持原样，不联网执行。
