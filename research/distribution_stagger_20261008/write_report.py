from pathlib import Path
import json,numpy as np,pandas as pd
OUT=Path(__file__).resolve().parent
x=json.loads((OUT/'results.json').read_text());h=json.loads((OUT/'hourly_results.json').read_text());audit=json.loads((OUT/'audit.json').read_text())
for f,obj in [('returns.npz',x),('hourly_returns.npz',h)]:
 data=np.load(OUT/f,allow_pickle=True);r={z:data[z] for z in data.files};r['dates']=np.asarray(r['dates'],dtype='U');np.savez_compressed(OUT/f,**r)
 masks={'full':np.ones(len(r['dates']),bool),'2023-24':np.array([s[:4]<='2024' for s in r['dates']]),'2025-26':np.array([s[:4]>='2025' for s in r['dates']])}
 for z in obj['summary']:
  if f.startswith('hourly'):
   obj['summary'][z]['correlation_baseline']=float(np.corrcoef(r[z].mean(axis=0),r['baseline'].mean(axis=0))[0,1]);obj['summary'][z]['correlation_same_phase']=float(np.mean([np.corrcoef(r[z][o],r['baseline'][o])[0,1] for o in range(3)]))
  else:
   for p,m in masks.items():
    if len(r[z])==3:obj['summary'][z][p]['correlation_same_phase']=float(np.mean([np.corrcoef(r[z][o,m],r['baseline'][o,m])[0,1] for o in range(3)]))
(OUT/'results.json').write_text(json.dumps(x,indent=2));(OUT/'hourly_results.json').write_text(json.dumps(h,indent=2))
pd.DataFrame([dict(strategy=z,period=p,**v) for z,q in x['summary'].items() for p,v in q.items()]).to_csv(OUT/'metrics.csv',index=False)
pd.DataFrame([dict(strategy=z,**v) for z,v in h['summary'].items()]).to_csv(OUT/'hourly_metrics.csv',index=False)
s=x['summary'];b=s['baseline'];besth=max((z for z in h['summary'] if z!='baseline'),key=lambda z:h['summary'][z]['sharpe'])
lines=['# 分布因子与错峰细化：没有合格候选','',
'2026-10-08。本次共预声明 44 个配置：12 个分布因子方案（6 个固定方向，各自单因子/20%混合）、8 个错峰方案、24 个 UTC 小时（包含 hour0 控制配置；23 个非平凡小时比较）。没有一个通过五项检验。没有修改实盘代码。','',
'## 最重要的前提：基线没有复现为 1.79','',
f"读取 validation-protocol.md 和 trader.rs 后按当前代码建模，234 个日线文件，{x['meta']['start']} 至 {x['meta']['end']}，两个区间分别 506 / 641 个交易日。历史液态宇宙均值 {x['meta']['universe_mean']:.1f}、中位 {x['meta']['universe_median']:.0f}，不是把当前约44币倒灌到历史。",
'',f"当前 cap5、2.7x 实际目标名义、全3相位基线 Sharpe={b['full']['sharpe']:.3f}；相位分别 {b['full']['phase_sharpes']}，标准差 {b['full']['phase_sd']:.3f}。2023–24 相位 Sharpe={b['2023-24']['phase_sharpes']}。这是对基线本身的重要稳健性疑点，不能将某个起点的结果当成全相位效果。",
'',f"只做口径诊断：cap5 / 1x 相位平均={audit['cap5_lev1']['sharpe']:.3f}；cap8 / 1x 相位平均={audit['cap8_lev1']['sharpe']:.3f}，而后者单一相位0={audit['cap8_lev1']['phases'][0]:.3f}。单相位能得到约1.79，不证明历史1.79就是这么来的：原始基线脚本/账户规模/样本口径未提供，不能推定出处。所有主比较只对本次相同口径基线进行，不能声称已验证超过正式基线1.79。与前一研究保存的数据文件哈希相比，日线变化数量为0。",
'', '## 成本上界：还有有限空间，但没有找到能兑现的错峰改进','',
'换手必须说明分母。这里逐笔买卖名义绝对额 / 权益是 one-way traded notional；传统换手还常用 Σ|Δw|/2。下表主要用“半双边换手 / 2.7x目标总名义”，以便对照约13%的描述。当前实际成交总名义 / 权益约74%/日，半双边换手 / 目标总名义约13.7%。成本按逐笔交易总名义收取，不把半换手直接乘单边费率。',
'',f"基线 taker+实测滑点=7.5bp/交易名义，权益年化成本 {b['full']['annual_cost']:.2%}；除以2.7后单位目标名义成本 {b['full']['annual_cost_unit_gross']:.2%}，与7.3%的原描述接近。1x口径回测成本 {audit['cap5_lev1']['annual_cost']:.2%}。这是单位差异，不是凭空多收三倍手续费。",
'', '| 固定基线毛收益路径、只替换成本 | 全期Sharpe | 2023–24 | 2025–26 | 权益年成本 |','|---|---:|---:|---:|---:|']
lines.append(f"| 当前3档 | {b['full']['sharpe']:.3f} | {b['2023-24']['sharpe']:.3f} | {b['2025-26']['sharpe']:.3f} | {b['full']['annual_cost']:.2%} |")
labels={'5pct_half_gross':'5% 半双边换手 / 总名义（主要口径）','5pct_unit_gross':'5% 成交总额 / 总名义','5pct_equity':'5% 成交总额 / 权益','zero_cost':'零手续费与滑点'}
for z,q in x['upper'].items():lines.append(f"| {labels[z]} | {q['full']['sharpe']:.3f} | {q['2023-24']['sharpe']:.3f} | {q['2025-26']['sharpe']:.3f} | {q['full']['annual_cost']:.2%} |")
lines += ['', '这是固定实际收益路径的成本节省反事实：保留已实现的价格损益和风险，取消实际费用后替换为恒定目标换手费用，不模拟换手下降后持仓质量变化，也不重新计算免手续费下的权益反馈。因此它是“只靠省成本”的条件上界/空间估计，不是任意新调仓策略的数学Sharpe上界。主要5%口径提高约0.161 Sharpe，零成本提高约0.254。成本仍有空间，不能诚实地说“已经用完”；但空间有限，需显著降换手且不损害毛收益。5/6档的毛收益损失已吃掉部分节约，不能将上界当成可实现业绩。', '', '## 日频所有候选：相位平均结果', '', '下表ρ为各方案相位平均日收益与基线相位平均日收益的相关性。JSON/CSV另列3相位方案的同相位相关性。Sharpe使用各相位Sharpe的算术平均，绝不使用平均收益序列的Sharpe。p为30日块的全家族FWER，所有日频方案15/30/60日块全期p也均为1.000。', '', '| 方案 | 全期S | 2023–24 | 2025–26 | 相位SD | 半换手/总名义 | 权益年成本 | ρ | pFWER |','|---|---:|---:|---:|---:|---:|---:|---:|---:|']
for z,q in s.items():
 v=q['full'];p=x['bootstrap']['full']['30'].get(z,{}).get('p_fwer_global');lines.append(f"| {z} | {v['sharpe']:.3f} | {q['2023-24']['sharpe']:.3f} | {q['2025-26']['sharpe']:.3f} | {v['phase_sd']:.3f} | {v['daily_half_turnover_unit_gross']:.2%} | {v['annual_cost']:.2%} | {v['correlation_baseline']:.3f} | {p if p is not None else '控制'} |")
lines += ['', '偏度/峰度使用pandas20日样本偏度与超额峰度，要求20个完整收益；MAX是过去20日最大简单日收益；协偏度为 E[(ri−μi)(rm−μm)²]/(σiσm²)，市场分别BTC与当时流动宇宙等权。方向固定为低偏度、低协偏度、低MAX；峰度方向没有确定先验，因此正反两向都计入检验。没有测试其他窗口、混合比例、因子替代或剔除指定币。每个单因子各自top/bottom等权，再20%与基线合并并归一化；未成熟窗口不填充。', '', '最漂亮的20%低峰度混合，两子区间都改善，但全期ΔS=0.130，30日块95%区间[-0.108, 0.370]，日频家族内部p=0.803，全家族p=1.000。这是不显著的点估计，不能推荐。全部相关性在文件中可查，高相关混合更不能被当作独立新边际。', '', 'uniform_5/6全期提高，但2025–26从2.327降到2.289/2.278；6档全期相位SD=0.494，range=1.286。非均匀4方案均未同时改善两段。波动/换手按横截面三分位分配，低值币更慢，高值币更快；退出液态宇宙的币保留最近周期，不能借此偷偷每天退出。均匀N档枚举N相位；2/3/4混合枚举lcm=12相位；1/3/6混合枚举6相位。高换手更多档在此明确实现为更高调仓频率，而不是同时运行多个独立子账户。', '', '## UTC调仓小时：没有显著最佳小时', '',f"小时线使用77个在所有执行时点完整可见的币的限制宇宙，{h['meta']['start']} 至 {h['meta']['end']}，206天。信号仍是上个UTC日已收盘的日线，只改变次日执行小时；不是重新定义日线收盘，也没有测试其他日历标签。所有24小时、所有3相位都计算；不存在日线价格代理缺失小时，未持有缺价仓位。这个限制宇宙有当前可见样本选择偏差，不能外推到完整历史实盘宇宙。",
'',f"控制hour0平均S={h['summary']['baseline']['sharpe']:.3f}；事后最佳UTC15点S={h['summary'][besth]['sharpe']:.3f}，ΔS={h['bootstrap']['30'][besth]['delta_sharpe']:.3f}，30日块区间={h['bootstrap']['30'][besth]['ci95']}，小时家族内部p={h['bootstrap']['30'][besth]['p_fwer_within']:.4f}，全家族p=1.000。全部小时校正后都不显著。缺失2023–24整段，五项标准第5项必定不合格。",
'', '## 五项门槛与成交模型', '', '1. 全相位枚举并报告SD/range。没有预先指定允许的SD阈值，因此不另挑阈值筛选；5/6档明显离散，且其他门槛已失败。基线本身的相位敏感性必须记录。', '2. 每日组合收益使用9999次配对环形移动块bootstrap，块长15/30/60日，所有相位与候选共用日期重采样。没有事件逐笔朴素t检验。', '3. 主模型次日开盘/指定小时开盘，以taker4.5bp+滑点3bp扣每笔买卖名义；不是OHLC触价即maker成交。开盘价是执行基准代理，不能保证每笔IOC同价成交。maker1.5bp+滑点3bp仅保存为乐观费用敏感性，不用于通过检验：未建盘口队列/取消/条件成交后的不利价格变化，无法识别成交的那批好坏。', '4. 用Sharpe影响函数对配对差值进行studentized maxT，日频20个检验与小时24个检验分别maxT，再Bonferroni×2控制联合FWER。影响函数是渐近近似，不是有限样本精确检验。全期和两个子区间分别计算，不能用全期显著替代两段改善。块长敏感性不是新增参数筛选；所有结果都保存。控制hour0零差也纳入，保守计数44。', '5. 报告两段Sharpe、净算术年化收益/CAGR、换手/成本、与基线相关性，均保存CSV/JSON；6bp滑点压力、缺价仓位额外5%不利退出、maker乐观费用也保存。没有合格候选。', '', '## 数据与执行限制（不能隐去）', '', '资金费率缺乏完整2023–26覆盖，主收益未包含资金费。已有HL费用数据目录起点和币数见audit.json，不能把缺失费率默认为0并宣称真实净收益。大账户近似忽略$10最小订单、$15最小持仓和数量精度，保留2%调仓带；没有提供账户权益，无法精确复现小账户的收缩cap。价格和权益漂移保留，不在每天把未轮到的币重新归一化，因此实际总/净敞口可漂移；见mean_gross / mean_abs_net。', '',f"缺失持仓价格采用最后可见标记强制退出，这是乐观回测假设；基线单次最大缺价敞口为权益{x['meta']['missing']['baseline']['max_equity_exposure']:.2%}，各相位事件数{x['meta']['missing']['baseline']['events']}。额外5%不利退出压力不能替代真实退市成交数据。终点统一taker清仓计费，未模拟保证金强平/订单拒绝/盘口容量；结果不能用于部署认证。", '', '## 重现与产物', '', '```bash', 'cd /Users/wonder/Code/stars', 'OPENBLAS_NUM_THREADS=1 python research/distribution_stagger_20261008/run.py', 'OPENBLAS_NUM_THREADS=1 python research/distribution_stagger_20261008/hourly.py', 'OPENBLAS_NUM_THREADS=1 python research/distribution_stagger_20261008/audit.py', 'python research/distribution_stagger_20261008/write_report.py', '```', '', 'engine.py共享装载/精确信号/漂移回测；run.py日频候选、成本反事实、FWER与压力；hourly.py完整小时报价子集；results.json/hourly_results.json完整指标、p值与区间；metrics.csv/hourly_metrics.csv便于审计；returns.npz/hourly_returns.npz每相位日收益；audit.json口径诊断/资金费/本地数据库覆盖；results.json含234输入文件SHA256。已完成目标中性、有限收益、未破产、日线连续及小时无缺价敞口断言。', '', '**最终判断：没有。成本并未完全失去改善空间，但本次高阶矩、调仓小时和错峰细化均没有提供通过五项标准的新边际。现有1.79基线应先用原始脚本核对相位平均与成本分母；不能把本次点估计包装为可上线改进。**']
(OUT/'REPORT.md').write_text('\n'.join(lines)+'\n')
print('Report written',OUT/'REPORT.md')
