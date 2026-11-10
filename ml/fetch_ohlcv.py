"""拉 Hyperliquid 日线 OHLCV → ml/data/hl2/。
增量：已有数据只拉最后日期之后的新数据，合并去重；无数据才全量。"""
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
# 向下对齐到日线边界 ✗ 排除【当前未闭合的 bar】（否则 08:10 跑时会把当天未走完的日线当成完整数据 ✗ 特征/选币都会偏）
_DAY_MS = 86400000
end = (end // _DAY_MS) * _DAY_MS
full_start = int(datetime.datetime(2023, 2, 26).timestamp() * 1000)  # 和 alphapilot 数据起点一致
out_dir = BASE / 'data' / 'hl2'
os.makedirs(out_dir, exist_ok=True)

def fetch_one(c):
    p = str(out_dir / f'{c}.json')
    existing = []
    if os.path.exists(p):
        try:
            existing = json.load(open(p))
        except Exception:
            existing = []
    last_t = max((int(x['t']) for x in existing), default=0)
    # 增量：从最后一天之后拉；保留 2 天重叠（防漏）
    req_start = max(full_start, last_t - 2 * 86400000) if existing else full_start
    if existing and req_start >= end:
        return c, 0
    cs = post("https://api.hyperliquid.xyz/info",
              {"type": "candleSnapshot", "req": {"coin": c, "interval": "1d",
                                                 "startTime": req_start, "endTime": end}})
    if not cs:
        return c, 0
    by_t = {int(x['t']): x for x in existing}
    for x in cs:
        by_t[int(x['t'])] = x
    merged = sorted(by_t.values(), key=lambda x: int(x['t']))
    json.dump(merged, open(p, 'w'))
    return c, len(cs)

cnt = 0
with ThreadPoolExecutor(max_workers=16) as ex:
    futs = {ex.submit(fetch_one, c): c for c in universe}
    done = 0
    for f in as_completed(futs):
        done += 1
        if f.result()[1] > 0:
            cnt += 1
        if done % 20 == 0:
            print(f"  {done}/{len(universe)}（更新 {cnt}）", flush=True)
print(f"完成 {cnt} 币", flush=True)
