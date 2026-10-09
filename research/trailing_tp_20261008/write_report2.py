from pathlib import Path
import json,hashlib,datetime
import numpy as np
R=Path(__file__).resolve().parent;S=R/'full_grid2';rows=json.loads((S/'results.json').read_text());audit=json.loads((R/'grid2/audit.json').read_text());control=json.loads((S/'controls_summary.json').read_text());hour=json.loads((S/'hour_window.json').read_text());ref=json.loads((S/'original_reference.json').read_text())
def date(t):return datetime.datetime.fromtimestamp(t/1000,datetime.timezone.utc).strftime('%Y-%m-%d')
def pair(x,y,fmt='.3f'):return ' / '.join(f'{z:{fmt}}' if np.isfinite(z) else 'N/A' for z in [x,y])
def label(x):return f"{int(x['activation']*100)}% | {int(x['distance']*100)}% | {'逐币' if x['mode']=='coin' else '全仓'}"
text=['# 移动止盈网格：REPORT2',
'**判定：18 组均不可用。** 没有配置满足四条固定标准；完整 1343 日的日调仓结果在两个情景下，2023–24 Sharpe 全部为负。逐币悲观结果明显恶化；全仓部分情景有改善，但没有通过相位检验的多重校正。日调仓完整样本的配对显著性、全仓真实盘中上下界仍证据不足，不能把“未通过使用门槛”解释成已证明任何移动止盈都无效。',
'## 数据与输入冻结',
'已逐文件核实日线：234 币、188,833 根完整记录、229 币 ≥100 根，上市后至最后观测之间 **0 个内部缺口**。5 个不足100根的是 CASHCAT(89)、GRAM(98)、JELLY(56)、PONS(38)、USELESS(30)，与用户已经核实的新上市名单一致；不把上市前无记录当缺口，也不声称独立取得了上市公告。原始并集从2023-01-01起，早期部分币有记录；完整回测采用原75行基线的 `T < end` 条件、预热33个并集日期，收益日期实际是 **2023-02-03 → 2026-10-07，1343日 = 698 + 645**，含早期零仓位日。这与“2023-04起”的概述不完全一致，但没有事后裁剪原基线样本。',
'小时线本次快照234文件、395,206根完整记录，179币≥100根，55币为空，内部0缺口。229币≥100根的核实结论适用于日线，不能套用于小时线。快照、逐币起止、哈希见 `grid2/audit.json` 与 `grid2/data_manifest.json`；小时验证另有 `full_grid2/hour_manifest.json`。仍在外部抓取的目录不被视为永久固定输入。',
'**并发改写造成的限制：** 本轮先读取原75行 `baseline.py` 和原1343日 `baseline_results.json`。计算期间该脚本被另一写入方增加 `daily_start_ms` 过滤，原名基线NPZ/JSON被覆盖为1192日（2023-07-04起）结果。它不是用户给定的1.1766基线。已冻结两版加载器：`grid2/frozen_full_baseline.py` 恢复最初读到的过滤表达式，`grid2/frozen_baseline.py` 保存被截断版；候选完整重算到 `full_grid2/`。**没有重跑原日调仓基线，也没有尝试复现1.62。** 原1343日基线逐日收益已不可读，因此主表的原日调仓配对p和相关性如实标为N/A。原始标量引用保存于 `full_grid2/original_reference.json`，不是拿1192日p冒充1343日p。',
'## 基线（已复核现存数字与口径，未重跑）',
'|基线|全期 Sharpe|2023–24|2025–26|核实来源|','|---|---:|---:|---:|---|',
'|cap5_open_0.0007|1.176562|−0.246602|2.484797|本轮开始读取的原1343日JSON|',
'|cap5_close_0.0007|1.181562|−0.233924|2.480335|原REPORT2及JSON引用|',
'|cap8_open_0.0007|1.143721|−0.094688|2.293708|原REPORT2及JSON引用|',
'|用户提供的cap5 open子区间引用|1.1766|−0.2767|2.4452|这两段在原文件实际属于fee=0.00075（全期1.141721）|',
'窗口20日含当日、成交额30日均值移位1日、14日动量、流动性5e6、gross2.7、cap5、floor数量精度、$10最小订单、2%调仓带沿用基线；无资金费、无终点强制平仓。原7bp基线日换手1.4270、年化成本36.46%。后续Δ采用已复核7bp对应的−0.246602/2.484797，不能把7.5bp子区间混入7bp主基线。',
'## 规则与路径情景',
'逐币独立：按实际数量、入场名义价格收益跟踪，多头用high形成有利峰值，空头用low形成有利峰值；阈值为未杠杆化单币价格收益，回撤为有利峰价的比例：多头止盈线=最高峰价×(1−距离)，空头止盈线=最低峰价×(1+距离)。新增同向数量时按数量加权入场价，保留并重新表示历史有利峰价，重新检查激活；减仓不重置，反手/清仓后重入重置。增加仓位后激活按新的加权入场价重新检查，此状态规则已明示。',
'逐币悲观：当日high/low构成有利峰值后，使用不利极值检查触发，多头以low、空头以high计成交；乐观：用close检查并以close成交。两者都另收7bp。**悲观极值成交是额外保守的压力假设，不能伪称精确止盈价或真实成交证据；close情景也不是严格的最佳可能路径。** 尤其同日新激活时，OHLC仍不知道high/low顺序；这里报告两种明确情景，不能保证覆盖所有真实路径及组合Sharpe的数学上下界。账户耗尽时吸收为破产：开盘耗尽当日计−100%，盘中耗尽在下一记账日计−100%，随后零仓，不删除失败配置；不模拟保证金强平。',
'全仓统一：初始全仓净值作为激活分母，净值峰值**只在每日同步close更新**，持续跨调仓；出场后下一可调仓时重置新一轮。净值=已结算权益+全部剩余数量按同一时刻价格重估的PnL，手续费计入净值。悲观多检查一次同步open相对前一close峰值的回撤，然后检查close；乐观只检查close。**没有把各币high/low拼成组合峰值或组合低点。** 因此全仓两情景只覆盖开盘/收盘采样差异，不能识别两者之间的真实全仓峰值；该项盘中证据不足。',
'止盈后到该币下次计划调仓之前数量保持0：日调仓次日可重新参与；错峰是该币下次轮到的日期。全仓触发平掉所有有可用同步报价的腿，缺价腿保留。7bp = 0.045% taker费 + 0.025%滑点，按成交名义收单边；重开再收费，反手两边收费。成本和换手按发生日记录；止盈PnL计入下一开盘到开盘收益，与原基线收益时钟配合，末日盘中退出不进入下一日收益，保留“不终点强制平仓”口径。',
'## 完整样本：9×2主表（每格悲观 / 乐观）',
'Δ均为相同完整样本原7bp基线之差，子区间括号也是Δ。**p=N/A是原日调仓基线逐日序列缺失，绝不是p=1。** 下节给出可实际配对的完整样本三相位p。',
'|激活|回撤|模式|Sharpe 悲/乐|Δ基线 悲/乐|2023–24 Sharpe（Δ）悲/乐|2025–26 Sharpe（Δ）悲/乐|p|p_fwer|',
'|---|---|---|---:|---:|---|---|---|---|']
for x in rows:
 a=x['paths']['pess'];b=x['paths']['opt'];subs=[]
 for j in range(2):subs.append(' / '.join(f"{z['sub'][j]['sharpe']:.3f} ({z['sub'][j]['delta']:+.3f})" for z in [a,b]))
 text.append(f"|{label(x)}|{pair(a['sharpe'],b['sharpe'])}|{pair(a['delta'],b['delta'])}|{subs[0]}|{subs[1]}|N/A|N/A|")
text+=['两端直接列出，没有只报乐观。逐币9组在两端的Δ都为负。全仓10%/15%为1.239/1.166，相对基线+0.063/−0.011，方向翻转；20%/15%也翻转。其他全仓均未满足两段为正。情景名称不保证Sharpe大小关系：退出时点不同，风险变化能让“悲观”Sharpe更高。',
'## 全部调仓相位与实际配对检验',
'原基线是每天全量调仓，只有一个日历相位；删掉首日/错开回测开始几天不能冒充完整多相位。另按 `src/trader.rs::slice_of`：币名字节和mod3， `(UTC day + slot + phase) mod 3 == 0`，跑三档错峰phase=0/1/2，所有候选与各自无止盈错峰控制配对；这些是**额外稳健性试验，不把错峰改善算成止盈改善**。三档无止盈控制Sharpe分别为 '+', '.join(f"{z['sharpe']:.4f}" for z in control)+f"，均值{np.mean([z['sharpe'] for z in control]):.4f}、相位SD{np.std([z['sharpe'] for z in control],ddof=1):.4f}。这本身说明相位不可忽略。",
'每个相位使用配对循环moving-block bootstrap，块30日、9999次、seed20261009、相同索引同时抽候选和控制。检验统计量为Sharpe差；单侧零假设Δ≤0，p=(1+次数[bootstrapΔ−观测Δ ≥ 观测Δ])/(B+1)。不是i.i.d. t检验。块长度固定，未挑选有利块长；30日也不能保证消除所有长期相关。',
'原规划18候选×4执行配置（日调仓+3相位）×2路径=144；输入被改写后另完成原面板，**总探索族保守扩大到288**，Bonferroni `min(1,288p)`，不降低0.05门槛。两路径均列出，三相位检验要求全部通过；下表p取三个相位的最大值，是“全部相位改善”严格规则的报告值，各相位逐项p、子区间、相关性在 `full_grid2/results.json`。日调仓N/A不能被该检验替代。',
'|激活|回撤|模式|相位平均Sharpe±SD 悲/乐|相位平均Δ±SD 悲/乐|p（三相位最大）悲/乐|p_fwer（三相位最大）悲/乐|',
'|---|---|---|---|---|---|---|']
for x in rows:
 a=x['paths']['pess'];b=x['paths']['opt']
 text.append(f"|{label(x)}|"+' / '.join(f"{z['phase_mean_sharpe']:.3f}±{z['phase_sd_sharpe']:.3f}" for z in [a,b])+'|'+' / '.join(f"{z['phase_mean_delta']:+.3f}±{z['phase_sd_delta']:.3f}" for z in [a,b])+f"|{pair(a['phase_p_max'],b['phase_p_max'],'.5f')}|{pair(a['phase_p_fwer_max'],b['phase_p_fwer_max'],'.3f')}|")
text+=['全部配置的最坏相位FWER为1.000，没有任何配置通过。相位平均Δ必须大于其相位SD；不能用绝对Sharpe的相位SD代替Δ噪声，也不能凭某一相位挑出赢家。另保存1192日被截断快照结果于 `grid2/results.json`；它不是主结论的样本，也不是缺失原基线的替身；该探索使用入场名义百分点距离，作为已尝试的额外变体计入288族，最终完整样本采用标准峰价比例距离。',
'## 成本、换手与基线相关性',
'主样本原日调仓逐日序列不可恢复，所以日调仓相关性N/A。下表同时给主日调仓实际名义换手、成本，及三相位候选与**匹配无止盈错峰基线**的平均收益相关性；两类不混同。三相位逐币数值见JSON。',
'|激活|回撤|模式|日换手 悲/乐|年化交易成本 悲/乐|三相位平均相关性 悲/乐|',
'|---|---|---|---|---|---|']
for x in rows:
 a=x['paths']['pess'];b=x['paths']['opt'];text.append(f"|{label(x)}|{pair(a['turnover'],b['turnover'])}|{pair(a['annual_cost']*100,b['annual_cost']*100,'.2f')}%|{pair(a['phase_mean_correlation'],b['phase_mean_correlation'])}|")
text+=['成本是fee×交易名义/当时权益的日均×365，不是额外从已扣费净收益中再扣一次。极端压力情景的权益缩小会使成本比例上升；破产后的零收益拉低全期均值，不能解读为可交易收益。未提供maker结果，因为本任务是taker主动止盈。',
'## 小时线触发核对（短样本）',
f"使用真实candleSnapshot小时OHLC，同一规则沿小时重新跟踪；窗口 **{date(hour['first'])} → {date(hour['last_exclusive']-86400000)}，{hour['days']}完整日**。{hour['symbols_with_any_hourly']}币有小时线，其中{hour['full_hourly_symbols']}币覆盖完整窗口。窗口外用日线情景保持既有状态，只解读窗口内结果。所有18组×2情景都实际跑过，保存在 `full_grid2/hour_*.npz/.json`。下面是窗口内日线与小时线Sharpe及退出腿数（悲/乐）；数值不参与全样本显著性选优。",
'|激活|回撤|模式|日线窗口Sharpe 悲/乐|小时线窗口Sharpe 悲/乐|日线退出腿数 悲/乐|小时线退出腿数 悲/乐|',
'|---|---|---|---|---|---|---|']
for x in rows:
 vals=[];counts=[]
 for source in ['s1_p0','hour']:
  vs=[];ns=[]
  for path in ['pess','opt']:
   key=f"{source}_{x['activation']}_{x['distance']}_{x['mode']}_{path}";v=np.load(S/(key+'.npz'));mask=(v['t']>=hour['first'])&(v['t']<hour['last_exclusive']);r=v['r'][mask];vs.append(float(np.mean(r)/np.std(r,ddof=1)*np.sqrt(365)) if np.std(r,ddof=1)>0 else float('nan'));ev=json.loads((S/(key+'.json')).read_text())['events'];ns.append(sum(hour['first']<=e[0]<hour['last_exclusive'] for e in ev))
  vals.append(pair(*vs));counts.append(pair(*ns,'.0f'))
 text.append(f"|{label(x)}|{vals[0]}|{vals[1]}|{counts[0]}|{counts[1]}|")
text+=['这是**真实小时价格数据的触发重放，不是成交回报验证**。它能证明给定规则在小时分辨率有实际报价和触发、量化日线和小时触发差异，并将全仓峰值更新为同步小时close；不能消除同一小时high/low顺序歧义、证明止盈单能在指定价成交、验证盘口冲击/实际滑点、验证2023–24或跨行情稳健性。无小时线的退市腿保留陈旧报价；逐币OHLC触发仍是情景而非tick级真实订单执行。',
'## 固定判定与局限',
'|事前条件|本次结果|','|---|---|',
'|p_fwer < 0.05|完整样本三相位全部未通过；原日调仓配对p缺失，不能算通过|',
'|两段净Sharpe都为正且同向改善|全部18组的日调仓两情景早期段均负，明确失败|',
'|改善大于相位噪声|部分全仓平均Δ可超过SD；不能抵消其他条失败|',
'|悲观下改善仍为正且结论不翻转|逐币全部失败；部分全仓两端方向翻转|',
'**按使用门槛明确判定：全部不可用。** 原日调仓统计证据和全仓真实盘中路径证据不足，所以不能宣称对真实移动止盈规则完成了严格否定检验，更不能部署。负的早期基线没有被藏起来，也未改成“只要求优于负基线”以便过关。',
'其他局限：当前交易所元数据精度映射到历史；幸存者/退市市场历史不完整；无资金费按用户基线保留；缺报价持仓继续沿用最后报价，上市后内部0缺口不等于所有持仓都有可成交报价；无保证金强平模型；日线极值压力成交与真实stop-market执行有距离；close检查延迟会改变风险分布；样本没有独立留出集；9999次bootstrap的最小p=0.0001，288重校正后最小FWER=0.0288，能够分辨0.05门槛；近门槛结论仍受蒙特卡洛误差影响，实际各配置未校正p远大于通过阈值。如有边界候选，应提高重复次数而不是把零次尾部当p=0；本次没有边界候选。',
'## 可追溯产物与复跑',
'指定Python和工作目录一直用于研究计算。原基线默认入口未执行；`baseline.py --trailing-grid-audited` 新增完整网格入口；新增研究代码从冻结基线读取面板及target/精度/调仓公式，不运行原 `run`。原始数据哈希、预声明、每配置收益/交易腿、三相位控制、统计JSON均分段保存。`src/`未由本轮修改，无commit/push/部署/实盘操作。工作区原有 `src/live.rs` 修改保持原样。验证已通过：144条候选+36条小时重放均为1343日、收益有限且不低于−100%，没有同币同日重复退出；详见 `full_grid2/verification.json`。两个悲观错峰配置发生破产，明确保留。',
'复跑（已存在配置会跳过；要更改模型必须换新输出目录或明确清理旧配置）：',
'```sh\ncd /Users/wonder/Code/stars\n# 单入口可用 baseline.py --trailing-grid-audited；下面分段执行\n/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/trailing_tp_20261008/trailing_grid_full.py\n/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/trailing_tp_20261008/hour_full_grid2.py\n/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/trailing_tp_20261008/stats_full_grid2.py\n/Users/wonder/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3 research/trailing_tp_20261008/write_report2.py\n```']
report='\n\n'.join(text)+'\n'
# Markdown table rows need adjacent lines.
lines=report.splitlines();out=[]
for line in lines:
 if line=='' and out and out[-1].startswith('|'):continue
 out.append(line)
report='\n'.join(out)+'\n'
(R/'REPORT2.md').write_text(report);(S/'REPORT2.frozen.md').write_text(report)
manifest={str(f):hashlib.sha256(f.read_bytes()).hexdigest() for f in [R/'baseline.py',R/'trailing_grid.py',R/'trailing_grid_full.py',R/'hour_full_grid2.py',R/'stats_full_grid2.py',R/'write_report2.py',R/'grid2/frozen_baseline.py',R/'grid2/frozen_full_baseline.py',R/'grid2/frozen_control.npz',Path('src/trader.rs'),Path('docs/validation-protocol.md')]}
(S/'implementation_manifest.json').write_text(json.dumps(manifest,indent=2))
print('REPORT2 written',len(rows),'rows')
