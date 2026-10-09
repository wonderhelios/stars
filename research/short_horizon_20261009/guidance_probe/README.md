# 公司前瞻指引研究：数据来源探测

当前状态：已完成VEEV连续22季源文核对及6项固定规则试验，见../guidance/REPORT.md；未找到达标候选，跨公司扩展尚未完成。目的不是重复已完成的10-Q历史营收增长过滤，而是核实业绩公告时点、公司对同一未来财年的预期及其修订。以下探测日志保留发生时状态，最新结果以报告为准。

用户列出的VEEV/GWRE/TYL是来源探测样本，不能事后挑其中历史赚钱股票作为无偏股票池。三家公司官方季度归档均可在搜索中定位，但本机正常请求全部HTTP403，已停止直接请求这些IR归档，不换身份/代理/浏览器规避。manifest.json保留失败状态；同名.html为失败响应，不能当公告。

独立官方来源SEC submissions（data.sec.gov）三项正常请求也均HTTP403，已停止，未取得业绩申报目录；submissions_manifest.json记录结果。不能把这些失败响应当有效JSON申报目录。可继续查找公司在独立公开新闻发布渠道提供的正式公告，但不绕过上述来源限制，也不能仅靠filingDate冒充首次公告发布时间。

后续在读取新策略盈亏之前再确定固定股票池、信号及持有期。用户原话是“最好”一两天，并非硬性上限；可保留五个交易日对照，但应明确资本占用与更短持仓的代价，不能改写成用户已经接受长持仓。以前冻结的短持仓失败批次不据此改参数救活。

已定位官方来源：
- https://ir.veeva.com/financials/quarterly-results/default.aspx
- https://ir.guidewire.com/financial-information/quarterly-results
- https://investors.tylertech.com/financials/quarterly-results/default.aspx
- https://ir.veeva.com/news/news-details/2024/Veeva-Announces-Fourth-Quarter-and-Fiscal-Year-2024-Results/default.aspx
- https://ir.guidewire.com/news-releases/news-release-details/guidewire-announces-fourth-quarter-and-fiscal-year-2024
- https://www.sec.gov/Archives/edgar/data/860731/000086073124000023/a991earningsrelease-3312024.htm

## 独立新闻发布渠道进展

PR Newswire正常公开请求成功，VEEV公司发布的2024-05-30公告已保存。JSON-LD原发布时间为16:40美国东部时间；全年FY2025收入指引2700–2710百万美元，非GAAP每股盈利约6.16美元。这是未来指引，不能与已经实现的季度数字混淆。该渠道公司目录支持公开分页，guidance_collect.py按目录顺序收集所有英语业绩公告（包括下调/维持），不依赖上涨关键词搜索，未计算本方向绩效。

Business Wire独立发布渠道一次普通请求Guidewire公告返回403，保存GWRE_BusinessWire.html（失败响应，不是公告）；停止该渠道，不换身份重试。

探索偏差记录：定位VEEV2024-05-30原文时，搜索摘要意外出现次日下跌约10%的报道；这一个事件已有结果暴露。后续即使先冻结新规则，也不能将该样本或本次股票名单称为干净样本外。VEEV目录能否完整覆盖所有季度需另行核对，缺失不能被视作“无信号”。

最终核对：12页公司目录、22份连续季度英语公告，从FY2022Q1至FY2027Q2，无季度缺口。22项季度收入均与原文表格金额交叉验证；JSON-LD与目录时间一致。24条年度指引包含两条未来财年预览，初始或不可比不触发；真实前瞻修订只对16次同财年、连续季度计算，8次为正。代码显式禁止用上一财年Q4实际值去调整下一财年初始指引。

这里的完整只指连续季度公告，不能声称已覆盖投资者日、电话会或中间指引更新。interim_guidance_candidates.json列出同一目录2021–2025投资者日通知；材料尚未全部获得。修订信号是季度公告之间的变化，不是相对于市场最新一致预期的意外。2023合同TFC生效日早于所用同年度指引基准；保留电话会材料未全部核对的限制。

Guidewire主公司新闻稿网站（与之前IR不同的独立发布页面）一次正常请求2025-09-04原文返回429，保存GWRE_corporate_20250904.html为失败响应，停止该来源。本轮没有任何未结束的数据采集进程。
