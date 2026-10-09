import json
import numpy as np
from fetch_funding_panel import OUT,save


def main():
    result=json.loads((OUT/'results.json').read_text());audit=json.loads((OUT/'audit.json').read_text())
    summary=[]
    for name in ['raw','residual']:
        for hold in [1,2]:
            keys=[f'{name}_h{hold}_p{p}_base' for p in range(hold+1)]
            r=[result[k] for k in keys]
            row=dict(strategy=name,hold=hold,mean_trades=float(np.mean([x['trades'] for x in r])),funding_income=float(np.mean([x['funding_income'] for x in r])),price_pnl=float(np.mean([x['price_pnl'] for x in r])),fees=float(np.mean([x['fees'] for x in r])),phase_final_equity=[x['final_equity'] for x in r],metrics={})
            for part in ['full','2023-24','2025-26']:
                m=[x['metrics'][part] for x in r];s=[x['sharpe'] for x in m]
                row['metrics'][part]=dict(annual=float(np.mean([x['annual_arithmetic'] for x in m])),sharpe=float(np.mean(s)) if all(x is not None for x in s) else None,phase_sharpes=s)
            summary.append(row)
    save(OUT/'summary.json',summary)
    f=lambda x:'未形成收益序列' if x is None else f'{x:.2f}'
    lines=['# 横截面资金费短持仓研究：本批不晋级','',
           '固定4项假设都未形成能提升组合的合格候选。24小时结构没有任何符合成本门槛的入场；48小时结构只有少量事件，相位结果明显分歧。不能把其中一个赚钱相位单独选出来。','',
           '| 规则 | 持有 | 各相位平均交易次数 | 全期年化 | 全期Sharpe | 2025–26年化 | 2025–26 Sharpe |',
           '|---|---|---|---|---|---|---|']
    for r in summary:
        a=r['metrics']['full'];b=r['metrics']['2025-26']
        lines.append(f"| {r['strategy']} | {r['hold']*24}小时 | {r['mean_trades']:.1f} | {a['annual']:.2%} | {f(a['sharpe'])} | {b['annual']:.2%} | {f(b['sharpe'])} |")
    lines+=['','2023–24全部规则均无达到预先成本门槛的入场；这不能算跨期稳健。24小时无交易时Sharpe不定义，不能写成高Sharpe或以零波动夸大指标。', '',
            '| 48小时规则 | 平均资金费收入 | 平均相对价格盈亏 | 平均交易成本 | 三相位期末权益（初始100,000 USDT） |',
            '|---|---|---|---|---|']
    for r in summary:
        if r['hold']==2:lines.append(f"| {r['strategy']} | {r['funding_income']:.2f} | {r['price_pnl']:.2f} | {r['fees']:.2f} | {', '.join(f'{v:.2f}' for v in r['phase_final_equity'])} |")
    lines+=['','上述各项按相位平均，仅用于对照，不是假设同时投入多份本金。收到了资金费并不等于盈利；这里平均资金费收入不足以覆盖交易成本，价格变化还带来额外盈亏。', '',
            '收益口径为整份账户、日历日、2.7倍gross、每边8bp；另留存15bp、早期资金费mark不利1%和gross=1结果。每笔24/48小时平仓，此后至少24小时现金。未放宽交易门槛或调整持有期来改善本批结果。',
            f"独立现金核对通过：{audit['paths']}条路径、{audit['completed_trades_including_stresses']}笔交易（含压力场景）、{audit['funding_records_verified']:,}条真实资金费记录；两次截断未来检查通过；最大现金差{audit['max_cash_error_usdt']:.3g} USDT，最长持仓{audit['max_hold_hours']:.0f}小时。",'',
            '这仍是研究代理：有限固定币池、历史交易过滤器/费率档位与盘口未完整重建；早期mark使用官方同一整点bar首价（毫秒级时差）替代API缺失值，原始来源逐项标记。保证金日内不利包络没有报警，不等于证明实际逐tick不会清算。','',
            '未把失败候选加入原策略，也未对这4项做显著性通过的主张。当前仍没有保留约100%成本后年化、Sharpe接近3的已验证组合。规则与来源见../FUNDING_PANEL_SPEC.md；完整结果results.json、summary.json、逐笔events.json、资金费来源marks_manifest.json、独立核对audit.json。']
    (OUT/'REPORT.md').write_text('\n'.join(lines)+'\n')
    print('\n'.join(lines[:12]))


if __name__=='__main__':main()
