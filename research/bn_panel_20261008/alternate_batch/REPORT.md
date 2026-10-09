# TxFlow 币安日线与符号映射校验

采集日期：2026-10-09 北京时间（UTC 2026-10-08）。只做研究数据；未改策略、未 commit/push/部署；TxFlow 仅一次 `POST /info {"type":"perpMeta","dex":""}`，未调用 explorer。

## 覆盖统计

TxFlow 共 **227** 个市场，返回清单全部无 haltTrading/delisted/onlyIsolated 标记。币安现货候选 115 个，其中 XMRUSDT、LITUSDT 为 BREAK。下载 **113** 个 symbol、**97,154** 根完整日线。

严格通过并进入白名单 **82** 个 ✓，其中币安/TxFlow 单位不同的倍数合约 **1** 个 ✓（这是通过数的子集）。价格通过但成交量待复核 **0** 个；有币安数据但 Hyperliquid 无法完成验证 **31** 个；价格拒绝 **0** 个；无可用币安现货日线 **114** 个。以上互斥状态合计 227。

本批选择 **币安 USDT 现货**。永续接口曾可返回 BTC 探针，但之后 exchangeInfo 返回 HTTP 418 / code -1003：代理出口 IP 被封至 2026-10-09 02:12:58.741 UTC；已停止永续请求。**不是 HTTP 451，未证实地区限制。** 不能把「本批现货没有」写成「币安现货和永续都没有」。原始错误见 `raw/bn_futures_exchange.json`；大响应超时的部分文件只作诊断，绝未进入数据面板。

## 校验口径与单位

固定数据截止：2026-10-08 00:00:00 UTC，最新允许的日线起点是 2026-10-07 00:00 UTC。每个现货币最多最近 1000 根，不补值、不伪造。比较最近最多 7 个双方都有的 UTC 完整日线；收盘属于相同日历日，而不是各自最新一根。详细同日样本保存在 `raw/validation_<TxFlow base>.json`。

`txflow_unit`、`binance_unit`、`hl_unit` = 每一交易报价单位包含的基础币枚数。现货 binance_unit=1；TxFlow 1000BONK=1000（元数据 description 明确），PEPE/SHIB=1；Hyperliquid kBONK/kPEPE/kSHIB=1000。币安没有选用 1000PEPE/1000SHIB 永续，所以本批 PEPE/SHIB 的币安→TxFlow 倍数是 1。实际 TxFlow 名称为 PEPE-USDC，接口中没有 PEPU-USDC；禁止把 PEPU 猜成 PEPE。

表中「倍数 M」是 `binance_unit / txflow_unit`；从币安换成 TxFlow 报价单位：**价格 ÷ M，成交量 × M**。例如 BONKUSDT→1000BONK-USDC：M=0.001，价格×1000，量÷1000。允许明确单位的倒数，不能只硬匹配 1/100/1000/10000。

表中「价格比」= `(币安 close / binance_unit) / (HL close / hl_unit)`；「成交量比」= `(币安 volume × binance_unit) / (HL volume × hl_unit)`。两者都已归一化为同一种基础币。额外列出未归一化原始价比，保留千倍证据。对所有重叠日要求价格比落在 [0.95,1.05]；>10000 或 <0.0001 拒绝，中间非单位比也拒绝。映射需先有明确名称/单位证据，价格接近不充分证明资产身份。

成交量比任何一天超出 [0.01,100]（至少相差两数量级），或 HL 零量，标为量待复核并禁入白名单。不同交易所活跃度不同，量差大是风险提示，不能单凭量差认定单位错误。`quote_volume_ratio` 使用币安真实 quote volume / (HL close×HL volume)，HL 后者只是收盘价近似，不是精确成交额。

## 完整映射清单（每个市场一行）

价格/量列为最新共同日；判定依据全部重叠日。— 表示不能测量，绝不填 1 或 0。币安 symbol 是候选，只有 ✓ 通过才在白名单。

| TxFlow名 | 币安symbol（现货） | 倍数 M | 价格比（归一） | 成交量比（归一） | 原始价比 BN/HL | HL symbol | 日期 / 样本数 | 判定 |
|---|---|---:|---:|---:|---:|---|---|---|
| BTC-USDC | BTCUSDT | 1 | 1.00024 | 0.61773 | 1.00024 | BTC | 2026-10-07 / 7 | ✓ 通过 |
| ETH-USDC | ETHUSDT | 1 | 1.00029 | 0.795745 | 1.00029 | ETH | 2026-10-07 / 7 | ✓ 通过 |
| BNB-USDC | BNBUSDT | 1 | 1.00038 | 9.33751 | 1.00038 | BNB | 2026-10-07 / 7 | ✓ 通过 |
| BCH-USDC | BCHUSDT | 1 | 1.0004 | 3.6257 | 1.0004 | BCH | 2026-10-07 / 7 | ✓ 通过 |
| LTC-USDC | LTCUSDT | 1 | 1.00024 | 1.98273 | 1.00024 | LTC | 2026-10-07 / 7 | ✓ 通过 |
| ETC-USDC | ETCUSDT | 1 | 0.998857 | 5.86356 | 0.998857 | ETC | 2026-10-07 / 7 | ✓ 通过 |
| LINK-USDC | LINKUSDT | 1 | 1.0003 | 1.82072 | 1.0003 | LINK | 2026-10-07 / 7 | ✓ 通过 |
| ADA-USDC | ADAUSDT | 1 | 1.00067 | 4.76267 | 1.00067 | ADA | 2026-10-07 / 7 | ✓ 通过 |
| ATOM-USDC | ATOMUSDT | 1 | 1.00145 | 3.34687 | 1.00145 | ATOM | 2026-10-07 / 7 | ✓ 通过 |
| DOGE-USDC | DOGEUSDT | 1 | 1.00018 | 7.74542 | 1.00018 | DOGE | 2026-10-07 / 7 | ✓ 通过 |
| DOT-USDC | DOTUSDT | 1 | 0.999642 | 4.05433 | 0.999642 | DOT | 2026-10-07 / 7 | ✓ 通过 |
| CRV-USDC | CRVUSDT | 1 | 1.00045 | 0.565284 | 1.00045 | CRV | 2026-10-07 / 7 | ✓ 通过 |
| SOL-USDC | SOLUSDT | 1 | 1.0006 | 0.881858 | 1.0006 | SOL | 2026-10-07 / 7 | ✓ 通过 |
| UNI-USDC | UNIUSDT | 1 | 0.999394 | 2.05331 | 0.999394 | UNI | 2026-10-07 / 7 | ✓ 通过 |
| AVAX-USDC | AVAXUSDT | 1 | 1.00045 | 2.92821 | 1.00045 | AVAX | 2026-10-07 / 7 | ✓ 通过 |
| FIL-USDC | FILUSDT | 1 | 1.00057 | 4.82063 | 1.00057 | FIL | 2026-10-07 / 7 | ✓ 通过 |
| AAVE-USDC | AAVEUSDT | 1 | 1.00017 | 1.85681 | 1.00017 | AAVE | 2026-10-07 / 7 | ✓ 通过 |
| XRP-USDC | XRPUSDT | 1 | 1.00049 | 1.82695 | 1.00049 | XRP | 2026-10-07 / 7 | ✓ 通过 |
| APT-USDC | APTUSDT | 1 | 1.00052 | 2.99907 | 1.00052 | APT | 2026-10-07 / 7 | ✓ 通过 |
| TRX-USDC | TRXUSDT | 1 | 1.00051 | 10.6247 | 1.00051 | TRX | 2026-10-07 / 7 | ✓ 通过 |
| ARB-USDC | ARBUSDT | 1 | 1.00146 | 1.07144 | 1.00146 | ARB | 2026-10-07 / 7 | ✓ 通过 |
| OP-USDC | OPUSDT | 1 | 1.00041 | 4.95922 | 1.00041 | OP | 2026-10-07 / 7 | ✓ 通过 |
| LDO-USDC | LDOUSDT | 1 | 1 | 1.80955 | 1 | LDO | 2026-10-07 / 7 | ✓ 通过 |
| NEAR-USDC | NEARUSDT | 1 | 0.998839 | 0.718441 | 0.998839 | NEAR | 2026-10-07 / 7 | ✓ 通过 |
| SUI-USDC | SUIUSDT | 1 | 1.00062 | 3.35639 | 1.00062 | SUI | 2026-10-07 / 7 | ✓ 通过 |
| WLD-USDC | WLDUSDT | 1 | 1.00086 | 1.79032 | 1.00086 | WLD | 2026-10-07 / 7 | ✓ 通过 |
| XLM-USDC | XLMUSDT | 1 | 1.0007 | 5.35722 | 1.0007 | XLM | 2026-10-07 / 7 | ✓ 通过 |
| ICP-USDC | ICPUSDT | 1 | 0.999877 | 3.92777 | 0.999877 | ICP | 2026-10-07 / 7 | ✓ 通过 |
| SEI-USDC | SEIUSDT | 1 | 1.00086 | 3.61722 | 1.00086 | SEI | 2026-10-07 / 7 | ✓ 通过 |
| HBAR-USDC | HBARUSDT | 1 | 0.999139 | 6.71331 | 0.999139 | HBAR | 2026-10-07 / 7 | ✓ 通过 |
| CAKE-USDC | CAKEUSDT | 1 | 1.00009 | 12.426 | 1.00009 | CAKE | 2026-10-07 / 7 | ✓ 通过 |
| CRO-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| MNT-USDC | — | — | — | — | — | MNT | — / 0 | ✗ 无可用数据 |
| 1000BONK-USDC | BONKUSDT | 0.001 | 1.00057 | 1.65018 | 0.00100057 | kBONK | 2026-10-07 / 7 | ✓ 通过 |
| ONDO-USDC | ONDOUSDT | 1 | 0.99932 | 1.21207 | 0.99932 | ONDO | 2026-10-07 / 7 | ✓ 通过 |
| TAO-USDC | TAOUSDT | 1 | 0.999794 | 1.69822 | 0.999794 | TAO | 2026-10-07 / 7 | ✓ 通过 |
| JUP-USDC | JUPUSDT | 1 | 1.00025 | 1.08806 | 1.00025 | JUP | 2026-10-07 / 7 | ✓ 通过 |
| STRK-USDC | STRKUSDT | 1 | 1.0002 | 2.77269 | 1.0002 | STRK | 2026-10-07 / 7 | ✓ 通过 |
| ENA-USDC | ENAUSDT | 1 | 0.999735 | 0.64991 | 0.999735 | ENA | 2026-10-07 / 7 | ✓ 通过 |
| POL-USDC | POLUSDT | 1 | 1.00068 | 6.17027 | 1.00068 | POL | 2026-10-07 / 7 | ✓ 通过 |
| VIRTUAL-USDC | VIRTUALUSDT | 1 | 1.00021 | 2.83851 | 1.00021 | VIRTUAL | 2026-10-07 / 7 | ✓ 通过 |
| PENGU-USDC | PENGUUSDT | 1 | 1.00035 | 2.23654 | 1.00035 | PENGU | 2026-10-07 / 7 | ✓ 通过 |
| HYPE-USDC | HYPEUSDT | 1 | 1.00086 | 0.0538773 | 1.00086 | HYPE | 2026-10-07 / 7 | ✓ 通过 |
| TRUMP-USDC | TRUMPUSDT | 1 | 1.00091 | 4.63857 | 1.00091 | TRUMP | 2026-10-07 / 7 | ✓ 通过 |
| XAUT-USDC | XAUTUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| PUMP-USDC | PUMPUSDT | 1 | 1.00032 | 0.239821 | 1.00032 | PUMP | 2026-10-07 / 7 | ✓ 通过 |
| SKY-USDC | SKYUSDT | 1 | 1.00111 | 1.08412 | 1.00111 | SKY | 2026-10-07 / 7 | ✓ 通过 |
| WLFI-USDC | WLFIUSDT | 1 | 1.00152 | 6.35336 | 1.00152 | WLFI | 2026-10-07 / 7 | ✓ 通过 |
| ASTER-USDC | ASTERUSDT | 1 | 1.00083 | 3.10321 | 1.00083 | ASTER | 2026-10-07 / 7 | ✓ 通过 |
| XAU-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| XAG-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| CL-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| MSTR-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| CRCL-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| XPD-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| TSLA-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| COIN-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| XPT-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| COPPER-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| EWJ-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| AERO-USDC | AEROUSDT | 1 | 0.999096 | 1.10157 | 0.999096 | AERO | 2026-10-07 / 7 | ✓ 通过 |
| BZ-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| 1INCH-USDC | 1INCHUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| ANKR-USDC | ANKRUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| AXL-USDC | AXLUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| BLUR-USDC | BLURUSDT | 1 | 1.00021 | 1.03571 | 1.00021 | BLUR | 2026-10-07 / 7 | ✓ 通过 |
| CELR-USDC | CELRUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| COTI-USDC | COTIUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| ETHFI-USDC | ETHFIUSDT | 1 | 0.999488 | 1.76156 | 0.999488 | ETHFI | 2026-10-07 / 7 | ✓ 通过 |
| GMX-USDC | GMXUSDT | 1 | 1.00032 | 3.06766 | 1.00032 | GMX | 2026-10-07 / 7 | ✓ 通过 |
| HOOD-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| INJ-USDC | INJUSDT | 1 | 1.0008 | 3.20306 | 1.0008 | INJ | 2026-10-07 / 7 | ✓ 通过 |
| JASMY-USDC | JASMYUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| LINEA-USDC | LINEAUSDT | 1 | 0.999268 | 2.14331 | 0.999268 | LINEA | 2026-10-07 / 7 | ✓ 通过 |
| META-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| MU-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| SPY-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| AAPL-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| ALICE-USDC | ALICEUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| AMZN-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| API3-USDC | API3USDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| AR-USDC | ARUSDT | 1 | 1.00026 | 3.33742 | 1.00026 | AR | 2026-10-07 / 7 | ✓ 通过 |
| ARKM-USDC | ARKMUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| AXS-USDC | AXSUSDT | 1 | 1.00096 | 13.8343 | 1.00096 | AXS | 2026-10-07 / 7 | ✓ 通过 |
| BAND-USDC | BANDUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| BAT-USDC | BATUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| CC-USDC | — | — | — | — | — | CC | — / 0 | ✗ 无可用数据 |
| CELO-USDC | CELOUSDT | 1 | 1.0002 | 5.25816 | 1.0002 | CELO | 2026-10-07 / 7 | ✓ 通过 |
| CETUS-USDC | CETUSUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| CHZ-USDC | CHZUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| COMP-USDC | COMPUSDT | 1 | 1.00052 | 6.43647 | 1.00052 | COMP | 2026-10-07 / 7 | ✓ 通过 |
| CYBER-USDC | CYBERUSDT | 1 | — | — | — | CYBER | — / 0 | ✗ 未校验 |
| DYDX-USDC | DYDXUSDT | 1 | 0.999779 | 4.80112 | 0.999779 | DYDX | 2026-10-07 / 7 | ✓ 通过 |
| FET-USDC | FETUSDT | 1 | 1 | 6.03144 | 1 | FET | 2026-10-07 / 7 | ✓ 通过 |
| FLOW-USDC | FLOWUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| GAS-USDC | GASUSDT | 1 | 1.00111 | 4.65156 | 1.00111 | GAS | 2026-10-07 / 7 | ✓ 通过 |
| GOOGL-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| GRT-USDC | GRTUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| HOT-USDC | HOTUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| ID-USDC | IDUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| IMX-USDC | IMXUSDT | 1 | 1.00017 | 1.42427 | 1.00017 | IMX | 2026-10-07 / 7 | ✓ 通过 |
| INTC-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| IOTA-USDC | IOTAUSDT | 1 | 0.996474 | 5.10417 | 0.996474 | IOTA | 2026-10-07 / 7 | ✓ 通过 |
| IOTX-USDC | IOTXUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| JTO-USDC | JTOUSDT | 1 | 1.00029 | 2.29297 | 1.00029 | JTO | 2026-10-07 / 7 | ✓ 通过 |
| KAS-USDC | — | — | — | — | — | KAS | — / 0 | ✗ 无可用数据 |
| KITE-USDC | KITEUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| LUNA2-USDC | LUNAUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| MANA-USDC | MANAUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| MANTA-USDC | MANTAUSDT | 1 | 0.999846 | 2.89789 | 0.999846 | MANTA | 2026-10-07 / 7 | ✓ 通过 |
| MINA-USDC | MINAUSDT | 1 | 1.00437 | 2.09637 | 1.00437 | MINA | 2026-10-07 / 7 | ✓ 通过 |
| MORPHO-USDC | MORPHOUSDT | 1 | 0.998547 | 0.74289 | 0.998547 | MORPHO | 2026-10-07 / 7 | ✓ 通过 |
| MTL-USDC | MTLUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| NATGAS-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| NEO-USDC | NEOUSDT | 1 | 1.00119 | 1.8378 | 1.00119 | NEO | 2026-10-07 / 7 | ✓ 通过 |
| NIGHT-USDC | NIGHTUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| NVDA-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| ONT-USDC | ONTUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| PLTR-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| QQQ-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| SNDK-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| TSM-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| ZEC-USDC | ZECUSDT | 1 | 0.99991 | 0.451111 | 0.99991 | ZEC | 2026-10-07 / 7 | ✓ 通过 |
| XMR-USDC | XMRUSDT | 1 | — | — | — | XMR | — / 0 | ✗ 无可用数据 |
| DASH-USDC | DASHUSDT | 1 | 0.999425 | 8.18742 | 0.999425 | DASH | 2026-10-07 / 7 | ✓ 通过 |
| TIA-USDC | TIAUSDT | 1 | 0.999343 | 4.32581 | 0.999343 | TIA | 2026-10-07 / 7 | ✓ 通过 |
| FARTCOIN-USDC | — | — | — | — | — | FARTCOIN | — / 0 | ✗ 无可用数据 |
| WIF-USDC | WIFUSDT | 1 | 0.999396 | 2.61266 | 0.999396 | WIF | 2026-10-07 / 7 | ✓ 通过 |
| RENDER-USDC | RENDERUSDT | 1 | 0.998275 | 3.87553 | 0.998275 | RENDER | 2026-10-07 / 7 | ✓ 通过 |
| MSFT-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| AMD-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| LITE-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| SOXL-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| MRVL-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| AVGO-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| CRWV-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| QCOM-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| ARM-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| URNM-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| PEPE-USDC | PEPEUSDT | 1 | 1.00244 | 2.30018 | 0.00100244 | kPEPE | 2026-10-07 / 7 | ✓ 通过 |
| SHIB-USDC | SHIBUSDT | 1 | 0.998713 | 7.84994 | 0.000998713 | kSHIB | 2026-10-07 / 7 | ✓ 通过 |
| SPCX-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| LRCX-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| LIT-USDC | LITUSDT | 1 | — | — | — | LIT | — / 0 | ✗ 无可用数据 |
| GRAM-USDC | GRAMUSDT | 1 | 1.00118 | 0.815451 | 1.00118 | GRAM | 2026-10-07 / 7 | ✓ 通过 |
| SKHYNIX-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| SAMSUNG-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| UVXY-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| STXX-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| MON-USDC | — | — | — | — | — | MON | — / 0 | ✗ 无可用数据 |
| LAB-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| VVV-USDC | — | — | — | — | — | VVV | — / 0 | ✗ 无可用数据 |
| TURBO-USDC | TURBOUSDT | 1 | 1 | 6.91822 | 1 | TURBO | 2026-10-07 / 7 | ✓ 通过 |
| KAITO-USDC | KAITOUSDT | 1 | 1.00025 | 4.9901 | 1.00025 | KAITO | 2026-10-07 / 7 | ✓ 通过 |
| CASHCAT-USDC | — | — | — | — | — | CASHCAT | — / 0 | ✗ 无可用数据 |
| DRAM-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| NBIS-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| ZAMA-USDC | ZAMAUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| XPL-USDC | XPLUSDT | 1 | 1.00079 | 0.486097 | 1.00079 | XPL | 2026-10-07 / 7 | ✓ 通过 |
| PENDLE-USDC | PENDLEUSDT | 1 | 1.00009 | 1.19794 | 1.00009 | PENDLE | 2026-10-07 / 7 | ✓ 通过 |
| EIGEN-USDC | EIGENUSDT | 1 | 1.00042 | 4.92819 | 1.00042 | EIGEN | 2026-10-07 / 7 | ✓ 通过 |
| PLUME-USDC | PLUMEUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| SOXS-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| MRNA-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| PONS-USDC | — | — | — | — | — | PONS | — / 0 | ✗ 无可用数据 |
| IONQ-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| USELESS-USDC | — | — | — | — | — | USELESS | — / 0 | ✗ 无可用数据 |
| EWY-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| TWLO-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| CGNX-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| RAY-USDC | RAYUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| ZHIPU-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| MINIMAX-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| RKLB-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| CBRS-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| MET-USDC | METUSDT | 1 | 1.00417 | 1.94456 | 1.00417 | MET | 2026-10-07 / 7 | ✓ 通过 |
| XOM-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| OPENAI-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| ANTHROPIC-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| LGELECTRONICS-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| NAVER-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| HANMI-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| TQQQ-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| SQQQ-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| RDDT-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| CRWD-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| SMCI-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| APLD-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| TLT-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| STONK-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| USDJPY-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| HUT-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| TEAM-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| OKLO-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| NIL-USDC | NILUSDT | 1 | 0.99895 | 1.80931 | 0.99895 | NIL | 2026-10-07 / 7 | ✓ 通过 |
| GRASS-USDC | — | — | — | — | — | GRASS | — / 0 | ✗ 无可用数据 |
| PATH-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| CYPH-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| MOONSHOT-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| OURA-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| IREN-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| CRDO-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| APP-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| ADBE-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| COST-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| BX-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| DKNG-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| BRKB-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| KO-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| IBM-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| G-USDC | GUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| QNTX-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| UNH-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| SNOW-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| HIMS-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| KLAC-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| GS-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| TMF-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| ZRO-USDC | ZROUSDT | 1 | 1.00068 | 0.543563 | 1.00068 | ZRO | 2026-10-07 / 7 | ✓ 通过 |
| XDP-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| TWST-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| CVNA-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| CT-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| MCD-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| KIOXIA-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |
| QNT-USDC | QNTUSDT | 1 | — | — | — | — | — / 0 | ✗ 未校验 |
| NKE-USDC | — | — | — | — | — | — | — / 0 | ✗ 无可用数据 |

## 无数据、不可验证、拒绝项及原因

以下每项均不可直接交给策略；缺现货的项包括可能有永续的币，不能全局断言未上市。

| TxFlow名 | 原因 |
|---|---|
| CRO-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| MNT-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| XAUT-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| XAU-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| XAG-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| CL-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| MSTR-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| CRCL-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| XPD-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| TSLA-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| COIN-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| XPT-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| COPPER-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| EWJ-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| BZ-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| 1INCH-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| ANKR-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| AXL-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| CELR-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| COTI-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| HOOD-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| JASMY-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| META-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| MU-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| SPY-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| AAPL-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| ALICE-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| AMZN-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| API3-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| ARKM-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| BAND-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| BAT-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| CC-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| CETUS-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| CHZ-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| CYBER-USDC | Hyperliquid 未返回同日完整日线 |
| FLOW-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| GOOGL-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| GRT-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| HOT-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| ID-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| INTC-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| IOTX-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| KAS-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| KITE-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| LUNA2-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| MANA-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| MTL-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| NATGAS-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| NIGHT-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| NVDA-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| ONT-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| PLTR-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| QQQ-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| SNDK-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| TSM-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| XMR-USDC | 币安现货 symbol 存在但非 TRADING: BREAK |
| FARTCOIN-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| MSFT-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| AMD-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| LITE-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| SOXL-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| MRVL-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| AVGO-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| CRWV-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| QCOM-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| ARM-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| URNM-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| SPCX-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| LRCX-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| LIT-USDC | 币安现货 symbol 存在但非 TRADING: BREAK |
| SKHYNIX-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| SAMSUNG-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| UVXY-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| STXX-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| MON-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| LAB-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| VVV-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| CASHCAT-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| DRAM-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| NBIS-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| ZAMA-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| PLUME-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| SOXS-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| MRNA-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| PONS-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| IONQ-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| USELESS-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| EWY-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| TWLO-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| CGNX-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| RAY-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| ZHIPU-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| MINIMAX-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| RKLB-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| CBRS-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| XOM-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| OPENAI-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| ANTHROPIC-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| LGELECTRONICS-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| NAVER-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| HANMI-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| TQQQ-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| SQQQ-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| RDDT-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| CRWD-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| SMCI-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| APLD-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| TLT-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| STONK-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| USDJPY-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| HUT-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| TEAM-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| OKLO-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| GRASS-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| PATH-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| CYPH-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| MOONSHOT-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| OURA-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| IREN-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| CRDO-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| APP-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| ADBE-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| COST-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| BX-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| DKNG-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| BRKB-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| KO-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| IBM-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| G-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产 |
| QNTX-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| UNH-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| SNOW-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| HIMS-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| KLAC-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| GS-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| TMF-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| XDP-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| TWST-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| CVNA-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| CT-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| MCD-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| KIOXIA-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |
| QNT-USDC | 币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产；HIP-3 xyz:QNT 是 Quantinuum 股票，非 Quant 代币（XYZ 官方规格），禁止同名替代 |
| NKE-USDC | 币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有） |

## 倍数与原始成交量核对

| TxFlow名 | Binance | HL | 原始价格比 BN/HL | 原始成交量比 BN/HL | 归一价格比 | 归一成交量比 |
|---|---|---|---:|---:|---:|---:|
| 1000BONK-USDC | BONKUSDT | kBONK | 0.00100057 | 1650.18 | 1.00057 | 1.65018 |
| PEPE-USDC | PEPEUSDT | kPEPE | 0.00100244 | 2300.18 | 1.00244 | 2.30018 |
| SHIB-USDC | SHIBUSDT | kSHIB | 0.000998713 | 7849.94 | 0.998713 | 7.84994 |

上述原始成交量确实差数千倍；还原 HL 千倍报价单位后，最新同日量比分别落在正常量级。币安→TxFlow 的非 1 倍数只有 1000BONK；跨币安→HL 的千倍校验共有 PEPE、SHIB、BONK 三项。

## 可疑项与成交量量级

| TxFlow名 | 归一价格比范围 | 归一成交量比范围 | 说明 |
|---|---|---|---|
| 无 | — | — | 无超阈值项 |

所有同日验证中最大价格误差：MINA-USDC / 2026-10-06，比值 1.00512（误差 0.5118%）。归一成交量比范围 0.0538773（HYPE-USDC）～29.0762（TRX-USDC），未发现 ≥100 倍异常。

**同名陷阱：QNT-USDC 的 TxFlow fullName 是 Quant；HIP-3 中 xyz:QNT 则是 Quantinuum 股权市场，不能拿来验证 QNTUSDT。** 已核对全部 10 个 HIP-3 dex 的元数据；其余默认市场缺失币没有明确同资产补充匹配。该项保持未验证，不把近似价格当作资产身份。[XYZ 官方规格](https://docs.trade.xyz/perpetuals/specifications-and-schedules/pre-ipo-specification-index.md) 明确其 QNT 标的是 Quantinuum Inc. 股票。

同日量的原始比与归一比均保留，若千倍差在归一后消失，属于已解释的单位差；归一后仍异常则不得用单位修正硬凑通过。

## 数据文件、结构与自动审计

全部位于 `/Users/wonder/Code/stars/research/bn_panel_20261008/`：

- `data/<BinanceSymbol>.csv`：`ts,open,high,low,close,volume`，ts 是 UTC 日线开盘毫秒；OHLC 是 USDT/币安原生报价单位，volume 是原生基础币数量。
- `data/<BinanceSymbol>.json`：同批日线的 `t,o,h,l,c,v` 适配格式，供原框架 pandas 读取；绝不暗中改单位。
- `txflow_markets.json`：227 个完整市场元数据；`mapping.csv` / `mapping.json`：全市场，包含失败原因、单位、数据路径、校验日及资格；`approved_mapping.json`：仅严格通过。
- `raw/`：原始元数据、币安 K 线、HL 日线、逐日比值及错误响应；`request_log.json`：下载阶段请求证据；`manifest_sha256.json`：原始与数据文件哈希。
- `build_panel.py`：串行克制下载、最多一次短重试，HTTP 418/429/451 停止后续下载；已有缓存优先。`finalize.py`：完全离线重算比值、检查覆盖与格式、生成报告与白名单。

离线审计通过：227 行完整性、113 组 CSV/JSON/原始 K 线逐值一致、**574** 次同日对照重新计算、无重复日、无未收盘日、OHLC 合法、白名单阈值一致。日线缺口市场：无。`src/` 文件哈希与任务中快照一致；已有 `src/live.rs` 工作区修改未触碰。

同目录另一个并行采集进程曾重写 REPORT.md，现已结束。本报告认证的数据是 data/ 根目录的 <BinanceSymbol>.csv/json 与根目录 mapping.json / approved_mapping.json；data/ 子目录、data/mapping_* 等另一次采集产物不属于本报告白名单，不能混用。VERIFIED_REPORT.md 是本报告一致性备份。

## 可见局限与是否能直接喂策略

**全量映射表不能直接喂策略。** `approved_mapping.json` 可作下一步集成的候选白名单，但不是交易安全担保；必须按其中单位显式转换，保持独立组合，并在下单前重新检查 TxFlow 的最新交易状态和资产规格。没有改接任何策略。

无法担保以上无数据、未验证、拒绝、成交量待复核的市场。即使通过也只是跨交易所历史数据一致性检查；没有用 TxFlow 自身价格验证资产身份，没有验合约地址，也不能保证未来映射、流动性或价格偏离。PEPU 不存在于本次权威清单，不提供猜测映射。

价格对照使用 Hyperliquid 默认 perp universe；另核对全部 10 个 HIP-3 dex 的市场名称，未发现可安全补充的同资产匹配。股票和商品未用近似资产代替。部分币虽有现货日线但没有 HL 默认市场，因此保持未验证。币安永续被封，不能完成永续覆盖审计。现货成交量/价格与永续信号存在市场差异，USDT/USDC 也有基差。

最多 1000 日、部分新币更短；现有市场清单有存活偏差，最新 7 日价格接近不能证明整个历史无重命名/换币。现货与 HL 的跨场成交量不应拿来推导 TxFlow 承载容量。旧 `/tmp/hl-daily-full/` 是空目录，因此新建明确定义的数据格式与适配 JSON。

接口与单位字段参考：[币安现货 Market 文档](https://developers.binance.com/en/docs/catalog/core-trading-spot-trading/api/rest-api/market)、[Hyperliquid Info 文档](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint)；具体清单和数值以保存的 API 原始响应为证。
