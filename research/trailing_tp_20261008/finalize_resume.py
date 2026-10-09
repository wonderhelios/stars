import json, hashlib, datetime
from pathlib import Path
import numpy as np

D=Path(__file__).resolve().parent
def dt(t): return datetime.datetime.fromtimestamp(t/1000,datetime.UTC).isoformat()
audit={}
for label,folder in [('daily','/tmp/hl-daily-full'),('hourly','/tmp/hl-hist')]:
    rows=[]
    for p in sorted(Path(folder).glob('*.json')):
        b=p.read_bytes(); x=json.loads(b)
        times=[int(v['t']) for v in x]
        rows.append(dict(coin=p.stem,records=len(x),start=min(times) if times else None,end=max(times) if times else None,sha256=hashlib.sha256(b).hexdigest()))
    audit[label]=dict(files=len(rows),nonempty=sum(r['records']>0 for r in rows),records=sum(r['records'] for r in rows),start=dt(min(r['start'] for r in rows if r['start'] is not None)),end=dt(max(r['end'] for r in rows if r['end'] is not None)),files_manifest=rows)
src=Path('src/trader.rs').read_text(); harness=(D/'source_factor_check.rs').read_text()
factor=src[src.index('pub struct FactorPanel'):src.index('/// 目标权重（带符号')]
audit['factor_harness_verbatim']=factor.strip() in harness
audit['source_sha256']=hashlib.sha256(Path('src/trader.rs').read_bytes()).hexdigest()
audit['gate_sha256']=hashlib.sha256((D/'gate_predeclared.json').read_bytes()).hexdigest()
(D/'resume_data_audit.json').write_text(json.dumps(audit,indent=2))
r=json.loads((D/'baseline_resume_results.json').read_text()); verification=json.loads((D/'factor_verification_resume.json').read_text())
lines=['# 移动止盈回测续跑：基线复现不出来','', '**四选一结论：基线复现不出来。** 本次已核实数据并实际计算基线；按 `gate_predeclared.json` 的前置门槛停止，移动止盈候选实际执行数为 0。不能据此宣称移动止盈有效或无效。','', '## 数据核实','',f"指定 Python：`/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3`；numpy 2.3.5 / pandas 3.0.1。日线 {audit['daily']['files']} 个文件、{audit['daily']['nonempty']} 个非空市场、{audit['daily']['records']:,} 根；小时线盘点时 {audit['hourly']['files']} 个文件、{audit['hourly']['records']:,} 根，仍在外部抓取。逐文件哈希和覆盖见 `resume_data_audit.json`。文件数不等于有效市场数，不将空数组当作有历史。",'',f"日线原始并集覆盖 {audit['daily']['start']} 至 {audit['daily']['end']}；回测固定采用 UTC 2023-06-01 起至 2026-10-08 完整日线，条件 `t + 86400000 <= 1791504000000`，价格/量转 float。BTC 实查 1226 根，2023-06-01 至 2026-10-08，最后收盘 80641。不是沿用此前空目录结论。",'', '## 骨架与源码核对','', '- `baseline.py`：三因子分别建多空账本、平均后归一化，14日动量、包含当日的20日样本波动、排除当日的30日成交额均值均符合源码；但读取所有历史而未限制请求起点，且请求结束时间会排除10月8日完整日线。它也依赖可选 `T` 字段。保留原文件，另存修正入口 `baseline_resume.py`。', '- `verify_factors.py`：可用作权重校验；续跑入口 `verify_factors_resume.py` 使用修正面板。Rust harness 的 FactorPanel 与当前源码逐字一致检查：'+str(audit['factor_harness_verbatim'])+f"；全 {verification['days']} 日 × {verification['coins']} 币最大权重误差 {verification['max_abs_difference']:.3g}，不同条目 0。仅验证因子，不等于验证真实成交。", '- `gate_predeclared.json`：原样保留；仅当源码一致、可识别的等价配置复现1.62或2.14才运行网格。不新增容忍阈值，不调参凑数。', '- `legacy_stag2_snapshot.py`：不能作为当前基线。它 cap=8、1倍名义、固定权重而非持仓数量漂移，波动 ddof=0 且不含当日，流动性包含当日（分母还存在窗口计数差异）、Python round 平局口径、缺价收益填零、起点45及末日处理均有差异。仅保留作旧口径证据，未用它放行。', '', '源码逐条：`weights_at` 的 cap=5、20%截面、最低8币、至少5个有效成交额/收益观测、币名打破平局、排除冒号市场、权重抵消后归一化；`build_plan` 按权益×0.90×3=2.7定目标，floor到当前szDecimals，最小订单$10、最小仓位$15、2%调仓带，反手平仓与重开各收费；缺mid保留旧仓。默认 slices=3，本次明确覆盖为1。源码slippage=0.005是IOC限价容忍参数，不能当成实际成本；本次单边7bp按用户给定。', '', '## 实际基线结果', '', '初始权益$1,000,000；主口径前日已收盘信号→次日UTC开盘成交。持有数量逐日盯市，收费为交易名义×0.0007；不含资金费，不含终点强制清仓。Sharpe为日净收益均值/样本标准差×√365。1193个收益观测包含预热后早期零仓位日。', '', '|口径|全期Sharpe|2023–24|2025–26|对1.62差值|对2.14差值|日交易名义/权益|年化成本/权益|', '|---|---:|---:|---:|---:|---:|---:|---:|']
for label,z in r['runs'].items():
    f=z['full']; lines.append(f"|{label}|{f['sharpe']:.6f}|{z['2023-24']['sharpe']:.6f}|{z['2025-26']['sharpe']:.6f}|{f['sharpe']-1.62:+.6f}|{f['sharpe']-2.14:+.6f}|{f['daily_turnover']:.4f}|{f['annual_cost']:.2%}|")
main=r['runs']['cap5_open_0.0007']; misses=main['missing_held_quotes']
lines += ['', 'cap8与7.5bp仅为预先骨架已有的口径诊断，不能替代cap5/7bp主基线；close口径是在日收盘形成信号并假定该收盘成交，存在执行理想化。所有诊断均未复现参考值。', '', f"缺价持仓共 {len(misses)} 日，最大陈旧持仓名义/权益 {max(x['stale_notional_equity'] for x in misses):.2%}；涉及 {sorted(set(c for x in misses for c in x['coins']))}。遵循源码无报价不平仓，以最后报价盯市；这会漏掉退市损失及资金费，是局限，不是实际可成交价格。", '', '与参考差异不能唯一归因：原1.62/2.14准确脚本、配置、历史数据快照及收益序列未确定。旧协议1.62是旧每日策略叙述；已有distribution_stagger报告也显示cap8/1倍与cap5/2.7倍等口径不同。本次确认的模型差异包括cap、杠杆、窗口、持仓漂移、成本、缺价与样本覆盖；没有证据给每项分配解释份额。数值差距不小，且无已识别等价配置，门槛不通过。', '', '## 9组 × 2模式结果', '', 'N/A表示未执行，绝不是零、p=1或检验失败。', '', '|激活|回撤|模式|Sharpe|Δ基线|2023–24 Sharpe/Δ|2025–26 Sharpe/Δ|p|p_fwer|悲观–乐观区间|', '|---|---|---|---|---|---|---|---|---|---|']
for a in [10,20,30]:
    for b in [5,10,15]:
        for mode in ['逐币','全仓']: lines.append(f'|{a}%|{b}%|{mode}|N/A|N/A|N/A|N/A|N/A|N/A|N/A|')
lines += ['', '## 五项协议、路径与小时验证', '', '1. 相位/起点稳健性：未执行候选，噪声N/A。每日调仓slices=1只有一个日相位，但执行小时和监控时点仍需验证；不能把零个相位差当作已通过。', '2. 重叠样本：未执行块bootstrap；p=N/A，未使用i.i.d. t检验。', '3. 真实成交：基线主动成交按单边7bp；未模拟盘口、延迟、强平及订单拒绝。没有maker碰价成交假设。', '4. 多重检验：预设18候选（9×2），实际0；p_fwer=N/A。路径两端及实际额外搜索应纳入预先冻结统计方案，不能只择优报告。', '5. 成本与子区间：基线子区间、换手及权益年化成本见上表；候选净收益、相关性和7.5bp压力结果均N/A。协议maker1.5bp只适用于经验证的maker执行，不能用于主动止盈。', '', '悲观/乐观两端均未执行，区间N/A。若后续基线门槛通过，逐币需枚举同日先触发与后触发路径，结论翻转即不显著；触发后至下一次调仓不重入，平仓和重开分别付单边7bp。', '', '全仓组合峰值本次未定义并计算，不把逐币日高低拼接成同步组合峰值。后续应先冻结定义：例如以同步观测价格计算固定持仓的组合权益峰值；日/小时收盘峰值只代表离散监控策略，会漏掉内部峰值。OHLC可提供极宽可行界限，不能恢复真实同步路径，不得冒充连续止盈。', '', f"小时数据原始并集 {audit['hourly']['start']} 至 {audit['hourly']['end']}，不是每币完整共同覆盖。基线门槛失败，真实触发验证未执行。即使小时数据齐全，其OHLC仍不能识别小时内部顺序；短样本只能检验同期同步小时观测下的触发频次、成本和实现差异，不能证明2023–24及全期改善或替代FWER检验。", '', '## 明确结论与局限', '', '**基线复现不出来。** 四条可用条件均未得到候选检验结果，不判可用，也不把未测试当作不可用。按预设停止规则交付此结论。', '', '数据现已可读，剩余障碍是参考基线的等价可复现口径，以及缺价/退市、当前元数据替代历史元数据、资金费、成交时点、保证金强平等建模限制。仓位初始化使用大账户使最小订单影响很小，不能推广到小账户。输出收益NPZ、逐阶段JSON、日志与输入哈希已落盘。未修改src/，未commit/push/部署或操作实盘；工作区已有src/live.rs修改不是本次所做。']
(D/'REPORT2.md').write_text('\n'.join(lines)+'\n')
print(json.dumps({k:{z:v for z,v in x.items() if z!='files_manifest'} for k,x in audit.items() if isinstance(x,dict)},ensure_ascii=False))
