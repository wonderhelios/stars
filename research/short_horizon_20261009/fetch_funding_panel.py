"""Public Binance settled funding and matching perpetual daily prices.

Data collection only. No predicted rates, spot substitution, proxy changes, keys,
or trading endpoints. Empty historical funding mark prices remain missing.
"""
from pathlib import Path
from datetime import datetime, timezone
from urllib.parse import urlencode
import subprocess
import json
import time
import hashlib

ROOT=Path(__file__).resolve().parent
OUT=ROOT/'funding_panel'
RAW=OUT/'raw'
RAW.mkdir(parents=True,exist_ok=True)
COINS='BTC ETH SOL XRP DOGE ADA AVAX LINK LTC BCH DOT ATOM BNB NEAR APT ARB'.split()
START=1672531200000
END=1791417600000  # 2026-10-08 00 UTC, exclusive


def save(path,data):
    temp=path.with_suffix(path.suffix+'.tmp')
    temp.write_text(json.dumps(data,indent=2))
    temp.replace(path)


def get(endpoint,params,stem):
    path=RAW/(stem+'.json')
    if path.exists():
        return json.loads(path.read_text())
    url='https://fapi.binance.com/fapi/v1/'+endpoint+'?'+urlencode(params)
    for attempt in range(3):
        temp=RAW/(stem+'.download')
        p=subprocess.run(['curl','-sS','--connect-timeout','8','--max-time','30','-w','%{http_code}','-o',str(temp),url],capture_output=True)
        status=p.stdout.decode()
        evidence=dict(url=url,http=status,attempt=attempt,retrieved_utc=datetime.now(timezone.utc).isoformat(),error=p.stderr.decode()[:300])
        with (OUT/'requests.jsonl').open('a') as f:f.write(json.dumps(evidence)+'\n')
        if status in ['403','418','429','451']:
            raise PermissionError('Stop source on access/rate restriction '+status)
        if p.returncode==0 and status=='200':
            data=json.loads(temp.read_text())
            assert isinstance(data,list),str(data)[:300]
            temp.replace(path)
            time.sleep(.6)
            return data
        if attempt==2:
            raise RuntimeError(str(evidence))
        time.sleep(2+attempt)


def main():
    manifest=[]
    for coin in COINS:
        symbol=coin+'USDT'
        record=dict(coin=coin,symbol=symbol)
        try:
            rows=[];cursor=START
            while cursor<END:
                batch=get('fundingRate',dict(symbol=symbol,startTime=cursor,endTime=END-1,limit=1000),f'funding_{coin}_{cursor}')
                if not batch:break
                assert all(cursor<=r['fundingTime']<END and r['symbol']==symbol for r in batch)
                rows.extend(batch)
                cursor=max(r['fundingTime'] for r in batch)+1
                if len(batch)<1000:break
            rows.sort(key=lambda r:r['fundingTime'])
            assert len({r['fundingTime'] for r in rows})==len(rows)
            save(OUT/(coin+'_funding.json'),rows)
            price=get('klines',dict(symbol=symbol,interval='1d',startTime=START,endTime=END-1,limit=1500),f'prices_{coin}')
            assert len(price)<1500, 'Price pagination required'
            price=[r for r in price if int(r[0])+86400000<=END]
            assert len({r[0] for r in price})==len(price)
            save(OUT/(coin+'_daily.json'),price)
            record.update(status='complete',funding_rows=len(rows),price_rows=len(price),funding_start=rows[0]['fundingTime'] if rows else None,funding_end=rows[-1]['fundingTime'] if rows else None,missing_mark_prices=sum(not r.get('markPrice') for r in rows),funding_sha256=hashlib.sha256((OUT/(coin+'_funding.json')).read_bytes()).hexdigest(),price_sha256=hashlib.sha256((OUT/(coin+'_daily.json')).read_bytes()).hexdigest())
        except PermissionError:
            raise
        except Exception as exc:
            record.update(status='missing',error=str(exc)[:400])
        manifest.append(record)
        save(OUT/'manifest.json',manifest)
        print(record,flush=True)
    print('Data collection complete. No funding-panel performance tested.',flush=True)


if __name__=='__main__':main()
