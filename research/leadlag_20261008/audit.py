import engine as e, numpy as np,pandas as pd,json
from statsmodels.tsa.stattools import coint
p=e.OUT; screen=pd.read_csv(p+'/pair_screen.csv'); result=[]
for row in screen.itertuples():
 a=e.names.index(row.a);b=e.names.index(row.b);y=np.log(e.P[50:230,a]);x=np.log(e.P[50:230,b]);ok=np.isfinite(y)&np.isfinite(x)
 if ok.sum()<180:continue
 stat,pv,_=coint(y[ok],x[ok],trend='c',maxlag=5,autolag=None)
 result.append({'a':row.a,'b':row.b,'formation_coint_t':stat,'p_raw':pv,'p_fwer_bonferroni':min(1,pv*len(screen)), 'p_fwer_global':min(1,pv*len(screen)*3)})
pd.DataFrame(result).to_csv(p+'/formation_cointegration.csv',index=False)
# Persist sanity checks and missing exit stress for daily baseline, with no candidate selection.
e.START=45;e.ix=np.arange(e.START,e.END);e.outdates=e.dates[e.ix+1];base,_,_=e.targets()
assert np.nanmax(np.abs(base.sum(axis=1)))<1e-10
assert np.all(np.diff(e.times)==e.DAY)
rr=[e.run(base,o,missing_penalty=.05) for o in range(3)]
json.dump({'target_neutrality_check':'PASS','timeline_check':'PASS','missing_exit_stress_baseline_sharpe':float(np.mean([e.stat(v['r'])['sharpe'] for v in rr])),'formation_coint_tests':len(result),'formation_coint_pass':sum(r['p_fwer_bonferroni']<.05 for r in result)},open(p+'/audit.json','w'),indent=2)
