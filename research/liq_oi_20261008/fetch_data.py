"""Public, unauthenticated, read-only probes and Binance daily metrics download."""
import threading
import requests,json,os,glob,pandas as pd,zipfile,io,concurrent.futures,time,sqlite3
from pathlib import Path
OUT=Path(__file__).resolve().parent; CACHE=Path('/tmp/liq-oi-binance');CACHE.mkdir(exist_ok=True)
LOCAL=threading.local()
COINS='BTC ETH SOL XRP DOGE ADA AVAX LINK LTC BCH DOT ATOM BNB NEAR APT ARB'.split()
def probe():
 items=[]
 bodies=[{'type':'metaAndAssetCtxs'},{'type':'liquidations','coin':'BTC'},{'type':'openInterestHistory','coin':'BTC'},{'type':'userFills','user':'0x0000000000000000000000000000000000000000'},{'type':'clearinghouseState','user':'0x0000000000000000000000000000000000000000'}]
 for body in bodies:
  try:
   r=requests.post('https://api.hyperliquid.xyz/info',json=body,timeout=20); items.append({'request':body,'status':r.status_code,'body':r.text[:2500]})
  except Exception as e:items.append({'request':body,'error':str(e)})
 urls=['https://hyperliquid-archive.s3.amazonaws.com/asset_ctxs/20240701.csv.lz4','https://hl-mainnet-node-data.s3.amazonaws.com/?list-type=2&prefix=node_fills_by_block/&max-keys=1']
 for u in urls:
  try:
   r=requests.get(u,timeout=20);items.append({'url':u,'status':r.status_code,'body':r.text[:700]})
  except Exception as e:items.append({'url':u,'error':str(e)})
 c=sqlite3.connect('/tmp/stars-live/candles.sqlite');items.append({'local_oi':c.execute('select count(*),count(distinct ts),count(distinct coin),min(ts),max(ts) from oi_snapshots').fetchone()})
 json.dump(items,open(OUT/'api_probes.json','w'),indent=2);print('probes saved',flush=True)
def get(task):
 coin,day=task
 if not hasattr(LOCAL,'session'):LOCAL.session=requests.Session()
 p=CACHE/f'{coin}-{day}.json'
 if p.exists():return json.load(open(p))
 symbol=coin+'USDT';u=f'https://data.binance.vision/data/futures/um/daily/metrics/{symbol}/{symbol}-metrics-{day}.zip'
 result={'coin':coin,'day':day,'url':u}
 for attempt in range(3):
  try:
   r=LOCAL.session.get(u,timeout=12);result['status']=r.status_code
   if r.status_code==200:
    z=zipfile.ZipFile(io.BytesIO(r.content));df=pd.read_csv(z.open(z.namelist()[0]));df['create_time']=pd.to_datetime(df.create_time,utc=True)
    # Intraday observation safely before UTC cutoff; 1-day publication allowance applied downstream.
    cutoff=pd.Timestamp(day,tz='UTC')+pd.Timedelta(days=1)-pd.Timedelta(minutes=15)
    df=df[df.create_time<=cutoff].sort_values('create_time')
    if len(df):result.update({k:(str(v) if k=='create_time' else v) for k,v in df.iloc[-1].items()})
    result['rows']=len(df)
   elif r.status_code not in [404,403]:continue
   break
  except Exception as e:result['error']=str(e)
 json.dump(result,open(p,'w'));return result
if __name__=='__main__':
 probe()
 raw=json.load(open('/tmp/hl-daily-full/BTC.json')); dates=pd.date_range(pd.to_datetime(raw[0]['t'],unit='ms').normalize()-pd.Timedelta(days=3),pd.to_datetime(raw[-1]['t'],unit='ms').normalize())
 tasks=[(c,d.strftime('%Y-%m-%d')) for d in dates for c in COINS];results=[];start=time.time()
 with concurrent.futures.ThreadPoolExecutor(max_workers=96) as ex:
  for i,row in enumerate(ex.map(get,tasks)):
   results.append(row)
   if i%500==0:print(i,'/',len(tasks),'elapsed',round(time.time()-start),flush=True)
 pd.DataFrame(results).to_csv(OUT/'binance_daily_metrics.csv',index=False)
 print('done',pd.Series([r.get('status') for r in results]).value_counts().to_dict(),flush=True)
