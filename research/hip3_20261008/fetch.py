import json,time,datetime, pathlib,concurrent.futures,requests
ROOT=pathlib.Path(__file__).parent; RAW=ROOT/'raw';RAW.mkdir(exist_ok=True)
# Fixed liquid large US single-name universe, not selected on returns.
STOCKS='TSLA NVDA AAPL MSFT GOOGL META AMZN AMD PLTR INTC NFLX ORCL MU COIN HOOD MSTR'.split()
COINS=STOCKS+['XYZ100','SP500','GOLD','SILVER','CL','EUR','JPY']
# Immutable research as-of; actual UTC clock, not future Asia/Shanghai calendar date.
END=int(datetime.datetime(2026,10,7,17,30,tzinfo=datetime.timezone.utc).timestamp()*1000)
def call(payload):
 for k in range(5):
  try:
   r=requests.post('https://api.hyperliquid.xyz/info',json=payload,timeout=45);r.raise_for_status();return r.json()
  except Exception:
   if k==4:raise
   time.sleep(2+k*2)
def fetch(coin):
 f=RAW/f'{coin}_30m.json'
 if not f.exists():f.write_text(json.dumps(call({'type':'candleSnapshot','req':{'coin':'xyz:'+coin,'interval':'30m','startTime':END-105*86400000,'endTime':END,'dex':'xyz'}})))
 d=json.loads(f.read_text());print(coin,len(d),flush=True)
 # funding requests paginated, independently of candles; funding is required for actual net.
 f=RAW/f'{coin}_funding.json'
 if not f.exists():
  rows=[];start=END-366*86400000
  for _ in range(20):
   x=call({'type':'fundingHistory','coin':'xyz:'+coin,'startTime':start,'endTime':END})
   if not x:break
   rows+=x;ns=max(int(z['time']) for z in x)+1
   if ns<=start or len(x)<500:break
   start=ns
  f.write_text(json.dumps(rows))
 return coin
if __name__=='__main__':
 with concurrent.futures.ThreadPoolExecutor(max_workers=3) as e:
  for x in e.map(fetch,COINS):pass
 for coin in ['XYZ100','SP500','TSLA','NVDA','GOLD']:
  f=RAW/f'{coin}_book.json';f.write_text(json.dumps(call({'type':'l2Book','coin':'xyz:'+coin})))
 for typ in ['perpDexs']:
  ROOT.joinpath(typ+'.json').write_text(json.dumps(call({'type':typ})))
 print('DONE',flush=True)
