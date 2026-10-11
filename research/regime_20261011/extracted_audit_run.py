def run(cap=5,clock='open',fee=.0007):
 # large initial capital; exchange size decimals/minimum/band applied at every order
 prices=a['o'] if clock=='open' else P
 eq=1e6;qty=np.zeros(k);last=np.full(k,np.nan);rets=[];turn=[];missing=[];dates=[]
 for i in range(33,n):
  px=prices[i];held=np.abs(qty)>1e-12
  miss=held&~np.isfinite(px);missing.append(dict(t=ts[i],coins=np.array(names)[miss].tolist(),stale_notional_equity=float(np.nansum(abs(qty[miss])*last[miss])/eq))) if miss.any() else None
  mark=np.where(np.isfinite(px),px,last)
  gain=np.nansum(qty*(mark-last));before=eq;eq+=gain
  if eq<=0:raise ValueError('insolvent')
  w=target(i-1 if clock=='open' else i,eq,cap)
  wanted=np.zeros(k)
  for j,c in enumerate(names):
   if not np.isfinite(px[j]) or abs(w[j])<1e-12:continue
   f=10**meta[c]['szDecimals'];size=np.floor(abs(w[j])*eq*LEV[i-33]/px[j]*f)/f
   if size*px[j]>=10:wanted[j]=np.sign(w[j])*size
  delta=np.zeros(k)
  for j in range(k):
   if not np.isfinite(px[j]):continue  # cannot liquidate stale missing quote
   f=10**meta[names[j]]['szDecimals'];d=wanted[j]-qty[j]
   if qty[j]*wanted[j]<0: # closing plus reopening charged both sides, equivalent abs delta
    delta[j]=d
   elif wanted[j]==0:
    if abs(qty[j])*px[j]>=10:delta[j]=-qty[j]
   elif np.floor(abs(d)*f)/f*px[j]>=max(10,abs(wanted[j])*px[j]*.02):
    delta[j]=np.sign(d)*np.floor(abs(d)*f)/f
  cost=fee*np.sum(abs(delta)*np.nan_to_num(px));tr=np.sum(abs(delta)*np.nan_to_num(px))/eq
  eq-=cost;qty+=delta;last=mark;rets.append(eq/before-1);turn.append(tr);dates.append(ts[i])
 rr=np.array(rets);dd=pd.to_datetime(dates,unit='ms',utc=True)
 results={}
 for label,mask in [('full',np.ones(len(rr),bool)),('2023-24',dd.year<=2024),('2025-26',dd.year>=2025)]:
  r=rr[mask];t=np.array(turn)[mask]
  results[label]=dict(days=len(r),sharpe=sh(r),annual_arithmetic=float(r.mean()*365) if len(r) else None,annual_cost=float(t.mean()*fee*365) if len(t) else None,daily_turnover=float(t.mean()) if len(t) else None)
 results['missing_held_quotes']=missing;results['ending_equity']=eq
 REC.update(r=rr,turn=np.array(turn))
 return results
