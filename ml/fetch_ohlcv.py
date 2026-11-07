"""拉 Hyperliquid 日线 OHLCV（2023-04 起）→ ml/data/hl2/。"""
from pathlib import Path
BASE = Path(__file__).resolve().parent
import json, urllib.request, time, datetime, os

def post(url, body):
    req = urllib.request.Request(url, data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    for _ in range(4):
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                return json.load(r)
        except Exception:
            time.sleep(1.5)
    return None

meta = post("https://api.hyperliquid.xyz/info", {"type": "metaAndAssetCtxs"})
universe = [m['name'] for m in meta[0]['universe'] if ':' not in m['name']]
print(f"宇宙 {len(universe)} 币", flush=True)

end = int(datetime.datetime.utcnow().timestamp() * 1000)
start = int(datetime.datetime(2023, 4, 1).timestamp() * 1000)
out_dir = BASE / 'data' / 'hl2'
os.makedirs(out_dir, exist_ok=True)

cnt = 0
for c in universe:
    p = str(out_dir / f'{c}.json')
    try:
        cs = post("https://api.hyperliquid.xyz/info",
                  {"type": "candleSnapshot", "req": {"coin": c, "interval": "1d",
                                                     "startTime": start, "endTime": end}})
        if cs:
            json.dump(cs, open(p, 'w'))
            cnt += 1
    except Exception:
        pass
    if cnt % 30 == 0:
        print(f"  {cnt}/{len(universe)}", flush=True)
    time.sleep(0.04)
print(f"完成 {cnt} 币", flush=True)
