from pathlib import Path
import numpy as np
p=Path(__file__).resolve().parent/'run.py'
ns={'__file__':str(p)}
exec(compile(p.read_text().split("runs={'baseline':baselines}")[0],str(p),'exec'),ns)
# Constant multiplier must agree with a physically implemented lower-leverage book.
for o in range(3):
 a=ns['run'](o,scale=np.full(ns['N'],2/3));b=ns['run'](o,lev=1.8)
 assert np.max(np.abs(a['r']-b['r']))<1e-12
# Future realized returns cannot alter any decision before the future starts.
cut=500
for c in ns['configs']:
 if c['kind'] in ['tail','actual_dd']:continue
 for o in range(3):
  a,_=ns['control'](c,o);old=ns['baselines'][o]['r'].copy()
  ns['baselines'][o]['r'][cut:]=.15
  b,_=ns['control'](c,o);ns['baselines'][o]['r']=old
  assert np.allclose(a[:cut+1],b[:cut+1])
assert np.all(np.diff(ns['times'])==ns['DAY'])
assert np.max(np.abs(ns['base'].sum(axis=1)))<1e-10
print('PASS: constant-exposure equivalence, future-return causality, daily grid, neutral targets')
