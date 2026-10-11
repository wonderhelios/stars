"""Strictly feasible logarithmic-barrier convex QP, NumPy-only.
min x'Hx subject to Gx+b>=0, Ex=h. Equality feasible start required.
"""
import numpy as np

def solve(H,G,b,E,h,x):
 x=x.copy();nit=0
 for mu in 10.**np.arange(-1,-12,-1):
  for _ in range(100):
   slack=G@x+b
   if np.any(slack<=0):return x,False,nit
   grad=2*H@x-mu*(G.T@(1/slack))
   hes=2*H+mu*((G.T/(slack*slack))@G)
   K=np.block([[hes,E.T],[E,np.zeros((len(h),len(h)))]])
   try:dx=np.linalg.solve(K,np.r_[-grad,h-E@x])[:len(x)]
   except np.linalg.LinAlgError:return x,False,nit
   nit+=1
   dec=-grad@dx
   if dec<max(1e-13,mu*.01):break
   ds=G@dx;negative=ds<0
   step=min(1.,float(np.min(-.99*slack[negative]/ds[negative]))) if negative.any() else 1.
   obj=x@H@x-mu*np.log(slack).sum()
   for back in range(60):
    xx=x+step*dx;ss=G@xx+b
    if (ss>0).all() and xx@H@xx-mu*np.log(ss).sum()<=obj+.01*step*(grad@dx):break
    step*=.5
   else:return x,False,nit
   x=xx
  else:return x,False,nit
 return x,True,nit
