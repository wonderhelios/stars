"""AST-extracted frozen engine. Never import/execute baseline's run-at-import footer."""
import ast, hashlib, json, os, sys
from pathlib import Path
import numpy as np
import pandas as pd
ROOT=Path.cwd()
assert ROOT==Path('/Users/wonder/Code/stars')
OUT=ROOT/'research/k12_validation_20261010'
SRC=ROOT/'research/trailing_tp_20261008/baseline.py'
source=SRC.read_text();tree=ast.parse(source)
cutoff=next(x.lineno for x in tree.body if isinstance(x,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='out' for t in x.targets))
g={'__file__':str(SRC),'__name__':'frozen_engine'}
exec(compile('\n'.join(source.splitlines()[:cutoff-1]),str(SRC),'exec'),g)
g['OUT']=OUT
original_target=g['target'];original_run=g['run']
runsource=ast.get_source_segment(source,next(x for x in tree.body if isinstance(x,ast.FunctionDef) and x.name=='run'))
n,k=g['n'],g['k']; names=np.array(g['names']);P,V,SD,M,Q,R=[g[z] for z in ['P','V','SD','M','Q','R']]
N=np.full((n,k),np.nan)
for j,c in enumerate(names):
 for bar in g['raw'][c]:N[g['ti'][bar['t']],j]=float(bar.get('n',np.nan))
# Missing n is allowed only when absent in original bar; v must always be positive.
badtrade=~(g['a']['v']>0)|(np.isfinite(N)&(N<=0))
elig=[np.flatnonzero((V[i]>=5e6)&np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])&np.array([':' not in c for c in names])) if i>=32 else np.array([],int) for i in range(n)]
orders={(i,f):idx[np.lexsort((names[idx],s[idx]))] for i,idx in enumerate(elig) if len(idx)>=8 for f,s in enumerate([M[i]/np.maximum(SD[i],1e-9),-SD[i],Q[i]/V[i]])}

def savejson(path,obj):
 p=Path(path);tmp=p.with_suffix(p.suffix+'.tmp');tmp.write_text(json.dumps(obj,indent=2,ensure_ascii=False,default=lambda x:x.item() if isinstance(x,np.generic) else str(x)));tmp.replace(p)
def savenpz(path,**obj):
 p=Path(path);tmp=p.with_suffix('.tmp.npz');np.savez_compressed(tmp,**obj);tmp.replace(p)
def freeze():
 paths=[SRC,ROOT/'docs/validation-protocol.md',ROOT/'research/sharpe_20261010/REPORT.md',SRC.parent/'retrieval/request.json',SRC.parent/'retrieval/meta.json']+sorted(Path('/tmp/hl-daily-v2').glob('*.json'))+sorted((ROOT/'src').rglob('*'))
 hashes={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in paths if p.is_file()}
 p=OUT/'input_manifest.json'
 if p.exists():assert json.loads(p.read_text())==hashes,'Frozen inputs changed'
 else:savejson(p,hashes)
 assert k==178
 savejson(OUT/'data_scope.json',dict(python=sys.executable,files=k,loaded_days=n,loaded_start=str(pd.to_datetime(g['ts'][0],unit='ms',utc=True)),loaded_end=str(pd.to_datetime(g['ts'][-1],unit='ms',utc=True)),return_start=str(pd.to_datetime(g['ts'][33],unit='ms',utc=True)),return_end=str(pd.to_datetime(g['ts'][-1],unit='ms',utc=True))))

class Target:
 def __init__(self,name,m=1.):self.name=name;self.m=m;self.info={}
 def __call__(self,i,equity,cap=5):
  idx=elig[i];w=np.zeros(k);kk=0
  if len(idx)>=8:
   kk=min(12,len(idx)//2) if self.name=='K12' else min(max(1,int(np.floor(len(idx)*.2+.5))),min(cap,max(1,int(np.floor(equity*2.7/15))//4)),len(idx)//2)
   for f in range(3):
    order=orders[i,f];w[order[:kk]]-=.5/kk/3;w[order[-kk:]]+=.5/kk/3
  raw=w.copy();normalizer=float(abs(w).sum());w=w/normalizer if normalizer>0 else w
  if self.name=='B0':assert np.array_equal(w,original_target(i,equity,cap))
  if self.name=='A':w=raw
  elif self.name=='A_const':w=w*self.m
  elif self.name in ['B','B_sign','C','C_diag']:
   assert cap==5 and equity*2.7/15>=20,'Equity-cap fallback required'
   w=EXTRA[self.name][i].copy()
  self.info=dict(g=normalizer,kk=kk,universe=len(idx),raw=raw,normalized=raw/normalizer if normalizer else raw)
  return w
EXTRA={}

def key(name,p,fee,delay=0):return f'{name}_p{p}_f{fee}_d{delay}'
def engine(name,p,fee,delay=0,m=1.,diagnostics=True):
 target=Target(name,m);records=[];quant=[];deltas=[];targets=[];raws=[];norms=[]
 def record(d):
  i=d['i'];qty=d['qty'];dq=d['delta'];mark=d['mark'];eq=d['eq'];cost=d['cost'];post=eq-cost;q=qty+dq
  dollar=np.nan_to_num(q*mark);gross=float(abs(dollar).sum()/post);grosspre=float(np.nansum(abs(qty*mark))/eq)
  frac=abs(dollar)/abs(dollar).sum() if abs(dollar).sum()>0 else np.zeros(k)
  records.append(dict(date=str(pd.to_datetime(g['ts'][i],unit='ms',utc=True).date()),equity_before=d['before'],equity_premark=eq,equity=post,price_pnl=d['gain'],cost=cost,turn=d['tr'],gross_pre=grosspre,gross=gross,target_gross=float(abs(d['w']).sum()*2.7),neff=float(1/(frac@frac)) if frac.any() else np.nan,g=target.info['g'],kk=target.info['kk'],universe=target.info['universe'],held_badbar=int(((abs(qty)>1e-12)&badtrade[i]).sum()),order_badbar=int(((abs(dq)>1e-12)&badtrade[i]).sum()),order_badbar_dollars=float(np.sum(abs(dq[badtrade[i]])*np.nan_to_num(d['px'][badtrade[i]]))),missing_held=int(((abs(qty)>1e-12)&~np.isfinite(d['px'])).sum())))
  quant.append(q.copy());deltas.append(dq.copy());targets.append(d['w'].copy());raws.append(target.info['raw']);norms.append(target.info['normalized'])
 g['target']=target;g['record']=record
 s=runsource.replace('range(33,n)',f'range({33+p},n)').replace("i-1 if clock=='open' else i",f"i-1-{delay} if clock=='open' else i")
 if diagnostics:s=s.replace('  eq-=cost;','  record(locals())\n  eq-=cost;')
 exec(compile(s,'<frozen_run_instrumented>','exec'),g);res=g['run'](fee=fee)
 temp=OUT/f'baseline_cap5_open_{fee}.npz';data=dict(np.load(temp));temp.unlink()
 if diagnostics:
  df=pd.DataFrame(records);data.update({c:df[c].to_numpy() for c in df.columns if c!='date'})
  data.update(qty=np.array(quant),delta_qty=np.array(deltas),target=np.array(targets),raw_target=np.array(raws),normalized_target=np.array(norms))
  res.update(m=m,mean_gross=float(df.gross.mean()),peak_gross=float(df.gross.max()))
 return res,data

def run_save(name,p,fee,delay=0,m=1.):
 label=key(name,p,fee,delay);jp=OUT/f'{label}.json';npz=OUT/f'{label}.npz'
 if jp.exists() and npz.exists():return json.loads(jp.read_text()),dict(np.load(npz))
 res,data=engine(name,p,fee,delay,m);savenpz(npz,**data)
 pd.DataFrame({x:data[x] for x in data if data[x].ndim==1}).to_csv(OUT/f'{label}_daily.csv',index=False)
 savejson(jp,res);print(label,'S',res['full']['sharpe'],'gross',res['mean_gross'],flush=True)
 return res,data

def match_const(p,fee,delay):
 label=key('A_const',p,fee,delay)
 if (OUT/f'{label}.json').exists():return run_save('A_const',p,fee,delay)
 ar,_=run_save('A',p,fee,delay);br,_=run_save('B0',p,fee,delay)
 m=ar['mean_gross']/br['mean_gross'];history=[]
 for attempt in range(20):
  res,data=engine('A_const',p,fee,delay,m);err=res['mean_gross']/ar['mean_gross']-1
  history.append(dict(m=m,mean_gross=res['mean_gross'],relative_error=err))
  if abs(err)<=.0001:break
  m/=1+err
 assert abs(err)<=.005,'Gross match failed'
 res.update(match_error=err,match_target=ar['mean_gross'],match_history=history)
 savenpz(OUT/f'{label}.npz',**data);pd.DataFrame({x:data[x] for x in data if data[x].ndim==1}).to_csv(OUT/f'{label}_daily.csv',index=False);savejson(OUT/f'{label}.json',res)
 print(label,'m',m,'err',err,'S',res['full']['sharpe'],flush=True)
 return res,data

def main():
 freeze()
 for delay,fees in [(0,[.0007,.00075,.00105,.00045,.00015]),(1,[.0007,.00075,.00105])]:
  for fee in fees:
   for p in range(3):
    for name in ['B0','K12','A']:run_save(name,p,fee,delay)
    match_const(p,fee,delay)
 if (OUT/'extra_targets.npz').exists():
  EXTRA.update(dict(np.load(OUT/'extra_targets.npz')))
  for name in ['B','B_sign','C','C_diag']:
   for p in range(3):run_save(name,p,.0007)
if __name__=='__main__':main()
