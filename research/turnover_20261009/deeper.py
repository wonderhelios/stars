import json,itertools,math
import numpy as np,pandas as pd
import study as s
original=s.record
extra=[]
def record(d):
 original(d)
 i=d['i'];px=np.nan_to_num(d['px']);qty=d['qty'];eq=d['eq'];dollars=abs(d['delta'])*px/eq
 gn=s.norm[i-1] or 1;go=s.norm[i-2] or 1
 changes=(s.sleeves[i-1]/gn-s.sleeves[i-2]/go)*2.7
 drift=2.7*s.wt[i-2]-qty*px/eq;players=np.vstack([changes,drift]);denom=abs(players.sum(0));scale=np.divide(dollars,denom,out=np.zeros(s.k),where=denom>1e-15)
 values={}
 for bits in itertools.product([0,1],repeat=4):
  subset=tuple(j for j in range(4) if bits[j]);values[subset]=abs(players[list(subset)].sum(0)) if subset else np.zeros(s.k)
 alloc=[]
 for j in range(4):
  others=[x for x in range(4) if x!=j];v=np.zeros(s.k)
  for z in range(4):
   for sub in itertools.combinations(others,z):v+=math.factorial(z)*math.factorial(3-z)/24*(values[tuple(sorted(sub+(j,)))]-values[sub])
  alloc.append(float(np.sum(v*scale)))
 changed=s.elig[i-1]!=s.elig[i-2]
 extra.append(dict(zip(['exec_momentum','exec_lowvol','exec_volume','exec_drift'],alloc))|dict(eligibility_changed_turn=float(dollars[changed].sum()),eligibility_entries=int((s.elig[i-1]&~s.elig[i-2]).sum()),eligibility_exits=int((~s.elig[i-1]&s.elig[i-2]).sum()),k_changed=int(s.kk[i-1]!=s.kk[i-2]),**{f'rank_{f}':float(np.nanmean(abs(s.ranks[i-1,f]-s.ranks[i-2,f]))) for f in range(3)}))
s.record=record
res,data=s.engine(s.configs[0],0,.0007,diag=True)
d=pd.read_csv(s.OUT/'daily_analysis.csv');e=pd.DataFrame(extra);d=pd.concat([d,e],axis=1)
assert np.allclose(e[['exec_momentum','exec_lowvol','exec_volume','exec_drift']].sum(1),d.turn)
d.to_csv(s.OUT/'daily_analysis.csv',index=False)
d.groupby('period').mean(numeric_only=True).to_csv(s.OUT/'deep_summary.csv')
# Actual nonlinear paired Sharpe-difference bootstrap as an audit of influence-function inference.
common=data['t'][2:];returns=[]
for c in s.configs:
 rr=[]
 for p in range(3):
  a=np.load(s.OUT/f"{c['name']}_p{p}_f0.0007.npz");rr.append(a['r'][np.isin(a['t'],common)])
 returns.append(np.array(rr))
r=np.array(returns);sh=lambda x:np.mean(x,axis=-1)/np.std(x,axis=-1,ddof=1)*np.sqrt(365)
obs=sh(r).mean(1)[1:]-sh(r).mean(1)[0];T=r.shape[2];B=4999;L=20;rng=np.random.default_rng(20261029);boot=[]
for start in range(0,B,100):
 nb=min(100,B-start);st=rng.integers(T,size=(nb,int(np.ceil(T/L))));idx=((st[:,:,None]+np.arange(L))%T).reshape(nb,-1)[:,:T];v=sh(r[:,:,idx]).mean(1);boot.append((v[1:]-v[0]).T)
boot=np.concatenate(boot);null=boot-obs;se=null.std(0,ddof=1);mx=(null/se).max(1);p=(1+(mx[:,None]>=obs/se).sum(0))/(B+1)
audit={c['name']:dict(delta=float(obs[j]),p_fwer=float(p[j]),ci95=np.quantile(boot[:,j],[.025,.975]).tolist()) for j,c in enumerate(s.configs[1:])};(s.OUT/'nonlinear_bootstrap_audit.json').write_text(json.dumps(audit,indent=2));print(json.dumps(audit,indent=2))
