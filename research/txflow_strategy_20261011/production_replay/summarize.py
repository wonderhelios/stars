from pathlib import Path
import json,numpy as np,pandas as pd,math,collections
P=Path(__file__).resolve().parent;x=json.load(open(P/'replay.json'));dates=pd.to_datetime(x['dates'],unit='ms',utc=True)
masks={'full':np.ones(len(dates),bool),'first_half':dates.month<7,'third_quarter':dates.month>=7};rows=[];returns={};errors=[]
for a in x['results']:
 d=pd.DataFrame(a['rows']);assert np.isfinite(d[['r','equity','gross','net']]).all().all();returns[(a['factor'],a['offset'],a['case'])]=d.r.values
 assert np.isclose(d.equity.iloc[-1],3000*np.prod(1+d.r))
 # Account cash ledger reconstructed from each day's stored costs and equity.
 for part,mask in masks.items():
  b=d[mask];r=b.r.values;c=np.cumprod(1+r);peak=np.maximum.accumulate(np.r_[1,c])[1:]
  rows.append(dict(factor=a['factor'],offset=int(a['offset']),case=a['case'],part=part,net_return=float(c[-1]-1),sharpe=float(r.mean()/r.std(ddof=1)*np.sqrt(365)),drawdown=float((c/peak-1).min()),rejected=int((~b.accepted).sum()),resized=int(b.resized.sum()),turnover=float(b.turnover.mean()*365),annual_cost=float(b.cost.mean()*365),gross=float(b.gross.mean()),net_breach=int((b.net.abs()>.1).sum()),missing_days=int((b.missing>0).sum())))
 for date,row in zip(dates,d.to_dict('records')):
  if row['error']:errors.append(dict(factor=a['factor'],offset=a['offset'],case=a['case'],date=str(date),error=row['error']))
pd.DataFrame(rows).to_csv(P/'metrics.csv',index=False);pd.DataFrame(errors).to_csv(P/'rejections.csv',index=False)
rng=np.random.default_rng(20261013);infer=[]
def sh(a):return a.mean()/a.std(ddof=1)*np.sqrt(365)
for part,mask in masks.items():
 for off in ['0','5','60']:
  for case in ['base','slip6','slip6_funding3']:
   a=returns[('MV',off,case)][mask];b=returns[('MLV',off,case)][mask];delta=sh(a)-sh(b);n=len(a)
   for length in [7,15,30]:
    bs=[]
    for _ in range(1999):
     starts=rng.integers(n,size=math.ceil(n/length));ids=((starts[:,None]+np.arange(length))%n).ravel()[:n];bs.append(sh(a[ids])-sh(b[ids]))
    bs=np.array(bs);p=(1+(bs-delta>=delta).sum())/2000
    infer.append(dict(part=part,offset=int(off),case=case,block=length,delta=delta,ci_low=np.quantile(bs,.025),ci_high=np.quantile(bs,.975),p=float(p),p_bonf21=float(min(1,p*21)),correlation=float(np.corrcoef(a,b)[0,1])))
pd.DataFrame(infer).to_csv(P/'inference.csv',index=False)
print(pd.DataFrame(rows).query('offset==5').to_string(index=False))
print(pd.DataFrame(infer).query('offset==5 and part=="full"').to_string(index=False))
