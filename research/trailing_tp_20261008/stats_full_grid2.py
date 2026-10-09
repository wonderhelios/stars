import trailing_grid_full as g
import numpy as np,json
S=g.SAVE;N=len(g.ts)-33;B=9999;rng=np.random.default_rng(20261009);block=30
starts=rng.integers(0,N,size=(B,(N+block-1)//block));ix=((starts[:,:,None]+np.arange(block))%N).reshape(B,-1)[:,:N]
def sh(r,axis=-1):return r.mean(axis=axis)/r.std(axis=axis,ddof=1)*np.sqrt(365)
D=g.D
# Baseline numbers from existing baseline_results.json; scalar reference only, no rerun.
ref=json.loads((S/'original_reference.json').read_text())
controls=[np.load(S/f'control_s3_p{p}.npz')['r'] for p in range(3)]
results=[]
for act in [.1,.2,.3]:
 for dist in [.05,.1,.15]:
  for mode in ['coin','portfolio']:
   row=dict(activation=act,distance=dist,mode=mode,paths={})
   for path in ['pess','opt']:
    key=f's1_p0_{act}_{dist}_{mode}_{path}';v=np.load(S/(key+'.npz'));r=v['r'];dd=json.loads((S/(key+'.json')).read_text());ss=dict(sharpe=float(sh(r)),delta=float(sh(r)-ref['full']['sharpe']),sub=[dict(sharpe=float(sh(r[m])),delta=float(sh(r[m])-ref[label]['sharpe'])) for m,label in [(D.year<=2024,'2023-24'),(D.year>=2025,'2025-26')]],p=None,p_fwer=None,annual_cost=float(v['cost'].mean()*365),turnover=float(v['turn'].mean()),exits=len(dd['events']),bankruptcy_t=dd['bankruptcy_t'],phases=[])
    for phase,b in enumerate(controls):
     v3=np.load(S/f's3_p{phase}_{act}_{dist}_{mode}_{path}.npz');r3=v3['r'];delta=float(sh(r3)-sh(b));count=0
     for st in range(0,B,100):
      ii=ix[st:st+100];db=sh(r3[ii])-sh(b[ii]);count+=int(np.sum(db-delta>=delta))
     p=(count+1)/(B+1);ss['phases'].append(dict(sharpe=float(sh(r3)),delta=delta,p=p,p_fwer=min(1,p*288),correlation=float(np.corrcoef(r3,b)[0,1]),annual_cost=float(v3['cost'].mean()*365),turnover=float(v3['turn'].mean()),sub=[dict(sharpe=float(sh(r3[m])),delta=float(sh(r3[m])-sh(b[m]))) for m in [D.year<=2024,D.year>=2025]]))
    for metric in ['sharpe','delta','correlation','annual_cost','turnover']:
     ss['phase_mean_'+metric]=float(np.mean([z[metric] for z in ss['phases']]));ss['phase_sd_'+metric]=float(np.std([z[metric] for z in ss['phases']],ddof=1))
    ss['phase_p_max']=max(z['p'] for z in ss['phases']);ss['phase_p_fwer_max']=max(z['p_fwer'] for z in ss['phases']);row['paths'][path]=ss
   results.append(row);(S/'results.json').write_text(json.dumps(results,indent=2));print(act,dist,mode,flush=True)
(S/'controls_summary.json').write_text(json.dumps([dict(phase=p,sharpe=float(sh(b)),sub=[float(sh(b[m])) for m in [D.year<=2024,D.year>=2025]]) for p,b in enumerate(controls)],indent=2))
