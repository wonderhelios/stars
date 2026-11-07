"""下载 OI 月度 parquet（2023-04 ~ 2026-11）→ ml/data/hl_oi/。"""
from pathlib import Path
BASE = Path(__file__).resolve().parent
import urllib.request, os

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

for i, mm in enumerate(months):
    fp = str(out_dir / f'{mm}.parquet')
    if os.path.exists(fp) and os.path.getsize(fp) > 10000:
        print(f"[{i+1}/{len(months)}] {mm} 已存在", flush=True)
        continue
    url = f'https://www.getopendata.io/v1/hyperliquid/perps/oi/1h/{mm}.parquet'
    try:
        req = urllib.request.Request(url, headers={'User-Agent': 'Mozilla/5.0'})
        data = urllib.request.urlopen(req, timeout=60).read()
        open(fp, 'wb').write(data)
        print(f"[{i+1}/{len(months)}] {mm} OK {len(data)}B", flush=True)
    except Exception as e:
        print(f"[{i+1}/{len(months)}] {mm} 失败 {e}", flush=True)
print("完成")
