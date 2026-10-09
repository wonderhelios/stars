"""Recover missing files after primary retrieval completes, sequential and rate limited."""
import json,subprocess,time,hashlib
from pathlib import Path
D=Path(__file__).resolve().parent;req=json.loads((D/'retrieval/request.json').read_text()); out=D/'retrieval/recovery_recheck.jsonl'
with out.open('a') as log:
 for interval,root,start in [('1d','/tmp/hl-daily-full',req['daily_start_ms']),('1h','/tmp/hl-hist',req['hourly_start_ms'])]:
  for coin in req['coins']:
   path=Path(root,coin+'.json')
   if path.exists():continue
   payload={'type':'candleSnapshot','req':dict(coin=coin,interval=interval,startTime=start,endTime=req['end_ms'])}; result=dict(coin=coin,interval=interval)
   for attempt in range(3):
    time.sleep(.2);p=subprocess.run(['curl','--noproxy','*','--max-time','40','-sS','https://api.hyperliquid.xyz/info','-H','Content-Type: application/json','-d',json.dumps(payload)],capture_output=True)
    try:
     data=json.loads(p.stdout);assert p.returncode==0 and isinstance(data,list)
     for x in data:assert x['s']==coin and x['i']==interval
     data=sorted({x['t']:x for x in data}.values(),key=lambda x:x['t']);tmp=path.with_suffix('.recheck.part');tmp.write_text(json.dumps(data,separators=(',',':')));tmp.replace(path)
     result.update(count=len(data),sha256=hashlib.sha256(path.read_bytes()).hexdigest());break
    except Exception as e:result['error']=str(e);time.sleep(1)
   log.write(json.dumps(result)+'\n');log.flush();print(result,flush=True)
