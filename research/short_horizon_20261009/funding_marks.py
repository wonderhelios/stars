"""Fill absent funding mark references with timestamped official mark bars.

The bar open is an explicit near-time proxy, never relabeled as an exact mark.
"""
from datetime import datetime,timezone
import json
import time
import math
from fetch_funding_panel import OUT,COINS,get,save,END

HOUR=3600000


def main():
    manifest=[]
    for coin in COINS:
        f=OUT/(coin+'_funding.json')
        started=time.monotonic()
        while not f.exists():
            if time.monotonic()-started>1800:
                raise TimeoutError('Funding source not ready: '+coin)
            time.sleep(3)
        rows=json.loads(f.read_text())
        missing=[r for r in rows if not r.get('markPrice')]
        marks={}
        years=sorted({datetime.fromtimestamp(r['fundingTime']/1000,timezone.utc).year for r in missing})
        for year in years:
            group=[r for r in missing if datetime.fromtimestamp(r['fundingTime']/1000,timezone.utc).year==year]
            interval=next(h for h in [8,4,2,1] if all((r['fundingTime']//HOUR)%h==0 for r in group))
            assert all(r['fundingTime']%HOUR<60000 for r in group),'Funding is not near an hour boundary'
            begin=min(r['fundingTime']//HOUR*HOUR for r in group)
            end=max(r['fundingTime']//HOUR*HOUR for r in group)+interval*HOUR-1
            while begin<=end:
                until=min(end,begin+interval*HOUR*1499-1)
                batch=get('markPriceKlines',dict(symbol=coin+'USDT',interval=f'{interval}h',startTime=begin,endTime=until,limit=1500),f'mark_{coin}_{interval}h_{begin}_{until}')
                for b in batch:
                    assert begin<=int(b[0])<=until
                    marks[int(b[0])]=b
                begin=until+1
        combined=[]
        for r in rows:
            x=dict(r)
            if r.get('markPrice'):
                x.update(payment_mark=float(r['markPrice']),mark_source='funding_record',mark_time=r['fundingTime'],mark_lag_ms=0)
            else:
                key=r['fundingTime']//HOUR*HOUR
                if key not in marks:
                    raise ValueError(f'Missing official historical mark bar: {coin} {key}')
                x.update(payment_mark=float(marks[key][1]),mark_source='official_mark_bar_open_proxy',mark_time=key,mark_lag_ms=r['fundingTime']-key)
            assert x['payment_mark']>0
            combined.append(x)
        save(OUT/(coin+'_funding_with_marks.json'),combined)
        record=dict(coin=coin,rows=len(rows),exact_mark=len(rows)-len(missing),proxy_mark=len(missing),max_lag_ms=max([r['mark_lag_ms'] for r in combined]+[0]),status='complete')
        manifest.append(record);save(OUT/'marks_manifest.json',manifest);print(record,flush=True)
    print('All funding marks available with provenance; no strategy performance read.',flush=True)


if __name__=='__main__':main()
