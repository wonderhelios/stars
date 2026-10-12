# TxFlow 移除检查报告（2026-10-12）

## 结论：发现 HL 依赖，按约束停止删除

**本次未完成 TxFlow 移除，不能作为删除验收通过或部署依据。** 在修改源代码之前，发现任务要求删除的 `merge_txflow_reconciliation` 已被 HL 实盘状态路径直接使用。遵守「如果发现某处删除会改变 HL 行为，停下来在报告里说明，不要猜」的约束，停止删除，仅完成当前代码的基线检查并记录证据。

检查起点：`489359e0ba18aa2837acea807fb12c0edce76db3`。开始时 `git status --short` 为空。没有修改 `src/`、`static/`、Cargo.toml 或 Cargo.lock；没有删除文件或测试；没有 commit、push、部署、重启服务。新增内容仅为本报告。

## 阻断证据与影响

1. `src/web.rs:80` 将 HL 路由 `/api/live` 注册到 `live_status`。
2. `src/web.rs:393` 在 `live_status` 中直接调用 `crate::live::merge_txflow_reconciliation(&mut current, &st)`。这里没有 `config.txflow` 条件；配置 HL 账户且超过对账间隔时同样会走到此处。
3. `src/live.rs:313–323` 的函数实现先核对账户，再按 `tid` 合并、去重成交记录，同时以 `max` 更新 `reconciled_to` 和 `last_reconcile_ms`。直接删除函数会编译失败；连调用一起删掉则会丢失 HL 对账结果的合并和进度更新，并非等价删除。
4. `src/live.rs:1975` 的现有测试 `live::monitor_receipt_tests::first_receipt_watermark_survives_merge_without_fills` 调用同一函数，验证无新成交时首次对账进度仍能保存。此测试属于共享/HL 功能，不能当成 TxFlow 专属测试删除。
5. `src/live.rs:1771` 的 `txflow_reconciliation_preserves_concurrent_orders_and_deduplicates_fills` 尽管名字带 TxFlow，实际检查的也是上述共享合并逻辑，应保留覆盖并移除名称中的交易所限定。

函数原文：

```rust
pub fn merge_txflow_reconciliation(current: &mut LiveState, snapshot: &LiveState) {
    if !current.config.account.eq_ignore_ascii_case(&snapshot.config.account) { return; }
    let mut known:std::collections::HashSet<u64>=current.records.iter().filter_map(|r|r.tid).collect();
    for record in &snapshot.records {
        if let Some(tid)=record.tid {
            if known.insert(tid) {current.records.push(record.clone());}
        }
    }
    current.reconciled_to=current.reconciled_to.max(snapshot.reconciled_to);
    current.last_reconcile_ms=current.last_reconcile_ms.max(snapshot.last_reconcile_ms);
}
```

可行的继续方案：**保留完整函数体，将函数改名为 `merge_reconciliation`，同步修改 HL 调用和相关测试名称**，然后继续移除真正的 TxFlow 专属实现。这样可以消除 `src/ static/` 中的 TxFlow 名称并保留已有 HL 行为。此方案尚未实施；这里明确记录了原删除清单与 HL 保留要求的冲突，没有自行推断可删除其行为。

## 已检查的其他边界

- `main.rs:68–69` 的 HL AppState 确实将 `binance_store` 和 `hl_store` 设为 `None`。`web.rs:21` 的 `signal_store()` 返回 `binance_store`；`live.rs:754–759` 仅在 TxFlow 模式使用该外部信号库，HL 分支直接调用 `trader::load_panel(store)`。移除这些字段时须同步调整调用参数，不改变 HL 使用自身 Store 的行为。
- `run_gate` 也被 HL 的 `live_run` 使用（`web.rs:536`），不能因为它与 TxFlow 数据准备一起出现就顺手删除。`exec_gate` 同样必须保留。
- `account_meets_targets` 当前的执行调用位于 TxFlow 专属最终核验段，但任务明确要求保留该函数。后续应保留函数和按 `plan.orders` 建立目标的逻辑，不把最终核验新增到原 HL 执行路径中。
- HL 路由实际有 9 条，包括清单之外已有的 `/api/live/capital/time`。全部原样保留。
- `research/` 中存在旧研究代码快照、回放 harness，`tools/` 中存在 TxFlow 脚本，另有映射配置与审计文档。本次因上述冲突停止，未清理这些历史材料，也未宣称整个仓库已不存在 TxFlow。

## 删除与保留清单

- 删除的文件、函数、字段、测试：**无**。
- 改写或重命名的测试：**无**。
- HL 所有代码与原有测试原样保留，包括 `LiveState`、`LiveConfig`、`build_plan`、`execute`、`execute_inner`、`account_meets_targets`、HL Exec 和全部路由，以及 `min_order_pct`、`net_deposit`、`capital_flows`、`exec_daily`、`start_equity`、`capital_baseline`。
- 没有为通过编译保留空分支、屏蔽测试、增加 allow 属性或修改业务逻辑。

## 实际命令输出

以下输出来自**未经删除的原始代码**，只说明当前基线状态，不构成删除后的验收。所有输出均完整保留，没有以预期结果替代实际输出。

### 1. 构建（退出码 0）

```sh
cargo build --release --locked
```

```text
   Compiling stars v0.3.0 (/Users/wonder/Code/stars)
    Finished `release` profile [optimized] target(s) in 7.80s
```

### 2. 测试（退出码 0）

```sh
cargo test --release
```

```text
warning: unused variable: `cfg`
    --> src/trader.rs:1142:13
     |
1142 |         let cfg = TradeConfig {
     |             ^^^ help: if this is intentional, prefix it with an underscore: `_cfg`
     |
     = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default

warning: `stars` (bin "stars" test) generated 1 warning
    Finished `release` profile [optimized] target(s) in 0.58s
     Running unittests src/main.rs (target/release/deps/stars-7a9da671a4b16392)

running 104 tests
test live::config_parity_tests::every_live_field_reaches_trade_config ... ok
test live::capital_flow_tests::deposit_does_not_change_trading_pnl ... ok
test live::capital_flow_tests::cash_flows_follow_the_account_baseline ... ok
test live::capital_regressions::saved_baseline_wins_over_trimmed_history_and_cannot_cross_accounts ... ok
test hl::tests::coin_meta_reads_is_delisted ... ok
test live::config_robustness::empty_object_still_parses ... ok
test hl::tests::coin_meta_reads_sz_decimals ... ok
test live::config_robustness::missing_fields_do_not_wipe_the_account ... ok
test live::min_order_pct_tests::effective_threshold_scales_with_equity ... ok
test live::min_order_pct_tests::min_order_pct_reaches_trade_config ... ok
test live::tests::take_profit_buys_below_market_for_shorts ... ok
test live::tests::take_profit_price_is_never_zero_or_negative ... ok
test live::monitor_receipt_tests::first_receipt_watermark_survives_merge_without_fills ... ok
test binance::refresh_regressions::embedded_mapping_preserves_verified_units_without_research_files ... ok
test live::fill_dedup_tests::split_fills_matching_our_aggregate_order_are_not_imported ... ok
test live::fill_dedup_tests::fills_with_no_matching_order_are_still_imported ... ok
test live::monitor_receipt_tests::split_close_receipts_are_not_assumed_to_be_take_profits ... ok
test live::tests::take_profit_sells_above_market_for_longs ... ok
test live::tests::take_profit_stays_on_the_correct_side_after_rounding ... ok
test live::txflow_fix_tests::txflow_concentration_and_net_exposure_are_hard_limits ... ok
test binance::tests::import_command_rejects_hl_and_tx_database_aliases ... ok
test live::txflow_fix_tests::txflow_fractional_leverage_uses_actual_integer_budget ... ok
test live::txflow_fix_tests::txflow_nonempty_weights_still_require_factor_universe_and_two_legs ... ok
test live::txflow_fix_tests::txflow_reconciliation_preserves_concurrent_orders_and_deduplicates_fills ... ok
test trader::close_threshold_tests::small_position_below_pct_threshold_must_still_close ... ok
test trader::slice_plan_tests::non_slot_positions_are_left_alone ... ok
test trader::slice_plan_tests::aborted_execution_keeps_receipts_and_marks_unsent_orders ... ok
test trader::close_threshold_tests::small_out_of_target_position_with_pct_threshold_is_still_closed ... ok
test trader::slice_tests::slice_of_is_deterministic_and_spreads_coins ... ok
test trader::tests::k_never_exceeds_half_the_universe ... ok
test trader::tests::long_and_short_legs_are_balanced ... ok
test trader::tests::only_closed_bars_are_used ... ok
test live::report5_state_tests::report5_required_fields_and_unknown_fields ... ok
test trader::tests::weights_are_normalised_to_unit_gross ... ok
test trader::txflow_portfolio_tests::txflow_actual_low_leverage_or_isolated_positions_fail_admission ... ok
test trader::txflow_portfolio_tests::txflow_balanced_portfolio_without_resting_orders_is_admitted ... ok
test trader::txflow_portfolio_tests::txflow_budget_resize_preserves_off_slot_positions_and_order_floor ... ok
test trader::txflow_portfolio_tests::txflow_healthy_plan_does_not_change_targets_or_generate_churn ... ok
test trader::txflow_portfolio_tests::txflow_invalid_weight_and_missing_target_price_fail_closed ... ok
test trader::txflow_portfolio_tests::txflow_margin_failure_reports_amounts_without_ignoring_resting_orders ... ok
test trader::txflow_portfolio_tests::txflow_margin_gate_rejects_small_balanced_resting_fills ... ok
test trader::txflow_portfolio_tests::txflow_possible_open_orders_cannot_exceed_final_margin ... ok
test trader::txflow_portfolio_tests::txflow_reduce_only_fill_can_break_neutrality ... ok
test trader::txflow_portfolio_tests::txflow_retained_slot_position_cannot_exceed_risk_budget ... ok
test live::report5_state_tests::report5_corrupt_state_fails_closed ... ok
test trader::txflow_portfolio_tests::txflow_small_retained_drift_is_resized_inside_the_same_margin_budget ... ok
test live::capital_regressions::baseline_is_observed_not_inferred_and_rejects_old_account_and_failed_save ... ok
test txflow::binance_schedule_regressions::utc_rollover_does_not_wait_six_hours_and_failures_back_off ... ok
test live::capital_regressions::legacy_baseline_migrates_before_hour_dedup_and_survives_clear_restart ... ok
test live::txflow_fix_tests::txflow_atomic_save_preserves_previous_file_when_temp_write_fails ... ok
test txflow::fix_regressions::account_lock_child ... ok
test binance::tests::import_is_idempotent_filters_mapping_preserves_units_and_skips_short_missing ... ok
test txflow::fix_regressions::pending_recovery_takes_priority_over_today_success_timestamp ... ok
test binance::refresh_regressions::refresh_import_keeps_missing_symbols_but_updates_healthy_ones_and_rejects_foreign_db ... ok
test txflow::fix_regressions::account_lock_excludes_a_second_process_and_releases_on_drop ... ok
test live::capital_regressions::binance_rollover_and_gaps_are_rejected_and_open_bar_is_not_ranked ... ok
test live::txflow_fix_tests::txflow_signal_requires_binance_source_and_yesterdays_contiguous_bars ... ok
test live::txflow_fix_tests::binance_universe_differs_from_hl_and_markets_without_changing_weights ... ok
test txflow::fix_regressions::equity_and_pnl_reader_select_txflow_without_signing_key ... ok
test txflow::fix_regressions::persistent_rotation_covers_resonant_universe_and_survives_restart ... ok
test txflow::fix_regressions::execution_partial_fill_requires_final_account_verification ... ok
test txflow::fix_regressions::execution_second_leg_429_requires_final_account_verification ... ok
test txflow::fix_regressions::backfill_preserves_complete_metadata ... ok
test txflow::public_tests::mainnet_public_reads ... ignored, read-only mainnet integration; requires network
test txflow::tests::concurrent_clients_share_submission_spacing ... ok
test txflow::tests::every_submission_waits_300ms_even_after_failure ... ok
test txflow::tests::pepe_integer_lots_and_nine_decimal_prices_survive_encoding ... ok
test txflow::tests::signatures_match_browser_wire_fixtures ... ok
test txflow::tests::signing_nonce_and_order_key_order_are_binding ... ok
test txflow::tests::tick_rounding_respects_slippage_and_decimal_normalization ... ok
test txflow_agent::tests::approval_receipt_persists_configuration_and_does_not_resubmit ... ok
test txflow_agent::tests::authorization_rejects_cross_origin ... ok
test txflow_agent::tests::server_key_is_private_and_never_in_public_challenge ... ok
test txflow_agent::tests::signature_binds_account_agent_nonce_and_wallet_chain ... ok
test txflow_agent::tests::unknown_or_expired_approval_cannot_be_replayed ... ok
test txflow_audit::tests::archive_survives_reload_and_rejects_corruption ... ok
test txflow_audit::tests::cashflow_conflicts_and_unknown_ledger_adjustments_are_visible ... ok
test txflow_audit::tests::exact_order_evidence_survives_clearing_records_and_never_crosses_accounts ... ok
test txflow_audit::tests::missing_funding_never_means_verified_zero ... ok
test txflow_audit::tests::old_archives_wait_for_real_snapshots_and_missing_fields_stay_incomplete ... ok
test txflow_audit::tests::points_weeks_use_utc_monday_and_do_not_estimate_points ... ok
test txflow_audit::tests::reconciles_same_interval_without_order_associations ... ok
test txflow_audit::tests::retains_same_timestamp_and_late_fills_without_duplicates ... ok
test txflow_points::tests::correction_replaces_and_accounts_are_isolated ... ok
test txflow_points::tests::monday_boundaries ... ok
test web::capital_time_tests::capital_time_requires_a_known_baseline_and_past_time ... ok
test web::capital_time_tests::legacy_capital_flow_does_not_invent_an_effective_time ... ok
test web::repair_integration_tests::completed_downloads_resume_without_network_or_recovery_marker ... ok
test web::repair_integration_tests::manual_signal_failure_does_not_mark_execution_started_or_read_a_signing_key ... ok
test web::report5_unavailable_tests::report5_corrupt_txflow_routes_reject_every_operation ... ok
test web::txflow_routes_tests::txflow_page_and_configuration_are_isolated_from_hyperliquid ... ok
test txflow::fix_regressions::execution_residual_reverse_position_is_not_success has been running for over 60 seconds
test txflow::fix_regressions::execution_timeout_already_filled_is_confirmed_from_account has been running for over 60 seconds
test txflow::fix_regressions::incomplete_metadata_is_rejected_atomically has been running for over 60 seconds
test txflow::fix_regressions::partial_books_preserve_successful_prices has been running for over 60 seconds
test txflow::fix_regressions::pepe_order_wire_preserves_exchange_lot_and_price_precision has been running for over 60 seconds
test txflow::fix_regressions::pnl_cache_is_account_scoped has been running for over 60 seconds
test txflow::fix_regressions::pnl_ledger_failure_recovery_and_venue_interval_isolation has been running for over 60 seconds
test txflow::fix_regressions::pnl_read_errors_are_not_zero_profit has been running for over 60 seconds
test txflow::fix_regressions::report5_balanced_1000x_must_abort_and_recover has been running for over 60 seconds
test txflow::fix_regressions::report5_realistic_rounding_is_accepted has been running for over 60 seconds
test txflow::fix_regressions::report5_unplanned_balanced_gross_must_abort has been running for over 60 seconds
test txflow::fix_regressions::execution_residual_reverse_position_is_not_success ... ok
test txflow::fix_regressions::execution_timeout_already_filled_is_confirmed_from_account ... ok
test txflow::fix_regressions::incomplete_metadata_is_rejected_atomically ... ok
test txflow::fix_regressions::partial_books_preserve_successful_prices ... ok
test txflow::fix_regressions::pepe_order_wire_preserves_exchange_lot_and_price_precision ... ok
test txflow::fix_regressions::pnl_cache_is_account_scoped ... ok
test txflow::fix_regressions::pnl_ledger_failure_recovery_and_venue_interval_isolation ... ok
test txflow::fix_regressions::scheduler_info_submit_approve_share_spacing has been running for over 60 seconds
test txflow::fix_regressions::pnl_read_errors_are_not_zero_profit ... ok
test txflow::fix_regressions::report5_balanced_1000x_must_abort_and_recover ... ok
test txflow::fix_regressions::report5_realistic_rounding_is_accepted ... ok
test txflow::fix_regressions::report5_unplanned_balanced_gross_must_abort ... ok
test txflow::fix_regressions::scheduler_info_submit_approve_share_spacing ... ok
test txflow::fix_regressions::scheduler_reads_retry_429_only_once has been running for over 60 seconds
test txflow::fix_regressions::scheduler_reads_retry_429_only_once ... ok

test result: ok. 103 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 113.96s

```

### 3. TxFlow 残留检查（退出码 0；有匹配，移除验收不通过）

```sh
grep -rn "txflow\|TxFlow\|TXFLOW" src/ static/
```

```text
src//exchange.rs:80:    txflow: Option<crate::txflow::Client>,
src//exchange.rs:88:    pub fn test_txflow(client: crate::txflow::Client) -> Self {
src//exchange.rs:89:        Self { txflow:Some(client), http:reqwest::Client::new(), trading:None, account:None }
src//exchange.rs:91:    pub fn is_txflow(&self) -> bool { self.txflow.is_some() }
src//exchange.rs:92:    pub async fn txflow_prices(&self, coins: &[String]) -> Option<Result<HashMap<String, f64>>> {
src//exchange.rs:93:        if let Some(tx) = &self.txflow { Some(tx.prices(coins).await) } else { None }
src//exchange.rs:95:    pub async fn txflow_maintenance_margin(&self) -> Option<Result<f64>> {
src//exchange.rs:96:        if let Some(tx) = &self.txflow { Some(tx.maintenance_margin().await) } else { None }
src//exchange.rs:99:        if cfg.txflow { Self::txflow(&cfg.account, Some(Path::new(&cfg.key_path))).await }
src//exchange.rs:102:    pub async fn txflow(account: &str, key: Option<&Path>) -> Result<Self> {
src//exchange.rs:103:        let client = crate::txflow::Client::new(account, key).await?;
src//exchange.rs:104:        Ok(Self { txflow: Some(client), http: reqwest::Client::new(), trading: None, account: None })
src//exchange.rs:153:            txflow: None,
src//exchange.rs:161:        if cfg.txflow {Self::txflow(&cfg.account,None).await}
src//exchange.rs:174:            txflow: None,
src//exchange.rs:203:            txflow: None,
src//exchange.rs:238:        if let Some(tx) = &self.txflow { return tx.all_mids().await; }
src//exchange.rs:258:        if let Some(tx) = &self.txflow { return tx.account().await; }
src//exchange.rs:353:        if let Some(tx) = &self.txflow { return tx.set_leverage(coin, leverage).await; }
src//exchange.rs:374:        if let Some(tx) = &self.txflow { return tx.resting_reduce_order(coin, buy, size, px).await; }
src//exchange.rs:400:    /// TxFlow 的真实业绩汇总（已实现/手续费/净入金）。非 TxFlow 时返回 None。
src//exchange.rs:401:    pub async fn txflow_pnl(&self, since_ms: i64) -> Option<anyhow::Result<crate::txflow::PnlSummary>> {
src//exchange.rs:402:        match &self.txflow {
src//exchange.rs:408:    pub async fn txflow_account_funding(&self, start: i64, end: i64) -> anyhow::Result<Vec<serde_json::Value>> {
src//exchange.rs:409:        self.txflow.as_ref().context("非 TxFlow 账户")?.account_funding(start, end).await
src//exchange.rs:415:        if let Some(tx) = &self.txflow { return tx.user_fills(limit).await; }
src//exchange.rs:431:        if let Some(tx) = &self.txflow { return tx.open_orders().await; }
src//exchange.rs:461:        if let Some(tx) = &self.txflow { return tx.cancel_orders(orders).await; }
src//exchange.rs:533:        if let Some(tx) = &self.txflow { return tx.ioc(coin, buy, reduce_only, size, mid, slippage).await; }
src//exchange.rs:658:/// TxFlow-only evidence marker: acceptance without a fill stays unverified.
src//exchange.rs:659:pub fn describe_txflow(status: &ExchangeDataStatus, requested: f64) -> String {
src//txflow_points.rs:39:    anyhow::ensure!(!input.account.is_empty(), "请先配置 TxFlow 账户");
src//txflow_points.rs:55:        anyhow::ensure!(st.config.txflow, "非 TxFlow 账户");
src//txflow_points.rs:58:        let activity = crate::txflow_audit::weekly_activity(&state.live_path, &st.config.account)?;
src//txflow_points.rs:67:    if !st.config.txflow || !st.config.account.eq_ignore_ascii_case(&input.account) {
src//store.rs:42:    /// TxFlow-only scheduler state. SQLite serializes increments across processes.
src//store.rs:43:    pub fn txflow_cursor(&self, total: usize, advance: usize) -> Result<usize> {
src//store.rs:44:        anyhow::ensure!(total>0,"TxFlow cursor universe is empty");
src//store.rs:46:        conn.execute_batch("CREATE TABLE IF NOT EXISTS txflow_refresh_cursor (id INTEGER PRIMARY KEY CHECK(id=1), value INTEGER NOT NULL); INSERT OR IGNORE INTO txflow_refresh_cursor VALUES(1,0);")?;
src//store.rs:48:        let cursor:usize=tx.query_row("SELECT value FROM txflow_refresh_cursor WHERE id=1",[],|r|r.get::<_,u64>(0))? as usize % total;
src//store.rs:49:        if advance>0 { tx.execute("UPDATE txflow_refresh_cursor SET value=? WHERE id=1",[((cursor+advance)%total) as u64])?; }
src//txflow_audit.rs:297:            if let Some(p) = exec.txflow_pnl(since).await {
src//txflow_audit.rs:299:                let funding = match tokio::time::timeout(std::time::Duration::from_secs(10),exec.txflow_account_funding(since,point.ts)).await {
src//txflow_audit.rs:312:            other => tracing::warn!("TxFlow 成交审计抓取未完成: {other:?}"),
src//txflow_audit.rs:331:        let w = weeks.entry(crate::txflow_points::epoch_start(ts)).or_default();
src//web.rs:27:    /// Dedicated Binance daily signal cache; never refreshed with TxFlow candles.
src//web.rs:50:    /// TxFlow daily availability is separate from complete trading metadata.
src//web.rs:94:/// The same monitor/config handlers backed by an isolated TxFlow AppState.
src//web.rs:97:async fn txflow_pnl(State(state): State<AppState>) -> Response {
src//web.rs:99:    if !st.config.txflow {
src//web.rs:100:        return Json(json!({"ok": false, "error": "非 TxFlow 账户"})).into_response();
src//web.rs:108:    match exec.txflow_pnl(since).await {
src//web.rs:114:            let audit = match crate::txflow_audit::update(&state.live_path, &st.config.account, since,
src//web.rs:132:        None => Json(json!({"ok": false, "error": "无 TxFlow 客户端"})).into_response(),
src//web.rs:137:pub fn txflow_unavailable_router(error: String) -> Router {
src//web.rs:143:        .route("/txflow", axum::routing::any(handler.clone()))
src//web.rs:144:        .route("/txflow/*path", axum::routing::any(handler.clone()))
src//web.rs:145:        .route("/api/txflow/*path", axum::routing::any(handler))
src//web.rs:148:pub fn txflow_router(state: AppState) -> Router {
src//web.rs:150:        .route("/txflow", get(txflow_html))
src//web.rs:151:        .route("/txflow/app.js", get(txflow_js))
src//web.rs:152:        .route("/txflow/agent.js", get(crate::txflow_agent::script))
src//web.rs:153:        .route("/txflow/points.js", get(|| async { ([(header::CONTENT_TYPE,"text/javascript; charset=utf-8")], include_str!("../static/txflow-points.js")) }))
src//web.rs:154:        .route("/api/txflow/points", get(crate::txflow_points::get).post(crate::txflow_points::save))
src//web.rs:155:        .route("/api/txflow/agent/prepare", post(crate::txflow_agent::prepare))
src//web.rs:156:        .route("/api/txflow/agent/approve", post(crate::txflow_agent::approve))
src//web.rs:157:        .route("/api/txflow/status", get(status))
src//web.rs:158:        .route("/api/txflow/live", get(live_status))
src//web.rs:159:        .route("/api/txflow/pnl", get(txflow_pnl))
src//web.rs:160:        .route("/api/txflow/live/config", post(live_config))
src//web.rs:161:        .route("/api/txflow/live/run", post(live_run))
src//web.rs:162:        .route("/api/txflow/live/reset", post(live_reset))
src//web.rs:163:        .route("/api/txflow/live/rebuild", post(live_rebuild))
src//web.rs:164:        .route("/api/txflow/live/tp", post(txflow_tp))
src//web.rs:165:        .route("/api/txflow/live/records/clear", post(live_records_clear))
src//web.rs:166:        .route("/api/txflow/live/capital", post(live_capital))
src//web.rs:167:        .route("/api/txflow/live/capital/time", post(live_capital_time))
src//web.rs:171:async fn txflow_html() -> impl IntoResponse {
src//web.rs:172:    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], include_str!("../static/txflow.html"))
src//web.rs:174:async fn txflow_js() -> impl IntoResponse {
src//web.rs:175:    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], APP_JS.replace("/api/", "/api/txflow/")
src//web.rs:176:        .replace("const LIVE_EXCHANGE = \"Hyperliquid\";", "const LIVE_EXCHANGE = \"TxFlow\";"))
src//web.rs:201:    if state.live.lock().await.config.txflow {
src//web.rs:203:        return Json(json!({"exchange":"txflow","universe_total":meta.universe.len(),
src//web.rs:393:                        crate::live::merge_txflow_reconciliation(&mut current, &st);
src//web.rs:477:        c.leverage = if c.txflow { v.floor().clamp(1.0, 20.0) } else { v.clamp(1.0, 20.0) };
src//web.rs:547:    let txflow = state.live.lock().await.config.txflow;
src//web.rs:548:    let execution = if !txflow {
src//web.rs:564:    guard.last_plan = vec![if txflow { "计划任务执行中：正在准备 TxFlow 行情…（期间可以保存配置）".into() } else { "调仓执行中…（页面会自动刷新结果）".into() }];
src//web.rs:566:        if st.config.txflow {
src//web.rs:578:            let markets = if st.config.txflow {
src//web.rs:579:                crate::txflow::ensure_binance_panel(&state).await?;
src//web.rs:580:                crate::txflow::refresh_meta_only(&state).await?;
src//web.rs:583:            let _tx_execution = if txflow && live {
src//web.rs:591:            if txflow {
src//web.rs:594:            if txflow && live {
src//web.rs:597:                current.begin_txflow_execution(&live_path)?;
src//web.rs:604:                if txflow && live {
src//web.rs:605:                    g.complete_txflow_execution(result.aborted, crate::live::now_ms_pub());
src//web.rs:606:                } else if live || !st.config.txflow { g.last_run_at = Some(crate::live::now_ms_pub()); }
src//web.rs:788:/// TxFlow's paced TP replacement runs independently of the browser connection.
src//web.rs:789:async fn txflow_tp(State(state): State<AppState>) -> Response {
src//web.rs:814:mod txflow_routes_tests {
src//web.rs:820:    async fn txflow_page_and_configuration_are_isolated_from_hyperliquid() {
src//web.rs:837:        tx_live.config.txflow = true;
src//web.rs:848:        let app = router(base.clone()).merge(txflow_router(tx.clone()));
src//web.rs:849:        for path in ["/", "/txflow", "/txflow/app.js", "/txflow/points.js", "/api/txflow/points", "/api/txflow/status", "/api/txflow/live"] {
src//web.rs:853:            if path == "/txflow" { assert!(body.contains("300ms") && body.contains("/txflow/app.js")); assert!(!body.contains("Sharpe <b>2.14")); }
src//web.rs:854:            if path == "/txflow/app.js" {
src//web.rs:855:                assert!(body.contains("/api/txflow/live/run"));
src//web.rs:856:                assert!(body.contains("fetch(\"/api/txflow/pnl\""));
src//web.rs:857:                assert!(!body.contains("/api/txflow/txflow/"));
src//web.rs:859:                assert!(body.contains("const LIVE_EXCHANGE = \"TxFlow\";"));
src//web.rs:864:        let request = Request::builder().method("POST").uri("/api/txflow/points")
src//web.rs:870:        let request = Request::builder().method("POST").uri("/api/txflow/points")
src//web.rs:876:        let request = Request::builder().method("POST").uri("/api/txflow/live/config")
src//web.rs:877:            .header("content-type", "application/json").body(Body::from(r#"{"lookback":21,"target_positions":4,"txflow":false}"#)).unwrap();
src//web.rs:881:        assert!(tx.live.lock().await.config.txflow);
src//web.rs:884:        assert!(!base.live.lock().await.config.txflow);
src//web.rs:894:        let request = Request::builder().method("POST").uri("/api/txflow/live/config")
src//web.rs:897:        let prepare_request = || Request::builder().method("POST").uri("/api/txflow/agent/prepare")
src//web.rs:905:        let request = Request::builder().method("POST").uri("/api/txflow/live/config")
src//web.rs:919:    async fn report5_corrupt_txflow_routes_reject_every_operation() {
src//web.rs:922:        let router=super::txflow_unavailable_router("状态文件损坏；拒绝实盘".into());
src//web.rs:923:        for (method,path) in [("GET","/txflow"),("GET","/api/txflow/live"),("POST","/api/txflow/live/config"),("POST","/api/txflow/live/run"),("POST","/api/txflow/agent/approve"),("POST","/api/txflow/live/reset")] {
src//web.rs:937:        live.config.txflow = true;
src//web.rs:961:            std::fs::write(daily.join(format!("{}.csv",entry["txflow_name"].as_str().unwrap())), &csv).unwrap();
src//web.rs:963:        tokio::time::timeout(std::time::Duration::from_secs(2), crate::txflow::ensure_binance_panel(&app)).await.unwrap().unwrap();
src//binance.rs:24:    anyhow::ensure!(resolved(a)? != resolved(b)?, "币安库不能与 HL/TxFlow 行情库相同");
src//binance.rs:36:    match result { Ok(s)=>Some(Arc::new(s)), Err(e)=>{ tracing::warn!("币安库不可用，TxFlow 拒绝计划，HL 不受影响: {e:#}"); None } }
src//binance.rs:50:        let name=entry["txflow_name"].as_str().context("missing txflow_name")?;
src//binance.rs:80:    let tx=std::env::var("STARS_TXFLOW_DB").map(PathBuf::from).unwrap_or_else(|_|hl.with_file_name("txflow-candles.sqlite"));
src//binance.rs:93:        for db in [hl.clone(),root.join("txflow-candles.sqlite")] {
src//binance.rs:108:        let mapping=root.join("mapping.json");std::fs::write(&mapping,r#"[{"txflow_name":"1000BONK-USDC"},{"txflow_name":"SHORT-USDC"},{"txflow_name":"MISSING-USDC"}]"#).unwrap();
src//binance.rs:164:        let tx = e.get("txflow_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
src//binance.rs:172:            report.skipped.push((tx, "映射缺 txflow 或 binance_symbol".into()));
src//binance.rs:266:        let names: std::collections::HashSet<_> = mapping.iter().map(|e| e["txflow_name"].as_str().unwrap()).collect();
src//binance.rs:268:        let bonk = mapping.iter().find(|e| e["txflow_name"] == "1000BONK-USDC").unwrap();
src//binance.rs:276:        let entries = serde_json::from_str::<Vec<serde_json::Value>>(r#"[{"txflow_name":"A-USDC"},{"txflow_name":"B-USDC"}]"#).unwrap();
src//main.rs:10:mod txflow;
src//main.rs:11:mod txflow_audit;
src//main.rs:12:mod txflow_points;
src//main.rs:13:mod txflow_agent;
src//main.rs:62:    hl_live.config.txflow = false;
src//main.rs:86:    // TxFlow owns its database, settings and records; no HL data is reused.
src//main.rs:87:    let tx_db = std::env::var("STARS_TXFLOW_DB").unwrap_or_else(|_| {
src//main.rs:88:        std::path::Path::new(&db_path).with_file_name("txflow-candles.sqlite").to_string_lossy().into_owned()
src//main.rs:90:    let tx_path = std::env::var("STARS_TXFLOW_LIVE").unwrap_or_else(|_| {
src//main.rs:91:        std::path::Path::new(&live_path).with_file_name("txflow-live.json").to_string_lossy().into_owned()
src//main.rs:93:    anyhow::ensure!(tx_db != db_path && tx_path != live_path, "TxFlow 数据路径不能与 Hyperliquid 相同");
src//main.rs:94:    let (mut tx_live, tx_load_error) = match live::LiveState::load_txflow(std::path::Path::new(&tx_path)) {
src//main.rs:101:    tx_live.config.txflow = true;
src//main.rs:102:    // TxFlow 的**数据库**打不开时，绝不能拖垮整个进程 —— HL 正在跑真钱。
src//main.rs:104:    // 原来这里是 `Store::open(&tx_db)?`：TxFlow 的 sqlite 损坏/被锁/路径不可写，
src//main.rs:106:    // 并把 TxFlow 标记为不可用（与"状态文件损坏"走同一套降级路径）。
src//main.rs:110:            warn!("TxFlow 数据库打开失败，不影响 Hyperliquid：{e:#}");
src//main.rs:111:            let tmp = std::env::temp_dir().join("stars-txflow-unavailable.sqlite");
src//main.rs:131:        tokio::spawn(async move { txflow::background(tx).await; });
src//main.rs:133:        tokio::spawn(async move { txflow_audit::background(audit).await; });
src//main.rs:285:        Some(error) => web::txflow_unavailable_router(error),
src//main.rs:286:        None => web::txflow_router(tx_state),
src//txflow.rs:1://! TxFlow mainnet adapter. Public data is independent of the Hyperliquid cache.
src//txflow.rs:2://! Ordered MessagePack payloads follow the current TxFlow web client's wire format.
src//txflow.rs:23:const BASE: &str = "https://api.txflow.com";
src//txflow.rs:35:        string txflowNetwork;
src//txflow.rs:62:/// TxFlow 的真实业绩汇总（全部来自交易所流水，不依赖我们自己的记账）。
src//txflow.rs:149:    // The endpoint identifies the venue/network. This cache belongs only to TxFlow.
src//txflow.rs:162:/// 两次 TxFlow 请求之间的最小间隔。
src//txflow.rs:164:/// 300ms（3.3 req/s）实测仍然触发 429 —— TxFlow 的限制比这更严。放宽到 1s。
src//txflow.rs:167:/// 两次 TxFlow 请求之间的默认最小间隔：20ms ≈ **50 QPS**。
src//txflow.rs:175:/// 环境变量 `STARS_TXFLOW_MIN_GAP_MS` 覆盖。
src//txflow.rs:179:        std::env::var("STARS_TXFLOW_MIN_GAP_MS")
src//txflow.rs:193:        anyhow::ensure!(v["code"] == 200, "TxFlow: {v}");
src//txflow.rs:197:            .context("TxFlow response missing data");
src//txflow.rs:214:        anyhow::ensure!(response["status"] == "ok", "TxFlow 拒绝授权: {response}");
src//txflow.rs:227:                .context("无效的 TxFlow 主账户地址")?
src//txflow.rs:231:            let key = std::fs::read_to_string(path).context("无法读取 TxFlow Agent 密钥文件")?;
src//txflow.rs:234:                .map_err(|_| anyhow::anyhow!("TxFlow Agent 密钥格式错误"))?;
src//txflow.rs:255:            owned_path: std::env::var("STARS_TXFLOW_ORDERS")
src//txflow.rs:258:                    let live_path = std::env::var("STARS_TXFLOW_LIVE")
src//txflow.rs:261:                    PathBuf::from(live_path).with_file_name("txflow-orders.json")
src//txflow.rs:265:            let dir=Path::new("/var/tmp/stars-txflow-account-locks");
src//txflow.rs:266:            std::fs::create_dir_all(dir).context("无法创建 TxFlow 账户锁目录")?;
src//txflow.rs:269:                .context("无法打开 TxFlow 账户锁")?;
src//txflow.rs:270:            lock.try_lock().map_err(|e|anyhow::anyhow!("TxFlow 账户锁被其他实例占用或不可用: {e}"))?;
src//txflow.rs:298:                if retried_429 { bail!("TxFlow 429 Too Many Requests（已重试一次）"); }
src//txflow.rs:300:                last=Some(anyhow::anyhow!("TxFlow 429 Too Many Requests"));
src//txflow.rs:315:        Err(last.unwrap_or_else(|| anyhow::anyhow!("TxFlow 请求失败")))
src//txflow.rs:323:            .context("TxFlow perpMeta 缺少 universe")?
src//txflow.rs:351:        anyhow::ensure!(!markets.is_empty(), "TxFlow 可交易市场为空");
src//txflow.rs:359:            .with_context(|| format!("未知 TxFlow 市场 {coin}"))?;
src//txflow.rs:363:        let bid = number(&book["levels"][0][0]["px"]).context("TxFlow 买盘为空")?;
src//txflow.rs:364:        let ask = number(&book["levels"][1][0]["px"]).context("TxFlow 卖盘为空")?;
src//txflow.rs:365:        anyhow::ensure!(ask >= bid && bid > 0., "TxFlow 盘口价格无效");
src//txflow.rs:372:            Err(e) => { tracing::warn!("TxFlow {coin} 无盘口: {e}"); None }
src//txflow.rs:386:        anyhow::ensure!(!self.account.is_empty(), "未配置 TxFlow 账户");
src//txflow.rs:390:        v.as_array().cloned().context("TxFlow ledger 格式错误")
src//txflow.rs:416:        let fills: Vec<Value> = raw.as_array().context("TxFlow fills 格式错误")?.clone();
src//txflow.rs:478:            .with_context(|| format!("未知或不可交易的 TxFlow 持仓/订单: {coin} {id:?}"))
src//txflow.rs:482:        anyhow::ensure!(!self.account.is_empty(), "未配置 TxFlow 主账户地址");
src//txflow.rs:485:            number(&v["marginSummary"]["accountValue"]).context("TxFlow 账户缺少净值字段")?;
src//txflow.rs:489:            .context("TxFlow 账户缺少持仓字段")?
src//txflow.rs:492:            let size = number(&p["szi"]).context("TxFlow 持仓缺少 szi，无法安全调仓")?;
src//txflow.rs:499:                "TxFlow 双向持仓不支持，请先切换单向持仓"
src//txflow.rs:531:            .context("TxFlow 客户端未配置 Agent 密钥")?;
src//txflow.rs:557:            result.context("TxFlow 发单结果未知；请核对账户后再操作，系统不会自动重发")?;
src//txflow.rs:558:        anyhow::ensure!(response["status"] == "ok", "TxFlow 拒绝: {response}");
src//txflow.rs:563:        let m = self.markets.get(coin).context("unknown TxFlow market")?;
src//txflow.rs:596:        let m = self.markets.get(coin).context("unknown TxFlow market")?;
src//txflow.rs:625:            .context("TxFlow 缺少逐单回执，不能确认成交")?;
src//txflow.rs:626:        anyhow::ensure!(statuses.len() == 1, "TxFlow 逐单回执数量错误");
src//txflow.rs:631:        // TxFlow 的回执是 `{"filled":{"totalSz":"7444","avgPx":"0.058205","oid":…}}`
src//txflow.rs:651:            serde_json::from_value(raw.clone()).context("未知 TxFlow 订单回执")?;
src//txflow.rs:653:            bail!("TxFlow 订单拒绝: {e}");
src//txflow.rs:671:    // TxFlow does not document client order ids. Persist the returned order ids
src//txflow.rs:675:            Ok(s) => Ok(serde_json::from_str(&s).context("TxFlow 订单归属文件损坏")?),
src//txflow.rs:709:            other => bail!("TxFlow 止盈单未挂上: {other:?}"),
src//txflow.rs:716:        for o in v.as_array().context("TxFlow openOrders 格式错误")? {
src//txflow.rs:730:                cloid: owned.contains(&oid).then(|| "0x5741txflow".to_string()),
src//txflow.rs:755:                .context("TxFlow 缺少撤单回执")?;
src//txflow.rs:758:                "TxFlow 撤单未确认: {response}"
src//txflow.rs:779:        v.as_array().cloned().context("TxFlow funding 格式错误或接口不支持")
src//txflow.rs:784:        number(&v["crossMaintenanceMarginUsed"]).context("TxFlow 缺少维持保证金字段")
src//txflow.rs:846:        txflowNetwork: "TxFlow-Mainnet".into(),
src//txflow.rs:852:        name: Some("TxFlow-Mainnet".into()),
src//txflow.rs:861:/// Refresh all TxFlow markets before calculating a signal; reject stale/gapped daily data.
src//txflow.rs:867:        .context("TxFlow 行情更新超过 3 分钟，请稍后重试")?
src//txflow.rs:890:        "TxFlow 元数据不完整：{} 个市场，之前 {} 个；拒绝交易", universe.len(), meta.universe.len());
src//txflow.rs:892:        && (-8..=8).contains(&m.decimals)), "TxFlow 元数据字段非法");
src//txflow.rs:894:    anyhow::ensure!(markets.iter().all(|m| indices.insert(m.index)), "TxFlow 重复市场索引");
src//txflow.rs:904:    store.txflow_cursor(total,0)
src//txflow.rs:907:    store.txflow_cursor(total,processed)?;Ok(())
src//txflow.rs:959:                "TxFlow 回填达时间预算（{}/{}），本轮提前收尾，剩余下轮继续",
src//txflow.rs:979:                tracing::warn!("TxFlow 回填 {} 失败，跳过: {e}", m.name);
src//txflow.rs:988:                tracing::warn!("TxFlow 回填 {} 解析失败，跳过: {e}", m.name);
src//txflow.rs:1019:        tracing::warn!("TxFlow 回填完成，{failed}/{} 个市场失败（已跳过）", markets.len());
src//txflow.rs:1156:        let coin = entry["txflow_name"].as_str().context("币安映射缺少 txflow_name")?;
src//txflow.rs:1201:                    if due { st.last_plan = vec![format!("TxFlow 调仓等待币安数据：{e:#}")]; let _ = st.save(&state.live_path); }
src//txflow.rs:1210:            if let Err(e)=run_auto(&state).await { tracing::warn!("TxFlow 优先恢复失败: {e}"); }
src//txflow.rs:1216:                        tracing::warn!("TxFlow 自动调仓未完成: {e}");
src//txflow.rs:1222:                    tracing::warn!("TxFlow 日线更新失败: {e}");
src//txflow.rs:1255:        st.begin_txflow_execution(&state.live_path)?;
src//txflow.rs:1256:        st.last_plan = vec!["TxFlow 自动调仓执行中…".into()];
src//txflow.rs:1268:            st.complete_txflow_execution(r.aborted, now);
src//txflow.rs:1274:        Err(e) => st.last_plan = vec![format!("TxFlow 自动调仓失败（等待优先恢复）: {e}")],
src//txflow.rs:1409: let exec=crate::exchange::Exec::test_txflow(client);
src//txflow.rs:1427:    let mut state=crate::live::LiveState::default();state.pending_recovery=Some(crate::live::RecoveryState{account:"mock".into(),started_at:1,reason:"in progress".into()});state.complete_txflow_execution(o.aborted,2);assert!(state.pending_recovery.is_some());assert!(state.last_run_at.is_none());
src//txflow.rs:1486:     let Ok(key)=std::env::var("TXFLOW_FIX_CHILD_KEY") else {return;};
src//txflow.rs:1487:     MOCK_PORT.store(std::env::var("TXFLOW_FIX_CHILD_PORT").unwrap().parse().unwrap(),Ordering::SeqCst);
src//txflow.rs:1488:     let result=Client::new(&std::env::var("TXFLOW_FIX_CHILD_ACCOUNT").unwrap(),Some(Path::new(&key))).await;
src//txflow.rs:1498:     let owned=root.join("orders.json");let old=std::env::var_os("STARS_TXFLOW_ORDERS");std::env::set_var("STARS_TXFLOW_ORDERS",&owned);
src//txflow.rs:1502:         .arg("--exact").arg("txflow::fix_regressions::account_lock_child").arg("--nocapture")
src//txflow.rs:1503:         .env("TXFLOW_FIX_CHILD_ACCOUNT",&account).env("TXFLOW_FIX_CHILD_KEY",&key).env("TXFLOW_FIX_CHILD_PORT",port.to_string()).env("STARS_TXFLOW_ORDERS",&owned)
src//txflow.rs:1507:     if let Some(old)=old {std::env::set_var("STARS_TXFLOW_ORDERS",old);} else {std::env::remove_var("STARS_TXFLOW_ORDERS");}
src//txflow.rs:1509:     let _=std::fs::remove_file(Path::new("/var/tmp/stars-txflow-account-locks").join(format!("{}.lock",account.to_ascii_lowercase())));
src//txflow.rs:1607: #[tokio::test] async fn equity_and_pnl_reader_select_txflow_without_signing_key() {
src//txflow.rs:1612:     let mut cfg=crate::live::LiveConfig{txflow:true,account:"0x0707070707070707070707070707070707070707".into(),..Default::default()};
src//txflow.rs:1613:     let tx=crate::exchange::Exec::reader_config(&cfg).await.unwrap();cfg.txflow=false;
src//txflow.rs:1616:     assert!(tx.is_txflow(),"TxFlow equity/PNL factory selected Hyperliquid");assert!(!hl.is_txflow());
src//live.rs:40:    pub txflow: bool,
src//live.rs:54:    /// （每天微调一堆）。这就是"HL 不需要调、TxFlow 一大堆要调"的真实原因。
src//live.rs:76:            txflow: false,
src//live.rs:108:        if self.txflow { self.leverage.floor().clamp(1., 20.) } else { self.leverage }
src//live.rs:313:pub fn merge_txflow_reconciliation(current: &mut LiveState, snapshot: &LiveState) {
src//live.rs:351:    pub fn begin_txflow_execution(&mut self, path: &std::path::Path) -> Result<()> {
src//live.rs:354:                "TxFlow 恢复账户与当前配置不一致，拒绝执行");
src//live.rs:367:    pub fn complete_txflow_execution(&mut self, aborted: bool, now: i64) {
src//live.rs:378:    /// TxFlow must not trade after losing persisted configuration/recovery.
src//live.rs:380:    pub fn load_txflow(path: &std::path::Path) -> Result<Self> {
src//live.rs:396:        })().with_context(|| format!("TxFlow 状态文件损坏或不可读取 {}；拒绝启动实盘，请修复状态文件（恢复记录不得丢弃）", path.display()));
src//live.rs:413:        if !self.config.txflow {
src//live.rs:466:    /// TxFlow：交易所口径的已实现盈亏、手续费、成交量、净入金。
src//live.rs:544:    let exec_res = if cfg.txflow {
src//live.rs:545:        Exec::txflow(&cfg.account, None).await
src//live.rs:566:    // TxFlow：**不拉盘口**。
src//live.rs:568:    // 这里的 mids 只用来给持仓估值，而 TxFlow 的 clearinghouseState 已经返回了
src//live.rs:573:    let mids = if cfg.txflow {
src//live.rs:594:                // TxFlow 状态页本来就不拉盘口（为省掉每个持仓一次请求），
src//live.rs:665:    if let Some(result) = exec.txflow_maintenance_margin().await {
src//live.rs:671:            Err(e) => { snap.error = Some(format!("读取 TxFlow 维持保证金失败: {e}")); snap.liq_buffer_pct = 0.; }
src//live.rs:677:    // 曾经在 snapshot() 里调 txflow_pnl()：那要发 2 个请求，而 snapshot 是页面
src//live.rs:680:    // 改成独立接口 /api/txflow/pnl，由页面单独、低频地拉。
src//live.rs:705:    if cfg.txflow {
src//live.rs:707:            "TxFlow 有效因子宇宙不足 20，拒绝计划");
src//live.rs:710:        anyhow::ensure!(longs >= 3 && shorts >= 3, "TxFlow 每腿至少 3 个名字，拒绝计划");
src//live.rs:712:        anyhow::ensure!(scale.is_finite() && scale > 0., "TxFlow 风险预算非法");
src//live.rs:714:            "TxFlow 单币名义不得超过净值 50%，拒绝计划");
src//live.rs:718:            "TxFlow 每腿名义上限 150%、净敞口上限 10%，拒绝计划");
src//live.rs:723:    // 消息里必须说清**用的是哪个数据源** —— 信号来自 Hyperliquid、执行在 TxFlow，
src//live.rs:724:    // 之前一律写"TxFlow 有效日线"，排查时分不清是哪个库的币不够。
src//live.rs:725:    if cfg.txflow && liquid.len() < 8 {
src//live.rs:734:    if cfg.txflow {
src//live.rs:747:/// TxFlow ranks the full Binance universe before execution filtering.
src//live.rs:754:    if cfg.txflow {
src//live.rs:755:        let hs=signal_store.context("TxFlow 缺少币安信号源，拒绝计划")?;
src//live.rs:779:        "TxFlow 新鲜且连续的币安信号宇宙不足 20：合格 {}/{total}，需要截至昨日 UTC 的连续 {need} 根日线，库内最新距今日 {age} 天；拒绝计划",
src//live.rs:784:fn signal_description(txflow: bool, count: usize, markets: usize) -> String {
src//live.rs:785:    if txflow { format!("信号源: 币安（TxFlow 独立组合） · 可排名 {count} 币 / TxFlow 共 {markets} 市场（已验证映射 82 币）") }
src//live.rs:813:    let exec = if cfg.txflow {
src//live.rs:814:        Exec::txflow(&cfg.account, if live { Some(std::path::Path::new(&cfg.key_path)) } else { None }).await?
src//live.rs:821:    if cfg.txflow && state.pending_recovery.is_some() { tc.rebalance_slices=1; }
src//live.rs:831:    let source = if cfg.txflow { "币安（TxFlow 独立组合）" } else { "Hyperliquid" };
src//live.rs:834:    let dropped_unsupported = if cfg.txflow {
src//live.rs:845:    if cfg.txflow {
src//live.rs:847:            "TxFlow 存在缺价持仓，拒绝计划");
src//live.rs:853:                "TxFlow: {dropped} 个目标市场没有盘口，已从目标中剔除（剩余 {}）",
src//live.rs:863:            "TxFlow 所选市场杠杆上限低于组合杠杆，请降低杠杆"
src//live.rs:866:    let plan = if cfg.txflow {
src//live.rs:867:        trader::build_txflow_plan(&weights, &acct, &exec.open_order_details().await?, markets, &mids, &tc)?
src//live.rs:882:            "注: 以下 {} 个币 TxFlow 不支持交易，已跳过（权重未重新归一化，总名义相应略低）: {}",
src//live.rs:906:    plan_lines.push(signal_description(cfg.txflow, panel.len(), markets.len()));
src//live.rs:988:            r.result = if cfg.txflow {
src//live.rs:1050:    let exec = if cfg.txflow { Exec::txflow(&cfg.account, Some(std::path::Path::new(&cfg.key_path))).await? } else { Exec::signer(&cfg.account, std::path::Path::new(&cfg.key_path)).await? };
src//live.rs:1070:        if cfg.txflow { "币安（TxFlow 独立组合）" } else { "Hyperliquid" },
src//live.rs:1261:            Err(e) if exec.is_txflow() => return Err(e.context("TxFlow 止盈挂单失败，已停止后续发单")),
src//live.rs:1739:mod txflow_fix_tests {
src//live.rs:1742:    fn txflow_nonempty_weights_still_require_factor_universe_and_two_legs() {
src//live.rs:1743:        let cfg=LiveConfig { txflow:true, ..Default::default() };
src//live.rs:1753:    fn txflow_concentration_and_net_exposure_are_hard_limits() {
src//live.rs:1754:        let cfg=LiveConfig { txflow:true, ..Default::default() };
src//live.rs:1762:    fn txflow_atomic_save_preserves_previous_file_when_temp_write_fails() {
src//live.rs:1764:        let path=root.join("live.json");let mut state=LiveState::default();state.config.txflow=true;
src//live.rs:1771:    fn txflow_reconciliation_preserves_concurrent_orders_and_deduplicates_fills() {
src//live.rs:1775:        merge_txflow_reconciliation(&mut current,&snapshot);
src//live.rs:1777:        merge_txflow_reconciliation(&mut current,&snapshot);assert_eq!(current.records.len(),3);
src//live.rs:1780:    fn txflow_signal_requires_binance_source_and_yesterdays_contiguous_bars() {
src//live.rs:1790:        let cfg=LiveConfig{txflow:true,..Default::default()};
src//live.rs:1795:        assert!(stale.is_err(),"stale HL factor bars must not produce a TxFlow live signal");
src//live.rs:1808:        let panel=signal_panel(&hl,Some(&bn),&LiveConfig{txflow:true,..Default::default()},&markets).unwrap();
src//live.rs:1813:        assert_eq!(signal_description(true,82,227),"信号源: 币安（TxFlow 独立组合） · 可排名 82 币 / TxFlow 共 227 市场（已验证映射 82 币）");
src//live.rs:1818:    fn txflow_fractional_leverage_uses_actual_integer_budget() {
src//live.rs:1819:        let cfg = LiveConfig { txflow: true, leverage: 3.49, ..Default::default() };
src//live.rs:1821:        let hl = LiveConfig { txflow: false, leverage: 3.49, ..Default::default() };
src//live.rs:1831:  let mut st=LiveState::default();st.config.txflow=true;st.config.account="mock".into();st.config.key_path="mock-key".into();st.begin_txflow_execution(&path).unwrap();
src//live.rs:1832:  assert!(LiveState::load_txflow(&path).unwrap().pending_recovery.is_some());
src//live.rs:1833:  std::fs::write(&path,"{truncated").unwrap();let result=LiveState::load_txflow(&path);std::fs::remove_dir_all(root).unwrap();
src//live.rs:1839:  std::fs::write(&path,value.to_string()).unwrap();assert!(LiveState::load_txflow(&path).is_ok());
src//live.rs:1840:  value["config"].as_object_mut().unwrap().remove("account");std::fs::write(&path,value.to_string()).unwrap();let result=LiveState::load_txflow(&path);std::fs::remove_dir_all(root).unwrap();assert!(result.is_err(),"missing required account must be explicit error");
src//live.rs:1978:        merge_txflow_reconciliation(&mut current, &snapshot);
src//txflow_agent.rs:28:        string txflowNetwork;
src//txflow_agent.rs:61:        include_str!("../static/txflow-agent.js"),
src//txflow_agent.rs:86:        .join("txflow-agents")
src//txflow_agent.rs:158:                    {"name":"txflowNetwork","type":"string"}, {"name":"chainId","type":"uint32"},
src//txflow_agent.rs:163:            "domain": {"name":"TxFlow-Mainnet","version":"1","chainId":self.signature_chain_id,
src//txflow_agent.rs:166:            "message": {"txflowNetwork":"TxFlow-Mainnet","chainId":869,"apiVersion":1,
src//txflow_agent.rs:172:            txflowNetwork: "TxFlow-Mainnet".into(),
src//txflow_agent.rs:180:            name: Some("TxFlow-Mainnet".into()),
src//txflow_agent.rs:255:        finish_approval(state, body, "https://api.txflow.com/exchange").await
src//txflow_agent.rs:263:            "授权任务中断，请核对 TxFlow 账户后重新授权",
src//txflow_agent.rs:302:        crate::txflow::approve_agent(&state.http, &p.exchange_body(sig), endpoint)
src//txflow_agent.rs:306:        save_pending(&dir, &p).context("TxFlow 已接受授权，但本地回执保存失败，请勿启用实盘")?;
src//txflow_agent.rs:318:            "TxFlow 已授权，保存配置失败: {e}。修复目录权限后点击重试保存即可，不会重复发授权"
src//txflow_agent.rs:403:        live.config.txflow = true;
src//txflow_agent.rs:411:            live_path: Arc::new(root.join("txflow-live.json")),
src//txflow_agent.rs:455:            live_path: Arc::new(root.join("txflow-agents")),
src//trader.rs:40:    /// 对 $2900 的账号是「3.8%」（每天微调一堆）。这就是"HL 不需要调、TxFlow 一大堆要调"
src//trader.rs:121:    /// Admitted TxFlow final book, including positions retained by slicing/bands.
src//trader.rs:122:    pub txflow_targets: HashMap<String, (f64, f64, i32)>,
src//trader.rs:620:struct TxFlowMarginExceeded {
src//trader.rs:627:impl std::fmt::Display for TxFlowMarginExceeded {
src//trader.rs:629:        write!(f, "TxFlow 保留仓＋可能成交挂单＋计划保证金超预算：预计 ${:.2}，预算 ${:.2}，超出 ${:.2}；其中目标仓位 ${:.2}（含保留仓 ${:.2}），{} 笔挂单额外预留 ${:.2}",
src//trader.rs:634:impl std::error::Error for TxFlowMarginExceeded {}
src//trader.rs:636:/// TxFlow planning entry point; extracted from live::run to test portfolio admission offline.
src//trader.rs:637:pub fn build_txflow_plan(
src//trader.rs:641:    let first_error = match build_txflow_plan_at_scale(weights, acct, open_orders, markets, mids, cfg, 1.) {
src//trader.rs:645:    let Some(mut failure) = first_error.downcast_ref::<TxFlowMarginExceeded>().cloned() else { return Err(first_error); };
src//trader.rs:661:        match build_txflow_plan_at_scale(weights, acct, open_orders, markets, mids, cfg, scale) {
src//trader.rs:664:                    first_error.downcast_ref::<TxFlowMarginExceeded>().unwrap().required,
src//trader.rs:668:            Err(error) => match error.downcast_ref::<TxFlowMarginExceeded>() {
src//trader.rs:677:fn build_txflow_plan_at_scale(
src//trader.rs:683:        && cfg.margin_buffer>0. && cfg.margin_buffer<=1., "TxFlow 风险预算非法");
src//trader.rs:687:        anyhow::ensure!(weight.is_finite() && names.insert(coin), "TxFlow 权重非法或币名重复");
src//trader.rs:688:        let mid = mids.get(coin).context("TxFlow 目标缺价格")?;
src//trader.rs:690:            "TxFlow 目标价格/市场元数据非法");
src//trader.rs:693:        anyhow::ensure!(position.size.is_finite(), "TxFlow 实际仓位数量非法");
src//trader.rs:696:                "TxFlow 已有逐仓或杠杆未知，必须先显式重建");
src//trader.rs:705:            let mid=*mids.get(coin).context("TxFlow 保留目标缺价格")?;
src//trader.rs:706:            let decimals=markets.get(coin).context("TxFlow 保留目标缺市场")?.sz_decimals;
src//trader.rs:707:            plan.txflow_targets.insert(coin.clone(),(*size,mid,decimals));
src//trader.rs:717:        anyhow::ensure!(o.sz.is_finite() && o.sz>=0. && o.px.is_finite() && o.px>0., "TxFlow 挂单字段非法");
src//trader.rs:718:        anyhow::ensure!(o.side == "B" || o.side == "A", "TxFlow 挂单方向未知，拒绝计划");
src//trader.rs:719:        let mid = risk_prices.get_mut(&o.coin).context("TxFlow 挂单缺价格")?;
src//trader.rs:720:        anyhow::ensure!(mid.is_finite() && *mid > 0., "TxFlow 挂单风险价格非法");
src//trader.rs:737:        let mid=*risk_prices.get(&coin).with_context(||format!("TxFlow 缺价风险 {coin}"))?;
src//trader.rs:738:        anyhow::ensure!(mid.is_finite() && mid>0. && low.is_finite() && high.is_finite(),"TxFlow 风险价格/数量非法");
src//trader.rs:739:        let market=markets.get(&coin).context("TxFlow 风险市场元数据缺失")?;
src//trader.rs:740:        anyhow::ensure!(cfg.leverage<=market.max_leverage as f64,"TxFlow 市场杠杆上限不足");
src//trader.rs:744:            anyhow::ensure!(p.is_cross && p.leverage>0,"TxFlow 保留仓为逐仓或杠杆未知，必须先显式重建");
src//trader.rs:751:        anyhow::ensure!(worst<=acct.equity*0.5+1e-8,"TxFlow 最终单币名义超净值 50%");
src//trader.rs:757:    anyhow::ensure!(long_names>=3 && short_names>=3,"TxFlow 最终组合每腿不足 3 个名字");
src//trader.rs:759:        && net_low.abs().max(net_high.abs())<=acct.equity*0.1+1e-8,"TxFlow 最终两腿/可能净敞口超限");
src//trader.rs:762:        return Err(TxFlowMarginExceeded { required:margin, budget, positions:position_margin,
src//trader.rs:765:    plan.gross_notional = plan.txflow_targets.values().map(|(size, mid, _)| size.abs() * mid).sum();
src//trader.rs:766:    plan.per_coin = plan.gross_notional / plan.txflow_targets.len().max(1) as f64;
src//trader.rs:798:    if !live || !exec.is_txflow() { return Ok(outcome); }
src//trader.rs:824:            let mut targets = plan.txflow_targets.clone();
src//trader.rs:845:                "TxFlow 最终账户未达到逐币目标、逐币敞口、总敞口或净敞口门槛，必须受控恢复".into()
src//trader.rs:846:            } else { "TxFlow 最终账户已核验逐币目标、逐币敞口、总敞口及净敞口".into() });
src//trader.rs:850:            outcome.prelim.push("TxFlow 最终账户读取失败（重试 4 次仍不可用），必须受控恢复".into());
src//trader.rs:859:    let mut targets = plan.txflow_targets.clone();
src//trader.rs:909:                if exec.is_txflow() { crate::exchange::describe_txflow(&st, o.size) } else { crate::exchange::describe(&st) },
src//trader.rs:912:            Err(e) if exec.is_txflow() => return Ok(Outcome::abort(prelim, orders, Some(i), format!("TxFlow 平仓失败: {e}"))),
src//trader.rs:925:        Err(e) if exec.is_txflow() => return Ok(Outcome::abort(prelim, orders, None, format!("TxFlow 重读账户失败: {e}"))),
src//trader.rs:952:                Err(e) if exec.is_txflow() => return Ok(Outcome::abort(prelim, orders, None, format!("{coin} TxFlow 杠杆设置失败: {e}"))),
src//trader.rs:986:                        if exec.is_txflow() { crate::exchange::describe_txflow(&st, actual.abs()) } else { crate::exchange::describe(&st) }
src//trader.rs:988:                    Err(e) if exec.is_txflow() => return Ok(Outcome::abort(prelim, orders, Some(i), format!("TxFlow 残余平仓失败: {e}"))),
src//trader.rs:1023:                    if exec.is_txflow() { crate::exchange::describe_txflow(&st, dsize) } else { crate::exchange::describe(&st) },
src//trader.rs:1030:                Err(e) if exec.is_txflow() => return Ok(Outcome::abort(prelim, orders, Some(i), format!("TxFlow 下单失败: {e}"))),
src//trader.rs:1043:    if let Some(result) = exec.txflow_prices(coins).await {
src//trader.rs:1044:        return result.unwrap_or_else(|e| { tracing::warn!("TxFlow 盘口读取失败: {e}"); HashMap::new() });
src//trader.rs:1285:mod txflow_portfolio_tests {
src//trader.rs:1293:    #[test] fn txflow_possible_open_orders_cannot_exceed_final_margin() {
src//trader.rs:1296:        assert!(build_txflow_plan(&w,&a,&orders,&m,&p,&c).is_err(),"possible resting fills must consume risk budget");
src//trader.rs:1298:    #[test] fn txflow_retained_slot_position_cannot_exceed_risk_budget() {
src//trader.rs:1303:        assert!(build_txflow_plan(&w,&a,&[],&m,&p,&c).is_err(),"retained slot risk must not bypass admission");
src//trader.rs:1305:    #[test] fn txflow_actual_low_leverage_or_isolated_positions_fail_admission() {
src//trader.rs:1308:        assert!(build_txflow_plan(&w,&a,&[],&m,&p,&c).is_err(),"actual low leverage consumes more margin");
src//trader.rs:1311:        assert!(build_txflow_plan(&w,&a,&[],&m,&p,&c).is_err(),"isolated positions require explicit rebuild");
src//trader.rs:1314:    fn txflow_margin_gate_rejects_small_balanced_resting_fills() {
src//trader.rs:1322:        let error = build_txflow_plan(&w, &a, &orders, &m, &p, &c).unwrap_err();
src//trader.rs:1326:    fn txflow_reduce_only_fill_can_break_neutrality() {
src//trader.rs:1330:        let error = build_txflow_plan(&w, &a, &orders, &m, &p, &c).unwrap_err();
src//trader.rs:1334:    fn txflow_invalid_weight_and_missing_target_price_fail_closed() {
src//trader.rs:1337:        assert!(build_txflow_plan(&w, &a, &[], &m, &p, &c).is_err());
src//trader.rs:1340:        assert!(build_txflow_plan(&w, &a, &[], &m, &p, &c).is_err());
src//trader.rs:1343:    fn txflow_small_retained_drift_is_resized_inside_the_same_margin_budget() {
src//trader.rs:1354:        let plan = build_txflow_plan(&w, &a, &[], &m, &p, &c).expect("resize targets instead of relaxing the budget");
src//trader.rs:1357:        let margin: f64 = plan.txflow_targets.values().map(|(size, mid, _)| size.abs() * mid / 3.).sum();
src//trader.rs:1363:    fn txflow_budget_resize_preserves_off_slot_positions_and_order_floor() {
src//trader.rs:1374:        let plan = build_txflow_plan(&w, &a, &[], &m, &p, &c).unwrap();
src//trader.rs:1382:                assert_eq!(plan.txflow_targets[coin].0, position.size);
src//trader.rs:1385:        let margin: f64 = plan.txflow_targets.values().map(|(size, mid, _)| size.abs() * mid / 3.).sum();
src//trader.rs:1390:    fn txflow_margin_failure_reports_amounts_without_ignoring_resting_orders() {
src//trader.rs:1396:        let error = build_txflow_plan(&w, &a, &orders, &m, &p, &c).unwrap_err();
src//trader.rs:1404:    fn txflow_healthy_plan_does_not_change_targets_or_generate_churn() {
src//trader.rs:1412:        let plan = build_txflow_plan(&w, &a, &[], &m, &p, &c).unwrap();
src//trader.rs:1415:        for (coin, position) in &a.positions { assert_eq!(plan.txflow_targets[coin].0, position.size); }
src//trader.rs:1418:    #[test] fn txflow_balanced_portfolio_without_resting_orders_is_admitted() {
src//trader.rs:1419:        let (w,a,m,p,c)=fixture();assert!(build_txflow_plan(&w,&a,&[],&m,&p,&c).is_ok());
static//index.html:237:        <a class="btn ghost" href="/txflow">TxFlow 策略 →</a>
static//txflow-agent.js:36:    const response = await fetch(`/api/txflow/agent/${path}`, {
static//txflow-agent.js:47:      throw new Error("钱包未返回有效账户，请连接你在 TxFlow 使用的主账户。");
static//txflow-points.js:1:/* TxFlow only. Points never feed trading decisions or financial returns. */
static//txflow-points.js:31:      const r=await fetch('/api/txflow/points',{cache:'no-store'}), d=await r.json();
static//txflow-points.js:52:      const r=await fetch('/api/txflow/points',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({
static//txflow.html:7:    <title>TxFlow · stars · 动量 alpha 研究台</title>
static//txflow.html:301:        <div class="panel-head setup-heading"><div><div class="panel-kicker">LIVE SETUP · TXFLOW</div><h2>实盘设置</h2><p class="sub">账户连接、策略参数与执行控制，集中管理。</p></div><span class="setup-chip">设置变更后请保存</span></div>
static//txflow.html:304:        <section class="setup-card"><header><span class="setup-number">02</span><div><h3>账户连接</h3><p>使用交易主账户与服务器上的 API 密钥文件。</p></div></header><div class="setup-fields"><div class="field"><label for="lv-account">账户地址（主账户，不是 API 钱包）</label><input id="lv-account" placeholder="0x..." /></div><div class="field"><label for="lv-key">API 钱包密钥文件路径</label><input id="lv-key" placeholder="必填，例如 /etc/stars/txflow-agent.key" /></div></div></section>
static//txflow.html:332:<details class="setup-reference"><summary>策略说明与检查记录</summary>          <div class="note"><b>复用同一策略：</b>波动调整动量、低波动与成交量冲击的组合，多空对冲，按设置错峰调仓。信号只使用 TxFlow 已收盘日线与滚动成交额。<b>所有交易请求串行执行，间隔至少 300ms。</b></div>
static//txflow.html:334:          <p class="note"><b>流动性：</b>按近 30 个已收盘日的平均成交额筛选，策略至少需要 8 个候选市场。TxFlow 新配置默认 $500K；已有配置保持原值，候选不足时可调整门槛并保存后再生成计划。</p>
static//txflow.html:335:          <p class="note">可在上方生成并授权策略专用 Agent，成功后自动填写主账户和密钥路径。也可手动使用已有的已授权 Agent 文件。主钱包私钥不参与策略。账户需为单向持仓；资金、仓位和挂单均从 TxFlow 读取。维持保证金取自 TxFlow 账户接口；强平价以交易所返回值为准。</p>
static//txflow.html:336:        <div class="panel" style="margin-top:16px"><h2>TxFlow 接入状态</h2><p class="note">行情接口已验证。策略逻辑复用 Hyperliquid；在 TxFlow 的历史表现和真实成交链路尚待验证。实盘默认关闭，可先生成调仓计划。</p></div>
static//txflow.html:372:            <p class="note"><a href="https://docs.txflow.com/points/how-points-work" target="_blank" rel="noopener noreferrer">官方积分规则</a></p>
static//txflow.html:391:    <script src="/txflow/app.js"></script>
static//txflow.html:392:    <script src="/txflow/agent.js"></script>
static//txflow.html:393:    <script src="/txflow/points.js"></script>
static//app.js:46:        `已缓存 ${d.cached_coins} 币 · 流动性 ${d.universe_liquid} · ${d.exchange === "txflow" ? "TxFlow 日线就绪" : d.paper?.running ? "纸交易运行中" : "纸交易未启动"}`;
static//app.js:431:// TxFlow 业绩单独、低频拉取（60 秒一次），不放在状态轮询里 ——
static//app.js:436:  if (LIVE_EXCHANGE !== "TxFlow") return;
static//app.js:442:    // TxFlow's server rewrites the common /api/ prefix exactly once.
static//app.js:598:  if (LIVE_EXCHANGE !== "TxFlow") return "";
static//app.js:600:  if (!a) return `<section class="tx-audit"><header class="tx-audit-head"><div><span class="section-eyebrow">EXECUTION REVIEW</span><h3>TxFlow 成交核验</h3></div><span class="tx-audit-tag">等待数据</span></header><p class="tx-audit-note">成交证据暂不可用，策略归因待核验。</p></section>`;
static//app.js:602:    <header class="tx-audit-head"><div><span class="section-eyebrow">EXECUTION REVIEW</span><h3>TxFlow 成交核验</h3><p>已保存的账户流水 · 不等同于策略净收益</p></div><span class="tx-audit-tag">${escapeHTML(a.reconciliation?.label || "等待账户对账快照")}</span></header>
static//app.js:760:    (LIVE_EXCHANGE !== "TxFlow" && d.tx_fills
static//app.js:894:          } else if (LIVE_EXCHANGE === "TxFlow" && /Success|已发送/.test(r.result || "")) {
```

共有 465 行匹配。这里没有将残留宣称为必须保留的例外：除上述已证实的共享合并逻辑外，其余条目均因删除任务触发停止条件而尚未处理。它们是未完成项，不能用“基线构建/测试通过”抵销。

### 4. HL 路由清单（退出码 0）

```sh
grep -n 'route("/api/live' src/web.rs
```

```text
80:        .route("/api/live", get(live_status))
81:        .route("/api/live/config", post(live_config))
82:        .route("/api/live/run", post(live_run))
83:        .route("/api/live/reset", post(live_reset))
84:        .route("/api/live/rebuild", post(live_rebuild))
85:        .route("/api/live/tp", post(live_tp))
86:        .route("/api/live/records/clear", post(live_records_clear))
87:        .route("/api/live/capital", post(live_capital))
88:        .route("/api/live/capital/time", post(live_capital_time))
```

## 验收状态与未解决项

- 原始代码构建：通过，0 error、0 warning。
- 原始代码测试：103 passed / 0 failed / 1 ignored，与给定基线一致；保留原有 unused variable `cfg` warning（`src/trader.rs:1142`），未作无关修复。
- 删除后构建/测试：未进行，因为尚未实施删除。
- TxFlow 残留为空：不通过，实际 465 行匹配。
- HL 路由：9 条全部保留。
- 删除测试清单：空；无测试被删、跳过或改名。
- 需要解决的冲突：是否采用上述“完整保留共享合并实现、仅改名称与调用”的方案，以替代清单中的直接删除函数。
- 本报告没有对尚未改动的其他分支做完整等价性保证，也没有验证实盘交易；没有为了验证而启动应用服务或发出实盘交易请求。
