from collections import Counter
import json
import numpy as np
from fetch_options import OUT


def number(x, percent=False):
    return '不可用' if x is None else (f'{x:.2%}' if percent else f'{x:.2f}')


def main():
    results=json.loads((OUT/'results.json').read_text())
    ledgers=json.loads((OUT/'ledgers.json').read_text())
    audit=json.loads((OUT/'audit.json').read_text())
    manifest=json.loads((OUT/'entry_manifest.json').read_text())
    selections=json.loads((OUT/'selections.json').read_text())
    summary=[]
    for coin in ['BTC','ETH']:
        for hold in [1,2]:
            row=dict(coin=coin,hold=hold)
            for fee in [5,15]:
                names=[f'fomc_long_{coin}_h{hold}_p{phase}_fee{fee}' for phase in range(3)]
                for part in ['full','2023-24','2025-26']:
                    m=[results[n]['metrics'].get(part) for n in names]
                    valid=all(a is not None and a['sharpe'] is not None for a in m)
                    row[f'{part}_fee{fee}']=dict(sharpe=float(np.mean([a['sharpe'] for a in m])),annual=float(np.mean([a['annual_arithmetic'] for a in m])),phase_min=min(a['sharpe'] for a in m),phase_max=max(a['sharpe'] for a in m)) if valid else None
            names=[f'fomc_long_{coin}_h{hold}_p{phase}_fee5' for phase in range(3)]
            events=[e for n in names for e in ledgers[n]]
            row.update(mean_events_filled=float(np.mean([results[n]['events_filled'] for n in names])),mean_imbalanced=float(np.mean([results[n]['imbalanced_events'] for n in names])),mean_account_spend=float(np.mean([e['cost']/e['cash_before'] for e in events])),phase_cash_pnl=[results[n]['cash_pnl'] for n in names],phase_status=[results[n]['status'] for n in names])
            summary.append(row)
    (OUT/'summary.json').write_text(json.dumps(summary,indent=2))
    lines=['# 议息事件短期买入期权：成交代理研究','',
           '**本批没有形成合格候选。** 基本模型49次有成交事件全部为单腿或两腿数量不平衡，没有一次等量建成跨式；不能据此验证完整的事件波动率策略。以下权益使用历史普通买方向成交作为买入代理，没有历史盘口，不能把成交假设当成真实订单结果。','',
           f"固定27次议息事件、4项假设、3个入场相位、两档成本。取得{sum(r['status']=='complete' for r in manifest)}/54个事件币种完整时间窗，共{sum(r.get('rows',0) for r in manifest):,}笔原始成交。选约状态：{dict(Counter(r['status'] for r in selections))}。",'',
           '| 结构 | 全期年化/Sharpe | 2023–24 Sharpe | 2025–26年化/Sharpe | 后段相位范围 | 压力成本后段Sharpe | 平均有成交事件/27 | 每事件实际投入账户比例 |',
           '|---|---|---|---|---|---|---|---|']
    for r in summary:
        full=r['full_fee5'];early=r['2023-24_fee5'];late=r['2025-26_fee5'];stress=r['2025-26_fee15']
        pair=lambda a:'不可用' if a is None else number(a['annual'],True)+' / '+number(a['sharpe'])
        span='不可用' if late is None else f"{late['phase_min']:.2f}–{late['phase_max']:.2f}"
        lines.append(f"| {r['coin']}，{r['hold']}日到期 | {pair(full)} | {number(early['sharpe'] if early else None)} | {pair(late)} | {span} | {number(stress['sharpe'] if stress else None)} | {r['mean_events_filled']:.1f} | {r['mean_account_spend']:.3%} |")
    lines+=['','“不可用”需区分原因：BTC两日结构有两处持仓中间日没有规定时限内的mark；ETH两个结构的10:05相位整段没有成交，Sharpe不定义。它们不是同一种数据缺失，下面分别列出。', '',
            '| 币种/到期 | 入场UTC | 有成交事件 | 全期现金盈亏（USD） | 全期Sharpe | 指标限制 |',
            '|---|---|---|---|---|---|']
    for coin in ['BTC','ETH']:
        for hold in [1,2]:
            for phase,hour in enumerate([8,9,10]):
                r=results[f'fomc_long_{coin}_h{hold}_p{phase}_fee5']
                limit='；'.join(f'{d}: {why}' for d,why in r['invalid']) if r['invalid'] else '零成交，Sharpe不定义' if not r['events_filled'] else '成交与估值代理'
                sr=r['metrics'].get('full',{}).get('sharpe')
                lines.append(f"| {coin}/{hold}日 | {hour:02d}:05 | {r['events_filled']} | {r['cash_pnl']:.2f} | {number(sr)} | {limit} |")
    lines+=['','年化为整个10万美元账户的日历日算术年化；收益没有按投入权利金或持仓小时放大。三个相位等权汇总的是指标，不是三条策略同时共享一份资金。未成交事件保留为现金，未把缺失中间估值填成零。现金盈亏即使完整，也不能替代缺失日频估值来计算Sharpe。','',
            '入场限价、数量和合约均由当时已知信息决定；普通成交每笔最多10%参与率，并加入价格跳动及换汇成本。总预算2%是上限，实际投入可能远低于它。单腿或不等量成交仍持有到期，不能称完全中性跨式。',
            f"独立核对{audit['fills_checked']}笔代理成交，{audit['point_in_time_checks']}次截断选约检查；最大现金误差{audit['max_cash_error_usd']:.3g}美元，最长持仓{audit['max_hold_hours']:.3f}小时。审计只证明记账与规则一致，不证明代理成交可实现。",'',
            '两日到期结构中间日采用此前最多一小时的历史交易所mark，存在陈旧估值；次日到期结构在每日08:00估值时已清仓。未做与原策略同步08:00估值的组合检验，未做新增4项与此前18项的联合显著性检验，不能据表中个别正值宣布发现alpha或Sharpe达标。','',
            '所有到期均保守收交割费（含实际可能豁免者）。2026-08-01后的两步结算按官方说明保持净支付不变。资金转换用交割TWAP加价差代理，仍须取得更完整现货报价才能核实。压力情景增加价格跳动后，部分打印不再落在预设限价内，因此成交清单也会变化；压力结果偶尔改善不是更高成本带来优势的证据。','',
            '规则见../OPTIONS_SPEC.md；原始数据raw/及entry_manifest.json；逐笔现金流ledgers.json；每日权益各*_nav.csv；独立核对audit.json。', '',
            '来源：[美联储日历](https://www.federalreserve.gov/monetarypolicy/fomccalendars.htm)、[Deribit成交字段](https://docs.deribit.com/api-reference/market-data/public-get_last_trades_by_currency_and_time)、[逆向期权及结算规则](https://support.deribit.com/hc/en-us/articles/31424939096093-Inverse-Options)、[2020年交易费调整](https://insights.deribit.com/exchange-updates/deribit-lowers-option-trading-fees-to-make-the-market-more-accessible-to-retail-traders/)。']
    (OUT/'REPORT.md').write_text('\n'.join(lines)+'\n')
    print('\n'.join(lines[:14]))


if __name__=='__main__':main()
