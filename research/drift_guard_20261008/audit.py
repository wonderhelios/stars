from panel import *
T,_,_=targets()
H={}
for f in glob.glob('/tmp/hl-hist/*.json'):
 H[os.path.basename(f)[:-5]]={int(x['t']):float(x['o']) for x in json.load(open(f))}
rows=[]
for i,t in enumerate(times):
 if not (pd.Timestamp('2026-03-11',tz='UTC').value//1000000<=t<=pd.Timestamp('2026-10-02',tz='UTC').value//1000000):continue
 ids=np.flatnonzero(T[i-1]);absw=np.abs(T[i-1,ids]);cv=[];missing=set()
 for h in range(24):
  known=[t+h*3600000 in H.get(names[j],{}) for j in ids];cv.append(float(absw@np.array(known)));missing.update(names[j] for j,ok in zip(ids,known) if not ok)
 rows.append(dict(date=str(dates[i]),min_coverage=min(cv),missing=sorted(missing)))
json.dump(rows,open(OUT+'/hourly_coverage.json','w'),indent=2)
print('complete days',sum(x['min_coverage']>1-1e-9 for x in rows),'/',len(rows),'mean coverage',np.mean([x['min_coverage'] for x in rows]));print('missing',sorted(set(c for x in rows for c in x['missing'])))
