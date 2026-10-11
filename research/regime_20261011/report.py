"""Generate the complete reviewable report and machine-readable verdicts."""
import json,hashlib
import numpy as np
import pandas as pd
from common import *
from study import WINDOWS,DIRECTION
NAMES={'corr':'币间相关性','return_disp':'收益离散度','score_disp':'动量÷波动分数离散度','raw_mom_disp':'原始14日动量离散度','top5':'成交额Top5占比','hhi':'成交额HHI','universe':'合格宇宙数量','volume_count':'纯V≥500万数量','factor_ac':'动量组合收益自相关','neff':'实际持仓N_eff','realized_vol':'策略已实现波动（对照）'}

def num(x,d=3):return '—' if not np.isfinite(float(x)) else f'{x:.{d}f}'
def pct(x,d=1):return '—' if not np.isfinite(float(x)) else f'{x*100:.{d}f}%'
def table(headers,rows):return '\n'.join(['| '+' | '.join(headers)+' |','| '+' | '.join(['---']*len(headers))+' |']+['| '+' | '.join(map(str,row))+' |' for row in rows])+'\n'

def main():
    inf=pd.read_csv(OUT/'inference.csv');pol=pd.read_csv(OUT/'policy_metrics.csv');groups=pd.read_csv(OUT/'quintiles.csv');base=pd.read_csv(OUT/'baseline_daily.csv');feat=pd.read_csv(OUT/'features.csv');ths=pd.read_csv(OUT/'thresholds.csv');states=np.load(OUT/'policy_states.npz');returns=np.load(OUT/'policy_returns.npz')
    dates=base.date.to_numpy();dd=(dates>'2024-03-05')&(dates<='2024-10-15');inc=(dates>='2024-03-05')&(dates<='2024-10-15');peak=int(np.flatnonzero(dates=='2024-03-05')[0]);bm=stats(base.r)
    order=[f'{f}_{w}' for f,ww in WINDOWS.items() for w in [*ww,'avg']]
    def ir(c,p,t):return inf[(inf.candidate==c)&(inf.period==p)&(inf.test==t)].iloc[0]
    def pr(c,p='full',fee=.0007):return pol[(pol.candidate==c)&(pol.period==p)&(pol.fee==fee)].iloc[0]
    def qs(c,p='full',mode='online'):
        a=groups[(groups.candidate==c)&(groups.period==p)&(groups['mode']==mode)].set_index('quintile')
        return '/'.join(num(a.loc[q,'sharpe'],2)+('†' if 0<a.loc[q,'days']<20 else '') for q in [1,5])
    def rr(c,p='full'):return '/'.join(num(ir(c,p,f'corr{h}').effect) for h in [20,60])
    verdict=[];band=[];alarms=[]
    for f,ww in WINDOWS.items():
        c=f'{f}_avg';p=pr(c)
        significant=all(ir(c,'full',f'corr{h}').p_fwer_all_max<.05 for h in [20,60])
        signs=all(ir(f'{f}_{w}',period,f'corr{h}').effect*DIRECTION[f]<0 for w in [*ww,'avg'] for period in ['full','2023-24','2025-26'] for h in [20,60])
        early=p.pre20>=.5 and p.peak_bad==1 and p.coverage224>=.5
        econ=True
        for fee in [.0007,.00075,.00045]:
            v=pr(c,fee=fee);b=stats(returns[f'baseline@{fee}']);fixed=stats(returns[f'baseline@{fee}'][dd])['total']
            econ &= v.mdd-b['mdd']>=.1 and v.fixed_dd_return-fixed>=.1 and v.terminal_retention>=.9 and all(pr(c,z,fee).sharpe_delta>0 for z in ['2023-24','2025-26'])
        verdict.append(dict(family=f,significance=bool(significant),all_windows_and_subperiods_expected_sign=bool(signs),early_warning=bool(early),economic=bool(econ),passed=bool(significant and signs and early and econ)))
        for period in ['full','2023-24','2025-26']:
            for h in [20,60]:
                z=inf[(inf.candidate.isin([f'{f}_{w}' for w in ww]))&(inf.period==period)&(inf.test==f'corr{h}')]
                band.append(dict(family=f,period=period,horizon=h,rho_mean=z.effect.mean(),rho_sd=z.effect.std(ddof=0),phase_mean=z.phase_mean.mean(),phase_sd_across_windows=z.phase_mean.std(ddof=0),worst_p_fwer=z.p_fwer_all_max.max(),min=z.effect.min(),max=z.effect.max()))
    for c in order:
        p=pr(c);q=ths[c].to_numpy();x=feat[c].to_numpy()
        if c.startswith('neff_'):
            n=pd.read_csv(OUT/f'neff_gate_{c.rsplit("_",1)[1]}.csv');q=n.threshold.to_numpy();x=n.signal.to_numpy()
        ready=np.isfinite(q)&np.isfinite(x);earned_ready=np.r_[False,ready[:-1]];didx=np.flatnonzero(dd);b=states[f'{c}@0.0007'].astype(bool);earn=np.r_[False,b[:-1]]
        alarms.append(dict(candidate=c,ready_date=dates[np.flatnonzero(ready)[0]] if ready.any() else '',ready224=int(earned_ready[dd].sum()),bad224=int(earn[dd].sum()),bad225=int(earn[inc].sum()),pre20=p.pre20,pre60=p.pre60,peak_bad=p.peak_bad,first20=p.first20,first_bad=p.first_bad,bad_fraction=p.bad_fraction))
    pd.DataFrame(verdict).to_csv(OUT/'verdict.csv',index=False);pd.DataFrame(band).to_csv(OUT/'bandwidth_summary.csv',index=False);pd.DataFrame(alarms).to_csv(OUT/'alarm_summary.csv',index=False)
    text=[];add=text.append
    add('# 横截面 regime 指标：没有找到通过全部要求的提前预警信号\n')
    add('研究日期：2026-10-12。数据截止 2026-10-07。**没有找到能提前预警的指标**，这里指同时具备“校正后显著、能提前报警 2024 回撤、降低回撤且不破坏收益”的证据。不是说所有门控都没有样本内效果。\n')
    add('32 个单窗口、11 个历史分位带宽合成版本，共 43 个序列；联合检验 516 个假设。**没有一个全期未来 20/60 日相关通过全部 10/20/40 日块长的 FWER<0.05**；最小的稳健全期 p 也为 {min_corr_p}。加入 80/120 日块长后结论不变。全部门控 Sharpe 增益的三个要求块长最坏 FWER p 最低为 {min_policy_p}，不能把账面改善称为已验证的优势。\n')
    add('## 1. 先复现基线\n')
    add(table(['项目','复现结果'],[['收益日','2023-07-04 → 2026-10-07，1192 日'],['Sharpe','**1.1292998945 → 1.1293，通过**'],['算术年化 / CAGR','89.9400% / 79.3557%'],['累计净收益 / 终值（初始100万元）','573.8630% / 6,738,629.80'],['全期最大回撤','−80.8983093%'],['2023–24 / 2025–26 Sharpe','0.0101 / 2.0566'],['逐日收益 / 换手差','均为 0；日期完全相同'],['回撤口径','2024-03-05 调仓后净值 → 2024-10-15 调仓后净值；收益日 3/6–10/15，224日']]))
    add('使用 AST 提取原脚本初始化及函数，未 import 原 baseline，也未运行底部批处理。只在本研究目录输出。读过并遵循 [验证协议](../../docs/validation-protocol.md) 和 [回撤归因](../drawdown_attr_20261010/REPORT.md)。归因中的 BTC 方向、因子 IC 等结论直接作为前提，没有复测。\n')
    add('## 2. 定义、机制和无前视时钟\n')
    mechanisms={'corr':'共同波动增强可能让多空尾部同时受冲击、独立横截面机会减少；但相关性高也可能帮助市场中性对冲，方向并非必然。','return_disp':'收益分化不足时，价差机会可能不足以覆盖换手成本。','score_disp':'动量÷波动分数难以拉开，排名对微小变化更敏感。','raw_mom_disp':'去掉波动分母，检查信号离散度是否只是归一化尺度效应。','top5':'成交额集中于少量币，外围币的价格冲击及流动性风险可能增强。','hhi':'成交额份额的平方和，保留前五以外的集中信息。','universe':'原 target 合格币少，尾部组合可选币少、持仓容易集中。','volume_count':'用户指定的纯成交额门槛计数；低流动性覆盖可能增加组合集中和换手风险。','factor_ac':'动量组合收益不延续或反复翻转，近期押注可能失去持续性。','neff':'绝对名义权重集中，少数尾部事件更易主导净值。','realized_vol':'已发生的组合波动可能反映压力；是后视对照，不能自动解释为领先指标。'}
    add(table(['ID / 指标','窗口（日）','坏方向','机制'],[[f'`{f}` {NAMES[f]}','/'.join(map(str,ww)),'高' if DIRECTION[f]>0 else '低',mechanisms[f]] for f,ww in WINDOWS.items()]))
    add('宇宙逐日复用原 `target` 的 V≥500万美元、价格/14日历史/波动有效、排除 `:` 的过滤。`V` 是原引擎 **30日成交额均值再滞后1日**，不是当天成交额。相关性、截面离散度至少8币；相关性每对至少 max(20,ceil(0.8×窗口)) 个共同有效收益，再对有效对等权平均。三种离散度用样本标准差；平滑需完整有效窗口。Top5/HHI 以本地全体非 `:` 有正成交额币的当日 `close×volume` 为分母，**只代表此交易所数据覆盖，不代表全球市场**。成交额近似方式与原引擎相同。\n')
    add('`universe` 使用真正能进原策略的合格币数；另存 [universe_check.csv](universe_check.csv) 的纯 V 过滤数量。两者有362天不同，不能默默把“纯流动性数量”当成实际可交易宇宙。最终完整性审阅按用户指定口径补齐 `volume_count` 全部窗口及合成；与原 universe 一并纳入扩大后的516项联合校正。该修订发生在初轮结果之后，在PLAN.md明示，未改方向、阈值或挑选版本。\n')
    add('决策时点为 d 日开盘。价格/成交额特征最多用 d−1 日收盘；动量收益自相关和策略波动最多用 d−1 日开盘已经完成的收益。`factor_ac` 是同一精度/最低单/调仓带下、1x 动量÷波动多空组合的毛价格收益 lag-1 相关，完整窗口每天必须有持仓。`neff=1/Σ(|w|/gross)²` 使用上次实际成交数量在 d−1 收盘的名义权重，空仓为缺失；门控重放中用自己的真实数量重新计算，避免把基线集中度硬套到改变后的账本。策略已实现波动监测无门控影子基线。\n')
    add('预测标签为 **r[d+1]…r[d+h] 的复利净收益，h=20/60**；d 日调仓费没有伪装成未来收益。末尾不足 h 日剔除，子区间最后 h 日也剔除，避免结果跨年混入另一子区间。分位组 Sharpe 用 **下一日净收益**按 √365 年化，不把重叠的20/60日累计收益当独立日收益年化。所有相关都是 Pearson。\n')
    add('坏状态阈值固定：高坏 > 历史80分位，低坏 < 历史20分位；历史严格截至上一决策日，至少60个有效观测，缺值/未暖机维持2.7x。等号不触发。当天开盘触发即目标总名义1x，否则2.7x。每个反事实从原起点独立重放数量取整、10美元最低单、2%调仓带和真实切换成交费用。不是把基线每日收益直接乘1/2.7。\n')
    add('`avg` 是额外可执行的带宽合成：先把每个固定窗口转为仅用过去资料计算的历史分位，等权平均，要求全部窗口有效，再对合成值使用同样历史20/80分位。**它不是各窗口相关系数的算术平均，也不是各门控净值的平均。** 双重暖机可能使它比单窗口更晚可用；不以 avg 替代单窗口结果。真正的窗口统计等权平均及离散度见第5节。\n')
    add('## 3. 总表：所有窗口，不筛选\n')
    add('`ρ20/60` 是未来20/60日相关；`L/H` 是在线历史低20%/高20%组的下一日日收益 Sharpe；`p20/60` 是各期限 10/20/40 块长 FWER 的最大值（逐块长数值在第6节）。两个子区间栏也按“ρ20/ρ60；L/H”写。`提前/覆盖` = 峰值前20决策日报警率 / 224收益日保护覆盖率；`MDD/固定` = 门控全期最大回撤 / 原峰谷区间净收益。`净利Δ` = 门控累计净收益减基线573.863%，以百分点计，负值代表损失。所有门控本表费率为原0.0007。\n')
    rows=[]
    for c in order:
        p=pr(c);rows.append([f'`{c}`',rr(c),qs(c),'/'.join(num(ir(c,'full',f'corr{h}').p_fwer_required_max,4) for h in [20,60]),rr(c,'2023-24')+'; '+qs(c,'2023-24'),rr(c,'2025-26')+'; '+qs(c,'2025-26'),pct(p.pre20)+'/'+pct(p.coverage224),pct(p.mdd)+'/'+pct(p.fixed_dd_return),f'{(p.total-bm["total"])*100:+.1f}pp',pct(p.terminal_retention)])
    add(table(['指标×窗口','全期ρ20/60','全期L/H','FWER p20/60','2023–24：ρ；L/H','2025–26：ρ；L/H','提前/覆盖','MDD/固定','净利Δ','终值保留'],rows))
    add('日收益高低组是非连续子样本，Sharpe 只作条件表现描述，不代表一条含完整进出场费用的可交易择时策略。在线分位随历史变化，分组天数不保证正好20%；离散指标有大量并列值时尤其如此。†表示该组不足20天（推断不据此宣称显著）。完整五组、样本数、均值见 [quintiles.csv](quintiles.csv)。全样本分位只用于描述，绝不驱动杠杆。\n')
    add('## 4. 是否真的在2024事故前报警\n')
    add('覆盖列按产生收益的**前一日开盘**状态计数，不能拿损失当日收盘才知道的状态保护当天。`3/5开盘` 决定3/6价格暴露；峰值前20/60日不包含3/5。225日列对应用户字面日期包含3/5的额外核对，不能拿这225日收益冒充−80.9%的峰谷损失。就绪数是224日里已经有合法阈值的日数，未就绪也保留在覆盖率分母中。\n')
    rows=[]
    for a in alarms:
        rows.append([f'`{a["candidate"]}`',a['ready_date'],a['first_bad'],pct(a['pre20'])+'/'+pct(a['pre60']),'是' if a['peak_bad'] else '否',f'{a["bad224"]}/224',f'{a["bad225"]}/225',a['ready224'],pct(a['first20']),pct(a['bad_fraction'])])
    add(table(['候选','首个合法阈值日','首次报警日','峰前20/60日','3/5开盘','224日覆盖','225日覆盖','就绪日数','首20亏损日覆盖','全期降杠杆率'],rows))
    add('**集中度的若干单窗口确实提前亮灯，不能笼统说全部来不及。** Top5的20日窗口在峰前20日全部报警、3/5仍报警，保护194/224日，最大回撤从−80.90%降至−50.37%，固定峰谷亏损降至−49.35%，累计净收益从573.86%升至679.35%。但全期相关的 FWER p 为1.0000，2025–26 Sharpe 从2.0566降至1.8854；好年份终值仅保留46.19%。不能据此选中20日窗口。\n')
    add('HHI的10日窗口样本内最大回撤为−56.59%，累计净收益937.18%，两个子区间Sharpe都提高（0.7051/2.1456）。**这是真实报告的样本内好结果**，但全期未来20/60日相关分别+0.0286/+0.0474（与预定“高集中坏”方向相反），FWER均为1；其收益改进也没有通过FWER检验。它说明“没有找到可靠预警”不等于“所有数值都变差”。不能因该窗口漂亮而忽略全家族。\n')
    add('N_eff带宽合成最大回撤−67.44%、累计净收益1012.23%，但峰前20/60日均未报警，首次亮灯2024-05-05，覆盖105/224日（46.88%），全期两期限FWER均1。**它是本样本中的后期减损，不是对3月事故的提前预警。** 这与已知归因一致：前半段大损失发生在宇宙和N_eff尚不很小的时候。\n')
    add('## 5. 相位与带宽：平均全部结果\n')
    add('对每个 h 使用原始日索引 mod h 的全部 h 个非重叠相位，逐相位算相关，再等权平均；每相位至少5个有效点，否则显示缺失。相位之间共享总体行情，不是 h 个独立验证集。窗口统计平均也对所有预定窗口等权；不从中挑峰值。“窗口ρ均值±SD”和“相位ρ均值”均是描述，不另外冒充独立显著性检验。每个相位的点数/相关见 [phases.csv](phases.csv)，每个窗口的相位均值、SD、最小/最大和可用相位数见 [inference.csv](inference.csv)。\n')
    rows=[]
    for f in WINDOWS:
        for period in ['full','2023-24','2025-26']:
            vals=[next(z for z in band if z['family']==f and z['period']==period and z['horizon']==h) for h in [20,60]]
            rows.append([f'`{f}`',period,*[num(z['rho_mean'])+'±'+num(z['rho_sd']) for z in vals],'/'.join(num(z['phase_mean']) for z in vals)])
    add(table(['家族（全部单窗口）','区间','20日：窗口ρ均值±SD','60日：窗口ρ均值±SD','全部相位均值20/60'],rows))
    add('带宽合成自身的相位检查：\n')
    rows=[]
    for f in WINDOWS:
        c=f'{f}_avg'
        for period in ['full','2023-24','2025-26']:
            zs=[ir(c,period,f'corr{h}') for h in [20,60]]
            rows.append([f'`{c}`',period,*[f'{num(z.phase_mean)}±{num(z.phase_sd)} [{num(z.phase_min)},{num(z.phase_max)}]; {int(z.phase_n)}/{h}' for z,h in zip(zs,[20,60])]])
    add(table(['带宽合成','区间','20日：相位均值±SD [min,max]；可用数','60日：相位均值±SD [min,max]；可用数'],rows))
    add('## 6. FWER：完整块长结果与统计边界\n')
    add('冻结方案 [PLAN.md](PLAN.md) 在候选结果生成前写下，但已知事故背景，因此不是外部预注册。测试家族为43序列×3区间×（未来20日相关、未来60日相关、在线高低组Sharpe差、门控相对基线Sharpe差）=**516**。双侧、single-step max|T|，同时对所有窗口/期限/区间校正；无效假设赋p=1。每个块长9999次，固定seed=20261011+块长，Monte Carlo p=(1+超越次数)/10000，最小分辨率0.0001。检验家族含全部已算候选，不仅表中看起来好的那几条。\n')
    add('具体实现是**零假设中心化的相关系数/Sharpe影响函数循环移动块bootstrap**：每次所有假设共用同一组连续日期块，精确截取1192个日期；用bootstrap误差的标准差标准化，单次最大绝对T同时覆盖全部假设。相关的影响函数为 zx·zy−ρ(zx²+zy²)/2；Sharpe用均值/方差的delta影响函数，条件样本按有效样本比例加权，中心化误差不携带观测效应。这是渐近线性bootstrap，**不是逐次完整重估相关的精确有限样本检验**，也不是外部随机化实验。代码/数值导数核验见 [inference.py](inference.py)、[audit.json](audit.json)。\n')
    add('maxT联合校正及时间序列重采样的依据见 [Romano与Wolf原论文](https://www.ssc.wisc.edu/~bhansen/718/RomanoWolf2005.pdf)。本次使用单步法，未声称实现论文更有力的逐步算法。块bootstrap的渐近有效性需要适当弱依赖/近似平稳性，本样本显然存在结构变化，因此p只能在这些假设下解读。\n')
    add('10/20/40是用户要求的全量结果；60日标签高度重叠，额外固定80/120作更长依赖敏感性。结论要求三个指定块长全部显著，且额外长块不推翻，不以最小p挑块长。下面每格是**20日p / 60日p**。\n')
    rows=[]
    for c in order:
        for period in ['full','2023-24','2025-26']:
            rows.append([f'`{c}`',period,*['/'.join(num(ir(c,period,f'corr{h}')[f'p_fwer_{L}'],4) for h in [20,60]) for L in [10,20,40,80,120]]])
    add(table(['候选','区间','块10','块20','块40','块80','块120'],rows))
    add('全期无显著关系，但部分子区间关系显著，不能隐去：Top5、HHI部分版本在2023–24与未来60日收益显著负相关；宇宙数量三个原始窗口也是显著**负**相关，方向反而违背“小宇宙坏”的预定机制；已实现波动原始20/60窗口在该子区间有显著负相关。它们未稳定延续至2025–26。全部516项的未校正p、各块长校正p和效应量保存在 [inference.csv](inference.csv)，包含分组差异及门控Sharpe增益，不能把它们与相关显著性混为一谈。\n')
    add('## 7. 收益代价、费用与两个子区间\n')
    add('以下全列32单窗口及11合成，不只列改善者。原费率0.0007；“ΔSharpe”相对各子区间原基线。maker敏感性0.00045=0.00015+0.00030滑点；taker敏感性0.00075=0.00045+0.00030滑点。所有费率下都重跑成交；maker列仅是费率场景，**未假定现实可全部maker成交**。分位条件Sharpe不能替代此表执行结果。\n')
    rows=[]
    for c in order:
        p=pr(c);rows.append([f'`{c}`',num(p.sharpe,4),'/'.join(num(pr(c,z).sharpe,4) for z in ['2023-24','2025-26']),'/'.join(f'{pr(c,z).sharpe_delta:+.3f}' for z in ['2023-24','2025-26']),num(p.turn),pct(p.annual_cost),num(p.daily_return_corr),pct(pr(c,fee=.00045).mdd)+'/'+pct(pr(c,fee=.00075).mdd),pct(pr(c,fee=.00045).total)+'/'+pct(pr(c,fee=.00075).total)])
    add(table(['候选','全期Sharpe','子区间Sharpe','子区间ΔSharpe','日均换手','年化成本','与基线日收益ρ','maker/taker MDD','maker/taker累计净收益'],rows))
    add('对应每档费用的全期/两个子区间净收益、算术年化、CAGR、Sharpe、回撤、换手、成本、终值保留以及固定峰谷亏损完整保存于 [policy_metrics.csv](policy_metrics.csv)。终值保留=门控终值/相同费用基线终值；总表净利Δ以同一初始本金计算，二者不能混用。基线日均换手1.59765倍、年化成本40.8200%；大量年化成本是原2.7x账本的绝对成交额口径，不除2。\n')
    add('## 8. 判定与下一步规格\n')
    add('冻结通过标准：全期两个期限在所有块长校正显著；所有单窗口及两个子区间方向符合机制；带宽合成峰前20日报警≥50%、3/5已报警且224日覆盖≥50%；全期及固定峰谷回撤改善≥10个百分点、终值保留≥90%、两个子区间Sharpe都提高，三档费用不反转。这些经济阈值是明确的研究门槛，不是统计定理。即使不使用这些额外经济门槛，**全期相关和门控收益增益的显著性已经失败**，所以最终否决不依赖调严门槛。\n')
    add(table(['家族','显著','全部窗口/区间方向','提前报警','经济效果','全部通过'],[[NAMES[z['family']],*['是' if z[k] else '否' for k in ['significance','all_windows_and_subperiods_expected_sign','early_warning','economic','passed']]] for z in verdict]))
    add('**没有找到。没有任何获准交给策略修改/实盘的指标规格。** 已把所有未通过版本的精确定义、阈值、时钟、状态、成本与重放代码保存，便于复核；这些是研究重现规格，不是可部署建议。不能改成“只用HHI10”“只在2023–24有效”“挑一个更好分位”来补救本轮失败。若后续研究改变阈值、报警持续期或加入尾部共同暴露，必须建立新检验家族，并以未来未见数据验证，而不是在本次已知回撤上继续筛选。\n')
    add('## 9. 局限\n')
    add('1. 仅一次被事先点名的大回撤；本次所有经济改善都是已知事故上的回放，缺少独立未来危机验证。子区间不是全新的样本外。FWER只覆盖本轮516项，未覆盖此前研究和未来继续搜索。\n2. 1192日中早期有较长空仓期，不同指标暖机/缺失造成比较支持集不同。全期相关按各自可用日计算，并额外在 [inference.csv](inference.csv) 报活跃目标持仓日相关；不能把有效样本更多等同证据更强。合成二次暖机使corr/离散度/N_eff等无法在3月初就绪；原始窗口也完整披露，不能只用合成的晚启动去否定它们。\n3. 扩展历史的分位会受早期小宇宙/低波动影响。宇宙门控原始窗口在该回撤里零覆盖，就是在如实使用当时历史阈值，不是证明少币绝对安全；没有事后改成全样本分位、固定币数或滚动短历史。\n4. 非平稳、长依赖、少数极端收益和有效非重叠样本很少，使渐近影响函数maxT推断有限。尤其部分短子区间60日相位连5个点都不足；显示缺失而非伪造精度。没有对宏观时间趋势做另行去趋势选择，也不把浓度的子区间相关称为因果。\n5. 基线数据可能有存活者、上市覆盖、历史修订和当前meta精度回用历史偏差。当前资格宇宙回看历史收益是决策时可做的计算，但短历史资产的相关覆盖不同；逐日共同样本对数在correlation_coverage.csv。\n6. 只包含原引擎固定费用及两档敏感性，不含资金费、真实冲击容量、盘中强平/保证金和逐笔成交延迟。日线开盘成交遵循冻结基线，不能证明现实能在刚收盘后的同一价格无延迟执行。\n7. 高低分位Sharpe含基线下一日收益及下一日调仓费用，是条件诊断；直接用它拼接投资策略会漏掉切换成本。只有完整门控重放用于收益/回撤判断。\n8. 本轮预定坏方向失败后没有反向挑选。高相关、高离散、集中度变化的经济解释可能有多种，本报告只检验写定的机制，不声称排除所有可能定义。\n')
    add('## 10. 审计与重现\n')
    add('已通过：[audit.json](audit.json)。包含原AST成交循环在交替门控与N_eff真实门控日程下逐日一致；2024-03-06、2024-07-17、2025-07-01三处物理截断价格/成交额后全部43特征、在线分位、内生N_eff状态与完整历史前缀一致；阈值严格排除当天/未来；未来标签索引；影响函数数值导数；FWER不小于原始p；所有输入SHA-256未变。原源码、baseline、其他研究输出未改；没有commit/push/部署或实盘访问。\n')
    add('复现时从 `/Users/wonder/Code/stars` 依次运行（指定Python，无新增第三方依赖）：\n\n```sh\n/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/regime_20261011/common.py\n/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/regime_20261011/study.py\n/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/regime_20261011/inference.py\n/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/regime_20261011/audit.py\n/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/regime_20261011/report.py\n```\n')
    add('主要文件：[冻结方案](PLAN.md)、[基线门槛](baseline_gate.json)、[输入哈希](input_manifest.json)、[指标与执行](study.py)、[联合检验](inference.py)、[审计代码](audit.py)、[逐日指标](features.csv)、[在线分位](online_ranks.csv)、[逐日阈值](thresholds.csv)、[报警汇总](alarm_summary.csv)、[带宽统计](bandwidth_summary.csv)、[全量统计](inference.csv)、[分组](quintiles.csv)、[所有相位](phases.csv)、[门控收益及费用](policy_metrics.csv)、[最终判定](verdict.csv)。policy_returns.npz / policy_states.npz 保留所有费率日收益与开盘门控状态，bootstrap_maxT_*.npy保留每次联合最大统计量。\n')
    add('### 补表：全样本描述性最高/最低20%组\n\n此表满足直接按各期指标水平分位分组的阅读口径，使用该期样本内分位，因此仅用于描述，不参与阈值选择。顺序仍为低/高Sharpe；在线版本在总表。完整5组及样本数见quintiles.csv。\n')
    add(table(['候选','全期低/高','2023–24低/高','2025–26低/高'],[[f'`{c}`',*[qs(c,p,'ex_post') for p in ['full','2023-24','2025-26']]] for c in order]))
    report_text='\n'.join(text).replace('{min_corr_p}',f"{inf[(inf.period=='full')&inf.test.str.startswith('corr')].p_fwer_required_max.min():.4f}").replace('{min_policy_p}',f"{inf[inf.test=='policy_sharpe_gain'].p_fwer_required_max.min():.4f}")
    (OUT/'REPORT.md').write_text(report_text)
    files=sorted(p for p in OUT.iterdir() if p.is_file() and p.name not in ['output_manifest.json','report.log'])
    (OUT/'output_manifest.json').write_text(json.dumps({p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in files},indent=2))
    print('REPORT READY; passed:',sum(z['passed'] for z in verdict),'bytes',(OUT/'REPORT.md').stat().st_size,flush=True)
if __name__=='__main__':main()
