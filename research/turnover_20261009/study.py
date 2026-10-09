import ast, json, hashlib, sys
from pathlib import Path
import numpy as np
import pandas as pd
ROOT=Path.cwd(); OUT=ROOT/'research/turnover_20261009'; SRC=ROOT/'research/trailing_tp_20261008/baseline.py'
source=SRC.read_text(); tree=ast.parse(source); cutoff=next(x.lineno for x in tree.body if isinstance(x,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='out' for t in x.targets))
g={'__file__':str(SRC),'__name__':'frozen_engine'}; exec(compile('\n'.join(source.splitlines()[:cutoff-1]),str(SRC),'exec'),g); g['OUT']=OUT
n,k=g['n'],g['k']; names=np.array(g['names']); P,V,SD,M,Q,R=[g[z] for z in ['P','V','SD','M','Q','R']]
elig=np.zeros((n,k),bool); ranks=np.full((n,3,k),np.nan); orders={}; kk=np.zeros(n,int); norm=np.zeros(n); wt=np.zeros((n,k)); sleeves=np.zeros((n,3,k)); market=np.full(n,np.nan)
for i in range(32,n):
 idx=np.flatnonzero((V[i]>=5e6)&np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])&np.array([':' not in c for c in names])); elig[i,idx]=1
 if len(idx):market[i]=np.nanmean(R[i,idx])
 if len(idx)<8:continue
 kk[i]=min(max(1,int(np.floor(len(idx)*.2+.5))),5,len(idx)//2)
 for f,s in enumerate([M[i]/np.maximum(SD[i],1e-9),-SD[i],Q[i]/V[i]]):
  order=idx[np.lexsort((names[idx],s[idx]))]; orders[i,f]=order; ranks[i,f,order]=np.arange(len(idx))/(len(idx)-1)
  sleeves[i,f,order[:kk[i]]]=-.5/kk[i]/3; sleeves[i,f,order[-kk[i]:]]=.5/kk[i]/3
 raw=sleeves[i].sum(0); norm[i]=abs(raw).sum();wt[i]=raw/norm[i] if norm[i]>0 else raw
 assert np.allclose(wt[i],g['target'](i,1e6))
vol=pd.Series(market).rolling(20,min_periods=20).std().values
threshold={q:pd.Series(vol).shift(1).rolling(252,min_periods=60).quantile(q).values for q in [.6,.75,.9]}
configs=[dict(name='baseline',kind='baseline')]+[dict(name=f'smooth_{a}',kind='smooth',alpha=a) for a in [.3,.5,.7]]+[dict(name=f'vol_q{q}_a{a}',kind='vol',q=q,alpha=a) for q in [.6,.75,.9] for a in [.3,.5]]+[dict(name=f'hyst_{q}',kind='hyst',q=q) for q in [.25,.3,.4]]
class Target:
 def __init__(self,c):self.c=c;self.prev=np.zeros(k);self.holds={}
 def __call__(self,i,equity,cap):
  raw=g['original_target'](i,equity,cap);c=self.c
  if c['kind']=='baseline':return raw
  # Preserve the original universe safety gate; smoothing must not renormalize stale holdings into exposure.
  if elig[i].sum()<8:
   self.prev=np.zeros(k);self.holds={};return raw
  if c['kind'] in ['smooth','vol']:
   alpha=c['alpha'] if c['kind']=='smooth' or (np.isfinite(threshold[c['q']][i]) and vol[i]>threshold[c['q']][i]) else 1.
   w=(1-alpha)*self.prev+alpha*raw; w[~elig[i]]=0
  else:
   w=np.zeros(k); count=int(elig[i].sum()); kval=min(int(kk[i]),min(cap,max(1,int(np.floor(equity*2.7/15))//4)))
   if count>=8:
    for f in range(3):
     for side in [-1,1]:
      key=(f,side);order=orders[i,f] if side==-1 else orders[i,f][::-1]; pos={int(j):p for p,j in enumerate(order)}
      retained=[j for j in self.holds.get(key,[]) if j in pos and pos[j]<max(kval,int(np.ceil(c['q']*count)))]; retained=sorted(retained,key=lambda j:pos[j])[:kval]
      retained+= [int(j) for j in order if j not in retained][:kval-len(retained)]; self.holds[key]=retained;w[retained]+=side*.5/kval/3
  gross=abs(w).sum();w=w/gross if gross else w;self.prev=w.copy();return w
g['original_target']=g['target']
run_node=next(x for x in tree.body if isinstance(x,ast.FunctionDef) and x.name=='run'); runsource=ast.get_source_segment(source,run_node)
rows=[];previous_w=np.zeros(k)
def record(d):
 global previous_w
 w=d['w']; px=d['px']; eq=d['eq']; qty=d['qty']; wanted=d['wanted'];delta=d['delta'];i=d['i']; dollars=abs(delta)*np.nan_to_num(px)/eq
 old=abs(previous_w)>1e-12; new=abs(w)>1e-12;flip=old&new&(previous_w*w<0);change=old&new&~flip&(abs(w-previous_w)>1e-12)
 cats={'entry':~old&new,'exit':old&~new,'target_flip':flip,'same_weight_change':change,'target_unchanged':~((~old&new)|(old&~new)|flip|change)}
 row={'date':pd.to_datetime(g['ts'][i],unit='ms',utc=True).strftime('%Y-%m-%d'),'equity':eq,'turn':dollars.sum(),'universe':elig[i-1].sum(),'kk':kk[i-1],'normalizer':norm[i-1],'active_names':new.sum(),'xs_vol':np.nanstd(R[i-1,elig[i-1]],ddof=1),'market_vol20':vol[i-1],'rank_distance':np.nanmean(abs(ranks[i-1]-ranks[i-2]))}
 row.update({key:float(dollars[mask].sum()) for key,mask in cats.items()})
 qold=abs(qty)>1e-12;qnew=abs(qty+delta)>1e-12
 row.update(actual_flip=float(dollars[(qty*(qty+delta))<0].sum()),actual_entry=float(dollars[~qold&qnew].sum()),actual_exit=float(dollars[qold&~qnew].sum()),actual_same=float(dollars[qold&qnew&(qty*(qty+delta)>0)].sum()))
 # Symmetric Shapley allocation of executed turnover to signal vs mark/equity drift. Signed contributions can be negative.
 s=2.7*(w-previous_w);b=2.7*previous_w-qty*np.nan_to_num(px)/eq; total=abs(s+b);signal=(abs(s)+total-abs(b))/2;frac=np.divide(dollars,total,out=np.zeros(k),where=total>1e-15)
 row['signal_shapley']=float(np.sum(signal*frac));row['drift_shapley']=float(np.sum((total-signal)*frac));row['ideal_signal_turn']=float(abs(s).sum());row['sleeve_signal_raw']=float(abs(sleeves[i-1]-sleeves[i-2]).sum()*2.7)
 f=np.array([10**g['meta'][c]['szDecimals'] for c in names]); desired=w*eq*2.7/px;desired=np.nan_to_num(desired);rounded=np.sign(desired)*np.floor(abs(desired)*f)/f
 row['small_target_n']=int(((abs(rounded)*np.nan_to_num(px)<10)&(abs(rounded)>0)).sum());row['small_target_turn']=float((abs(rounded)*np.nan_to_num(px)/eq)[(abs(rounded)*np.nan_to_num(px)<10)&(abs(rounded)>0)].sum())
 dd=abs(wanted-qty);rounded_d=np.floor(dd*f)/f*np.nan_to_num(px); blocked=(abs(delta)<1e-15)&(dd>1e-12)&np.isfinite(px);small=blocked&(rounded_d<10);band=blocked&~small&(rounded_d<abs(wanted)*np.nan_to_num(px)*.02)
 row['skip_min_n']=int(small.sum());row['skip_min_turn']=float((dd*np.nan_to_num(px)/eq)[small].sum());row['skip_band_n']=int(band.sum());row['skip_band_turn']=float((dd*np.nan_to_num(px)/eq)[band].sum())
 rows.append(row); previous_w=w.copy()
def engine(c,phase,fee,diag=False,no_min=False):
 g['target']=Target(c);g['record']=record
 s=runsource.replace('range(33,n)',f'range({33+phase},n)')
 if diag:s=s.replace('  cost=fee*','  record(locals())\n  cost=fee*')
 if no_min:s=s.replace('size*px[j]>=10','size*px[j]>=0').replace('abs(qty[j])*px[j]>=10','abs(qty[j])*px[j]>=0').replace('max(10,','max(0,')
 exec(compile(s,'<original_run_instrumented>','exec'),g);res=g['run'](fee=fee)
 p=OUT/f'baseline_cap5_open_{fee}.npz';data=dict(np.load(p));p.unlink()
 return res,data

def main():
 allres={}
 for c in configs:
  for phase in range(3):
   for fee in [.0007,.00045,.00015,.00075]:
    key=f"{c['name']}_p{phase}_f{fee}";path=OUT/f'{key}.npz';jp=OUT/f'{key}.json'
    if path.exists() and jp.exists():res=json.loads(jp.read_text())
    else:
     res,data=engine(c,phase,fee,diag=(c['name']=='baseline' and phase==0 and fee==.0007));np.savez_compressed(path,**data);jp.write_text(json.dumps(res,indent=2))
     if rows:pd.DataFrame(rows).to_csv(OUT/'diagnostics.csv',index=False)
    if c['name']=='baseline' and phase==0 and fee==.0007:
     assert abs(res['full']['sharpe']-1.1293)<.00005
     original=np.load(SRC.parent/'baseline_cap5_open_0.0007.npz');current=np.load(path);assert np.array_equal(original['r'],current['r']) and np.array_equal(original['turn'],current['turn'])
    allres[key]=res
   print(c['name'],phase,'checkpoint',flush=True)
  (OUT/'grid_results.json').write_text(json.dumps(allres,indent=2))
 res,data=engine(configs[0],0,.0007,no_min=True);np.savez_compressed(OUT/'no_min.npz',**data);(OUT/'no_min.json').write_text(json.dumps(res,indent=2))
 manifest={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in [SRC,ROOT/'docs/validation-protocol.md',OUT/'SPEC.md',Path(__file__)]+sorted(Path('/tmp/hl-daily-v2').glob('*.json'))};(OUT/'manifest.json').write_text(json.dumps(manifest,indent=2))
if __name__=='__main__':main()
