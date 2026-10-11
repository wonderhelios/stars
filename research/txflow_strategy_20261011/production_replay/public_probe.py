import json,urllib.request,time
from pathlib import Path
P=Path(__file__).resolve().parent
requests=[{'type':'l2Book','coin':'1'},{'type':'fundingHistory','coin':'1','startTime':1782864000000,'endTime':1782950400000}]
out=[]
for payload in requests:
 record={'payload':payload,'time':time.time()}
 try:
  req=urllib.request.Request('https://api.txflow.com/info',data=json.dumps(payload).encode(),headers={'Content-Type':'application/json'})
  with urllib.request.urlopen(req,timeout=20) as r: record.update(http=r.status,response=json.load(r))
 except Exception as e:record['error']=str(e)
 out.append(record);print(json.dumps(record)[:1200]);time.sleep(.5)
(P/'public_probe.json').write_text(json.dumps(out,indent=2))
