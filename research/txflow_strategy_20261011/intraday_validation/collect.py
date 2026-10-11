from pathlib import Path
import concurrent.futures as cf,subprocess,zipfile,io,csv,json,hashlib,sqlite3,time,sys,datetime
P=Path(__file__).resolve().parent;ROOT=P.parents[2];(P/'data').mkdir(exist_ok=True)
with sqlite3.connect(f"file:{ROOT/'research/txflow_binance_20261009/binance-candles.sqlite'}?mode=ro",uri=True) as c:names={r[0] for r in c.execute('select distinct coin from candles')}
mp=[r for r in json.load(open(ROOT/'config/binance-mapping.json')) if r['txflow_name'] in names]
def get(a):
 m,month=a;s=m['binance_symbol'];out=P/'data'/f'{s}-{month}.json'
 if out.exists():
  old=json.load(open(out))
  if old['status']=='ok' or old.get('http')=='404' or '--retry' not in sys.argv:return old['status']
 url=f'https://data.binance.vision/data/spot/monthly/klines/{s}/5m/{s}-5m-{month}.zip'
 dest=out.with_suffix('.zip');r=subprocess.run(['curl','-L','-sS','--connect-timeout','5','--max-time','20','-o',str(dest),'-w','%{http_code}',url],capture_output=True,text=True)
 record={'symbol':s,'coin':m['txflow_name'],'url':url,'http':r.stdout,'status':'missing','bars':[],'retrieved_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'curl_returncode':r.returncode,'error':r.stderr[:300]}
 if r.returncode==0 and r.stdout=='200':
  raw=dest.read_bytes();record['sha256']=hashlib.sha256(raw).hexdigest();z=zipfile.ZipFile(io.BytesIO(raw));bars=[]
  for row in csv.reader(io.TextIOWrapper(z.open(z.namelist()[0]))):
   ts=int(row[0]);ts=ts//1000 if ts>10**14 else ts
   if ts%86400000 in [0,300000,3600000]:bars.append([ts,float(row[1])*m['bn_price_to_tx_mult']])
  record.update(status='ok',bars=bars)
 out.write_text(json.dumps(record));dest.unlink(missing_ok=True);return record['status']
months=[f'2026-{m:02}' for m in range(1,7)] if '--first-half' in sys.argv else ['2026-07','2026-08','2026-09']
jobs=[(m,month) for m in mp for month in months]
with cf.ThreadPoolExecutor(max_workers=8) as e:
 counts={}
 for k,s in enumerate(e.map(get,jobs)):
  counts[s]=counts.get(s,0)+1
  if k%20==0:print(k+1,len(jobs),counts,flush=True)
print(counts,flush=True)
(P/('collection_first_half.json' if '--first-half' in sys.argv else 'collection.json')).write_text(json.dumps({'jobs':len(jobs),'counts':counts,'months':months},indent=2))
