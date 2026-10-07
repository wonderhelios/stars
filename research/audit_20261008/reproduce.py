from pathlib import Path
import sys,types,json,hashlib,warnings
import numpy as np
warnings.filterwarnings('ignore')
ROOT=Path(__file__).resolve().parents[2]; OUT=Path(__file__).resolve().parent
results={}
def load_engine(d):
 p=ROOT/'research'/d/'engine.py';m=types.ModuleType('engine');m.__file__=str(p);exec(compile(p.read_text(),str(p),'exec'),m.__dict__);sys.modules['engine']=m;return m
def sharp(r):return np.mean(r,axis=-1)/np.std(r,axis=-1,ddof=1)*np.sqrt(365)
for d,script,cut in [('marginal_20261008','test_candidates.py','# Basic implementation checks'),('distribution_stagger_20261008','run.py','upper={}'),('liq_oi_20261008','validate.py','# Centered bootstrap'),('leadlag_20261008','research.py',"# Common evaluation")]:
 p=ROOT/'research'/d/script;ns={'__file__':str(p)}
 if d!='marginal_20261008':e=load_engine(d)
 exec(compile(p.read_text().split(cut)[0],str(p),'exec'),ns)
 if d=='leadlag_20261008':
  br=[e.run(ns['base'],o) for o in range(3)]; rr={'baseline':br}
  for z,t in ns['C'].items():
   mix=.8*ns['base']+.2*t;g=np.abs(mix).sum(1);mix=np.divide(mix,g[:,None],out=np.zeros_like(mix),where=g[:,None]>0);rr[z]=[e.run(mix,o) for o in range(3)]
 else:rr=ns['runs']
 res={z:sharp(np.array([v['r'] for v in vs])).tolist() for z,vs in rr.items()}
 results[d]={'phase_sharpes':res,'means':{z:float(np.mean(s)) for z,s in res.items()}}
 if d=='distribution_stagger_20261008':
  results[d]['rollback1']=sharp(np.array([e.run(ns['base'],0,period=1)['r']])).tolist()
  results[d]['counterfactuals']={}
  for cap in [5,8]:
   b,books,_=e.targets(cap);t=.8*b+.2*books['low_kurt'];g=np.abs(t).sum(1);t=np.divide(t,g[:,None],out=np.zeros_like(t),where=g[:,None]>0)
   for per in [1,2,3,4]:
    results[d]['counterfactuals'][f'cap{cap}_period{per}']={z:float(np.mean([sharp(e.run(T,o,period=per)['r']) for o in range(per)])) for z,T in [('baseline',b),('kurt_mix',t)]}
 if d=='marginal_20261008':
  results[d]['year_counterfactual']={}
  for year in [2023,2024,2025,2026]:
   m=ns['outdates'].year==year;results[d]['year_counterfactual'][str(year)]={z:float(np.mean([sharp(v['r'][m]) for v in rr[z]])) for z in ['baseline','blend_close_pressure']}
 print(d,results[d]['means'].get('baseline'),flush=True)
# Risk prefix contains one write; explicitly remove it and stop before output writes.
d='risk_20261008';p=ROOT/'research'/d/'run.py';src=p.read_text().split('for z,ss in summary.items():')[0];src=src.replace("json.dump(configs,open(OUT/'candidates.json','w'),indent=2)",'pass');ns={'__file__':str(p)};exec(compile(src,str(p),'exec'),ns)
results[d]={'means':{z:q['full']['sharpe'] for z,q in ns['summary'].items()},'matched':{z:q['full']['sharpe'] for z,q in ns['matched'].items()}}
# Independent recomputation from saved return arrays, across periods and all candidates.
checks={}
for d,file in [('distribution_stagger_20261008','returns.npz'),('leadlag_20261008','returns.npz'),('risk_20261008','paths.npz')]:
 f=ROOT/'research'/d/file;a=np.load(f);checks[d]={'keys':a.files,'recomputed_means':{z:float(np.mean(sharp(a[z]))) for z in a.files if z!='dates' and np.issubdtype(a[z].dtype,np.number) and a[z].ndim==2}}
results['saved_return_checks']=checks
# Input and reviewed source fingerprint, never print secrets/state.
results['source_sha256']={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((ROOT/'src').glob('*.rs'))}
(OUT/'reproduction.json').write_text(json.dumps(results,indent=2))
