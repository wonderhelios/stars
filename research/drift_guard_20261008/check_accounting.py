"""Independent coin-quantity / dollar-equity accounting against weight simulator."""
from panel import *
T,_,_=targets(8);saved=np.load(OUT+'/hourly_returns.npz')
hr={os.path.basename(f)[:-5]:{int(x['t']):float(x['o']) for x in json.load(open(f))} for f in glob.glob('/tmp/hl-hist/*.json')}
errors={}
for mode in ['baseline','net2']:
 out=[]
 for st,en in [('2026-03-15','2026-05-06'),('2026-06-17','2026-07-28')]:
  lo=ti[int(pd.Timestamp(st,tz='UTC').value//1000000)];hi=ti[int(pd.Timestamp(en,tz='UTC').value//1000000)]
  qty=np.zeros(k);equity=570.;last=None
  for day in range(lo,hi):
   initial=equity
   for h in range(24):
    stamp=times[day]+h*3600000;p=np.array([hr.get(name,{}).get(stamp,np.nan) for name in names]);held=qty!=0
    if last is not None:equity+=qty[held]@(p[held]-last[held])
    notion=np.where(held,qty*np.nan_to_num(p),0)
    if h==0 or (mode=='net2' and abs(notion.sum())/equity>.02):
     target=T[day-1]*2.7*equity;delta=target-notion;use=(abs(delta)>=.02*abs(target))|(target*notion<=0);delta=np.where(use,delta,0)
     ids=delta!=0;qty[ids]+=delta[ids]/p[ids];equity-=abs(delta).sum()*.00075
    last=p
   stamp=times[day+1];p=np.array([hr.get(name,{}).get(stamp,np.nan) for name in names]);held=qty!=0;equity+=qty[held]@(p[held]-last[held]);last=p
   if day==hi-1:equity-=np.abs(qty[held]*p[held]).sum()*.00075
   out.append(equity/initial-1)
 expected=saved['0.00075|'+mode][0];error=float(np.max(np.abs(expected-np.array(out))));assert error<1e-12;errors[mode]=error
for f in ['0.00075','0.00105','0.00145']:
 for t in [2,3,5,8]:assert np.array_equal(saved[f+'|net'+str(t)],saved[f+'|net'+str(t)+'_lev85'])
json.dump(errors,open(OUT+'/accounting_check.json','w'),indent=2);print(errors)
