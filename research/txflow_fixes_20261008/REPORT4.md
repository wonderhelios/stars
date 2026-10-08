# TxFlow PnL 修复与完整回归 REPORT4

## 基线与修复归属

用户提供的基线为 50 passed / 3 failed / 1 ignored。本次进入时工作树已有 src/exchange.rs、live.rs、store.rs、trader.rs、txflow.rs、web.rs 的未提交修改；两个 PnL 测试所需的账户键、fills/ledger 请求错误传播已在当前源码中存在。本次首次定向运行实际为 **2 passed / 0 failed / 54 filtered out**（report4_before.log），不能冒称本次复现或独自修复历史失败。

历史失败证据取自任务开始前已有的 pnl-before.log，以下明确区分历史日志与本次实测。

## 两个测试：历史失败 → 实现 → 本次通过

### txflow::fix_regressions::pnl_cache_is_account_scoped

历史输出（pnl-before.log）：

```text
assertion `left == right` failed: same interval must not reuse another account PNL
  left: 1.0
 right: 2.0
test txflow::fix_regressions::pnl_cache_is_account_scoped ... FAILED
```

已有工作树已把账户加入键。本次改为显式 PnlCacheKey，包含 venue（TxFlow API endpoint，区分场所/网络）、完整 account 字符串、since_ms，三个字段严格相等且未超过 60 秒才命中。账户大小写变化也不复用；不再对键做小写转换。此缓存仅由 TxFlow adapter 使用，HL 不访问它。时间区间语义保持 [since_ms, latest]，滚动终点以现有 60 秒 TTL 控制，并非固定 end_ms 区间。

本次最终定向输出（report4_after.log）：

```text
test txflow::fix_regressions::pnl_cache_is_account_scoped ... ok
```

### txflow::fix_regressions::pnl_read_errors_are_not_zero_profit

历史输出（pnl-before.log）：

```text
read errors must not become cached zero profit
test txflow::fix_regressions::pnl_read_errors_are_not_zero_profit ... FAILED
```

已有代码以 `?` 传播 user_fills/ledger 请求错误，并对 fills 非数组响应报错。本次进一步把 ledger 非数组响应从 unwrap_or_default 空流水改为显式格式错误。缓存写入仍只发生在两次完整成功读取之后；任何请求或顶层数组解析错误提前返回 Err，不写成功零值。独立 /api/txflow/pnl 将 Some(Err) 返回为 `{ok:false,error:...}`，上层可见；未将错误转换为 Ok(0)。

本次最终定向输出（report4_after.log）：

```text
test txflow::fix_regressions::pnl_read_errors_are_not_zero_profit ... ok
```

两个原测试的全部断言保持原样。仅将共用 mock client 构造改为 builder().no_proxy()，避免本机 HTTP 代理干扰 127.0.0.1。

## 补充验证及编译问题

新增 pnl_ledger_failure_recovery_and_venue_interval_isolation，验证 ledger HTTP 503 与非数组响应均返回错误且缓存为空；随后同账户同区间恢复为 realized=7、net_deposit=12；重复成功查询不发请求；缓存 venue 不同时重新读取；since_ms 改变也重新读取，区间确实排除旧成交。新增测试不访问真实交易所。

新增 fixture 首次漏填 coin/market，被 user_fills 的未知币种安全校验提前拒绝，日志保存在 report4_fixture_failure.log。随后补齐 fixture 的币种和市场，所有断言保持不变。最终定向运行：**3 passed / 0 failed / 0 ignored / 55 filtered out**，25.05 秒。

重新编译曾发现 live.rs 现有 signal_store.context(...) 缺少 anyhow::Context trait 导入（E0599）；本次仅补 `use anyhow::{Context, Result};`。这是导入修正，未修改公共路径运行逻辑或 HL 行为。已有 unused variable cfg 警告仍存在，未为清理警告扩大范围。

## 完整回归

运行原命令 `cargo test --release --offline`，退出码 **0**。完整日志：report4_full.log。

```text
test result: ok. 57 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 126.24s
```

最终总计 **58 个测试，57 passed / 0 failed / 1 ignored**。本次仍然失败的测试：**无**。唯一 ignored 为 `txflow::public_tests::mainnet_public_reads`，其既有原因是只读 mainnet integration 需要网络；本次没有新增 ignore。

用户标记的既有失败 `txflow_agent::tests::approval_receipt_persists_configuration_and_does_not_resubmit` 在本次完整日志中实际为 **ok**。本次未修改该测试或其实现，不能把它记成仍失败，也不能宣称本次已修复其既有问题。它在 test_state 中仍使用 reqwest::Client::new()，未由本次加 no_proxy；其代理相关稳定性未在本次范围内修复或验证。

特别核对 HL 相关结果：`hl::tests::coin_meta_reads_is_delisted`、`hl::tests::coin_meta_reads_sz_decimals` 和 `web::txflow_routes_tests::txflow_page_and_configuration_are_isolated_from_hyperliquid` 全部 ok；通用 `trader::tests::*`、slice 计划测试、live take_profit/config 测试也全部 ok。没有观测到 HL 相关测试回归。

完整运行前后所有 src/*.rs 哈希一致（report4_source_before_full.json / report4_source_after_full.json）；因此本次最终计数覆盖报告归档时的源码版本。`git diff --check` 通过。

## HL 隔离与共享工作区

本次未修改 src/hl.rs 或 src/trader.rs；hl.rs 与本次开始快照字节一致。HL 源码 SHA-256：d0b5495bb52f98d8fc37e5b31ade11d91849d079b768ab38587416f321121c36。

本次修改的生产逻辑集中在 txflow.rs 的缓存和 ledger 格式校验；live.rs 仅由本次补 Context 导入。工作期间另外观察到共享工作区将 live::refresh_equity 与 web::txflow_pnl 的 reader_for 改为 reader_config，这不是本次编辑。当前 reader_config 在 cfg.txflow=false 时直接委托原 reader_for(Some(account))，HL 仍使用原读取路径。报告不把他人的共享修改归为本次成果。

完整回归开始前保存所有 src/*.rs 的哈希到 report4_source_before_full.json，结束后另行核对；计数以本次真实日志为准，不沿用用户的旧基线。测试总数已从本次首次运行的 56 变为 58，本次增加一个测试，另一个增加来自共享工作区。

## 未做事项与限制

- 未修改鉴权/网络绑定，未 commit、push、部署；未访问真实账户或 TxFlow explorer，未运行 ignored mainnet integration。
- 未修改 txflow_agent.rs，也未修复用户指出的既有 approval receipt 问题；其本次观测结果见完整回归，不能据此宣称既有问题已修复或永不复现。
- 未把 pnl_summary 放回 live::snapshot。仍是独立 API，前端 60 秒拉取。API 错误可见，但现有前端静默处理并可能保留上次成功值；本次未扩展为前端错误/陈旧状态提示。
- 保留现有 1000 条 fills、流水截断下界与 60 秒缓存语义；未实现分页、单条流水字段完整性校验、实时账户验证或其他审查发现。
- 本地离线测试通过不等于实盘验证，未对真钱账户作任何操作。
