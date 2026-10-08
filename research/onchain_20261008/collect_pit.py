"""Collect a fresh immutable first-seen snapshot. Does not trade or backdate availability."""
from pathlib import Path
import datetime,os,subprocess,sys,json,uuid
if '--help' in sys.argv:
 print('python collect_pit.py : create fresh UTC timestamp snapshot; proxy http://127.0.0.1:7897; no trading');raise SystemExit
root=Path(__file__).resolve().parent
stamp=datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')+'_'+uuid.uuid4().hex[:8]
archive=root/'snapshots'/stamp;archive.mkdir(parents=True,exist_ok=False)
env=dict(os.environ,ONCHAIN_ARCHIVE_DIR=str(archive))
r=subprocess.run([sys.executable,str(root/'fetch.py')],env=env)
meta={'snapshot_directory':str(archive),'capture_finished_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'first_seen_rule':'Use each response only after retrieved_end_utc. This is not its original publication date. Do not overwrite past decision vintages.','returncode':r.returncode}
if (archive/'api_audit.json').exists():
 audit=json.load(open(archive/'api_audit.json'));meta['failed']=[k for k,v in audit.items() if v.get('returncode') or v.get('parse_error')]
(archive/'snapshot.json').write_text(json.dumps(meta,indent=2));print(json.dumps(meta,indent=2))
raise SystemExit(r.returncode)
