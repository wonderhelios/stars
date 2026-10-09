"""Read-only input inventory; deliberately does not run candidates before baseline parity."""
from pathlib import Path
import json, hashlib
from datetime import datetime, timezone
OUT=Path(__file__).resolve().parent
result={'audited_at_utc':datetime.now(timezone.utc).isoformat(),'inputs':{}}
for root in ['/tmp/hl-daily-full','/tmp/hl-hist','/tmp/m15']:
    p=Path(root); files=sorted(p.glob('*.json')); rows=0; times=[]; hashes={}; errors=[]
    for f in files:
        hashes[f.name]=hashlib.sha256(f.read_bytes()).hexdigest()
        try:
            x=json.loads(f.read_text()); rows+=len(x)
            times.extend(int(r['t']) for r in x)
        except Exception as e: errors.append({'file':str(f),'error':str(e)})
    result['inputs'][root]={'exists':p.exists(),'json_files':len(files),'rows':rows,'first_utc':datetime.fromtimestamp(min(times)/1000,timezone.utc).isoformat() if times else None,'last_utc':datetime.fromtimestamp(max(times)/1000,timezone.utc).isoformat() if times else None,'sha256':hashes,'errors':errors}
result['baseline_status']='not_reproduced_missing_daily_data'
result['candidate_runs']=0
result['source_sha256']={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in [OUT.parents[1]/'src/trader.rs',OUT.parents[1]/'docs/validation-protocol.md']}
(OUT/'data_audit.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
print(json.dumps({k:{kk:vv for kk,vv in v.items() if kk!='sha256'} for k,v in result['inputs'].items()},ensure_ascii=False,indent=2))
