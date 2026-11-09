import json,urllib.request,concurrent.futures,time
from pathlib import Path
ROOT=Path(__file__).resolve().parent;OUT=ROOT/'recent';OUT.mkdir(exist_ok=True)
# Fixed window: latest completed UTC day is October 16, 2026.
END=1792195200000 # 2026-10-17T00:00:00Z
START=END-60*86400000
names=[p.stem for p in Path('/tmp/hl-daily-v2').glob('*.json')]
def fetch(coin):
 f=OUT/(coin+'.json')
 if f.exists():return coin,'cached'
 payload={'type':'candleSnapshot','req':{'coin':coin,'interval':'1d','startTime':START,'endTime':END-1}}
 for attempt in range(3):
  try:
   req=urllib.request.Request('https://api.hyperliquid.xyz/info',data=json.dumps(payload).encode(),headers={'Content-Type':'application/json'})
   a=json.load(urllib.request.urlopen(req,timeout=20));assert isinstance(a,list)
   a=[r for r in a if r['T']<END];f.write_text(json.dumps(a));return coin,len(a)
  except Exception as e:
   if attempt==2:return coin,str(e)
   time.sleep(1+attempt)
with concurrent.futures.ThreadPoolExecutor(max_workers=3) as p:
 results=[]
 for i,r in enumerate(p.map(fetch,names)):
  results.append(r)
  if i%20==0:print(i,r,flush=True)
(ROOT/'recent_fetch.json').write_text(json.dumps({'start':START,'end':END,'results':results},indent=2))
print('DONE',flush=True)
