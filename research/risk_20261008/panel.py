"""Fixed four hypotheses. No parameter/direction selection. Run: python test_candidates.py"""
import os,json,glob,hashlib,warnings
import numpy as np,pandas as pd
warnings.filterwarnings('ignore',category=RuntimeWarning)
OUT=os.path.dirname(os.path.abspath(__file__)); DAY=86400000
FILES=sorted(glob.glob('/tmp/hl-daily-full/*.json'))
raw={os.path.basename(f)[:-5]:json.load(open(f)) for f in FILES}
names=sorted(raw); times=sorted({int(x['t']) for d in raw.values() for x in d if float(x['c'])>0})
dates=pd.to_datetime(times,unit='ms',utc=True); ti={t:i for i,t in enumerate(times)}; n=len(times); k=len(names)
a={z:np.full((n,k),np.nan) for z in ['o','c','h','l','v','n']}
for j,name in enumerate(names):
 for x in raw[name]:
  if int(x['t']) not in ti or float(x['c'])<=0:continue
  i=ti[int(x['t'])]
  for z in a:a[z][i,j]=float(x.get(z,np.nan))
P=a['c']; Q=P*a['v']; R=P/np.roll(P,1,axis=0)-1;R[0]=np.nan
# Exactly excludes today from liquidity mean, missing observations excluded.
V=pd.DataFrame(Q).rolling(30,min_periods=5).mean().shift(1).values
SD=pd.DataFrame(R).rolling(20,min_periods=5).std(ddof=1).values
M=P/np.roll(P,14,axis=0)-1
shock=Q/V
ill=pd.DataFrame(np.abs(R)/np.where(Q>0,Q,np.nan)).rolling(30,min_periods=20).mean().values
# Downside share: holds total volatility approximately separate from downside asymmetry.
down=pd.DataFrame(np.minimum(R,0)**2).rolling(30,min_periods=20).sum().values
sq=pd.DataFrame(R**2).rolling(30,min_periods=20).sum().values
down=down/sq
clv=np.where(a['h']>a['l'],(2*P-a['h']-a['l'])/(a['h']-a['l']),0)
flow=pd.DataFrame(clv*Q).rolling(30,min_periods=20).sum().values/pd.DataFrame(Q).rolling(30,min_periods=20).sum().values
# Size conditional on volume: avg trade $ / its own previous 30d mean.
size=Q/np.where(a['n']>0,a['n'],np.nan)
large=pd.DataFrame(size).rolling(30,min_periods=20).mean().values
# Use cross-sectional average dollar trade size, not volume shock or price momentum.
scores={'illiquidity':ill,'downside_share':down,'close_pressure':flow,'trade_size':large}
slots=np.array([sum(c.encode())%3 for c in names])

def book(idx,s,cap):
 t=np.zeros(k); idx=idx[np.isfinite(s[idx])]
 if len(idx)<8:return t
 kk=min(max(1,int(np.floor(len(idx)*.2+.5))),cap,len(idx)//2)
 order=idx[np.lexsort((np.array(names)[idx],s[idx]))]
 t[order[:kk]]=-.5/kk;t[order[-kk:]]=.5/kk
 return t

def targets(cap=5):
 base=np.zeros((n,k)); books={z:np.zeros((n,k)) for z in scores}; univ=[]
 for i in range(32,n-1):
  ok=(V[i]>=5e6)&np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])
  idx=np.flatnonzero(ok);univ.append(len(idx))
  if len(idx)<8:continue
  t=sum(book(idx,s,cap) for s in [M[i]/np.maximum(SD[i],1e-9),-SD[i],shock[i]])/3
  if np.abs(t).sum()>1e-12:t/=np.abs(t).sum()
  base[i]=t
  for z,s in scores.items():books[z][i]=book(idx,s[i],cap)
 return base,books,univ

