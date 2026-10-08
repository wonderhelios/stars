# TxFlow 组合准入验证 REPORT3

## 归属与基线

本次进入时工作树已有五个源码文件的未提交修改；`build_txflow_plan` 已有真实准入，并非仍直接委托 `build_plan`。首次实测四个原组合测试全部通过（REPORT3-before.log）。因此不能把已有修复冒称为本次从零完成，也不能声称本次首次运行复现三项失败。

历史失败证据来自本目录已有 `04-portfolio-before.log`：1 passed / 3 failed / 43 filtered out。原三个失败测试及合法组合通过测试均保持原断言、未删除、未加 ignore。

## 三项：失败 → 实现 → 通过

| 原测试 | 历史失败输出 | 当前实现 | 改后 |
| --- | --- | --- | --- |
| txflow_retained_slot_position_cannot_exceed_risk_budget | `retained slot risk must not bypass admission`，FAILED | 从全部实际仓位建立最终仓位图，仅用实际生成的订单目标覆盖本档；其余档、门槛以下未调整仓位继续计入。保留 100×100=$10000 的仓位不能绕过集中度和保证金限制 | REPORT3-after.log 与 REPORT3-full.log 均 ok |
| txflow_possible_open_orders_cannot_exceed_final_margin | `possible resting fills must consume risk budget`，FAILED | 最终仓位加所有未减仓挂单的可能成交区间；不假设取消成功或必定不成交，按逐币最坏绝对名义计算保证金、集中度和净敞口 | 两份日志均 ok |
| txflow_actual_low_leverage_or_isolated_positions_fail_admission | `actual low leverage consumes more margin`，FAILED；历史日志在第一条断言停止，不能冒称当时单独复现逐仓断言 | 已持仓按 min(实际逐市场杠杆, 配置杠杆) 算保证金；已有逐仓或未知杠杆拒绝，不能靠计划理想杠杆放行。450/1+2250/3=$1200，超过 $900 预算 | 两份日志均 ok，测试现在实际走过低杠杆与逐仓两条断言 |

## 本次补充

本次仅修改 src/trader.rs 的 TxFlow 准入和增加三个测试：

- 在 build_plan 静默跳过币之前拒绝非有限权重、重复币名、缺价和缺市场；全部已有非零仓位必须为已知杠杆全仓，即使计划想平仓也不假设平仓成功后即可安全开仓。
- 挂单先累积普通买卖量，再处理只减仓可能成交；风险区间与挂单列表顺序无关，只减仓不被当作必定释放保证金。风险价格采用 max(mid, 挂单价)，缺行情拒绝。
- 新增小额均衡多空挂单测试：两单各 $10，集中度和净敞口仍合法，但最终保证金约 $906.67>$900，必须由保证金门槛拒绝。
- 新增只减仓挂单测试：可能平掉一条 $450 空仓导致净敞口超 10%，拒绝。
- 新增 NaN 权重和目标缺价拒绝测试。

## 门槛口径与 30% 的冲突

现有原测试合法 fixture 为净值 $1000、3 倍杠杆、90% 部署、六币各 |w|=1/6，即每币名义 $450 = 净值 45%。因此“单币名义/净值 ≤30%”与保留合法组合 `is_ok()` 断言矛盾。三个失败测试本身并未指定精确集中度上限。

当前保留已有源码明确的单币名义/净值 50% 门槛；每腿至少三币、每腿名义/净值 ≤150%、任何挂单成交组合的净敞口/净值 ≤10%；最终保证金 ≤净值×margin_buffer。50% 不是本次为通过测试而放宽的值。该口径对 fixture 相当于每币初始保证金约 15% 净值；它会拒绝审查中的单币 $7830/$2900=270%，但不能宣称阻止所有“单币名义占净值30%”情形。

若上线要求确为名义/净值30%，需要策略扩大币数或降低部署、杠杆，并重新审定合法 fixture；本次遵守不改断言、不放宽阈值要求，没有偷偷改测试。通过这些测试不是达到严格30%安全要求的证明。

## 隔离、范围与限制

live::run 仅在 cfg.txflow 分支调用 build_txflow_plan；HL 分支仍调用原 build_plan。本次未修改 build_plan、HL 执行、鉴权或网络绑定。mock 已有 no_proxy()；未访问真钱账户或 TxFlow explorer，未 commit/push/部署。

本次未做压力损失模型、手续费/实时价格冲击额外预留、跨市场保证金净额或交易所可用保证金精确仿真；采用名义/实际杠杆的保守初始保证金估算及现有 margin_buffer。最终计划准入不能保证执行途中所有部分成交组合都中性，也不能代替已有最终对账和受控恢复。未扩大到其他审查问题。

用户指出的既有 `txflow_agent::tests::approval_receipt_persists_configuration_and_does_not_resubmit` 失败不是本次责任，未修改 txflow_agent.rs；本轮日志该项为 ok，不据此声称修复该已知问题或干净树必然通过。

## 最终验证

- 首轮 `cargo test --release --offline`：REPORT3-full.log，**53 passed / 0 failed / 1 ignored**，54 个测试，113.28 秒。
- 最终组合定向验证：REPORT3-portfolio-final.log，**7 passed / 0 failed / 0 ignored / 48 filtered out**。三个原失败测试、原合法测试及本次三个新增测试均通过。
- 首轮结束前后共享工作树新增其他测试，总数变为55；src/trader.rs 与 src/live.rs 哈希未变（REPORT3-source-sha256.txt）。最终对当前55测试版本完整验证：REPORT3-full-final.log，**54 passed / 0 failed / 1 ignored / 0 filtered out**，113.24 秒。原有测试没有观测到回归；测试数量变化如实按日志记录。
- `git diff --check` 通过；有原已存在的 unused variable cfg 编译警告，未为清理警告扩大修改范围。

最终归档时再次校验：src/trader.rs 哈希仍一致；src/live.rs 已被共享工作树的其他修改改变。因此最终计数严格对应 REPORT3-full-final.log 编译版本，不声称完整验证覆盖其后的 live.rs 修改。本次没有修改 live.rs。
