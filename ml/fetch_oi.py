"""下载 OI 月度 parquet（2023-04 ~ 2026-11）→ ml/data/hl_oi/。
并发版：8 线程并发下载。"""
from pathlib import Path
BASE = Path(__file__).resolve().parent
import urllib.request, os
from concurrent.futures import ThreadPoolExecutor, as_completed

out_dir = BASE / 'data' / 'hl_oi'
os.makedirs(out_dir, exist_ok=True)

months = []
for y in (2023, 2024, 2025, 2026):
    for m in range(1, 13):
        if y == 2023 and m < 4:
            continue
        if y == 2026 and m > 11:
            continue
        months.append(f"{y}-{m:02d}")

def fetch(mm):
    fp = str(out_dir / f'{mm}.parquet')
    if os.path.exists(fp) and os.path.getsize(fp) > 10000:
        return mm, "已存在"
    url = f'https://www.getopendata.io/v1/hyperliquid/perps/oi/1h/{mm}.parquet'
    try:
        req = urllib.request.Request(url, headers={'User-Agent': 'Mozilla/5.0'})
        data = urllib.request.urlopen(req, timeout=60).read()
        open(fp, 'wb').write(data)
        return mm, f"OK {len(data)}B"
    except Exception as e:
        return mm, f"失败 {e}"

with ThreadPoolExecutor(max_workers=8) as ex:
    futs = {ex.submit(fetch, mm): mm for mm in months}
    done = 0
    for f in as_completed(futs):
        mm, status = f.result()
        done += 1
        print(f"[{done}/{len(months)}] {mm} {status}", flush=True)
print("完成")
