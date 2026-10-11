"""Original registered nuisance paths, solely to preserve the maxT family."""
import sys
from pathlib import Path
from qp import solve
from engine import *

def main():
 freeze();dest=OUT/'extra_targets.npz';checkpoint=OUT/'extra_targets_checkpoint.npz';logs=[];start=0
 arr={x:np.zeros((n,k)) for x in ['B','B_sign','C','C_diag']}
 if dest.exists():return
 if checkpoint.exists():
  d=np.load(checkpoint);start=int(d['done']);arr={x:d[x] for x in arr};logs=json.loads((OUT/'extra_target_audit.json').read_text())['days']
 for i in range(start,n):
  idx=elig[i];w0=original_target(i,1e6);row=dict(i=i,date=str(pd.to_datetime(g['ts'][i],unit='ms',utc=True).date()),active=bool(abs(w0).sum()>0))
  if len(idx)>=8:
   scores=[M[i]/np.maximum(SD[i],1e-9),-SD[i],Q[i]/V[i]]
   if not all(np.isfinite(s[idx]).all() for s in scores):
    arr['B'][i]=w0;arr['B_sign'][i]=w0;row['B']='nonfinite_fallback'
   else:
    for name in ['B','B_sign']:
     w=np.zeros(k)
     for s in scores:
      z=2*(pd.Series(s[idx]).rank(method='average').to_numpy()-1)/(len(idx)-1)-1
      if name=='B_sign':
       pos=z>0;neg=z<0
       z=np.where(pos,.5/max(1,pos.sum()),np.where(neg,-.5/max(1,neg.sum()),0)) if pos.any() and neg.any() else np.zeros(len(idx))
      gross=abs(z).sum();w[idx]+=z/gross/3 if gross else z
     gross=abs(w).sum();arr[name][i]=w/gross if gross else w
    row['B']='active'
  J=np.flatnonzero(abs(w0)>1e-12);valid=bool(len(J) and i>=60 and np.all(np.diff(np.array(g['ts'][i-60:i+1]))==86400000) and np.isfinite(R[i-59:i+1,J]).all() and not badtrade[i-60:i+1,J].any())
  for name in ['C','C_diag']:
   arr[name][i]=w0
   if not valid:row[name]='window_fallback' if len(J) else 'zero_target';continue
   X=R[i-59:i+1,J];S=np.cov(X,rowvar=False,ddof=1);eps=max(1e-12,1e-6*np.trace(S)/len(J))
   cov=(.5*S+.5*np.diag(np.diag(S)) if name=='C' else np.diag(np.diag(S)))+eps*np.eye(len(J))
   s=np.sign(w0[J]);a0=abs(w0[J]);H=cov*np.outer(s,s);h=H/max(np.trace(H)/len(J),1e-12);q=len(J)
   E=np.zeros((2,2*q));E[0,:q]=(s>0);E[1,:q]=(s<0)
   A=np.zeros((2*q+1,2*q));A[:q,:q]=-np.eye(q);A[:q,q:]=np.eye(q);A[q:2*q,:q]=np.eye(q);A[q:2*q,q:]=np.eye(q);A[-1,q:]=-1
   b=np.r_[a0,-a0,.20]
   G=np.vstack([A,np.c_[np.eye(q),np.zeros((q,q))],np.c_[-np.eye(q),np.zeros((q,q))]])
   bb=np.r_[b,-.5*a0,1.5*a0];HH=np.zeros((2*q,2*q));HH[:q,:q]=h
   xx,success,nit=solve(HH,G,bb,E,np.array([.5,.5]),np.r_[a0,np.full(q,.10/q)])
   aa=xx[:q];res=max(float(abs(E@xx-.5).max()),float(np.maximum(.5*a0-aa,0).max()),float(np.maximum(aa-1.5*a0,0).max()),float(max(0,abs(aa-a0).sum()-.20)),float(max(0,aa@H@aa-a0@H@a0)))
   row[name+'_residual']=res;row[name+'_solver']=str(nit)+' Newton iterations'
   if success and res<=1e-8:
    arr[name][i,J]=s*aa;row[name]='active';row[name+'_variance0']=float(a0@H@a0);row[name+'_variance']=float(aa@H@aa)
   else:row[name]='solver_fallback'
  logs.append(row)
  if (i+1)%25==0 or i==n-1:
   savejson(OUT/'extra_target_audit.json',dict(solver='NumPy logarithmic barrier QP v1',days=logs));savenpz(checkpoint,done=i+1,**arr);print('extra target checkpoint',i+1,n,flush=True)
 savenpz(dest,**arr)
if __name__=='__main__':main()
