import json,time,datetime,pathlib,urllib.request,os
OUT=pathlib.Path(__file__).resolve().parent
for k,v in {'https_proxy':'http://127.0.0.1:7897','http_proxy':'http://127.0.0.1:7897','all_proxy':'socks5://127.0.0.1:7897'}.items():os.environ[k]=v
H=3600000
now=int(time.time()*1000)//H*H
log=[]
def iso(t):return datetime.datetime.fromtimestamp(t/1000,datetime.timezone.utc).isoformat()
def call(coin,start,end,label):
 payload={'type':'candleSnapshot','req':{'coin':coin,'interval':'1h','startTime':start,'endTime':end}}
 for attempt in range(4):
  try:
   req=urllib.request.Request('https://api.hyperliquid.xyz/info',json.dumps(payload).encode(),{'Content-Type':'application/json'})
   with urllib.request.urlopen(req,timeout=40) as r: data=json.load(r)
   if not isinstance(data,list):raise ValueError(str(data))
   rec=dict(coin=coin,label=label,payload=payload,rows=len(data),first=iso(min(x['t'] for x in data)) if data else None,last=iso(max(x['t'] for x in data)) if data else None,attempt=attempt+1,retrieved_utc=iso(int(time.time()*1000)))
   log.append(rec);(OUT/'api_evidence.json').write_text(json.dumps(log,indent=2));print(rec,flush=True)
   time.sleep(.35);return data
  except Exception as e:
   print(coin,label,attempt,str(e),flush=True)
   if attempt==3:
    log.append(dict(coin=coin,label=label,payload=payload,error=str(e)));(OUT/'api_evidence.json').write_text(json.dumps(log,indent=2));return []
   time.sleep(2**attempt)
# Independent tests of both bounded windows and backward pagination.
for coin in ['BTC','ETH','SOL']:
 recent=call(coin,now-3*365*24*H,now-1,'three_year_start_recent_end')
 (OUT/f'{coin}_recent.json').write_text(json.dumps(recent))
 if recent:
  first=min(x['t'] for x in recent)
  call(coin,first-4999*H,first-1,'backward_page_before_first')
 for y in [2023,2024,2025]:
  t=int(datetime.datetime(y,10,1,tzinfo=datetime.timezone.utc).timestamp()*1000)
  call(coin,t,t+30*24*H-1,f'bounded_{y}_october')
# Existing cached coverage and 30d mean daily dollar volume, excludes last incomplete UTC day.
files=sorted(pathlib.Path('/tmp/hl-hist').glob('*.json'));coverage=[]
for f in files:
 d=json.loads(f.read_text());d=sorted({int(x['t']):x for x in d}.values(),key=lambda x:x['t'])
 if not d:continue
 end=(int(d[-1]['t'])//(24*H))*(24*H);dayvol={}
 for x in d:
  t=int(x['t']);day=t//(24*H)*(24*H)
  if end-30*24*H<=t<end:dayvol[day]=dayvol.get(day,0)+float(x['c'])*float(x['v'])
 mean=sum(dayvol.values())/30
 coverage.append(dict(coin=f.stem,hours=len(d),first=iso(d[0]['t']),last=iso(d[-1]['t']),missing_hours=int((d[-1]['t']-d[0]['t'])//H+1-len(d)),mean_daily_dvol_30d=mean,liquid=mean>=5e6))
(OUT/'coverage.json').write_text(json.dumps(coverage,indent=2))
# Retrieve latest full available window for every cached liquid name.
for r in coverage:
 if r['liquid'] and r['coin'] not in ['BTC','ETH','SOL']:
  d=call(r['coin'],now-3*365*24*H,now-1,'liquid_universe_full_request')
  (OUT/f"{r['coin']}_recent.json").write_text(json.dumps(d))
