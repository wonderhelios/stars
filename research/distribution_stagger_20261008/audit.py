"""Diagnostics only, not candidate selection. Recreates audit.json."""
import engine as e
import json,glob,os,pandas as pd,sqlite3
res={}
for cap in [5,8]:
 t,_,_=e.targets(cap)
 for lev in [1,2.7]:
  rr=[e.run(t,o,lev=lev) for o in range(3)]
  res[f'cap{cap}_lev{lev}']=dict(sharpe=float(e.np.mean([e.stat(v['r'])['sharpe'] for v in rr])),phases=[e.stat(v['r'])['sharpe'] for v in rr],annual_cost=float(e.np.mean([v['turn'].mean() for v in rr])*.00075*365))
fmeta={}
for folder in ['/tmp/hl-funding','/tmp/hl-f2']:
 stamps=[];coins=0
 for f in glob.glob(folder+'/*.json'):
  rows=json.load(open(f))
  if isinstance(rows,list):coins+=1;stamps.extend(int(x['time']) for x in rows if 'time' in x)
 fmeta[folder]=dict(coins=coins,rows=len(stamps),start=str(pd.to_datetime(min(stamps),unit='ms',utc=True)) if stamps else None,end=str(pd.to_datetime(max(stamps),unit='ms',utc=True)) if stamps else None)
old=json.load(open(os.path.join(e.OUT,'../marginal_20261008/results.json')))['meta']
res['prior_manifest_changed']=[os.path.basename(f) for f in e.FILES if old['manifest'].get(os.path.basename(f))!=e.hashlib.sha256(open(f,'rb').read()).hexdigest()];res['funding_coverage']=fmeta
p='/tmp/stars-live/candles.sqlite'
if os.path.exists(p):
 con=sqlite3.connect('file:'+p+'?mode=ro',uri=True);res['database_tables']=con.execute("select name from sqlite_master where type='table'").fetchall();res['database_coverage']=con.execute('select count(distinct coin),min(t),max(t),count(*) from candles').fetchone()
json.dump(res,open(e.OUT+'/audit.json','w'),indent=2)
