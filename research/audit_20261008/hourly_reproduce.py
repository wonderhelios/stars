from pathlib import Path
import sys,types,json,numpy as np
ROOT=Path(__file__).resolve().parents[2];OUT=Path(__file__).resolve().parent;result={}
for d,script,cut in [('leadlag_20261008','hourly.py','X=np.array(daily)'),('liq_oi_20261008','hourly_events.py','# Paired calendar-day bootstrap')]:
 p=ROOT/'research'/d/script;ns={'__file__':str(p)};exec(compile(p.read_text().split(cut)[0],str(p),'exec'),ns);result[d]=ns.get('summary',ns.get('rows'))
d='distribution_stagger_20261008';p=ROOT/'research'/d/'engine.py';m=types.ModuleType('engine');m.__file__=str(p);exec(compile(p.read_text(),str(p),'exec'),m.__dict__);sys.modules['engine']=m
p=p.with_name('hourly.py');ns={'__file__':str(p)};src=p.read_text().split('meta=dict(')[0].replace("mask=np.ones(len(ix),bool);boot=maxT(runs,'baseline',mask)","mask=np.ones(len(ix),bool);boot={}");exec(compile(src,str(p),'exec'),ns);result[d]=ns['res']
# Legacy baseline is also independently rerun, no output files in source.
p=Path('/tmp/stag2.py');ns={'__file__':str(p)};exec(compile(p.read_text(),str(p),'exec'),ns);result['legacy_phase_sharpes']={str(sl):[ns['stat'](x)[1] for x in arr] for sl,arr in ns['res'].items()}
(OUT/'hourly_reproduction.json').write_text(json.dumps(result,indent=2,default=lambda x:float(x)))
print('hourly core and legacy reproduction completed')
