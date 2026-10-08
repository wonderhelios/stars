from pathlib import Path
import types,sys,json,numpy as np
R=Path(__file__).resolve().parents[2];O=Path(__file__).resolve().parent
p=R/'research/onchain_20261008/engine.py';e=types.ModuleType('engine');e.__file__=str(p);exec(compile(p.read_text(),str(p),'exec'),e.__dict__);sys.modules['engine']=e
p=p.with_name('run.py');s=p.read_text().split('def sif(r):')[0];s=s.replace("json.dump(mapping,open(OUT/'mapping.json','w'),indent=2)",'pass').replace("json.dump(meta_candidates,open(OUT/'candidates.json','w'),indent=2)",'pass');ns={'__file__':str(p)};exec(compile(s,str(p),'exec'),ns)
r={'onchain':{'summary':ns['summary'],'market_prediction':ns['market_stats']}}
old=json.load(open(p.with_name('results.json')));r['onchain']['max_sharpe_difference']=max(abs(v['full']['sharpe']-old['summary'][z]['full']['sharpe']) for z,v in ns['summary'].items())
p=R/'research/vrp_20261008/analyze.py';s=p.read_text().split("json.dump(results,open(O/'results.json'")[0].replace("f.to_csv(O/f'{coin}_aligned.csv',index_label='timestamp_observable')",'pass');ns={'__file__':str(p)};exec(compile(s,str(p),'exec'),ns);r['vrp']=ns['results']
(O/'late_reproduction.json').write_text(json.dumps(r,indent=2));print('onchain discrepancy',r['onchain']['max_sharpe_difference']);print('VRP BTC/ETH',r['vrp']['BTC']['full']['vol_gap_pp'],r['vrp']['ETH']['full']['vol_gap_pp'])
