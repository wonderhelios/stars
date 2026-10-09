"""Report all frozen pilot paths, without selecting a new rule."""
import json
from pathlib import Path
import pandas as pd
from guidance_screen import OUT
from screen import ROOT


def main():
    result=json.loads((OUT/'results.json').read_text())
    audit=json.loads((OUT/'audit.json').read_text())
    rows=json.loads((ROOT/'guidance_probe/guidance_rows.json').read_text())
    manifest=json.loads((OUT/'manifest.json').read_text())
    lines=['# 剩余财年收入指引：VEEV试验结果', '',
        '**没有达到高收益或组合晋级条件。** 这是用户点名公司的探索性机制试验，不是无偏股票池或干净样本外。全年指引上调不等于未来季度预期上调；本次检验先扣除了本季度已经实现的收入超预期。', '',
        f'覆盖 {manifest["start"]} 至 {manifest["end"]}，22份连续季度公告、16次可比修订、8次正向触发。10万美元账户，每个名字占账户20%，其余现金不计息。下表为全账户、成本后、日历日算术年化；不是单笔收益的年化外推。', '',
        '| 入场/持仓 | 笔数 | 全期年化 | 全期Sharpe | 2023–24年化 | 2025–26年化 / Sharpe | 压力全期年化 | 最长持仓 |',
        '|---|---:|---:|---:|---:|---:|---:|---:|']
    summary=[]
    for name,label in [('guidance_immediate','公告后开盘'),('guidance_pullback','等第一次回调')]:
        for hold in [1,2,5]:
            key=f'{name}_h{hold}_fee5';r=result[key];m=r['metrics'];d=r['diagnostics']
            stress=result[f'{name}_h{hold}_fee15']['metrics']['full']
            fmt=lambda x:'不定义' if x is None else f'{x:.2f}'
            lines.append(f'| {label} / {hold}日 | {d["trades"]} | {m["full"]["annual_return"]:.2%} | {fmt(m["full"]["sharpe"])} | {m["2023-24"]["annual_return"]:.2%} | {m["2025-26"]["annual_return"]:.2%} / {fmt(m["2025-26"]["sharpe"])} | {stress["annual_return"]:.2%} | {d["max_hold_hours"]:.1f}小时 |')
            summary.append(dict(strategy=key,**m['full'],trades=d['trades'],late=m['2025-26'],stress=stress))
    lines += ['',
        '1/2日版本最多48小时，跨周末等不能按时退出的事件跳过；5日版本最长174.5小时，仅作占用资金更久的对照。两种入场的可成交次数不同，不能偷偷改成共同事后样本。基本成本每侧5bp、压力每侧15bp，融资6%，卖出款按历史T+2/T+1结算。未回放历史盘口，属于开收盘价成交代理。', '',
        '## 一个可核实的区别', '',
        '2024年8月公告将FY2025全年收入区间中点从2705提高至2707百万美元，看似上调2百万美元。但该季度实际收入676.2百万美元，比前次季度指引666–669的中点高8.7百万美元。扣除已经实现的部分，剩余季度预期实际减少6.7百万美元。不能仅凭“上调全年预期”的标题做多。', '',
        '来源：[2024年5月原公告](https://www.prnewswire.com/news-releases/veeva-announces-fiscal-2025-first-quarter-results-302159990.html)、[2024年8月原公告](https://www.prnewswire.com/news-releases/veeva-announces-fiscal-2025-second-quarter-results-302233348.html)。该计算比较公司前后两份指引，不是相对于分析师一致预期，也没有证明它能预测股价。', '',
        '## 核验和限制', '',
        f'22份公告的日期与发布目录一致；季度收入与原文财务报表独立交叉核对通过。7份正文修改时间晚33–67秒，入场按较晚时间再加一小时。12条基本/压力路径、74笔含压力交易，权益重算最大差{audit["max_cash_error"]:.3g}美元；2023/2025截断检查通过；旧引擎默认结果差为0。', '',
        '季度目录连续，不代表穷尽全部中间公告、投资者日演讲或分析师一致预期。已在同一公开目录找到2021–2025投资者日通知；尚未取得全部会议材料，因此本信号严格说是“季度公告之间的修订”，不能称作市场第一次获得的意外上修。VEEV的FY2024合同变更从2023-02-01生效，早于所用2023-03-01年度基准；本计算不混用跨年度营收增长。完整的电话会口径材料仍缺。', '',
        '只有8次触发，无法据此推断朋友判断的真实依据，或证明不存在更长期的基本面优势。今天选出的股票、2024年5月单例搜索结果暴露、有限源文覆盖均使本研究不具备干净样本外资格。未做高精度显著性承诺，未拼接不同估值时点的原加密组合，也未宣称提高组合Sharpe。', '',
        '后续股票池已在读取本批收益前写入GUIDANCE_EXPANSION_SPEC.md，保持先前30家公司，不按这次盈亏选股。使用同一六条规则，先完成各公司源文和口径审计，再评价总体；该扩展尚未完成。', '',
        '## 全部前瞻修订记录', '',
        '| 公告日期 | 目标财年结束 | 全年收入中点（百万美元） | 已实现季度超预期（百万） | 剩余季度修订（百万） |',
        '|---|---|---:|---:|---:|']
    for r in rows:
        rng=r['revenue_range_million'];s=r['realized_quarter_surprise_million'];v=r['remaining_year_revision_million']
        lines.append(f'| [{r["published"][:10]}]({r["source"]}) | {r["fiscal_year_end"]} | {sum(rng)/2 if rng else "非有限区间"} | {f"{s:.1f}" if s is not None else "不可比/初始"} | {f"{v:.1f}" if v is not None else "不可比/初始"} |')
    (OUT/'REPORT.md').write_text('\n'.join(lines)+'\n')
    (OUT/'summary.json').write_text(json.dumps(dict(status='pilot_not_qualified',strategies=summary),indent=2))
    print('\n'.join(lines[:15]))


if __name__=='__main__':main()
