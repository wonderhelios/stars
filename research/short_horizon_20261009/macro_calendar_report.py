"""Complete frozen configurations, account-level costs and capital occupation."""
import json
import numpy as np
import pandas as pd
from macro_calendar_screen import OUT,CONFIGS


def main():
    results=json.loads((OUT/'results.json').read_text());audit=json.loads((OUT/'audit.json').read_text())
    inference=json.loads((OUT/'inference.json').read_text());trace=json.loads((OUT/'signal_trace.json').read_text())
    trades=pd.read_csv(OUT/'trades.csv');costs={}
    days=(pd.Timestamp('2026-10-07')-pd.Timestamp('2021-01-01')).days+1
    for key,r in results.items():
        ledger=pd.read_csv(OUT/f'{key}_ledger.csv').set_index('date');prior=ledger.nav.shift(1).fillna(100000.)
        fees=ledger.fees.diff().fillna(ledger.fees.iloc[0]);finance=ledger.finance.diff().fillna(ledger.finance.iloc[0])
        ts=trades[trades.strategy==key];turn=0.
        for t in ts.itertuples():
            for field,price in [('entry',t.entry_price),('exit',t.exit_price)]:
                date=str(pd.Timestamp(getattr(t,field)).tz_convert('America/New_York').date())
                turn+=t.qty*price/prior.loc[date]
        costs[key]=dict(annual_turnover_both_sides=turn*365/days,
            annual_fee_drag=float((fees/prior).sum()*365/days),annual_financing_drag=float((finance/prior).sum()*365/days))
    fmt=lambda x:f'{x:.2f}' if x is not None else '不定义'
    lines=['# 事前议息日历：股票、黄金与长期美债','',
        '**八项固定假设全部不晋级。** 全期最高单位资金年化只有0.88%、Sharpe 0.28；该长期美债两日方案在2025年以来略亏。固定等权组合也没有改善，不能作为保留原策略约100%年化、将Sharpe推近3的证据。','',
        '2021-01-01至2026-10-07，10万美元独立账户。SPY、GLD、TLT分别代表美国大盘股票、黄金基金、长期美债基金；不是黄金现货或期货实盘成交。pre1为会议最后一天前一交易日开盘至当日收盘，event2为同一开盘至会议最后一天收盘。每侧5bp基本成本，15bp压力成本，空仓现金零息，负现金融资6%。','',
        '| 配置 | 全期年化 | CAGR | Sharpe | 最大回撤 | 2021–22年化 / S | 2023–24年化 / S | 2025–26年化 / S |',
        '|---|---:|---:|---:|---:|---:|---:|---:|']
    for name in CONFIGS:
        m=results[name+'_g1_fee5']['metrics'];f=m['full']
        parts=' | '.join(f'{m[p]["annual_return"]:.2%} / {fmt(m[p]["sharpe"])}' for p in ['2021-22','2023-24','2025-26'])
        lines.append(f'| {name} | {f["annual_return"]:.2%} | {f["cagr"]:.2%} | {fmt(f["sharpe"])} | {f["max_drawdown"]:.2%} | {parts} |')
    lines+=['','所有收益均为整个账户的连续日历收益，保留现金日；不是把少数持仓日收益外推成年化。Sharpe相对于零收益现金，不是相对于无风险利率的超额Sharpe。未与原加密策略拼接：原核心完整资金费与跨市场同步估值尚未完成。','',
        '## 信号时点与来源','',
        '47份原始会议纪要及公开日期逐项核实，其中46项下一次会议处于研究窗口。45项产生交易；2021-09-22会议纪要页面最后修改于2021-11-26，晚于计划的2021-11-03会议，因此保守延迟可用时间、跳过这次入场，没有回填旧版本。另1项计划的2026-10-28会议在样本外。实际会议日期只作事后核对，不作为历史信号。','',
        '每份纪要明确写出下一次会议日期，使用正式发布日期与页面最后修改日中较晚者，再等至次日纽约时间01:00。2020年12月纪要于2021-01-06公开，不能在2020年12月使用。原文证据：[2020年12月发布页](https://www.federalreserve.gov/monetarypolicy/fomcpresconf20201216.htm)、[2021年9月纪要](https://www.federalreserve.gov/monetarypolicy/fomcminutes20210922.htm)。未证明所有临时改期消息都已归档，当前版本只能作保守探索。','',
        '## 真实账户与资金回笼','',
        '| 配置 | 事件 / ETF订单 | 最长持仓小时 | 入场至卖出款结算小时：中位 / 最长 | 交易日平均开盘gross | 年双边换手 | 年交易费 / 融资拖累 |',
        '|---|---:|---:|---:|---:|---:|---:|']
    for name in CONFIGS:
        key=name+'_g1_fee5';d=results[key]['diagnostics'];a=audit['per_path'][key];c=costs[key]
        count=sum(t['configuration']==name and t['eligible'] for t in trace)
        lines.append(f'| {name} | {count} / {d["trades"]} | {d["max_hold_hours"]:.1f} | {a["entry_to_sale_settlement_hours_median"]:.1f} / {a["entry_to_sale_settlement_hours_max"]:.1f} | {d["mean_open_gross_in_research_window"]:.2%} | {c["annual_turnover_both_sides"]:.2f}倍 | {c["annual_fee_drag"]:.2%} / {c["annual_financing_drag"]:.2%} |')
    lines+=['','单ETF主方案单次最多使用约100%独立账户，三资产组合各约1/3；日历平均占用较低源于一年约八次事件。卖出款按历史T+2/T+1交易日结算，分红除息日记应收、30天后付款代理；负现金按实际等待区间计融资。表中成本为各日费用除前收盘净值后年化，双边换手包括入场和退出。结算用交易所日历，银行假日差异及券商现金可提取时间尚未核实，不能承诺48小时内提款。','',
        '## 全部成本与杠杆敏感性','',
        '| 配置 | gross | 每侧bp | 全期年化 / S | 2025–26年化 / S | 累计融资美元 | 保证金压力标记 |',
        '|---|---:|---:|---:|---:|---:|---:|']
    for key,r in results.items():
        f=r['metrics']['full'];late=r['metrics']['2025-26'];d=r['diagnostics']
        lines.append(f'| {r["configuration"]} | {r["gross"]:g} | {r["fee"]*10000:g} | {f["annual_return"]:.2%} / {fmt(f["sharpe"])} | {late["annual_return"]:.2%} / {fmt(late["sharpe"])} | {d["finance"]:.2f} | {d["adverse_margin_flags"]} |')
    lines+=['','gross2.7只是预登记融资敏感性，未确认真实券商初始保证金许可；日内最低价与25%维持保证金只是压力代理。放大仓位不创造独立收益来源，也不能当提高Sharpe的证明。','',
        '## 核验与推断','',
        f'32条成本/杠杆路径、{audit["trades_including_stress_and_gross"]}笔含敏感性ETF订单（不是2160个独立事件）。逐日现金、分红、融资、费用、卖出结算独立重算，最大净值差{audit["max_daily_nav_replay_error"]:.3g}美元。两次截断（2023年底、2025年6月底）的全部32路径过去信号及收益不变；旧共享引擎默认策略回归误差为0。没有同名重叠，等权组合入场名义金额一致。','',
        '15/30/60日移动循环块bootstrap各9999次，对本批8项主方案做maxT，并对本系列累计43项登记假设给出保守Bonferroni校正；全部主方案全期最保守校正p值均为1。该计算是稀疏事件下的近似推断，只覆盖本系列，不能声称控制了整个项目全部搜索，也不检验原组合增量。完整分段结果见inference.json。','',
        '日线开收盘是成交代理，未取得历史盘口和精确公告前下午时点；pre1没有覆盖公告前整个隔夜时段，event2又包含公告后的价格反应。因此不能把本批失败扩大为否定所有公告前溢价。经济机制参考：[美联储对股市传导及公告效应的综述](https://www.federalreserve.gov/econres/feds/the-effect-of-the-federal-reserve-on-the-stock-market-magnitudes-channels-and-shocks.htm)。既有历史已看过，本批不是干净样本外；不反向、不挑年份或改窗口救活。']
    (OUT/'REPORT.md').write_text('\n'.join(lines)+'\n')
    (OUT/'summary.json').write_text(json.dumps(dict(status='completed_not_qualified',hypotheses=8,cumulative_series_hypotheses=43,
        qualified=[],costs=costs,events_traded=45),indent=2))
    (OUT/'run_status.json').write_text(json.dumps(dict(status='completed_not_qualified',market_performance_read=True,
        source_audit_complete=True,cash_audit_complete=True,inference_complete=True),indent=2))
    print('\n'.join(lines[:17]))


if __name__=='__main__':main()
