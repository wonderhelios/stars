"""Paired block resampling, recompute nonlinear Sharpe, single global maxT family."""
import os,json,math,numpy as np
OUT=os.path.dirname(__file__);z=np.load(OUT+'/hourly_returns.npz');res=json.load(open(OUT+'/results.json'));B=9999
fees=['0.00075','0.00105','0.00145'];names=['baseline','always_twice']+[f'net{t}{suffix}' for suffix in ['', '_lev85'] for t in [2,3,5,8]]
keys=[f+'|'+name for f in fees for name in names];R=np.concatenate([z[key] for key in keys],axis=0).T;N=R.shape[0];mu=R.mean(0);sd=R.std(0,ddof=1)
observed=(np.sqrt(365)*mu/sd).reshape(30,24).mean(1)
labels=[];ab=[]
for fi,f in enumerate(fees):
 for ni,name in enumerate(names):
  if ni==0:continue
  for comp in ([0,1] if ni>1 else [0]):labels.append(f+'|'+name+'|'+names[comp]);ab.append((fi*10+ni,fi*10+comp))
a,b=np.array(ab).T;ob=observed[a]-observed[b];out={}
for L in [5,10,20,30]:
 rng=np.random.default_rng(20261008+L);vals=[]
 for start in range(0,B,250):
  count=np.zeros((min(250,B-start),N));pos=0
  for nn in [52,41]:
   st=rng.integers(nn,size=(len(count),math.ceil(nn/L)));ind=((st[:,:,None]+np.arange(L))%nn).reshape(len(count),-1)[:,:nn]+pos
   for j in range(len(count)):count[j]+=np.bincount(ind[j],minlength=N)
   pos+=nn
  m=count@R/N;v=(count@(R*R)-N*m*m)/(N-1);s=np.sqrt(365)*m/np.sqrt(np.maximum(v,1e-18));s=s.reshape(len(count),30,24).mean(2);vals.append(s[:,a]-s[:,b])
 vals=np.concatenate(vals);centered=vals-ob;se=vals.std(0,ddof=1);mt=(centered/np.maximum(se,1e-12)).max(1)
 out[str(L)]={key:dict(delta_sharpe=float(ob[j]),se=float(se[j]),ci95=(ob[j]-np.quantile(centered[:,j],[.975,.025])).tolist(),p_fwer=float((1+(mt>=ob[j]/max(se[j],1e-12)).sum())/(B+1))) for j,key in enumerate(labels)}
 print('nonlinear maxT',L,min(v['p_fwer'] for v in out[str(L)].values()),flush=True)
res['bootstrap_influence_sensitivity']=res['bootstrap'];res['bootstrap']=out;res['meta']['bootstrap']='9999 paired circular moving blocks independently within 52/41-day observed segments; recomputed mean of 24 phase Sharpes; centered delta / bootstrap SE; one-sided maxT across 51 hypotheses; blocks5/10/20/30; seed20261008+block'
json.dump(res,open(OUT+'/results.json','w'),indent=2)
