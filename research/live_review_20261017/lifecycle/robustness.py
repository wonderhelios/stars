import json
import test_lifecycle as m
R=m.ROOT;out={};windows={'first_half':(0,(m.N//48)*24),'second_half':((m.N//48)*24,m.N),'recent':(int((1791417600000-m.start)//m.HOUR),m.N)}
for w,(lo,hi) in windows.items():
 out[w]={}
 for label,mode,kw in [('control_gross2','baseline',{'target_gross':2.0}),('candidate_delay2','long_tp8_short_stop5',{'exit_delay_hours':2}),('control_rebalance02','baseline',{'rebalance_hour':2}),('candidate_rebalance02','long_tp8_short_stop5',{'rebalance_hour':2})]:
  a=m.sim(mode,lo=lo,hi=hi,**kw);z=m.b.metrics(a);z['missing']=a['missing'];out[w][label]=z;print(w,label,round(z['total']*100,2),round(z['sharpe'],2),round(z['mdd']*100,2),flush=True)
(R/'robustness.json').write_text(json.dumps(out,indent=2))
