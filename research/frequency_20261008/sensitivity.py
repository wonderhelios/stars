import ast,pathlib,json
import numpy as np,pandas as pd
O=pathlib.Path(__file__).resolve().parent
# Load the frozen data, targets, and simulator without rerunning the main family.
s=(O/'analyze.py').read_text();exec(s[:s.index('runs={};summary={}')])
x=json.loads((O/'results.json').read_text());S=x['summary'];out={}
# Root in common fixed slippage, by bisection with complete cost/equity resimulation.
for name in list(PERIODS)[1:]:
 def delta(s):return float(sharpe(sim(PERIODS[name],s)['r']).mean()-sharpe(sim(24,s)['r']).mean())
 lo=0.;hi=.004;a=delta(lo);b=delta(hi)
 if a*b<0:
  for _ in range(16):
   mid=(lo+hi)/2
   if delta(mid)>0:lo=mid
   else:hi=mid
  out[name]=dict(common_slippage_crossing=(lo+hi)/2,bracket=[lo,hi])
 else:out[name]=dict(common_slippage_crossing=None,delta_zero=a,delta_40bp=b)
# Weekly observed-slippage anchor only; not an additional return hypothesis.
week=sim(168,.00025);tw=float(week['turn'].mean()*365)
models={}
for alpha,floor in [('floor_1bp',.0001),('pure_sqrt',0.)]:
 vals={}
 for name,p in PERIODS.items():
  t=S[f'{name}_0.0003']['full']['annual_turnover'];slip=floor+(.00025-floor)*np.sqrt(t/tw);v=sim(p,slip)
  vals[name]=dict(slippage=float(slip),turnover_proxy=t,sharpe=float(sharpe(v['r']).mean()))
 for name in vals:vals[name]['delta_vs_daily']=vals[name]['sharpe']-vals['1x']['sharpe']
 models[alpha]=vals
(O/'sensitivity.json').write_text(json.dumps(dict(crossings=out,weekly_turnover=tw,models=models),indent=2));print('done',flush=True)
