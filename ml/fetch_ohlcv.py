"""拉 Hyperliquid 日线 OHLCV → ml/data/hl2/。
增量：已有数据只拉最后日期之后的新数据，合并去重；无数据才全量。"""
from pathlib import Path
BASE = Path(__file__).resolve().parent
import json, urllib.request, time, datetime, os
from concurrent.futures import ThreadPoolExecutor, as_completed
from candle_rules import DAY_MS, daily_request_end, merge_closed_daily_bars

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

now_ms = int(datetime.datetime.now(datetime.UTC).timestamp() * 1000)
# Hyperliquid endTime 是包含式边界：必须减 1ms，否则日界整点也会返回当天未闭合 bar。
end = daily_request_end(now_ms)
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
    # 旧文件可能已被未闭合日线污染；计算增量起点前先剔除。
    existing = merge_closed_daily_bars(existing, [], now_ms)
    last_t = max((int(x['t']) for x in existing), default=0)
    # 增量：从最后一天之后拉；保留 2 天重叠（防漏）
    req_start = max(full_start, last_t - 2 * DAY_MS) if existing else full_start
    if existing and req_start >= end:
        json.dump(existing, open(p, 'w'))
        return c, 0
    cs = post("https://api.hyperliquid.xyz/info",
              {"type": "candleSnapshot", "req": {"coin": c, "interval": "1d",
                                                 "startTime": req_start, "endTime": end}})
    if not cs:
        if existing or os.path.exists(p):
            json.dump(existing, open(p, 'w'))
        return c, 0
    # 请求和本地合并双重过滤，避免包含式边界或历史快照污染特征。
    merged = merge_closed_daily_bars(existing, cs, now_ms)
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
