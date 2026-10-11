from pathlib import Path
import urllib.request,json,re,numpy as np
P=Path(__file__).resolve().parent
try:
 d=json.load(urllib.request.urlopen('http://8.219.113.154:3000/api/txflow/live',timeout=20));rows=[]
 for x in d.get('records',[]):
  m=re.search(r'成交\s+([\d.eE+\-]+)@([\d.eE+\-]+)',x.get('result',''))
  if not m or not x.get('live') or x.get('price',0)<=0 or x.get('side') not in ['买','卖']:continue
  sz,px=map(float,m.groups());mid=x['price'];signed=(px/mid-1)*(1 if x['side']=='买' else -1)*10000
  rows.append({'ts':x['ts'],'coin':x['coin'],'plan_price':mid,'fill_price':px,'fill_notional':sz*px,'adverse_bp':signed})
 out={'matched_system_fills':len(rows),'note':'Only records with explicit system reference price and filled size/price; excludes exchange-only receipts, no fill completeness claim.'}
 if rows:
  a=np.array([r['adverse_bp'] for r in rows]);w=np.array([r['fill_notional'] for r in rows]);out.update(mean_adverse_bp=float(a.mean()),weighted_adverse_bp=float(np.average(a,weights=w)),p95_adverse_bp=float(np.quantile(a,.95)),max_adverse_bp=float(a.max()),sample_notional=float(w.sum()),first_ts=min(r['ts'] for r in rows),last_ts=max(r['ts'] for r in rows))
 (P/'slippage_summary.json').write_text(json.dumps(out,indent=2));print(out)
except Exception as e:print(type(e).__name__,str(e))
