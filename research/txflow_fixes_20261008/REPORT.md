# TxFlow 修复记录 — 2026-10-09

**未全部修完，不能把本报告当作上线验收通过。** 鉴权/网络绑定按用户要求排除。

已在改源代码前完整读取审查 REPORT.md。未访问生产账户、TxFlow explorer；网络测试使用本进程随机端口的本地 axum mock、随机临时 Agent 密钥及模拟账户。没有 commit、push、部署。产物目录保留全部实际失败/通过日志。

## 测试证据规则与执行偏差

下面引用的 before 日志在对应功能修复前运行，断言预期是安全行为，失败证明缺陷。组合预算和对账增量为了离线直测先做了保留旧行为的入口提取：初始 build_txflow_plan 仅返回原 build_plan；初始 merge 函数仍覆盖整个 records。运行断言失败后才加保护。轮换提取初始仍使用时间取模、advance 为 no-op。没有把编译失败或端口冲突当作修复前证据。

没有严格逐项顺序完成：为验证保留盘口后的安全性，一并先写了候选/杠杆红灯测试；因此 01-after-full.log 中元数据测试已通过，但全套仍因尚未修的杠杆测试失败。最终以 final-full.log 为准，不伪称每个中间全套都通过。state-atomic-after-full.log 当时被新加入且初版 fixture 不完整的 PNL 测试阻断；PNL fixture 修正后的 before 日志才是有效证据，后续全套复验。早期 fixed-port 并行碰撞日志不作为功能证据，后改为随机端口。

工作期间检测到并发源代码修改：live.rs Context 导入，以及 trader.rs 中额外的输入合法性检查、挂单风险价格/区间逻辑和三项测试不是本执行链写入。未覆盖这些修改，已向用户询问来源。随后找到并核查 [REPORT2.md](REPORT2.md)、[REPORT3.md](REPORT3.md)、[REPORT4.md](REPORT4.md)：这些工作复用本执行链的历史 before 日志；其额外新增校验的独立先失败证据未提供，不能声称新增校验全部符合用户铁律。它们随当前树一起做最终测试。

## 1. 元数据覆盖（已修覆盖路径，完整性验收仍有缺口）

完整交易 universe/liquid 与 candle_available 分开；元数据在锁内一次替换；至少 20 个、不能比已有列表缩小，校验字段和唯一索引。手动、自动都强制 refresh_meta_only，错误不吞掉。日线回填不能更新交易元数据的新鲜时间。

- 测试名：`backfill_preserves_complete_metadata`、`incomplete_metadata_is_rejected_atomically`。
- 修复前日志：[01-before.log](01-before.log), [01-incomplete-before.log](01-incomplete-before.log)。
- 修复后全套：[03-04-07-after-full.log](03-04-07-after-full.log)；最终也纳入 [final-full.log](final-full.log)。

`01-before.log` 的实际失败输出：

```text
thread 'txflow::fix_regressions::backfill_preserves_complete_metadata' (54980190) panicked at src/txflow.rs:1128:40:
assertion `left == right` failed
  left: 0
 right: 80
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test txflow::fix_regressions::backfill_preserves_complete_metadata ... FAILED
```

`01-incomplete-before.log` 的实际失败输出：

```text
thread 'txflow::fix_regressions::incomplete_metadata_is_rejected_atomically' (54982332) panicked at src/txflow.rs:1159:2:
five-market partial metadata must fail closed
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test txflow::fix_regressions::incomplete_metadata_is_rejected_atomically ... FAILED
```

## 2. 成交结果与恢复（部分修复）

execute 对 TxFlow 在正常结束及中止后重读真实账户，对订单最终目标数量、反向残余和账户净敞口核验；RunResult 暴露 aborted。执行前持久化 pending_recovery，只有已核验成功才写 last_run_at。恢复优先于当天时间戳、自动开关和错峰，恢复时按实际账户生成全组合计划；armed 关闭仍不发单。未知签名请求不在当前调用中重发。

- 测试名：`execution_partial_fill_requires_final_account_verification`、`execution_second_leg_429_requires_final_account_verification`、`execution_timeout_already_filled_is_confirmed_from_account`、`execution_residual_reverse_position_is_not_success`、`pending_recovery_takes_priority_over_today_success_timestamp`。
- 修复前日志：[02-before.log](02-before.log), [02-recovery-before.log](02-recovery-before.log)。
- 修复后全套：[02-recovery-after-full.log](02-recovery-after-full.log)；最终也纳入 [final-full.log](final-full.log)。

`02-before.log` 的实际失败输出：

```text
thread 'txflow::fix_regressions::execution_residual_reverse_position_is_not_success' (54989533) panicked at src/txflow.rs:1205:5:
residual opposite position is not success
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test txflow::fix_regressions::execution_residual_reverse_position_is_not_success ... FAILED

thread 'txflow::fix_regressions::execution_partial_fill_requires_final_account_verification' (54989532) panicked at src/txflow.rs:1193:5:
partial IOC is not a completed risk target
test txflow::fix_regressions::execution_partial_fill_requires_final_account_verification ... FAILED

thread 'txflow::fix_regressions::execution_second_leg_429_requires_final_account_verification' (54989534) panicked at src/txflow.rs:1197:25:
missing final account verification after first leg filled
test txflow::fix_regressions::execution_second_leg_429_requires_final_account_verification ... FAILED

thread 'txflow::fix_regressions::execution_timeout_already_filled_is_confirmed_from_account' (54989535) panicked at src/txflow.rs:1201:5:
account confirms both targets despite transport timeout
test txflow::fix_regressions::execution_timeout_already_filled_is_confirmed_from_account ... FAILED
```

`02-recovery-before.log` 的实际失败输出：

```text
thread 'txflow::fix_regressions::pending_recovery_takes_priority_over_today_success_timestamp' (54991137) panicked at src/txflow.rs:1212:5:
pending recovery must not wait until tomorrow or next slice
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test txflow::fix_regressions::pending_recovery_takes_priority_over_today_success_timestamp ... FAILED
```

## 3. 逐币盘口（报价保留及计划拒绝检查已接入）

price_results 返回每币 Result；prices 保留成功项并记录失败项。缺价持仓拒绝计划；剔除目标后重新检查两腿、因子宇宙与权重风险。没有把剩余单边直接放行。

- 测试名：`partial_books_preserve_successful_prices`、`txflow_nonempty_weights_still_require_factor_universe_and_two_legs`。
- 修复前日志：[03-before.log](03-before.log), [04-candidates-before.log](04-candidates-before.log)。
- 修复后全套：[03-04-07-after-full.log](03-04-07-after-full.log)；最终也纳入 [final-full.log](final-full.log)。

`03-before.log` 的实际失败输出：

```text
thread 'txflow::fix_regressions::partial_books_preserve_successful_prices' (54984394) panicked at src/txflow.rs:1149:20:
one failed book must not erase the successful book: TxFlow 买盘为空
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test txflow::fix_regressions::partial_books_preserve_successful_prices ... FAILED
```

`04-candidates-before.log` 的实际失败输出：

```text
thread 'live::txflow_fix_tests::txflow_nonempty_weights_still_require_factor_universe_and_two_legs' (54985383) panicked at src/live.rs:1421:9:
assertion failed: ensure_candidates(&one, &[], &cfg, 20, "mock").is_err()
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test live::txflow_fix_tests::txflow_nonempty_weights_still_require_factor_universe_and_two_legs ... FAILED
```

## 4. 硬风险门槛（已加静态预算，实盘口径仍有缺口）

有效流动因子宇宙至少 20、每腿至少 3 名；单币名义上限为净值 50%，每腿 150%，净敞口 10%。build_txflow_plan 汇总保留仓、计划最终目标、可能成交挂单，按实际已有仓杠杆（与配置取更保守值）估算保证金，超 margin_buffer 拒绝；逐仓/未知杠杆拒绝普通调仓。HL 仍调用原 build_plan。

- 测试名：`txflow_concentration_and_net_exposure_are_hard_limits`、`txflow_possible_open_orders_cannot_exceed_final_margin`、`txflow_retained_slot_position_cannot_exceed_risk_budget`、`txflow_actual_low_leverage_or_isolated_positions_fail_admission`、`txflow_balanced_portfolio_without_resting_orders_is_admitted`。
- 修复前日志：[04-concentration-before.log](04-concentration-before.log), [04-portfolio-before.log](04-portfolio-before.log)。
- 修复后全套：[04-portfolio-after-full.log](04-portfolio-after-full.log)；最终也纳入 [final-full.log](final-full.log)。

`04-concentration-before.log` 的实际失败输出：

```text
thread 'live::txflow_fix_tests::txflow_concentration_and_net_exposure_are_hard_limits' (54986054) panicked at src/live.rs:1433:9:
assertion failed: ensure_candidates(&concentrated, &liquid, &cfg, 20, "mock").is_err()
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test live::txflow_fix_tests::txflow_concentration_and_net_exposure_are_hard_limits ... FAILED
```

`04-portfolio-before.log` 的实际失败输出：

```text
thread 'trader::txflow_portfolio_tests::txflow_retained_slot_position_cannot_exceed_risk_budget' (55007936) panicked at src/trader.rs:1067:9:
retained slot risk must not bypass admission
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test trader::txflow_portfolio_tests::txflow_balanced_portfolio_without_resting_orders_is_admitted ... ok

thread 'trader::txflow_portfolio_tests::txflow_actual_low_leverage_or_isolated_positions_fail_admission' (55007933) panicked at src/trader.rs:1072:9:
actual low leverage consumes more margin

thread 'trader::txflow_portfolio_tests::txflow_possible_open_orders_cannot_exceed_final_margin' (55007935) panicked at src/trader.rs:1060:9:
possible resting fills must consume risk budget
test trader::txflow_portfolio_tests::txflow_retained_slot_position_cannot_exceed_risk_budget ... FAILED
test trader::txflow_portfolio_tests::txflow_actual_low_leverage_or_isolated_positions_fail_admission ... FAILED
test trader::txflow_portfolio_tests::txflow_possible_open_orders_cannot_exceed_final_margin ... FAILED
```

## 5. 统一请求调度（已修并通过本地路径测试）

info 每次尝试、submit、approve 共用进程级 1 秒调度和 429 冷却。执行/授权等待者优先于普通只读请求；取消等待会释放优先级登记。第二次 429 直接返回，不进入一般错误第三次重试；签名写仍不自动重发。原 300ms 签名 nonce 串行闸门保留，但不再替代全局限速。

- 测试名：`scheduler_reads_retry_429_only_once`、`scheduler_info_submit_approve_share_spacing`。
- 修复前日志：[05-before.log](05-before.log)。
- 修复后全套：[05-after-full.log](05-after-full.log)；最终也纳入 [final-full.log](final-full.log)。

`05-before.log` 的实际失败输出：

```text
thread 'txflow::fix_regressions::scheduler_info_submit_approve_share_spacing' (54995740) panicked at src/txflow.rs:1253:2:
read/write/approve did not share the one-second scheduler: [Instant { tv_sec: 1641056, tv_nsec: 843077541 }, Instant { tv_sec: 1641056, tv_nsec: 843493625 }, Instant { tv_sec: 1641057, tv_nsec: 145486875 }]
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test txflow::fix_regressions::scheduler_info_submit_approve_share_spacing ... FAILED

thread 'txflow::fix_regressions::scheduler_reads_retry_429_only_once' (54995741) panicked at src/txflow.rs:1237:2:
assertion `left == right` failed: 429 permits exactly one retry
  left: 3
 right: 2
test txflow::fix_regressions::scheduler_reads_retry_429_only_once ... FAILED
```

## 6. 跨进程账户锁（同一主机已修）

带 signer 的 TxFlow Client 在任何元数据/下单前取得主账户规范化地址对应的 OS 独占文件锁，持有到 Client 释放；竞争失败拒绝创建签名客户端。锁文件固定放 /var/tmp/stars-txflow-account-locks，跨配置路径共用；崩溃后由 OS 释放。真实子进程验证排他和释放。只读客户端不取锁。

- 测试名：`account_lock_excludes_a_second_process_and_releases_on_drop`、`account_lock_child`。
- 修复前日志：[06-before.log](06-before.log)。
- 修复后全套：[06-after-full.log](06-after-full.log)；最终也纳入 [final-full.log](final-full.log)。

`06-before.log` 的实际失败输出：

```text
thread 'txflow::fix_regressions::account_lock_excludes_a_second_process_and_releases_on_drop' (54999874) panicked at src/txflow.rs:1301:6:
child stdout: 
running 1 test
test txflow::fix_regressions::account_lock_child ... FAILED
```

## 7. 小数杠杆（整数预算已修）

TxFlow 配置保存及 effective_leverage 统一向下取整，3.49 全链路按 3；旧配置文件也走同一有效杠杆。目标、展示预算、重建和实际设置使用有效值；已有仓实际低杠杆由组合风险门槛约束。HL 的 3.49 仍保留。

- 测试名：`txflow_fractional_leverage_uses_actual_integer_budget`、`txflow_actual_low_leverage_or_isolated_positions_fail_admission`。
- 修复前日志：[07-before.log](07-before.log), [04-portfolio-before.log](04-portfolio-before.log)。
- 修复后全套：[03-04-07-after-full.log](03-04-07-after-full.log)；最终也纳入 [final-full.log](final-full.log)。

`07-before.log` 的实际失败输出：

```text
thread 'live::txflow_fix_tests::txflow_fractional_leverage_uses_actual_integer_budget' (54981321) panicked at src/live.rs:1420:9:
assertion `left == right` failed
  left: 3.49
 right: 3.0
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test live::txflow_fix_tests::txflow_fractional_leverage_uses_actual_integer_budget ... FAILED
```

`04-portfolio-before.log` 的实际失败输出：

```text
thread 'trader::txflow_portfolio_tests::txflow_retained_slot_position_cannot_exceed_risk_budget' (55007936) panicked at src/trader.rs:1067:9:
retained slot risk must not bypass admission
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test trader::txflow_portfolio_tests::txflow_balanced_portfolio_without_resting_orders_is_admitted ... ok

thread 'trader::txflow_portfolio_tests::txflow_actual_low_leverage_or_isolated_positions_fail_admission' (55007933) panicked at src/trader.rs:1072:9:
actual low leverage consumes more margin

thread 'trader::txflow_portfolio_tests::txflow_possible_open_orders_cannot_exceed_final_margin' (55007935) panicked at src/trader.rs:1060:9:
possible resting fills must consume risk budget
test trader::txflow_portfolio_tests::txflow_retained_slot_position_cannot_exceed_risk_budget ... FAILED
test trader::txflow_portfolio_tests::txflow_actual_low_leverage_or_isolated_positions_fail_admission ... FAILED
test trader::txflow_portfolio_tests::txflow_possible_open_orders_cannot_exceed_final_margin ... FAILED
```

## 8a. 持久化轮换游标（已修）

TxFlow 专用 SQLite 游标，按实际完成请求的数量推进；市场数及时间不再决定起点。刷新中断后保留已推进位置；测试含重启读取。HL 不调用该表/方法。155 秒请求级硬截止仍未修。

- 测试名：`persistent_rotation_covers_resonant_universe_and_survives_restart`。
- 修复前日志：[cursor-before.log](cursor-before.log)。
- 修复后全套：[cursor-after-full.log](cursor-after-full.log)；最终也纳入 [final-full.log](final-full.log)。

`cursor-before.log` 的实际失败输出：

```text
thread 'txflow::fix_regressions::persistent_rotation_covers_resonant_universe_and_survives_restart' (55021537) panicked at src/txflow.rs:1351:6:
assertion `left == right` failed: resonant wall clock must not starve 180 markets
  left: 60
 right: 240
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test txflow::fix_regressions::persistent_rotation_covers_resonant_universe_and_survives_restart ... FAILED
```

## 8b. 对账增量合并（已修覆盖，新流水清理互斥未修）

仅 TxFlow 在 live 锁内按 tid 去重追加增量，保留当前新订单，不覆盖旧克隆整段 records；水位取 max，账户变化时不合并。HL 保持原分支。

- 测试名：`txflow_reconciliation_preserves_concurrent_orders_and_deduplicates_fills`。
- 修复前日志：[records-before.log](records-before.log)。
- 修复后全套：[pnl-records-after-full.log](pnl-records-after-full.log)；最终也纳入 [final-full.log](final-full.log)。

`records-before.log` 的实际失败输出：

```text
thread 'live::txflow_fix_tests::txflow_reconciliation_preserves_concurrent_orders_and_deduplicates_fills' (55017974) panicked at src/live.rs:1519:9:
assertion `left == right` failed: stale snapshot must not erase a concurrent order
  left: 2
 right: 3
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test live::txflow_fix_tests::txflow_reconciliation_preserves_concurrent_orders_and_deduplicates_fills ... FAILED
```

## 8c. PNL 缓存（已修；止盈归属未修）

TxFlow 最终缓存键含 venue/network、完整账户字符串和 since（REPORT4 的后续修改不再转换大小写）；fills/ledger 请求错误传播，不能吞成成功零收益缓存。该缓存只供 TxFlow 使用。

- 测试名：`pnl_cache_is_account_scoped`、`pnl_read_errors_are_not_zero_profit`。
- 修复前日志：[pnl-before.log](pnl-before.log)。
- 修复后全套：[pnl-records-after-full.log](pnl-records-after-full.log)；最终也纳入 [final-full.log](final-full.log)。

`pnl-before.log` 的实际失败输出：

```text
thread 'txflow::fix_regressions::pnl_read_errors_are_not_zero_profit' (55015672) panicked at src/txflow.rs:1339:44:
read errors must not become cached zero profit
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test txflow::fix_regressions::pnl_read_errors_are_not_zero_profit ... FAILED

thread 'txflow::fix_regressions::pnl_cache_is_account_scoped' (55015671) panicked at src/txflow.rs:1336:56:
assertion `left == right` failed: same interval must not reuse another account PNL
  left: 1.0
 right: 2.0
test txflow::fix_regressions::pnl_cache_is_account_scoped ... FAILED
```

## 审查附加：TxFlow 状态原子保存（仅保存部分已修）

TxFlow 临时文件写入、fsync、旧文件备份、rename、目录 fsync；失败阻止执行前状态确认。HL 保持原直接写文件分支。外层缺字段及损坏 load 的静默默认仍未修。

- 测试名：`txflow_atomic_save_preserves_previous_file_when_temp_write_fails`。
- 修复前日志：[state-atomic-before.log](state-atomic-before.log)。
- 修复后全套：[pnl-records-after-full.log](pnl-records-after-full.log)；最终也纳入 [final-full.log](final-full.log)。

`state-atomic-before.log` 的实际失败输出：

```text
thread 'live::txflow_fix_tests::txflow_atomic_save_preserves_previous_file_when_temp_write_fails' (55012522) panicked at src/live.rs:1493:9:
failed temporary write must abort replacement
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test live::txflow_fix_tests::txflow_atomic_save_preserves_previous_file_when_temp_write_fails ... FAILED
```

## 审查附加：HL 信号新鲜度（代码门槛已修）

TxFlow 必须有 HL 信号源；候选需要昨天的最新完整日线、至少 max(lookback,30)+3 根连续且有效日线，合格映射不足 20 拒绝。HL 自身加载行为不变。跨场所盈利适用性不因此获得证明。

- 测试名：`txflow_signal_requires_hl_source_and_yesterdays_contiguous_bars`。
- 修复前日志：[signal-before.log](signal-before.log)。
- 修复后全套：[final-full.log](final-full.log)；最终也纳入 [final-full.log](final-full.log)。

`signal-before.log` 的实际失败输出：

```text
thread 'live::txflow_fix_tests::txflow_signal_requires_hl_source_and_yesterdays_contiguous_bars' (55028924) panicked at src/live.rs:1544:9:
stale HL factor bars must not produce a TxFlow live signal
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test live::txflow_fix_tests::txflow_signal_requires_hl_source_and_yesterdays_contiguous_bars ... FAILED
```

## 审查附加：净值/PNL 只读客户端（已修选择）

reader_config 根据 venue 选择 TxFlow 无密钥客户端；后台净值及 /api/txflow/pnl 使用同一工厂。HL reader_for 保持原实现。净值曲线口径未统一。

- 测试名：`equity_and_pnl_reader_select_txflow_without_signing_key`。
- 修复前日志：[reader-before.log](reader-before.log)。
- 修复后全套：[final-full.log](final-full.log)；最终也纳入 [final-full.log](final-full.log)。

`reader-before.log` 的实际失败输出：

```text
thread 'txflow::fix_regressions::equity_and_pnl_reader_select_txflow_without_signing_key' (55031766) panicked at src/txflow.rs:1433:6:
TxFlow equity/PNL factory selected Hyperliquid
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test txflow::fix_regressions::equity_and_pnl_reader_select_txflow_without_signing_key ... FAILED
```

## Hyperliquid 约束

未改 HL exchange/info/signer、原 build_plan/execute_inner 的交易逻辑；新增风险、最终对账、信号新鲜度、原子保存、增量合并分支仅对 TxFlow 生效。LiveConfig 的 HL 小数杠杆、HL 旧日线加载、reader venue 有明确负对照；既有 trader/live/web 测试纳入全套。RunResult 增加 aborted 字段及 LiveState 增加 optional pending_recovery 字段，接口/状态 schema 有新增字段，不能声称字节级完全不变。测试证明的是已覆盖路径的行为，不是生产 $570 账户已验证；没有访问该账户。

用户提到的 approval_receipt_persists_configuration_and_does_not_resubmit 在本次实际各完整通过日志中是 ok，未改 txflow_agent.rs 来掩盖它。第一次所谓 baseline 有端口冲突，不算干净树基线。本报告不声称干净树全套已跑过。

## 未修/未验收清单

另见 [NOT_FIXED.md](NOT_FIXED.md)。主清单 1、2、4 的更强验收仍有缺口；止盈归属以及审查附加问题也未全修。

## 最终验证与交付

- `cargo test --release --offline`：**57 passed、0 failed、1 ignored**，58 个测试，126.24 秒，退出码 0，见 [final-full.log](final-full.log)。唯一 ignored 是既有 mainnet 集成测试，没有新增 ignore。
- `cargo check --release --offline`：退出码 0，见 [production-check.log](production-check.log)，证明非 test 的生产分支也能编译；没有运行该二进制。
- `git diff --check`：通过。源代码哈希见 [source-manifest.json](source-manifest.json)，完整未提交补丁见 [changes.patch](changes.patch)。
- 并发补充报告 [REPORT2](REPORT2.md)、[REPORT3](REPORT3.md)、[REPORT4](REPORT4.md) 的较早计数对应各自编译版本，不能替代本报告最终计数。REPORT4 另保存了完整测试前后源哈希一致的证据。
- 本轮没有改变鉴权/网络绑定，未 commit、push、部署。

**结论仍为未全部完成。测试通过是当前已覆盖路径的回归结果，不等同于已满足所有清单验收。**
