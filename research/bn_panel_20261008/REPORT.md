# TxFlow 币安现货数据与符号校验

生成 UTC：2026-10-08T18:00:55.628881+00:00。状态：完整批次已结束

目标已收盘日：**2026-10-07 UTC**。运行冻结截止：2026-10-08 00:00 UTC；不会混入正在形成的日线。

## 1. 覆盖统计

TxFlow `/info` perpMeta 返回 **227** 个市场（全部未标 delisted/haltTrading）→ 映射校验通过 **82** ✓，其中 TxFlow/币安单位不同 **1** ✓；HL 参照倍数市场 **3**。未通过或缺数据 **145** ✗。严格候选 **79**；已处理 227/227。

选择 **币安 USDT 现货**；不是永续。BTC 现货代理探针 HTTP 200、1000 根真实日线。永续探针 HTTP 418 / code -1003：代理出口 IP 临时被封禁（不是已证实的地区限制），见 `data/raw/btc_futures_probe.json`。没有用测试网、模拟数据或 TxFlow 价量补洞。

| 状态 | 数量 |
|---|---:|
| ✗ 币安现货已停用 | 1 |
| ✗ 身份不符 | 1 |
| ✗ 无币安现货数据 | 113 |
| ✗ 没有共同日 | 2 |
| ✗ HL 同 ticker 身份未证实 | 2 |
| ✗ 有币安数据但无 HL 对照 | 26 |
| ✓ 价格/量级校验通过 | 82 |

## 2. 判定与单位定义

币安候选由本次完整 ticker 清单筛选，再用 exchangeInfo 核对 baseAsset/交易状态。同名仅生成候选；不做模糊字符串猜测。TxFlow `PEPU-USDC` 不在实际返回清单中，实际为 `PEPE-USDC`，不能把 PEPU 自动当 PEPE。

每个有币安候选且存在 HL 对照的币独立请求最多 14 日 HL candleSnapshot，与币安按 UTC 开盘毫秒对齐。必须包含目标日，至少 3 个同日点；最近最多 7 日归一价格全部在 1±5% 才通过（超过 1% 另标提示）。极端原始比 >10000 / <0.0001 直接剔除。没有 HL 同日数据的币绝不标成功。

表中 **倍数 = 币安 OHLC → TxFlow 单位的价格乘数**；成交量用其倒数。**价格比 = HL 原始 close / 币安原始 close**；**成交量比 = 币安原始基础币 volume / HL 原始 volume**。HL kPEPE/kSHIB/kBONK 为 1000 币单位，所以原始价格比约 1000；这不代表币安现货是倍数合约。归一价格比 = Binance close / (HL close / HL单位)；归一量比 = Binance volume / (HL volume × HL单位)。

量级异常：目标日或最近最多 7 日中位数的归一量比不在 [0.01,100]（或 HL 零成交量），列入复核，不进严格候选。不同交易所/现货与永续的成交活跃度本来不同，量比异常只能提示单位或流动性问题，不能仅凭此认定映射错。

`1000BONK-USDC → BONKUSDT`：TxFlow metadata description 明确 1000 BONK，OHLC ×1000、volume ÷1000。PEPE/SHIB 现货无需改单位；HL 对照需除以 1000。

## 3. 完整映射表（每个市场一行）

| TxFlow名 | 币安symbol | 倍数 | 价格比 HL/BN | 成交量比 BN/HL | 判定 | HL参照 | 归一价格比 | 归一量比 | 同日 UTC | 说明 |
|---|---|---:|---:|---:|---|---|---:|---:|---|---|
| BTC-USDC | BTCUSDT | 1 | 0.999762 | 0.61773 | ✓ 价格/量级校验通过 | BTC | 1.00024 | 0.61773 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| ETH-USDC | ETHUSDT | 1 | 0.999713 | 0.795745 | ✓ 价格/量级校验通过 | ETH | 1.00029 | 0.795745 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| BNB-USDC | BNBUSDT | 1 | 0.999625 | 9.33751 | ✓ 价格/量级校验通过 | BNB | 1.00038 | 9.33751 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| BCH-USDC | BCHUSDT | 1 | 0.9996 | 3.6257 | ✓ 价格/量级校验通过 | BCH | 1.0004 | 3.6257 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| LTC-USDC | LTCUSDT | 1 | 0.999758 | 1.98273 | ✓ 价格/量级校验通过 | LTC | 1.00024 | 1.98273 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| ETC-USDC | ETCUSDT | 1 | 1.00114 | 5.86356 | ✓ 价格/量级校验通过 | ETC | 0.998857 | 5.86356 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| LINK-USDC | LINKUSDT | 1 | 0.9997 | 1.82072 | ✓ 价格/量级校验通过 | LINK | 1.0003 | 1.82072 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| ADA-USDC | ADAUSDT | 1 | 0.999335 | 4.76267 | ✓ 价格/量级校验通过 | ADA | 1.00067 | 4.76267 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| ATOM-USDC | ATOMUSDT | 1 | 0.998551 | 3.34687 | ✓ 价格/量级校验通过 | ATOM | 1.00145 | 3.34687 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| DOGE-USDC | DOGEUSDT | 1 | 0.99982 | 7.74542 | ✓ 价格/量级校验通过 | DOGE | 1.00018 | 7.74542 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| DOT-USDC | DOTUSDT | 1 | 1.00036 | 4.05433 | ✓ 价格/量级校验通过 | DOT | 0.999642 | 4.05433 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| CRV-USDC | CRVUSDT | 1 | 0.999554 | 0.565284 | ✓ 价格/量级校验通过 | CRV | 1.00045 | 0.565284 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| SOL-USDC | SOLUSDT | 1 | 0.999398 | 0.881858 | ✓ 价格/量级校验通过 | SOL | 1.0006 | 0.881858 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| UNI-USDC | UNIUSDT | 1 | 1.00061 | 2.05331 | ✓ 价格/量级校验通过 | UNI | 0.999394 | 2.05331 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| AVAX-USDC | AVAXUSDT | 1 | 0.999551 | 2.92821 | ✓ 价格/量级校验通过 | AVAX | 1.00045 | 2.92821 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| FIL-USDC | FILUSDT | 1 | 0.99943 | 4.82063 | ✓ 价格/量级校验通过 | FIL | 1.00057 | 4.82063 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| AAVE-USDC | AAVEUSDT | 1 | 0.999828 | 1.85681 | ✓ 价格/量级校验通过 | AAVE | 1.00017 | 1.85681 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| XRP-USDC | XRPUSDT | 1 | 0.999508 | 1.82695 | ✓ 价格/量级校验通过 | XRP | 1.00049 | 1.82695 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| APT-USDC | APTUSDT | 1 | 0.999476 | 2.99907 | ✓ 价格/量级校验通过 | APT | 1.00052 | 2.99907 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| TRX-USDC | TRXUSDT | 1 | 0.999493 | 10.6247 | ✓ 价格/量级校验通过 | TRX | 1.00051 | 10.6247 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| ARB-USDC | ARBUSDT | 1 | 0.998547 | 1.07144 | ✓ 价格/量级校验通过 | ARB | 1.00146 | 1.07144 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| OP-USDC | OPUSDT | 1 | 0.999594 | 4.95922 | ✓ 价格/量级校验通过 | OP | 1.00041 | 4.95922 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| LDO-USDC | LDOUSDT | 1 | 1 | 1.80955 | ✓ 价格/量级校验通过 | LDO | 1 | 1.80955 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| NEAR-USDC | NEARUSDT | 1 | 1.00116 | 0.718441 | ✓ 价格/量级校验通过 | NEAR | 0.998839 | 0.718441 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| SUI-USDC | SUIUSDT | 1 | 0.99938 | 3.35639 | ✓ 价格/量级校验通过 | SUI | 1.00062 | 3.35639 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| WLD-USDC | WLDUSDT | 1 | 0.999136 | 1.79032 | ✓ 价格/量级校验通过 | WLD | 1.00086 | 1.79032 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| XLM-USDC | XLMUSDT | 1 | 0.999302 | 5.35722 | ✓ 价格/量级校验通过 | XLM | 1.0007 | 5.35722 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| ICP-USDC | ICPUSDT | 1 | 1.00012 | 3.92777 | ✓ 价格/量级校验通过 | ICP | 0.999877 | 3.92777 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| SEI-USDC | SEIUSDT | 1 | 0.999144 | 3.61722 | ✓ 价格/量级校验通过 | SEI | 1.00086 | 3.61722 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| HBAR-USDC | HBARUSDT | 1 | 1.00086 | 6.71331 | ✓ 价格/量级校验通过 | HBAR | 0.999139 | 6.71331 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| CAKE-USDC | CAKEUSDT | 1 | 0.999911 | 12.426 | ✓ 价格/量级校验通过 | CAKE | 1.00009 | 12.426 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| CRO-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| MNT-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| 1000BONK-USDC | BONKUSDT | 1000 | 999.432 | 1650.18 | ✓ 价格/量级校验通过 | kBONK | 1.00057 | 1.65018 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| ONDO-USDC | ONDOUSDT | 1 | 1.00068 | 1.21207 | ✓ 价格/量级校验通过 | ONDO | 0.99932 | 1.21207 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| TAO-USDC | TAOUSDT | 1 | 1.00021 | 1.69822 | ✓ 价格/量级校验通过 | TAO | 0.999794 | 1.69822 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| JUP-USDC | JUPUSDT | 1 | 0.999749 | 1.08806 | ✓ 价格/量级校验通过 | JUP | 1.00025 | 1.08806 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| STRK-USDC | STRKUSDT | 1 | 0.999796 | 2.77269 | ✓ 价格/量级校验通过 | STRK | 1.0002 | 2.77269 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| ENA-USDC | ENAUSDT | 1 | 1.00027 | 0.64991 | ✓ 价格/量级校验通过 | ENA | 0.999735 | 0.64991 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| POL-USDC | POLUSDT | 1 | 0.999319 | 6.17027 | ✓ 价格/量级校验通过 | POL | 1.00068 | 6.17027 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| VIRTUAL-USDC | VIRTUALUSDT | 1 | 0.999793 | 2.83851 | ✓ 价格/量级校验通过 | VIRTUAL | 1.00021 | 2.83851 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| PENGU-USDC | PENGUUSDT | 1 | 0.999655 | 2.23654 | ✓ 价格/量级校验通过 | PENGU | 1.00035 | 2.23654 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| HYPE-USDC | HYPEUSDT | 1 | 0.999142 | 0.0538773 | ✓ 价格/量级校验通过 | HYPE | 1.00086 | 0.0538773 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内；history_under_100_days |
| TRUMP-USDC | TRUMPUSDT | 1 | 0.999096 | 4.63857 | ✓ 价格/量级校验通过 | TRUMP | 1.00091 | 4.63857 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| XAUT-USDC | XAUTUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| PUMP-USDC | PUMPUSDT | 1 | 0.999679 | 0.239821 | ✓ 价格/量级校验通过 | PUMP | 1.00032 | 0.239821 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| SKY-USDC | SKYUSDT | 1 | 0.998891 | 1.08412 | ✓ 价格/量级校验通过 | SKY | 1.00111 | 1.08412 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| WLFI-USDC | WLFIUSDT | 1 | 0.998478 | 6.35336 | ✓ 价格/量级校验通过 | WLFI | 1.00152 | 6.35336 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| ASTER-USDC | ASTERUSDT | 1 | 0.999169 | 3.10321 | ✓ 价格/量级校验通过 | ASTER | 1.00083 | 3.10321 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| XAU-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| XAG-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| CL-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| MSTR-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| CRCL-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| XPD-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| TSLA-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| COIN-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| XPT-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| COPPER-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| EWJ-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| AERO-USDC | AEROUSDT | 1 | 1.0009 | 1.10157 | ✓ 价格/量级校验通过 | AERO | 0.999096 | 1.10157 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内；history_under_100_days |
| BZ-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| 1INCH-USDC | 1INCHUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| ANKR-USDC | ANKRUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| AXL-USDC | AXLUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| BLUR-USDC | BLURUSDT | 1 | 0.999793 | 1.03571 | ✓ 价格/量级校验通过 | BLUR | 1.00021 | 1.03571 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| CELR-USDC | CELRUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| COTI-USDC | COTIUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| ETHFI-USDC | ETHFIUSDT | 1 | 1.00051 | 1.76156 | ✓ 价格/量级校验通过 | ETHFI | 0.999488 | 1.76156 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| GMX-USDC | GMXUSDT | 1 | 0.999676 | 3.06766 | ✓ 价格/量级校验通过 | GMX | 1.00032 | 3.06766 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| HOOD-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| INJ-USDC | INJUSDT | 1 | 0.999196 | 3.20306 | ✓ 价格/量级校验通过 | INJ | 1.0008 | 3.20306 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| JASMY-USDC | JASMYUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| LINEA-USDC | LINEAUSDT | 1 | 1.00073 | 2.14331 | ✓ 价格/量级校验通过 | LINEA | 0.999268 | 2.14331 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| META-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| MU-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| SPY-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| AAPL-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| ALICE-USDC | ALICEUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| AMZN-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| API3-USDC | API3USDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| AR-USDC | ARUSDT | 1 | 0.999741 | 3.33742 | ✓ 价格/量级校验通过 | AR | 1.00026 | 3.33742 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| ARKM-USDC | ARKMUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| AXS-USDC | AXSUSDT | 1 | 0.999039 | 13.8343 | ✓ 价格/量级校验通过 | AXS | 1.00096 | 13.8343 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| BAND-USDC | BANDUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| BAT-USDC | BATUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| CC-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| CELO-USDC | CELOUSDT | 1 | 0.9998 | 5.25816 | ✓ 价格/量级校验通过 | CELO | 1.0002 | 5.25816 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| CETUS-USDC | CETUSUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| CHZ-USDC | CHZUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| COMP-USDC | COMPUSDT | 1 | 0.999485 | 6.43647 | ✓ 价格/量级校验通过 | COMP | 1.00052 | 6.43647 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| CYBER-USDC | CYBERUSDT | 1 | — | — | ✗ 没有共同日 | CYBER | — | — | — | 没有相同 UTC 日的已收盘 K 线 |
| DYDX-USDC | DYDXUSDT | 1 | 1.00022 | 4.80112 | ✓ 价格/量级校验通过 | DYDX | 0.999779 | 4.80112 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| FET-USDC | FETUSDT | 1 | 1 | 6.03144 | ✓ 价格/量级校验通过 | FET | 1 | 6.03144 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| FLOW-USDC | FLOWUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| GAS-USDC | GASUSDT | 1 | 0.998887 | 4.65156 | ✓ 价格/量级校验通过 | GAS | 1.00111 | 4.65156 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| GOOGL-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| GRT-USDC | GRTUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| HOT-USDC | HOTUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| ID-USDC | IDUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| IMX-USDC | IMXUSDT | 1 | 0.999829 | 1.42427 | ✓ 价格/量级校验通过 | IMX | 1.00017 | 1.42427 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| INTC-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| IOTA-USDC | IOTAUSDT | 1 | 1.00354 | 5.10417 | ✓ 价格/量级校验通过 | IOTA | 0.996474 | 5.10417 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| IOTX-USDC | IOTXUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| JTO-USDC | JTOUSDT | 1 | 0.999708 | 2.29297 | ✓ 价格/量级校验通过 | JTO | 1.00029 | 2.29297 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| KAS-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| KITE-USDC | KITEUSDT | 1 | 0.996677 | 560.24 | ✗ HL 同 ticker 身份未证实 | @310 | 1.00333 | 560.24 | 2026-10-07 | HL 现货仅同 ticker，未确认 token 身份；数值比通过也不纳入映射白名单 |
| LUNA2-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| MANA-USDC | MANAUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| MANTA-USDC | MANTAUSDT | 1 | 1.00015 | 2.89789 | ✓ 价格/量级校验通过 | MANTA | 0.999846 | 2.89789 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| MINA-USDC | MINAUSDT | 1 | 0.995645 | 2.09637 | ✓ 价格/量级校验通过 | MINA | 1.00437 | 2.09637 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| MORPHO-USDC | MORPHOUSDT | 1 | 1.00146 | 0.74289 | ✓ 价格/量级校验通过 | MORPHO | 0.998547 | 0.74289 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| MTL-USDC | MTLUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| NATGAS-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| NEO-USDC | NEOUSDT | 1 | 0.998807 | 1.8378 | ✓ 价格/量级校验通过 | NEO | 1.00119 | 1.8378 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| NIGHT-USDC | NIGHTUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| NVDA-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| ONT-USDC | ONTUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| PLTR-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| QQQ-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| SNDK-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| TSM-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| ZEC-USDC | ZECUSDT | 1 | 1.00009 | 0.451111 | ✓ 价格/量级校验通过 | ZEC | 0.99991 | 0.451111 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| XMR-USDC | XMRUSDT | 1 | — | — | ✗ 币安现货已停用 | XMR | — | — | — | 币安现货状态 BREAK；日线截至 2024-02-20，无最新同日校验 |
| DASH-USDC | DASHUSDT | 1 | 1.00058 | 8.18742 | ✓ 价格/量级校验通过 | DASH | 0.999425 | 8.18742 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| TIA-USDC | TIAUSDT | 1 | 1.00066 | 4.32581 | ✓ 价格/量级校验通过 | TIA | 0.999343 | 4.32581 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| FARTCOIN-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| WIF-USDC | WIFUSDT | 1 | 1.0006 | 2.61266 | ✓ 价格/量级校验通过 | WIF | 0.999396 | 2.61266 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| RENDER-USDC | RENDERUSDT | 1 | 1.00173 | 3.87553 | ✓ 价格/量级校验通过 | RENDER | 0.998275 | 3.87553 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| MSFT-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| AMD-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| LITE-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| SOXL-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| MRVL-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| AVGO-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| CRWV-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| QCOM-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| ARM-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| URNM-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| PEPE-USDC | PEPEUSDT | 1 | 997.561 | 2300.18 | ✓ 价格/量级校验通过 | kPEPE | 1.00244 | 2.30018 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| SHIB-USDC | SHIBUSDT | 1 | 1001.29 | 7849.94 | ✓ 价格/量级校验通过 | kSHIB | 0.998713 | 7.84994 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| SPCX-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| LRCX-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| LIT-USDC | LITUSDT | 1 | — | — | ✗ 身份不符 | LIT | — | — | — | TxFlow perpMeta fullName=Lighter Protocol；币安旧 LITUSDT 属于 Litentry（已换币为 HEI），本次 exchangeInfo 为 BREAK。是 ticker 重用/不同项目，明确剔除，不能用旧 LIT 数据给 Lighter 发信号。 |
| GRAM-USDC | GRAMUSDT | 1 | 0.998822 | 0.815451 | ✓ 价格/量级校验通过 | GRAM | 1.00118 | 0.815451 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内；history_under_100_days |
| SKHYNIX-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| SAMSUNG-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| UVXY-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| STXX-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| MON-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| LAB-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| VVV-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| TURBO-USDC | TURBOUSDT | 1 | 1 | 6.91822 | ✓ 价格/量级校验通过 | TURBO | 1 | 6.91822 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| KAITO-USDC | KAITOUSDT | 1 | 0.999752 | 4.9901 | ✓ 价格/量级校验通过 | KAITO | 1.00025 | 4.9901 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| CASHCAT-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| DRAM-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| NBIS-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| ZAMA-USDC | ZAMAUSDT | 1 | 1.00026 | 69.8358 | ✗ HL 同 ticker 身份未证实 | @292 | 0.99974 | 69.8358 | 2026-10-07 | HL 现货仅同 ticker，未确认 token 身份；数值比通过也不纳入映射白名单 |
| XPL-USDC | XPLUSDT | 1 | 0.999213 | 0.486097 | ✓ 价格/量级校验通过 | XPL | 1.00079 | 0.486097 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| PENDLE-USDC | PENDLEUSDT | 1 | 0.999911 | 1.19794 | ✓ 价格/量级校验通过 | PENDLE | 1.00009 | 1.19794 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| EIGEN-USDC | EIGENUSDT | 1 | 0.99958 | 4.92819 | ✓ 价格/量级校验通过 | EIGEN | 1.00042 | 4.92819 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| PLUME-USDC | PLUMEUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| SOXS-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| MRNA-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| PONS-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| IONQ-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| USELESS-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| EWY-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| TWLO-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| CGNX-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| RAY-USDC | RAYUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| ZHIPU-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| MINIMAX-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| RKLB-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| CBRS-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| MET-USDC | METUSDT | 1 | 0.995846 | 1.94456 | ✓ 价格/量级校验通过 | MET | 1.00417 | 1.94456 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| XOM-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| OPENAI-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| ANTHROPIC-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| LGELECTRONICS-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| NAVER-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| HANMI-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| TQQQ-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| SQQQ-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| RDDT-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| CRWD-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| SMCI-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| APLD-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| TLT-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| STONK-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| USDJPY-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| HUT-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| TEAM-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| OKLO-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| NIL-USDC | NILUSDT | 1 | 1.00105 | 1.80931 | ✓ 价格/量级校验通过 | NIL | 0.99895 | 1.80931 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| GRASS-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| PATH-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| CYPH-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| MOONSHOT-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| OURA-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| IREN-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| CRDO-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| APP-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| ADBE-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| COST-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| BX-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| DKNG-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| BRKB-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| KO-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| IBM-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| G-USDC | GUSDT | 1 | — | — | ✗ 没有共同日 | @76 | — | — | — | 没有相同 UTC 日的已收盘 K 线 |
| QNTX-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| UNH-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| SNOW-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| HIMS-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| KLAC-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| GS-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| TMF-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| ZRO-USDC | ZROUSDT | 1 | 0.999317 | 0.543563 | ✓ 价格/量级校验通过 | ZRO | 1.00068 | 0.543563 | 2026-10-07 | 交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内 |
| XDP-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| TWST-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| CVNA-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| CT-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| MCD-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| KIOXIA-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |
| QNT-USDC | QNTUSDT | 1 | — | — | ✗ 有币安数据但无 HL 对照 | — | — | — | — | Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT） |
| NKE-USDC | — | 1 | — | — | ✗ 无币安现货数据 | — | — | — | — | 币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据 |

## 4. 无数据/未校验币清单与原因

以下均未进入验证映射白名单；“无现货”只表示本次所选 USDT 现货源无数据，不代表币安永续或其他报价币绝对没有。
- **CRO-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **MNT-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **XAUT-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **XAU-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **XAG-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **CL-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **MSTR-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **CRCL-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **XPD-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **TSLA-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **COIN-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **XPT-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **COPPER-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **EWJ-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **BZ-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **1INCH-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **ANKR-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **AXL-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **CELR-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **COTI-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **HOOD-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **JASMY-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **META-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **MU-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **SPY-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **AAPL-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **ALICE-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **AMZN-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **API3-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **ARKM-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **BAND-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **BAT-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **CC-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **CETUS-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **CHZ-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **CYBER-USDC**：没有相同 UTC 日的已收盘 K 线
- **FLOW-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **GOOGL-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **GRT-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **HOT-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **ID-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **INTC-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **IOTX-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **KAS-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **KITE-USDC**：HL 现货仅同 ticker，未确认 token 身份；数值比通过也不纳入映射白名单
- **LUNA2-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **MANA-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **MTL-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **NATGAS-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **NIGHT-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **NVDA-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **ONT-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **PLTR-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **QQQ-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **SNDK-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **TSM-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **XMR-USDC**：币安现货状态 BREAK；日线截至 2024-02-20，无最新同日校验
- **FARTCOIN-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **MSFT-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **AMD-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **LITE-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **SOXL-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **MRVL-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **AVGO-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **CRWV-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **QCOM-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **ARM-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **URNM-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **SPCX-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **LRCX-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **LIT-USDC**：TxFlow perpMeta fullName=Lighter Protocol；币安旧 LITUSDT 属于 Litentry（已换币为 HEI），本次 exchangeInfo 为 BREAK。是 ticker 重用/不同项目，明确剔除，不能用旧 LIT 数据给 Lighter 发信号。
- **SKHYNIX-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **SAMSUNG-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **UVXY-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **STXX-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **MON-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **LAB-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **VVV-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **CASHCAT-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **DRAM-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **NBIS-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **ZAMA-USDC**：HL 现货仅同 ticker，未确认 token 身份；数值比通过也不纳入映射白名单
- **PLUME-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **SOXS-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **MRNA-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **PONS-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **IONQ-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **USELESS-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **EWY-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **TWLO-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **CGNX-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **RAY-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **ZHIPU-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **MINIMAX-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **RKLB-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **CBRS-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **XOM-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **OPENAI-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **ANTHROPIC-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **LGELECTRONICS-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **NAVER-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **HANMI-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **TQQQ-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **SQQQ-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **RDDT-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **CRWD-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **SMCI-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **APLD-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **TLT-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **STONK-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **USDJPY-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **HUT-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **TEAM-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **OKLO-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **GRASS-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **PATH-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **CYPH-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **MOONSHOT-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **OURA-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **IREN-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **CRDO-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **APP-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **ADBE-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **COST-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **BX-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **DKNG-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **BRKB-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **KO-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **IBM-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **G-USDC**：没有相同 UTC 日的已收盘 K 线
- **QNTX-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **UNH-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **SNOW-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **HIMS-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **KLAC-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **GS-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **TMF-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **XDP-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **TWST-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **CVNA-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **CT-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **MCD-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **KIOXIA-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据
- **QNT-USDC**：Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）
- **NKE-USDC**：币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据

## 5. 可疑项与成交量异常

| TxFlow | 价格归一最大偏差（最近7日） | 归一量比（当日/7日中位） | 状态/提示 |
|---|---:|---|---|
| HYPE-USDC | 0.000890086 | 0.0538773 / 0.0759819 | ✓ 价格/量级校验通过；history_under_100_days |
| AERO-USDC | 0.00163778 | 1.10157 / 1.28931 | ✓ 价格/量级校验通过；history_under_100_days |
| CYBER-USDC | — | — / — | ✗ 没有共同日； |
| KITE-USDC | 0.00776738 | 560.24 / 1464.08 | ✗ HL 同 ticker 身份未证实； |
| XMR-USDC | — | — / — | ✗ 币安现货已停用； |
| GRAM-USDC | 0.00117949 | 0.815451 / 1.49356 | ✓ 价格/量级校验通过；history_under_100_days |
| ZAMA-USDC | 0.00511934 | 69.8358 / 443.693 | ✗ HL 同 ticker 身份未证实； |
| G-USDC | — | — / — | ✗ 没有共同日； |

特别剔除 **LIT-USDC**：TxFlow metadata 是 Lighter Protocol；币安旧 LIT 为 Litentry 并已换币为 HEI，本次状态 BREAK。不能把旧 LIT 历史给 Lighter 使用。[币安官方换币公告](https://www.binance.com/en/support/announcement/detail/2a9feaa556f74dcdaa2192366f0e247c)。

## 6. 文件位置与结构

根目录：`/Users/wonder/Code/stars/research/bn_panel_20261008`。

- `data/txflow_markets.csv/json`：全部 TxFlow metadata 市场与可交易标记，原始响应 `data/raw/txflow_perpMeta.json`。
- `data/binance_spot/<symbol>.csv`：币安原始单位，最新最多 1000 根已收盘日线。
- `data/daily/<TxFlow名>.csv`：换算到 TxFlow 单位的币安日线，所有有有效数据的候选（**包含未通过映射的候选，不能整目录无筛选读取**）。
- 日线统一表头 `ts,open,high,low,close,volume`，ts 为 UTC 开盘 Unix 毫秒，价格报价币为 USDT，volume 为基础币/合约单位数量。归一文件只换单位，来源仍是币安现货。
- `data/hyperliquid/`：独立校验参照日线；`data/validation/<TxFlow名>.json`：每个同日的价格、量比证据。
- `data/mapping_all.csv/json`：227 行全部状态；`mapping_verified.json` 包括量级待复核项；`mapping_strict.json` 仅无价格基差、量级、短历史、缺日提示的候选。
- `alternate_batch/`：目录中另一批采集产物完整保留，含不同倍数字段约定；不属于本次规范输出，不要混用。其通过币名单与本批一致。
- `data/raw/`：真实 API JSON；`requests.jsonl`：请求时刻、HTTP、重试次数及耗时；`run_config.json`：冻结日期及阈值；`coverage.json`：统计；`manifest.json`：SHA256 与文件清单。
- `/tmp/hl-daily-full/` 本次存在但为空，无法推断旧格式，因此采用并明示上述六列格式。
- 重跑 `python3 collect_validate.py` 使用已有成功响应；失败可重试，已完成币落盘 checkpoint 保留。不会刷新旧日期，若要新的快照应新建目录或另建采集批次。

离线校验：`python3 verify_artifacts.py` 对照原始 JSON 检查每个 CSV 单元、全部同日价量比、单位换算、日期截止、完整名单与哈希。最近结果见 `verification.json`；源码与目录内既有快照的核对见 `src_integrity.json`。

## 7. 看得见的局限与策略使用结论

**不能把完整映射表直接喂给策略。** 这一步没有修改策略代码，也没有完成策略适配、仓位/执行单位或独立组合验证。严格候选只是本次数据校验清单，仍需按字段读取和重新校验最新数据。

价格比能发现单位或严重错配，不能独立证明 token 身份；HL 同名现货只做数值对照，未证实身份者一律排除。HIP-3 同 ticker（如 xyz:QNT）不自动当作同一加密资产。未校验项、退市项、价格异常项均不敢担保；通过但带 volume_scale_anomaly / price_basis_over_1pct / history_under_100_days / daily_gaps 的币也需人工复核。

只取最新最多 1000 根；上线不足 1000 天的币不会补齐或伪造。新币/换币/重用 ticker 可能有结构变化，逐日完整历史的 token 身份与拆并币没有额外权威核验。同日收盘可能有现货/永续基差，USDT/USDC 报价差没有额外汇率换算。TxFlow 倍数只从名称与 perpMeta description 提取，没有调用其价量/盘口。

网络走指定本机代理，采集全局最多 2 并发；单次 40 秒，瞬态失败最多重试 2 次，418/429/451 不盲重试。没有调用 TxFlow explorer。

API 文档：[币安官方现货 Kline 定义](https://github.com/binance/binance-spot-api-docs/blob/master/rest-api.md#klinecandlestick-data)、[Hyperliquid Info](https://hyperliquid.gitbook.io/Hyperliquid-docs/for-developers/api/info-endpoint)、[全部永续 metadata](https://hyperliquid.gitbook.io/Hyperliquid-docs/for-developers/api/info-endpoint/perpetuals)。实际数据证据以本地原始响应为准。
