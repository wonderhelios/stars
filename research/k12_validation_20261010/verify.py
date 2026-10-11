"""Reference parity, independent ledger conservation, causality and input audit."""
from engine import *
from analyze import load,NAMES,dd

def main():
 freeze();checks={};reference={}
 for name,path in [('B0',SRC.parent/'baseline_cap5_open_0.0007.npz'),('K12',ROOT/'research/universe_size_20261009/K12_p0_f0.0007.npz')]:
  a=load(name);b=dict(np.load(path));exact={x:bool(np.array_equal(a[x],b[x])) for x in ['r','t','turn']};assert all(exact.values())
  reference[name]=dict(exact=exact,reference=str(path),sha256=hashlib.sha256(path.read_bytes()).hexdigest(),sharpe=g['sh'](a['r']))
 # Exact uninstrumented frozen-run parity, changing targets only.
 for name in NAMES[:4]:
  for delay in [0,1]:
   d=load(name,0,.0007,delay);res=json.loads((OUT/f'{key(name,0,.0007,delay)}.json').read_text());_,plain=engine(name,0,.0007,delay,res['m'],diagnostics=False)
   assert all(np.array_equal(d[x],plain[x]) for x in ['r','t','turn'])
 checks['uninstrumented_exact_paths']=8
 ledger=[];signals=[]
 for p in sorted(OUT.glob('*_d[01].npz')):
  if not p.name.split('_p')[0] in NAMES:continue
  d=dict(np.load(p));label=p.stem;fee=float(label.split('_f')[1].split('_d')[0]);delay=int(label[-1]);ii=np.array([g['ti'][int(t)] for t in d['t']]);qty=d['qty'];dq=d['delta_qty'];prev=np.vstack([np.zeros(k),qty[:-1]]);px=g['a']['o'][ii]
  assert np.allclose(qty,prev+dq,rtol=0,atol=1e-8)
  marks=np.empty_like(px);last=np.full(k,np.nan)
  for j in range(len(ii)):marks[j]=np.where(np.isfinite(px[j]),px[j],last);last=marks[j]
  oldmarks=np.vstack([np.full(k,np.nan),marks[:-1]]);pnl=np.nansum(prev*(marks-oldmarks),axis=1);cost=fee*np.sum(abs(dq)*np.nan_to_num(px),axis=1);eq=1e6+np.cumsum(pnl-cost)
  assert np.allclose(pnl,d['price_pnl'],rtol=1e-10,atol=1e-7);assert np.allclose(cost,d['cost'],rtol=1e-12,atol=1e-7);assert np.allclose(eq,d['equity'],rtol=1e-10,atol=1e-6)
  before=np.r_[1e6,d['equity'][:-1]];assert np.allclose(d['r'],d['equity']/before-1,rtol=0,atol=1e-13)
  if label.startswith('A_p'):assert d['target_gross'].max()<=2.7+1e-12
  pd_eq=pd.Series(np.r_[1.,np.cumprod(1+d['r'])]);md=float((pd_eq/pd_eq.cummax()-1).min());assert abs(md-dd(d['r'],d['t'])['max_drawdown'])<1e-12
  ledger.append(dict(path=label,days=len(ii),missing_held=int(d['missing_held'].sum()),held_badbar=int(d['held_badbar'].sum()),order_badbar=int(d['order_badbar'].sum()),order_badbar_dollars=float(d['order_badbar_dollars'].sum()),max_equity_identity_error=float(abs(eq-d['equity']).max())))
  if '_p0_f0.0007_d0' in label:
   bad_signal_days=0;bad_signal_coin_windows=0;eligible_bad_days=0;details=[]
   for z,i in enumerate(ii):
    t=i-1-delay;J=np.flatnonzero(abs(d['target'][z])>1e-12);E=elig[t]
    # Union of volume30 (t-30..t-1), SD20 endpoint (t-20..t), momentum14 endpoints and Q[t].
    window=badtrade[max(0,t-30):t+1]
    touched=J[window[:,J].any(0)] if len(J) else J
    eligible_bad_days+=int(len(E)>0 and window[:,E].any())
    if len(touched):
     bad_signal_days+=1;bad_signal_coin_windows+=len(touched);details.append(dict(date=str(pd.to_datetime(d['t'][z],unit='ms',utc=True).date()),coins=names[touched].tolist()))
   signals.append(dict(name=label.split('_p')[0],target_signal_window_bad_days=bad_signal_days,target_bad_coin_windows=bad_signal_coin_windows,eligible_signal_window_bad_days=eligible_bad_days,details=details))
 matches=[]
 for p in OUT.glob('A_const_p*_d[01].json'):
  r=json.loads(p.read_text());assert abs(r['match_error'])<=.005;matches.append(dict(path=p.stem,m=r['m'],relative_error=r['match_error']))
 # C's valid coverage and numerical constraints, independently recomputed.
 extra=dict(np.load(OUT/'extra_targets.npz'));audit=json.loads((OUT/'extra_target_audit.json').read_text());coverage={};maxres=0.
 for name in ['C','C_diag']:
  for row in audit['days']:
   i=row['i'];w0=original_target(i,1e6);w=extra[name][i]
   if row[name]=='active':
    J=abs(w0)>1e-12;a0=abs(w0[J]);aa=abs(w[J]);rr=max(abs(w[w>0].sum()-.5),abs(w[w<0].sum()+.5),max(0.,float(abs(aa-a0).sum()-.20)),float(np.maximum(.5*a0-aa,0).max()),float(np.maximum(aa-1.5*a0,0).max()));maxres=max(maxres,rr);assert rr<=1e-8;assert np.all(np.sign(w[J])==np.sign(w0[J]));assert np.all(w[~J]==0)
  coverage[name]={}
  for label,lo,hi in [('2024','2024-01-01','2024-12-31'),('2025-26','2025-01-01','2026-12-31')]:
   rows=[r for r in audit['days'] if lo<=r['date']<=hi and r['active']];coverage[name][label]=dict(eligible_days=len(rows),active_days=sum(r[name]=='active' for r in rows),coverage=sum(r[name]=='active' for r in rows)/len(rows),solver_fallback=sum(r[name]=='solver_fallback' for r in rows),window_fallback=sum(r[name]=='window_fallback' for r in rows))
 checks.update(reference=reference,ledger_paths=len(ledger),ledger=ledger,signal_zero_trade_dependence=signals,gross_matches=matches,extra_coverage=coverage,max_C_constraint_residual=maxres,all_input_hashes_unchanged=True)
 old=json.loads((ROOT/'research/universe_size_20261009/manifest.json').read_text());now=json.loads((OUT/'input_manifest.json').read_text());shared=set(old)&set(now);checks['historical_manifest_comparison']=dict(shared_files=len(shared),changed=[p for p in sorted(shared) if old[p]!=now[p]])
 savejson(OUT/'verification.json',checks);pd.DataFrame(ledger).to_csv(OUT/'ledger_audit.csv',index=False);print('verified',len(ledger),'paths',reference,'coverage',coverage,flush=True)
if __name__=='__main__':main()
