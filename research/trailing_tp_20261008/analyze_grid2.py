import trailing_grid as g
import json,hashlib
import numpy as np
from pathlib import Path
S=g.SAVE
# Snapshot audit, frozen candle end; do not require gaps before listing/after delisting.
audit={};hours={};starts=[];ends=[];allts=set();files=[]
for root,step in [('/tmp/hl-daily-full',86400000),('/tmp/hl-hist',3600000)]:
 rows=[]
 for f in sorted(Path(root).glob('*.json')):
  blob=f.read_bytes();d=json.loads(blob);t=sorted(set(int(x['t']) for x in d if int(x.get('T',int(x['t'])+step-1))<g.end))
  rows.append(dict(coin=f.stem,bars=len(t),gaps=sum((b-a)//step-1 for a,b in zip(t,t[1:]) if b-a>step),first=t[0] if t else None,last=t[-1] if t else None))
  files.append((str(f),hashlib.sha256(blob).hexdigest()))
  if step==3600000 and f.stem in g.names:
   j=g.names.index(f.stem)
   for x in d:
    if int(x.get('T',x['t']+step-1))<g.end:hours.setdefault(int(x['t']),[]).append((j,x))
 audit[root]=dict(files=len(rows),bars=sum(x['bars'] for x in rows),at_least100=sum(x['bars']>=100 for x in rows),short=[x for x in rows if x['bars']<100],gaps=sum(x['gaps'] for x in rows),first=min(x['first'] for x in rows if x['first'] is not None),last=max(x['last'] for x in rows if x['last'] is not None),rows=rows)
(S/'audit.json').write_text(json.dumps(audit,indent=2));(S/'data_manifest.json').write_text(json.dumps(dict(files),indent=2))
# Match hourly bars to UTC days, using daily fallback outside common BTC coverage.
btc=[x for x in audit['/tmp/hl-hist']['rows'] if x['coin']=='BTC'][0];first=(btc['first']//86400000+1)*86400000;last=(btc['last']//86400000)*86400000
hd={}
for t,b in sorted(hours.items()):
 day=t//86400000*86400000
 if first<=day<last:hd.setdefault(day,[]).append((t,b))
# Coverage-complete per held asset is assessed separately; missing bar retains last price, never synthesizes an extremum.
for day in g.ts:
 if day not in hd:
  i=g.ti[day];hd[day]=[(day,[(j,dict(o=g.a['o'][i,j],h=g.H[i,j],l=g.L[i,j],c=g.P[i,j])) for j in range(g.k) if np.isfinite(g.P[i,j])])]
for act in [.1,.2,.3]:
 for dist in [.05,.1,.15]:
  for mode in ['coin','portfolio']:
   for path in ['pess','opt']:
    key=f'hour_{act}_{dist}_{mode}_{path}'
    if not (S/(key+'.npz')).exists():g.persist(key,g.simulate(act,dist,mode,path,hourly=hd))
(S/'hour_window.json').write_text(json.dumps(dict(first=first,last_exclusive=last,days=(last-first)//86400000,hourly_resolution='OHLC; intrahour sequence still unknown',outside='daily fallback')))
print('hourly done',flush=True)
base=np.load(S/'frozen_control.npz');br=base['r'];bt=base['t'];assert np.array_equal(bt,g.ts[33:])
# Code-equivalence checks without rerunning baseline: compare formula and all daily non-trigger prefixes.
checks=[]
for mode in ['coin','portfolio']:
 for path in ['pess','opt']:
  v=np.load(S/f's1_p0_0.3_0.15_{mode}_{path}.npz')['r'];ev=json.loads((S/f's1_p0_0.3_0.15_{mode}_{path}.json').read_text())['events'];firstevent=min(x[0] for x in ev) if ev else max(bt)+1;mask=bt<=firstevent
  checks.append(dict(mode=mode,path=path,prefix_days=int(mask.sum()),max_error=float(np.max(abs(v[mask]-br[mask])))))
(S/'prefix_checks.json').write_text(json.dumps(checks,indent=2))
# Paired circular moving-block bootstrap of Sharpe difference, centered at observed difference.
B=3999;rng=np.random.default_rng(20261009);N=len(br);block=30
starts=rng.integers(0,N,size=(B,(N+block-1)//block));ix=((starts[:,:,None]+np.arange(block))%N).reshape(B,-1)[:,:N]
def shar(r,axis=-1):return np.mean(r,axis=axis)/np.std(r,axis=axis,ddof=1)*np.sqrt(365)
def stats(r,b):
 delta=float(shar(r)-shar(b));count=0
 for st in range(0,B,100):
  ii=ix[st:st+100];bd=shar(r[ii])-shar(b[ii]);count+=int(np.sum(bd-delta>=delta))
 p=(count+1)/(B+1)
 return dict(sharpe=float(shar(r)),delta=delta,sub=[dict(sharpe=float(shar(r[m])),delta=float(shar(r[m])-shar(b[m]))) for m in [g.D.year<=2024,g.D.year>=2025]],p=p,p_fwer=min(1,p*144),correlation=float(np.corrcoef(r,b)[0,1]))
results=[]
for act in [.1,.2,.3]:
 for dist in [.05,.1,.15]:
  for mode in ['coin','portfolio']:
   row=dict(activation=act,distance=dist,mode=mode,paths={})
   for path in ['pess','opt']:
    key=f's1_p0_{act}_{dist}_{mode}_{path}';v=np.load(S/(key+'.npz'));ss=stats(v['r'],br);ss.update(bankruptcy_t=json.loads((S/(key+'.json')).read_text()).get('bankruptcy_t'),annual_cost=float(v['cost'].mean()*365),turnover=float(v['turn'].mean()),exits=len(json.loads((S/(key+'.json')).read_text())['events']))
    ps=[]
    for phase in range(3):
     r=np.load(S/f's3_p{phase}_{act}_{dist}_{mode}_{path}.npz')['r'];b=np.load(S/f'control_s3_p{phase}.npz')['r'];ps.append(stats(r,b))
    ss['phases']=ps;ss['phase_mean_delta']=float(np.mean([x['delta'] for x in ps]));ss['phase_sd_delta']=float(np.std([x['delta'] for x in ps],ddof=1));ss['phase_mean_sharpe']=float(np.mean([x['sharpe'] for x in ps]));ss['phase_sd_sharpe']=float(np.std([x['sharpe'] for x in ps],ddof=1))
    gates=dict(fwer=ss['p_fwer']<.05 and all(x['p_fwer']<.05 for x in ps),subperiods=all(x['sharpe']>0 and x['delta']>0 for x in ss['sub']) and all(all(z['sharpe']>0 and z['delta']>0 for z in x['sub']) for x in ps),phase=ss['delta']>ss['phase_sd_delta'] and ss['phase_mean_delta']>ss['phase_sd_delta'],positive=ss['delta']>0)
    ss['gates']=gates;ss['pass']=all(gates.values());row['paths'][path]=ss
   row['usable']=row['paths']['pess']['pass'] and row['paths']['opt']['pass'];results.append(row);(S/'results.json').write_text(json.dumps(results,indent=2));print(act,dist,mode,flush=True)
(S/'controls.json').write_text(json.dumps([dict(phase=p,sharpe=float(shar(np.load(S/f'control_s3_p{p}.npz')['r']))) for p in range(3)],indent=2))
