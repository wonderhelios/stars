"""Report generated from saved results, without result-dependent thresholds."""
import json,hashlib
from pathlib import Path
import numpy as np
import pandas as pd
from analyze import OUT,NAMES,PAIRS,FEES,load
read=lambda f:json.loads((OUT/f).read_text())
M=read('metrics.json');C=read('contrasts.json');B=read('bootstrap.json');G=read('gates.json');V=read('verification.json');D=read('drawdown_diagnostics.json');R=read('g_relation.json');COR=read('correlations.json')
lines=[]
def add(s):lines.append(s)
def f(x):return '—' if x is None else f'{x:.4f}'
def pct(x):return '—' if x is None else f'{100*x:.2f}%'
def tab(headers,rows):add('\n| '+' | '.join(headers)+' |\n| '+' | '.join(['---']*len(headers))+' |\n'+'\n'.join('| '+' | '.join(map(str,r))+' |' for r in rows)+'\n')
def avg(x,case,p,k):return np.mean([z[p][k] for z in M[x][case]])
add('''# K12 严格验证与实验 A

研究日期：2026-10-11。全部收益口径：**净交易成本、资金费未计**。仅研究；未修改 src/ 或原 baseline.py，未 commit/push/部署，未操作实盘。

## 1. 结论与未通过的门槛

**K12 未通过，当前无可晋级的 Sharpe 改进。** K12全期、两段点估计均提高，换手下降，但三个块长FWER均未通过。额外延迟一日后，2025–26相对B0的提升翻为负，属于执行敏感，不能称稳健。

**A 未晋级，也不能声称比 K12 更强。** A对B0及A_const的样本内ΔSharpe为正，但全部三个FWER均大于0.05；A对K12的主档全期及两段差值均为负。结论：**保留抵消现金可降风险、未确认 Sharpe 择时价值**。风险下降是本样本描述，不是另一个已通过统计检验的晋级标准。
''')
labels={'positive_delta':'净ΔS>0','all_three_fwer':'三档FWER<0.05','both_segments_positive_and_improved':'两段S正且提高','both_segments_improved_at_7_5bp':'7.5bp两段提高','startup_mean_exceeds_range':'启动平均Δ>极差','turnover_budget':'③换手≤B0的110%'}
tab(['比较']+list(labels.values())+['结论'],[[pair]+['通过' if v['checks'][k] else '**未过**' for k in labels]+['未晋级' if pair in ['K12-B0','A-B0'] else '未确认'] for pair,v in G.items()])
add('A−A_const的等gross匹配门槛通过，机制归因的正差显著性门槛未过。A−K12还未过主档正差、分段改善、7.5bp分段改善和启动平均正提升门槛。没有择较小p、挑块长或调整门槛。')
add('''## 2. 冻结输入、复现与统计登记

178币缓存、原request/meta及源码hash均已冻结。加载2023-06-01至2026-10-07，沿用 T<end_ms；原收益为2023-07-04至2026-10-07（1192日）。使用指定Python，在 /Users/wonder/Code/stars 运行。

AST截取原引擎定义并重定向OUT，未import原文件。B0保留cap5/20%、至少8币开关、1e6资本、每日完整调仓、2.7目标gross、取整、$10、2%带和缺价处理。反手按绝对数量差计两边费用，无资金费、现金利息或终点清仓。K12仅改 kk=min(12,len(idx)//2)；A仅移除末尾w/g。
''')
tab(['路径','记录值','本次原起点','与记录值差异','参考逐日r/t/turn'],[[x,h,f'{V["reference"][x]["sharpe"]:.10f}',f'{V["reference"][x]["sharpe"]-float(h):+.10f}','完全相等'] for x,h in [('B0','1.1293'),('K12','1.6411')]])
add('''1.6411是K12的记录，不是第二个B0。差异仅来自四位小数舍入。178份数据及共享request/meta/底座/协议等182个历史manifest条目全部无变化，没有换口径追小数。

启动1/2在后移1/2日从现金开始。主统计使用三个启动的共同连续日期2023-07-06至2026-10-07（1190日），先分别算各启动Sharpe差再取平均；不把启动当3倍样本，不先平均收益再算Sharpe。与原起点数字的差异来自共同日期删去前两日及初始订单差异。

原报告将A/B/C的9个比较登记在同一家族；保留该登记并加入本次K12−B0，合计**8路径、10比较**。B/B_sign/C/C_diag只补算7bp的三个启动以保留maxT家族，不作新推荐。没有使用只含4项的较小家族。完整登记见 [SPEC.md](SPEC.md)。

循环移动块10/20/40日，各9999次，各档初始化seed=20261010。所有路径/启动共用抽样日索引，直接重算非线性Sharpe差；bootstrap差减观测差居中，除以该比较bootstrap标准差，取10比较的单侧maxT。p=(1+超越次数)/10000。沿用已有研究的bootstrap-SE studentization，未改成均值差或iid检验。每500次原子落盘并保存RNG状态。

## 3. 主档完整结果：原起点与连续日历区间

以下为启动0、7bp。Δ均同日配对。净Sharpe=mean(r)/std(r,ddof=1)×sqrt(365)，算术年化=365mean(r)，CAGR按区间日收益复利。现金日全部保留。
''')
for period,title in [('full','全期'),('2023-24','2023–24'),('2025-26','2025–26'),('since2024','2024-01-01起连续日历区间')]:
 ms={x:M[x]['f0.0007_d0'][0][period] for x in NAMES[:4]};b=ms['B0']
 add(f'### {title}（{b["start"]}至{b["end"]}，{b["days"]}日）')
 tab(['路径','净S','ΔB0','ΔK12','ΔA_const','算术年化','CAGR','日波动','最大回撤'],[[x,f(v['sharpe'])]+[f(v['sharpe']-ms[y]['sharpe']) for y in ['B0','K12','A_const']]+[pct(v[z]) for z in ['annual_arithmetic','cagr','daily_vol','max_drawdown']] for x,v in ms.items()])
 rows=[]
 for x,v in ms.items():
  a=load(x);bb=load('B0');dates=pd.to_datetime(a['t'],unit='ms',utc=True);mask=(dates>=pd.Timestamp(v['start'],tz='UTC'))&(dates<=pd.Timestamp(v['end'],tz='UTC'));cor=np.corrcoef(a['r'][mask],bb['r'][mask])[0,1]
  rows.append([x,f(v['daily_turnover']),pct(v['annual_cost']),f(v['gross_mean']),f(v['gross_peak']),f(v['neff_active_mean'])+' ['+f(v['neff_min'])+','+f(v['neff_max'])+']',v['cash_days'],f(cor)])
 tab(['路径','日换手/权益','年成本','平均gross','峰值gross','N_eff均值 [范围]','现金日','与B0收益相关'],rows)
 tab(['路径','峰值日','谷底日','恢复日','下跌天数','谷后恢复天数','该事件水下天数','最长水下天数'],[[x,v['peak'],v['trough'],v['recovery'] or '未恢复',v['decline_days'],v['recovery_days'] if v['recovery_days'] is not None else '未恢复',str(v['underwater_days'])+('（截至期末）' if v['censored'] else ''),v['longest_underwater_days']] for x,v in ms.items()])
add('''实际gross=成交后绝对名义/扣费权益，故峰值可略高于2.7；目标gross仍遵守定义。换手=成交金额/盯市后扣费前权益；年成本=平均日换手×费率×365。实际美元费用另存，不把它除初始本金冒充年成本。N_eff=1/Σ(|w|/gross)^2，按实际标记持仓计算，现金日无定义；表中N_eff均值仅非现金日，但所有收益指标保留现金日。区间回撤含区间初始净值，未恢复事件明确右截尾。

### 共同日期启动敏感性与分段门槛
''')
rows=[]
for x in NAMES[:4]:
 a=M[x]['f0.0007_d0'];d=[z['common']['sharpe']-b['common']['sharpe'] for z,b in zip(a,M['B0']['f0.0007_d0'])]
 rows.append([x]+[f(z['common']['sharpe']) for z in a]+[f(np.mean([z['common']['sharpe'] for z in a])),f(np.mean(d)),f(np.ptp(d))]+[f(avg(x,'f0.0007_d0',p,'sharpe'))+' / '+f(avg(x,'f0.0007_d0',p,'sharpe')-avg('B0','f0.0007_d0',p,'sharpe')) for p in ['common_2023-24','common_2025-26']])
tab(['路径','启动0 S','启动1 S','启动2 S','平均S','平均ΔB0','配对Δ极差','2023–24 S/ΔB0','2025–26 S/ΔB0'],rows)
tab(['预定比较','平均ΔS','启动Δ极差','2023–24 ΔS','2025–26 ΔS'],[[x,f(C[x]['f0.0007_d0']['common']['delta_mean']),f(C[x]['f0.0007_d0']['common']['delta_range'])]+[f(C[x]['f0.0007_d0'][p]['delta_mean']) for p in ['common_2023-24','common_2025-26']] for x in G])
add('所有路径/成本/启动/区间完整指标：[metrics.csv](metrics.csv)、[metrics.json](metrics.json)；全部成对差值：[contrasts.json](contrasts.json)。')
add('## 4. 配对ΔSharpe的studentized maxT / FWER')
tab(['注册比较','平均ΔS','SE L20','FWER L10','FWER L20','FWER L40','L20单比较95%区间'],[[pair,f(B['20']['observed_delta'][j]),f(B['20']['se'][j])]+[f(B[L]['p_fwer'][j]) for L in ['10','20','40']]+['['+f(B['20']['basic_ci95_low'][j])+', '+f(B['20']['basic_ci95_high'][j])+']'] for j,pair in enumerate(B['20']['comparisons'])])
add('区间是单比较basic bootstrap区间，**不是同时置信区间**，不能代替FWER门槛。完整结果见 [bootstrap.json](bootstrap.json)。C/C_diag的2024有效覆盖'+pct(V['extra_coverage']['C']['2024']['coverage'])+'，2025–26只有'+pct(V['extra_coverage']['C']['2025-26']['coverage'])+'，按原规格覆盖不足；未缩短窗口救结果。B/C只保留统计登记，不另作推荐。')
add('''## 5. 成本与额外执行延迟

每成本/启动完整重跑权益和订单，A_const逐情景纯gross匹配。下表为共同日期三个启动平均；早段545日、后段645日。1.5/4.5bp仅成本下界，不赋予maker可实现成交资格。7.5bp=4.5+3，10.5bp=4.5+6。
''')
for delay in [0,1]:
 add('### 正常执行' if delay==0 else '### 额外延迟一日：信号i−2，在i开盘执行')
 rows=[]
 for fee in (FEES if delay==0 else FEES[:3]):
  case=f'f{fee}_d{delay}'
  for x in NAMES[:4]:
   row=[f'{fee*10000:g}',x]+[f(avg(x,case,p,'sharpe'))+' / '+f(avg(x,case,p,'sharpe')-avg('B0',case,p,'sharpe')) for p in ['common','common_2023-24','common_2025-26']]
   if delay==0:row += [pct(avg(x,case,'common','annual_arithmetic')),pct(avg(x,case,'common','cagr')),f(avg(x,case,'common','daily_turnover')),pct(avg(x,case,'common','annual_cost'))]
   rows.append(row)
 tab(['单边bp','路径','全期S / ΔB0','2023–24 S / ΔB0','2025–26 S / ΔB0']+(['算术年化','CAGR','日换手','年成本'] if delay==0 else []),rows)
add('''K12延迟后的后段相对优势翻负，不能称稳健；10.5bp正常执行下两段差值仍正。A对B0/A_const在所测高成本及延迟情景的相对Δ未翻负；但延迟且7.5/10.5bp时A早段绝对Sharpe已转负，不能泛称执行稳健。压力点估计不能补救主检验失败。部分压力情景A胜过K12不能替换主档比较。延迟不是实盘00:05精确模型或严格收益下界。

## 6. A的固定倍率与机制诊断

A_const=原归一化目标×固定m。只用每条路径全期成交后平均gross迭代匹配A，子区间不重新拟合。这是事后归因控制，不是可部署信号。误差=(A_const平均gross/A平均gross−1)，所有情景均小于0.5%。
''')
tab(['情景','m','相对gross误差'],[[v['path'],f'{v["m"]:.9f}',f'{100*v["relative_error"]:.6f}%'] for v in sorted(V['gross_matches'],key=lambda x:x['path'])])
add('''### 固定回撤窗口与其后恢复

固定2024-03-05至2024-10-15，包含端点当天收益。恢复段从10-16起，到各路径净值首次回到其03-05成交后净值；未恢复则截至期末。各候选真正最大回撤峰谷见第3节，不与本固定诊断窗口混用。价格PnL及成本除各段期初权益；每日CSV同时保存美元累计值、gross和净值。
''')
rows=[]
for x in NAMES[:4]:
 for p in ['fixed_drawdown','recovery']:
  v=D[x][p];rows.append([x,'固定回撤' if p=='fixed_drawdown' else '其后恢复',v['start']+' ~ '+v['end'],f(v['gross_mean']),pct(v['price_pnl_per_start']),pct(v['cost_per_start']),pct(v['net_return']),f'{v["starting_equity"]:,.0f} → {v["ending_equity"]:,.0f}'])
tab(['路径','阶段','区间','平均gross','累计价格PnL/期初','累计成本/期初','净收益','起止权益$'],rows)
add('''逐日分解：[B0](drawdown_recovery_B0.csv)、[K12](drawdown_recovery_K12.csv)、[A](drawdown_recovery_A.csv)、[A_const](drawdown_recovery_A_const.csv)。降名义同时影响亏损、反弹与费用，窗口标签不参与信号。

### g与下一持有日毛收益/平方收益

g在信号日结束已知，次日open执行，配对执行后下一open的价格PnL/期初权益。最后一日无下一期而不纳入。并列A及未缩放B0，避免把机械缩放当作预测能力。非零g子集仅描述，正式绩效及bootstrap不裁剪现金日。
''')
tab(['描述样本','日数','g均值','corr(g,A毛收益)','corr(g,A平方收益)','corr(g,B0毛收益)','corr(g,B0平方收益)'],[[label,v['n'],f(v['g_mean'])]+[f(v[k]) for k in ['corr_A_gross','corr_A_squared','corr_B0_gross','corr_B0_squared']] for label,v in R.items()])
add('相关关系不证明条件Sharpe可预测，也不构成新增g门槛。逐日数据见 [g_next_return.csv](g_next_return.csv)。')
add('### 收益相关性')
tab(['共同全期']+NAMES[:4],[[x]+[f(COR['periods']['common'][NAMES.index(x)][NAMES.index(y)]) for y in NAMES[:4]] for x in NAMES[:4]])
add('以上为共同日期启动平均日收益相关，各段矩阵另存CSV。A与B0相关性超过0.95，属于同策略风险覆盖/构造微调，不是独立新因子。')
add('## 7. 数据依赖、执行审计与复跑')
add(f"完成{V['ledger_paths']}条路径的独立数量递推、美元PnL/费用、权益守恒、目标gross及回撤核验。8条主路径的无诊断原函数运行与加诊断版本逐日完全相等。B0及K12与历史r/t/turn逐元素相等。bootstrap计数充分统计量与直接抽样重算最大误差{max(x['direct_check_max_error'] for x in B.values()):.2e}。")
add('108条路径均无持仓缺价事件，实际持仓/订单当日均未依赖零成交bar。但**信号历史窗口存在零成交依赖**，不能据此声称全历史成交已认证：')
tab(['启动0主档路径','目标涉及坏bar的窗口日数','相关币×窗口数','合格集合涉及坏bar的窗口日数'],[[v['name'],v['target_signal_window_bad_days'],v['target_bad_coin_windows'],v['eligible_signal_window_bad_days']] for v in V['signal_zero_trade_dependence'] if v['name'] in NAMES[:4]])
add('''坏bar为v不正，或n字段存在且n不正；审计窗口取V30、SD20收益端点、M14端点及Q的联合t−30..t。按原指标实际读取的日历窗口核查，未因此修改资格或剔除日期。该计数是保守依赖，不等于每根bar都有非零边际影响；合格集合统计还包含零目标日及未被最终选中的名字。完整日期/币名与订单审计见 [verification.json](verification.json)、[ledger_audit.csv](ledger_audit.csv)。

冻结输入hash见 [input_manifest.json](input_manifest.json)，脚本及结果hash见 [implementation_manifest.json](implementation_manifest.json)。每日NPZ保存数量、delta数量、原始/归一化/实际目标、g、执行前后gross、PnL、费用及权益，对应CSV可审阅；各路径单独落盘，长bootstrap可续跑。

开发核验曾发现辅助C求解器L1约束常数的实现符号错误，造成全部回退；修正后把旧结果隔离到 audit_initial_solver_bug/，重新生成受影响的目标、路径、指标和所有bootstrap，没有沿用错误统计。指定Python不能加载新装SciPy的签名扩展，最终使用纯NumPy凸二次规划内点求解；未更改窗口、协方差、约束或回退规则，失败日志保留。
''')
add(f"最终C/C_diag可用窗口无求解失败，最大约束残差{V['max_C_constraint_residual']:.2e}；另用有解析解的凸二次规划核验求解器。该对照的覆盖不足仍保留为原规格失败。")
add('复跑必须从 /Users/wonder/Code/stars 执行；按以下顺序，现有完整路径与bootstrap检查点会恢复，不重新下载数据：')
add('```sh\n'+'\n'.join('OPENBLAS_NUM_THREADS=1 VECLIB_MAXIMUM_THREADS=1 /Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/k12_validation_20261010/'+s for s in ['extra_targets.py','engine.py','analyze.py','verify.py','report.py'])+'\n```')
add('''## 8. 局限

- 资金费未计，不能称完整可实现净收益；未将缺失资金费填0后声称通过。任何未来正面结论仍需资金费/执行核验及未来冻结样本。
- 日open代理与固定成本不能认证逐币容量、冲击及00:05执行，K12已有延迟敏感。当前meta和可得币缓存仍有历史精度、退市与币池选择偏差；信号窗口零成交依赖另列，未擅改底座美化数据。
- 本批FWER不能消除130+历史搜索及已看过窗口的选择偏差。循环块不能消除非平稳性和尾部稀少问题；三个相邻启动不是独立市场重复。
- A_const用全期gross校准，是事后控制；bootstrap内不重新拟合m，检验对冻结回测路径/校准条件化，不是未来可部署倍率的置信保证。
- N_eff是名义集中度，不是独立风险因子数。回撤改善是路径极值描述，未另证实其统计显著性。
- A的FWER接近但仍高于0.05。蒙特卡洛误差不构成追加次数、挑seed或放宽门槛的理由；所有否决保留。
''')
(OUT/'REPORT.md').write_text('\n\n'.join(line.strip() for line in lines)+'\n')
paths=[p for p in OUT.iterdir() if p.is_file() and p.suffix in ['.py','.md','.json','.npz','.csv'] and p.name!='implementation_manifest.json' and not p.name.endswith('_checkpoint.npz')]
(OUT/'implementation_manifest.json').write_text(json.dumps({p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(paths)},indent=2))
