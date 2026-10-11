from pathlib import Path
import sys,json,numpy as np
P=Path(__file__).resolve().parent;v=P.parent/'intraday_validation/verify.py'
sys.argv=['verify.py','--long'];n={'__file__':str(v)};exec(v.read_text().split('runs={};rows=[]')[0],n)
names=n['names'];ix=n['ix'];ns=n['ns']
def clean(a):return [[float(x) if np.isfinite(x) else None for x in row] for row in a]
x={'names':names,'dates':[int(x) for x in n['dates'][ix]],'markets':[[int(ns['meta'][k]['szDecimals']),int(ns['meta'][k]['maxLeverage'])] for k in names],'prices':{str(k//60000):clean(a) for k,a in n['price'].items()},'targets':{k:ns['targets'][k][ix-1].tolist() for k in ['MLV','MV']}}
# Match live::fresh_binance_panel: each ranked name needs 33 consecutive closed bars.
fresh=np.isfinite(ns['P']['c']) & (ns['P']['c']>0) & np.isfinite(ns['P']['v']) & (ns['P']['v']>=0)
updated={k:[] for k in ['MLV','MV']};diffdays={k:0 for k in updated};minimum=81
for d,t in enumerate(ix):
 j=t-1;eligible=fresh[j-32:j+1].all(0);minimum=min(minimum,int(eligible.sum()))
 assert eligible.sum()>=20
 ok=eligible & (ns['V'][j]>=5e6) & np.isfinite(ns['M'][j]) & np.isfinite(ns['SD'][j]);ids=np.flatnonzero(ok);assert len(ids)>=20
 k=min(8,int(np.floor(len(ids)*.2+.5)),len(ids)//2)
 for combo in updated:
  a=np.zeros(len(names))
  for c in combo:
   order=ids[np.lexsort((np.array(names)[ids],ns['scores'][c][j,ids]))];a[order[:k]]-=.5/k/len(combo);a[order[-k:]]+=.5/k/len(combo)
  a[abs(a)<1e-12]=0.;a/=abs(a).sum();diffdays[combo]+=not np.allclose(a,x['targets'][combo][d],atol=1e-12);updated[combo].append(a.tolist())
x['targets']=updated
(P/'freshness_audit.json').write_text(json.dumps({'minimum_fresh_coins':minimum,'days_weights_changed':diffdays,'required_consecutive_days':33},indent=2))
x['panel']=[{'coin':coin,'candles':[{'t':int(row.t),'c':float(row.c),'v':float(row.v)} for row in ns['frame'][ns['frame'].coin==coin].itertuples()]} for coin in names]
(P/'input.json').write_text(json.dumps(x,separators=(',',':')));print('prepared',len(ix),len(names))
