import json,time,datetime,subprocess,os,hashlib,threading,concurrent.futures
from pathlib import Path
OUT=Path(__file__).resolve().parent/'retrieval'; OUT.mkdir(exist_ok=True)
NOW=int(time.time()*1000); START=int(datetime.datetime(2023,6,1,tzinfo=datetime.timezone.utc).timestamp()*1000)
lock=threading.Lock();last=0

def get(payload):
 global last
 for attempt in range(3):
  with lock:
   time.sleep(max(0,.2-(time.monotonic()-last)));last=time.monotonic()
  cmd=['curl','--noproxy','*','--connect-timeout','10','--max-time','40','-sS','-X','POST','https://api.hyperliquid.xyz/info','-H','Content-Type: application/json','-d',json.dumps(payload)]
  p=subprocess.run(cmd,capture_output=True)
  try:
   if p.returncode:raise ValueError(p.stderr.decode())
   d=json.loads(p.stdout)
   if not isinstance(d,(list,dict)):raise ValueError(str(d))
   return d
  except Exception as e:
   err=str(e);time.sleep(1+attempt)
 raise RuntimeError(err)
meta=get({'type':'meta'});(OUT/'meta.json').write_text(json.dumps(meta));coins=[x['name'] for x in meta['universe']]
(OUT/'request.json').write_text(json.dumps(dict(end_ms=NOW,daily_start_ms=START,hourly_start_ms=NOW-93*86400000,coins=coins,concurrency=2,interval_ms=200,retries=2,transport='direct')))

def fetch(job):
 coin,interval,root,start=job;path=Path(root)/(coin+'.json')
 try:
  d=get({'type':'candleSnapshot','req':dict(coin=coin,interval=interval,startTime=start,endTime=NOW)})
  if not isinstance(d,list):raise ValueError(str(d))
  for x in d:
   assert x['s']==coin and x['i']==interval and all(k in x for k in ['t','T','o','h','l','c','v'])
  d=sorted({x['t']:x for x in d}.values(),key=lambda x:x['t'])
  tmp=path.with_suffix('.part');tmp.write_text(json.dumps(d,separators=(',',':')));tmp.replace(path)
  return dict(coin=coin,interval=interval,count=len(d),first=d[0]['t'] if d else None,last=d[-1]['t'] if d else None,sha256=hashlib.sha256(path.read_bytes()).hexdigest())
 except Exception as e:return dict(coin=coin,interval=interval,error=str(e))
for root in ['/tmp/hl-daily-full','/tmp/hl-hist']:
 Path(root).mkdir(exist_ok=True);Path(root,'README.md').write_text('Hyperliquid info candleSnapshot; one <coin>.json per meta universe entry, raw JSON array sorted/deduplicated by t. t/T UTC milliseconds; o/h/l/c price strings; v base volume; n trades; s symbol; i interval. Includes potentially incomplete current bar: exclude T >= frozen request end for research. Empty arrays retained. Metadata and per-file hashes in stars/research/trailing_tp_20261008/retrieval. Current universe includes delisted entries; historical coverage varies.\n')
jobs=[(c,i,r,s) for i,r,s in [('1d','/tmp/hl-daily-full',START),('1h','/tmp/hl-hist',NOW-93*86400000)] for c in coins]
with concurrent.futures.ThreadPoolExecutor(max_workers=2) as ex,open(OUT/'manifest.jsonl','w') as f:
 for j,r in enumerate(ex.map(fetch,jobs)):
  f.write(json.dumps(r)+'\n');f.flush()
  if j%10==0:print(j+1,'/',len(jobs),r,flush=True)
print('DONE',flush=True)
