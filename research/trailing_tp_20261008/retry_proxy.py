from pathlib import Path
import json,concurrent.futures
p=Path(__file__).resolve().parent
s=(p/'retrieve.py').read_text();ns={'__file__':str(p/'retrieve.py')}
exec(s[:s.index("meta=get(")].replace(".2-(time.monotonic()-last)","1.0-(time.monotonic()-last)").replace("['curl','--noproxy','*'","['curl','--proxy','http://127.0.0.1:7897','--noproxy',''"),ns)
exec(s[s.index('def fetch(job):'):s.index("for root in ")],ns)
req=json.load(open(p/'retrieval/request.json'));ns['NOW']=req['end_ms']
rows=json.load(open(p/'retrieval/final_manifest.json'));final={(r['coin'],r['interval']):r for r in rows}
jobs=[(r['coin'],r['interval'],'/tmp/hl-daily-full' if r['interval']=='1d' else '/tmp/hl-hist',req['daily_start_ms'] if r['interval']=='1d' else req['hourly_start_ms']) for r in final.values() if 'error' in r]
print('Retry jobs',len(jobs),flush=True)
with concurrent.futures.ThreadPoolExecutor(max_workers=2) as ex,open(p/'retrieval/proxy_manifest.jsonl','a') as f:
 for i,r in enumerate(ex.map(ns['fetch'],jobs)):
  f.write(json.dumps(r)+'\n');f.flush();final[(r['coin'],r['interval'])]=r
  print(i+1,r,flush=True)
(p/'retrieval/final_manifest.json').write_text(json.dumps(list(final.values()),indent=2))
print('DONE',flush=True)
