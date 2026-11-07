"""拉 Hyperliquid 资金费率（fundingRate + premium）→ ml/data/hl_funding/。
并发版：16 线程并发 + 每币翻页进度。"""
from pathlib import Path
BASE = Path(__file__).resolve().parent
import json, urllib.request, datetime, time, os
from concurrent.futures import ThreadPoolExecutor, as_completed

HEAD = ['BTC','ETH','SOL','BNB','XRP','ADA','DOGE','AVAX','LINK','SUI','NEAR','UNI','AAVE',
        'INJ','TAO','XMR','ZEC','ARB','STRK','CRV','ENA','WLD','ZRO','ONDO','HYPE','PUMP',
        'FARTCOIN','kPEPE','CASHCAT','PONS','LIT','VVV','XPL']

def hl_info(body):
    req = urllib.request.Request('https://api.hyperliquid.xyz/info',
        data=json.dumps(body).encode(), headers={'Content-Type': 'application/json'})
    return json.load(urllib.request.urlopen(req, timeout=30))

out_dir = BASE / 'data' / 'hl_funding'
os.makedirs(out_dir, exist_ok=True)
start_ms = int(datetime.datetime(2023, 5, 1).timestamp() * 1000)
end_ms = int(datetime.datetime.now(datetime.UTC).timestamp() * 1000)

def pull(coin):
    out = []
    cur = start_ms
    guard = 0
    while cur < end_ms and guard < 300:
        try:
            r = hl_info({'type': 'fundingHistory', 'coin': coin, 'startTime': cur})
        except Exception:
            time.sleep(1)
            continue
        if not r:
            break
        out.extend(r)
        cur = r[-1]['time'] + 1
        if len(r) < 2:
            break
        guard += 1
    daily = {}
    for x in out:
        day = datetime.datetime.fromtimestamp(int(x['time'])/1000, datetime.UTC).strftime('%Y-%m-%d')
        d = daily.setdefault(day, {'fr': [], 'prem': []})
        try:
            d['fr'].append(float(x['fundingRate']))
        except Exception:
            pass
        try:
            d['prem'].append(float(x['premium']))
        except Exception:
            pass
    agg = {dd: {'fr': sum(v['fr'])/len(v['fr']) if v['fr'] else None,
                'prem': sum(v['prem'])/len(v['prem']) if v['prem'] else None}
           for dd, v in daily.items()}
    return coin, agg

with ThreadPoolExecutor(max_workers=16) as ex:
    futs = {ex.submit(pull, c): c for c in HEAD}
    done = 0
    for f in as_completed(futs):
        coin, agg = f.result()
        if agg:
            json.dump(agg, open(str(out_dir / f'{coin}.json'), 'w'))
        done += 1
        print(f"[{done}/{len(HEAD)}] {coin} {len(agg)}天", flush=True)
print("funding 回补完成")
