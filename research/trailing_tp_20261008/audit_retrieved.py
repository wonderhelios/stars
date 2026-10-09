from pathlib import Path
import json,hashlib,datetime,collections,tarfile
p=Path(__file__).resolve().parent;req=json.load(open(p/'retrieval/request.json'));out={}
for interval,root in [('1d','/tmp/hl-daily-full'),('1h','/tmp/hl-hist')]:
 rows=[];alltimes=[];completed=[]
 for coin in req['coins']:
  f=Path(root)/(coin+'.json')
  if not f.exists():rows.append(dict(coin=coin,error='file absent'));continue
  d=json.loads(f.read_text());tt=[x['t'] for x in d];ct=[x['t'] for x in d if x['T']<req['end_ms']];alltimes+=tt;completed+=ct
  step=86400000 if interval=='1d' else 3600000
  bound=req['daily_start_ms'] if interval=='1d' else req['hourly_start_ms']//step*step
  before=sum(t<bound for t in tt)
  rows.append(dict(coin=coin,count=len(d),before_requested_start=before,in_requested_range=len(d)-before,completed=len(ct),first=min(tt) if tt else None,last=max(tt) if tt else None,internal_missing_bars=sum(max(0,(b-a)//step-1) for a,b in zip(tt,tt[1:])),sha256=hashlib.sha256(f.read_bytes()).hexdigest()))
 valid=[r for r in rows if 'error' not in r];fingerprint=hashlib.sha256('\n'.join(r['coin']+' '+r['sha256'] for r in valid).encode()).hexdigest()
 out[interval]=dict(requested=len(req['coins']),files=len(valid),nonempty=sum(r['count']>0 for r in valid),empty=sum(r['count']==0 for r in valid),failed=len(rows)-len(valid),records=sum(r['count'] for r in valid),before_requested_start=sum(r['before_requested_start'] for r in valid),in_requested_range=sum(r['in_requested_range'] for r in valid),completed_records=sum(r['completed'] for r in valid),first=min(alltimes) if alltimes else None,last=max(alltimes) if alltimes else None,completed_last=max(completed) if completed else None,internal_missing_bars=sum(r['internal_missing_bars'] for r in valid),manifest_sha256=fingerprint,rows=rows)
(p/'retrieved_audit.json').write_text(json.dumps(out,indent=2));print({k:{z:v for z,v in d.items() if z!='rows'} for k,d in out.items()})
with tarfile.open(p/'retrieval/data_snapshot.tar.gz','w:gz') as tar:
 for root in ['/tmp/hl-daily-full','/tmp/hl-hist']:tar.add(root,arcname=Path(root).name)
(p/'retrieval/archive_sha256.txt').write_text(hashlib.sha256((p/'retrieval/data_snapshot.tar.gz').read_bytes()).hexdigest()+'  data_snapshot.tar.gz\n')
