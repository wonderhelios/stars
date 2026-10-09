from pathlib import Path
import json,hashlib
p=Path(__file__).resolve().parent;results={}
for name,cut in [('legacy_stag2_snapshot.py','print("=== 全期'),('legacy_honest_snapshot.py','base,days=run()')]:
 file=p/name;s=file.read_text();s=s.replace("d=[x for x in d if float(x['c'])>0]","d=[x for x in d if float(x['c'])>0 and int(x['t'])>=1685577600000]");ns={'__file__':str(file)}
 exec(s.split(cut)[0],ns)
 for fee,slip in [(.00045,.0003),(.00045,.00025),(.00045,0)]:
  if 'stag2' in name:
   r,turn=ns['run'](1,fee=fee,slip=slip);stats=ns['stat'](r);years=ns['days']
  else:
   r,dates=ns['run'](fee=fee,slip=slip);stats=ns['st'](r);years=ns['np'].array([ns['datetime'].datetime.fromtimestamp(t/1000,ns['datetime'].UTC).year for t in dates])
  def sharpe(v):return float(v.mean()/v.std(ddof=1)*365**.5)
  label=f'{name}|fee={fee}|slip={slip}'
  results[label]=dict(sharpe=sharpe(r),days=len(r),sub_2023_24=sharpe(r[years<=2024]),sub_2025_26=sharpe(r[years>=2025]),source_hash=hashlib.sha256(file.read_bytes()).hexdigest())
  print(label,results[label],flush=True)
(p/'legacy_baseline_results.json').write_text(json.dumps(results,indent=2))
