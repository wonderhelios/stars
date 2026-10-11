"""Offline attribution. Execute only from stars; never imports/runs baseline top-level outputs."""
import ast, hashlib, json, itertools, math
from pathlib import Path
import numpy as np
import pandas as pd
ROOT=Path.cwd(); OUT=ROOT/'research/drawdown_attr_20261010'; SRC=ROOT/'research/trailing_tp_20261008/baseline.py'
assert ROOT==Path('/Users/wonder/Code/stars')
source=SRC.read_text(); tree=ast.parse(source)
cutoff=next(x.lineno for x in tree.body if isinstance(x,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='out' for t in x.targets))
g={'__file__':str(SRC),'__name__':'attribution_extract'}
exec(compile('\n'.join(source.splitlines()[:cutoff-1]),str(SRC),'exec'),g); g['OUT']=OUT
n,k=g['n'],g['k']; names=np.array(g['names']); ts=np.array(g['ts']); P,V,SD,M,Q=[g[z] for z in ['P','V','SD','M','Q']]
O=g['a']['o']; factors=['momentum','lowvol','volume']; original_target=g['target']
elig=np.zeros((n,k),bool); kk=np.zeros(n,int); sleeves=np.zeros((n,3,k)); norms=np.zeros(n)
for i in range(32,n):
 idx=np.flatnonzero((V[i]>=5e6)&np.isfinite(P[i])&np.isfinite(P[i-14])&np.isfinite(SD[i])&np.array([':' not in c for c in names]));elig[i,idx]=1
 if len(idx)<8:continue
 kk[i]=min(max(1,int(np.floor(len(idx)*.2+.5))),5,len(idx)//2)
 for f,s in enumerate([M[i]/np.maximum(SD[i],1e-9),-SD[i],Q[i]/V[i]]):
  order=idx[np.lexsort((names[idx],s[idx]))]
  sleeves[i,f,order[:kk[i]]]=-.5/kk[i]/3;sleeves[i,f,order[-kk[i]:]]=.5/kk[i]/3
 raw=sleeves[i].sum(0);norms[i]=abs(raw).sum()
 assert np.allclose(raw/norms[i] if norms[i]>0 else raw,original_target(i,1e6))
run_node=next(x for x in tree.body if isinstance(x,ast.FunctionDef) and x.name=='run'); runsource=ast.get_source_segment(source,run_node)
# Extract original run verbatim, add observers and parameterize gross. Original numerical statements are retained.
s=runsource.replace("def run(cap=5,clock='open',fee=.0007):","def observed_run(cap=5,clock='open',fee=.0007):")
s=s.replace('eq*2.7/px[j]','eq*GROSS/px[j]')
s=s.replace('gain=np.nansum(qty*(mark-last));before=eq;eq+=gain','observe_before(i,qty,last,mark,eq)\n  gain=np.nansum(qty*(mark-last));before=eq;eq+=gain')
s=s.replace('eq-=cost;qty+=delta;last=mark;rets.append(eq/before-1);turn.append(tr);dates.append(ts[i])','observe_trade(i,w,qty,delta,px,eq,cost,tr)\n  eq-=cost;qty+=delta;last=mark;rets.append(eq/before-1);turn.append(tr);dates.append(ts[i])')
s=s.replace("np.savez_compressed(OUT/f'baseline_cap{cap}_{clock}_{fee}.npz',r=rr,t=dates,turn=turn)",'LAST_RESULT.update(r=rr,t=np.array(dates),turn=np.array(turn))')
(OUT/'extracted_observed_run.py').write_text(s+'\n')
exec(compile(s,'extracted_observed_run.py','exec'),g)
class Recorder:
 def __init__(self,subset,normalize,gross):
  self.subset=subset;self.normalize=normalize;self.gross=gross;self.rows=[];self.coin=[];self.costcoin=[];self.exposures=[];self.endweights=[];self.factorpnl=[];self.component_qty=np.zeros((3,k));self.component=np.zeros((3,k));self.cap_min=5
 def target(self,i,equity,cap):
  effective=min(cap,max(1,int(np.floor(equity*2.7/15))//4));self.cap_min=min(self.cap_min,effective)
  if effective<5:raise ValueError('Capital cap activated; precomputed cap5 invalid')
  self.component=np.zeros((3,k));self.component[list(self.subset)]=sleeves[i,list(self.subset)]*3/len(self.subset)
  w=self.component.sum(0); norm=abs(w).sum()
  if self.normalize and norm>0:self.component/=norm;w/=norm
  if self.normalize=='equal_side' and norm>0:
   w=np.where(w>1e-12,.5/max(1,(w>1e-12).sum()),np.where(w< -1e-12,-.5/max(1,(w< -1e-12).sum()),0))
  return w
 def before(self,i,qty,last,mark,eq):
  pnl=np.nan_to_num(qty*(mark-last));fp=np.nansum(self.component_qty*(mark-last)[None,:],axis=1)
  self.coin.append(pnl);self.factorpnl.append(fp);self.exposures.append(np.nan_to_num(qty*last)/eq)
  self.row=dict(i=i,date=pd.to_datetime(ts[i],unit='ms',utc=True).strftime('%Y-%m-%d'),before=eq,price_pnl=pnl.sum(),execution_residual=pnl.sum()-fp.sum())
 def trade(self,i,w,qty,delta,px,eq,cost,tr):
  ew=np.nan_to_num((qty+delta)*px)/(eq-cost);self.endweights.append(ew)
  self.costcoin.append(abs(delta)*np.nan_to_num(px)*FEE)
  self.component_qty=np.divide(self.component*eq*self.gross,px[None,:],out=np.zeros((3,k)),where=np.isfinite(px[None,:])&(px[None,:]!=0))
  gross=abs(ew).sum();frac=abs(ew)/gross if gross else ew
  self.row.update(after=eq-cost,cost=cost,turn=tr,gross=gross,net=ew.sum(),held=int((abs(qty+delta)>1e-12).sum()),neff=1/(frac@frac) if gross else np.nan,max_weight=abs(ew).max(),top1_share=frac.max(),top3_share=np.sort(frac)[-3:].sum(),universe=int(elig[i-1].sum()),kk=int(kk[i-1]),normalizer=float(norms[i-1]))
  self.rows.append(self.row)
def run(label,subset=(0,1,2),gross=2.7,fee=.0007,normalize=True):
 global FEE
 FEE=fee;rec=Recorder(subset,normalize,gross)
 g.update(target=rec.target,GROSS=gross,observe_before=rec.before,observe_trade=rec.trade,LAST_RESULT={})
 res=g['observed_run'](fee=fee);d=pd.DataFrame(rec.rows);d['r']=g['LAST_RESULT']['r'];d['price_r']=d.price_pnl/d.before;d['cost_r']=d.cost/d.before
 arrays=dict(**g['LAST_RESULT'],coin_pnl=np.array(rec.coin),coin_cost=np.array(rec.costcoin),exposure=np.array(rec.exposures),endweights=np.array(rec.endweights),factor_pnl=np.array(rec.factorpnl))
 ex=arrays['exposure'];bg=abs(ex).sum(1);fr=np.divide(abs(ex),bg[:,None],out=np.zeros_like(ex),where=bg[:,None]>0)
 d['begin_gross']=bg;d['begin_held']=(abs(ex)>1e-12).sum(1);d['begin_neff']=np.divide(1,(fr**2).sum(1),out=np.full(len(d),np.nan),where=bg>0);d['begin_top1_share']=fr.max(1);d['begin_top3_share']=np.sort(fr,axis=1)[:,-3:].sum(1);d['begin_max_weight']=abs(ex).max(1)
 d['earning_universe']=elig[d.i.to_numpy()-2].sum(1);d['earning_kk']=kk[d.i.to_numpy()-2];d['earning_normalizer']=norms[d.i.to_numpy()-2]
 np.savez_compressed(OUT/f'{label}.npz',**arrays);d.to_csv(OUT/f'{label}_daily.csv',index=False)
 assert np.allclose(d.price_pnl-d.cost,d.after-d.before,rtol=0,atol=1e-7)
 assert np.allclose(arrays['coin_pnl'].sum(1),d.price_pnl)
 assert np.allclose(arrays['coin_cost'].sum(1),d.cost)
 print(label,res['full']['sharpe'],res['ending_equity'],flush=True)
 return dict(d=d,a=arrays,result=res,cap_min=rec.cap_min)
def stats(r):
 r=np.array(r);e=np.r_[1,np.cumprod(1+r)];dd=e/np.maximum.accumulate(e)-1
 return dict(days=len(r),total=e[-1]-1,annual_arithmetic=r.mean()*365,cagr=e[-1]**(365/len(r))-1,mdd=dd.min(),sharpe=r.mean()/r.std(ddof=1)*np.sqrt(365),log_return=np.log1p(r).sum())
def linked(values,r):
 # Initial-capital dollars: each day's beginning wealth times component daily return.
 wealth=np.r_[1,np.cumprod(1+r)[:-1]]
 return (np.asarray(values)*wealth[:,None]).sum(0)
def describe(v):
 v=np.asarray(v);v=v[np.isfinite(v)]
 return dict(mean=float(np.mean(v)),min=float(np.min(v)),p10=float(np.quantile(v,.1)),median=float(np.median(v)),p90=float(np.quantile(v,.9)),max=float(np.max(v))) if len(v) else {}
def ols(y,x):
 X=np.column_stack([np.ones(len(x)),x]);b=np.linalg.lstsq(X,y,rcond=None)[0];u=y-X@b;bread=np.linalg.inv(X.T@X);z=X*u[:,None];S=z.T@z
 for lag in range(1,8):
  C=z[lag:].T@z[:-lag];S+=(1-lag/8)*(C+C.T)
 se=np.sqrt(np.diag(bread@S@bread)*len(x)/(len(x)-2))
 return dict(alpha_daily=b[0],beta=b[1],beta_hac95=[float(b[1]-1.96*se[1]),float(b[1]+1.96*se[1])],r2=1-u@u/np.sum((y-np.mean(y))**2))
def main():
 runs={}
 for f in range(3):runs[factors[f]]=run(factors[f],(f,))
 runs['combined']=run('combined')
 runs['equal_weight_sides']=run('equal_weight_sides',normalize='equal_side')
 for label,gross,fee,norm in [('combined_1x',1,.0007,True),('combined_nofee',2.7,0,True),('combined_1x_nofee',1,0,True),('no_renormalization',2.7,.0007,False),('no_renormalization_nofee',2.7,0,False)]:runs[label]=run(label,gross=gross,fee=fee,normalize=norm)
 # Also all two-factor leave-one-out portfolios; rerun the same engine, not regression attribution.
 for omitted in range(3):runs['without_'+factors[omitted]]=run('without_'+factors[omitted],tuple(x for x in range(3) if x!=omitted))
 d=runs['combined']['d'];dates=pd.to_datetime(d.date);a=runs['combined']['a'];ii=d.i.to_numpy();peak=pd.Timestamp('2024-03-05');trough=pd.Timestamp('2024-10-15')
 masks={'pre':(dates<=peak).to_numpy(),'drawdown':((dates>peak)&(dates<=trough)).to_numpy(),'post':(dates>trough).to_numpy(),'calendar_2024':(dates.dt.year==2024).to_numpy(),'inclusive_225':((dates>=peak)&(dates<=trough)).to_numpy(),'full':np.ones(len(d),bool)};m=masks['drawdown'];start=float(d.loc[m,'before'].iloc[0]);loss=-stats(d.loc[m,'r'])['total']
 ref=np.load(SRC.parent/'baseline_cap5_open_0.0007.npz');gate=dict(max_return_difference=float(np.max(abs(ref['r']-a['r']))),max_turn_difference=float(np.max(abs(ref['turn']-a['turn']))),same_dates=bool(np.array_equal(ref['t'],a['t'])),baseline_sha256=hashlib.sha256(SRC.read_bytes()).hexdigest())
 assert gate['max_return_difference']<1e-12 and gate['max_turn_difference']<1e-12 and gate['same_dates']
 e=np.r_[1,np.cumprod(1+d.r)];end_idx=np.argmin(e/np.maximum.accumulate(e));peak_idx=np.argmax(e[:end_idx+1]);gate.update(full_mdd=float((e/np.maximum.accumulate(e)-1).min()),peak=str(dates.iloc[peak_idx-1].date()),trough=str(dates.iloc[end_idx-1].date()),window_observations=int(m.sum()),window_start_equity=start,window_end_equity=float(d.loc[m,'after'].iloc[-1]),missing_held_quotes=runs['combined']['result']['missing_held_quotes'])
 rows=[]
 for label,book in runs.items():
  for period,mask in masks.items():rows.append(dict(strategy=label,period=period,**stats(book['d'].loc[mask,'r'])))
 metrics=pd.DataFrame(rows);metrics.to_csv(OUT/'metrics.csv',index=False)
 concentration=[]
 for label in ['momentum','lowvol','volume','combined']:
  b=runs[label]['d']
  for period,mask in masks.items():
   active=mask&(b.gross.to_numpy()>1e-10)
   for field in ['universe','kk','held','neff','gross','net','max_weight','top1_share','top3_share','normalizer','begin_gross','begin_held','begin_neff','begin_max_weight','begin_top1_share','begin_top3_share','earning_universe','earning_kk','earning_normalizer']:
    concentration.append(dict(strategy=label,period=period,field=field,active_days=int(active.sum()),**describe(b.loc[active,field])))
 pd.DataFrame(concentration).to_csv(OUT/'concentration.csv',index=False)
 # Source-of-position attribution of actual price P&L, with execution mismatch explicitly retained.
 fp=a['factor_pnl'][m].sum(0)/start;resid=d.loc[m,'execution_residual'].sum()/start;cost=d.loc[m,'cost'].sum()/start
 source_rows=[dict(component=f,contribution=float(v),loss_share=float(-v/loss)) for f,v in zip(factors,fp)]+[dict(component='execution_residual',contribution=resid,loss_share=-resid/loss),dict(component='cost',contribution=-cost,loss_share=cost/loss)]
 assert abs(sum(x['contribution'] for x in source_rows)+loss)<1e-10
 pd.DataFrame(source_rows).to_csv(OUT/'factor_attribution.csv',index=False)
 coins=pd.DataFrame(dict(coin=names,price_contribution=a['coin_pnl'][m].sum(0)/start,cost_contribution=-a['coin_cost'][m].sum(0)/start))
 coins['net_contribution']=coins.price_contribution+coins.cost_contribution;coins['loss_share']=-coins.net_contribution/loss;coins.sort_values('net_contribution').to_csv(OUT/'coin_attribution.csv',index=False)
 event=[]
 for t,j in itertools.product(np.flatnonzero(m),range(k)):
  if abs(a['exposure'][t,j])>1e-12:event.append(dict(date=d.date.iloc[t],coin=names[j],begin_weight=a['exposure'][t,j],asset_return=a['coin_pnl'][t,j]/(d.before.iloc[t]*a['exposure'][t,j]),daily_contribution=a['coin_pnl'][t,j]/d.before.iloc[t],peak_capital_contribution=a['coin_pnl'][t,j]/start))
 pd.DataFrame(event).sort_values('daily_contribution').to_csv(OUT/'coin_day_events.csv',index=False)
 days=d.loc[m,['date','before','r','price_r','cost_r','price_pnl','cost']].copy();days['peak_capital_net']=(days.price_pnl-days.cost)/start;days.sort_values('r').to_csv(OUT/'worst_days.csv',index=False)
 # Predetermined asset betas from open-to-open returns through prior open, matching earnings interval.
 rr=O/np.roll(O,1,axis=0)-1;rr[0]=np.nan;btc_idx=list(names).index('BTC');btc=rr[:,btc_idx];beta_rows=[];beta_daily={};beta_contrib={}
 for lookback,minobs in [(30,20),(60,20),(120,20),(60,10)]:
  rd=pd.DataFrame(rr);b=pd.Series(btc)
  pair_b=pd.DataFrame(np.broadcast_to(btc[:,None],rr.shape)).where(rd.notna())
  betas=rd.rolling(lookback,min_periods=minobs).cov(b).div(pair_b.rolling(lookback,min_periods=minobs).var()).shift(1).to_numpy()[ii]
  covered=np.isfinite(betas);B=np.nansum(a['exposure']*betas,axis=1);unknown=(abs(a['exposure'])*(~covered)).sum(1);gross=abs(a['exposure']).sum(1);coverage=1-np.divide(unknown,gross,out=np.zeros(len(d)),where=gross>0);br=B*btc[ii]
  key=f'{lookback}_{minobs}';beta_daily[key]=dict(B=B,br=br,coverage=coverage,betas=betas)
  for period,mask in masks.items():
   active=mask&(gross>1e-10);r=d.r.to_numpy()[mask];gp=d.price_r.to_numpy()[mask];x=btc[ii][mask];bpart=br[mask]
   beta_rows.append(dict(lookback=lookback,minobs=minobs,period=period,beta_mean=float(B[mask].mean()),beta_active_mean=float(B[active].mean()),beta_min=float(B[mask].min()),beta_max=float(B[mask].max()),positive_fraction=float((B[active]>0).mean()),coverage_mean=float(coverage[active].mean()),coverage_min=float(coverage[active].min()),beta_contribution=float(linked(bpart[:,None],r)[0]),beta_arithmetic_sum=float(bpart.sum()),btc_return=float(np.prod(1+x)-1),btc_annual_arithmetic=float(x.mean()*365),**{'net_ols_'+q:v for q,v in ols(r,x).items()},**{'price_ols_'+q:v for q,v in ols(gp,x).items()}))
  beta_contrib[key]=float(np.sum(d.before.to_numpy()[m]*br[m])/start)
 pd.DataFrame(beta_rows).to_csv(OUT/'beta_summary.csv',index=False)
 bd=beta_daily['60_20'];beta_detail=pd.DataFrame(dict(date=d.date,btc_return=btc[ii],beta=bd['B'],coverage=bd['coverage'],beta_return=bd['br'],price_return=d.price_r,net_return=d.r))
 beta_detail['residual_price_return']=beta_detail.price_return-beta_detail.beta_return;beta_detail.to_csv(OUT/'beta_daily.csv',index=False)
 # Horizon subsets and BTC shocks: explain sign/timing instead of multiplying mean beta by market CAGR.
 beta_condition=[]
 for label,mask in [('btc_down',m&(btc[ii]<0)),('btc_up',m&(btc[ii]>=0)),('btc_down_5pct',m&(btc[ii]<=-.05)),('btc_up_5pct',m&(btc[ii]>=.05))]:
  beta_condition.append(dict(condition=label,days=int(mask.sum()),mean_beta=float(bd['B'][mask].mean()),price_contribution=float(d.price_pnl.to_numpy()[mask].sum()/start),beta_contribution=float(np.sum(d.before.to_numpy()[mask]*bd['br'][mask])/start),cost_contribution=float(-d.cost.to_numpy()[mask].sum()/start)))
 pd.DataFrame(beta_condition).to_csv(OUT/'beta_conditions.csv',index=False)
 # Same-window endpoint loss Shapley across leverage and cost. All full-history reruns, masks fixed.
 losses={label:-stats(book['d'].loc[m,'r'])['total'] for label,book in runs.items()}
 L00=losses['combined_1x_nofee'];L10=losses['combined_nofee'];L01=losses['combined_1x'];L11=losses['combined']
 leverage=.5*((L10-L00)+(L11-L01));cost_shapley=.5*((L01-L00)+(L11-L10))
 shapley=dict(base_1x_nofee_loss=L00,leverage=leverage,cost=cost_shapley,leverage_share=leverage/loss,cost_share=cost_shapley/loss,base_share=L00/loss,interaction=L11-L10-L01+L00)
 assert abs(L00+leverage+cost_shapley-loss)<1e-12
 # Equally weighted separately capitalized sleeves and daily reset sleeves have equal initial total capital.
 singles=np.array([runs[f]['d'].r.to_numpy()[m] for f in factors]);single_totals=np.prod(1+singles,axis=1)-1
 equal_buyhold=single_totals.mean();equal_reset=stats(singles.mean(0))
 # Monthly breakdown by dollars, including actual source and costs.
 monthly=[]
 for month in sorted(dates[m].dt.strftime('%Y-%m').unique()):
  z=m&(dates.dt.strftime('%Y-%m').to_numpy()==month);row=dict(month=month,**stats(d.loc[z,'r']),universe_mean=float(d.loc[z,'earning_universe'].mean()),neff_mean=float(d.loc[z,'begin_neff'].mean()),beta_mean=float(bd['B'][z].mean()),btc_return=float(np.prod(1+btc[ii][z])-1),beta_contribution=float(np.sum(d.before.to_numpy()[z]*bd['br'][z])/start),cost_contribution=float(-d.cost.to_numpy()[z].sum()/start),peak_capital_net=float((d.price_pnl.to_numpy()[z]-d.cost.to_numpy()[z]).sum()/start))
  row.update({f:float(v) for f,v in zip(factors,a['factor_pnl'][z].sum(0)/start)});monthly.append(row)
 pd.DataFrame(monthly).to_csv(OUT/'monthly.csv',index=False)
 # Fixed realized return path, cost removal / leverage scaling, distinguished from independent reruns.
 frozen_fee=stats(d.loc[m,'price_r'])
 scaled=stats(d.loc[m,'r']/2.7)
 log_net=np.log1p(d.loc[m,'r']).sum();log_gross=np.log1p(d.loc[m,'price_r']).sum()
 summary=dict(gate=gate,window_loss=loss,source_attribution=source_rows,losses=losses,shapley=shapley,beta_contribution=beta_contrib,combined_price_contribution=float(d.loc[m,'price_pnl'].sum()/start),cost_cash_contribution=float(cost),mean_turn=float(d.loc[m,'turn'].mean()),annual_cost=float(d.loc[m,'turn'].mean()*.0007*365),sum_turn=float(d.loc[m,'turn'].sum()),sum_cost_return=float(d.loc[m,'cost_r'].sum()),sum_price_return=float(d.loc[m,'price_r'].sum()),sum_net_return=float(d.loc[m,'r'].sum()),log_net=float(log_net),log_price=float(log_gross),log_cost=float(log_net-log_gross),arithmetic_to_log=float(d.loc[m,'r'].sum()-log_net),single_totals=dict(zip(factors,single_totals.tolist())),sum_single_totals=float(single_totals.sum()),equal_capital_sleeves_total=float(equal_buyhold),daily_reset_sleeves=equal_reset,frozen_no_cost=frozen_fee,frozen_scaled_1x=scaled,universe_all=describe(d.loc[m,'earning_universe']),neff=describe(d.loc[m,'begin_neff']),normalizer=describe(d.loc[m,'earning_normalizer']),normalization_amplifier=describe(1/d.loc[m,'earning_normalizer']),no_norm_gross=describe(runs['no_renormalization']['d'].loc[m,'gross']),minimum_cap=min(x['cap_min'] for x in runs.values()))
 (OUT/'summary.json').write_text(json.dumps(summary,indent=2));(OUT/'verification.json').write_text(json.dumps(gate,indent=2))
 manifest={str(p):hashlib.sha256(Path(p).read_bytes()).hexdigest() for p in [SRC,SRC.parent/'retrieval/request.json',SRC.parent/'retrieval/meta.json',Path(__file__),*map(Path,g['files'])]}
 (OUT/'input_manifest.json').write_text(json.dumps(manifest,indent=2));np.savez_compressed(OUT/'target_diagnostics.npz',t=ts,names=names,elig=elig,kk=kk,sleeves=sleeves,normalizer=norms)
 print(json.dumps(summary,indent=2),flush=True)
if __name__=='__main__':main()
