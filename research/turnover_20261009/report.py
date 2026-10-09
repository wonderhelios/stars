import json,sys,hashlib
from pathlib import Path
import numpy as np,pandas as pd
O=Path(__file__).resolve().parent
summary=json.loads((O/'mechanism_summary.json').read_text());res=json.loads((O/'results.json').read_text());deep=pd.read_csv(O/'deep_summary.csv',index_col=0);daily=pd.read_csv(O/'daily_analysis.csv');audit=json.loads((O/'nonlinear_bootstrap_audit.json').read_text())
pre,dd,post,pa=[summary[x] for x in ['pre','drawdown','post','pre_active']];base=json.loads((O/'baseline_p0_f0.0007.json').read_text());b=base['full'];delta=dd['turn']-pre['turn'];composition=pa['turn']-pre['turn']
def pct(x):return f'{x*100:.2f}%'
def table(headers,rows):return '| '+' | '.join(headers)+' |\n| '+' | '.join(['---']*len(headers))+' |\n'+'\n'.join('| '+' | '.join(map(str,row))+' |' for row in rows)+'\n'
def corr(name):
 x=pd.read_csv(O/f'corr_{name}_pearson.csv',index_col=0);labels=['换手','截面波动','市场20日波动','组合20日波动','排名距离','合格币数'];return table(['Pearson']+labels,[[labels[i]]+[f'{v:.3f}' for v in x.iloc[i]] for i in range(len(x))])
text=f'''# 回撤期换手机制与降速实验

本次 **12 个候选，没有一个达到“可用”标准**。按预先独立设定的描述性标准，权重平滑 α=0.3/0.5、排名滞回 30%/40% 共四个达到“低成本改进”；这不等于收益改善已经得到统计支持，也不表示可以进入实盘。

“越亏越换”的表象主要来自比较样本的仓位状态不同：回撤前 245 天中 **156 天合格币不足 8 个，触发全空仓目标**。只比较有目标仓位的日子，换手从 **{pa['turn']:.3f} 到 {dd['turn']:.3f}，增加 {pct(dd['turn']/pa['turn']-1)}**，而不是三倍。余下换手主要是排名引起的名单/净权重变化，特别是每日相对成交量 `Q/V` 因子；不是 10 美元订单门槛，也没有证据表明回撤期内它只是波动率的副产物。

## 1. 基线复现、数据边界与事先判定

直接执行原 `research/trailing_tp_20261008/baseline.py`：cap=5、open、gross=2.7、fee=0.0007、初始资金 $1,000,000，得到 **Sharpe {b['sharpe']:.10f}**，通过 1.1293±0.00005 门槛。诊断版从源文件提取同一个 `run` 函数，仅插入只读记录点；候选只替换 `target`。诊断版每日收益、每日换手与原版 **逐元素完全相同**。价格、数量精度、2% 调仓带、最低订单、手续费、缺失报价处理全部复用。

{table(['区间','天数','Sharpe','日换手','年成本','算术年化净利'],[[z,base[z]['days'],f"{base[z]['sharpe']:.6f}",f"{base[z]['daily_turnover']:.6f}",pct(base[z]['annual_cost']),pct(base[z]['annual_arithmetic'])] for z in ['full','2023-24','2025-26']])}
原始全样本期末净值 {base['ending_equity']/1e6:.6f}x；没有持仓缺失报价。换手单位为成交名义金额/交易前净值，包含 2.7 倍杠杆，不除以 2；成本为日均换手×单边费率×365。

**数据范围按原引擎原样保留，不能把 2023-01-01 写成实际回测起点。** `/tmp/hl-daily-v2/` 为 178 个 JSON；原引擎读取的 request 指定起始日 2023-06-01，完整日线过滤后最后一天为 2026-10-07，加上 33 日启动，实际收益为 **2023-07-04～2026-10-07，共1192天**。本研究没有补造此前数据，也没有改变时间过滤来凑基线。固定回撤窗口按用户给定的 2024-03-05～2024-10-15（含两端，225个观测）划分；没有重新选择峰谷。

候选计算前写入 [SPEC.md](SPEC.md)：

- “可用”：12候选共同 maxT 校正，10/20/40日块长的 `p_fwer` 均 <0.05；2023–24、2025–26 净 Sharpe 均为正且各自改善；正改进大于配对启动偏移 ΔSharpe 极差；0.00075 成本下两个子区间仍改善；无持仓报价缺失。
- **“低成本改进”独立判定**：全样本 ΔSharpe≥−0.10，固定回撤期换手≤基线2/3（≤{dd['turn']*2/3:.6f}）；不混入可用名单，不要求收益提升显著。原始全样本与共同日期启动均值两种口径的四个入选结果一致。
- 每个候选每日调仓，日历相位只有一个；20日滚动窗口不是20个调仓相位。另做启动偏移0/1/2日，并在 **2023-07-06～2026-10-07共同1190日** 计算起点均值/离散度与配对检验。它们是启动敏感性，不是独立样本；不通过扩充样本数取得显著性。

## 2. 问题一：换手为何从0.64升到2.02

### 2.1 互斥、可加总的实际成交分解（核心表）

分类根据“今日净目标 vs 昨日净目标”，统计对应币实际成交金额/净值。三个因子同一币的多空先抵消，所以这里的“腿”是合并后的净多/净空腿。反手单列，不与名单进出重复计算。

'''
cat=[('entry','净名单进入'),('exit','净名单退出'),('target_flip','净目标反手'),('same_weight_change','仍在同侧，目标权重变动'),('target_unchanged','净目标不变，价格/净值漂移调仓')]
text+=table(['互斥类别','回撤前','回撤期','回撤后','回撤期占比','前→回撤增量','占总增量'],[[lab,f"{pre[key]:.4f}",f"{dd[key]:.4f}",f"{post[key]:.4f}",pct(dd[key]/dd['turn']),f"{dd[key]-pre[key]:.4f}",pct((dd[key]-pre[key])/delta)] for key,lab in cat]+[['合计',f"{pre['turn']:.4f}",f"{dd['turn']:.4f}",f"{post['turn']:.4f}",'100%',f'{delta:.4f}','100%']])
text+=f'''
按实际下单前后持仓方向重新分类，得到同样的进入/退出/反手数值；同向调仓合计为回撤前 **{pre['actual_same']:.4f}**、回撤期 **{dd['actual_same']:.4f}**、回撤后 **{post['actual_same']:.4f}**。因此回撤期 **{pct((dd['entry']+dd['exit'])/dd['turn'])}** 是整腿进出，**{pct(dd['target_flip']/dd['turn'])}** 是反手，**{pct(dd['actual_same']/dd['turn'])}** 是同向调仓。这里“目标变动”类别也可能含该币的价格漂移，不能把它全部当成纯信号成本。

进一步对“信号目标变化 + 持仓价格/净值漂移”用对称 Shapley 分摊逐币绝对成交量（缩放到实际成交后精确加总），回撤期为 **{dd['signal_shapley']:.4f} + {dd['drift_shapley']:.4f} = {dd['turn']:.4f}**，即信号占 **{pct(dd['signal_shapley']/dd['turn'])}**。这是机械金额归因，不是收益因果估计。

### 2.2 宇宙大小与“不足8币就不交易”开关

'''
text+=table(['指标','回撤前245日','其中有目标89日','回撤期225日','回撤后722日'],[[label]+[f'{x[key]:.4f}' if key not in ['days','active_days','universe_min','universe_max'] else str(x[key]) for x in [pre,pa,dd,post]] for key,label in [('active_days','有非零目标的天数'),('universe','合格币数均值'),('universe_min','合格币数最小'),('universe_max','合格币数最大'),('kk','每因子每侧席位数均值'),('active_names','合并后非零币数均值'),('normalizer','抵消后原始总绝对权重g'),('turn','日换手')]])
text+=f'''
合格集合严格按原引擎：滞后30日美元成交量均值 `V>=5e6`（至少5个观测）、当日和14日前收盘价有限、20日收益标准差有限（至少5个观测），且币名不含冒号；这里不额外要求每个滚动窗口逐日无缺口。基线在 `len(idx)<8` 时直接返回零目标。回撤前有仓目标占比 **89/245={pct(89/245)}**，回撤期为100%。156个零目标日平均换手仅 **{(pre['turn']*245-pa['turn']*89)/156:.6f}**，主要是切换为空仓时的平仓；这把前期均值明显压低。

恒等式分解：`2.0210−0.6421 = (1.7328−0.6421) + (2.0210−1.7328)`。前项 **{composition:.4f}（{pct(composition/delta)}）** 是把前期均值换为其有目标日均值的组合效应；后项 **{dd['turn']-pa['turn']:.4f}（{pct((dd['turn']-pa['turn'])/delta)}）** 才是有目标日之间的差异。该分解依赖此替换顺序，**不是声称“如果空仓日强行交易会如何”的反事实**。

资格数量增多本身并不必然提高日换手：剔除零目标日后，换手与合格币数的相关系数为 **−0.153**。回撤期自身资格发生进入/退出的币只贡献 **{deep.loc['drawdown','eligibility_changed_turn']:.4f}（{pct(deep.loc['drawdown','eligibility_changed_turn']/dd['turn'])}）** 日换手；绝大部分是在仍然合格的币之间变换目标。资格进入/退出平均 {deep.loc['drawdown','eligibility_entries']:.3f}/{deep.loc['drawdown','eligibility_exits']:.3f} 币/日，每侧席位数k有变化的天数占 {pct(deep.loc['drawdown','k_changed'])}。

### 2.3 具体是哪种排名在制造交易

基线不是“名次每变化一位就连续改权重”。每个因子分别等权持有前后k名，三个因子各占1/3，合并后再除以净绝对权重 `g`。同侧目标权重变化来自 **跨越名单边界、因子投票/抵消变化、k变化、以及g重新归一化**；即使某币自己的投票没变，其他币改变g也会改变它的权重。每因子最多5席，宇宙扩大后实际入选分位可能小于20%。

对三个归一化因子目标差与漂移作四参与者 Shapley 分摊，逐币映射回实际成交。这与上一节两参与者归因是不同分组，不能交叉相加：

'''
text+=table(['实际成交归因','回撤前','回撤期','回撤后','回撤期占比'],[[lab]+[f'{deep.loc[z,key]:.4f}' for z in ['pre','drawdown','post']]+[pct(deep.loc['drawdown',key]/dd['turn'])] for key,lab in [('exec_momentum','14日动量/波动率排名'),('exec_lowvol','低波动排名'),('exec_volume','当日相对成交量Q/V排名'),('exec_drift','价格与净值漂移')]])
text+=f'''
**相对成交量排名贡献回撤期实际换手的 {pct(deep.loc['drawdown','exec_volume']/dd['turn'])}**，它每日重排；对应平均百分位排名距离 **{deep.loc['drawdown','rank_2']:.3f}**，动量为 **{deep.loc['drawdown','rank_0']:.3f}**，低波动为 **{deep.loc['drawdown','rank_1']:.3f}**。所以这不是一个仅由慢速14日动量驱动的组合。

在理想目标变化（未计漂移与成交门槛）中，回撤期 L1换手为 **{dd['ideal_signal_turn']:.4f}**；把 `u/g` 的变化对称分为原始净票权变化与g变化，对应 **{dd['raw_change_shapley']:.4f} / {dd['normalization_shapley']:.4f}**。归一化相关分量占 {pct(dd['normalization_shapley']/dd['ideal_signal_turn'])}。这是同一次权重变化的另一种金额分摊，不是增加一笔成本。

### 2.4 10美元门槛：本回测几乎没有约束力

'''
text+=table(['指标','回撤前','回撤期','回撤后'],[[label]+[(str(int(x[key])) if key=='skip_min_count' else f'{x[key]:.9f}' if 'turn' in key else f'{x[key]:.3f}') for x in [pre,dd,post]] for key,label in [('skip_min_count','<$10的未成交调仓次数'),('skip_min_usd','这些未成交意向金额合计($)'),('skip_min_turn','其日均意向金额/净值'),('skip_band_n','≥$10但未过2%调仓带的次数/日'),('skip_band_turn','其日均意向金额/净值'),('small_target_n','因目标不足$10被置零的币/日')]])
text+='''
完整关闭原引擎中三个10美元门槛后，**1192天每日收益与换手逐元素完全不变**。上述全期5笔<$10意向也被2%调仓带阻挡；并非拿走最低金额门槛就会成交。最小订单不会造成换手上升，且此处从百万美元起始资金跌至谷底后仍有数十万美元，与小资金实盘不同。精度取整仍照原引擎执行。

### 2.5 相关系数矩阵

T=当日开盘调仓换手；截面波动=该日所有有有限收益的币（不限合格名单）的收盘收益截面标准差，仅作事后相关分析；市场20日波动=每日合格币等权收益的滚动20日标准差，截至调仓前日；组合20日波动=基线净收益过去20日标准差，截至前日。排名距离=连续两信号日共同合格币、三个因子的平均绝对百分位名次差，取值0～1；不将新入/退出宇宙的币硬设为排名零。排名在引擎零目标日未定义，相关系数使用成对有效样本。

全样本 Pearson：

'''+corr('full')+'\n固定回撤期 Pearson（225日）：\n\n'+corr('drawdown')
text+='''
全样本换手与组合时序波动的0.522，混入了前期长期零仓位、零组合收益的状态差别。限制到有目标的1036天，换手与市场20日波动 **−0.000**、组合20日波动 **0.005**，与排名距离 **0.532**；回撤期内对应 **−0.049 / −0.018 / 0.592**。因此不能用全样本相关性推断“亏损→波动→换手”；这里更直接的机械驱动是排名产生目标变化，以及是否跨过最低宇宙门槛。相关性也不能独自证明因果关系。

每对变量的有效样本量见 `corr_pairwise_counts.csv`。完整日级数据、全期/回撤期/有目标日的 Pearson 和 Spearman 矩阵分别保存在 `daily_analysis.csv` 与 `corr_*_*.csv`。截面波动还保存了信号日全体币、信号日合格币两种替代定义，便于复核时间口径。

## 3. 问题二：三类修法、全部参数结果

实现规则：

- **平滑**：`w=(1−α)×上日平滑目标+α×今日原始目标`，之后归一化Σ|w|=1；新币旧权重零，从α倍开始；仍合格但退出名单的币逐渐衰减；失去资格币立即清零。没有额外人为截尾。原始N<8开关强制清空目标并重置状态；若全向量为零则保持零。
- **波动降速**：信号日市场20日波动超过此前最多252日、至少60日的q分位数时，使用α平滑；否则α=1。所有阈值严格使用当时已有历史，次日开盘执行。没有按未来回撤标签调参。
- **滞回**：三个因子各侧独立保留旧名单，离开该侧前q分位才退出，缺额用当前最优排名补齐；有效退出界线至少为k/N，避免小宇宙的退出界线比入场还窄。保持每因子每侧k个席位，合并并归一化。新币依照排名补入。

### 3.1 原始全样本完整结果（与1.1293直接对照）

下表全部为原始启动、1192日；Δ为相对于1.1292998945。算术净利=日净收益均值×365，CAGR=复利年化，两者均已扣0.0007成交成本，未扣资金费。两个子区间列净Sharpe。

'''
fullrows=[]
for r in res:
 name=r['name'];x=json.loads((O/f'{name}_p0_f0.0007.json').read_text());a=np.load(O/f'{name}_p0_f0.0007.npz');f=x['full'];ca=np.expm1(np.log1p(a['r']).mean()*365);dt=pd.to_datetime(a['t'],unit='ms',utc=True);dm=(dt>=pd.Timestamp('2024-03-05',tz='UTC'))&(dt<=pd.Timestamp('2024-10-15',tz='UTC'));draw_turn=float(a['turn'][dm].mean())
 fullrows.append([name,f"{f['sharpe']:.4f}",f"{f['sharpe']-b['sharpe']:+.4f}",f"{f['daily_turnover']:.4f}",pct(f['annual_cost']),pct(f['annual_arithmetic']),pct(ca),f"{x['2023-24']['sharpe']:.4f}",f"{x['2025-26']['sharpe']:.4f}",f"{draw_turn:.4f}",pct(1-draw_turn/dd['turn'])])
text+=table(['候选','Sharpe','Δ基线','日换手','年成本','算术年化净利','CAGR','2023–24 S','2025–26 S','回撤期换手','回撤期降幅'],fullrows)
text+='''
### 3.2 相位/启动、配对显著性与独立判定

以下是三个启动在共同1190日上的均值。该共同样本基线为 **1.117597**，不同于完整1192日的1.129300，原因是共同评估删除最初两日，并非复现失败。两子区间改善、相位噪声与检验均配对同一日期。各候选启动Sharpe原值见 `results.json`。

统计：配对Sharpe差的影响函数、中心化圆形移动块 bootstrap，4999次，固定随机种子；每次联合抽取同一批日期，12候选 maxT 控制本次家族错误率，单侧检验正改善。20日为主块长，10/40日为敏感性，均需通过。起点之间不作为独立重复观测。再用直接重算非线性Sharpe差的20日块bootstrap审核，所有结论不变。

'''
text+=table(['候选','启动均值S','Δ均值','启动S标准差','配对Δ极差','子期ΔS 23–24/25–26','p块20','pFWER 10/20/40','与基线收益相关','可用','低成本'],[[r['name'],f"{r['sharpe']:.4f}",f"{r['delta']:+.4f}",f"{r['phase_sd']:.6f}",f"{r['phase_range']:.6f}",f"{r['d1']:+.4f}/{r['d2']:+.4f}",f"{r.get('p_L20',float('nan')):.4f}",'/'.join(f"{r.get(f'pfwer_L{l}',float('nan')):.4f}" for l in [10,20,40]),f"{r['corr']:.4f}",'是' if r['usable'] else '否','是' if r['low_cost'] else '否'] for r in res])
text+='''
“低成本改进”名单单列：**smooth_0.3、smooth_0.5、hyst_0.3、hyst_0.4**。这些配置只满足用户事先指定的样本点估计门槛；没有进行统计意义上的非劣效确认。

- **40%滞回**：完整样本Sharpe 1.4039（+0.2746），回撤期换手1.0400（−48.54%），共同日期的两个子区间均改善，可保留作后续独立样本验证；但20日块pFWER=0.6400，没有通过可用门槛。直接非线性bootstrap的ΔSharpe 95%区间为 '''+f"[{audit['hyst_0.4']['ci95'][0]:.3f}, {audit['hyst_0.4']['ci95'][1]:.3f}]"+'''，不确定性很大。
- **30%滞回**：回撤期换手降低34.34%，全期Sharpe上升，但前子区间变差且仍负，因此也不能称作可靠收益改善。
- **α=0.3/0.5平滑（保留N<8开关）**：回撤期换手降低63.74%/45.46%，完整样本Sharpe分别1.4907/1.3714，两子区间均改善；主块pFWER分别0.3250/0.2790，均未显著。α=0.3年成本从40.82%降至15.29%，是本轮点估计上最突出的低成本配置，但不能判为可用。α=0.7收益点估计也改善，回撤期换手降幅不够1/3。
- **波动降速**：q=0.60、α=0.3 全期与两个子区间点估计改善，但回撤期换手只降低30.75%，未达1/3，且pFWER=0.6142。其他波动阈值/速率也未满足低成本条件；高波动触发器没有稳定抓到排名换手。
- **25%滞回**：全期和两个子区间均变差，回撤期换手降幅不足1/3。本轮没有可用候选。

### 3.3 成本敏感性：每个费率完整重跑

按共同1190日起点均值；每格为“净Sharpe / 算术年化净利 / 年成本”。0.00015仅是理想maker费率下界，**不包含排队、成交选择或逆向选择，不能当成可实现maker回测**。0.00045为taker费；0.00075=taker0.045%+滑点0.03%。主表沿用原引擎0.0007以保证与基线一致。

'''
text+=table(['候选','maker费率0.00015（理想）','taker 0.00045','taker+滑点0.00075','0.00075子期ΔS'],[[r['name']]+[f"{r['cost_cases'][str(f)]['sharpe']:.4f} / {pct(r['cost_cases'][str(f)]['annual'])} / {pct(r['cost_cases'][str(f)]['cost'])}" for f in [.00015,.00045,.00075]]+[f"{r['cost_cases']['0.00075']['d1']:+.4f}/{r['cost_cases']['0.00075']['d2']:+.4f}"] for r in res])
text+='''
### 3.4 实现审计修正与核对

初轮平滑代码在N<8时把残留旧权重再归一化为1，导致69日绕过基线的零仓位开关。此问题在最终审计发现后修正：所有候选在N<8时强制零目标并清空状态，固定原参数与原判定，重跑全部12候选，未进行结果驱动调参。初版输出仅保存在 `audit_pre_gate_fix/` 供审计，**本报告所有候选结果均使用修正版**。该修正明显改变平滑的前子区间表现，所以不能沿用初轮结果。FWER按最终12候选共同重新计算，未改变的候选其校正p值也会随联合零假设分布改变。

额外验证：所有候选逐日目标均有限，非零时Σ|w|=1，无N<8开关违例；α=1平滑对照与原始基线收益/换手在1e−12内一致。逐笔归因加总误差小于1.4e−15；156条最终资金路径无持仓报价缺失。详见 `engine_checks.json` 与 `verification.json`。α=0.3直接非线性bootstrap的配对ΔSharpe 95%区间为[−0.160, 0.887]，再次表明点估计改善尚未显著。

## 4. 结论与局限

机制链条是：**前期大量不足8币的零目标日压低均值 → 后期持续满额目标 → 日更相对成交量等排名跨边界 → 因子净抵消及归一化改变其他币权重 → 实际交易与成本**。不能据此把回撤导致交易变多当作因果事实。回撤前有仓日与回撤期之间仍有16.6%的换手增长，其中大部分交易来自名单变化，10美元门槛没有实质影响。

40%排名滞回和两档较慢平滑确实在本样本内降低了固定回撤期的换手；但**没有修法通过预设显著性门槛**。保留四个“低成本改进”实验记录，与“可用”严格分开；不建议据此修改实盘。

局限：

- 本次12候选是样本内探索，回撤窗口事先已知。FWER仅覆盖本次12个候选，不覆盖项目历史130+方向，也不能消除研究选择偏差。
- 仅225个固定回撤观测，且收益厚尾；block bootstrap并不能解决结构断点或数据覆盖变化。非线性复核同样未显著。
- 合格宇宙是当前磁盘数据与原引擎规则下的宇宙。前期不足8币可能混合市场发展、流动性、数据覆盖等因素；未建立历史完整币表反事实，不能把它全部归于真实市场规模变化。美元成交量代理Q=收盘价×基础币量与原引擎一致。
- 实际范围不是2023-01-01开始；沿用现有meta数量精度，未按历史交易规则变更重建。未计资金费、容量冲击、借币/资金约束、强平与终端清仓；本结果是原研究引擎内的比较。
- 2.7倍目标总敞口保持，但平滑/滞回可以改变单币集中度、净方向暴露与持仓残留；权重变化并不只是减少执行成本，也改变了信号策略。归因Shapley是对抵消项作对称分配，不等价于关掉某因子后的收益或因果效应。
- 相位主问题在每日调仓中只有一个相位；本次额外三启动敏感性不能替代样本外验证。相关矩阵使用成对有效值，排名缺失日包括零目标日和相邻启动/重启日；没有把有效样本量当作1192日一概使用。
- 最低金额结论限于百万美元初始资金路径；不能直接外推到小额账户。maker费率表只作成本敏感性，不模拟成交。

## 5. 复核与落盘

均在 `/Users/wonder/Code/stars` 使用指定解释器：

```sh
/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/turnover_20261009/study.py
/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/turnover_20261009/analyze.py
/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/turnover_20261009/deeper.py
/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/turnover_20261009/validate.py
/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/turnover_20261009/report.py
```

`study.py`按候选×启动×成本逐个写入NPZ/JSON，可从检查点继续；13（含基线）×3×4=156条资金路径。`SPEC.md`为事先方案；`daily_analysis.csv`为可加总逐日归因；`results.csv/json`为完整结果；`bootstrap_L*.npz`保存联合抽样统计量；`nonlinear_bootstrap_audit.json`为直接Sharpe复核；`no_min.npz/json`为最低订单反事实。输入及脚本哈希见 `manifest.json`，计算与守恒检查见 `verification.json`。直接执行原基线时，其原目录中的生成文件NPZ/JSON/manifest由原脚本自行重写，未改基线源码。本研究没有修改 `src/`，没有commit/push/部署或接触实盘。
'''
text=text.replace('| nan | nan/nan/nan |','| — | — |')
(O/'REPORT.md').write_text(text)
pd.DataFrame(fullrows,columns=['name','sharpe','delta','daily_turnover','annual_cost','annual_net_arithmetic','cagr','sharpe_2023_24','sharpe_2025_26','drawdown_turnover','drawdown_reduction']).to_csv(O/'results_full.csv',index=False)
# Runtime and mechanical checks, including pairwise counts and original full-sample low-cost labels.
verification={'python':sys.executable,'cwd':str(Path.cwd()),'baseline_sharpe':b['sharpe'],'candidate_count':12,'grid_paths':156,'all_grid_paths_present':all((O/f"{r['name']}_p{p}_f{f}.npz").exists() for r in res for p in range(3) for f in [.0007,.00045,.00015,.00075]),'decomposition_error':float(abs(daily[['entry','exit','target_flip','same_weight_change','target_unchanged']].sum(axis=1)-daily.turn).max()),'four_player_error':float(abs(daily[['exec_momentum','exec_lowvol','exec_volume','exec_drift']].sum(axis=1)-daily.turn).max()),'all_missing_quotes':sum(len(json.loads(p.read_text()).get('missing_held_quotes',[])) for p in O.glob('*_p*_f*.json')),'original_full_low_cost':[]}
for r in res[1:]:
 x=json.loads((O/f"{r['name']}_p0_f0.0007.json").read_text())
 if x['full']['sharpe']-b['sharpe']>=-.1 and r['dd_reduction']>=1/3:verification['original_full_low_cost'].append(r['name'])
a=np.load(O/'baseline_p0_f0.0007.npz');z=np.load(O/'no_min.npz');verification['no_min_exact_equal']=bool(np.array_equal(a['r'],z['r']) and np.array_equal(a['turn'],z['turn']))
cols=['turn','xs_vol','market_vol20','portfolio_vol20','rank_distance','universe'];valid=daily[cols].notna().astype(int);(valid.T@valid).to_csv(O/'corr_pairwise_counts.csv');(O/'verification.json').write_text(json.dumps(verification,indent=2))
manifest=json.loads((O/'manifest.json').read_text())
for p in list(O.glob('*.py'))+[O/'REPORT.md',O.parent/'trailing_tp_20261008/retrieval/request.json',O.parent/'trailing_tp_20261008/retrieval/meta.json']:manifest[str(p)]=hashlib.sha256(p.read_bytes()).hexdigest()
(O/'manifest.json').write_text(json.dumps(manifest,indent=2));print(json.dumps(verification,indent=2))
