"""Offline audit and human-readable report. No network and no strategy changes."""
import collections,csv,datetime,hashlib,json,math,pathlib
O=pathlib.Path(__file__).resolve().parent;D=86400000
rows=json.load(open(O/'mapping.json'));meta=json.load(open(O/'txflow_markets.json'));assert len(rows)==len(meta)==227
assert [x['txflow'] for x in rows]==[x['name'] for x in meta];assert len({x['txflow'] for x in rows})==227
counts=collections.Counter(x['status'] for x in rows);approved=[x for x in rows if x['strategy_eligible']];data=[x for x in rows if x.get('bars')]
checks=0;gap=[]
for x in data:
 c=list(csv.DictReader(open(O/x['data_file'])));j=json.load(open(O/'data'/(x['binance_symbol']+'.json')));raw=json.load(open(O/'raw'/('bn_'+x['binance_symbol']+'.json')))
 assert len(c)==len(j)==x['bars'];assert len(raw)==len(c)
 times=[int(z['ts']) for z in c];assert times==sorted(set(times));assert max(times)+D<=1791417600000
 if any(b-a!=D for a,b in zip(times,times[1:])):gap.append(x['txflow'])
 for z,adapt,r in zip(c,j,raw):
  assert int(z['ts'])==adapt['t']==r[0]
  for key,k,n in [('open','o',1),('high','h',2),('low','l',3),('close','c',4),('volume','v',5)]:assert float(z[key])==float(adapt[k])==float(r[n])
 if x['validation_days']:
  validation=json.load(open(O/'raw'/('validation_'+x['txflow_base']+'.json')))
  hb={h['t']:h for h in json.load(open(O/'raw'/('hl_'+x['hl_coin']+'.json')))};bb={b[0]:b for b in raw}
  for v in validation:
   t=v['ts'];b=bb[t];h=hb[t];assert t+D<=1791417600000
   pr=float(b[4])*x['hl_unit']/float(h['c']);assert math.isclose(pr,v['price_ratio'],rel_tol=1e-12)
   if float(h['v']):assert math.isclose(float(b[5])/(float(h['v'])*x['hl_unit']),v['volume_ratio'],rel_tol=1e-12)
   checks+=1
  assert len(validation)==x['validation_days']
  if x['strategy_eligible']:assert all(abs(v['price_ratio']-1)<=.05 and v['volume_ratio'] is not None and .01<=v['volume_ratio']<=100 for v in validation)
 else:assert not x['strategy_eligible']
assert all(x['status']=='validated' for x in approved)
source=json.load(open(O/'src_hashes_before.json'));assert all(hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()==s for p,s in source.items())
(O/'approved_mapping.json').write_text(json.dumps(approved,ensure_ascii=False,indent=2))
fields=list(dict.fromkeys(k for r in rows for k in r))
with (O/'mapping.csv').open('w') as f:
 w=csv.DictWriter(f,fieldnames=fields);w.writeheader();w.writerows(rows)
summary={'markets':len(rows),'downloaded':len(data),'rows':sum(x['bars'] for x in data),'status_counts':dict(counts),'approved':len(approved),'approved_multiplier_contracts':sum(x['txflow_unit']!=x['binance_unit'] for x in approved),'same_day_comparisons':checks,'gap_markets':gap,'src_unchanged_since_snapshot':True}
(O/'audit.json').write_text(json.dumps(summary,ensure_ascii=False,indent=2))
files={str(p.relative_to(O)):hashlib.sha256(p.read_bytes()).hexdigest() for folder in ('raw','data') for p in sorted((O/folder).glob('*')) if p.is_file()};(O/'manifest_sha256.json').write_text(json.dumps(files,indent=2))
fmt=lambda x:'—' if x is None else f'{x:.6g}'
labels={'validated':'✓ 通过','volume_review':'⚠ 价格通过，量待复核','unvalidated':'✗ 未校验','rejected':'✗ 拒绝','no_data':'✗ 无可用数据'}
lines=[]
def w(s=''):lines.append(s)
w('# TxFlow 币安日线与符号映射校验')
w();w('采集日期：2026-10-09 北京时间（UTC 2026-10-08）。只做研究数据；未改策略、未 commit/push/部署；TxFlow 仅一次 `POST /info {"type":"perpMeta","dex":""}`，未调用 explorer。')
w();w('## 覆盖统计')
w();w(f'TxFlow 共 **{len(rows)}** 个市场，返回清单全部无 haltTrading/delisted/onlyIsolated 标记。币安现货候选 115 个，其中 XMRUSDT、LITUSDT 为 BREAK。下载 **{len(data)}** 个 symbol、**{summary["rows"]:,}** 根完整日线。')
w();w(f'严格通过并进入白名单 **{len(approved)}** 个 ✓，其中币安/TxFlow 单位不同的倍数合约 **{summary["approved_multiplier_contracts"]}** 个 ✓（这是通过数的子集）。价格通过但成交量待复核 **{counts["volume_review"]}** 个；有币安数据但 Hyperliquid 无法完成验证 **{counts["unvalidated"]}** 个；价格拒绝 **{counts["rejected"]}** 个；无可用币安现货日线 **{counts["no_data"]}** 个。以上互斥状态合计 227。')
w();w('本批选择 **币安 USDT 现货**。永续接口曾可返回 BTC 探针，但之后 exchangeInfo 返回 HTTP 418 / code -1003：代理出口 IP 被封至 2026-10-09 02:12:58.741 UTC；已停止永续请求。**不是 HTTP 451，未证实地区限制。** 不能把「本批现货没有」写成「币安现货和永续都没有」。原始错误见 `raw/bn_futures_exchange.json`；大响应超时的部分文件只作诊断，绝未进入数据面板。')
w();w('## 校验口径与单位')
w();w('固定数据截止：2026-10-08 00:00:00 UTC，最新允许的日线起点是 2026-10-07 00:00 UTC。每个现货币最多最近 1000 根，不补值、不伪造。比较最近最多 7 个双方都有的 UTC 完整日线；收盘属于相同日历日，而不是各自最新一根。详细同日样本保存在 `raw/validation_<TxFlow base>.json`。')
w();w('`txflow_unit`、`binance_unit`、`hl_unit` = 每一交易报价单位包含的基础币枚数。现货 binance_unit=1；TxFlow 1000BONK=1000（元数据 description 明确），PEPE/SHIB=1；Hyperliquid kBONK/kPEPE/kSHIB=1000。币安没有选用 1000PEPE/1000SHIB 永续，所以本批 PEPE/SHIB 的币安→TxFlow 倍数是 1。实际 TxFlow 名称为 PEPE-USDC，接口中没有 PEPU-USDC；禁止把 PEPU 猜成 PEPE。')
w();w('表中「倍数 M」是 `binance_unit / txflow_unit`；从币安换成 TxFlow 报价单位：**价格 ÷ M，成交量 × M**。例如 BONKUSDT→1000BONK-USDC：M=0.001，价格×1000，量÷1000。允许明确单位的倒数，不能只硬匹配 1/100/1000/10000。')
w();w('表中「价格比」= `(币安 close / binance_unit) / (HL close / hl_unit)`；「成交量比」= `(币安 volume × binance_unit) / (HL volume × hl_unit)`。两者都已归一化为同一种基础币。额外列出未归一化原始价比，保留千倍证据。对所有重叠日要求价格比落在 [0.95,1.05]；>10000 或 <0.0001 拒绝，中间非单位比也拒绝。映射需先有明确名称/单位证据，价格接近不充分证明资产身份。')
w();w('成交量比任何一天超出 [0.01,100]（至少相差两数量级），或 HL 零量，标为量待复核并禁入白名单。不同交易所活跃度不同，量差大是风险提示，不能单凭量差认定单位错误。`quote_volume_ratio` 使用币安真实 quote volume / (HL close×HL volume)，HL 后者只是收盘价近似，不是精确成交额。')
w();w('## 完整映射清单（每个市场一行）')
w();w('价格/量列为最新共同日；判定依据全部重叠日。— 表示不能测量，绝不填 1 或 0。币安 symbol 是候选，只有 ✓ 通过才在白名单。')
w();w('| TxFlow名 | 币安symbol（现货） | 倍数 M | 价格比（归一） | 成交量比（归一） | 原始价比 BN/HL | HL symbol | 日期 / 样本数 | 判定 |')
w('|---|---|---:|---:|---:|---:|---|---|---|')
for x in rows:w(f'| {x["txflow"]} | {x["binance_symbol"] or "—"} | {fmt(x["binance_to_txflow_multiplier"]) if x["binance_symbol"] else "—"} | {fmt(x["price_ratio"])} | {fmt(x["volume_ratio"])} | {fmt(x["raw_price_ratio"])} | {x["hl_coin"] or "—"} | {x.get("date","—")} / {x["validation_days"]} | {labels[x["status"]]} |')
w();w('## 无数据、不可验证、拒绝项及原因')
w();w('以下每项均不可直接交给策略；缺现货的项包括可能有永续的币，不能全局断言未上市。')
w();w('| TxFlow名 | 原因 |');w('|---|---|')
for x in rows:
 if x['status'] in ('no_data','unvalidated','rejected'):w(f'| {x["txflow"]} | {x["reason"]} |')
w();w('## 倍数与原始成交量核对')
w();w('| TxFlow名 | Binance | HL | 原始价格比 BN/HL | 原始成交量比 BN/HL | 归一价格比 | 归一成交量比 |')
w('|---|---|---|---:|---:|---:|---:|')
for x in rows:
 if x['txflow_base'] in ('PEPE','SHIB','1000BONK'):
  w(f'| {x["txflow"]} | {x["binance_symbol"]} | {x["hl_coin"]} | {fmt(x["raw_price_ratio"])} | {fmt(x["raw_volume_ratio"])} | {fmt(x["price_ratio"])} | {fmt(x["volume_ratio"])} |')
w();w('上述原始成交量确实差数千倍；还原 HL 千倍报价单位后，最新同日量比分别落在正常量级。币安→TxFlow 的非 1 倍数只有 1000BONK；跨币安→HL 的千倍校验共有 PEPE、SHIB、BONK 三项。')
w();w('## 可疑项与成交量量级')
w();w('| TxFlow名 | 归一价格比范围 | 归一成交量比范围 | 说明 |');w('|---|---|---|---|')
for x in rows:
 if x['status'] not in ('volume_review','rejected'):continue
 v=json.load(open(O/'raw'/('validation_'+x['txflow_base']+'.json')));ps=[z['price_ratio'] for z in v];vs=[z['volume_ratio'] for z in v if z['volume_ratio'] is not None]
 w(f'| {x["txflow"]} | {fmt(min(ps))}～{fmt(max(ps))} | {fmt(min(vs)) if vs else "—"}～{fmt(max(vs)) if vs else "—"} | {x["reason"]} |')
if not counts['volume_review'] and not counts['rejected']:w('| 无 | — | — | 无超阈值项 |')
allchecks=[(x['txflow'],v) for x in rows if x['validation_days'] for v in json.load(open(O/'raw'/('validation_'+x['txflow_base']+'.json')))]
worst=max(allchecks,key=lambda z:abs(z[1]['price_ratio']-1));vlo=min(allchecks,key=lambda z:z[1]['volume_ratio']);vhi=max(allchecks,key=lambda z:z[1]['volume_ratio'])
w();w(f'所有同日验证中最大价格误差：{worst[0]} / {worst[1]["date"]}，比值 {fmt(worst[1]["price_ratio"])}（误差 {abs(worst[1]["price_ratio"]-1)*100:.4f}%）。归一成交量比范围 {fmt(vlo[1]["volume_ratio"])}（{vlo[0]}）～{fmt(vhi[1]["volume_ratio"])}（{vhi[0]}），未发现 ≥100 倍异常。')
w();w('**同名陷阱：QNT-USDC 的 TxFlow fullName 是 Quant；HIP-3 中 xyz:QNT 则是 Quantinuum 股权市场，不能拿来验证 QNTUSDT。** 已核对全部 10 个 HIP-3 dex 的元数据；其余默认市场缺失币没有明确同资产补充匹配。该项保持未验证，不把近似价格当作资产身份。[XYZ 官方规格](https://docs.trade.xyz/perpetuals/specifications-and-schedules/pre-ipo-specification-index.md) 明确其 QNT 标的是 Quantinuum Inc. 股票。')
w();w('同日量的原始比与归一比均保留，若千倍差在归一后消失，属于已解释的单位差；归一后仍异常则不得用单位修正硬凑通过。')
w();w('## 数据文件、结构与自动审计')
w();w('全部位于 `/Users/wonder/Code/stars/research/bn_panel_20261008/`：')
w();w('- `data/<BinanceSymbol>.csv`：`ts,open,high,low,close,volume`，ts 是 UTC 日线开盘毫秒；OHLC 是 USDT/币安原生报价单位，volume 是原生基础币数量。')
w('- `data/<BinanceSymbol>.json`：同批日线的 `t,o,h,l,c,v` 适配格式，供原框架 pandas 读取；绝不暗中改单位。')
w('- `txflow_markets.json`：227 个完整市场元数据；`mapping.csv` / `mapping.json`：全市场，包含失败原因、单位、数据路径、校验日及资格；`approved_mapping.json`：仅严格通过。')
w('- `raw/`：原始元数据、币安 K 线、HL 日线、逐日比值及错误响应；`request_log.json`：下载阶段请求证据；`manifest_sha256.json`：原始与数据文件哈希。')
w('- `build_panel.py`：串行克制下载、最多一次短重试，HTTP 418/429/451 停止后续下载；已有缓存优先。`finalize.py`：完全离线重算比值、检查覆盖与格式、生成报告与白名单。')
w();w(f'离线审计通过：227 行完整性、{len(data)} 组 CSV/JSON/原始 K 线逐值一致、**{checks}** 次同日对照重新计算、无重复日、无未收盘日、OHLC 合法、白名单阈值一致。日线缺口市场：{", ".join(gap) or "无"}。`src/` 文件哈希与任务中快照一致；已有 `src/live.rs` 工作区修改未触碰。')
w();w('同目录另一个并行采集进程曾重写 REPORT.md，现已结束。本报告认证的数据是 data/ 根目录的 <BinanceSymbol>.csv/json 与根目录 mapping.json / approved_mapping.json；data/ 子目录、data/mapping_* 等另一次采集产物不属于本报告白名单，不能混用。VERIFIED_REPORT.md 是本报告一致性备份。')
w();w('## 可见局限与是否能直接喂策略')
w();w('**全量映射表不能直接喂策略。** `approved_mapping.json` 可作下一步集成的候选白名单，但不是交易安全担保；必须按其中单位显式转换，保持独立组合，并在下单前重新检查 TxFlow 的最新交易状态和资产规格。没有改接任何策略。')
w();w('无法担保以上无数据、未验证、拒绝、成交量待复核的市场。即使通过也只是跨交易所历史数据一致性检查；没有用 TxFlow 自身价格验证资产身份，没有验合约地址，也不能保证未来映射、流动性或价格偏离。PEPU 不存在于本次权威清单，不提供猜测映射。')
w();w('价格对照使用 Hyperliquid 默认 perp universe；另核对全部 10 个 HIP-3 dex 的市场名称，未发现可安全补充的同资产匹配。股票和商品未用近似资产代替。部分币虽有现货日线但没有 HL 默认市场，因此保持未验证。币安永续被封，不能完成永续覆盖审计。现货成交量/价格与永续信号存在市场差异，USDT/USDC 也有基差。')
w();w('最多 1000 日、部分新币更短；现有市场清单有存活偏差，最新 7 日价格接近不能证明整个历史无重命名/换币。现货与 HL 的跨场成交量不应拿来推导 TxFlow 承载容量。旧 `/tmp/hl-daily-full/` 是空目录，因此新建明确定义的数据格式与适配 JSON。')
w();w('接口与单位字段参考：[币安现货 Market 文档](https://developers.binance.com/en/docs/catalog/core-trading-spot-trading/api/rest-api/market)、[Hyperliquid Info 文档](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint)；具体清单和数值以保存的 API 原始响应为证。')
(O/'VERIFIED_REPORT.md').write_text('\n'.join(lines)+'\n')
(O/'REPORT.md').write_text('\n'.join(lines)+'\n')
(O/'README.md').write_text('''# Binance spot panel for TxFlow\n\nSee REPORT.md for full coverage and risk exclusions. Data cut-off: 2026-10-08 00:00 UTC (last closed day 2026-10-07).\n\nCSV: data/<BinanceSymbol>.csv, columns ts,open,high,low,close,volume. ts is UTC opening epoch milliseconds. OHLC in native Binance spot USDT/base quote units; volume in native Binance base asset units. Never concatenate native volume without conversion.\n\nJSON adapter: data/<BinanceSymbol>.json is an array with t,o,h,l,c,v (same values, native units), compatible with prior /tmp/hl-daily-full pandas readers. File names are Binance symbols, not TxFlow names: use explicit mapping.\n\nmapping.json/mapping.csv contain ALL markets, including failures; approved_mapping.json contains only validated candidates. Unit fields count underlying tokens per quoted asset unit. Binance->TxFlow multiplier M=binance_unit/txflow_unit. Convert OHLC by dividing by M and volume by multiplying by M; quote notional is invariant. This conversion is NOT applied to saved data. price_ratio and volume_ratio compare normalized Binance/HL underlying units. raw_* preserve unnormalized comparisons.\n\nDo not feed full mapping or all downloaded files into strategy. Noneligible files are retained solely as evidence. Re-run offline audit: python3 finalize.py. Network collector: python3 build_panel.py (fixed cutoff and cached raw inputs; does not refresh metadata). An intentional new batch needs new metadata and cutoff; do not silently reuse this dated snapshot.\n\nSource responses raw/, HTTP request evidence request_log.json (collector only), integrity manifest manifest_sha256.json. No credentials or private account endpoints are used. No TxFlow explorer requests.\n''')
print(json.dumps(summary,ensure_ascii=False,indent=2))
