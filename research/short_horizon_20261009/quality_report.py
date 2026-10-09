import json
import pandas as pd
from quality import OUT
from screen import ROOT

def main():
    summary=pd.read_csv(OUT/'phase_summary.csv');cov=pd.read_csv(OUT/'coverage.csv');a=json.loads((OUT/'audit.json').read_text());r=json.loads((OUT/'results.json').read_text());e=pd.read_csv(OUT/'financial_events.csv')
    lines=['# 第二批：财务质量与增长加速的短持仓检验','',
    '研究目标仍是保留约100%成本后年化、将组合Sharpe向3提高，优先1–2天平仓。本报告不会以单独低收益策略或点估计改善代替这个目标。','',
    '## 数据与真正可用时间','',
    f'30个预设发行人全部取得SEC公开财务信息并核对法定名称；ANSS缺失价格，因此实际29只股票。共核对{a["financial_records_verified"]}条截至当时已公开的财务状态。30家中8家大文件遇到传输故障，通过同一公开API的companyconcept接口只取所需字段恢复，原始响应、失败记录与SHA256均保留。接口范围见[SEC官方文档](https://www.sec.gov/search-filings/edgar-application-programming-interfaces)。',
    '每个申报仅在filed之后第二个交易日开盘才可使用。营收与经营利润需同一起止期；去年可比期长度相近且期末相隔350–380天；每次只读取当时已filed的版本。季度增速与季度比较，全年与全年比较。缺失增速加速度保持未知，不从其他周期填值；期末超过210天的旧数据失效。',
    '**这是已公开财务信息筛选，不是财报新闻公布瞬间交易，也不是真正的“超预期”因子。** 尚未取得系统性的历史一致预期与管理层指引；用10-Q/10-K申报日期冒充财报新闻日期会误导。TEAM使用当前取得的发行人历史，早期财务覆盖不足；不能说所有股票2015年起覆盖完整。','',
    '## 用户提到的三家公司的最新可用状态','',
    '|公司|可用日期|口径|营收同比|经营利润率|较上一同类期增速变化|','|---|---|---|---:|---:|---:|']
    for sym in ['VEEV','GWRE','TYL']:
        row=e[e.symbol==sym].iloc[-1]
        lines.append(f'|{sym}|{row.available}|{row.kind}|{row.growth:.2%}|{row.margin:.2%}|{row.acceleration:.2%}|')
    lines += ['', '这些数字只说明截至当时的财务状态，不构成买入建议。尤其TYL没有通过预先设定的“营收同比>10%且增速加快”筛选，因此三只推荐股票不能简单解释成同一个高速增长因子。可能还涉及业务稳定性、估值或不同持有期，不能为让名单全通过而事后降低门槛。','',
    '## 预设6项结果','',
    '三类信号：财务质量回调、增长加速回调、新财报加速且价格反应有限；各测试首日/次日收盘退出。次日退出两个起点分别运行。单公司20%、总gross不超过1，10万美元账户，空仓计零收益；包含分红、实际数量漂移、交易费、融资与卖出款结算应收。单边5bp为基准、15bp为压力，均为情景假设而非真实经纪商历史报价。','',
    '|信号|2015–22 Sharpe|2023–24 Sharpe|2025–26 Sharpe|后段年化|18项校正p|','|---|---:|---:|---:|---:|---:|']
    names={'quality_dip':'财务质量回调','quality_accelerating_dip':'增长加速回调','quality_new_filing':'新财报加速/有限价格反应'}
    for name,g in summary[summary.strategy.str.startswith('quality_')].groupby('strategy'):
        rows=g.set_index('part');base,h=name.rsplit('_h',1)
        lines.append(f'|{names[base]}/{h}日|{rows.loc["2015-22","sharpe"]:.2f}|{rows.loc["2023-24","sharpe"]:.2f}|{rows.loc["2025-26","sharpe"]:.2f}|{rows.loc["2025-26","annual_return"]:.2%}|{rows.loc["2025-26","p_fwer_worst_block"]:.3f}|')
    lines += ['', 'Sharpe按各起点分别计算后平均；算术年化含空仓日，不能把交易时的资金收益直接全年化。所有路径、压力成本、CAGR和回撤见quality/results.json。','',
    '4999次15/30/60日循环块maxT，联合首批12项共18个经济假说，采用最差块长p。此校正仍不能消除整个项目过去大量研究和固定当前股票池的选择偏差。历史后段属于时间分割验证，不称作完全未见样本。','',
    '## 审计与结论边界','',
    f'所有选用的本期营收、同期经营利润和去年可比营收已逐条回到原始申报观测核对。2018/2024截止的财务特征前缀一致。独立按完成交易重算权益最大差{a["max_trade_reconciliation_error"]:.2e}美元；持仓最长{a["max_holding_hours"]:.1f}小时，没有同资产重叠持仓。',
    '股票池仍有幸存者偏差；拍卖成交价代理、实际滑点、银行结算日历和历史借券/融资条件未全部验证。不同市场估值时间未统一前，不公布把股票收盘与加密UTC日线直接拼接的高Sharpe组合。原策略完整资金费审计仍需补齐。',
    '本批结果只能决定哪些机制值得继续，不证明推荐者的真实判断过程；也不能仅因筛选后波动小就宣称完成高收益目标。']
    # Mechanical gate: deliberately stricter than selecting a visually good late period.
    passes=[]
    for name,g in summary[summary.strategy.str.startswith('quality_')].groupby('strategy'):
        rows=g.set_index('part')
        if all(rows.loc[p,'p_fwer_worst_block']<.05 and rows.loc[p,'annual_return']>0 for p in ['full','2023-24','2025-26']): passes.append(name)
    lines.insert(2,'本批初筛通过者：'+(', '.join(passes) if passes else '无。不能晋级组合或实盘验证。'))
    (OUT/'REPORT.md').write_text('\n'.join(lines)+'\n')
    print('Passes:',passes)

if __name__=='__main__':main()
