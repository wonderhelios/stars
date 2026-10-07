import json,numpy as np
from pathlib import Path
OUT=Path(__file__).resolve().parent
D=json.load(open(OUT/'results_daily.json'));H=json.load(open(OUT/'results_hourly.json'));G=json.load(open(OUT/'diagnostics.json'));C=json.load(open(OUT/'baseline_check.json'))
keys=list(D['bootstrap']['30']);s=D['summary'];b=s['baseline'];lines=[]
def add(x=''):
 if (x.startswith('## ') or x.startswith('- ') or x.startswith('|')) and lines and lines[-1] and not lines[-1].startswith(('- ', '|')):lines.append('')
 lines.append(x)
 if x.startswith('## '):lines.append('')
def pct(v):return f'{100*v:.2f}%'
add('# 清算级联与持仓量结构：验证结果（2026-10-08）')
add()
passed=[]
for k in keys:
 z=s['blend_'+k]
 if all(D['bootstrap'][L][k]['p_fwer_all14']<.05 for L in ['15','30','60']) and all(z[p]['sharpe']>b[p]['sharpe'] for p in ['2023-24','2025-26']) and abs(s[k]['full']['correlation_baseline'])<.9:passed.append(k)
add('**没有合格信号。**' if not passed else '**有统计门槛候选，但无法视为实盘合格：'+', '.join(passed)+'。仍须解决基线差异、完整资金费率和成交限制。**')
add('这次测试 14 个固定候选/配置：8 个日频候选、6 个小时事件配置；没有收益驱动的参数搜索。不能从本报告推断“真实清算没有反弹”，因为小时事件是 OHLCV 代理，没有真实清算标签。')
add()
add('## 基线、样本与成交口径')
add(f"日频评估 {D['meta']['dates'][0]} 至 {D['meta']['dates'][1]}；两子区间天数 {D['meta']['days']['2023-24']} / {D['meta']['days']['2025-26']}。234 日线文件均保留，逐时点筛此前 30 日均成交额 ≥$5M；不因完整历史长度筛掉新币。")
add(f"严格按当前 trader.rs：14 日动量/含当日的 20 日样本波动、负波动、当日量/此前30日均量；各因子每腿最多5币，权重先平均再归一化；币名字节和取模分3档。独立逐币标量实现的8个抽查日权重误差均 <1e-10。流动宇宙平均 {C['liquid_universe_mean']:.1f} 币。")
add(f"本引擎基线相位平均 Sharpe **{b['full']['sharpe']:.3f}**，两个子区间 **{b['2023-24']['sharpe']:.3f}/{b['2025-26']['sharpe']:.3f}**；全期三相位 {', '.join(f'{v:.3f}' for v in b['full']['phase_sharpes'])}。这不是原报告的1.79，不能混口径比较。")
add('本地旧 `/tmp/stag2.py` 使用 cap8、波动不含当日且 ddof0、量窗口包括当日、币下标分档、同收盘固定权重；当前代码使用 cap5、含当日 ddof1、此前30日量、字节和分档。本次额外模拟价格/净值导致的仓位漂移与次日open执行。未调参数强凑1.79。')
add('所有日频候选固定20%混入基线、80%保留原书，在单账户先净额再归一化总目标；全部3相位独立记账。2.7倍权益名义对应3x×90%部署，2%调仓带，大账户忽略$10最小单、数量取整。每单位绝对交易名义扣 taker0.045%+滑点0.03%。年化成本=日均绝对成交名义/权益×365×0.075%；常用双边换手=表中绝对成交/2。')
add('maker0.015%+滑点0.03%只作为费用理想下界，不能证明挂单会成交。没有“触价即成交”模型；暴跌中挂买单成交与继续下跌相关，可能吞掉任何表面收益。没有订单簿/排队/成交条件数据，不声称maker改善。')
add('缺失执行价时按最后已知价假设退出，暴露与5%额外退出惩罚结果全部报告；这是假设，不能当真实成交。完整历史资金费率未纳入；这两项限制足以阻止任何候选直接上线。')
add()
add('## 数据可得性：清算、OI、多空比')
add('- **清算可观测，但用户级不是全市场。** 官方 WS `WsFill.liquidation` 包含 liquidatedUser、markPx、market/backstop；`userEvents` 和非费率账本也有清算。`userFills` 至多最近2000条，按时间查询只覆盖最近10000条，需要地址，不能从一个幸运的钱包恢复无偏全市场历史。全零地址测试只用于端点连通性，不作为清算样本。')
add('- 探测 `liquidations` 和 `openInterestHistory` type 均422，只说明这些猜测type不可用，不证明链上数据不可恢复。官方S3 `asset_ctxs` 可含历史OI，节点fills可含清算；两者本次匿名访问均403，明确要求认证的 requester-pays。没有可用的全市场历史清算标签。')
add('- 本地OI数据库534行=178币×3个UTC日期（2026-10-05～07）；`upsert_oi_snapshot` 按日覆盖，同日6小时采样丢失，跨度仅2天。完全不用于收益推断，也不用于验证代理。')
add('- **历史OI确有公开渠道：Binance Vision daily metrics。** 固定16个同名USDT永续，19,152个归档请求中19,151成功、1个404；用币本位 `sum_open_interest`，不用USD OI避免价格上涨机械增加名义OI。它是跨市场拥挤度输入，不是HL OI重建。每天最后≤23:45记录，信号至少延迟一整UTC日，OI变化与同样延迟的价变对应；无前向填充，缺失当天无该信号。')
add('- HL `clearinghouseState` 可读已知地址的当前仓位/清算价；当前 `metaAndAssetCtxs` 可读每币总OI。没有本次发现的官方全市场散户/大户分组历史指标。需无偏地址全集、事前大户门槛与历史状态归档，不能用今天榜单回溯历史。币安的全体账户多空比不等于散户，多空仓位数量总量又是成对的；本报告只使用“全体账户拥挤度”“top/global差异”，不称其真实散户 vs 大户。')
add()
add('## 8个日频候选')
add('price_volume_confirm/exhaust：sign(当日收益)×log(当日量/此前30日量)，及其反向；capacity_confirm：sign(收益)×log([量/振幅]/此前30日中位数)，振幅下限0.1%；oi_confirm/exhaust：延迟价变符号×币安币本位OI对数变化，及其反向；account_crowding_contra：负log全体账户多空比；top_global_gap：log大户仓位比−log全体账户比；cascade_rebound：日跌幅>max(5%,2.5σ)且放量>2倍时做多事件币、做空其余流动币。稀疏事件不对大量零分数按币名字典序强行排名。')
add()
add('|20%候选混合|全期S|2023–24 S|2025–26 S|三相位S标准差|候选独立收益corr|混合corr|30日块全族p|日均绝对成交/权益|年化成本|')
add('|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|')
for k in ['baseline']+['blend_'+v for v in keys]:
 z=s[k];raw=k.replace('blend_','');p=D['bootstrap']['30'][raw]['p_fwer_all14'] if raw in keys else None
 if p is None:
  add(f"|baseline|{z['full']['sharpe']:.3f}|{z['2023-24']['sharpe']:.3f}|{z['2025-26']['sharpe']:.3f}|{z['full']['phase_sd']:.3f}|1|1|—|{pct(z['full']['daily_abs_traded_equity'])}|{pct(z['full']['annual_cost'])}|")
 else:
  add(f"|{k}|{z['full']['sharpe']:.3f}|{z['2023-24']['sharpe']:.3f}|{z['2025-26']['sharpe']:.3f}|{z['full']['phase_sd']:.3f}|{s[raw]['full']['correlation_baseline']:.3f}|{z['full']['correlation_baseline']:.3f}|{p:.4f}|{pct(z['full']['daily_abs_traded_equity'])}|{pct(z['full']['annual_cost'])}|")
add()
add('成本后年化算术收益、CAGR、回撤、全部相位及每个子区间成本/相关性见 metrics_daily.csv/results_daily.json。独立因子和20%混合都报告；接受判据以混合的Sharpe改善与独立因子相关性<0.9为最低要求，混合相关性也单列，不能把接近1的混合宣称成完全独立策略。')
add()
add('|候选|15/30/60日块全族p|30日块ΔS 95%区间|两子区间都改善|')
add('|---|---|---|---|')
for k in keys:
 ps=[D['bootstrap'][L][k]['p_fwer_all14'] for L in ['15','30','60']];ci=D['bootstrap']['30'][k]['ci95'];both=all(s['blend_'+k][p]['sharpe']>b[p]['sharpe'] for p in ['2023-24','2025-26'])
 add(f"|{k}|{' / '.join(f'{v:.4f}' for v in ps)}|[{ci[0]:.3f}, {ci[1]:.3f}]|{'是' if both else '否'}|")
add('4999次循环移动块bootstrap，共享日索引跨所有币、候选和相位；日频对同引擎基线的相位平均ΔSharpe bootstrap，以居中、按bootstrap标准误标准化的最大统计量maxT校正8候选。小时族对净PnL均值maxT校正6配置。再对两族作Bonferroni×2控制14候选总FWER。要求15/30/60日块均p<0.05。区间是各候选点态区间，不是同时置信区间；子区间只作方向稳健性筛查，没有把其当独立p值。')
add()
add()
add('费用与缺失退出敏感性（全期Sharpe；maker列仅费率理想下界）：')
add('|策略|正常taker+3bps|taker+6bps|缺失退出额外5%惩罚|maker费率理想下界|')
add('|---|---:|---:|---:|---:|')
for k in ['baseline']+['blend_'+v for v in keys]:
 z=D['stress'][k]
 add(f"|{k}|{s[k]['full']['sharpe']:.3f}|{z['slippage_6bp']['full']:.3f}|{z['missing_exit_5pct']['full']:.3f}|{z['maker_fee_optimistic']['full']:.3f}|")
add()
add('## 清算代理：小时事件')
add(f"{H['meta']['files']}币小时线；评估 {H['meta']['start']} 至 {H['meta']['end']}。1h跌幅>max(3%,2.5×此前168h波动)、此前720h日均成交额≥$5M；放量>2倍此前168h均量作为清算代理，非放量暴跌作为负对照。全部6/12/24小时相位分别建组合，当前bar关闭后下根open入场；多事件币、空其余流动币，各腿50%。固定初始单位名义，不借用理想maker价。")
add('每相位窗口不重叠；不同相位和币仍会共享行情，用日块保留相关性。所有相位的事件数是同一候选事件集合的分割，不视作独立重复样本。事件交易差价收益扣多/空两腿的入场费与按退出名义计算的退出费，正常价格下合计约0.30%；单位总名义组合约0.15%。')
add()
add('|事件|相位平均S±标准差|事件币次数|事件平均净市场相对收益|与基线corr|15/30/60日块全族p|')
add('|---|---:|---:|---:|---:|---|')
for k,z in H['summary'].items():
 ps=[H['bootstrap'][L][k]['p_fwer_all14'] for L in ['15','30','60']]
 add(f"|{k}|{z['sharpe_phase_mean']:.3f} ± {z['sharpe_phase_sd']:.3f}|{z['events_all_phases']}|{pct(z['net_market_relative_trade_all_phases'])}|{z.get('correlation_baseline',float('nan')):.3f}|{' / '.join(f'{v:.4f}' for v in ps)}|")
add('小时窗口只有2026年，2023–24完全缺失，所以无论结果如何都不能合格。24小时个别相位的好数值不能覆盖其他相位；全部原始相位、成交成本、14日反转分数相关性与先前动量均值见results_hourly.json。基线相关性按同一日历重叠日计算，没有拿小时收益与日收益直接相关。没有发现显著独立边际，因此不宣称已证明独立于14日因子。')
add()
add('## OI代理是否成立，以及价涨量缩/量增')
corr=G['proxy_oi_correlations'];add(f"成交额/振幅的日变动与币安币本位OI变化逐币相关，16币中有效样本≥100天的 {len(corr)} 币，相关性中位数 **{np.median([v['correlation_capacity_change_bn_oi_change'] for v in corr]):.3f}**。这只是跨所诊断，不能验证它等同HL OI；完整逐币相关性见diagnostics.json。成交额是流量、OI是存量，无法从OHLCV唯一重建开平仓关系。")
add('|价格上涨后的分组|子区间|有效日|下一日平均市场相对收益（毛）|')
add('|---|---|---:|---:|')
for z in G['conditional_upprice_groups']:
 add(f"|{z['variable']} {'增' if z['expanding'] else '缩'}|{z['period']}|{z['active_calendar_days']}|{pct(z['mean_day_equal_weight_market_relative_next_return']) if z['mean_day_equal_weight_market_relative_next_return'] is not None else '无'}|")
add('此表只作机制描述，各日横截面等权后再平均；事件币次数重叠，未做朴素t检验，不把毛收益或分组差异当新增通过信号。真正的统计结论来自上述8候选成本后混合检验。')
add()
add('## 为什么没有通过')
for k in keys:
 z=s['blend_'+k];reason=[]
 if not all(z[p]['sharpe']>b[p]['sharpe'] for p in ['2023-24','2025-26']):reason.append('至少一个子区间未改善')
 if not all(D['bootstrap'][L][k]['p_fwer_all14']<.05 for L in ['15','30','60']):reason.append('全族FWER未达0.05')
 if abs(s[k]['full']['correlation_baseline'])>=.9:reason.append('独立收益相关性≥0.9')
 add(f"- {k}：{'；'.join(reason) if reason else '统计筛查通过，但完整资金费率、真实成交和基线口径尚未验证，不能合格'}。")
add('- 小时清算代理：无真实清算标签、校正不显著、相位离散大、缺早期子区间。')
add('不修改实盘策略。能继续研究的数据路径是认证的HL asset_ctxs与全市场node fills历史，而不是用两天OI或价格暴跌替代清算标签制造显著性。')
add()
add('## 来源与复现')
add('- [Hyperliquid 用户成交接口及历史长度限制](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint)')
add('- [Hyperliquid WebSocket 清算字段](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)')
add('- [Hyperliquid 官方历史数据归档](https://hyperliquid.gitbook.io/hyperliquid-docs/historical-data)')
add('- [Hyperliquid 清算机制](https://hyperliquid.gitbook.io/hyperliquid-docs/trading/liquidations)')
add('- [Binance 官方公开数据仓库](https://github.com/binance/binance-public-data)，[metrics 数据入口](https://data.binance.vision/?prefix=data/futures/um/daily/metrics/BTCUSDT/)')
add('- [Binance 官方账户/持仓指标文档](https://developers.binance.info/en/docs/catalog/core-trading-derivatives-trading-usd-s-m-futures/api/rest-api/market-data)')
add('代码与运行顺序见README.md；公开请求原始状态见api_probes.json；日线SHA256清单见results_daily.json。币安派生逐日观察CSV与失败状态保存在binance_daily_metrics.csv；原始snapshot缓存含URL、状态与行数。')
(OUT/'report.md').write_text('\n'.join(lines)+'\n')
print('wrote report; statistical screen',passed)
