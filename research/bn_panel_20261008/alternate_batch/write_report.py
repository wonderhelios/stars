import json,pathlib,collections,hashlib
O=pathlib.Path(__file__).resolve().parent
rows=json.loads((O/'mapping.json').read_text()); universe=json.loads((O/'txflow_markets.json').read_text())
assert len(rows)==len(universe)==227
assert [r['txflow'] for r in rows]==[m['name'] for m in universe]
counts=collections.Counter(r['status'] for r in rows)
good=[r for r in rows if r['strategy_eligible']]; multi=[r for r in good if r['txflow_unit']!=1]; bad=[r for r in rows if not r['strategy_eligible']]
for r in good:
 checks=json.loads((O/'raw'/('validation_'+r['txflow_base']+'.json')).read_text())
 assert len(checks)==r['validation_days'] and checks
 assert all(abs(c['price_ratio']-1)<=.05 and c['volume_ratio'] is not None and .01<=c['volume_ratio']<=100 for c in checks)
 assert (O/r['data_file']).exists()
fmt=lambda x:'—' if x is None else f'{x:.6g}'
labels={'validated':'✓ 已验证','no_data':'✗ 无数据','unvalidated':'✗ 未验证','rejected':'✗ 价格拒绝','volume_review':'✗ 成交量待复核'}
s=[ '# 币安映射校验报告', '',f'覆盖：{len(rows)}/{len(universe)} 个市场全部处理。普通映射成功 {len(good)-len(multi)} ✓ / 倍数合约 {len(multi)} ✓ / 无可用已验证数据 {len(bad)} ✗。成功总计 {len(good)}（包含倍数合约）。', '',f'细分状态：{dict(counts)}。这里“无可用已验证数据”包括未验证、价格拒绝、成交量待复核，不等于币安全市场不存在。', '', '## 方法与边界', '', '币安必须走本地代理，续跑并发 2，40 秒超时，最多 2 次重试；每完成一币即落盘。mapping.json 保持原格式，原始请求、日线和逐日校验保存在 raw/ 与 data/。', '', '截止时间为 2026-10-08 00:00 UTC，只使用此前完整日线。币安现货最多 1000 根日线；校验窗口为 2026-10-01 至 2026-10-07，检查窗口内全部重叠日，并非全部历史。价格参考来自 Hyperliquid 主 perp universe，未直接抓取 TxFlow 成交日线。TxFlow 资产身份依据其市场元数据；名称匹配不足以排除同名资产。', '', '价格比 = Binance close / (Hyperliquid close / hl_unit)，逐日误差必须 ≤5%。成交量比 = Binance base volume / (Hyperliquid volume × hl_unit)，逐日要求 [0.01,100]。不同交易所、现货与永续的成交量本就不同，该阈值仅检查量级，不能证明资产身份或流动性可互换。', '', '表中价格比、成交量比为最后一个重叠日；通过与否按全部重叠日判断。倍数列表示一个 TxFlow 报价单位对应多少底层币。币安现货每单位均为 1；转换现货价格到 TxFlow 单位应乘 txflow_unit，转换成交量应除 txflow_unit。现有 binance_to_txflow_multiplier 字段为 1/txflow_unit，适合数量转换，不能不辨含义地用于价格。', '', '币安永续 exchangeInfo 已返回 HTTP 418 / -1003 IP 封禁，未核实永续市场；无现货匹配不能解释为无永续合约。LUNA2 → LUNA 等别名仍须核对身份、迁移与合约规格。', '', '## 完整映射表', '', '| TxFlow名 | 币安symbol | 倍数 | 价格比 | 成交量比 | 判定 |','|---|---|---:|---:|---:|---|']
for r in rows:s.append(f'| {r["txflow"]} | {r["binance_symbol"] or "—"} | {r["txflow_unit"]} | {fmt(r["price_ratio"])} | {fmt(r["volume_ratio"])} | {labels[r["status"]]}（{r["validation_days"]}日） |')
s+=['','## 无可用已验证数据清单与原因','']
for r in bad:s.append(f'- **{r["txflow"]}**：{r["status"]}；{r["reason"]}')
s+=['','## 可疑项','', '倍数检查使用 5% 容差，并考虑报价方向：原始 Binance/HL 比可能是 1/1000，倒数才对应 1000。归一化后应接近 1，不要求观测价格比精确等于整数。']
sus=[]
for r in rows:
 p=O/'raw'/('validation_'+r['txflow_base']+'.json')
 cs=json.loads(p.read_text()) if p.exists() else []
 strange=[c for c in cs if not any(abs(c['raw_price_ratio']/k-1)<=.05 or abs(c['raw_price_ratio']*k-1)<=.05 for k in (1,100,1000,10000))]
 if strange or r['volume_flag'] or r['status']=='rejected':
  sus.append(r['txflow']);s.append(f'- **{r["txflow"]}**：{r["reason"]}；归一化价格范围 {fmt(min(c["price_ratio"] for c in cs)) if cs else "—"}～{fmt(max(c["price_ratio"] for c in cs)) if cs else "—"}；成交量比范围 {fmt(min(c["volume_ratio"] for c in cs if c["volume_ratio"] is not None)) if any(c["volume_ratio"] is not None for c in cs) else "—"}～{fmt(max(c["volume_ratio"] for c in cs if c["volume_ratio"] is not None)) if any(c["volume_ratio"] is not None for c in cs) else "—"}。')
if not sus:s.append('- 有重叠日的资产中未发现超出上述容差或成交量阈值的项；缺少参考数据的资产仍不能确认。')
s+=['', '倍数项：'+('、'.join(r['txflow'] for r in multi) or '无')+'。PEPE/SHIB 的 Hyperliquid k 单位与 TxFlow 倍数是不同维度，不能混用。', '', '## 可以直接喂给策略吗？', '', '**整表不能直接作为实盘策略的可信映射表。** strategy_eligible=true 只表示本次跨市场日线校验通过，可作为研究候选；先按该字段过滤，明确现货/永续、价格/数量倍数、日期覆盖与 USDT/USDC 基差，并直接核实 TxFlow 的价格及合约定义后，再评估策略适用性。1000 根现货日线和 7 日交叉校验不保证历史映射始终成立。', '', '**不敢担保的币**：上面全部无可用已验证数据项和可疑项；即便通过，也不担保实盘执行、永续映射或未来表现。倍数项、LUNA2 别名尤其需要执行前复核。', '', '## 文件核验', '']
h=json.loads((O/'src_hashes_before.json').read_text()); root=O.parents[2]; changed=[p for p,v in h.items() if hashlib.sha256((root/p).read_bytes()).hexdigest()!=v]
s.append('相对于原始 src 哈希快照：'+('全部一致。' if not changed else '发生变化：'+str(changed)+'；这可能来自共享工作区的其他工作，本任务未修改 src。'))
s.append(f'已检查 227 条唯一顺序映射、{len(good)} 条通过项的逐日价格/成交量阈值及数据文件存在性。')
(O/'REPORT.md').write_text('\n'.join(s)+'\n');print(counts, 'validated',len(good),'multi',len(multi),'bad',len(bad),'src_changed',changed)
