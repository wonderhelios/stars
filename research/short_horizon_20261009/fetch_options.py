"""Public FOMC calendar and raw option trades; no performance calculation."""
from pathlib import Path
from urllib.parse import urlencode
from datetime import datetime, timezone
from collections import Counter
import hashlib
import json
import re
import subprocess
import time
from bs4 import BeautifulSoup

ROOT = Path(__file__).resolve().parent
OUT = ROOT / 'options_events'
RAW = OUT / 'raw'
RAW.mkdir(parents=True, exist_ok=True)
BASE = 'https://history.deribit.com/api/v2/public/'


def digest(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()


def request(url, path, is_json=True):
    if path.exists():
        return json.loads(path.read_text()) if is_json else path.read_text()
    for attempt in range(3):
        p = subprocess.run(['curl', '-sS', '--max-time', '40', '--connect-timeout', '10', url], capture_output=True)
        try:
            if p.returncode:
                raise RuntimeError(p.stderr.decode()[:200])
            result = json.loads(p.stdout) if is_json else p.stdout.decode()
            if is_json and ('error' in result or 'result' not in result):
                raise RuntimeError(str(result)[:300])
            if not is_json and 'fomc-meeting' not in result:
                raise RuntimeError('Expected calendar markup absent')
            path.write_bytes(p.stdout)
            path.with_suffix(path.suffix+'.source.json').write_text(json.dumps({'url': url, 'retrieved_utc': datetime.now(timezone.utc).isoformat(), 'sha256': digest(path)}, indent=2))
            time.sleep(.8)
            return result
        except Exception:
            if attempt == 2:
                raise
            time.sleep(2 + attempt)


def calendar():
    url = 'https://www.federalreserve.gov/monetarypolicy/fomccalendars.htm'
    html = request(url, RAW / 'fomccalendars.html', False)
    soup = BeautifulSoup(html, 'html.parser')
    events = []
    for block in soup.select('.fomc-meeting'):
        if 'notation vote' in block.get_text(' ', strip=True).lower():
            continue
        for a in block.find_all('a', href=True):
            m = re.fullmatch(r'/newsevents/pressreleases/monetary(\d{8})a.htm', a['href'])
            if not m:
                continue
            date = datetime.strptime(m[1], '%Y%m%d').date().isoformat()
            if '2023-06-01' <= date <= '2026-10-07':
                events.append({'date': date, 'source': 'https://www.federalreserve.gov'+a['href'], 'calendar_text': block.get_text(' ', strip=True)})
    events = sorted({e['date']: e for e in events}.values(), key=lambda e:e['date'])
    if not events:
        raise ValueError('No meeting statements parsed')
    (OUT / 'events.json').write_text(json.dumps(events, indent=2))
    return events


def trades(coin, begin, end, instrument=None):
    """Inclusive disjoint interval recursion avoids losing same-ms trades."""
    params = dict(start_timestamp=begin, end_timestamp=end, count=1000, sorting='asc', include_old='true')
    if instrument:
        endpoint = 'get_last_trades_by_instrument_and_time'
        params['instrument_name'] = instrument
        stem = instrument
    else:
        endpoint = 'get_last_trades_by_currency_and_time'
        params.update(currency=coin, kind='option')
        stem = coin
    p = RAW / f'{stem}_{begin}_{end}.json'
    data = request(BASE+endpoint+'?'+urlencode(params), p)['result']
    rows = data['trades']
    if data.get('has_more'):
        if begin >= end:
            raise RuntimeError('More than 1000 trades at one millisecond; incomplete interval')
        mid = (begin+end)//2
        return trades(coin, begin, mid, instrument)+trades(coin, mid+1, end, instrument)
    assert all(begin <= t['timestamp'] <= end for t in rows)
    ids = [str(t['trade_id']) for t in rows]
    assert len(ids) == len(set(ids)), 'Duplicate trade ids in a leaf'
    return rows


def main():
    events = calendar()
    print('Events:', [e['date'] for e in events], flush=True)
    manifest = []
    for e in events:
        base = int(datetime.fromisoformat(e['date']).replace(tzinfo=timezone.utc).timestamp()*1000)
        for coin in ['BTC', 'ETH']:
            record = dict(event=e['date'], coin=coin)
            p = OUT / f"{e['date']}_{coin}_entry.json"
            try:
                if p.exists():
                    rows = json.loads(p.read_text())
                else:
                    rows = trades(coin, base+7*3600000, base+(10*60+20)*60000)
                    rows.sort(key=lambda t:(t['timestamp'], str(t['trade_id'])))
                    assert len({str(t['trade_id']) for t in rows}) == len(rows)
                    p.write_text(json.dumps(rows, separators=(',', ':')))
                counts = Counter('block' if t.get('block_trade_id') else 'combo' if t.get('combo_trade_id') else 'ordinary' for t in rows)
                record.update(rows=len(rows), categories=dict(counts), file=p.name, sha256=digest(p), status='complete')
            except Exception as exc:
                record.update(status='missing', error=str(exc)[:400])
            manifest.append(record)
            (OUT / 'entry_manifest.json').write_text(json.dumps(manifest, indent=2))
            print(record, flush=True)
    (OUT / 'spec_manifest.json').write_text(json.dumps({'spec_sha256':digest(ROOT/'OPTIONS_SPEC.md'), 'fetch_code_sha256':digest(Path(__file__)), 'events':len(events), 'complete':sum(r['status']=='complete' for r in manifest)}, indent=2))


if __name__ == '__main__':
    main()
