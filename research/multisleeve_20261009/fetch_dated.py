"""Quarterly inverse futures data, expiries fixed by the exchange calendar."""
from pathlib import Path
import calendar
import json
import subprocess
import time
import pandas as pd
from concurrent.futures import ThreadPoolExecutor, as_completed

OUT = Path(__file__).resolve().parent
RAW = OUT / 'raw'
START = 1685577600000
END = 1791331200000  # October 7 00 UTC


def fetch(name, url):
    f = RAW / name
    if f.exists():
        try:
            d = json.loads(f.read_text())
            if 'result' in d:
                return d['result']
        except ValueError:
            pass
    for attempt in range(3):
        p = subprocess.run(['curl', '--max-time', '30', '--connect-timeout', '8', '-sS', url], capture_output=True)
        try:
            d = json.loads(p.stdout)
            assert 'result' in d, str(d)[:200]
            f.write_bytes(p.stdout)
            time.sleep(.6)
            return d['result']
        except Exception as e:
            if attempt == 2:
                raise RuntimeError(f'{name}: {e}')
            time.sleep(2+attempt)


def main():
    expiries = []
    for year in range(2023, 2027):
        for month in [3, 6, 9, 12]:
            last = pd.Timestamp(year=year, month=month, day=calendar.monthrange(year, month)[1], hour=8, tz='UTC')
            last -= pd.Timedelta(days=(last.weekday()-4)%7)
            expiries.append(last)
    jobs, records = [], []
    for c in ['BTC', 'ETH']:
        for prev, expiry in zip(expiries, expiries[1:]):
            begin = max(START, int(prev.timestamp()*1000))
            end = min(END, int(expiry.timestamp()*1000))
            if begin >= end:
                continue
            inst = c+'-'+expiry.strftime('%d%b%y').upper().lstrip('0')
            name = f'dated_{inst}.json'
            url = f'https://history.deribit.com/api/v2/public/get_tradingview_chart_data?instrument_name={inst}&start_timestamp={begin}&end_timestamp={end}&resolution=60'
            job = dict(coin=c, instrument=inst, begin=begin, expiry=int(expiry.timestamp()*1000), end=end, file=name)
            jobs.append((job, url))
    with ThreadPoolExecutor(max_workers=2) as pool:
        futures = {pool.submit(fetch, j['file'], url): j for j, url in jobs}
        for future in as_completed(futures):
            job = futures[future]
            try:
                r = future.result()
                job.update(rows=len(r.get('ticks', [])), status=r.get('status'))
            except Exception as e:
                job['error'] = str(e)
            records.append(job)
            print(job, flush=True)
    for c in ['BTC', 'ETH']:
        delivery = []
        for offset in range(0, 1300, 100):
            r = fetch(f'delivery_{c}_{offset}.json', f'https://www.deribit.com/api/v2/public/get_delivery_prices?index_name={c.lower()}_usd&count=100&offset={offset}')
            delivery.extend(r['data'])
        (OUT / f'delivery_{c}.json').write_text(json.dumps(delivery))
    (OUT / 'dated_manifest.json').write_text(json.dumps(records, indent=2))
    p = subprocess.run(['curl', '--max-time', '30', '-sS', 'https://fred.stlouisfed.org/graph/fredgraph.csv?id=SOFR&cosd=2023-06-01&coed=2026-10-08'], capture_output=True)
    (RAW / 'sofr.csv').write_bytes(p.stdout)
    print('SOFR', p.stdout[:150], flush=True)


if __name__ == '__main__':
    main()
