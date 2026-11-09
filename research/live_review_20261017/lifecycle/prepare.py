import os,sys,json,hashlib,tarfile,concurrent.futures,urllib.request,time
from pathlib import Path
import numpy as np
os.environ['INCLUDE_RECENT']='1';ROOT=Path(__file__).resolve().parent;sys.path.insert(0,str(ROOT.parent));import backtest as b
# Restore only hourly files from existing archive, isolated from shared caches.
out=ROOT/'hourly';out.mkdir(exist_ok=True)
with tarfile.open('research/trailing_tp_20261008/retrieval/data_snapshot.tar.gz') as tar:
 members=[m for m in tar.getmembers() if m.isfile() and m.name.startswith('hl-hist/') and m.name.endswith('.json')]
 for m in members:
  p=out/Path(m.name).name
  if not p.exists():p.write_bytes(tar.extractfile(m).read())
print('restored hourly files',len(members),flush=True)
# Fetch only recent extension (Oct7 through Oct16); cache has prior ~90d.
END=1792195200000;START=END-10*86400000
# All symbols with any recent >$10m rolling volume; enough for filtered re-rank candidates.
start=int(np.searchsorted(b.ts,END-105*86400000));needed=[str(b.names[j]) for j in range(b.K) if np.any(b.V[start:,j]>=1e7)]
new=ROOT/'recent_hourly';new.mkdir(exist_ok=True)
def fetch(c):
 p=new/(c+'.json')
 if p.exists():return c,len(json.loads(p.read_text()))
 for attempt in range(3):
  try:
   payload={'type':'candleSnapshot','req':{'coin':c,'interval':'1h','startTime':START,'endTime':END-1}}
   q=urllib.request.Request('https://api.hyperliquid.xyz/info',data=json.dumps(payload).encode(),headers={'Content-Type':'application/json'})
   a=json.load(urllib.request.urlopen(q,timeout=20));assert isinstance(a,list);a=[r for r in a if r['T']<END];p.write_text(json.dumps(a));return c,len(a)
  except Exception as e:
   if attempt==2:return c,str(e)
   time.sleep(1+attempt)
res=[]
with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
 for i,z in enumerate(pool.map(fetch,needed)):
  res.append(z)
  if i%10==0:print(i,z,flush=True)
(ROOT/'fetch.json').write_text(json.dumps({'start':START,'end':END,'symbols':needed,'results':res},indent=2));print('DONE',flush=True)
