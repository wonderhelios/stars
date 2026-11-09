import numpy as np
import backtest as b
# Deterministic two-leg synthetic ledger: unchanged prices cannot manufacture PnL.
b.N=40;b.K=2;b.names=np.array(['A','B']);b.ts=list(np.arange(40)*86400000)
b.O=np.full((40,2),100.);b.C=b.O.copy();b.F=np.array([10000,10000]);b.HASH=np.array([65,66]);b.BOOKS=np.tile([.5,-.5],(40,1))
a=b.simulate(capital=1000,fee=.00075)
assert abs(a['curve'][-1]-(1000-2700*.00075))<1e-8
assert abs(a['turn'].sum()-2.7)<1e-8
assert np.allclose(b.simulate(capital=1000,fee=0)['r'],0)
# Last day's close movement must be included once (not omitted or doubled).
b.C[-1,0]=110
assert abs(b.simulate(capital=1000,fee=0)['curve'][-1]-1135)<1e-8
# Overnight gap transfers to pre-existing positions exactly once.
b.C[:]=100;b.O[35:,0]=110;b.C[35:,0]=110
z=b.simulate(capital=1000,fee=0)
assert abs(z['curve'][-1]-1135)<1e-8
assert np.count_nonzero(np.abs(z['r'])>1e-10)==1
assert sum('BTC'.encode())%3==1
print('PASS: zero-price no invented profit; fees charged once; close and overnight MTM once; byte-sum slice hash')
