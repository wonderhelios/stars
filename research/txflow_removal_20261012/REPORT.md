# TxFlow 移除完成报告（2026-10-12）

## 验收结论

已按本轮决定完成生产代码、路由、UI、专属工具与配置的移除。`cargo build --release --locked` 退出 0、零 error；`cargo test --release` 为 **36 passed / 0 failed / 0 ignored**；指定残留搜索为空；HL 9 条路由及其 handler 全部保留。

基线为 `489359e0ba18aa2837acea807fb12c0edce76db3`。开始时只有上轮报告目录未跟踪，生产源码没有已有改动。上轮阻断报告原文另存为 [REPORT.blocked.md](REPORT.blocked.md)，没有用本次结果覆盖历史证据。未 commit、push、部署、重启；没有启动服务或发出实盘交易请求。

构建有 1 个 warning：用户要求保留的 `account_meets_targets` 暂无调用；测试另有基线已有的 unused `cfg` warning。未加 allow、未屏蔽测试、未把最终核验接入 HL 来消除警告。

## HL 行为保留证据

- `merge_txflow_reconciliation` 改名为 `merge_reconciliation`，**函数体逐字节相同**；同步修改 `web::live_status` 和测试调用。
- `first_receipt_watermark_survives_merge_without_fills` 保留全部断言，仅更新调用名；并发订单/成交去重测试仅改模块、测试与函数名称。
- `run_gate`、`exec_gate` 保留。`live_run` 仍先拿两道锁、后台持锁执行、首次实盘执行前记资金起点；HL 保存失败处理、last_run_at 更新、成功/错误处理与原 HL 分支一致。
- 删除 `signal_store()`、`binance_store`、`hl_store` 及参数传递；`signal_panel(store)` 仍直接执行 `trader::load_panel(store)`。HL 自动调仓、手动调仓、转全仓都用自己的 Store。
- `build_plan`、`target_weights`、`trader::load_panel` 函数体不变。`execute` 直接返回 `execute_inner` 的结果；`execute_inner` 与删除前 HL 分支逐字一致（仅去除 TxFlow 条件分支）。
- `account_meets_targets` 保留；以空 HashMap 开始，仍逐条按 `plan.orders` 写入目标，并保留原容差/方向/门槛判定；未新增任何 HL 调用。
- `min_order_pct`、`net_deposit`、`capital_flows`、`exec_daily`、`start_equity`、`capital_baseline` 及 HL 止盈、对账、资金功能保留。
- `src/hl.rs`、`src/momentum.rs`、`src/paper.rs` 完全不变；`main.rs` 中刷新、研究、CLI trade 函数完全不变。HL 原始路由表完全相同。

上述源码比较通过 [verify_preserved.py](verify_preserved.py) 执行，实际输出见 [preserved.log](preserved.log)。这验证了指定函数与原 HL 分支的保留，不声称已进行线上交易验证。

## 删除范围

### 专属文件

- `config/binance-mapping.json`
- `docs/txflow-execution-audit.md`
- `src/binance.rs`
- `src/txflow.rs`
- `src/txflow_agent.rs`
- `src/txflow_audit.rs`
- `src/txflow_points.rs`
- `static/txflow-agent.js`
- `static/txflow-points.js`
- `static/txflow.html`
- `tools/test-txflow-agent.mjs`
- `tools/test-txflow-audit.mjs`
- `tools/test-txflow-points.mjs`

`tools/test-txflow-audit.mjs` 的共享成交回执解析断言迁至 `tools/test-receipts.mjs`，只删除审计 UI 部分。`research/` 的历史报告、快照、回放材料作为审计证据保留，未删除运行数据、账户文件、密钥或数据库；本次“残留为空”的验收范围严格为用户指定的 `src/ static/`。

### 共享文件中的删除项

| 文件 | 删除或化简内容 |
| --- | --- |
| `src/main.rs` | 5 个专属模块声明、import/fetch-binance CLI 入口、TxFlow 独立数据库/状态初始化和降级占位、行情/审计后台任务、路由 merge；去掉仅供专属使用的字段初始化。 |
| `src/web.rs` | TxFlow 全部路由、不可用 router、页面/JS handler、PnL、TP handler；signal_store、binance_store、hl_store、refresh_gate、candle_available；状态/杠杆/调仓的 TxFlow 分支。 |
| `src/live.rs` | config.txflow、RecoveryState、pending_recovery、begin/complete_txflow_execution、load_txflow、原子保存专属分支、5 个 tx_* snapshot 字段；专属保证金/行情/风险准入/恢复/回执标记分支；fresh_binance_panel、drop_unsupported、专属信号说明。有效杠杆和 ensure_candidates 保留原 HL 行为。 |
| `src/exchange.rs` | TxFlow Client 字段、构造/测试入口、is_txflow、prices/maintenance/pnl/funding 方法和全部方法中的 TxFlow 分支；describe_txflow。保留 HL Exec 实现。 |
| `src/trader.rs` | Plan.txflow_targets、TxFlowMarginExceeded、build_txflow_plan/build_txflow_plan_at_scale、仅 TxFlow 使用的 Outcome::abort、最终账户核验及 TxFlow 专属错误/盘口分支。 |
| `src/store.rs` | 仅 TxFlow 调用的 txflow_cursor 方法；未操作任何数据库中的历史表。 |
| `static/` | 导航入口、独立页面、授权/积分脚本、共享 app.js 的 PnL 拉取/缓存/注入、审计渲染及场所条件、专属 CSS。HL 原先 tx_fills 恒为 0 的空展示分支随字段删除。 |
| Cargo | 删除仅用于专属实现的直接依赖 rmp-serde、alloy-sol-types、futures，以及只用于其虚拟时间测试的 tokio test-util；tower dev-dependency 为保留的 HL 路由测试继续存在。锁文件仅裁剪依赖，没有升级或新增包。 |
| 文档/前端测试 | 清理 README 和部署说明中的专属功能说明，保留共享收益口径；调整共享测试的代码提取边界及 venue 分支。 |

## 测试清单核对

基线 **104 个测试 = 103 passed + 1 ignored**；本次完全删除 **68 个 = 原 67 passed + 原 1 ignored**。其余 **36 个**全部通过，其中 **30 个名称保持不变，6 个改名/保留共享断言**。没有新增 ignored，也没有删除失败测试来规避验收。

### 改名或提取保留的 6 条

| 原测试 | 当前测试 | 保留方式 |
| --- | --- | --- |
| `live::txflow_fix_tests::txflow_reconciliation_preserves_concurrent_orders_and_deduplicates_fills` | `live::reconciliation_tests::reconciliation_preserves_concurrent_orders_and_deduplicates_fills` | 只改模块、测试名及被调用函数名；测试体其余字符不变。 |
| `live::txflow_fix_tests::txflow_nonempty_weights_still_require_factor_universe_and_two_legs` | `live::hl_signal_tests::nonempty_weights_do_not_require_extra_candidate_limits` | 保留 HL 非空权重可通过的断言；删除 TxFlow 宇宙/双腿准入断言。 |
| `live::txflow_fix_tests::txflow_signal_requires_binance_source_and_yesterdays_contiguous_bars` | `live::hl_signal_tests::signal_panel_keeps_the_hl_store_bars` | 保留 HL 自身 Store 中 24 币旧日线可读的断言；删除外部币安源缺失/过期检查。 |
| `live::txflow_fix_tests::binance_universe_differs_from_hl_and_markets_without_changing_weights` | `live::hl_signal_tests::signal_panel_uses_the_hl_universe` | 保留 HL_ONLY 宇宙断言；删除 TxFlow 币安宇宙、市场剔除及专属信号说明断言。HL 信号说明仍为原字面量。 |
| `live::txflow_fix_tests::txflow_fractional_leverage_uses_actual_integer_budget` | `live::hl_signal_tests::fractional_leverage_is_preserved` | 保留 HL 3.49 杠杆不取整的断言；删除 TxFlow 向下取整断言。 |
| `web::txflow_routes_tests::txflow_page_and_configuration_are_isolated_from_hyperliquid` | `web::live_routes_tests::live_page_and_configuration_preserve_save_and_execution_gates` | 保留首页可用、HL 无积分接口的覆盖；共享配置保存、失败回滚、run_gate 不阻止保存、exec_gate 阻止保存的断言改在 HL 路由执行；删除双场所隔离、积分与钱包授权检查。 |

### 完全删除的 68 条（逐项理由）

以下每一行对应基线测试列表的一条；不将仅有名称变化的测试算作删除。

| # | 删除测试 | 纯 TxFlow 的依据 |
| --- | --- | --- |
| 1 | `binance::refresh_regressions::embedded_mapping_preserves_verified_units_without_research_files` | 检查编译内嵌的 82 个 TxFlow 市场映射及 BONK 单位换算；映射和币安专属模块已删除。 |
| 2 | `binance::refresh_regressions::refresh_import_keeps_missing_symbols_but_updates_healthy_ones_and_rejects_foreign_db` | 检查 TxFlow 币安信号导入器的增量更新、映射约束及异库拒绝；HL 不调用该导入器。 |
| 3 | `binance::tests::import_command_rejects_hl_and_tx_database_aliases` | 检查 import-binance 入口对 HL/TxFlow 库路径别名的保护；该入口仅导入 TxFlow 市场 CSV，随模块删除，HL Store 本身不变。 |
| 4 | `binance::tests::import_is_idempotent_filters_mapping_preserves_units_and_skips_short_missing` | 检查 TxFlow 市场映射 CSV 的幂等导入、单位和短数据过滤；HL 仍走自己的刷新器。 |
| 5 | `live::capital_regressions::binance_rollover_and_gaps_are_rejected_and_open_bar_is_not_ranked` | 虽位于 capital_regressions，实际只调用 fresh_binance_panel；检查 TxFlow 币安源跨日、断档及未收盘过滤。 |
| 6 | `live::report5_state_tests::report5_corrupt_state_fails_closed` | 只检查 load_txflow 和 pending_recovery 的损坏状态拒绝；HL 的 LiveState::load 原体保留。 |
| 7 | `live::report5_state_tests::report5_required_fields_and_unknown_fields` | 只检查 load_txflow 专属必填配置校验与未知字段兼容；未修改 HL 反序列化测试。 |
| 8 | `live::txflow_fix_tests::txflow_atomic_save_preserves_previous_file_when_temp_write_fails` | 设置 config.txflow=true，检查专属临时文件/fsync/备份保存路径；HL 一直使用普通写入分支。 |
| 9 | `live::txflow_fix_tests::txflow_concentration_and_net_exposure_are_hard_limits` | 只检查 TxFlow 的单币集中度、双腿和净敞口硬限；原 HL 不执行这些准入检查。 |
| 10 | `trader::slice_plan_tests::aborted_execution_keeps_receipts_and_marks_unsent_orders` | 虽位于 slice_plan_tests，调用的 Outcome::abort 在生产中仅由 exec.is_txflow() 错误分支使用；函数及专属中止日志一同删除。HL 错误处理保留。 |
| 11 | `trader::txflow_portfolio_tests::txflow_actual_low_leverage_or_isolated_positions_fail_admission` | 调用 build_txflow_plan，检查实际低杠杆/逐仓的专属准入拒绝。 |
| 12 | `trader::txflow_portfolio_tests::txflow_balanced_portfolio_without_resting_orders_is_admitted` | 调用 build_txflow_plan，检查无挂单均衡组合准入；HL build_plan 不变。 |
| 13 | `trader::txflow_portfolio_tests::txflow_budget_resize_preserves_off_slot_positions_and_order_floor` | 检查 build_txflow_plan 预算缩减中保留非当日仓位及下单门槛的行为。 |
| 14 | `trader::txflow_portfolio_tests::txflow_healthy_plan_does_not_change_targets_or_generate_churn` | 检查 build_txflow_plan 的预算内无额外调仓；不针对 HL build_plan。 |
| 15 | `trader::txflow_portfolio_tests::txflow_invalid_weight_and_missing_target_price_fail_closed` | 检查 build_txflow_plan 对非法权重和缺价的专属拒绝。 |
| 16 | `trader::txflow_portfolio_tests::txflow_margin_failure_reports_amounts_without_ignoring_resting_orders` | 检查 TxFlowMarginExceeded 专属报错及挂单预留金额。 |
| 17 | `trader::txflow_portfolio_tests::txflow_margin_gate_rejects_small_balanced_resting_fills` | 检查 build_txflow_plan 的挂单成交保证金上限。 |
| 18 | `trader::txflow_portfolio_tests::txflow_possible_open_orders_cannot_exceed_final_margin` | 检查 build_txflow_plan 将可能成交挂单纳入最终保证金预算。 |
| 19 | `trader::txflow_portfolio_tests::txflow_reduce_only_fill_can_break_neutrality` | 检查 build_txflow_plan 对只减仓挂单造成不平衡的准入拒绝。 |
| 20 | `trader::txflow_portfolio_tests::txflow_retained_slot_position_cannot_exceed_risk_budget` | 检查 build_txflow_plan 对错峰保留仓的风险预算。 |
| 21 | `trader::txflow_portfolio_tests::txflow_small_retained_drift_is_resized_inside_the_same_margin_budget` | 检查 build_txflow_plan 的目标缩减；HL 无此重试缩减分支。 |
| 22 | `txflow::binance_schedule_regressions::utc_rollover_does_not_wait_six_hours_and_failures_back_off` | 检查 TxFlow 后台币安信号刷新跨日调度及失败退避；HL 定时刷新任务不变。 |
| 23 | `txflow::fix_regressions::account_lock_child` | TxFlow 跨进程账户锁测试的子进程辅助测试，锁实现随 Client 删除。 |
| 24 | `txflow::fix_regressions::account_lock_excludes_a_second_process_and_releases_on_drop` | 检查 TxFlow Client 的账户文件锁；不是保留的 HL exec_gate/run_gate。 |
| 25 | `txflow::fix_regressions::backfill_preserves_complete_metadata` | 检查 TxFlow refresh 回填与交易市场元数据隔离。 |
| 26 | `txflow::fix_regressions::equity_and_pnl_reader_select_txflow_without_signing_key` | 检查 TxFlow reader 和 PnL Client 选择；HL reader 分支保留。 |
| 27 | `txflow::fix_regressions::execution_partial_fill_requires_final_account_verification` | 使用模拟 TxFlow Client 检查部分成交后的最终账户核验；该核验按要求不进入 HL。 |
| 28 | `txflow::fix_regressions::execution_residual_reverse_position_is_not_success` | 使用模拟 TxFlow Client 检查反向残仓触发最终核验失败及中止。 |
| 29 | `txflow::fix_regressions::execution_second_leg_429_requires_final_account_verification` | 使用模拟 TxFlow Client 检查第二腿限流后的专属中止/账户核验。 |
| 30 | `txflow::fix_regressions::execution_timeout_already_filled_is_confirmed_from_account` | 使用模拟 TxFlow Client 检查回执超时但已成交的最终账户确认。 |
| 31 | `txflow::fix_regressions::incomplete_metadata_is_rejected_atomically` | 检查 TxFlow 元数据不完整时原子拒绝更新；HL 元数据读取不变。 |
| 32 | `txflow::fix_regressions::partial_books_preserve_successful_prices` | 检查 TxFlow 逐市场盘口部分读取失败时保留成功报价；HL 用批量 mids。 |
| 33 | `txflow::fix_regressions::pending_recovery_takes_priority_over_today_success_timestamp` | 检查 TxFlow pending_recovery 自动调度优先级；HL 无恢复标记分支。 |
| 34 | `txflow::fix_regressions::pepe_order_wire_preserves_exchange_lot_and_price_precision` | 检查 TxFlow PEPE 协议订单的 lot 和价格编码精度。 |
| 35 | `txflow::fix_regressions::persistent_rotation_covers_resonant_universe_and_survives_restart` | 检查 TxFlow 日线轮转游标与专属 SQLite 游标表。 |
| 36 | `txflow::fix_regressions::pnl_cache_is_account_scoped` | 检查 TxFlow 专属 PnL 缓存的账户隔离。 |
| 37 | `txflow::fix_regressions::pnl_ledger_failure_recovery_and_venue_interval_isolation` | 检查 TxFlow ledger/PnL 读取失败恢复及区间、场所隔离。 |
| 38 | `txflow::fix_regressions::pnl_read_errors_are_not_zero_profit` | 检查 TxFlow 专属 PnL 接口错误不会伪装为零收益。 |
| 39 | `txflow::fix_regressions::report5_balanced_1000x_must_abort_and_recover` | 检查 TxFlow 最终账户总敞口暴增后的恢复；HL 没有这条最终核验路径。 |
| 40 | `txflow::fix_regressions::report5_realistic_rounding_is_accepted` | 检查 TxFlow 最终账户核验对合理舍入误差的接受。 |
| 41 | `txflow::fix_regressions::report5_unplanned_balanced_gross_must_abort` | 检查 TxFlow 最终核验拒绝计划外均衡总敞口。 |
| 42 | `txflow::fix_regressions::scheduler_info_submit_approve_share_spacing` | 检查 TxFlow info/submit/approve 共用限速队列。 |
| 43 | `txflow::fix_regressions::scheduler_reads_retry_429_only_once` | 检查 TxFlow scheduler 的 429 只读重试次数；HL info_post 重试不变。 |
| 44 | `txflow::public_tests::mainnet_public_reads` | 唯一原 ignored 测试，读取 TxFlow 主网公开接口；对应 Client 已删除。 |
| 45 | `txflow::tests::concurrent_clients_share_submission_spacing` | 检查 TxFlow 多 Client 的共享交易提交间隔。 |
| 46 | `txflow::tests::every_submission_waits_300ms_even_after_failure` | 检查 TxFlow 交易提交成功/失败后均等待 300ms。 |
| 47 | `txflow::tests::pepe_integer_lots_and_nine_decimal_prices_survive_encoding` | 检查 TxFlow 专属 MessagePack 订单数量和价格编码。 |
| 48 | `txflow::tests::signatures_match_browser_wire_fixtures` | 检查 TxFlow EIP-712/MessagePack 签名与浏览器协议 fixture 一致。 |
| 49 | `txflow::tests::signing_nonce_and_order_key_order_are_binding` | 检查 TxFlow nonce 和有序 MessagePack key 顺序参与签名。 |
| 50 | `txflow::tests::tick_rounding_respects_slippage_and_decimal_normalization` | 检查 TxFlow tick_price 小数规范化与滑点限价；HL order_price 保留。 |
| 51 | `txflow_agent::tests::approval_receipt_persists_configuration_and_does_not_resubmit` | 检查 TxFlow Agent 授权回执保存/重试不重复授权。 |
| 52 | `txflow_agent::tests::authorization_rejects_cross_origin` | 检查已删除的 TxFlow Agent 授权接口同源校验。 |
| 53 | `txflow_agent::tests::server_key_is_private_and_never_in_public_challenge` | 检查 TxFlow 服务器生成 Agent 密钥权限和公开 challenge 内容。 |
| 54 | `txflow_agent::tests::signature_binds_account_agent_nonce_and_wallet_chain` | 检查 TxFlow ApproveAgent 的账户、Agent、nonce、chain 签名绑定。 |
| 55 | `txflow_agent::tests::unknown_or_expired_approval_cannot_be_replayed` | 检查 TxFlow Agent 授权待办有效期和防重放。 |
| 56 | `txflow_audit::tests::archive_survives_reload_and_rejects_corruption` | 检查 TxFlow 专属审计 archive 的持久化与损坏拒绝；HL LiveState 不用此文件。 |
| 57 | `txflow_audit::tests::cashflow_conflicts_and_unknown_ledger_adjustments_are_visible` | 检查 TxFlow ledger 审计冲突与未知流水；HL 手工 capital_flows 逻辑和测试保留。 |
| 58 | `txflow_audit::tests::exact_order_evidence_survives_clearing_records_and_never_crosses_accounts` | 检查 TxFlow 审计订单证据归属和账户隔离；不是共享 records 对账合并。 |
| 59 | `txflow_audit::tests::missing_funding_never_means_verified_zero` | 检查 TxFlow 独立审计 archive 的缺失资金费状态。 |
| 60 | `txflow_audit::tests::old_archives_wait_for_real_snapshots_and_missing_fields_stay_incomplete` | 检查 TxFlow 审计 archive 的旧版迁移与缺失快照状态。 |
| 61 | `txflow_audit::tests::points_weeks_use_utc_monday_and_do_not_estimate_points` | 检查 TxFlow 审计模块积分周边界及不估算积分。 |
| 62 | `txflow_audit::tests::reconciles_same_interval_without_order_associations` | 检查 TxFlow 独立审计的同区间净值/账本对账；共享 merge_reconciliation 已完整保留。 |
| 63 | `txflow_audit::tests::retains_same_timestamp_and_late_fills_without_duplicates` | 检查 TxFlow 审计 archive 成交收集去重；共享 HL reconcile_fill_rows 测试保留。 |
| 64 | `txflow_points::tests::correction_replaces_and_accounts_are_isolated` | 检查 TxFlow 积分文件的修正覆盖及账户隔离。 |
| 65 | `txflow_points::tests::monday_boundaries` | 检查 TxFlow 积分周的 UTC 周一边界。 |
| 66 | `web::repair_integration_tests::completed_downloads_resume_without_network_or_recovery_marker` | 只创建 TxFlow AppState 并调用 ensure_binance_panel，检查专属下载恢复。 |
| 67 | `web::repair_integration_tests::manual_signal_failure_does_not_mark_execution_started_or_read_a_signing_key` | 只创建 TxFlow AppState，检查币安信号失败前不写专属恢复标记、不读取密钥。 |
| 68 | `web::report5_unavailable_tests::report5_corrupt_txflow_routes_reject_every_operation` | 只检查 txflow_unavailable_router 的所有操作返回 503；HL 路由独立保留。 |

### JavaScript 测试处理

- 删除 `test-txflow-agent.mjs`：只测试已移除钱包授权 UI 的授权、拒绝、重试、账户/网络变更、无钱包流程。
- 删除 `test-txflow-points.mjs`：只测试已移除积分 API/UI 的加载、修正、失败及不发交易请求。
- `test-txflow-audit.mjs`：成交解析（科学计数法、部分成交、Success 未确认、错误回执）迁入 `test-receipts.mjs` 保留；删除只调用 renderTxAudit/renderTxReconciliation 的缺数据、账户切换、资金费缺失、区间差额断言。
- `test-account-performance.mjs`：保留全部 HL 账户收益、资金流、持仓盈亏、成交标签、曲线窗口断言；删除只测试 TxFlow PnL 路由重写/60 秒缓存/账户隔离/读取失败，以及填充审计 UI 的两组场所专属场景。
- `test-exec-daily.mjs`：仅将提取片段的结束标记改为仍存在的 renderMonitor；原断言未改。资金流、保存、策略曲线测试未改。

## 四项验收的实际输出

以下全部来自最终代码的实际执行，完整输出亦保存为同目录 `.log`；不是预期结果。

### 1. build（退出码 0）

```sh
cargo build --release --locked
```

```text
   Compiling stars v0.3.0 (/Users/wonder/Code/stars)
warning: function `account_meets_targets` is never used
   --> src/trader.rs:635:4
    |
635 | fn account_meets_targets(account: &Acct, plan: &Plan, cfg: &TradeConfig) -> bool {
    |    ^^^^^^^^^^^^^^^^^^^^^
    |
    = note: `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default

warning: `stars` (bin "stars") generated 1 warning
    Finished `release` profile [optimized] target(s) in 4.65s
```

### 2. test（退出码 0）

```sh
cargo test --release
```

```text
   Compiling stars v0.3.0 (/Users/wonder/Code/stars)
warning: unused variable: `cfg`
   --> src/trader.rs:911:13
    |
911 |         let cfg = TradeConfig {
    |             ^^^ help: if this is intentional, prefix it with an underscore: `_cfg`
    |
    = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default

warning: function `account_meets_targets` is never used
   --> src/trader.rs:635:4
    |
635 | fn account_meets_targets(account: &Acct, plan: &Plan, cfg: &TradeConfig) -> bool {
    |    ^^^^^^^^^^^^^^^^^^^^^
    |
    = note: `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default

warning: `stars` (bin "stars" test) generated 2 warnings
    Finished `release` profile [optimized] target(s) in 3.61s
     Running unittests src/main.rs (target/release/deps/stars-47ae36db0d915975)

running 36 tests
test hl::tests::coin_meta_reads_sz_decimals ... ok
test hl::tests::coin_meta_reads_is_delisted ... ok
test live::capital_flow_tests::deposit_does_not_change_trading_pnl ... ok
test live::capital_flow_tests::cash_flows_follow_the_account_baseline ... ok
test live::config_parity_tests::every_live_field_reaches_trade_config ... ok
test live::capital_regressions::saved_baseline_wins_over_trimmed_history_and_cannot_cross_accounts ... ok
test live::config_robustness::empty_object_still_parses ... ok
test live::config_robustness::missing_fields_do_not_wipe_the_account ... ok
test live::hl_signal_tests::fractional_leverage_is_preserved ... ok
test live::hl_signal_tests::nonempty_weights_do_not_require_extra_candidate_limits ... ok
test live::fill_dedup_tests::fills_with_no_matching_order_are_still_imported ... ok
test live::fill_dedup_tests::split_fills_matching_our_aggregate_order_are_not_imported ... ok
test live::min_order_pct_tests::effective_threshold_scales_with_equity ... ok
test live::min_order_pct_tests::min_order_pct_reaches_trade_config ... ok
test live::monitor_receipt_tests::first_receipt_watermark_survives_merge_without_fills ... ok
test live::monitor_receipt_tests::split_close_receipts_are_not_assumed_to_be_take_profits ... ok
test live::reconciliation_tests::reconciliation_preserves_concurrent_orders_and_deduplicates_fills ... ok
test live::tests::take_profit_buys_below_market_for_shorts ... ok
test live::tests::take_profit_price_is_never_zero_or_negative ... ok
test live::tests::take_profit_sells_above_market_for_longs ... ok
test live::tests::take_profit_stays_on_the_correct_side_after_rounding ... ok
test trader::close_threshold_tests::small_out_of_target_position_with_pct_threshold_is_still_closed ... ok
test trader::close_threshold_tests::small_position_below_pct_threshold_must_still_close ... ok
test trader::slice_plan_tests::non_slot_positions_are_left_alone ... ok
test trader::slice_tests::slice_of_is_deterministic_and_spreads_coins ... ok
test live::capital_regressions::baseline_is_observed_not_inferred_and_rejects_old_account_and_failed_save ... ok
test web::capital_time_tests::capital_time_requires_a_known_baseline_and_past_time ... ok
test trader::tests::only_closed_bars_are_used ... ok
test trader::tests::k_never_exceeds_half_the_universe ... ok
test trader::tests::long_and_short_legs_are_balanced ... ok
test live::capital_regressions::legacy_baseline_migrates_before_hour_dedup_and_survives_clear_restart ... ok
test web::capital_time_tests::legacy_capital_flow_does_not_invent_an_effective_time ... ok
test trader::tests::weights_are_normalised_to_unit_gross ... ok
test live::hl_signal_tests::signal_panel_uses_the_hl_universe ... ok
test web::live_routes_tests::live_page_and_configuration_preserve_save_and_execution_gates ... ok
test live::hl_signal_tests::signal_panel_keeps_the_hl_store_bars ... ok

test result: ok. 36 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

```

### 3. residual（退出码 1）

```sh
grep -rin "txflow\|TxFlow\|TXFLOW" src/ static/
```

```text
```

上方输出块为空：stdout/stderr 均为 0 字节；grep 退出码 1 表示没有匹配。

### 4. routes（退出码 0）

```sh
grep -n 'route("/api/live' src/web.rs
```

```text
67:        .route("/api/live", get(live_status))
68:        .route("/api/live/config", post(live_config))
69:        .route("/api/live/run", post(live_run))
70:        .route("/api/live/reset", post(live_reset))
71:        .route("/api/live/rebuild", post(live_rebuild))
72:        .route("/api/live/tp", post(live_tp))
73:        .route("/api/live/records/clear", post(live_records_clear))
74:        .route("/api/live/capital", post(live_capital))
75:        .route("/api/live/capital/time", post(live_capital_time))
```

## 前端检查实际输出

```text
$ node --check static/app.js
exit code: 0

$ node tools/lint-frontend.mjs
✓ 前端静态检查通过（196 个定义 · 41 个元素引用）
exit code: 0

$ node tools/test-account-performance.mjs
✓ Account changes use persisted equity; unavailable baselines stay unknown
✓ Current PNL, unavailable accounts, neutral fill labels, close direction, bounded chart windows and receipt deduplication
✓ Deposits/withdrawals adjust principal and simple return, nonpositive principal stays undefined
exit code: 0

$ node tools/test-capital-flow.mjs
✓ Capital flow save refreshes metrics; read failure does not invite duplicate entries; invalid/save failures retain correct state
✓ Time correction is bound to the original account/record and parses UTC explicitly
exit code: 0

$ node tools/test-exec-daily.mjs
✓ Daily execution handles UTC windows, paging, missing API, unknown slippage and invalid equity without inventing zero values
✓ Every bar has a visible precise amount and date, with scale and scroll access
exit code: 0

$ node tools/test-live-save.mjs
✓ Config saving displays the server error, preserves settings, and allows retry
exit code: 0

$ node tools/test-receipts.mjs
✓ Receipt-only Success is unverified; partial fills use submitted size; scientific prices parse correctly
exit code: 0

$ node tools/test-strategy-chart.mjs
✓ Strategy curve removes deposits and withdrawals, preserves losses, respects effective times and window boundaries, detects inconsistent ledgers
exit code: 0
```
