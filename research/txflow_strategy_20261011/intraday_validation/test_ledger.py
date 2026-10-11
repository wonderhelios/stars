from pathlib import Path
import numpy as np
p=Path(__file__).resolve().parent
n={'__file__':str(p/'verify.py')}
exec((p/'verify.py').read_text().split('runs={};rows=[]')[0],n)
# Constant prices: account change must equal charged fees, no manufactured PNL.
n.update(K=2,steps=np.array([1.,1.]),lev=np.array([3.,3.]),ix=np.array([1,2,3]),price={0:np.ones((3,2))})
n['ns']['targets']={'MV':np.tile([.5,-.5],(4,1))};n['ns']['V']=np.full((4,2),1e8)
x,d=n['simulate']('MV',0,'base');equity=3000.;fees=0.
for r in x:fees+=equity*r[2];equity*=1+r[0]
assert np.isclose(equity,3000-fees)
assert not d[:,3].any()
# Same positive-price path: untraded future values cannot alter previous returns.
n['price'][0]=np.array([[1.,1.],[1.5,.7],[1.2,.8]])
a,_=n['simulate']('MV',0,'base');n['price'][0][-1]=[1.1,.9];b,_=n['simulate']('MV',0,'base');assert np.array_equal(a[:-1],b[:-1])
print('PASS constant-price fee conservation; admission limits; future-price causality')
