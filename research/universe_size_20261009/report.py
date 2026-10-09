import json, hashlib
from pathlib import Path
import numpy as np
import pandas as pd
O=Path('research/universe_size_20261009')
R=json.loads((O/'results.json').read_text());D=json.loads((O/'periods.json').read_text());V=json.loads((O/'verification.json').read_text());B=R[0]
f=lambda x:f'{x:.4f}'
pct=lambda x:f'{x*100:.2f}%'
yes=lambda x:'✓' if x else '✗'
label=lambda n:'基线' if n=='baseline' else n
lines=[]
def add(s=''):lines.append(s)
def table(headers,rows):
 add()
 add('| '+' | '.join(headers)+' |');add('|'+'|'.join(['---']*len(headers))+'|')
 for row in rows:add('| '+' | '.join(map(str,row))+' |')
 add()
add('# 显式持仓名额与流动性宇宙上限实验')
add('\n研究日期：2026-10-10。仅研究产物；未改动 src，未 commit/push/部署，未操作实盘。')
add('\n## 结论')
add('\n**没有候选通过“可用”四条；6 个候选达到事先固定的“风险显著降低”标准。** 所有收益改进均未通过 9 候选 FWER；最小主检验 p_fwer=0.2934，不能把样本内改善当成已确认收益优势。风险标准是“最大回撤改善至少10个百分点且Sharpe下降不超过0.15”，这里的“显著”是该业务判定，不是回撤改善的统计显著性。')
add('\n**以风险为重点，优先保留 K=12 作为后续独立验证对象。** 原始完整样本 Sharpe 1.6411（基线1.1293），最大回撤 −55.95%（基线−80.90%），最大回撤事件的水下总时长193天（基线609天），谷底后115天恢复（基线385天）。共同日期启动平均 Sharpe 1.6251、日换手1.4029，比配对基线低12.17%；年化成本35.84%，比基线低4.97个百分点。代价是后期实际净持仓由约17.97增至27.43个币；2025–26算术年化净利从基线165.54%降至149.84%，CAGR从278.24%降至263.39%，即后期收益也有牺牲；且仍有约56%的深回撤；p_fwer=0.2974，证据不足以判“可用”。')
add('\n**N50_K12 是本次样本内 Sharpe 数值第一**：共同日期1.6288，比K12只高0.0036，回撤和恢复日期相同。N=50只在33/1192天生效，没有足够证据说明加这一层有稳定价值。若更重视换手与恢复速度，N80_K15可保留为次选：Sharpe1.6107、日换手1.3622、水下164天；但N=80从未生效，本次它实际等同固定K=15（未额外搜索K15）。')
add('\n这次结果不是“缩小持仓就降低换手”：K3的换手升至1.7005，回撤恶化到−90.76%。增加到K8/12在本样本中同时改善回撤和换手；进一步增至K20，换手继续降低，但Sharpe和2025–26表现回落。它不是已被证明的免费午餐。')
add('\n## 预先规则与复现')
add('\n计算候选前写入 [SPEC.md](SPEC.md)。候选共9个；未按结果追加参数。默认保留V≥500万美元门槛，在合格币中按滞后30日平均成交额V取前N，币名打破并列。N是每日变化的宇宙人数上限，不是固定币名单；不足N时不补低流动性币。其余资格条件、N<8零目标开关保持原样。')
add('\n原 `research/trailing_tp_20261008/baseline.py` 已直接运行，cap5/open/fee0.0007得到 **Sharpe 1.1292998945108697**、日换手1.5976523473、年成本40.8200%、算术年化净利89.9400%。研究基线的1192个每日收益、时间戳、换手与原输出逐元素完全一致。见 [baseline_reproduction.log](baseline_reproduction.log)、[baseline_gate.json](baseline_gate.json)。')
add('\n178个输入文件；为严格复现原策略，读取窗口沿用原request.json：2023-06-01至2026-10-07的完整日线，共1225天。回测净收益是 **2023-07-04至2026-10-07，共1192天**。目录可能覆盖更早/更晚日期，本次没有将它们并入原基线。“2023–24”也仅指该窗口内的2023–24。')
add('\nkk改为 `min(K, len(idx)//2)`；基线仍保留20%/cap5原实现。三个因子各自有多空各kk个名额，之后净合并并归一化，因此**kk不是最终组合每侧的不同币数量，最终持仓也不是2kk**。固定K候选不再应用原cap5或20%；账户规模限额经审计在本实验全部远大于K，未影响对照。')
add('\n执行模型完全沿用原run：上一日信号、次日开盘成交，总绝对敞口2.7倍、10美元最低订单、2%调仓带、当前meta数量精度；无资金费、无末日强平。主成本单边0.07%。年成本=平均日成交名义/成交前净值×0.0007×365；年化净利同时列算术年化（日均净收益×365）和CAGR（逐日复利年化），两者不可混用。日换手1.60代表每日成交名义约为净值的160%，并非160%的币更换率。')
add('\n### 原始完整样本：直接对照1.1293')
add('\n以下均为原起点p0、1192天，Δ对同一行口径基线。')
rows=[]
for x in R:
 m=x['original_full'];base=B['original_full']
 rows.append([label(x['name']),f(m['sharpe']),f(m['sharpe']-base['sharpe']),f(m['daily_turnover']),pct(m['annual_cost']),pct(m['annual_arithmetic']),pct(m['cagr']),pct(m['max_drawdown'])])
table(['变体','Sharpe','Δ基线','日换手','年化成本','年化净利·算术','净利CAGR','最大回撤'],rows)
add('\n## 相位与共同日期主比较')
add('\n日频周期=1，所有日历调仓相位只有一个。另运行0/1/2日启动偏移，全部保持每日调仓；共同评价日期 **2023-07-06至2026-10-07，1190天**。下面取每条实际路径指标的平均，不用合成收益替代单条路径回撤。它是启动敏感性检验，不是3日调仓，也未检验日内执行时刻。')
add('\n共同日期基线Sharpe为1.1176，与完整样本1.1293不是复现误差：为公平配对剔除了7月4日−0.1890%和7月5日+3.5089%的收益，同时各起点订单历史略有区别。基线2023–24 Sharpe从原始+0.0101变为共同日期−0.0186，说明早段“略正”对端点敏感。')
rows=[]
for x in R:
 rows.append([label(x['name']),f(x['sharpe']),f(x['delta']),f(x['daily_turnover']),pct(x['annual_cost']),pct(x['annual_arithmetic']),pct(x['cagr']),pct(x['max_drawdown']),f(x['corr'])])
table(['变体','Sharpe均值','Δ配对基线','日换手','年化成本','年化净利·算术','净利CAGR','最大回撤','与基线收益相关'],rows)
rows=[]
for x in R:
 rows.append([label(x['name'])]+[f(m['sharpe']) for m in x['phase_metrics']]+[f"{x['phase_sd']:.6f}",f"{x['phase_delta_range']:.6f}"])
table(['变体','启动0 Sharpe','启动1 Sharpe','启动2 Sharpe','跨启动标准差','配对Δ极差（噪声门槛）'],rows)
add('\n“改进大于相位噪声”要求正的平均Δ超过最后一列。极差很小仅说明这三个相邻启动稳健，不能替代跨行情样本外验证。')
add('\n## 最大回撤、持续时间与恢复时间')
add('\n回撤从净值历史峰值计算，净值包含初始资本；“下跌”=峰到谷，“恢复”=谷到首次回到原峰，“水下总时长”=峰到恢复；另报全样本最长水下事件。以下日期在三个启动中一致，回撤深度采用共同日期均值。所有表列最大回撤事件均已恢复，没有把未恢复事件填成已恢复。每天使用开盘盯市/交易后净值，不代表日内最大回撤。')
rows=[]
for x in R:
 m=x['phase_metrics'][0]
 rows.append([label(x['name']),pct(x['max_drawdown']),m['peak'],m['trough'],m['recovery'],m['decline_days'],m['recovery_days'],m['underwater_days'],m['longest_underwater']['days']])
table(['变体','最大回撤','峰值日','谷底日','恢复日','下跌天数','谷后恢复天数','水下总天数','最长水下天数'],rows)
add('\nK12/K20/N50_K12/N80_K15的最大回撤深度几乎相同，不能视为四次独立风险证据：最大回撤落在同一2024-07-10至09-26区段；小宇宙会使不同名义K被压到相同实际名额。不同K仍会改变之后的恢复速度。')
add('\n## 三时期：名额是否解耦、实际持有几个币')
add('\n分组固定复用已有研究：pre=2023-07-04至2024-03-04（245天），drawdown=2024-03-05至2024-10-15（225天），post=2024-10-16至2026-10-07（722天）。这三组用于横向比较，**不是各候选自己的最大回撤日期**。以下使用原始p0完整日期，保证与已确认机制表同口径。')
add('\n每格为 **含空仓日平均kk / 平均日换手 / 成交后实际净持仓币数**。')
rows=[]
for x in R:
 rows.append([label(x['name'])]+[f"{D[x['name']][p]['kk']:.3f} / {D[x['name']][p]['turn']:.3f} / {D[x['name']][p]['actual_names']:.2f}" for p in ['pre','drawdown','post']])
table(['变体','pre：kk / 换手 / 持仓','drawdown：kk / 换手 / 持仓','post：kk / 换手 / 持仓'],rows)
add('\n**早期均值不能解释成“长期一对一”**：245天中只有89天有非零目标，156天因合格宇宙不足8而目标为空。基线含空仓kk=1.073，但活跃日平均2.955、范围2–5，净持仓活跃日均9.36个币。早期宇宙平均7.06不代表每天只有7个；交易日宇宙均值约14.98。这里直接沿用现有mechanism_summary.json的已确认分组事实，不重做换手机制归因。')
add('\n下表只在早期89个活跃日统计；“压缩”专指K被N//2压小，不包括156天空仓。实际持仓按成交后数量计数，可有精度/调仓带留下的零碎仓位；逐日目标与实际计数都在daily.csv。')
rows=[]
for x in R:
 d=D[x['name']]['pre']
 rows.append([label(x['name']),f"{d['active_kk_mean']:.3f}",f"{d['active_kk_min']}–{d['active_kk_max']}", '—' if x['name']=='baseline' else pct(d['constrained_active_fraction']),f"{d['active_actual_names']:.2f}",f"{d['active_actual_names_min']}–{d['active_actual_names_max']}",f"{d['active_actual_long']:.2f} / {d['active_actual_short']:.2f}"])
table(['变体','早期活跃日kk均值','kk范围','K被压缩日占比','实际币数均值','实际币数范围','实际多 / 空币数均值'],rows)
add('\n不能声称大K全程固定：K12早期94.38%的活跃日被压缩，post仍有9.70%；K20早期100%被压缩，post仍有68.01%。K3在所有活跃日固定为3；K5在drawdown/post固定为5；K8仅在post完全固定为8。固定K已移除20%规则，但剩余的容量约束仍会随宇宙变化。')
rows=[]
for x in R:
 cells=[]
 for p in ['pre','drawdown','post']:
  d=D[x['name']][p]
  cells.append(f"{d['active_kk_mean']:.3f} [{d['active_kk_min']}–{d['active_kk_max']}] / "+('—' if x['name']=='baseline' else pct(d['constrained_active_fraction'])))
 rows.append([label(x['name'])]+cells)
table(['变体','pre活跃kk均值[范围] / 压缩率','drawdown活跃kk均值[范围] / 压缩率','post活跃kk均值[范围] / 压缩率'],rows)
add('\n固定宇宙上限实际约束程度如下。保留流动性门槛后，原合格宇宙全样本最大仅55；因此不能把N50/N80描述成全程50/80币宇宙。')
rows=[]
for n in ['N30_K5','N50_K8','N50_K12','N80_K15']:
 rows.append([n]+[f"{D[n][p]['selected_n']:.2f} / {D[n][p]['top_n_binding_days']}" for p in ['pre','drawdown','post']]+[sum(D[n][p]['top_n_binding_days'] for p in ['pre','drawdown','post'])])
table(['变体','pre平均宇宙 / 上限生效天数','drawdown平均宇宙 / 生效天数','post平均宇宙 / 生效天数','全样本生效天数'],rows)
add('\n## 两个子区间')
add('\n以下为共同日期三个启动平均：2023–24为545天，2025–26为645天。每个子区间的CAGR从该区间日收益独立计算。正Sharpe/正算术均值不保证CAGR为正。')
for period in ['2023-24','2025-26']:
 add('\n### '+period+'\n')
 rows=[]
 for x in R:
  m=x['subperiods'][period]
  rows.append([label(x['name']),f(m['sharpe']),f(m['delta']),f(m['daily_turnover']),pct(m['annual_cost']),pct(m['annual_arithmetic']),pct(m['cagr']),pct(m['max_drawdown'])])
 table(['变体','Sharpe','Δ基线','日换手','年化成本','年化净利·算术','净利CAGR','区间最大回撤'],rows)
add('\n原始1192天口径的子区间Sharpe另列，避免共同日期删去两天影响被隐藏：')
rows=[]
for x in R:
 raw=json.loads((O/f"{x['name']}_p0_f0.0007.json").read_text())
 rows.append([label(x['name']),f(raw['2023-24']['sharpe']),f(raw['2025-26']['sharpe'])])
table(['变体','原起点2023–24（547天）','原起点2025–26（645天）'],rows)
add('\n## Block bootstrap、FWER与判定')
add('\n对9候选共同进行配对circular moving-block bootstrap；各候选/基线/启动共享抽样日期，直接重算三个启动的平均Sharpe差。主块长20天、9999次；10/40天各4999次。bootstrap差减观测差构造中心化零分布，以各候选bootstrap标准差学生化，取9候选maxT校正单侧正改进；p=(1+超过次数)/(B+1)。固定seed=20261009+块长，分500次检查点落盘。')
add('\n均值/方差充分统计量实现与直接抽样重算已独立核对；没有用i.i.d. t检验。95%区间是单候选basic bootstrap区间，**不是同时置信区间**。')
rows=[]
for x in R[1:]:
 s=x['bootstrap']['20']
 rows.append([x['name'],f(x['delta']),f(s['se']),f(s['p']),f(s['p_fwer']),f(x['bootstrap']['10']['p_fwer']),f(x['bootstrap']['40']['p_fwer']),f"[{s['basic_ci95_low']:.3f}, {s['basic_ci95_high']:.3f}]"])
table(['变体','平均ΔSharpe','block SE','p原始 L20','p_FWER L20','p_FWER L10','p_FWER L40','Δ的单候选95%区间'],rows)
add('\n“可用”严格分别列出四条；FWER门槛要求三种块长均<0.05。用户指定“两子区间为正”，协议更严格的“两子区间均改善”另列，未混淆。')
rows=[]
for x in R[1:]:
 c=x['checks']
 rows.append([x['name']]+[yes(c[k]) for k in ['fwer','positive_subperiods','improvement_gt_phase_noise','turnover_le_baseline']]+[yes(x['usable']),yes(x['protocol_both_improved'])])
table(['变体','FWER<0.05','两段为正','改进>相位噪声','换手≤基线','可用','协议：两段均改善'],rows)
add('\n### 单独判定：风险显著降低\n')
rows=[]
for x in R[1:]:
 rows.append([x['name'],pct(x['max_drawdown']),f"{x['drawdown_improvement']*100:+.2f} pp",f(x['delta']),yes(x['drawdown_improvement']>=.1),yes(x['delta']>=-.15),yes(x['risk_reduced']),yes(x['risk_all_starts'])])
table(['变体','最大回撤','较基线改善','ΔSharpe','回撤改善≥10pp','Sharpe降幅≤0.15','风险显著降低','三个启动各自均通过'],rows)
add('\n此标签不要求收益FWER显著，不赋予“可用”资格。10个百分点阈值在候选计算前写死；最大回撤为路径极值，未给其改善做独立统计显著性检验。')
add('\n## 成本敏感性')
add('\n四档均完整重跑订单/净值路径，不是从原收益简单减差额。0.015%为理想maker成本下界，不表示能实际按maker成交；0.045%为taker；0.075%=0.045%+实测滑点档0.03%；主档仍按用户指定0.07%。每格为 **Sharpe / 算术年化净利 / 年化成本**。')
rows=[]
for x in R:
 row=[label(x['name'])]
 for fee in ['0.00015','0.00045','0.0007','0.00075']:
  m=x['cost_cases'][fee];row.append(f"{m['sharpe']:.4f} / {pct(m['annual_arithmetic'])} / {pct(m['annual_cost'])}")
 rows.append(row)
table(['变体','单边0.015%','单边0.045%','单边0.070%','单边0.075%'],rows)
rows=[]
for x in R:
 cs=x['cost_cases']['0.00075']['subperiods']
 rows.append([label(x['name']),f(cs['2023-24']['sharpe']),f(cs['2023-24']['delta']),f(cs['2025-26']['sharpe']),f(cs['2025-26']['delta'])])
table(['变体','0.075%下2023–24 Sharpe','Δ基线','2025–26 Sharpe','Δ基线'],rows)
add('\n## 解读、局限与交付文件')
add('\n1. **币数与换手并非单调正相关。** 固定K3仍处于不断变化的流动性宇宙，K固定也不代表名单或权重固定。K变大时单币名义权重更小、三因子净合并结构不同，本次换手反而下降；这些结果不能单独识别每个渠道的因果贡献。上一轮跨时期机制不能直接替代这次控制参数实验。')
add('2. **风险改善具有样本内价值，但收益不显著。** K12两段均改善、换手较低、回撤少约24.95个百分点；然而FWER约0.30，ΔSharpe的单候选95%区间跨零。应在独立数据/未来数据继续验证，不能据此宣称稳定优势。')
add('3. **大K仍受容量限制。** 早期K12实际活跃日只平均7.09个因子每腿名额；K20平均7.21。净合并后实际币数分别约13.99和14.21。名义参数相差很大，早期组合却很接近。N80完全未约束，不能归功于固定宇宙。')
add('4. **成本与容量局限。** 采用日线开盘成交和恒定单边成本，没有逐币深度、市场冲击、借贷/资金费或历史交易精度；当前meta、可获得币列表与缺失历史仍可能有样本选择偏差。本实验所有路径无持仓报价缺失，但这不消除上述偏差。')
add('5. **统计范围有限。** FWER只覆盖本次9个事先指定候选，不覆盖历史研究方向；3个相邻启动不等同跨市场周期，block长度10/20/40也不能消除非平稳性。阈值所称风险显著不是最大回撤的统计显著性。')
add('6. **原窗口和固定分组。** 为复现冻结基线没有扩展到2023-01-01或纳入未闭合的末日。早期有大量空仓日；三时期比较固定按基线的旧日期分组，各候选真正的最大回撤日期在专表单独列出。')
add('\n复跑命令（必须从 `/Users/wonder/Code/stars` 执行，所有脚本使用指定解释器）：\n')
add('```sh\n/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/trailing_tp_20261008/baseline.py\n/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/universe_size_20261009/study.py\nOPENBLAS_NUM_THREADS=1 /Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/universe_size_20261009/analyze.py\n/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/universe_size_20261009/verify.py\n/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/universe_size_20261009/report.py\n```')
add('\n[study.py](study.py)复用原run并替换target；每个候选/启动/成本分别保存npz、json和daily.csv，共120条路径。 [analyze.py](analyze.py)生成指标与bootstrap，500次重采样落检查点，可续跑。 [results.json](results.json)含全部数值与判定，[periods.json](periods.json)含三时期与早期实际持仓，[bootstrap.json](bootstrap.json)含统计检验。')
add('\n[verification.json](verification.json)确认：10个配置主成本原起点全部与“原始run仅替换target、不加诊断”的执行逐日完全一致；120条路径回撤经独立pandas算法校验；原baseline.py哈希未变；无持仓报价缺失。[manifest.json](manifest.json)记录178个数据文件及原引擎/规则哈希；[implementation_manifest.json](implementation_manifest.json)记录最终分析脚本哈希。用户工作区原有其他研究改动保留。')
(O/'REPORT.md').write_text('\n'.join(lines)+'\n')
# Flat CSV deliverables retain numeric precision for reuse.
pd.DataFrame([{k:x[k] for k in ['name','sharpe','delta','daily_turnover','annual_cost','annual_arithmetic','cagr','max_drawdown','underwater_days','recovery_days','phase_sd','phase_delta_range','corr']} for x in R]).to_csv(O/'results.csv',index=False)
paths=[O/x for x in ['SPEC.md','study.py','analyze.py','verify.py','report.py','REPORT.md']]
(O/'implementation_manifest.json').write_text(json.dumps({str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in paths},indent=2))
print('REPORT.md written:',len(lines),'blocks')
