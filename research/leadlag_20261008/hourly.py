import json,glob,os,numpy as np,pandas as pd
OUT=os.path.dirname(os.path.abspath(__file__));raw={os.path.basename(f)[:-5]:json.load(open(f)) for f in glob.glob('/tmp/hl-hist/*.json')};names=sorted(raw);ts=sorted({int(c['t']) for v in raw.values() for c in v});ti={t:i for i,t in enumerate(ts)};N=len(ts);K=len(names)
P=np.full((N,K),np.nan);V=P.copy();OP=P.copy()
for j,name in enumerate(names):
 for c in raw[name]:
  i=ti[int(c['t'])];P[i,j]=float(c['c']);OP[i,j]=float(c['o']);V[i,j]=float(c['v'])*P[i,j]
R=P/np.roll(P,1,axis=0)-1;R[0]=np.nan;b=names.index('BTC');btc=R[:,b]
def roll(a):return pd.DataFrame(a).rolling(720,min_periods=600).mean().values
x=np.broadcast_to(btc[:,None],R.shape);beta=(roll(R*x)-roll(R)*roll(x))/(roll(x*x)-roll(x)**2)
# Realized residual uses beta known before target hour. Lag coefficients fit past observations only.
y=R-np.roll(beta,1,axis=0)*x
liq=pd.DataFrame(V).rolling(720,min_periods=120).mean().shift(1).values*24>=5e6
valid=liq&np.isfinite(P);valid[:,b]=False
start=800;end=N-2;inds=np.arange(start,end); dates=pd.to_datetime(np.array(ts)[inds+1],unit='ms',utc=True)
baseline_data=np.load(OUT+'/returns.npz',allow_pickle=True); bd=pd.to_datetime(baseline_data['dates'],utc=True); baseline_series=pd.Series(baseline_data['baseline'].mean(axis=0),index=bd.floor('D'))
rows=[];daily=[];keys=[];rng=np.random.default_rng(20261008)
for lag in [1,4,12,24]:
 lagpast=pd.Series(btc).rolling(lag).sum().shift(1).values;xx=np.broadcast_to(lagpast[:,None],R.shape)
 coef=(roll(y*xx)-roll(y)*roll(xx))/(roll(xx*xx)-roll(xx)**2)
 signal=coef*pd.Series(btc).rolling(lag).sum().values[:,None]
 # coefficient is heterogeneous by coin; identical BTC score itself cannot rank coins.
 target=np.zeros_like(P)
 for i in inds:
  idx=np.flatnonzero(valid[i]&np.isfinite(signal[i]));
  if len(idx)<8:continue
  kk=min(5,len(idx)//2);order=idx[np.lexsort((np.array(names)[idx],signal[i,idx]))];target[i,order[:kk]]=-.5/kk;target[i,order[-kk:]]=.5/kk
 # Diagnostic of direct residual predictability: realized next-hour residual, never executable P&L.
 residual_gross=np.nansum(target[inds]*y[inds+1],axis=1)
 residual_annual=float(np.mean(residual_gross)*24*365)
 for hold in [1,4,12]:
  rr=[];turn=[]
  for phase in range(hold):
   w=np.zeros(K);ret=[];to=[]
   for i in inds:
    r=np.nan_to_num(OP[i+1]/OP[i]-1);gain=w@r;w=w*(1+r)/(1+gain)
    delta=target[i]*2.7-w if (ts[i]//3600000+phase)%hold==0 else np.zeros(K)
    # Require observed executable next open; stale positions get adverse 5% exit charge.
    missing=~np.isfinite(OP[i+1]);extra=np.abs(w[missing]).sum()*.05;delta[missing]=-w[missing]
    t=np.abs(delta).sum();cost=t*.00075+extra;w=(w+delta)/(1-cost);ret.append((1+gain)*(1-cost)-1);to.append(t)
   terminal=np.abs(w).sum();ret[-1]=(1+ret[-1])*(1-terminal*.00075)-1;to[-1]+=terminal
   dr=pd.Series(ret,index=dates).groupby(dates.floor('D')).apply(lambda a:np.prod(1+a)-1).values;rr.append(dr);turn.append(np.mean(to))
  rr=np.array(rr);s=rr.mean(axis=1)/rr.std(axis=1,ddof=1)*np.sqrt(365);key=f'btc_hourly_lag{lag}_hold{hold}';keys.append(key);daily.append(rr.mean(axis=0));rows.append({'candidate':key,'next_hour_residual_gross_annual_diagnostic':residual_annual,'sharpe_phase_mean':s.mean(),'phase_sd':s.std(),'phase_min':s.min(),'phase_max':s.max(),'annual_net':rr.mean()*365,'annual_cost':np.mean(turn)*24*365*.00075,'daily_turnover_equity':np.mean(turn)*24})
for j,row in enumerate(rows):
 ds=pd.Series(daily[j],index=pd.DatetimeIndex(dates.floor('D')).unique()); aligned=pd.concat([ds,baseline_series],axis=1).dropna(); row['correlation_baseline_daily']=float(aligned.corr().iloc[0,1]);row['baseline_overlap_days']=len(aligned)
X=np.array(daily);obs=X.mean(axis=1);L=7;draws=[]
for rep in range(4999):
 st=rng.integers(X.shape[1],size=int(np.ceil(X.shape[1]/L)));ii=((st[:,None]+np.arange(L))%X.shape[1]).ravel()[:X.shape[1]];draws.append(X[:,ii].mean(axis=1))
draws=np.array(draws);se=draws.std(axis=0,ddof=1);maximum=((draws-obs)/se).max(axis=1)
# Allocate 1/3 of alpha to each inferential family, strong union-bound combination without aligned histories.
for j,row in enumerate(rows):row['p_fwer_global']=min(1,3*(1+np.sum(maximum>=obs[j]/se[j]))/5000)
pd.DataFrame(rows).to_csv(OUT+'/hourly_metrics.csv',index=False)
json.dump({'start':str(dates[0]),'end':str(dates[-1]),'coins':K,'candidates':len(rows),'training_hours':720,'bootstrap_block_days':7,'draws':4999,'missing_early_period':True,'rows':rows},open(OUT+'/hourly_results.json','w'),indent=2)
print(pd.DataFrame(rows).to_string(index=False))
