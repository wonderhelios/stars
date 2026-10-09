"""Report every frozen earnings-calendar rule and the actual data boundary."""
import json
from collections import Counter
from earnings_calendar_screen import OUT


def main():
    results=json.loads((OUT/'results.json').read_text());audit=json.loads((OUT/'audit.json').read_text())
    coverage=json.loads((OUT/'coverage.json').read_text());parsed=json.loads((OUT/'parse_summary.json').read_text())
    trace=json.loads((OUT/'signal_trace.json').read_text())
    fmt=lambda x:'不定义' if x is None else f'{x:.2f}'
    lines=['# 事前财报日历：三种短持仓规则','',
        '**本批三种规则未形成合格候选。** 两个财报前窗口全期成本后亏损；跨财报窗口全期虽盈利，Sharpe仅0.49，2025年以来约0.07，最大历史回撤21.32%。暂不晋级组合验证，不改变持仓、信号方向或挑年份救活结果。','',
        '这是提前公布财报日程的探索性检验，和公告后追涨或指引修订分开。使用通知当时明确的盘前/盘后安排，不把最终实际发布时间倒推成买点。尚未完成与原加密策略的同步估值组合检验，不能据此宣称组合达标。','',
        f'2021-01-01至2026-10-07，10万美元账户，单名20%、总gross1，未用现金0息。收集{audit["source"]["catalogued"]}条通知候选，下载{audit["source"]["downloaded"]}条，核实{audit["source"]["verified_notices"]}条明确发布日和时段；包含研究窗口前基准通知。来源覆盖只有{sum(r["parsed_notices"]>0 for r in coverage)}家，不代表预登记30家公司完整。','',
        '| 规则 | 交易数 | 全期年化 | 全期Sharpe | 全期最大回撤 | 2021–22年化 | 2023–24年化 | 2025–26年化 / Sharpe | 压力全期年化 / Sharpe |',
        '|---|---:|---:|---:|---:|---:|---:|---:|---:|']
    summaries=[]
    labels={'pre1':'发布前1日','pre2':'发布前2日','event2':'跨财报2日'}
    for rule,label in labels.items():
        base=results[rule+'_fee5'];stress=results[rule+'_fee15'];m=base['metrics'];s=stress['metrics']['full']
        lines.append(f'| {label} | {base["diagnostics"]["trades"]} | {m["full"]["annual_return"]:.2%} | {fmt(m["full"]["sharpe"])} | {m["full"]["max_drawdown"]:.2%} | {m["2021-22"]["annual_return"]:.2%} | {m["2023-24"]["annual_return"]:.2%} | {m["2025-26"]["annual_return"]:.2%} / {fmt(m["2025-26"]["sharpe"])} | {s["annual_return"]:.2%} / {fmt(s["sharpe"])} |')
        summaries.append(dict(rule=rule,base=m,stress=stress['metrics'],trades=base['diagnostics']['trades']))
    lines+=['',
        '年化是完整账户连续日历收益的算术年化，Sharpe按日历日计算，现金日保留；不是单笔收益或仅持仓日收益外推。单次公布日的涨跌不用于筛选。基本每侧5bp，压力每侧15bp，历史T+2/T+1卖出结算。6%融资规则参加逐段重算，本批因现金充足，实际融资费用为0。','',
        f'共有{audit["trades_including_stress"]}笔含压力交易，最长{audit["max_holding_hours"]:.1f}小时。所有入场晚于通知可用时点，按预定退出日平仓，同股票无重叠。独立重算买卖现金、手续费、分红、逐段融资和卖出结算，最大逐日净值差{audit["max_daily_nav_replay_error"]:.3g}美元；2023/2025截断行情和通知的信号、收益前缀一致。','',
        'ADSK已取得的通知只足以确认电话会安排，不能据此确定财报发布时点，未用实际财报时间补齐。个别源文件连接失败保留为缺失，没有将失败响应当公告。未取得全部历史改期通知；虽然按已知最新计划逐日更新，仍不能证明所有历史日程变化都已覆盖。实际发布时间仅作事后对照，有偏差也不删除已产生交易。','',
        '预登记参考：[Lamont与Frazzini财报公告溢价研究](https://www.nber.org/papers/w13090)。旧论文并不证明本样本今天仍有溢价。本批只检验三种固定短窗口，不能否定所有事件策略，也不把少数赚钱路径视为证明。拍卖价成交、银行结算日及分红付款日仍为代理；股票池有幸存者偏差，搜索个案已暴露，不是干净样本外。','',
        '## 全部固定股票池覆盖','',
        '| 股票 | 明确事前通知数 | 状态 |','|---|---:|---|']
    for row in coverage:lines.append(f'| {row["symbol"]} | {row["parsed_notices"]} | {"有限来源覆盖" if row["parsed_notices"] else "未取得合格通知，非零事件"} |')
    (OUT/'REPORT.md').write_text('\n'.join(lines)+'\n')
    (OUT/'summary.json').write_text(json.dumps(dict(status='not_qualified_restricted_calendar_exploration',strategies=summaries,
        signal_rejections=dict(Counter(t['reason'] for t in trace if not t['signal']))),indent=2))
    print('\n'.join(lines[:13]))


if __name__=='__main__':main()
