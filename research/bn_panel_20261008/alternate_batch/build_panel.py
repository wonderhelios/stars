"""Read-only public market data collection; no strategy imports or modifications."""
import csv,datetime,hashlib,json,pathlib,subprocess,time,urllib.parse,math
from concurrent.futures import ThreadPoolExecutor
import threading
O=pathlib.Path(__file__).resolve().parent; R=O/'raw'; D=86400000
CUTOFF=int(datetime.datetime(2026,10,8,tzinfo=datetime.timezone.utc).timestamp()*1000)
BN_STOP=threading.Event()
LOG=json.loads((O/'request_log.json').read_text()) if (O/'request_log.json').exists() else []
def save(p,x):
 tmp=p.with_suffix(p.suffix+'.tmp');tmp.write_text(json.dumps(x,ensure_ascii=False,indent=2));tmp.replace(p)
def call(name,url,body=None):
 p=R/(name+'.json')
 if p.exists():
  j=json.loads(p.read_text())
  if isinstance(j,list) or (isinstance(j,dict) and 'code' not in j and 'error' not in j):return j
 if name.startswith('bn_') and BN_STOP.is_set():raise RuntimeError('币安此前返回限流/封禁；停止后续网络请求')
 args=['curl','--compressed','--proxy','http://127.0.0.1:7897','--connect-timeout','10','--max-time','40','-sS','-w','\n%{http_code}',url]
 if body is not None:args+=['-H','Content-Type: application/json','--data',json.dumps(body)]
 for attempt in range(3):
  z=subprocess.run(args,capture_output=True);content,_,status=z.stdout.rpartition(b'\n');code=int(status) if status.isdigit() else 0
  LOG.append({'time':datetime.datetime.now(datetime.timezone.utc).isoformat(),'name':name,'url':url,'body':body,'http_status':code,'curl_exit':z.returncode,'error':z.stderr.decode()})
  if code in (418,429,451):
   if name.startswith('bn_'):BN_STOP.set()
   save(R/(name+'_error.json'),{'status':code,'body':content.decode(errors='replace')});raise RuntimeError(f'HTTP {code}: {content[:200]!r}')
  try:
   j=json.loads(content)
   if code!=200 or z.returncode or isinstance(j,dict) and ('code' in j or 'error' in j):raise ValueError(str(j)[:200])
   p.write_bytes(content);time.sleep(.35);return j
  except Exception as e:
   if attempt==2:save(R/(name+'_error.json'),{'status':code,'error':str(e),'stderr':z.stderr.decode(),'body':content.decode(errors='replace')});raise
   time.sleep(2)

def write_csv(path,rows):
 with path.open('w') as f:
  w=csv.writer(f);w.writerow(['ts','open','high','low','close','volume']);w.writerows(rows)

def fetch():
 tx=json.load(open(R/'txflow_meta.json'))['universe']; hl=json.load(open(R/'hl_meta.json'))['universe'];hs={m['name']:m for m in hl}
 bn=json.load(open(R/'bn_spot_selected_exchange.json'))['symbols'];bs={m['symbol']:m for m in bn}
 save(O/'txflow_markets.json',tx)
 out=json.loads((O/'mapping.json').read_text()) if (O/'mapping.json').exists() else [];fatal_bn=None
 assert [x['txflow'] for x in out]==[x['name'] for x in tx[:len(out)]], 'checkpoint is not a universe prefix'
 def evaluate(pair):
  i,m=pair
  fatal_bn=None
  base=m['baseCurrency'];under={'1000BONK':'BONK','LUNA2':'LUNA'}.get(base,base);symbol=under+'USDT';tm=1000 if base=='1000BONK' else 1
  hc={'PEPE':'kPEPE','SHIB':'kSHIB','1000BONK':'kBONK'}.get(base,base);hm=1000 if hc.startswith('k') and hc in ('kPEPE','kSHIB','kBONK') else 1
  row={'txflow':m['name'],'txflow_base':base,'underlying':under,'txflow_unit':tm,'binance_symbol':symbol if symbol in bs else None,'binance_market':'spot','binance_unit':1,'binance_to_txflow_multiplier':1/tm,'hl_coin':hc if hc in hs else None,'hl_unit':hm,'status':'no_data','reason':'','price_ratio':None,'raw_price_ratio':None,'volume_ratio':None,'raw_volume_ratio':None,'quote_volume_ratio':None,'volume_flag':False,'strategy_eligible':False,'validation_days':0,'dates':[]}
  if symbol not in bs:row['reason']='币安现货无对应 USDT symbol；永续因出口 IP HTTP 418 无法核实（不能断言币安全市场没有）'
  elif bs[symbol]['status']!='TRADING':row['reason']='币安现货 symbol 存在但非 TRADING: '+bs[symbol]['status']
  else:
   try:
    if fatal_bn:raise RuntimeError(fatal_bn)
    url='https://api.binance.com/api/v3/klines?'+urllib.parse.urlencode({'symbol':symbol,'interval':'1d','limit':1000,'endTime':CUTOFF-1})
    raw=call('bn_'+symbol,url)
    bars=[x for x in raw if x[0]+D<=CUTOFF]
    assert bars and len({x[0] for x in bars})==len(bars)
    for x in bars:assert all(math.isfinite(float(v)) and float(v)>0 for v in x[1:5]) and float(x[5])>=0 and float(x[3])<=min(float(x[1]),float(x[4]))<=max(float(x[1]),float(x[4]))<=float(x[2])
    rows=[[x[0],*x[1:6]] for x in bars];write_csv(O/'data'/(symbol+'.csv'),rows)
    # Framework adapter uses t/o/h/l/c/v; units remain native Binance spot.
    save(O/'data'/(symbol+'.json'),[dict(zip(['t','o','h','l','c','v'],x)) for x in rows])
    row.update(bars=len(bars),first_ts=bars[0][0],last_ts=bars[-1][0],data_file='data/'+symbol+'.csv')
    if hc not in hs:
     row['reason']='币安有日线，但 Hyperliquid 主 perp universe 无同资产；禁止猜测替代资产';row['status']='unvalidated'
     if base=='QNT':row['reason']+='；HIP-3 xyz:QNT 是 Quantinuum 股票，非 Quant 代币（XYZ 官方规格），禁止同名替代'
    else:
     hraw=call('hl_'+hc,'https://api.hyperliquid.xyz/info',{'type':'candleSnapshot','req':{'coin':hc,'interval':'1d','startTime':CUTOFF-8*D,'endTime':CUTOFF-1}})
     hb={x['t']:x for x in hraw if x['t']+D<=CUTOFF};bb={x[0]:x for x in bars};common=sorted(set(hb)&set(bb))[-7:]
     checks=[]
     for t in common:
      b=bb[t];h=hb[t];bp=float(b[4]);hp=float(h['c']);bv=float(b[5]);hv=float(h['v']);pr=bp/(hp/hm);vr=bv/(hv*hm) if hv>0 else None
      checks.append({'ts':t,'date':datetime.datetime.fromtimestamp(t/1000,datetime.timezone.utc).date().isoformat(),'binance_close':bp,'hl_close':hp,'binance_volume':bv,'hl_volume':hv,'raw_price_ratio':bp/hp,'price_ratio':pr,'raw_volume_ratio':bv/hv if hv else None,'volume_ratio':vr,'quote_volume_ratio':float(b[7])/(hp*hv) if hp*hv else None})
     save(R/('validation_'+base+'.json'),checks);row['validation_days']=len(checks);row['dates']=[x['date'] for x in checks]
     if not checks:row['status']='unvalidated';row['reason']='Hyperliquid 未返回同日完整日线'
     else:
      last=checks[-1];row.update({k:last[k] for k in ['price_ratio','raw_price_ratio','volume_ratio','raw_volume_ratio','quote_volume_ratio']});row['date']=last['date']
      row['volume_flag']=any(x['volume_ratio'] is None or x['volume_ratio']>100 or x['volume_ratio']<.01 for x in checks)
      good=all(abs(x['price_ratio']-1)<=.05 for x in checks)
      if any(x['price_ratio']>10000 or x['price_ratio']<.0001 for x in checks):row['status']='rejected';row['reason']='归一化价格比离谱；剔除'
      elif not good:row['status']='rejected';row['reason']='归一化价格偏差超过 5%；剔除，不将任意接近倍数认作同资产'
      elif row['volume_flag']:row['status']='volume_review';row['reason']='价格通过，归一化成交量比有日超出 [0.01,100]，待人工复核'
      else:row['status']='validated';row['reason']='同资产明确；全部重叠日归一化价格误差≤5%；成交量量级通过';row['strategy_eligible']=True
   except Exception as e:
    row['reason']=str(e);row['status']='unvalidated' if row.get('bars') else 'no_data'
    if 'HTTP 418' in str(e) or 'HTTP 429' in str(e) or 'HTTP 451' in str(e):fatal_bn=str(e)
  return i,row
 with ThreadPoolExecutor(max_workers=2) as pool:
  for i,row in pool.map(evaluate,list(enumerate(tx))[len(out):]):
   out.append(row);save(O/'mapping.json',out);save(O/'request_log.json',LOG)
   print(f'{i+1}/{len(tx)} {row["txflow_base"]}: {row["status"]} {row.get("price_ratio")} {row["reason"]}',flush=True)
 return out
if __name__=='__main__':fetch()
