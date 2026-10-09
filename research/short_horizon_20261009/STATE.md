# 持续研究状态

更新时间2026-10-09。目标仍未完成，尚无保留约100%成本后年化且Sharpe接近3的已验证组合。

已完成第一批：47个固定标的请求，46个成功，ANSS公开历史不可用；12项规则、18个相位路径、5/15bp两档成本，2015–2026历史。结果全部未通过。REPORT.md、phase_summary.csv、inference.json、audit.json已保存。无未来信息前缀差，独立权益重算最大差4.95e-10美元；最长持仓30.5小时。stock_gap_up_h1近期S1.36/年化4.59%，全期S0.27且早期负、校正p0.2228，不晋级。不要继续对第一批做阈值网格。

已完成Gemini整数代际1/2/3/4的4次官方发布案例审计：gemini_events.csv，完整记录次日开盘后的1/2/5/20交易日。短窗口有正有负；Gemini 3五日约12.52%不代表通用发布策略。用户具体是哪次未知，不能用案例冒认其13%收益来源。黄金10月7日月报不能回填10月1日前判断；9月21日周报可用。VEEV/GWRE/TYL行业软件共同逻辑是推断，不是推荐者真实方法的证据。NAM未确认，暂不使用。

持续目标已激活；当前线程每4小时heartbeat已创建，automation id `automation`。继续已有步骤，勿重复创建调度或重复启动未结束任务。只有实质新结果通知。

此前结论：原策略统一窗口算术年化113.2%、Sharpe1.28、波动88.8%，历史资金费尚未完整计入；低波动三策略年化5.4%、Sharpe5.81不符合用户高收益目标，不能包装为完成。

当前下一步：探测公开SEC companyfacts数据，加入公告当时可见的营收/利润信息，独立登记新批次再读取表现；若无法取得完整事件历史，继续宏观预定事件窗口研究，不停下来询问用户。不要将价格跳空称作已验证利好。普通隔夜漂移2026年已有纽约联储复核，不能照搬旧论文结果。后续仍需做跨市场同一时刻估值的组合验证、完整原策略资金费；这些未完成前不能声称达到目标。

运行环境：`/Users/wonder/miniconda3/bin/python`（命令python）；系统python3缺依赖。第一批命令fetch.py、screen.py、inference.py、events.py、audit.py、report.py在本目录，原始公开价格已缓存。避免重新抓取和重跑已完成批次。实盘、src和其他任务的文件保持不动。

SEC companyfacts首个VEEV探测在requests TLS握手时失败（未关闭证书验证），正在用正常curl再次确认连接；这不是已获得的数据。失败后可转向官方IR历史公告与公开宏观日历，不应将数据源连接故障当整个研究阻塞。

更新：系统curl成功取得SEC VEEV companyfacts（2.7MB，含2026-08-27 filed数据），已复制到fundamentals/VEEV.json。QUALITY_SPEC.md已冻结第二批6个财务条件信号，尚未测试；需与第一批12项联合校正。fetch_fundamentals.py正在下载ticker映射及固定股票池公开财报，日志fundamentals_fetch.log；继续前先看日志/进程，避免重复启动。下一实质步骤是实现按filed逐日可见的财务特征，严格使用同起止期并审计重述值，随后复用screen.simulate而不改第一批结果。

SEC ticker目录返回403，已停止访问该目录；改为直接使用公开发行人CIK访问已验证可用的companyfacts接口，并逐个校验返回法定名称。fetch_fundamentals.py重启一次，需检查fundamentals/manifest.json的name_verified与错误，不使用未匹配的主体。

财报下载当前运行会话11227，日志fundamentals_fetch.log。后续读取财报仅使用manifest中name_verified=true的公司；如TEAM历史主体变更、旧CIK覆盖截止，必须记录，不能把缺失前后财报填充伪装成全覆盖。第一批结果与事件审计已完成，无需重复运行。

本次自动接续对上一轮分类为progress：第一批回测、独立审计和事件证据已落盘；当前已验证下载会话11227仍活跃。quality.py已实现按filed保守滞后、同类周期增长、完整观测传播与210日期末过期规则，尚未读取本批绩效。先完成30项下载manifest，网络TLS/超时可作一次正常重试，403目录不可重试。

财报数据下载完成：初次30项中22项完整，余8项通过官方companyconcept小字段响应正常恢复，30项name_verified全部通过。恢复进程已完成（会话57623）。第二批quality.py运行会话80680，输出quality.log，完成后运行quality_audit.py，以及inference.main(quality/combined_returns.npz, quality/combined_results.json, quality)，最后quality_report.py。目前尚未读取绩效。

第二批已完成：quality.py会话80680结束；1442条财务状态逐项核对原申报值通过；2018/2024截断无前缀差；独立权益误差3.06e-10美元；联合18项maxT。新增6项后段年化全负（约-0.87%至-7.92%），全部否决。报告quality/REPORT.md，统计quality/phase_summary.csv；不重新调阈值。TYL当前总营收增长8.22%且增速放缓，说明朋友三股名单不能归结为同一高速增长筛选。

下一方向：核对Deribit公开历史期权真实成交与合约信息，优先事件前后24–48小时、损失有限的结构；旧research/vrp_20261008只检验DVOL差，不能当策略收益。options_probe正在探测2024年到期期权的真实成交与定义，必须先核验数据覆盖及可执行性再登记新批次。缺bid/ask不能拿理论价充数，也不宣称已经可回测。

期权探测完成（会话36166、78994均已结束）：公开history.deribit.com可取2024到期期权真实成交和定义。options_probe/coverage.json记录ATM63000 C/P六小时22/36笔（21/35笔普通成交），无配对完整跨式combo；60000-C首五笔全部combo，不能当独立单腿价格。options_probe/README.md列官方文档和下一步。历史bid/ask仍未取得，尚未测试期权收益，不得称已发现候选。下一步在事件前用全币种期权成交构建当时交易合约池，创建时间过滤，再预登记损失有限的事件多头策略；若仅能成交代理，应明确筛选而非可实现验证。

第三批推进：OPTIONS_SPEC.md在绩效读取前冻结4项FOMC买入短期期权假设（BTC/ETH × 次日/后日到期）、3个入场相位、两档成本。正式事件27个，已排除2025-08-22非定期议息的notation vote。先前下载51625在修复该日历过滤时停止；目前下载会话34440，日志options_fetch.log，准备会话69937，日志options_prepare.log，收尾会话18908，日志options_finish.log。收尾程序等待两份完成标记后，自动依次运行options_screen.py、options_audit.py、options_report.py。继续前核对这些现有进程和日志，勿重复启动。程序只访问公开数据，不下单。

执行代理限定普通买方向成交，剔除combo/block，按预先限价/数量，15分钟窗口、10%打印数量参与率、每次账户2%总预算。部分或单腿成交保留至到期。2026-08-01交割转同到期逆向期货，但官方说明净支付相同且不双收费；公式保留制度边界。缺少历史盘口，全部只能作代理研究。跨市场未统一08UTC估值，不得拼接组合。

中途覆盖检查（未读取到期盈亏）：前79个已核对选约中65个没有符合成交，14个不平衡成交；平均实际投入账户0.03247%，最大0.62485%。这提示容量不足，不能把未建成的跨式当完整结构收益。后续年份仍在获取，不根据早期覆盖调整规则，最终报告全部事件、相位及不平衡比例。若最终无成交或容量不足，应归为该执行规则未形成可验证候选，不宣称排除了所有事件期权策略。

独立备用数据探测：黄金Dukascopy公开小时BID文件首次超时，一次普通重试HTTP429后已停止，gold_probe/README.md和probe_source.json留档。未付费、未用凭据、未绕过限流，未测试黄金新绩效。后续可找合法可用的日内bid/ask数据研究独立宏观时段机制，不能把失败响应当行情文件。

本轮接续分类为progress：第三批54/54事件币种窗口下载已完成，会话34440已确认exit0；options_prepare会话69937仍在逐项核对，options_finish会话18908仍等最终完成标记。最新日志至2026年初，继续核对同一进程，不重启。第三批尚未读取到期绩效，结果以收尾日志和最终文件为准。

第四批已在数据字段核对后、绩效读取前冻结FUNDING_PANEL_SPEC.md：16币币安同所永续横截面7日资金费，原始及剔除趋势/波动后的残差排序，各24/48小时，平仓后至少24小时现金；只在预估资金费覆盖预设成本时入场，4项新增假设，累计26项本系列探索假设。fetch_funding_panel.py会话65085、funding_marks.py会话54656均exit0。共65,804条结算资金费，早期14,316条缺mark由官方markPriceKlines整点首价显式代理，最大时差29毫秒，未偷换为现货价格；余51,488条为资金费记录实际mark。

第四批funding_screen.py、funding_audit.py、funding_report.py全部完成：40条模型路径（含压力与单位gross），80笔交易，现金重算最大差7.28e-11 USDT，两个截断未来检查通过，最长48小时、至少1天现金。24小时全部无入场；48小时raw全期相位均值S-0.46/年化-0.20%，后段S-0.63/年化-0.38%；residual全期S-0.54/年化-0.98%，后段S-0.74/年化-1.87%。两规则2023–24均未入场，不能满足跨期稳健；raw有一个赚钱相位，但禁止挑选。资金费收入平均不足以覆盖交易费用。4项均不晋级，不做无意义的原组合混合。结果funding_panel/REPORT.md、summary.json、audit.json，日志funding_screen.log/funding_audit.log。不要为了救活结果调门槛或反转信号。

独立机制来源核对：unlock_probe/README.md记录APT/ARB/OP预期释放规则。ARB官网称2024-03-16首解锁，但官方部署脚本1678968508+365天得2024-03-15 12:08:28；合约按2628000秒一档。尚未验证实际部署地址/参数，不得把源码常数当全体投资者已执行时钟。OP官方说释放表为示意，APT缺精确时点，因此未回测解锁策略。GitHub匿名tree API403后停止该API，仅阅读可公开访问的原始源码；没有链上交易或访问任何钥匙。

下一优先步骤：等现有第三批选约/收尾完成，查看options_events/REPORT.md、audit.json及未成交/不平衡结构比例，必要时修复记账错误但不修改冻结策略。仍无达标组合。后续可研究真正公告时点的公司全年收入/盈利指引修订（与此前按10-Q日期的财务水平筛选不同），但需完整事前指引及发布时点，不能直接拿今天的好消息名单回溯赢家；或继续核实解锁的真实链上时钟。保留原策略完整资金费和同步估值两项未完成限制。

第三批最终完成：69937和18908均已确认exit0。54/54时间窗95,582笔成交；324条选约中298成功、26无事前合格对；24条路径核对通过，127笔代理成交（含压力）、298次前缀检查，现金差2.91e-11美元，最长47.916小时。基本模型49次有成交事件全部不等量或单腿，没有等量建成跨式，不能声称验证了完整事件波动率结构。BTC次日到期全期相位均值年化-0.20%、S-0.07，后段年化-0.07%、S-0.36。BTC两日有两处中间mark真缺失，不报完整S；ETH10:05两期限皆零成交，S不定义，不能把此混称为缺数据。所有相位现金盈亏已补进options_events/REPORT.md，最高累计盈利也仅约1089美元/10万美元账户，未达目标；不晋级，不修改执行规则救活。压力跳动会改变限价内成交清单，其结果不是原交易只多扣费。没有正在运行的第三/四批任务。

下一研究来源已探测：guidance_probe/README.md。VEEV/GWRE/TYL官方IR季度页本机直接请求均403，已停止，没有换身份绕过。独立SEC submissions正在获取三家公司业绩相关8-K与历史目录，检查最新submissions_manifest.json及工具会话后再继续，不能把目录当已获得指引。此方向尚未登记/计算绩效。用户“一两天”是偏好而非绝对限制，后续真正新机制可固定保留5交易日对照，明确占用资金；既有短持仓批次仍保持冻结失败记录。

最后核对：guidance SEC submissions会话10024已exit0，但三份请求均HTTP403，未获得目录；当前无本轮遗留下载/回测进程。勿重试被拒来源或切换身份绕过。guidance_probe/README.md已修正，可继续检索公司在独立公开新闻发布渠道（如公司正式发布的新闻稿）中的原文及历史日期。公司未来全年指引修订尚未测试，不得与已否决的历史财报增长因子混称为同一结果。全目标仍active，本轮完成第三/四批并取得实质否决证据，属于progress，不是全研究受阻。

第五批本轮已完成VEEV单公司机制试验，属于progress，全目标仍active。独立公司新闻分发渠道PR Newswire正常公开访问200；guidance_collect.py收集12页公司目录和FY2022Q1至FY2027Q2连续22份季度原文，无季度间隙。一次SSL失败作一次普通重试后成功；全部下载完成，会话39758exit0（原6187失败退出），无遗留采集任务。BusinessWire首条GWRE普通请求403、主公司www.guidewire.com独立新闻稿页面首条普通请求429，均停止该来源，失败HTML不能作原文。旧IR/SEC403仍不重试。

GUIDANCE_SPEC.md在绩效读取前冻结6项：剩余财年收入修订=本次全年收入中点−前次同年中点−（本季度实际收入−前次季度指引中点）；严格正值。即时入场或20交易日内第一次负一标准差QQQ相对回调，各持有1/2/5交易日。前两者最多48小时、5日最多8日历日；单股20%账户、总gross1、现金0息、5/15bp侧、融资6%与T+2/T+1。GUIDANCE_EXPANSION_SPEC.md也在读取VEEV策略绩效前锁定原30股跨公司扩展，无论试验结果如何不按盈亏挑股票。前五批累计32项假设（非全项目总数）。

数据和审计：guidance_extract.py保留24条年度指引（含两条未来财年预览），同年连续可比16次、正向8次。不能以FY2022Q4实际值扣FY2023初始指引，已按reported_fiscal_year强制过滤。7份修改时间晚33–67秒，按较晚时间+60分钟；均盘后发布。22份实际季度收入与正文独立财务表数核对通过，日期匹配目录；FY2024TFC变更2023-02-01生效早于2023-03-01初始年度基准。但季度完整不代表投资者日/中间指引或电话会完整，interim_guidance_candidates.json保留5次2021–2025投资者日通知，未下载全部材料；不能称市场第一次获知的意外上修。搜索时暴露过2024-05-30个案下跌方向，全试验不是干净样本外。

guidance_screen.py会话35149和guidance_audit.py会话32259均exit0。12条基本/压力路径、74笔含压力交易；现金重算最大差2.91e-11美元、两个截断检查通过、无重叠、最长174.5小时。screen.py只新增可选max_hold_hours/phase_filter参数支持新批次5日与事件自然时点，原默认策略不变；重跑第一批gap_up_h2_p0_fee5与冻结返回最大差0。guidance/REPORT.md、summary.json、audit.json已落盘。即时h1全期年化0.15%、S0.18、后段年化−0.38%/S−0.34；其余5基本方案全期负年化/负S；所有6项后段均负。没有组合晋级候选，不做无意义拼接，不改变正号/阈值/持仓救活结果。

新证据例子：2024-08-28全年营收中点较前公告仅上调2百万美元，但当季实际676.2比旧季度666–669中点高8.7，剩余季度预期实际下降6.7百万。该机制有区别于新闻标题的经济含义，但尚未显示交易优势。下一步按已冻结30股范围建立完整覆盖清单，优先可正常访问的公司原始发布渠道；通用财务解析必须逐公司审计，只有季度/订阅/ARR指引不可冒充全年总收入。不要再重复跑已经完成VEEV六项，不下单，不问用户重复问题。目前没有本轮遗留运行进程。原核心完整资金费与跨资产估值同步仍待完成，仍未找到年化约100%且Sharpe约3的合格组合。

新一轮跨公司扩展取得实质progress：guidance_expansion/coverage_status.json列全部原30股，无按盈亏删股。独立原文正常可用渠道PR Newswire（MDB/ADSK/WDAY/SNPS）、GlobeNewswire（DDOG）、Salesforce公司新闻室。已下载101份公告记录：MDB27、ADSK28、WDAY19、CRM14、DDOG13；它们不是独立交易，也不代表全窗口已覆盖。guidance_multi_collect.py MDB ADSK WDAY会话25647已确认exit0，guidance_other_collect.py DDOG会话42646已exit0，CRM初次79506已exit0，修正重复内容检测后95592已exit0。初始探测80872/67589/37177以及MongoDB独立目录探测45784均exit0。没有重复启动旧进程。

MongoDB源文guidance_parse_mdb.py通过27/27提取，覆盖FY2020Q4到FY2027Q2连续27季、20组同年可比修订，尚未读取市场绩效。新2026版把季度/全年预测分两张表，已经按附近实际标题解析，禁止仅按位置猜列。原文表格精确季度收入与正文约数最大差0.048百万、日期一致、修改滞后最多127秒，保留较晚时间。guidance_mdb_sec_audit.py利用已缓存fundamentals/MDB.json独立核对27/27实际收入和SEC申报编号：Q4=同年首次10-K减原9个月值；FY2025年报在公告后16天，年度核验窗口30天并限定同FY/FP=FY（季度15天）。这仅是事后核验，不把申报日或后来数字当交易信息。另一个官方MDB季度目录首次普通请求403已停止，SEC缓存可核实已实现季度但不能代替中间指引新闻清单。MDB会计/收购口径与中间指引审计仍pending。

发现真实覆盖问题，不能吞掉：DDOG发布渠道在2021–2025中间改PR Newswire，GlobeNewswire目录13份有缺口；已从官方原文找到PRN公司目录https://www.prnewswire.com/news/datadog%2C-inc./。CRM官方earnings目录分页重复最新14条，首次按不同page请求至第12页404后退出；现已加重复文章集合检测，重跑只复用缓存前两页并下载原始14份，未重试404。旧年份和FY2025Q3仍缺，不可将其视为无信号。WDAY19份起于2022年，Financial Outlook检查未见总收入指引而为订阅收入，早期仍未检查，不能全历史排除。ADSK28份含预告+正式报告同季重复，需保留先后并审核新交易模式/会计调查口径。SNPS原文确认2025-07-17Ansys收购及报告期变化，不能直接拼出“有机增长”信号。

当前仍在运行的唯一新任务：guidance_multi_collect.py SNPS DDOG_PRN，工具会话21186，日志guidance_additional_collect.log。先检查这个确切会话和日志再继续，不因观察超时重启。SNPS和DDOG_PRN资料分别保存guidance_expansion/SNPS与DDOG_PRN，后者symbol字段为DDOG，需与GlobeNewswire DDOG目录去重合并，不能覆盖旧13份downloaded清单。已完成的MD/ADSK/WDAY不重复下载。后续继续公司级财务源文解析、同时补30股覆盖与缺口，仍不准按子集赚钱与否选择股票。guidance_expansion/README.md记载当前状态。尚未计算扩展组合绩效，目标active，非blocked。

本轮新增实质progress：21186已确认exit0，SNPS27份及DDOG_PRN13份全部下载完成。新增DDOG缺失2024Q4第三个发布渠道Newsfile（官网原文明确来源Newsfile）；正常请求200，会话70028已exit0，原文在guidance_expansion/DDOG_NEWSFILE。当前没有遗留采集进程。现跨公司原始记录142份（MDB27+ADSK28+WDAY19+CRM14+DDOG Globe13+SNPS27+DDOG PRN13+Newsfile1），另有之前VEEV22；这些不是独立交易数量。

guidance_parse_adsk.py完成28/28提取，27个正式季度连续，另1个预告；guidance_mdb_sec_audit.py已扩为MDB/ADSK/DDOG三个固定发行人独立核验，三家各27季度原值全部通过，MDB原核验重跑仍27/27。ADSK例外是原FY2024年报2024-06-10延迟申报（季度值1469m=年度5497m−原9个月），及FY2025Q1 10-Q在2024-06-10提交、正式新闻稿2024-06-11发布。两者按具体申报编号0000769397-24-000090/91单独核对，不扩大所有时点窗口，也不回填早期交易信息。5月31日预告只有实际营收约1.42bn，6月11日正式1417m，同季不得再次当成新财报信号；原始值和先后均保留。

ADSK在绩效读取前登记source_review_gates.json：2021-05-27年度指引加入之前明确排除的Innovyze收入、2026-08-27加入之前排除的MaintainX收入，两组无法由原文分离未来收购收入，按既定口径一致要求排除。预告约数和同季度正式重复也no_new_signal。新交易模式FY2025初始指引已讨论，因此不能把名义GAAP收入修订称作纯有机需求，仍需完成剩余口径/中间新闻审计。数值原始计算19组不等于19组合格信号，未来回测必须使用这些源文资格门槛。

guidance_parse_ddog.py合并三个来源27/27、连续2019Q4–2026Q2、20组同年数值可比修订。Globe/PRN发布时间JSON-LD与页面/目录交叉核对；Newsfile缺JSON-LD和modified，使用正文可见2025-02-13 07:00 EST并与已读公司官方目录对照，modified存null而非伪造未修改，available_after按该已知发表时点。27项原始季度值全部匹配缓存SEC。DDOG收购和其他中间指引口径仍pending；scope_review_candidates.json只是粗筛候选（含导航和财务表噪音），不是已通过的审核。

WDAY19份Financial Outlook或CFO明确指引段落已全部读完，只有订阅收入指引，没有总收入指引，applicability_review.json标记这19份不适用固定总收入因子。早期2021年仍缺，不自动排除该年度。coverage_status.json列全部30股，仍有多数未完成，不允许把缺失当零信号。SNPS27份尚需解析52/53周日历、2025Ansys收购与停止经营口径；CRM14份仍有旧年及FY2025Q3缺口。跨公司市场绩效尚未读取；不要重复VEEV已失败六项，也不挑下载顺利的几家当最终整体结果。下一步继续SNPS/CRM源文解析和其余固定股票池覆盖，同时完成已有3家公司资格审计；整体目标仍active且未达成，不是blocked。


本轮完成实质源文审计progress，目标active，未找到达标组合，跨公司市场绩效仍未读取。没有遗留进程：CRM API探测25633、IR/SNPS检查21815、附件探测35076均exit0。

SNPS解析、独立核验27/27完成：guidance_parse_snps.py保留真实财周末、展示月末及原文展示规则，17季二者不同。最初18项不匹配是因为SEC XBRL采用展示月末，且全年收入有两个GAAP标签；guidance_snps_sec_audit.py只接受原文明示两端点、原始同FY申报和精确金额，Q4原年报减原9个月，修复后全通过。SNPS/source_review_gates.json预先排除2024-05-22停止经营、2025-09-09新Ansys范围、2026-05-27渠道会计+$60m和Processor IP出售-$40m无法桥接比较。2024额外周和2025财历转换在初始指引已体现，不误删所有后续比较。

CRM公开WordPress接口从既有HTML发现，普通请求200：/news/wp-json/wp/v2/sf_press_release?search=results&per_page=100&page=1。CRM/api_results_01.json保存100条响应，api_financial_candidates.json筛出连续24季FY2021Q3至FY2027Q2，补齐旧年/FY2025Q3。guidance_crm_source_audit.py验证无季度缺口及Pacific/UTC一致，写api_source_audit.json/api_release_audit.json。6篇modified晚于published超过24h；全部无HTML财务表，指引图片、实际收入约数，不能精确回填。原文版本、数字和口径资格仍pending，未访问WP私有revisions。

Salesforce IR原文普通请求403停止，ir_probe_2025q1.meta.json留证。搜索发现公开Yahoo保存的原始8-K附件https://cdn.yahoofinance.com/prod/sec-filings/0001108524/000110852425000027/crm-q1fy26xexhibit991.htm，正常200，CRM/exhibit_2025q1.html含25张HTML表，准确季度收入9829m、Q2FY26指引10110–10160m、FY26全年41000–41300m。仍需原申报时间及其他原版本核验。已发现但未下载：https://cdn.yahoofinance.com/prod/sec-filings/0001108524/000110852424000020/crm-q2fy25xexhibit991.htm；https://cdn.yahoofinance.com/prod/sec-filings/0001108524/000110852425000002/crm-q4fy25xexhibit991.htm。勿重试被拒IR/SEC submissions，不绕过权限。

MDB新增source_review_gates.json排除2026-05-28 Clarity无法桥接范围；DDOG同文件排除2021-05-06 Sqreen、2025-05-06 Eppo/Metaplane、2026-08-06 Adaptive ML无法核实同口径比较。这是保守数据资格门槛，不是收购一定重大收入的结论，其他中间新闻仍pending；后续screen必须加载门槛。coverage_status.json更新SNPS/CRM，README已重写去除过时状态。

此次网页搜索暴露社交媒体所称CRM2026年8月盘后上涨14%，未核实、非研究结果，不据此改规则/选股；不能宣称干净样本外。新发现但未启动的公开目录：https://www.prnewswire.com/news/oracle/、https://www.prnewswire.com/news/oracle-corporation/、https://www.prnewswire.com/news/hubspot/。HubSpot业绩似主要BusinessWire/公司IR，不能假定PRN完整。下一步完成可用公司的源文资格与时点、补固定30股覆盖，再执行全部6规则；受限探索必须明示覆盖限制，不把成功下载子集当完整样本。原核心资金费和同步估值限制仍未解决，继续高收益独立策略目标。


第六批已完成，属于实质progress，目标继续active。为检验独立的“事前财报日历溢价”，读取市场表现前冻结EARNINGS_CALENDAR_SPEC.md三项：pre1最后一个财报前完整交易日开盘到收盘；pre2前两日；event2最后财报前交易日开盘至次一交易日收盘。全部最多48h、单名20%、gross1、5/15bp每侧、真实现金与历史结算、6%融资。只用提前正式公布的未来日期和盘前/盘后，不能用实际财报日期倒推，固定30股不变、覆盖不足全列。本系列累计35假设，不含全项目旧研究。

采集过程全部结束：初探54436exit0；初采27620在一个URL两次正常SSL失败后结束；修改采集器只让此失败留缺失、对其他公开URL继续，3并发且403/429停止同源，10958exit0。没有重试耗尽URL、没有换身份。共141候选，140正常原文，1条ADSK连接失败。113条可明确核实，SNPS27/MDB26/DDOG19/VEEV22/WDAY19；27条ADSK只有电话会安排，无法确定财报发布时点，不自动补。113条计划日期与独立已保存实际公告日期全部一致，但实际日期只作事后审计、不作信号。部分公司历史覆盖不全，尤其DDOG19不能冒充27季度。CRM既有API结果中没有日期通知，不造事件。

新文件earnings_calendar_collect.py、earnings_calendar_parse.py、earnings_calendar_screen.py、earnings_calendar_audit.py、earnings_calendar_report.py。parse正确识别DDOG盘前/盘后历史变更；VEEV2023-01-31通知中当日期末不当发布日；WDAY“plans to announce”与“will announce”均需明确财报/盘前后证据。所有通知max(pub,modified)+60min，日程按可用时刻逐日更新，同季度每规则最多一次入场，固定退出不因事后改期或实际结果删交易。计划排序用真实Timestamp，不混用时区字符串排序。

source-only核验15113、回测96416、独立现金核验2591均exit0；没有遗留进程。6条基本/压力路径、606笔含压力交易，最长30.5h；独立逐日重算订单现金、费用、分红、融资及结算，净值最大差1.16e-10美元，2023/2025两个截断行情+通知前缀误差全0。账户现金充足，本批实际融资0，并非漏掉融资。source元数据的market_performance_read=false描述绩效前解析时点；当前已读取绩效，终态run_status.json记录。

结果earnings_calendar/REPORT.md、summary.json、audit.json：pre1全期年化−0.79%/S−0.35，pre2−0.18%/S−0.05；event2全期年化5.90%/S0.49/回撤21.32%，2021–22年化13.14%/S1.12、2023–24年化3.05%/S0.27、2025–26年化0.94%/S0.07，压力全期5.17%/S0.43。三规则不形成高收益高Sharpe候选，不晋级组合、不改符号/门槛/持仓、不挑年份救活；没有必要对未通过方向再做高精度显著性承诺。本批是5家公开资料受限探索，非完整30股、非干净样本外；无法证明所有改期通知已归档。

继续工作时避免重复旧方向：已重读research/liq_oi_20261008/report.md，其6/12/24h放量暴跌反弹与普通暴跌对照已全部失败，而且只2026小时样本，不应重新包装同一OHLCV代理为新清算因子。原核心资金费仍不完整；multisleeve/fetch.py曾成功分页取得BTC/ETH/SOL全期HL fundingHistory，但原资金费额需要历史oracle价，只有fundingRate/premium不能唯一重建oracle，不可把日价代理包装完整精确费用。当前14日核心框架不改。第五批跨公司指引原版/范围审计仍未完成，已有source_review_gates必须保留。后续可继续该扩展，或先核对真正事前宏观日历的短持仓独立机制；本轮尚未冻结或启动宏观回测。整体100%净年化/S3目标仍未达到。

第七批宏观日历已完整完成，实质progress但目标未达。MACRO_CALENDAR_SPEC.md在本批绩效前冻结SPY/GLD/TLT及等权mix × pre1/event2八项，累计本系列43假设。32路径为gross1/2.7 × 每侧5/15bp；开盘买入，最长48h，单ETF可使用子账户全额，mix等名义；6%融资、历史T+2/T+1结算、分红显式。共享screen.simulate仅新增默认per_name_cap=.2参数，ETF调用显式覆盖；旧stock_gap_up_h2_p0_fee5回归误差0。

采集47份Fed原始纪要完成，会话20930确认exit0；2020年12月公开日期证据页dec2020_publication.html正常200，会话97298exit0。纪要仅使用明确下一次会议安排，实际发布日期和Last Update较晚日结束后再留1h；当前日历仅查源/事后核验。46项下一会议在窗口内，45项可入场。2021-09-22纪要Last Update2021-11-26晚于计划11月3日，保守不可用，不回填旧版本；最后一项计划2026-10-28在样本外。47哈希/引文/发表日期全核实。CPI普通原文403已停止，不混入此批，失败响应不可当数据。

macro_calendar_audit.py --sources-only、macro_calendar_screen.py、macro_calendar_audit.py串行会话27082已exit0。32路径2160笔含杠杆/成本情景ETF订单（45独立会议，不是2160独立事件）；最长30.5h。独立逐日现金/结算/分红/融资/费重算最大误差2.91e-10美元；2023年底/2025年6月底所有32路径源文+行情截断信号与收益误差0。等权入场金额、无重叠、无未来源文通过。没有遗留进程。

macro_calendar_inference.py与macro_calendar_report.py均exit0，REPORT.md/summary.json/audit.json/inference.json/run_status.json已落盘终态completed_not_qualified。8主方案15/30/60日块各9999 bootstrap，本批maxT及43假设Bonferroni；全期最保守Bonferroni p全部1。仅本系列近似推断，不覆盖项目全部搜索，不是组合增量检验。全期单位资金最佳TLT_event2年化0.88%/S0.277，2023–24年化5.55%/S1.50，但2025–26年化−0.05%/S−0.022；mix_event2全期−0.25%/S−0.091、后段−1.96%/S−0.803，其他主方案全期年化负。8项全部不晋级、不反向不调时点。pre1持仓6.5h、入场至卖出款结算最长48h；event2持仓30.5h、结算最长72h（银行/券商提款仍未验证）。每年约八次低占用并不等于高收益。2.7仅融资敏感性，未验证经纪保证金许可，不当可执行收益。

暂停前的下一可执行步骤：继续已冻结第五批30股跨公司全年指引修订扩展，完成原版/范围/中间指引资格与覆盖，不挑成功下载子集、不重复已否决的VEEV六项；已有MDB/ADSK/DDOG/SNPS各27季源文与SEC实际数核验，CRM原始8K镜像精确数字及其他固定股票池尚待覆盖。必须加载source_review_gates。原核心完整资金费和跨市场同步估值仍未解决，不能称100%年化/S3目标达到。

最新get_goal实际返回status=usageLimited，tokensUsed=1340269、timeUsedSeconds=9682，非active、非complete、非blocked。本轮停止新增研究，保留全部进度；不要自行调用update_goal伪报完成或绕过用量限制。已有automation心跳不重复创建，后续需遵守届时实际运行状态。未下单、未改src/实盘配置、未提交/推送、未发送外部消息。
