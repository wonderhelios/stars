"""Read-only public prices. Fixed cutoff and immutable successful responses."""
import json, time, hashlib
from datetime import datetime, timezone
from pathlib import Path
import requests

ROOT = Path(__file__).resolve().parent
RAW = ROOT / 'raw'
STOCKS = 'GOOGL MSFT AAPL AMZN META NVDA AMD INTC ORCL CRM ADBE INTU NOW VEEV GWRE TYL ADSK ANSS CDNS SNPS PAYC PCTY WDAY TEAM DDOG ZS CRWD NET MDB HUBS'.split()
ETFS = 'SPY QQQ IWM EFA EEM TLT IEF GLD SLV USO XLE XLF XLK XLV XLU XLP XLI'.split()
START = int(datetime(2013, 1, 1, tzinfo=timezone.utc).timestamp())
END = int(datetime(2026, 10, 8, tzinfo=timezone.utc).timestamp())

def main():
    RAW.mkdir(exist_ok=True)
    manifest = []
    for symbol in STOCKS + ETFS:
        path = RAW / f'{symbol}.json'
        url = f'https://query1.finance.yahoo.com/v8/finance/chart/{symbol}'
        params = dict(period1=START, period2=END, interval='1d', events='div,splits')
        if not path.exists():
            for attempt in range(3):
                r = None
                try:
                    r = requests.get(url, params=params, headers={'User-Agent': 'Mozilla/5.0'}, timeout=30)
                    r.raise_for_status()
                    d = r.json()
                    assert d['chart']['result'] and d['chart']['result'][0].get('timestamp')
                    path.write_text(json.dumps(d))
                    break
                except Exception as exc:
                    print(symbol, attempt, str(exc)[:180], flush=True)
                    if r is not None and r.status_code in (401, 403, 404, 429):
                        break
                    time.sleep(2 + attempt)
            time.sleep(1)
        rec = dict(symbol=symbol, url=url, params=params, retrieved_utc=datetime.now(timezone.utc).isoformat())
        if path.exists():
            d = json.loads(path.read_text())['chart']['result'][0]
            rec.update(sha256=hashlib.sha256(path.read_bytes()).hexdigest(), rows=len(d['timestamp']), first=d['timestamp'][0], last=d['timestamp'][-1])
        else:
            rec['error'] = 'Public chart unavailable; not replaced with synthetic history.'
        manifest.append(rec)
        (ROOT / 'data_manifest.json').write_text(json.dumps(manifest, indent=2))
        print(symbol, rec.get('rows', rec.get('error')), flush=True)

if __name__ == '__main__':
    main()
