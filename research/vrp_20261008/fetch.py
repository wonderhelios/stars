import os,json,time,datetime,requests
from pathlib import Path
O=Path(__file__).resolve().parent; (O/'raw').mkdir(exist_ok=True)
os.environ.update(https_proxy='http://127.0.0.1:7897',http_proxy='http://127.0.0.1:7897',all_proxy='socks5://127.0.0.1:7897')
s=requests.Session();base='https://www.deribit.com/api/v2/public/'
def get(method,params):
 for k in range(4):
  try:
   r=s.get(base+method,params=params,timeout=40);r.raise_for_status();j=r.json();assert 'result' in j,j;return j
  except Exception:
   if k==3:raise
   time.sleep(2)
now=get('get_time',{})['result'];json.dump({'server_ms':now,'retrieved_utc':datetime.datetime.now(datetime.timezone.utc).isoformat()},open(O/'raw/metadata.json','w'))
start=int(datetime.datetime(2021,1,1,tzinfo=datetime.timezone.utc).timestamp()*1000);day=86400000
for coin in ['BTC','ETH']:
 rows=[]
 for a in range(start,now,180*day):
  path=O/'raw'/f'{coin}_dvol_{a}.json'
  if path.exists():j=json.load(open(path))
  else:j=get('get_volatility_index_data',dict(currency=coin,start_timestamp=a,end_timestamp=min(a+180*day,now),resolution='1D'));json.dump(j,open(path,'w'))
  rows+=j['result']['data']
 rows=list({x[0]:x for x in rows}.values());rows.sort();json.dump(rows,open(O/f'{coin}_dvol.json','w'));print(coin,len(rows),flush=True)
 j=get('get_book_summary_by_currency',dict(currency=coin,kind='option'));json.dump(j,open(O/f'raw/{coin}_summary.json','w'))
 j=get('get_instruments',dict(currency=coin,kind='option',expired='false'));json.dump(j,open(O/f'raw/{coin}_instruments.json','w'))
 # ATM call/put in nearest expiry to 30 days: real book snapshot, not historical fill evidence.
 inst=j['result'];expiry=min({x['expiration_timestamp'] for x in inst},key=lambda e:abs(e-now-30*day))
 summaries=json.load(open(O/f'raw/{coin}_summary.json'))['result']; idx={x['instrument_name']:x for x in summaries}
 avail=[x for x in inst if x['expiration_timestamp']==expiry];spot=next(idx[x['instrument_name']]['underlying_price'] for x in avail if x['instrument_name'] in idx)
 strike=min({x['strike'] for x in avail},key=lambda k:abs(k-spot))
 for x in avail:
  if x['strike']==strike:
   j=get('get_order_book',dict(instrument_name=x['instrument_name'],depth=20));json.dump(j,open(O/f"raw/{x['instrument_name']}_book.json",'w'))
