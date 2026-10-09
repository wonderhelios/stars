import json,hashlib
from pathlib import Path
import numpy as np
import pandas as pd

H=Path(__file__).resolve().parent
load=lambda f:json.loads((H/f).read_text())
base=load('baseline_results.json')['runs']['cap5_open_0.0007']
corr=load('correlations.json');daily=load('daily_analysis.json');holds=load('hold_analysis.json');verify=load('verification.json')
names={f'distance{n}':f'新高距离{n}' for n in [20,60,120,252]}
names.update({f'binary{n}':f'二元突破{n}' for n in [20,60,120,252]})
names.update(high52='52周高点比率(364日)',channel20='通道位置20',channel120='通道位置120')
mode={'mix':'三因子替换','single':'单因子'}
fmt=lambda x,n=4:'N/A' if x is None else f'{x:.{n}f}'
pair=lambda a:' / '.join(fmt(x) for x in a)
pct=lambda x:f'{x:.2%}'
ci=lambda a:f'[{a[0]:+.3f}, {a[1]:+.3f}]'
rows=[]
L=['# 通道突破 / 新高距离：能否降低换手并保住 Sharpe', '',
'**结论：本轮可用配置为 0。22 个日频变体、6 个持有期配置全部不可用。存在显著降低换手、样本内 Sharpe 点估计接近或高于基线的配置，但“保住 Sharpe”的统计证据不足，不能混入可用策略。**', '',
'最值得记录的是通道位置20、持有5天：三因子替换版相位平均 Sharpe 1.1209、日换手0.5322；单因子版 Sharpe 1.3404、日换手0.5977。它们相对当前基线分别降换手66.69% / 62.59%，但2025–26都退步，且相对相同5日调仓旧三因子的增量不足。全部 Sharpe 改善检验 p_fwer=1.0000。', '',
'## 1. 基线复现与参考数字差异', '',
'先完整读取指定 baseline.py 与 validation-protocol.md，随后原样执行 baseline.py。全期 Sharpe **1.1292998945108697**，相对1.1293仅差 **−0.0000001055**，满足先复现再测因子的门槛。衍生引擎在日频原三因子下的逐日收益、时间戳、换手三数组与原输出**逐元素完全相等**。', '',
'|口径|Sharpe|2023–24|2025–26|日换手|年成本|',
'|---|---:|---:|---:|---:|---:|',
f"|本次指定脚本复现|{fmt(base['full']['sharpe'])}|{fmt(base['2023-24']['sharpe'])}|{fmt(base['2025-26']['sharpe'])}|{fmt(base['full']['daily_turnover'])}|{pct(base['full']['annual_cost'])}|",
'|任务中的参考数字|1.1293|−0.247|2.485|1.427|36.5%|', '',
'差异原因：当前加载器有 daily_start_ms=2023-06-01 过滤，实际使用178个JSON，行情并集2023-06-01至2026-10-07；预热33天后收益为2023-07-04至2026-10-07，共1192日（547 / 645）。提示中的229个≥100根与2023-01-01起点不属于当前可读的178文件快照。原过滤还要求 T<end，未纳入10月8日未完成日线。', '',
'此前 [REPORT2](../trailing_tp_20261008/REPORT2.md) 记录的1343日、234文件版本，Sharpe为1.176562，附带−0.246602 / 2.484797、1.4270、36.46%，正好解释提示中附带数字的来源。该报告还记录了读取窗口和原名输出曾被其他写入方改写；本次不尝试把不同版本数字拼成一条基线。**主Δ全部对本次1.1293算；换手硬门槛仍固定为用户指定1.427。**', '',
'本次输入/源码/协议的181项哈希前后未变；完整成功运行无持仓缺报价。原始版本记录见 baseline_manifest.json、baseline_results.json、baseline_reproduction.log、verification.json。', '',
'## 2. 冻结定义、筛选及统计方法', '',
'先写 [SPEC.md](SPEC.md)，再计算候选。三因子模式只替换 M/SD 整项，保留低波动、成交额冲击；单因子模式只保留新项。各腿排序、等权、gross=2.7、cap5、流动性≥500万美元、开盘交易、数量向下取整、10美元最小订单、2%不交易带均沿用原引擎。7bp单边成本=4.5bp taker+2.5bp滑点；成本=365×平均日换手×0.0007。协议的3bp滑点与本任务明确的2.5bp不同，本轮按任务指定7bp执行，不引用7.5bp的收益。', '',
'新高距离用 P/max(P[t−N:t])−1，严格照题面含当日及前N天，共N+1根；二元突破用 1[P_t>max(P[t−N:t−1])]，必须严格超过过去N日收盘高点，等于不算；通道位置用同一闭区间收盘极值。信号在t收盘完成，t+1开盘交易，无同日收盘后回填成交。完整窗口不足或通道宽度为0记缺失。新因子有效集合供混合模式所有腿共用，并另跑同集合旧三因子控制。', '',
'股票常用252交易日的“高点百分比”P/max252与distance252只差常数1，排序/回测完全重复，作为同一候选的别名报告。为覆盖加密资产真正52周，本轮另测52×7=364个日历日跨度（含两端365根）的P/max364。未用日内high/low冒充题面收盘价P；不将二元未突破状态人为改成反向突破。', '',
'横截面相关性每天在共同可交易集合（至少8币）上计算，日期等权；报告Pearson和Spearman对M及实际M/SD的关系。主筛选取与M的两种平均相关性较大者，>0.8立即停止。没有候选触线；52周相关性低于0.3也不等于已有有效信息。常数信号日的相关性未定义，剔除该日而非填0。', '',
'相位：原日频只有一个日历相位，baseline.py没有多相位代码。额外3日整篮子调仓遍历phase0/1/2，按(UTC日数+phase)%K=0下单，其余日持有实际数量、照常逐日计价；与同频同相位旧三因子控制配对。省成本变体用同一机制把持仓最短锁定M=3/5/10天，枚举全部M相位，M3与稳健性测试复用。相位均值是各独立回测Sharpe的均值，不是多相位资金混合组合的Sharpe。不是错开几个起点，也不是每币错峰。', '',
'最佳因子按两种模式日频Sharpe均值挑选，通道位置20以1.4013胜出；选择记录在selection.json。锁定作用于整个策略篮子，包括混合模式另外两腿，因此特设同频旧三因子控制区分频率贡献。这里测试的是固定日历的最小持有期实现，未测试“任意一天突破立即入场、只在反向突破平仓”的另一种状态机。', '',
'统计：配对循环moving-block bootstrap，块长30日、49,999次、seed20261009，对候选/控制同日净收益同步重采样；统计量是年化Sharpe差。单侧H0: Δ≤0，p=(1+#(Δ*−Δ≥Δ))/(B+1)，观察Δ≤0设p=1。无i.i.d. t检验。Bonferroni上限 **11×2×(1+3+3+5+10)=484**，涵盖任何因子被选中的持有网格；不因未跑或M3重复而缩小。最小可报告p=0.00002，最小校正p=0.00968；本轮Sharpe检验远离阈值。', '',
'相位组p取各相位最差值，不挑最好相位。改善需超过配对Δ的相位SD且全部相位Δ>0；p_fwer<0.05；两子区间Sharpe均正且相对基线都改善；换手<1.427。按协议另要求同频配对控制也通过。四项任一失败判不可用。Sharpe持平的描述性发现预先设Δ≥−0.10且换手降≥20%，并报告95%区间；区间不是同时置信区间，未通过非劣性不得声称保住Sharpe。', '',
'## 3. 新因子与14日动量的横截面相关性', '',
'表内为逐日相关性的均值；SD是日间离散度。有效日数针对M Pearson，不是币×天独立样本数。完整分位数和逐日值在correlations.json及各correlation.csv。', '',
'|因子|M Pearson ± SD|M Spearman|M/SD Pearson|M/SD Spearman|有效日|筛选相关|处理|',
'|---|---:|---:|---:|---:|---:|---:|---|']
for r in corr:
    s=r['stats']; L.append(f"|{names[r['factor']]}|{s['M_pearson']['mean']:.4f} ± {s['M_pearson']['sd']:.4f}|{s['M_spearman']['mean']:.4f}|{s['MSD_pearson']['mean']:.4f}|{s['MSD_spearman']['mean']:.4f}|{s['M_pearson']['days']}|{r['screen_correlation']:.4f}|{'停止' if r['stopped'] else '继续'}|")
L+=['', '## 4. 每个日频变体结果', '',
'净Sharpe、Δ对原日频基线；两段列净Sharpe（括号为Δ）。ρ是净日收益与原基线的Pearson。p检验Sharpe提升，**不是“策略Sharpe是否大于0”**。所有配置判不可用，具体门槛布尔值保存在daily_analysis.json。', '',
'|因子|模式|Sharpe|Δ|日换手|年成本|2023–24（Δ）|2025–26（Δ）|ρ基线|p|p_fwer|判定|',
'|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|']
for r in daily:
    f=r['full'];v=r['vs_daily']; a=r['phase_summary']
    L.append(f"|{names[r['factor']]}|{mode[r['mode']]}|{f['sharpe']:.4f}|{v['delta']:+.4f}|{f['daily_turnover']:.4f}|{f['annual_cost']:.2%}|{r['2023-24']['sharpe']:.4f} ({v['subdelta'][0]:+.4f})|{r['2025-26']['sharpe']:.4f} ({v['subdelta'][1]:+.4f})|{v['correlation']:.4f}|{v['p']:.5f}|{v['p_fwer']:.4f}|不可用|")
    rows.append(dict(key=r['key'],factor=r['factor'],mode=r['mode'],period=1,phase=0,**f,delta=v['delta'],p=v['p'],p_fwer=v['p_fwer'],correlation=v['correlation'],usable=r['usable']))
L+=['', '### 全部3日调仓相位（附加稳健性）', '',
'这里Δ对同频旧三因子控制，不能把少调仓本身的改善归于因子。每行3相位全部保留，表中p/p_fwer为配对控制的最差相位值。', '',
'|因子|模式|Sharpe均值±SD|Sharpe范围|配对Δ均值±SD|日换手|前段 / 后段|p最差|p_fwer|',
'|---|---|---:|---:|---:|---:|---:|---:|---:|']
for r in daily:
    a=r['phase_summary']
    L.append(f"|{names[r['factor']]}|{mode[r['mode']]}|{a['sharpe']:.4f} ± {a['sharpe_sd']:.4f}|{pair(a['sharpe_range'])}|{a['matched_delta']:+.4f} ± {a['matched_delta_sd']:.4f}|{a['turnover']:.4f}|{pair(a['subsharpe'])}|{a['matched_p']:.5f}|{a['matched_p_fwer']:.4f}|")
L+=['', '## 5. 省成本重点：通道位置20 + 最小持有期', '',
'所有M相位均纳入，Δ对日频基线1.1293。10日混合版在phase1/3权益耗尽，**不计算仅存活相位的均值**，因此整体指标N/A且直接不可用。10日旧三因子控制也有2个失败相位，使10日单因子的同频配对结论不可识别；不能把缺失控制的相位删除。', '',
'|模式|M|Sharpe均值±SD|Δ|日换手|降换手|年成本|前段 / 后段|p最差|p_fwer|判定|',
'|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|']
for h in holds:
    a=h['summary']
    if a.get('invalid'):
        L.append(f"|{mode[h['mode']]}|{h['period']}|2/10相位权益耗尽|N/A|N/A|N/A|N/A|N/A|N/A|N/A|不可用|");continue
    L.append(f"|{mode[h['mode']]}|{h['period']}|{a['sharpe']:.4f} ± {a['sharpe_sd']:.4f}|{a['delta']:+.4f}|{a['turnover']:.4f}|{1-a['turnover']/base['full']['daily_turnover']:.2%}|{a['annual_cost']:.2%}|{pair(a['subsharpe'])}|{a['p']:.5f}|{a['p_fwer']:.4f}|不可用|")
L+=['', '相对同一因子的日频版，锁定期也有代价，不能只与较弱的原三因子基线比较：', '',
'|模式|M|同因子日频Sharpe|持有后Sharpe变化|相对同因子日频降换手|',
'|---|---:|---:|---:|---:|']
for h in holds:
    a=h['summary'];r=next(r for r in daily if r['factor']=='channel20' and r['mode']==h['mode'])
    if a.get('invalid'):
        L.append(f"|{mode[h['mode']]}|{h['period']}|{r['full']['sharpe']:.4f}|N/A：权益耗尽|N/A|");continue
    L.append(f"|{mode[h['mode']]}|{h['period']}|{r['full']['sharpe']:.4f}|{a['sharpe']-r['full']['sharpe']:+.4f}|{1-a['turnover']/r['full']['daily_turnover']:.2%}|")
L+=['', '同频控制帮助判断“只是少调仓”这一解释：', '',
'|M|旧三因子Sharpe均值±SD|旧三因子日换手|混合版配对Δ±SD|单因子配对Δ±SD|',
'|---:|---:|---:|---:|---:|']
for m in [3,5,10]:
    controls=[load(f'runs/baseline_mix_k{m}_p{p}.json') for p in range(m)]
    hm=next(h['summary'] for h in holds if h['mode']=='mix' and h['period']==m);hs=next(h['summary'] for h in holds if h['mode']=='single' and h['period']==m)
    if any(r.get('invalid') for r in controls):L.append(f'|{m}|2/10相位权益耗尽|N/A|N/A|N/A|');continue
    vals=[r['full']['sharpe'] for r in controls];turn=np.mean([r['full']['daily_turnover'] for r in controls])
    L.append(f"|{m}|{np.mean(vals):.4f} ± {np.std(vals,ddof=1):.4f}|{turn:.4f}|{hm['matched_delta']:+.4f} ± {hm['matched_delta_sd']:.4f}|{hs['matched_delta']:+.4f} ± {hs['matched_delta_sd']:.4f}|")
L+=['', '**关键判断：** 5日单因子相对日频Δ=+0.2111略大于自身相位SD=0.1939，但相对同频旧三因子只剩+0.0449，远小于配对相位SD=0.5242；2025–26平均Sharpe1.8857也低于日频基线2.0566。5日混合版Δ=−0.0084，点估计基本持平，区间仍太宽。10日单因子Sharpe范围0.6951–1.8399，相位敏感，且前段最差相位为负。', '',
'### 省成本但尚未证明保住Sharpe：单独列出', '',
'下表满足预先冻结的描述性门槛（Δ≥−0.10、换手降≥20%），**全部为证据不足，不是可用**。日频二元突破20/60也列入，不因未被选为最佳而隐藏。换手下降的配对块bootstrap均支持方向；表内95%区间未经多重校正。换手检验作为另一个指标，再用保守的2×484校正，其p_fwer均为0.01936。Sharpe差区间下端均低于−0.10，未获得非劣性证据。', '',
'|配置|Sharpe Δ|Δ的95%区间|换手下降/天|下降量95%区间|降换手|',
'|---|---:|---:|---:|---:|---:|']
for r in daily:
    v=r['vs_daily'];f=r['full']
    if v['delta']>=-.1 and f['daily_turnover']<=base['full']['daily_turnover']*.8:
        L.append(f"|{names[r['factor']]} {mode[r['mode']]} 日频|{v['delta']:+.4f}|{ci(v['delta_ci95'])}|{v['turnover_reduction']:.4f}|{ci(v['turnover_reduction_ci95'])}|{1-f['daily_turnover']/base['full']['daily_turnover']:.2%}|")
for h in holds:
    if not h['descriptive_cost_value']:continue
    a=h['summary'];L.append(f"|通道位置20 {mode[h['mode']]} M{h['period']}|{a['delta']:+.4f}|{ci(a['delta_ci95'])}|{base['full']['daily_turnover']-a['turnover']:.4f}|{ci(a['turnover_reduction_ci95'])}|{1-a['turnover']/base['full']['daily_turnover']:.2%}|")
L+=['', '二元突破单因子的低换手不能直接归因为突破状态持续。0/1信号有大量同分，原框架仍会把未突破的币按名字选进多头或空头；名字固定能机械地降低换手。实际诊断如下（按每日目标篮子统计）：', '',
'|二元信号|全截面常数日占比|多头实际创新高占比|空头实际创新高占比|',
'|---|---:|---:|---:|']
for r in verify['binary_diagnostics']:L.append(f"|{names[r['factor']]}|{r['constant_signal_day_fraction']:.2%}|{r['long_newhigh_fraction']:.2%}|{r['short_newhigh_fraction']:.2%}|")
L+=['', '这忠实保留了基线的同分处理，但削弱了“突破本身有效”的解释。改成随机同分、仅交易突破事件、对称高低突破，都属于新策略，未在本轮结果中偷换。', '',
'## 6. 长窗口的可交易集合影响', '',
'长窗口不能为短历史币伪造因子，因此有上市年龄筛选和早期空仓。下表同集合旧三因子保持原信号，用来识别宇宙变化；主结论没有改用这个更有利的基线。', '',
'|因子|首个有效信号日|有效交易目标日|不足8币日|同集合旧三因子Sharpe|候选混合版相对同集合Δ|',
'|---|---|---:|---:|---:|---:|']
for c in verify['coverage']:
    f=c['factor'];r=next(x for x in daily if x['factor']==f and x['mode']=='mix');u=load(f'runs/{f}_universe_k1_p0.json')
    L.append(f"|{names[f]}|{c['first_signal_date'][:10]}|{c['active_days']}|{c['zero_universe_days']}|{u['full']['sharpe']:.4f}|{r['vs_universe']['delta']:+.4f}|")
L+=['', '例如120日新高距离混合版Sharpe0.5300，同集合旧三因子1.4863，因子替换反而降低约0.9563；不能把长窗口减少可选币、空仓日增加造成的低换手当作额外信息。', '',
'## 7. 持有期每个相位明细', '',
'两段均为净Sharpe；p/fwer对原日频基线。ρ同样对原日频。失败相位N/A表示引擎拒绝继续计算，绝不表示收益为0。完整3日稳健性每相位记录在daily_analysis.json，所有每日收益与换手在runs/*.npz。', '',
'|模式|M|相位|Sharpe|Δ|日换手|年成本|前段 / 后段|ρ|p|p_fwer|',
'|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|']
for h in holds:
    for r in h['phases']:
        if r.get('invalid'):
            date=str(pd.to_datetime(r['failure_t'],unit='ms',utc=True))[:10]
            L.append(f"|{mode[h['mode']]}|{h['period']}|{r['phase']}|权益耗尽 {date}|N/A|N/A|N/A|N/A|N/A|N/A|N/A|");continue
        v=r['vs_daily'];f=r['full']
        L.append(f"|{mode[h['mode']]}|{h['period']}|{r['phase']}|{f['sharpe']:.4f}|{v['delta']:+.4f}|{f['daily_turnover']:.4f}|{f['annual_cost']:.2%}|{r['2023-24']['sharpe']:.4f} / {r['2025-26']['sharpe']:.4f}|{v['correlation']:.4f}|{v['p']:.5f}|{v['p_fwer']:.4f}|")
L+=['', '## 8. 明确结论与局限', '',
'- **可用：无。** 所有Sharpe提升检验都没有通过FWER；日频表现最好的通道20单因子还违反换手门槛。持有期配置不能同时通过两段改善、相位、显著性要求；10日混合版另有权益耗尽。',
'- **不可用：本轮22个日频变体与6个持有期配置。** M3与额外3日相位网格复用，不把它们计成新的独立发现。实际148个独立运行键=118个候选相位+19个日频/同频旧三因子控制+11个宇宙控制，其中4个运行失败（2候选、2控制）。',
'- **证据不足：省成本且保住Sharpe。** 有显著省换手的描述性配置，尤其5日持有；但未证明Sharpe非劣，不能升级为可用，也不能据此证明所有突破状态机都无效。', '',
'局限：本次基线可复现，但窗口/币集与提示附带参考不是同一版本；长窗口采用完整历史，混入可交易集合变化；52周日历定义与股票252交易日只以明确的两个口径处理；二元同分依赖名称排序；最佳因子同样本择优，没有独立留出期；30日块不能保证覆盖全部长期相关，区间是重采样近似；FWER保守且未覆盖项目历史130+方向的跨项目选择；两子区间长度与行情结构不同。', '',
'引擎局限原样保留：当前交易所数量精度映射历史、无资金费、无保证金/强平、无盘口冲击随交易规模变化、无订单拒绝/延迟、无终点强制平仓、100万美元初始权益。2.7倍目标gross下，固定数量长持可能使杠杆随权益下降显著上升；权益耗尽是原引擎报错，不是交易所真实强平路径。上市后内部无缺口不等于不存在幸存者偏差。', '',
'运行中首次遇到insolvent后只补了外层异常落盘与继续枚举逻辑，未更改原成交循环、失败相位的收益、判定门槛或择优规则。失败相位不计算存活均值。源码和输入哈希均未变化；未修改src，未commit/push/部署或操作实盘。', '',
'## 9. 复现与产物', '',
'在 `/Users/wonder/Code/stars` 使用指定 Python，依次执行：', '',
'```sh',
'PY=/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3',
'$PY research/breakout_20261009/run.py verify',
'$PY research/breakout_20261009/run.py screen',
'$PY research/breakout_20261009/run.py daily',
'$PY research/breakout_20261009/run.py phases',
'$PY research/breakout_20261009/run.py hold',
'$PY research/breakout_20261009/analyze.py',
'$PY research/breakout_20261009/verify.py',
'$PY research/breakout_20261009/write_report.py',
'```', '',
'run.py提取原baseline.py的定义段，成交循环只加非调仓日不下单的掩码与独立输出路径；原始基线数组已经复制到baseline_reference.npz。长计算按阶段和每个相位落盘，bootstrap按收益序列缓存；已有运行键会跳过，不应在改变输入或参数后复用旧缓存。原始基线复现日志、SPEC、selection、JSON分析、每日NPZ、bootstrap缓存、verification及results_daily.csv均在本目录。']
(H/'REPORT.md').write_text('\n'.join(L)+'\n')
pd.DataFrame(rows).to_csv(H/'results_daily.csv',index=False)
manifest={str(p.relative_to(H)):hashlib.sha256(p.read_bytes()).hexdigest() for p in H.rglob('*') if p.is_file() and p.name!='artifact_manifest.json' and '__pycache__' not in str(p)}
(H/'artifact_manifest.json').write_text(json.dumps(manifest,indent=2))
print('Wrote REPORT.md:',len(L),'lines')
