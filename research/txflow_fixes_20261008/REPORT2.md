# TxFlow 六项回归验证报告

## 范围与归属

本次开始时工作树已有 `src/exchange.rs`、`src/live.rs`、`src/trader.rs`、`src/txflow.rs`、`src/web.rs` 的未提交修改，其中已经包含下述实现修复。本报告区分已有修复与本次补充：本次补充的是 mock HTTP 客户端的代理隔离，以及新增代码调用不存在的 `Exec::open_orders` 的编译修复（改为现有 `open_order_details`），并重新执行验证；不把已有实现冒称为本次从零完成。运行期间源码与新增测试还有变化，最终结果以最后编译的工作树为准。

未弱化断言、删除测试或添加 ignore。未 commit、push、部署，未访问真实账户或 TxFlow explorer。已有 mainnet 集成测试保持原有 ignore。

## 六项逐条结果

以下“改之前”来自工作目录已有的 before 日志；本次首次运行时，六项实现修复已经存在，六项均通过。因此不能声称本次重新复现了六项全部失败。

| 测试（均位于 `txflow::fix_regressions`） | 改之前怎么失败 | 当前实现改了什么 | 改之后 |
| --- | --- | --- | --- |
| `backfill_preserves_complete_metadata` | `01-before.log`：回填后 universe 从 80 变为 0，`0 != 80`；用户另报告 mock 出现 502 | `publish_metadata` 校验完整交易元数据后在锁内替换；日线集合写入独立 `candle_available`，回填不覆盖 universe。交易入口刷新元数据失败即返回错误 | 通过，80 个市场保持完整，ready 断言保留 |
| `execution_partial_fill_requires_final_account_verification` | `02-before.log`：`partial IOC is not a completed risk target` | TxFlow 实盘执行结束再次读取账户，逐币比较实际仓位与目标，并检查净敞口；部分成交未达目标置 aborted | 通过，aborted 与至少两次账户读取断言均保留 |
| `execution_second_leg_429_requires_final_account_verification` | `02-before.log`：`missing final account verification after first leg filled` | 下单失败保留执行结果并返回 aborted outcome；外层仍执行最终账户核验 | 通过，两次账户读取要求保留 |
| `execution_timeout_already_filled_is_confirmed_from_account` | `02-before.log`：`account confirms both targets despite transport timeout` | 传输错误后由最终真实仓位判定结果；两腿都达标时可确认成功，不因超时重复提交订单 | 通过，成功与账户读取断言均保留 |
| `execution_residual_reverse_position_is_not_success` | `02-before.log`：`residual opposite position is not success` | 最终核验包括目标数量、方向及净敞口，反手残余不能当作成功 | 通过，aborted 与账户读取断言均保留 |
| `partial_books_preserve_successful_prices` | `03-before.log`：`one failed book must not erase the successful book: TxFlow 买盘为空` | `price_results` 逐币保留 Result；`prices` 收集成功价格并记录失败，不再整批 try_collect | 通过，成功币数量 1、价格 100 的断言保留 |

## mock 脚手架补充

本次仅在 `cfg(test)` 的 `Client::new` HTTP builder，以及 fix_regressions 的 mock 客户端上添加 `no_proxy()`。本地 `127.0.0.1` mock 不应经过机器代理，否则会产生与业务无关的 502。生产客户端不增加此选项；服务器绑定、鉴权、测试断言都没有被本次修改。本次没有再次观测到 502，不能断言已复现用户报告的代理故障。

## 恢复状态与 Hyperliquid 隔离

已有实现于 TxFlow 下单前保存 `pending_recovery`，失败或未知结果保持恢复状态，只有最终核验成功才清除并写 `last_run_at`。自动调度优先恢复；恢复计划使用全组合而非日切片。

最终账户核验仅在 `live && exec.is_txflow()` 执行。Hyperliquid 返回原执行结果；恢复、元数据入口和配置约束由 TxFlow 条件控制。`src/hl.rs` 未修改；`src/exchange.rs` 的新增构造器仅 `cfg(test)` 使用。验证是离线测试与源码隔离检查，不代表真实账户或实盘验证。

## 验证结果

- `REPORT2-full-final.log`：一个完整编译版本为 **42 passed、0 failed、1 ignored**。原有测试未出现回归。
- 当前共享工作树随后新增了 `trader::txflow_portfolio_tests` 四个测试，其中三个失败：`txflow_possible_open_orders_cannot_exceed_final_margin`、`txflow_retained_slot_position_cannot_exceed_risk_budget`、`txflow_actual_low_leverage_or_isolated_positions_fail_admission`。这些不在用户指定的六项中，不把较早版本的全绿结果冒充当前全绿。
- 当前新增的 `build_txflow_plan` 尚直接委托 `build_plan`，忽略 open_orders，无法满足这三个新规格。本次检查了函数与失败输出、修复了入口方法名造成的编译错误，未弱化这些断言；组合准入实现未在本次六项范围内完成。
- `cargo check --release --offline` 已通过，记录在 `REPORT2-check.log`；`git diff --check` 已通过。
- 指定命令 `cargo test --release --offline fix_regressions -- --nocapture`：**12 passed、0 failed、0 ignored**，用户指定六项全部通过。完整输出见 `REPORT2-regressions-verified.log`。
- 后一轮完整测试 `REPORT2-full-verified.log`：**42 passed、4 failed、1 ignored**。三项是上述新组合准入测试，另一项是新账户锁测试的子进程环境变量缺失（`NotPresent`）。运行过程中可执行文件被后续编译替换，父子测试版本不一致；无法将该轮称为全绿。
- 随后单独重跑当前账户锁测试：**1 passed、0 failed**，见 `REPORT2-account-lock.log`。其当前脚手架会传递随机账户环境变量。没有降低锁断言。
- 当前完整测试中 Hyperliquid 元数据、配置健壮性、止盈计算、切片、权重和路由隔离测试均通过；没有观测到原有测试回归。用户提到的“既有 31 个通过”与当前套件数量不完全对应，提供实际计数，不编造 31/31。
- 共享工作树持续变化，因此这些结果只证明各日志对应的编译版本；报告不声称之后的其他修改也已得到验证。

## 已知既有失败

用户明确指出 `txflow_agent::tests::approval_receipt_persists_configuration_and_does_not_resubmit` 在干净树上已失败。本次没有修改 `src/txflow_agent.rs`，也不把该问题认领为已修复；当前工作树的测试日志中该项显示通过，但这不能证明干净树问题不存在，也不声称本次修复了该已知失败。
