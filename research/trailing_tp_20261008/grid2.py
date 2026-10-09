"""Daily trailing scenarios; frozen baseline prefix, no baseline main execution."""
from pathlib import Path
src=Path(__file__).with_name('baseline.py').read_text()
exec(src.split('out=dict(config=')[0].split('def run(')[0])
for z in ['h','l']:
 a[z]=np.full((n,k),np.nan)
 for j,c in enumerate(names):
  for x in raw[c]:a[z][ti[x['t']],j]=float(x[z])
factors=np.array([10**meta[c]['szDecimals'] for c in names])
W=np.array([target(i,1e6) for i in range(n)])

def simulate(A=None,D=None,mode='coin',side='low',start=0,hourly=False):
 eq=1e6;qty=np.zeros(k);last=np.full(k,np.nan);entry=np.full(k,np.nan);peak=np.zeros(k);navpeak=eq;navref=eq
 rr=[];turn=[];costs=[];exits=0;missing=0
 for i in range(33+start,n):
  before=eq;px=a['o'][i];mark=np.where(np.isfinite(px),px,last)
  missing+=int(np.any((abs(qty)>1e-12)&~np.isfinite(px)))
  eq+=np.nansum(qty*(mark-last));w=W[i-1]
  wanted=np.sign(w)*np.floor(abs(w)*eq*2.7/px*factors)/factors
  wanted=np.where(np.isfinite(wanted)&(abs(wanted)*px>=10),wanted,0)
  d=wanted-qty;rounded=np.sign(d)*np.floor(abs(d)*factors)/factors
  delta=np.where((qty*wanted<0)|((wanted==0)&(abs(qty)*px>=10)),d,np.where((wanted!=0)&(abs(rounded)*px>=np.maximum(10,abs(wanted)*px*.02)),rounded,0))
  delta=np.where(np.isfinite(px),delta,0);nt=np.nansum(abs(delta)*px);cost=.0007*nt;eq-=cost
  old=qty.copy();qty+=delta
  reset=(old*qty<=0)&(abs(qty)>1e-12)
  add=(old*qty>0)&(abs(qty)>abs(old))
  entry[reset]=px[reset];peak[reset]=0
  oldpeakprice=entry*(1+np.sign(qty)*peak)
  entry[add]=(entry[add]*abs(old[add])+px[add]*abs(delta[add]))/abs(qty[add])
  peak[add]=np.maximum(0,np.sign(qty[add])*(oldpeakprice[add]/entry[add]-1))
  last=mark
  if A is not None:
   steps=hour_steps.get(ts[i],[]) if hourly else [(a['h'][i],a['l'][i],P[i])]
   for hi,lo,cl in steps:
    held=abs(qty)>1e-12
    if hourly and ts[i] in hour_steps:missing+=int(np.sum(held&~np.isfinite(cl)))
    closemark=np.where(np.isfinite(cl),cl,last)
    if mode=='coin':
     favorable=np.where(qty>0,hi,lo);adverse=np.where(qty>0,lo,hi) if side=='low' else cl
     priorpeak=peak.copy()
     profit=np.sign(qty)*(favorable/entry-1);peak=np.maximum(peak,np.nan_to_num(profit,nan=-np.inf))
     # A price retracement from the favorable extreme, symmetric for shorts.
     peakprice=entry*(1+np.sign(qty)*peak)
     hit=held&(peak>=A)&np.isfinite(adverse)&np.where(qty>0,adverse<=peakprice*(1-D),adverse>=peakprice*(1+D))
     stop=peakprice*np.where(qty>0,1-D,1+D)
     opening=last
     priorstop=entry*(1+np.sign(qty)*priorpeak)*np.where(qty>0,1-D,1+D)
     gap=(priorpeak>=A)&np.where(qty>0,opening<=priorstop,opening>=priorstop)
     execution=np.where(gap,opening,stop) if side=='low' else cl
     fill=np.where(hit,execution,closemark)
    else:
     # Only synchronized closes; individual extrema are never aggregated.
     nav=eq+np.nansum(qty*(closemark-last));navpeak=max(navpeak,nav)
     hit=held&np.isfinite(cl) if navpeak/navref-1>=A and nav/navpeak-1<=-D else np.zeros(k,bool)
     fill=closemark
    eq+=np.nansum(qty*(fill-last));last=closemark
    closing=np.nansum(abs(qty[hit])*fill[hit]);eq-=.0007*closing;cost+=.0007*closing;nt+=closing;exits+=int(hit.sum());qty[hit]=0;entry[hit]=np.nan;peak[hit]=0
    if mode=='portfolio' and hit.any() and not np.any(abs(qty)>1e-12):navref=eq;navpeak=eq
  # Control is also marked at close, retaining exactly the open rebalance rules.
  if A is None:
   cm=np.where(np.isfinite(P[i]),P[i],last);eq+=np.nansum(qty*(cm-last));last=cm
  if eq<=0:raise ValueError('insolvent')
  rr.append(eq/before-1);turn.append(nt/before);costs.append(cost/before)
 return np.array(rr),np.array(turn),np.array(costs),exits,missing

if __name__=='__main__':
 import hashlib
 dates=pd.to_datetime(ts[33:],unit='ms',utc=True);results=[]
 controls=[simulate(start=s)[0] for s in range(3)]
 np.savez_compressed(OUT/'grid2_controls.npz',**{f'r{s}':r for s,r in enumerate(controls)},t=ts[33:])
 rng=np.random.default_rng(20261009);B=4999;N=len(controls[0]);L=30
 # circular block bootstrap, same indices for every candidate; paired Sharpe difference.
 starts=rng.integers(0,N,(B,int(np.ceil(N/L))))
 ids=((starts[:,:,None]+np.arange(L))%N).reshape(B,-1)[:,:N]
 def stats(r,t,c,s):
  dd=dates[s:];base=controls[s]
  return dict(sharpe=sh(r),delta=sh(r)-sh(base),early=sh(r[dd.year<=2024]),late=sh(r[dd.year>=2025]),early_delta=sh(r[dd.year<=2024])-sh(base[dd.year<=2024]),late_delta=sh(r[dd.year>=2025])-sh(base[dd.year>=2025]),correlation=float(np.corrcoef(r,base)[0,1]),annual_cost=float(c.mean()*365),turnover=float(t.mean()))
 for mode in ['coin','portfolio']:
  for A in [.1,.2,.3]:
   for D in [.05,.1,.15]:
    row=dict(mode=mode,activation=A,distance=D,ends={})
    for side in ['low','close']:
     phases=[];pvals=[]
     for s in range(3):
      r,t,c,ex,mi=simulate(A,D,mode,side,s);st=stats(r,t,c,s);st.update(exits=ex,missing=mi);phases.append(st)
      np.savez_compressed(OUT/f'grid2_{mode}_{A}_{D}_{side}_{s}.npz',r=r,turn=t,cost=c,t=ts[33+s:])
      if True:
       base=controls[s];idx=ids%len(r);rb=r[idx];bb=base[idx]
       ds=rb.mean(1)/rb.std(1,ddof=1)*np.sqrt(365)-bb.mean(1)/bb.std(1,ddof=1)*np.sqrt(365)
       obs=st['delta'];p=float((1+np.sum(ds-obs>=obs))/(B+1)) if obs>0 else 1.
       st['p']=p;pvals.append(p)
     p=max(pvals)
     row['ends'][side]=dict(phases=phases,mean_sharpe=float(np.mean([p['sharpe'] for p in phases])),mean_delta=float(np.mean([p['delta'] for p in phases])),sd_delta=float(np.std([p['delta'] for p in phases],ddof=1)),p=p,p_fwer=min(1,p*18*3*2))
    results.append(row);(OUT/'grid2_results.json').write_text(json.dumps(results,indent=2));print(mode,A,D,row['ends']['low']['mean_delta'],flush=True)
 audit={}
 for c,d in raw.items():
  times=sorted(x['t'] for x in d);audit[c]=dict(count=len(times),gaps=sum(b-a!=86400000 for a,b in zip(times,times[1:])))
 (OUT/'grid2_audit.json').write_text(json.dumps(dict(days=n,coins=k,start=str(pd.to_datetime(ts[0],unit='ms',utc=True)),end=str(pd.to_datetime(ts[-1],unit='ms',utc=True)),short={c:v for c,v in audit.items() if v['count']<100},gap_coins={c:v for c,v in audit.items() if v['gaps']},controls=[sh(r) for r in controls],hashes={p:hashlib.sha256(Path(p).read_bytes()).hexdigest() for p in ['src/trader.rs',__file__]}),indent=2))
