"""Read-only public market data; resumable cache, no credentials or orders."""
from pathlib import Path
import concurrent.futures
import hashlib
import json
import subprocess
import threading
import time

OUT = Path(__file__).resolve().parent
RAW = OUT / 'raw'
RAW.mkdir(exist_ok=True)
START = 1685577600000  # 2023-06-01 UTC
END = 1791417600000    # 2026-10-08 UTC, matches frozen baseline endpoint
COINS = ['BTC', 'ETH', 'SOL']
LOCK = threading.Lock()
LAST_REQUEST = 0.


def get(url, payload=None):
    global LAST_REQUEST
    args = ['curl', '--connect-timeout', '8', '--max-time', '25', '-sS', url]
    if payload is not None:
        args += ['-H', 'Content-Type: application/json', '-d', json.dumps(payload)]
    for attempt in range(3):
        with LOCK:
            time.sleep(max(0., 1.2 - (time.monotonic() - LAST_REQUEST)))
            LAST_REQUEST = time.monotonic()
        p = subprocess.run(args, capture_output=True)
        try:
            if p.returncode:
                raise ValueError(p.stderr.decode()[:200])
            data = json.loads(p.stdout)
            if isinstance(data, dict) and ('error' in data or data.get('code', 0) < 0):
                raise ValueError(str(data)[:300])
            if not isinstance(data, list):
                raise ValueError(str(data)[:300])
            return data
        except (ValueError, TypeError) as e:
            if attempt == 2:
                raise RuntimeError(f'{url}: {e}')
            time.sleep(10 * (1 + attempt))


def funding(coin):
    cursor = START
    rows = []
    pages = 0
    while cursor < END:
        f = RAW / f'funding_{coin}_{cursor}.json'
        if f.exists():
            d = json.loads(f.read_text())
        else:
            d = get('https://api.hyperliquid.xyz/info',
                    dict(type='fundingHistory', coin=coin, startTime=cursor, endTime=END))
            assert isinstance(d, list)
            f.write_text(json.dumps(d))
            time.sleep(.25)
        if not d:
            break
        assert all(x['coin'] == coin and cursor <= x['time'] <= END for x in d)
        rows.extend(d)
        nxt = max(x['time'] for x in d) + 1
        assert nxt > cursor
        cursor = nxt
        pages += 1
        if pages % 10 == 0:
            print('funding', coin, pages, len(rows), cursor, flush=True)
        if len(d) < 500:
            break
    unique = {x['time']: x for x in rows}
    rows = [unique[t] for t in sorted(unique)]
    (OUT / f'funding_{coin}.json').write_text(json.dumps(rows))
    return dict(coin=coin, funding_rows=len(rows), start=rows[0]['time'], end=rows[-1]['time'])


def spot(symbol):
    cursor, rows = START, []
    while cursor <= END:
        f = RAW / f'spot_{symbol}_{cursor}.json'
        if f.exists():
            d = json.loads(f.read_text())
        else:
            url = f'https://api.binance.com/api/v3/klines?symbol={symbol}&interval=1d&startTime={cursor}&endTime={END}&limit=1000'
            d = get(url)
            assert isinstance(d, list)
            f.write_text(json.dumps(d))
            time.sleep(.3)
        if not d:
            break
        rows.extend(d)
        cursor = max(x[0] for x in d) + 86400000
        if len(d) < 1000:
            break
    (OUT / f'spot_{symbol}.json').write_text(json.dumps(rows))
    return dict(symbol=symbol, spot_rows=len(rows), start=rows[0][0], end=rows[-1][0])


if __name__ == '__main__':
    log = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        futures = [pool.submit(funding, c) for c in COINS]
        futures += [pool.submit(spot, c + 'USDT') for c in COINS]
        futures += [pool.submit(spot, 'USDCUSDT')]
        for job in concurrent.futures.as_completed(futures):
            try:
                result = job.result()
            except Exception as e:
                result = dict(error=str(e))
            log.append(result)
            print(result, flush=True)
            (OUT / 'fetch_status.json').write_text(json.dumps(log, indent=2))
    files = sorted(RAW.glob('*.json')) + sorted(OUT.glob('funding_*.json')) + sorted(OUT.glob('spot_*.json'))
    (OUT / 'fetch_manifest.json').write_text(json.dumps({str(f.relative_to(OUT)): hashlib.sha256(f.read_bytes()).hexdigest() for f in files}, indent=2))
