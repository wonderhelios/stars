#!/usr/bin/env python3
"""Resumable public data collection; only research output; 2 network workers."""
import concurrent.futures as cf
import csv, datetime as dt, hashlib, json, math, os, statistics, subprocess, threading, time, urllib.parse
from pathlib import Path
ROOT=Path(__file__).resolve().parent
DATA=ROOT/'data'; RAW=DATA/'raw'
DAY=86400000
ENV=os.environ.copy(); ENV.update(https_proxy='http://127.0.0.1:7897',http_proxy='http://127.0.0.1:7897',all_proxy='socks5://127.0.0.1:7897')
LOCK=threading.Lock()
for folder in ['binance_spot','hyperliquid','daily','validation']:(DATA/folder).mkdir(parents=True,exist_ok=True)
def save(path,obj):
 path=Path(path); path.parent.mkdir(parents=True,exist_ok=True);tmp=path.with_suffix(path.suffix+'.tmp');tmp.write_text(json.dumps(obj,ensure_ascii=False,indent=2));tmp.replace(path)
def read(path):return json.loads(Path(path).read_text())
def request(url,path,body=None):
 path=Path(path);path.parent.mkdir(parents=True,exist_ok=True)
 if path.exists():return read(path)
 for attempt in range(3):
  dest=path.with_suffix(path.suffix+'.download')
  cmd=['curl','-sS','--connect-timeout','15','--max-time','40','-o',str(dest),'-w','%{http_code}',url]
  if body is not None:cmd+=['-H','Content-Type: application/json','-d',json.dumps(body)]
  started=time.time();p=subprocess.run(cmd,env=ENV,capture_output=True,text=True)
  try:result=json.loads(dest.read_text())
  except Exception:result=None
  code=int(p.stdout) if p.stdout.isdigit() else 0
  rec={'time_utc':dt.datetime.now(dt.timezone.utc).isoformat(),'url':url,'body':body,'attempt':attempt+1,'http':code,'seconds':round(time.time()-started,3),'stderr':p.stderr[:400],'output':str(path.relative_to(ROOT))}
  with LOCK:
   with (DATA/'requests.jsonl').open('a') as f:f.write(json.dumps(rec,ensure_ascii=False)+'\n')
  if p.returncode==0 and code==200 and result is not None:
   dest.replace(path); time.sleep(.5); return result
  if code in [418,429,451] or (400<=code<500):
   save(path.with_suffix('.error.json'),{'request':rec,'response':result});raise RuntimeError(f'HTTP {code}: {result}')
  if attempt<2:time.sleep(2*(attempt+1))
 save(path.with_suffix('.error.json'),{'request':rec,'response':result});raise RuntimeError(f'HTTP {code}, {p.stderr}, response={str(result)[:200]}')
def csvwrite(path,rows,fields):
 path=Path(path);tmp=path.with_suffix('.csv.tmp')
 with tmp.open('w',newline='') as f:
  w=csv.DictWriter(f,fieldnames=fields);w.writeheader();w.writerows([{k:r.get(k,'') for k in fields} for r in rows])
 tmp.replace(path)
def date(ts):return dt.datetime.fromtimestamp(ts/1000,dt.timezone.utc).strftime('%Y-%m-%d')
TX=read(RAW/'txflow_perpMeta.json')['universe']
HL={x['name']:x for x in read(RAW/'hl_meta.json')['universe']}
SPOT=read(RAW/'hl_spotMeta.json'); TOKENS={x['index']:x for x in SPOT['tokens']}
if not (DATA/'run_config.json').exists():
 now=int(time.time()*1000);save(DATA/'run_config.json',{'snapshot_ms':now,'last_closed_open_ms':now//DAY*DAY-DAY,'source':'binance_spot_USDT','workers':2,'timeout_seconds':40,'retries':2,'price_tolerance':.05,'volume_anomaly_bounds':[.01,100]})
CFG=read(DATA/'run_config.json');TARGET=CFG['last_closed_open_ms'];CUTOFF=TARGET+DAY
SYMS={x['symbol'] for x in read(RAW/'bn_spot_prices.json')}
CAND={v['name']:('BONK' if v['baseCurrency']=='1000BONK' else v['baseCurrency'])+'USDT' for v in TX}
CAND={k:s for k,s in CAND.items() if s in SYMS}
def getmetadata():
 syms=sorted(set(CAND.values()));out=[]
 for i in range(0,len(syms),25):
  q=urllib.parse.urlencode({'symbols':json.dumps(syms[i:i+25],separators=(',',':'))})
  v=request('https://api.binance.com/api/v3/exchangeInfo?'+q,RAW/f'bn_exchange_batch_{i//25:02}.json')
  out.extend(v['symbols'])
 save(RAW/'bn_selected_exchange.json',{'symbols':out});return {x['symbol']:x for x in out}
def checked_bars(arr):
 if not isinstance(arr,list):raise ValueError('Kline response not a list')
 result=[];prev=None
 for x in arr:
  ts=int(x[0]);values=list(map(float,x[1:6]));o,h,l,c,v=values
  if not all(math.isfinite(a) for a in values) or min(o,h,l,c)<=0 or v<0 or h<max(o,c,l) or l>min(o,c,h):raise ValueError('Invalid OHLCV')
  if ts%DAY or (prev is not None and ts<=prev):raise ValueError('Bad daily timestamp')
  prev=ts
  if int(x[6])<CUTOFF:result.append({'ts':ts,'open':o,'high':h,'low':l,'close':c,'volume':v})
 return result
def hlref(base):
 alias={'PEPE':('kPEPE',1000),'SHIB':('kSHIB',1000),'1000BONK':('kBONK',1000)}
 if base in alias:return (*alias[base], 'perp',True)
 if base in HL:return (base,1,'perp',True)
 # A same-ticker spot token is only a numeric cross-check, NOT verified identity.
 token=[x for x in SPOT['tokens'] if x['name']==base]
 if len(token)==1:
  pairs=[p for p in SPOT['universe'] if p['tokens'][0]==token[0]['index'] and TOKENS[p['tokens'][1]]['name']=='USDC']
  if len(pairs)==1:return (pairs[0]['name'],1,'spot_same_ticker_unconfirmed',False)
 return (None,None,None,False)
def task(m,meta):
 name=m['name'];base=m['baseCurrency'];unit=1000 if base=='1000BONK' else 1
 r={'txflow_name':name,'txflow_index':m['index'],'base_currency':base,'tx_unit':unit,'binance_symbol':CAND.get(name,''),'binance_unit':1,'bn_price_to_tx_mult':unit,'bn_volume_to_tx_mult':1/unit,'status':'pending','reason':'','warnings':[],'strategy_ready':False,'mapping_verified':False}
 if name not in CAND:
  r.update(status='no_binance_spot',reason='币安现货 ticker 全清单无对应 USDT symbol；永续出口 IP 封禁，未声称币安全产品无数据');return r
 symbol=CAND[name];r['binance_status']=meta[symbol]['status'];r['binance_base_asset']=meta[symbol]['baseAsset']
 if meta[symbol]['baseAsset']!=('BONK' if base=='1000BONK' else base):
  r.update(status='identity_mismatch',reason='exchangeInfo baseAsset 不符');return r
 try:
  q=urllib.parse.urlencode({'symbol':symbol,'interval':'1d','limit':1000,'endTime':CUTOFF-1})
  arr=request('https://api.binance.com/api/v3/klines?'+q,RAW/'klines'/f'{symbol}.json')
  bars=checked_bars(arr);csvwrite(DATA/'binance_spot'/f'{symbol}.csv',bars,['ts','open','high','low','close','volume'])
  r.update(binance_rows=len(bars),binance_first_date=date(bars[0]['ts']) if bars else None,binance_last_date=date(bars[-1]['ts']) if bars else None)
  normalized=[{**b,**{k:b[k]*unit for k in ['open','high','low','close']},'volume':b['volume']/unit} for b in bars]
  csvwrite(DATA/'daily'/f'{name}.csv',normalized,['ts','open','high','low','close','volume'])
 except Exception as e:r.update(status='binance_request_failed',reason=str(e));return r
 coin,hunit,kind,trusted=hlref(base);r.update(hl_symbol=coin,hl_unit=hunit,hl_kind=kind,hl_identity_confirmed=trusted)
 if not coin:
  r.update(status='unvalidated_no_hl',reason='Hyperliquid 默认永续/同名现货无可靠对照；HIP-3 同名不自动等同（例如 xyz:QNT）')
  if meta[symbol]['status']!='TRADING':r.update(status='binance_inactive',reason=f"币安现货状态 {meta[symbol]['status']}；且无可靠 HL 同日对照")
  return r
 try:
  arr=request('https://api.hyperliquid.xyz/info',RAW/'hl_candles'/f'{coin.replace("/","_").replace(":","_")}.json',{'type':'candleSnapshot','req':{'coin':coin,'interval':'1d','startTime':TARGET-13*DAY,'endTime':CUTOFF-1}})
  if not isinstance(arr,list):raise ValueError('HL candles not list')
  hbars=[]
  for b in arr:
   ts=int(b['t']);vals={k:float(b[v]) for k,v in [('open','o'),('high','h'),('low','l'),('close','c'),('volume','v')]}
   if ts%DAY or min(vals[k] for k in ['open','high','low','close'])<=0 or vals['volume']<0 or not all(math.isfinite(x) for x in vals.values()):raise ValueError('Bad HL candle')
   if int(b['T'])<CUTOFF:hbars.append({'ts':ts,**vals})
  hbars.sort(key=lambda b:b['ts'])
  if len({b['ts'] for b in hbars})!=len(hbars):raise ValueError('duplicate HL candles')
  csvwrite(DATA/'hyperliquid'/f'{coin.replace("/","_").replace(":","_")}.csv',hbars,['ts','open','high','low','close','volume'])
 except Exception as e:r.update(status='hl_request_failed',reason=str(e));return r
 bn={b['ts']:b for b in bars};hn={b['ts']:b for b in hbars};common=sorted(set(bn)&set(hn))
 details=[]
 for ts in common:
  b=bn[ts];h=hn[ts];pv=h['close']/b['close'];norm=(b['close']/1)/(h['close']/hunit);vr=b['volume']/h['volume'] if h['volume']>0 else None;nv=b['volume']/(h['volume']*hunit) if h['volume']>0 else None
  details.append({'ts':ts,'date':date(ts),'binance_close':b['close'],'hl_close':h['close'],'price_ratio_hl_bn_raw':pv,'price_ratio_bn_hl_underlying':norm,'binance_volume':b['volume'],'hl_volume':h['volume'],'volume_ratio_bn_hl_raw':vr,'volume_ratio_bn_hl_underlying':nv})
 save(DATA/'validation'/f'{name}.json',details)
 r['comparison_days']=len(common)
 if not common:r.update(status='no_common_date',reason='没有相同 UTC 日的已收盘 K 线');return r
 last=details[-1];r.update({k:v for k,v in last.items() if k!='ts'});r['comparison_date']=last['date']
 recent=details[-7:];r['price_error_max_7d']=max(abs(d['price_ratio_bn_hl_underlying']-1) for d in recent)
 vs=[d['volume_ratio_bn_hl_underlying'] for d in recent if d['volume_ratio_bn_hl_underlying'] is not None];r['volume_ratio_underlying_median_7d']=statistics.median(vs) if vs else None
 if last['price_ratio_hl_bn_raw']>10000 or last['price_ratio_hl_bn_raw']<.0001:r.update(status='rejected_price_extreme',reason='原始价格比 >10000 或 <0.0001，剔除');return r
 if common[-1]!=TARGET:r.update(status='stale_common_date',reason=f'最近共同日 {date(common[-1])} 不是目标 {date(TARGET)}');return r
 if r['price_error_max_7d']>.05:r.update(status='rejected_price',reason='单位归一后最近最多 7 日的价格偏差超过 5%，剔除');return r
 if not trusted:r.update(status='unconfirmed_hl_spot_identity',reason='HL 现货仅同 ticker，未确认 token 身份；数值比通过也不纳入映射白名单');return r
 if meta[symbol]['status']!='TRADING':r.update(status='binance_inactive',reason=f"币安现货状态 {meta[symbol]['status']}，不能用旧价格冒充当前日线");return r
 if len(recent)<3:r.update(status='insufficient_overlap',reason='同日校验不足 3 根，不能担保');return r
 r.update(mapping_verified=True,status='verified',reason='交易状态正常；同日及最近最多 7 日价格在预设单位 ±5% 内')
 if r['price_error_max_7d']>.01:r['warnings'].append('price_basis_over_1pct')
 if last['volume_ratio_bn_hl_underlying'] is None or not .01<=last['volume_ratio_bn_hl_underlying']<=100 or r['volume_ratio_underlying_median_7d'] is None or not .01<=r['volume_ratio_underlying_median_7d']<=100:
  r['warnings'].append('volume_scale_anomaly');r['status']='verified_volume_review'
 if len(bars)<100:r['warnings'].append('history_under_100_days')
 if any(bars[i]['ts']-bars[i-1]['ts']!=DAY for i in range(1,len(bars))):r['warnings'].append('daily_gaps')
 r['strategy_ready']=not r['warnings'];return r
def identity_override(r):
 exclusions=read(DATA/'identity_exclusions.json') if (DATA/'identity_exclusions.json').exists() else {}
 if r['txflow_name'] in exclusions:
  x=exclusions[r['txflow_name']];r.update(status=x['status'],reason=x['reason'],mapping_verified=False,strategy_ready=False,identity_source=x['source'])
 elif r.get('binance_status') and r['binance_status']!='TRADING':
  r.update(status='binance_inactive',reason=f"币安现货状态 {r['binance_status']}；日线截至 {r.get('binance_last_date')}，无最新同日校验",mapping_verified=False,strategy_ready=False)
 return r
LABELS={'pending':'待处理','verified':'✓ 价格/量级校验通过','verified_volume_review':'✓ 价格通过；成交量待核对','no_binance_spot':'✗ 无币安现货数据','unvalidated_no_hl':'✗ 有币安数据但无 HL 对照','binance_inactive':'✗ 币安现货已停用','unconfirmed_hl_spot_identity':'✗ HL 同 ticker 身份未证实','rejected_price':'✗ 价格不符，剔除','rejected_price_extreme':'✗ 价格极端，剔除','stale_common_date':'✗ 无最新同日价格','no_common_date':'✗ 没有共同日','insufficient_overlap':'✗ 校验天数不足','hl_request_failed':'✗ HL 请求失败','binance_request_failed':'✗ 币安请求失败','identity_mismatch':'✗ 身份不符'}
def fmt(v):return '—' if v is None or v=='' else f'{v:.6g}' if isinstance(v,float) else str(v)
def outputs(rows,finished=False):
 ordered=[rows.get(m['name'],{'txflow_name':m['name'],'status':'pending','reason':'尚未处理','warnings':[]}) for m in TX]
 save(DATA/'mapping_all.json',ordered)
 fields=['txflow_name','txflow_index','base_currency','binance_symbol','binance_status','tx_unit','binance_unit','bn_price_to_tx_mult','bn_volume_to_tx_mult','hl_symbol','hl_unit','hl_kind','comparison_date','price_ratio_hl_bn_raw','price_ratio_bn_hl_underlying','price_error_max_7d','volume_ratio_bn_hl_raw','volume_ratio_bn_hl_underlying','volume_ratio_underlying_median_7d','binance_rows','status','mapping_verified','strategy_ready','reason','warnings']
 csvwrite(DATA/'mapping_all.csv',[{**r,'warnings':';'.join(r.get('warnings',[]))} for r in ordered],fields)
 save(DATA/'mapping_verified.json',[r for r in ordered if r.get('mapping_verified')]);save(DATA/'mapping_strict.json',[r for r in ordered if r.get('strategy_ready')])
 stats={s:sum(r['status']==s for r in ordered) for s in sorted({r['status'] for r in ordered})};save(DATA/'coverage.json',{'finished':finished,'markets':len(TX),'processed':len(rows),'verified':sum(r.get('mapping_verified',False) for r in ordered),'strict':sum(r.get('strategy_ready',False) for r in ordered),'statuses':stats})
 n=sum(r.get('mapping_verified',False) for r in ordered);multi=[r for r in ordered if r.get('mapping_verified') and (r['tx_unit']!=r['binance_unit'])];hkmulti=[r for r in ordered if r.get('mapping_verified') and r['hl_unit']!=1]
 lines=['# TxFlow 币安现货数据与符号校验',f'\n生成 UTC：{dt.datetime.now(dt.timezone.utc).isoformat()}。状态：'+('完整批次已结束' if finished else '采集中；中间结果已落盘'),f'\n目标已收盘日：**{date(TARGET)} UTC**。运行冻结截止：{date(CUTOFF)} 00:00 UTC；不会混入正在形成的日线。', '\n## 1. 覆盖统计',f'\nTxFlow `/info` perpMeta 返回 **{len(TX)}** 个市场（全部未标 delisted/haltTrading）→ 映射校验通过 **{n}** ✓，其中 TxFlow/币安单位不同 **{len(multi)}** ✓；HL 参照倍数市场 **{len(hkmulti)}**。未通过或缺数据 **{len(TX)-n}** ✗。严格候选 **{sum(r.get("strategy_ready",False) for r in ordered)}**；已处理 {len(rows)}/{len(TX)}。', '\n选择 **币安 USDT 现货**；不是永续。BTC 现货代理探针 HTTP 200、1000 根真实日线。永续探针 HTTP 418 / code -1003：代理出口 IP 临时被封禁（不是已证实的地区限制），见 `data/raw/btc_futures_probe.json`。没有用测试网、模拟数据或 TxFlow 价量补洞。','\n| 状态 | 数量 |\n|---|---:|']
 lines += [f'| {LABELS.get(s,s)} | {v} |' for s,v in stats.items()]
 lines += ['\n## 2. 判定与单位定义','\n币安候选由本次完整 ticker 清单筛选，再用 exchangeInfo 核对 baseAsset/交易状态。同名仅生成候选；不做模糊字符串猜测。TxFlow `PEPU-USDC` 不在实际返回清单中，实际为 `PEPE-USDC`，不能把 PEPU 自动当 PEPE。','\n每个有币安候选且存在 HL 对照的币独立请求最多 14 日 HL candleSnapshot，与币安按 UTC 开盘毫秒对齐。必须包含目标日，至少 3 个同日点；最近最多 7 日归一价格全部在 1±5% 才通过（超过 1% 另标提示）。极端原始比 >10000 / <0.0001 直接剔除。没有 HL 同日数据的币绝不标成功。','\n表中 **倍数 = 币安 OHLC → TxFlow 单位的价格乘数**；成交量用其倒数。**价格比 = HL 原始 close / 币安原始 close**；**成交量比 = 币安原始基础币 volume / HL 原始 volume**。HL kPEPE/kSHIB/kBONK 为 1000 币单位，所以原始价格比约 1000；这不代表币安现货是倍数合约。归一价格比 = Binance close / (HL close / HL单位)；归一量比 = Binance volume / (HL volume × HL单位)。','\n量级异常：目标日或最近最多 7 日中位数的归一量比不在 [0.01,100]（或 HL 零成交量），列入复核，不进严格候选。不同交易所/现货与永续的成交活跃度本来不同，量比异常只能提示单位或流动性问题，不能仅凭此认定映射错。','\n`1000BONK-USDC → BONKUSDT`：TxFlow metadata description 明确 1000 BONK，OHLC ×1000、volume ÷1000。PEPE/SHIB 现货无需改单位；HL 对照需除以 1000。','\n## 3. 完整映射表（每个市场一行）','\n| TxFlow名 | 币安symbol | 倍数 | 价格比 HL/BN | 成交量比 BN/HL | 判定 | HL参照 | 归一价格比 | 归一量比 | 同日 UTC | 说明 |','|---|---|---:|---:|---:|---|---|---:|---:|---|---|']
 for r in ordered:
  vals=[r['txflow_name'],r.get('binance_symbol',''),r.get('bn_price_to_tx_mult'),r.get('price_ratio_hl_bn_raw'),r.get('volume_ratio_bn_hl_raw'),LABELS.get(r['status'],r['status']),r.get('hl_symbol'),r.get('price_ratio_bn_hl_underlying'),r.get('volume_ratio_bn_hl_underlying'),r.get('comparison_date'),r.get('reason','')+('；'+','.join(r['warnings']) if r.get('warnings') else '')]
  lines.append('| '+' | '.join(fmt(v).replace('|','/').replace('\n',' ') for v in vals)+' |')
 lines+=['\n## 4. 无数据/未校验币清单与原因','\n以下均未进入验证映射白名单；“无现货”只表示本次所选 USDT 现货源无数据，不代表币安永续或其他报价币绝对没有。']
 lines += [f'- **{r["txflow_name"]}**：{r.get("reason","")}' for r in ordered if not r.get('mapping_verified')]
 lines+=['\n## 5. 可疑项与成交量异常','\n| TxFlow | 价格归一最大偏差（最近7日） | 归一量比（当日/7日中位） | 状态/提示 |\n|---|---:|---|---|']
 suspicious=[r for r in ordered if r.get('warnings') or r['status'] in ['rejected_price','rejected_price_extreme','stale_common_date','unconfirmed_hl_spot_identity','binance_inactive','no_common_date','insufficient_overlap']]
 for r in suspicious:lines.append(f'| {r["txflow_name"]} | {fmt(r.get("price_error_max_7d"))} | {fmt(r.get("volume_ratio_bn_hl_underlying"))} / {fmt(r.get("volume_ratio_underlying_median_7d"))} | {LABELS.get(r["status"],r["status"])}；{",".join(r.get("warnings",[]))} |')
 lines+=['\n特别剔除 **LIT-USDC**：TxFlow metadata 是 Lighter Protocol；币安旧 LIT 为 Litentry 并已换币为 HEI，本次状态 BREAK。不能把旧 LIT 历史给 Lighter 使用。[币安官方换币公告](https://www.binance.com/en/support/announcement/detail/2a9feaa556f74dcdaa2192366f0e247c)。', '\n## 6. 文件位置与结构',f'\n根目录：`{ROOT}`。','\n- `data/txflow_markets.csv/json`：全部 TxFlow metadata 市场与可交易标记，原始响应 `data/raw/txflow_perpMeta.json`。','- `data/binance_spot/<symbol>.csv`：币安原始单位，最新最多 1000 根已收盘日线。','- `data/daily/<TxFlow名>.csv`：换算到 TxFlow 单位的币安日线，所有有有效数据的候选（**包含未通过映射的候选，不能整目录无筛选读取**）。','- 日线统一表头 `ts,open,high,low,close,volume`，ts 为 UTC 开盘 Unix 毫秒，价格报价币为 USDT，volume 为基础币/合约单位数量。归一文件只换单位，来源仍是币安现货。','- `data/hyperliquid/`：独立校验参照日线；`data/validation/<TxFlow名>.json`：每个同日的价格、量比证据。','- `data/mapping_all.csv/json`：227 行全部状态；`mapping_verified.json` 包括量级待复核项；`mapping_strict.json` 仅无价格基差、量级、短历史、缺日提示的候选。','- `alternate_batch/`：目录中另一批采集产物完整保留，含不同倍数字段约定；不属于本次规范输出，不要混用。其通过币名单与本批一致。', '- `data/raw/`：真实 API JSON；`requests.jsonl`：请求时刻、HTTP、重试次数及耗时；`run_config.json`：冻结日期及阈值；`coverage.json`：统计；`manifest.json`：SHA256 与文件清单。','- `/tmp/hl-daily-full/` 本次存在但为空，无法推断旧格式，因此采用并明示上述六列格式。','- 重跑 `python3 collect_validate.py` 使用已有成功响应；失败可重试，已完成币落盘 checkpoint 保留。不会刷新旧日期，若要新的快照应新建目录或另建采集批次。','\n离线校验：`python3 verify_artifacts.py` 对照原始 JSON 检查每个 CSV 单元、全部同日价量比、单位换算、日期截止、完整名单与哈希。最近结果见 `verification.json`；源码与目录内既有快照的核对见 `src_integrity.json`。', '\n## 7. 看得见的局限与策略使用结论','\n**不能把完整映射表直接喂给策略。** 这一步没有修改策略代码，也没有完成策略适配、仓位/执行单位或独立组合验证。严格候选只是本次数据校验清单，仍需按字段读取和重新校验最新数据。','\n价格比能发现单位或严重错配，不能独立证明 token 身份；HL 同名现货只做数值对照，未证实身份者一律排除。HIP-3 同 ticker（如 xyz:QNT）不自动当作同一加密资产。未校验项、退市项、价格异常项均不敢担保；通过但带 volume_scale_anomaly / price_basis_over_1pct / history_under_100_days / daily_gaps 的币也需人工复核。','\n只取最新最多 1000 根；上线不足 1000 天的币不会补齐或伪造。新币/换币/重用 ticker 可能有结构变化，逐日完整历史的 token 身份与拆并币没有额外权威核验。同日收盘可能有现货/永续基差，USDT/USDC 报价差没有额外汇率换算。TxFlow 倍数只从名称与 perpMeta description 提取，没有调用其价量/盘口。','\n网络走指定本机代理，采集全局最多 2 并发；单次 40 秒，瞬态失败最多重试 2 次，418/429/451 不盲重试。没有调用 TxFlow explorer。','\nAPI 文档：[币安官方现货 Kline 定义](https://github.com/binance/binance-spot-api-docs/blob/master/rest-api.md#klinecandlestick-data)、[Hyperliquid Info](https://hyperliquid.gitbook.io/Hyperliquid-docs/for-developers/api/info-endpoint)、[全部永续 metadata](https://hyperliquid.gitbook.io/Hyperliquid-docs/for-developers/api/info-endpoint/perpetuals)。实际数据证据以本地原始响应为准。']
 (ROOT/'REPORT.md').write_text('\n'.join(lines)+'\n')
def main():
 save(DATA/'txflow_markets.json',TX);csvwrite(DATA/'txflow_markets.csv',[{**m,'tradable':not m.get('delisted',False) and not m.get('haltTrading',False)} for m in TX],['name','index','baseCurrency','quoteCurrency','fullName','delisted','haltTrading','tradable','description'])
 meta=getmetadata(); rows={}
 for p in (DATA/'checkpoints').glob('*.json'):r=identity_override(read(p));rows[r['txflow_name']]=r;save(p,r)
 outputs(rows)
 with cf.ThreadPoolExecutor(max_workers=2) as pool:
  jobs={pool.submit(task,m,meta):m for m in TX if m['name'] not in rows or rows[m['name']]['status'] in ['hl_request_failed','binance_request_failed']}
  for f in cf.as_completed(jobs):
   m=jobs[f]
   try:r=f.result()
   except Exception as e:r={'txflow_name':m['name'],'status':'binance_request_failed','reason':repr(e),'warnings':[]}
   r=identity_override(r);rows[m['name']]=r;save(DATA/'checkpoints'/f'{m["name"]}.json',r);outputs(rows)
   print(f'{len(rows)}/{len(TX)} {m["name"]} {r["status"]}',flush=True)
 outputs(rows,True)
 manifest=[]
 for p in sorted(DATA.rglob('*')):
  if p.is_file() and p.name not in ['manifest.json','collection.log']:manifest.append({'path':str(p.relative_to(ROOT)),'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()})
 save(DATA/'manifest.json',manifest)
 print(json.dumps(read(DATA/'coverage.json'),ensure_ascii=False),flush=True)
if __name__=='__main__':main()
