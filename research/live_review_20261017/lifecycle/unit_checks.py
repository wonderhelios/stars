import numpy as np
import test_lifecycle as m
m.N=24*7;m.HT=np.arange(m.start,m.start+m.N*m.HOUR,m.HOUR);m.A=np.full((m.N,m.K,4),100.)
w=np.zeros(m.K);w[:2]=[.5,-.5];m.daily_weights=lambda i,eq,mode:w.copy()
base=m.sim('baseline',fee=0)
assert np.allclose(base['r'],0),'static market invented pnl'
a=m.sim('max48_cool24',fee=0)
ex=[x for x in a['ledger'] if x['kind']=='risk_exit'];assert ex and all(x['age_hours']==48 for x in ex)
# Trigger at completed hour 2 low95 or less; next hourly open gaps to85.
m.A[2,0]=[100,100,90,90];m.A[3:,0]=85
s=m.sim('stop5_cool24',fee=0);ex=[x for x in s['ledger'] if x['kind']=='risk_exit' and x['coin']==str(m.b.names[0])]
assert ex[0]['price']==85 and ex[0]['t']==m.start+3*m.HOUR
entries=[x for x in s['ledger'] if x['kind']=='entry' and x['coin']==str(m.b.names[0])]
assert not any(ex[0]['t']<x['t']<ex[0]['t']+24*m.HOUR for x in entries),'cooldown bypassed'
assert any(x['t']==m.start+49*m.HOUR for x in entries),'expected eligible daily reentry missing'
# An hour cannot use its own favourable peak to trigger a pre-existing trailing barrier.
q=np.zeros(m.K);q[0]=1;e=np.full(m.K,100.);peak=e.copy();sigma=np.full(m.K,.03);hi=np.full(m.K,110.);lo=np.full(m.K,95.)
assert not m.trigger('trail8_4_cool24',q,e,peak,sigma,hi,lo)[0]
peak[0]=110;assert m.trigger('trail8_4_cool24',q,e,peak,sigma,hi,lo)[0]
print('PASS: static MTM, exact48h exit, next-hour gap fill,24h cooldown, no intra-hour peak lookahead')
