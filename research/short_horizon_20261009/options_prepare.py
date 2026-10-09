"""Point-in-time contract selection and metadata, before inspecting payoffs."""
from datetime import datetime, timezone
from collections import Counter
from pathlib import Path
from urllib.parse import urlencode
import json
import math
import re
import sys
import time
from fetch_options import OUT, RAW, BASE, request, trades, digest

DAY = 86400000


def ordinary(t):
    return not any(t.get(k) for k in ['combo_id', 'combo_trade_id', 'block_trade_id'])


def parse(name):
    m = re.fullmatch(r'(BTC|ETH)-(\d{1,2}[A-Z]{3}\d{2})-(\d+(?:\.\d+)?)-(C|P)', name)
    if not m:
        return None
    expiry = datetime.strptime(m[2], '%d%b%y').replace(hour=8, tzinfo=timezone.utc)
    return m[1], int(expiry.timestamp()*1000), float(m[3]), m[4]


def select(rows, decision, expiry):
    past = [t for t in rows if decision-3600000 <= t['timestamp'] < decision and ordinary(t)]
    if not past or decision-past[-1]['timestamp'] > 600000:
        return {'status':'no_recent_index'}
    index = float(past[-1]['index_price'])
    pairs = {}
    for t in past:
        p = parse(t['instrument_name'])
        if p and p[1] == expiry:
            pairs.setdefault(p[2], {})[p[3]] = t
    eligible = [k for k, pair in pairs.items() if len(pair)==2]
    if not eligible:
        return {'status':'no_pair_in_prior_hour', 'index':index}
    strike = min(eligible, key=lambda k:(abs(k-index), k))
    return {'status':'selected', 'index':index, 'index_timestamp':past[-1]['timestamp'], 'strike':strike, 'legs':pairs[strike], 'eligible_pairs':len(eligible)}


def tick(meta, price):
    size = float(meta['tick_size'])
    for step in sorted(meta.get('tick_size_steps', []), key=lambda x:x['above_price']):
        if price > float(step['above_price']):
            size = float(step['tick_size'])
    return size


def round_up(meta, price):
    size = tick(meta, price)
    value = math.ceil(price/size-1e-10)*size
    return math.ceil(value/tick(meta, value)-1e-10)*tick(meta, value)


def main():
    events = json.loads((OUT/'events.json').read_text())
    records = []
    for e in events:
        base = int(datetime.fromisoformat(e['date']).replace(tzinfo=timezone.utc).timestamp()*1000)
        for coin in ['BTC', 'ETH']:
            source = OUT / f"{e['date']}_{coin}_entry.json"
            if '--wait-for-entry' in sys.argv:
                started = time.monotonic()
                while not source.exists() and time.monotonic()-started < 600:
                    manifest_path = OUT/'entry_manifest.json'
                    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else []
                    if any(a['event']==e['date'] and a['coin']==coin and a['status']=='missing' for a in manifest):
                        break
                    if (OUT/'spec_manifest.json').exists():
                        break
                    time.sleep(2)
            rows = json.loads(source.read_text()) if source.exists() else None
            for hold in [1, 2]:
                expiry = base+hold*DAY+8*3600000
                for hour in [8, 9, 10]:
                    decision = base+(hour*60+5)*60000
                    r = dict(event=e['date'], coin=coin, hold=hold, hour=hour, decision=decision, expiry=expiry)
                    if rows is None:
                        r['status'] = 'source_missing'
                    else:
                        r.update(select(rows, decision, expiry))
                    if r['status'] == 'selected':
                        try:
                            r['metadata'] = {}
                            for side, t in r['legs'].items():
                                name = t['instrument_name']
                                meta = request(BASE+'get_instrument?'+urlencode({'instrument_name':name}), RAW/(name+'_definition.json'))['result']
                                assert meta['instrument_name'] == name
                                assert meta['creation_timestamp'] < decision
                                assert meta['expiration_timestamp'] == expiry
                                assert meta['base_currency'] == coin and meta['settlement_currency'] == coin
                                assert meta['kind']=='option' and meta['strike']==r['strike']
                                assert meta['contract_size']==1
                                r['metadata'][side] = meta
                            if hold == 2:
                                r['interim_marks'] = {}
                                middle = base+DAY+8*3600000
                                for side, t in r['legs'].items():
                                    markrows = trades(coin, middle-3600000, middle, t['instrument_name'])
                                    valid = sorted([a for a in markrows if a.get('mark_price') is not None], key=lambda a:a['timestamp'])
                                    r['interim_marks'][side] = valid[-1] if valid else None
                        except Exception as exc:
                            r['status'] = 'metadata_or_mark_source_error'
                            r['error'] = str(exc)[:400]
                    records.append(r)
                    (OUT/'selections.json').write_text(json.dumps(records, indent=2))
            print(e['date'], coin, dict(Counter(r['status'] for r in records if r['event']==e['date'] and r['coin']==coin)), flush=True)
    (OUT/'selection_summary.json').write_text(json.dumps({'counts':dict(Counter(r['status'] for r in records)), 'records':len(records), 'sha256':digest(OUT/'selections.json'), 'performance_tested':False}, indent=2))


if __name__ == '__main__':
    main()
