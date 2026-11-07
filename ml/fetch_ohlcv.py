"""拉 Hyperliquid 日线 OHLCV（2023-04 起）→ ml/data/hl2/。
并发版：235 币 16 线程并发拉，比串行快很多。"""
from pathlib import Path
BASE = Path(__file__).resolve().parent
import json, urllib.request, time, datetime, os
from concurrent.futures import ThreadPoolExecutor, as_completed

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

end = int(datetime.datetime.now(datetime.UTC).timestamp() * 1000)
start = int(datetime.datetime(2023, 4, 1).timestamp() * 1000)
out_dir = BASE / 'data' / 'hl2'
os.makedirs(out_dir, exist_ok=True)

def fetch_one(c):
    try:
        cs = post("https://api.hyperliquid.xyz/info",
                  {"type": "candleSnapshot", "req": {"coin": c, "interval": "1d",
                                                     "startTime": start, "endTime": end}})
        if cs:
            json.dump(cs, open(str(out_dir / f'{c}.json'), 'w'))
            return True
    except Exception:
        pass
    return False

cnt = 0
with ThreadPoolExecutor(max_workers=16) as ex:
    futs = {ex.submit(fetch_one, c): c for c in universe}
    done = 0
    for f in as_completed(futs):
        done += 1
        if f.result():
            cnt += 1
        if done % 20 == 0:
            print(f"  {done}/{len(universe)}（成功 {cnt}）", flush=True)
print(f"完成 {cnt} 币", flush=True)
