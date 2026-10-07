"""Archive immutable API bodies plus retrieval times/headers; historical timestamps are NOT availability timestamps."""
from pathlib import Path
import subprocess,json,hashlib,datetime,concurrent.futures
OUT=Path(__file__).resolve().parent;D=OUT/'data';D.mkdir(exist_ok=True)
chains=['Ethereum','Solana','Arbitrum','Optimism','Avalanche','BSC','Polygon','Sui','Aptos','Near','Cosmos','Cardano','Fantom','Tron','Injective','Sei','Mantle','Ton']
jobs={'stable_usdt':'https://stablecoins.llama.fi/stablecoin/1','stable_usdc':'https://stablecoins.llama.fi/stablecoin/2','stable_all':'https://stablecoins.llama.fi/stablecoincharts/all','stable_list':'https://stablecoins.llama.fi/stablecoins','chains':'https://api.llama.fi/v2/chains','etherscan_no_key':'https://api.etherscan.io/v2/api?chainid=1&module=account&action=txlist&address=0x28C6c06298d514Db089934071355E5743bf21d60&startblock=0&endblock=99999999&page=1&offset=1&sort=desc&apikey='}
for c in chains:
 jobs['tvl_'+c]='https://api.llama.fi/v2/historicalChainTvl/'+c
 jobs['stable_'+c]='https://stablecoins.llama.fi/stablecoincharts/'+c

def get(item):
 key,url=item;body=D/(key+'.json');h=D/(key+'.headers');start=datetime.datetime.now(datetime.timezone.utc).isoformat()
 if body.exists():
  try:
   json.loads(body.read_text())
   return key,{'url':url,'cached':True,'sha256':hashlib.sha256(body.read_bytes()).hexdigest()}
  except Exception:pass
 r=subprocess.run(['curl','--silent','--show-error','--location','--compressed','--retry','1','--max-time','120','--proxy','http://127.0.0.1:7897','-D',str(h),'-o',str(body),url],capture_output=True,text=True)
 m={'url':url,'retrieved_start_utc':start,'retrieved_end_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'returncode':r.returncode,'error':r.stderr[:300]}
 if body.exists():
  m['sha256']=hashlib.sha256(body.read_bytes()).hexdigest()
  try:
   j=json.loads(body.read_text());m['type']=type(j).__name__;m['length']=len(j);m['sample']=str(j)[:250]
  except Exception as e:m['parse_error']=str(e)
 print(key,m.get('type',m.get('error')),flush=True);return key,m
with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:meta=dict(pool.map(get,jobs.items()))
json.dump(meta,open(OUT/'api_audit.json','w'),indent=2)
