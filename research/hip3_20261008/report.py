from pathlib import Path
import json,hashlib,platform,importlib.metadata
import numpy as np,pandas as pd
P=Path(__file__).parent;M=pd.read_csv(P/'metrics.csv');A=M[M.part=='full'];AU=json.load(open(P/'audit.json'));E=pd.read_csv(P/'events.csv');C=pd.read_csv(P/'btc_correlations.csv');V=pd.read_csv(P/'coverage.csv');BOOK=pd.read_csv(P/'current_books.csv')
best='cross9_weekend_XYZ100_close'
def num(x,d=3):return f'{x:.{d}f}'
def pct(x):return f'{x*100:.2f}%'
def table(headers,rows):return '\n'.join(['| '+' | '.join(headers)+' |','| '+' | '.join(['---']*len(headers))+' |']+['| '+' | '.join(map(str,r))+' |' for r in rows])
selected=[best,'cross9_weekend_XYZ100_3h','cross9_daily_XYZ100_lb1_h3','all16_daily_SP500_lb1_h3','cross9_overnight_XYZ100_close','cross9_external_weekend_XYZ100_1h','cross9_external_weekend_SP500_3h']
rows=[]
for g in selected:
 r=A[A.candidate==g].iloc[0];h=M[M.candidate==g].set_index('part');rows.append([g,r.days,num(r.sharpe_phase_mean),num(r.sharpe_phase_sd),f'{h.loc["first","sharpe_phase_mean"]:.3f} / {h.loc["second","sharpe_phase_mean"]:.3f}',pct(r.net_ann),num(r.corr_baseline),num(r.blend_delta_sharpe_mean),num(max(r.p_fwer_blend_7,r.p_fwer_blend_14),4)])
fullbase=[x['sharpe'] for x in AU['full_baseline']['full']]
phase=[]
for path,ev in E[E.path.str.startswith(best+'_')].groupby('path'):
 r=pd.Series(0.,index=pd.date_range('2026-06-26','2026-10-03',tz='UTC'));ix=pd.to_datetime(ev.entry,utc=True).dt.normalize();net=ev.gross+ev.funding-ev.turn*.00075
 for t,v in zip(ix,net):r.loc[t]+=v
 phase.append([path.split('_')[-1],num(r.mean()/r.std(ddof=1)*np.sqrt(365)),pct(r.sum()),len(ev)])
cb=[]
for c in ['TSLA','NVDA','XYZ100','SP500','GOLD','CL','EUR','JPY','COIN','HOOD','MSTR']:
 r=C[(C.coin==c)&(C.part=='full')].iloc[0];cb.append([c,int(r.n),num(r['corr']),num(r.beta),num(r.r2)])
liq=[]
for c in ['TSLA','NVDA','MU','XYZ100','SP500','GOLD','CL','EUR','JPY']:
 r=V[V.coin==c].iloc[0];liq.append([c,int(r.n),f'${r.median_daily_usd:,.0f}',f'${r.p10_daily_usd:,.0f}'])
b=M[M.candidate==best].set_index('part');bf=b.loc['full'];bb=[]
for part in ['full','first','second']:
 r=b.loc[part];bb.append([part,num(r.baseline_phase_sharpe_mean),num(r.baseline_phase_sharpe_mean+r.blend_delta_sharpe_mean),num(r.blend_delta_sharpe_mean),num(r.blend_delta_sharpe_min),pct(r.net_ann)])
only=M[M.part.isin(['first','second'])].pivot(index='candidate',columns='part',values='blend_delta_sharpe_mean');both=only[(only['first']>0)&(only['second']>0)].index.tolist()
text=f'''# HIP-3 结构性缺口验证：没有通过的新增边际

**结论：72 项经济候选，没有一项满足五项检验。全部结论：样本极短，未经验证。** 7 个固定成本下净收益为正，但收益与组合 Sharpe 改善的 FWER 校正均无一达到 0.05。没有修改实盘代码、没有下单。

表面最优候选是 `cross9_weekend_XYZ100_close`：独立候选延迟平均 Sharpe 1.856，但仅12个独立周末，前/后半 Sharpe 3.081 / -1.945；固定90/10组合 Sharpe 平均改善0.0356，保守 p_fwer=0.9745。它应否决，不能称作 Sharpe 1.79 的改进。

## 1. 先核实机制与标的

- 直接 POST /info 的 xyz 元数据实际有132个条目（不是预设131）。`xyz:VIX` 当前 `isDelisted=true`、`strictIsolated`，日线返回 `[]`；**不可交易，未用外部 VIX 替代**。证据：meta_context.json、vix_daily.json。
- XYZ 的美股外部定价覆盖 **周日20:00至周五20:00 ET 的24/5**，含 BOATS 隔夜盘。9:30现金开盘只是另一个交易时段，不能把整晚价格变动当成无外部锚的错价。来源：[XYZ US市场说明](https://docs.trade.xyz/perpetuals/markets/stocks/us.md)。文档当前状态不保证全历史参数一致，未取得历史逐tick oracle/外部价格。
- 真正机制断点是 Fri20ET→Sun20ET：内部 oracle 在外部价格不可用时追踪冲击价格，外部恢复后切回外部价格；价格发现边界存在重锚，并非永远锁在周五收盘价。来源：[Oracle](https://docs.trade.xyz/perpetuals/mechanics/oracle-price.md)、[Discovery bounds](https://docs.trade.xyz/perpetuals/mechanics/discovery-bounds.md)。本研究只用成交K线，不能声称观察到了“perp-正股公允价”本身。
- 同账户不等于所有标的共用全仓：当前 AMD/PLTR/NFLX/ORCL/COIN/HOOD/MSTR 为 onlyIsolated/noCross。原16股宇宙保留为不可部署诊断；cross9仅TSLA/NVDA/AAPL/MSFT/GOOGL/META/AMZN/INTC/MU。

## 2. 数据与边界

UTC研究截点固定2026-10-07 17:30，剔除当根未完成日线/30m线。本地84份日线全有100天以上，实际100–329个已完成日线，XYZ100例外为359个（原文件360根含形成中当根）。这比请求中“17个≥100天”丰富，但不能增加独立周末数量。

补取23个标的30m线约104天、23个标的历史funding共{sum(x['n'] for x in AU['funding'].values()):,}条。日内测试机会窗口2026-06-26至10-03共100天，只有12个独立周末；日线机会窗口因上市和回看差异为194–324天。主DEX本地日线止于10-04开盘，因此组合检验只使用截至10-03的已实现收益；没有给缺失基线日期补零。服务器本地SQLite在candles/candles_h/oi_snapshots均无xyz数据。

XNYS日历使用America/New_York转换UTC，完整日历包含13:30/14:30开盘和早收盘；实际30m覆盖期全在夏令时，只观察到13:30，**未用数据验证冬令时收益**。周末oracle恢复精确对齐Sunday20ET，即本次样本Monday00:00 UTC；信号全部来自已完成K线。cash_calendar.csv及checks.json可复核。

## 3. 候选、相位和成交口径

每个宇宙36项：

1. 日线股票相对XYZ100/SP500反转：回看1/3/5天、持有1/3天，共12项；持有3天遍历全部0/1/2相位，禁止挑起点。
2. 普通隔夜/现金周一开盘修正：不对冲/XYZ100/SP500、持有1h/3h/现金收盘，共18项。
3. 真正外部价格恢复的周末修正：不对冲/XYZ100/SP500、持有1h/3h，共6项。

两宇宙合计72项，192条相位/延迟路径。日内事件锚固定外部时段，不存在可自由平移的UTC起点；全部30/60/90分钟执行延迟分别报告并平均，没有挑最快/最慢成交。持有期1h/3h/收盘均作为独立候选计入校正。未对冲版本属方向性诊断，不能称为加密中性配对方案。

信号是负的股票涨幅（配对时减指数涨幅），不挑反向赢家。入场前24h或上一完整日股票成交额至少$1M、指数$5M；每股等名义，指数腿净额合并后重新规范到Σ|w|=1。双边组合入场净名义为0，但不等于beta中性。入场用下一根可用开盘价加成本，日内至少延迟30分钟；它是交易价格代理，**OHLC不能证明IOC一定填满**。代码不模拟限价“触碰成交”。

主成本每边taker0.045%+滑点0.03%；另报0.10%/0.20%滑点。资金费率逐小时历史数值已纳入，以结算时最近已完成K线的价格估名义，因此尤其旧日线期间仍非精确mark notional。候选为单位入场NAV固定数量的每日MTM收益代理，跨日持仓没有精确重建逐tick账户权益/清算；年化数字只是单位gross=1的算术换算，不能当实际3x账户年收益。

Maker0.015%+0.03%只作为**费用下界**列出，未给任何maker可实现Sharpe；需要订单队列、成交选择与事后markout才能辨别成交的是好单还是坏单，当前数据没有这项证据，不能通过真实成交检验。

当前元数据多数normal股票为growthMode，官方base taker约0.009%，非growth标准约0.09%，并不是所有xyz都0.045%。另算当前逐标的feeScale情景，但**没有把今天费率倒灌当历史真实收费**。历史期间变更前的实际feeScale未恢复。来源：[Hyperliquid费用公式](https://hyperliquid.gitbook.io/hyperliquid-docs/trading/fees)、[XYZ费用](https://docs.trade.xyz/perpetuals/mechanics/fees.md)。主结论仍按请求统一成本检验。

## 4. 重叠样本与多重检验

单路径事件不重叠，持有3天不每天下新事件；股票间同日共振不算独立样本。日内执行延迟也不算三倍样本。保存{len(E):,}条组合事件记录、126,006条腿记录，独立周末仍是12个。

按UTC日同步循环块bootstrap，块长7和14天，分别3999次、固定seed20261008。收益均值采用centered bootstrap maxT，FWER家族含72经济平均路径和192单独路径，共264项；同日期固定90%基线+10% HIP3预算（HIP3单位gross乘2.7）的Sharpe改善采用配对同步maxT，共72项，基线全部3相位和候选全部相位/延迟平均。两组FWER作为不同必要门槛，任一失败即否决。最终取两个块长较大的p；**没有朴素t检验**。best的净收益p_fwer分别0.9950/0.9985，Sharpe改善0.9445/0.9745。

所有72项、包括无对冲诊断和不能全仓的all16，都保留在校正家族。当前便宜费率与成本压力仅作同候选情景，没有用它们挑选新策略或声称校正显著。

## 5. 代表结果（全部候选见 metrics.csv）

{table(['候选','天数','Sharpe相位/延迟均值','离散度SD','前半/后半Sharpe','净收益年化算术','与基线corr','混合ΔSharpe','改善p_fwer'],rows)}

表面最优候选的三条执行延迟：

{table(['执行延迟','Sharpe','100天净收益算术','独立事件数'],phase)}

它的同日期组合结果：

{table(['子样本','基线相位均值Sharpe','固定混合均值Sharpe','平均Δ','最差相位/延迟Δ','候选净年化算术'],bb)}

总计7/72固定成本净收益正、14/72当前fee情景净收益正，但**零FWER通过**。cross9为5/36和6/36。只有 `cross9_daily_XYZ100_lb1_h3` 在前后半的平均混合Sharpe都改善；它的三相位全期Sharpe范围-0.824至1.292，改善最差组合为-0.071，全期 p_fwer=1，且0.10%滑点下算术年化净收益-14.10%，同样否决。

真正external_weekend的cross9六项全部亏损：固定成本年化算术-4.86%至-6.78%，当前fee情景仍全部亏损。普通隔夜全部版本固定成本净收益为负（-17.71%至-39.14%年化算术）。因此没有发现“oracle外部价格恢复后可赚钱的纠偏”。现金周一最优也不是外部恢复的直接证据。

最佳现金周一候选每单位gross年化换手{bf.turn_ann:.2f}，年化成本{pct(bf.cost_ann)}，资金费率贡献{pct(bf.funding_ann)}；净收益{pct(bf.net_ann)}，0.10%滑点{pct(bf.net_ann_slip10)}，0.20%滑点{pct(bf.net_ann_slip20)}。**这些年度换算来自100天/12次周末，禁止当长期期望。**

## 6. BTC关系：不是所有HIP3都只是BTC，但没有可交易边际

以下为同UTC日close-to-close简单收益Pearson corr，pairwise真实同日期、不ffill。样本长度不同，不能横向把相关性当显著性或独立alpha证据：

{table(['标的','同日样本数','BTC corr','BTC beta','R²'],cb)}

MSTR/COIN非常像加密暴露（R²约73%/60%），且不允许全仓；TSLA/NVDA、宏观商品/汇率没有接近1的相关性，不能说全部是换马甲。候选P&L与**现有策略**corr已另算（上表）；低相关性本身不能修复负收益、多重检验失败或后半失效。全部标的前/后半及周末corr保存在btc_correlations.csv。

## 7. 流动性与容量

{table(['标的','完整日线数','日成交额中位','日成交额10分位'],liq)}

只用入场之前的数据给容量约束：每条腿 <=前24h成交额0.1%；日内另限前30m成交额1%，日线另限前一完整日成交额0.1%。按实际权重反推单位gross=1的策略NAV上限。

- 最优现金周一cross9候选全历史最小约${bf.capacity_min:,.0f}、事件10分位约${bf.capacity_p10:,.0f}。固定10%策略预算对应gross0.27×总权益，换算全账户上限约${bf.capacity_min/.27:,.0f}（保守最小），10分位约${bf.capacity_p10/.27:,.0f}。**只是成交额参与率估计，未证明该资金量能在3bps滑点成交。**
- 真正周末外部恢复窗口更薄：cross9指数对冲方案最小单位策略NAV约$442–531，事件10分位约$3.9k–5.1k（代表SP500 3h/XYZ100 1h）；按gross0.27映射账户权益仅约$1.6k–2.0k的保守最小约束。容量不是只看全日成交额就能通过。
- 当前单次L2快照的spread约TSLA0.53bps、NVDA0.84bps、XYZ1000.32bps、SP5000.13bps；3bps以内对手深度见current_books.csv。这是周中即时快照，不是历史周末深度；不能用它证明过去成交，更不能证明maker无逆选择。

## 8. 基线校准与未通过的硬检验

按现行trader.rs重建：14日动量/含当日的20日样本波动、低波动、当日成交额/排除当日30日均值、cap5、byte-sum分档、全3相位、下一UTC开盘执行、部署2.7倍权益、2%再平衡带。7个时点独立scalar权重核对最大误差1.39e-17。重建全期相位Sharpe为{', '.join(num(x) for x in fullbase)}，平均{np.mean(fullbase):.3f}；2023–24均值{np.mean([x['sharpe'] for x in AU['full_baseline']['2023-24']]):.3f}、2025–26均值{np.mean([x['sharpe'] for x in AU['full_baseline']['2025-26']]):.3f}。

这不复现历史1.79：历史报告脚本与现行cap/波动窗口/调仓计价不同，本次不能直接拿新的短期Sharpe减1.79。基线缓存退市/缺报价的处理是陈旧价格假定平仓，没有逐笔历史填单证明；基线资金费率尚未计入。候选资金费率已计入，但这种不完整账户还原意味着所有混合结果只是研究诊断，不能升级为实盘结论。未声称实际3x清算风险、统一账户保证金及IOC拒单都已模拟。

五项门槛状态：

| 门槛 | 状态 |
| --- | --- |
| 相位 | 全相位/全延迟已报告；唯一双半平均改善候选有负相位，失败 |
| 重叠样本 | 非重叠事件+同步块bootstrap已执行；12周末不扩成几百独立股票事件 |
| 真实成交 | 仅taker价格代理与压力，maker不可验证；历史IOC/mark/保证金还原不完整，不通过 |
| 多重检验 | 72候选/264收益均值测试，无任何p_fwer<0.05，失败 |
| 成本/子样本/相关性 | 全部输出；表面最好后半失效，其他正项FWER或相位/压力失败；年切分不可做 |

**最终：没有可进入实盘讨论的HIP-3新增边际。样本极短，未经验证。**

## 9. 可复现文件

- DESIGN.md：收益结果查看前的候选登记及机制/保证金修订记录（候选没有删掉以缩小校正）。
- fetch.py、raw/、meta_context.json、vix_daily.json：API原始证据；不包含签名或订单请求。
- run.py、baseline_engine.py：完整回测/校正；metrics.csv、bootstrap.json、daily_returns.csv：全部结果。
- events.csv、legs.csv：每笔信号起止、入出场UTC、权重、资金费率和容量估计。
- check.py、checks.json：零事件重叠、gross规范、配对净名义、cross合规、价格收益和资金费率独立复算；baseline_check.py、baseline_parity.json：权重核验。
- audit.json、provenance.json、cash_calendar.csv、coverage.csv、btc_correlations.csv、current_books.csv：数据范围、hash、日历与容量诊断。

复现：`python fetch.py`（缓存存在则不覆盖），`python run.py`，`python check.py`，`python baseline_check.py`，`python report.py`。研究截点固定，不自动滚动；升级数据应视作新验证集。Python依赖版本见provenance.json。
'''
(P/'REPORT.md').write_text(text)
files=[f for f in P.rglob('*') if f.is_file() and '__pycache__' not in str(f) and f.name not in ['provenance.json','run.log','fetch.log']]
provenance={'python':platform.python_version(),'dependencies':{x:importlib.metadata.version(x) for x in ['numpy','pandas','requests','exchange-calendars','scipy']},'asof_utc':'2026-10-07 17:30:00','sha256':{str(f.relative_to(P)):hashlib.sha256(f.read_bytes()).hexdigest() for f in files},'raw_daily_inputs_sha256':AU['input_sha256'],'source_baseline_engine':'research/liq_oi_20261008/engine.py copied then own run.py extends final opening and aligns PnL dates','no_live_trade_actions':True}
json.dump(provenance,open(P/'provenance.json','w'),indent=2)
print('REPORT.md written,',len(text),'characters')
