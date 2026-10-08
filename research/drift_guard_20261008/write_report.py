import os,json,numpy as np
OUT=os.path.dirname(__file__);r=json.load(open(OUT+'/results.json'));ver=json.load(open(OUT+'/verification.json'));ac=json.load(open(OUT+'/accounting_check.json'));rets=np.load(OUT+'/hourly_returns.npz')
f='0.00075';h=r['hourly'][f];base=h['baseline'];names=['baseline','always_twice','net2','net3','net5','net8'];label={'baseline':'每日基线','always_twice':'无条件每日两次','net2':'净漂移 2%','net3':'净漂移 3%','net5':'净漂移 5%','net8':'净漂移 8%'}
lines=[]
def w(s=''):lines.append(s)
def tab(headers,rows):
 w('| '+' | '.join(headers)+' |');w('| '+' | '.join(['---']*len(headers))+' |')
 for row in rows:w('| '+' | '.join(map(str,row))+' |')
 w()
def pct(x):return f'{x*100:.3f}%'
w('# 漂移保护独立评估 — 2026-10-08')
w()
w('**结论：目前没有证据表明漂移保护能独立提高 Sharpe，未通过验证门槛。不能声称已经证明全期无效。** 指定日线无法识别盘中提前调仓，因此全期与 2023–24 / 2025–26 的保护 Sharpe 均不可估计。真实小时线的 93 天补充样本中，2% 保护平均 Sharpe 为 3.103，每日基线为 2.723，但无条件每日两次为 3.229；保护没有优于这个关键对照，所有候选均未通过块 bootstrap + maxT。相位差异也很大。保护确实压低了观测净敞口；这属于风险控制效果，不等于已证实的收益优势。')
w()
w('## 1. 数据、基线与可识别性')
w()
w('已先读 `docs/validation-protocol.md` 与 `src/trader.rs`。协议末尾仍称错峰是“唯一通过的改进”，与用户当前约束冲突；本次以用户要求为准，全部每日完整调仓，`rebalance_slices=1`，无止盈，不测试错峰。')
w()
w('原始日线：`/tmp/hl-daily-full/`，234 文件，176,435 根，2023-06-29 至 2026-10-04。日线有 OHLC，但不同币的日内高低点不同时发生；不能把它们拼成同步组合价格，更不能据此推断越界顺序或成交时间。没有使用 OHLC 路径插值、日收益平分或未来收盘价提前成交。')
w()
w('三因子严格按源代码：14 日价格动量 / 包含当日的 20 日样本波动；负波动；当日成交额 / 不含当日的过去 30 日均值。流动性也使用过去 30 日成交额，至少 5 个有效观测，门槛 $5M；每因子各自等权多空，三个权重向量平均并抵消后归一化 Σ|w|=1。横截面平局按币名。每日目标 gross/equity = 0.9 × 3 = 2.7。')
w()
w('**“每腿 ≤8”按 `trader.rs` 的 `target_positions=8` 实现：每个因子每腿最多 8 个，三本账合并后单侧名字可能超过 8。** 若要求的是最终合并组合单侧最多 8 个，那与现有源代码的三因子叠加口径不同；本研究没有另加截断规则。逐日权重已经与实际 Rust `FactorPanel` 对照，1161 个索引最大绝对误差 '+f'{ver["max_abs_weight_error"]:.3e}。')
w()
w('保留源代码的 2% 单币调仓 band，所有币每日均可调；不额外改信号、增加止盈或改持仓周期。价格变化通过固定币数量逐步影响名义与权益。成本按每次实际买卖名义收取 taker 0.045% + 滑点，一次开仓、一次平仓均各收一次，反手计入两边名义。目标按扣费前权益计算，扣费后实际 gross/equity 略大于 2.7。不模拟 maker。')
w()
w('成交为可见开盘价加固定成本的研究近似，**并非 $570 小账户的逐笔实盘复刻**：没有历史数量精度、$10 最小订单、订单拒绝与资金费率完整历史；忽略这些微结构，不能直接推断小账户的触发有效性。$570 实测 −17.8% 是 net/equity，而下文风险表主指标是 |net|/gross，两者分母不同；以 gross/equity=2.7 粗略换算为 6.59%，不是 17.8%。')
w()
w('最早同时满足有效因子与流动性、可持仓的执行日为 **2023-12-31**。因此“2023–24”有 367 个收益日，不能把无交易的早期零收益补成有效历史。日线用前一根已收盘信号在下一日开盘执行；当日线缺失时不新开仓，已持有缺价仓位只能作明确标注的旧价假设平仓。这使全期日线基线仍有不可成交退出的不确定性，详见下表。')
w()
w('### 全期与子区间：能计算什么')
w()
rows=[]
for cost,slip in [('0.00075','0.03%'),('0.00105','0.06%'),('0.00145','0.10%')]:
 for period,v in r['daily'][cost].items():rows.append([slip,period,v['days'],f'{v["sharpe"]:.4f}',v['rebalance_events'],f'{v["daily_turnover_equity"]:.4f}',pct(v['annual_cost']),v['missing_mark_events']])
tab(['滑点','区间','有效日数','基线 Sharpe','实际调仓批次','日均交易名义/权益','年化交易成本','旧价退出事件'],rows)
w('换手定义为 Σ|Δ名义|/调仓前权益，含开仓与终止平仓；年化成本为每日成本/当时权益之和的日均值 ×365，不是以初始净值为分母的累计美元费用。四个缺价事件都在 2025–26，最大缺价持仓约占当时权益 '+pct(r['daily'][f]['full']['missing_mark_exposure_max'])+'；旧价退出假设对这些位置不提供实盘成交保证。')
w()
tab(['检验','全期','2023–24','2025–26','相位平均 / 离散度'],[
 ['基线（滑点0.03%）',f'{r["daily"][f]["full"]["sharpe"]:.4f}',f'{r["daily"][f]["2023-24"]["sharpe"]:.4f}',f'{r["daily"][f]["2025-26"]["sharpe"]:.4f}','日线1天周期只有1个可观测日相位；盘中相位不可估计'],
 ['2% / 3% / 5% / 8% 保护','不可识别','不可识别','不可识别','不可识别'],
 ['上述各自 + 杠杆<0.85','不可识别','不可识别','不可识别','不可识别'],
 ['无条件每日额外调仓一次','不可识别','不可识别','不可识别','不可识别']])
w('若只在日线开盘检查漂移，它与当天本来就要做的每日调仓合并：增加调仓数=0、增加换手=0、收益差=0。这是观察频率造成的退化，**不能当作提前保护无效的实证结果**，也不能把退化序列 bootstrap 得到的 p=1 冒充全期检验。全期保护的 FWER 为 N/A。没有给日频一个虚假的 0 相位离散度来代表盘中稳健性。')
w()
w('### 日线基线的漂移（观测到每日调仓前 / 后）')
w()
rows=[]
for period,v in r['daily'][f].items():
 for point,key in [('调仓前','net_gross_pre'),('调仓后','net_gross_post')]:
  d=v[key];rows.append([period,point,pct(d['mean']),pct(d['p90']),pct(d['max'])])
tab(['区间','观测点','|net|/gross 均值','90分位','最大'],rows)
w('上述调仓后下降是原有每日调仓的效果，不是新增保护效果；日线看不到盘中峰值。全期每日调仓前最大 |net|/equity = '+pct(r['daily'][f]['full']['net_equity_pre_max'])+'，说明敞口漂移确实存在。')
w()
w('## 2. 可观测小时线补充实验（不能代替全期验证）')
w()
w('本地 `/tmp/hl-hist/` 有 81 币、390,823 根小时线，覆盖 2026-03-09 至 2026-10-03。对 2026-03-11–10-02 共 206 日，用**全日线宇宙的原始目标权重**审计，完整一天24小时覆盖的日期仅 116 日，日内最差目标名义覆盖率平均 96.06%；缺价名字包括 BERA、BIO、CHIP、KAITO、TON、WLFI。96% 覆盖不能当成100%执行，尤其缺价可能正与大幅价格变化相关。')
w()
w('仅选价格覆盖连续≥30日的两个窗口，选择依据为完整价格覆盖，不使用收益：2026-03-15–05-06、2026-06-17–07-28。实际收益日分别为 03-15–05-05（52日）、06-17–07-27（41日），末日用于结束估值，共 **93 日**。窗口独立从空仓开始，计入开仓与终止平仓。没有限制选币宇宙、没有重新筛掉缺价币、没有旧价填补；运行断言确保每个实际持仓和每个交易目标在所有执行/估值点有真实价格。覆盖选择仍可能改变市场样本，故结果为探索性补充。')
w()
w('**24 个 offset：UTC 0–23点各作为每日固定调仓时间。** 每小时先按可见价格估值，在固定调仓点以外，|net|/equity > 2% / 3% / 5% / 8% 时立即按最近已收盘日线目标完整调仓；每日固定调仓保留，不因保护取消。一次小时检查最多调仓一次，固定调仓与保护同点合并。杠杆变体为额外 OR 条件 gross/equity < 0.85 × 有效目标2.7 = 2.295。对照在每个 offset 的固定时间与12小时后无条件各调一次，同样使用当时最近已收盘信号、同一 band、同一成本。')
w()
w('这是小时级保护，尚未测试秒级/分钟级监测和真实成交延迟。对跨UTC午夜的 offset，提前调仓或第二次调仓可能更新到新收盘信号；因此行为同时包含名义再中性化、杠杆恢复与更早信号更新，不能全部归因于净敞口本身。')
w()
w('### Sharpe 与相位离散度（滑点0.03%，24相位均值）')
w()
rows=[]
for name in names:
 v=h[name];rs=rets[f+'|'+name];rows.append([label[name],f'{v["sharpe_phase_mean"]:.3f}',f'{v["sharpe_phase_mean"]-base["sharpe_phase_mean"]:+.3f}',f'{v["phase_sd"]:.3f}',f'{v["phase_min"]:.3f}–{v["phase_max"]:.3f}',f'{np.mean([x[:52].mean()/x[:52].std(ddof=1)*np.sqrt(365) for x in rs]):.3f}',f'{np.mean([x[52:].mean()/x[52:].std(ddof=1)*np.sqrt(365) for x in rs]):.3f}'])
tab(['方案','平均Sharpe','Δ vs每日','相位标准差','相位最小–最大','窗口1平均','窗口2平均'],rows)
w('主统计量是**24个相位各自 Sharpe 的算术平均**，不是先平均收益序列再计算 Sharpe。所有逐相位值保存在 `results.json`。例如每日基线的相位标准差1.069，2%保护为0.714，依然很大；不能挑选一个漂亮 offset 推广。两个窗口都在2026年，不能冒充要求的2023–24 / 2025–26双区间通过。')
w()
w('四个“+杠杆0.85”变体在三档成本、全部24相位中，与同阈值净敞口保护的完整收益序列**逐元素完全相同**，没有带来额外可识别效果；仍作为预先列出的候选纳入多重检验。')
w()
w('### 调仓、换手、成本变化')
w()
rows=[]
for name in names:
 v=h[name];rows.append([label[name],f'{v["rebalance_events_mean"]:.2f}',f'{v["extra_events_mean"]:.2f}',f'{v["daily_turnover_equity"]:.4f}',f'{(v["daily_turnover_equity"]/base["daily_turnover_equity"]-1)*100:+.2f}%',pct(v['annual_cost']),f'{v["correlation_baseline"]:.4f}'])
tab(['方案','93日实际调仓批次','其中盘中保护/第二次','日均交易名义/权益','换手Δ','年化成本','日收益与基线相关'],rows)
w('批次按实际交易名义>0计数，每批可能包含多币订单；不等于订单条数。无条件两次每天有两个计划点，但受 band 影响少数点没有实际订单；保护也可能令当天固定点无需实际交易。终止平仓计入换手和成本，单独不计为调仓批次。相位均值批次可为小数。')
w()
w('2%保护比每日基线增加约121%实际调仓批次、增加4.71%换手；名义调仓次数增长远大于换手增长，因为很多额外订单只是小额调整。保护并没有省掉足以超越每日两次对照的成本。因果上只能说“独立价值未获支持”，不能证明所有改善都仅由频率导致；还需要更长历史以及可考虑的等次数对照来细分频率与择时，但本次没有追加寻优。')
w()
w('### 保护最直接的效果：净敞口')
w()
rows=[]
for name in names:
 for point,key in [('每小时检查前','net_gross_pre'),('检查/交易后','net_gross_post')]:
  v=h[name][key];rows.append([label[name],point,pct(v['mean']),pct(v['p90']),pct(v['max'])])
tab(['方案','观测点','|net|/gross 均值','90分位','最大'],rows)
w('这些分布对全部小时观测与24相位等权；“检查/交易后”包含没有触发的小时，**不是只挑成交后接近零的时刻**。以连续持有期间的整点检查后指标比较，2%保护将均值从0.510%降至0.260%，90分位从1.155%降至0.552%，最大从5.130%降至0.802%。检查前最大仍为2.008% gross，因为跳跃可在两个小时之间先越界；阈值是net/equity，不能把它理解为net/gross的严格上限，更不能说已控制所有盘中最大敞口。')
w()
w('## 3. 块 bootstrap + maxT / FWER')
w()
w('8个保护候选（4净敞口阈值 × 是否加杠杆条件），加1个每日两次对照，共9个非基线方案。对8个候选同时检验“优于每日基线”和“优于每日两次”；对每日两次检验“优于每日基线”，三档成本一起组成 **51个单侧假设**。没有把成本挑选留到显著性检验外，也没有把24相位当成24组独立样本；重复候选不人为增加独立度。')
w()
w('9999次成对循环移动块 bootstrap，窗口内分别采样52/41日，块不跨越两个真实数据窗口边界；所有候选、成本和相位共用同一抽样日索引。每次用重采样收益重新计算各相位Sharpe，再取24相位均值和候选差值。零假设分布为bootstrap差值−观测差值，按各假设bootstrap标准差缩放，逐次取51个统计量最大值；p_fwer=(1+#maxT≥观测T)/10000。随机种子20261008+块长。块长5/10/20/30日，报告全套，不按最小p挑选。主表用10日块；样本只有93日，长块有效独立信息极少，bootstrap也不能弥补历史缺失。')
w()
rows=[]
for name in names[2:]:
 a=f+'|'+name+'|baseline';b=f+'|'+name+'|always_twice';rows.append([label[name],f'{r["bootstrap"]["10"][a]["delta_sharpe"]:+.3f}',f'{r["bootstrap"]["10"][a]["p_fwer"]:.4f}',f'{r["bootstrap"]["10"][b]["delta_sharpe"]:+.3f}',f'{r["bootstrap"]["10"][b]["p_fwer"]:.4f}',', '.join(f'{r["bootstrap"][str(L)][a]["p_fwer"]:.4f}' for L in [5,10,20,30])])
tab(['方案','ΔSharpe vs每日','p_fwer','ΔSharpe vs两次','p_fwer','vs每日 p_fwer：块5/10/20/30'],rows)
w('三档成本、全部51假设的最小p_fwer，按块5/10/20/30分别为 '+', '.join(f'{min(x["p_fwer"] for x in r["bootstrap"][str(L)].values()):.4f}' for L in [5,10,20,30])+'。**没有任何检验达到0.05。** `results.json` 同时保留95%基本bootstrap区间及Sharpe影响函数版本作为方法敏感性核对；影响函数版本也没有通过。以上p值仅针对93日补充样本，全期候选p值仍是N/A。')
w()
w('## 4. 成本敏感性')
w()
rows=[]
for name in names:
 row=[label[name]]
 for cost in ['0.00075','0.00105','0.00145']:
  v=r['hourly'][cost][name];row.append(f'{v["sharpe_phase_mean"]:.3f} ± {v["phase_sd"]:.3f}')
 rows.append(row)
tab(['方案','滑点0.03%：均值±相位SD','滑点0.06%','滑点0.10%'],rows)
w('三档均加固定taker 0.045%，总单边成本为0.075% / 0.105% / 0.145%。每档重新模拟扣费后的权益、漂移、触发与换手，不是从低成本收益机械减常数。2%保护在三档都比每日基线高，幅度随着成本上升缩小，但始终低于每日两次；5%与8%均未改善。所有成本档位都没有统计通过，结论一致：**无已证实的独立Sharpe优势**。杠杆变体仍完全重合。')
w()
w('## 5. 验证与交付')
w()
w('独立用币数量与美元权益记账，核对小时权重递推：每日基线与2%保护（UTC0相位）日收益最大差分别为 '+f'{ac["baseline"]:.3e} / {ac["net2"]:.3e}。所有收益有限且未出现账面破产；全目标权重净和为0、总绝对权重为1；小时持仓/目标逐点有价断言全部通过。Rust源文件SHA256为 `'+ver['rust_source_sha256']+'`。')
w()
w('可复现命令（本机 `python` 为含NumPy/Pandas的Miniconda解释器，系统`python3`缺NumPy）：')
w('```sh\ncd /Users/wonder/Code/stars/research/drift_guard_20261008\nexport https_proxy=http://127.0.0.1:7897 http_proxy=http://127.0.0.1:7897 all_proxy=socks5://127.0.0.1:7897\npython audit.py\npython run.py\nOPENBLAS_NUM_THREADS=1 python bootstrap.py\npython verify.py\npython check_accounting.py\npython write_report.py\n```')
w()
w('脚本与 `REPORT.md` 在指定研究目录；原始大数据仍在 `/tmp`，压缩收益文件为gitignore排除的`.npz`。`results.json`、`hourly_coverage.json`、`verification.json`、`accounting_check.json`、`manifest.json`、`hourly_manifest.json`用于审计。未修改实盘代码，未提交或部署。')
w()
w('**尚缺的必需证据**：覆盖2023–2026、全候选持仓可同步估值的小时线或更高频价格，以及缺价/退市处理与完整资金费率。获得这些数据后，才能给出要求的全期、两个子区间、全天相位与有效FWER结论。在此之前，不建议以“能提高Sharpe”为理由将保护上线；若把它作为敞口风险限制，需要另行接受其成本和小账户执行约束。')
open(OUT+'/REPORT.md','w').write('\n'.join(lines)+'\n')
print(OUT+'/REPORT.md',len('\n'.join(lines)))
