#!/usr/bin/env python3
"""Offline invariant checks against actual saved API responses."""
import csv,json,math,hashlib
from pathlib import Path
ROOT=Path(__file__).resolve().parent;D=ROOT/'data';DAY=86400000
read=lambda p:json.loads(p.read_text())
def csvread(p):
 with p.open() as f:
  r=csv.DictReader(f);assert r.fieldnames==['ts','open','high','low','close','volume'],p
  return [{k:int(v) if k=='ts' else float(v) for k,v in row.items()} for row in r]
def near(a,b):assert math.isclose(a,b,rel_tol=1e-10,abs_tol=1e-12),(a,b)
rows=read(D/'mapping_all.json');tx=read(D/'raw/txflow_perpMeta.json')['universe'];cfg=read(D/'run_config.json');cut=cfg['last_closed_open_ms']+DAY
assert len(rows)==len(tx) and len(tx)>0
assert {r['txflow_name'] for r in rows}=={m['name'] for m in tx}
assert len({r['txflow_name'] for r in rows})==len(rows)
assert all(r['status']!='pending' for r in rows)
strict=read(D/'mapping_strict.json');verified=read(D/'mapping_verified.json')
assert {r['txflow_name'] for r in strict}=={r['txflow_name'] for r in rows if r.get('strategy_ready')}
assert {r['txflow_name'] for r in verified}=={r['txflow_name'] for r in rows if r.get('mapping_verified')}
nfiles=0;ncomparisons=0
for r in rows:
 s=r.get('binance_symbol');p=D/'binance_spot'/f'{s}.csv'
 if not s or not p.exists():continue
 b=csvread(p);raw=read(D/'raw/klines'/f'{s}.json');closed=[x for x in raw if int(x[6])<cut]
 assert len(b)==len(closed)==r['binance_rows'];assert len(b)<=1000
 for row,x in zip(b,closed):
  assert row['ts']==int(x[0]);assert row['ts']%DAY==0 and row['ts']<cut
  for k,i in [('open',1),('high',2),('low',3),('close',4),('volume',5)]:near(row[k],float(x[i]))
  assert row['low']<=min(row['open'],row['close'])<=max(row['open'],row['close'])<=row['high'];assert row['volume']>=0
 assert all(b[i]['ts']>b[i-1]['ts'] for i in range(1,len(b)))
 t=csvread(D/'daily'/f'{r["txflow_name"]}.csv');assert len(b)==len(t)
 for x,y in zip(b,t):
  assert x['ts']==y['ts']
  for k in ['open','high','low','close']:near(y[k],x[k]*r['bn_price_to_tx_mult'])
  near(y['volume'],x['volume']*r['bn_volume_to_tx_mult'])
 nfiles+=1;vpath=D/'validation'/f'{r["txflow_name"]}.json'
 if not vpath.exists():continue
 proof=read(vpath);coin=r['hl_symbol'];key=coin.replace('/','_').replace(':','_');h=csvread(D/'hyperliquid'/f'{key}.csv');hr=read(D/'raw/hl_candles'/f'{key}.json');hclosed=sorted([z for z in hr if int(z['T'])<cut],key=lambda z:int(z['t']))
 assert len(h)==len(hclosed)
 for row,x in zip(h,hclosed):
  assert row['ts']==int(x['t'])
  for k,kraw in [('open','o'),('high','h'),('low','l'),('close','c'),('volume','v')]:near(row[k],float(x[kraw]))
 bn={x['ts']:x for x in b};hn={x['ts']:x for x in h};assert [p['ts'] for p in proof]==sorted(set(bn)&set(hn))
 for z in proof:
  x=bn[z['ts']];y=hn[z['ts']];near(z['binance_close'],x['close']);near(z['hl_close'],y['close']);near(z['binance_volume'],x['volume']);near(z['hl_volume'],y['volume']);near(z['price_ratio_hl_bn_raw'],y['close']/x['close']);near(z['price_ratio_bn_hl_underlying'],x['close']/(y['close']/r['hl_unit']))
  if y['volume']>0:near(z['volume_ratio_bn_hl_raw'],x['volume']/y['volume']);near(z['volume_ratio_bn_hl_underlying'],x['volume']/(y['volume']*r['hl_unit']))
  else:assert z['volume_ratio_bn_hl_underlying'] is None
  ncomparisons+=1
 if r.get('mapping_verified'):
  assert proof[-1]['ts']==cfg['last_closed_open_ms'];assert len(proof)>=3;assert r['price_error_max_7d']<=.05;assert r['binance_status']=='TRADING';assert r['hl_identity_confirmed']
 if r.get('strategy_ready'):assert not r['warnings'] and r['mapping_verified'] and r['binance_rows']>=100
for item in read(D/'manifest.json'):
 p=ROOT/item['path'];assert p.stat().st_size==item['bytes'],p;assert hashlib.sha256(p.read_bytes()).hexdigest()==item['sha256'],p
summary={'result':'PASS','markets':len(rows),'binance_files_checked':nfiles,'same_day_comparisons_checked':ncomparisons,'mapping_verified':len(verified),'strict_candidates':len(strict)}
(ROOT/'verification.json').write_text(json.dumps(summary,indent=2)+'\n');print(json.dumps(summary))
