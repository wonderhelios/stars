from pathlib import Path
import subprocess,json,numpy as np
p=Path(__file__).resolve().parent
ns={'__file__':str(p/'baseline_resume.py')};src=(p/'baseline_resume.py').read_text();exec(src[:src.index('out=dict(config=')],ns)
input_path=p/'retrieval/factor_input_resume.tsv'
with input_path.open('w') as f:
 for coin,d in ns['raw'].items():
  for x in d:f.write(f"{coin}\t{x['t']}\t{x['c']}\t{x['v']}\n")
r=subprocess.run([str(p/'retrieval/source_factor_check'),str(input_path)],capture_output=True,text=True,check=True)
w=np.zeros((ns['n'],ns['k']));idx={c:j for j,c in enumerate(ns['names'])}
for l in r.stdout.splitlines():
 i,c,v=l.split('\t');w[int(i),idx[c]]=float(v)
py=np.array([ns['target'](i,1e6) for i in range(ns['n'])]);d=abs(py-w)
out=dict(max_abs_difference=float(d.max()),different_entries=int((d>1e-10).sum()),days=ns['n'],coins=ns['k'],source='FactorPanel copied verbatim from current src/trader.rs; rustc standalone harness',fixed_equity=1e6)
(p/'factor_verification_resume.json').write_text(json.dumps(out,indent=2));print(out)
assert d.max()<1e-10
input_path.unlink()
