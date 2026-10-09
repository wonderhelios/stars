"""Rebuild each closed trade's cash flows directly from public funding records."""
import json
import hashlib
from collections import Counter
import numpy as np
import pandas as pd
from fetch_funding_panel import OUT,COINS,END,save
from funding_screen import inputs,features,DAY,HOUR


def main():
    results=json.loads((OUT/'results.json').read_text());events=json.loads((OUT/'events.json').read_text())
    data,funding=inputs();targets,scores,eligible=features(data,funding)
    checked_marks=0;largest_error=0.;max_hold=0.;min_idle=None;trades=0;prefix=0
    for coin,rows in zip(COINS,funding):
        originals={r['fundingTime']:r for r in json.loads((OUT/(coin+'_funding.json')).read_text())}
        bars={}
        for f in (OUT/'raw').glob('mark_'+coin+'_*.json'):
            for b in json.loads(f.read_text()):bars[int(b[0])]=float(b[1])
        for r in rows:
            original=originals[r['fundingTime']]
            assert r['fundingRate']==original['fundingRate']
            if original.get('markPrice'):
                assert r['payment_mark']==float(original['markPrice']) and r['mark_source']=='funding_record'
            else:
                assert r['payment_mark']==bars[r['fundingTime']//HOUR*HOUR]
                assert r['mark_lag_ms']==r['fundingTime']%HOUR<60000
            checked_marks+=1
    for cutoff in ['2024-01-01','2025-01-01']:
        stop=int(pd.Timestamp(cutoff,tz='UTC').timestamp()*1000)
        count=int(np.searchsorted(data['times'],stop))
        sub={k:v if k=='names' else v[:count] for k,v in data.items()}
        past=[[r for r in rr if r['fundingTime']<stop] for rr in funding]
        a,b,c=features(sub,past)
        for key in targets:
            assert np.array_equal(a[key][:-1],targets[key][:count-1])
        assert np.allclose(b,scores[:count],equal_nan=True)
        prefix+=1
    bycoin=dict(zip(COINS,funding))
    for key,evs in events.items():
        label=key.rsplit('_',1)[-1];fee=.0015 if label=='stress' else .0008
        adverse=.01 if label=='markstress' else 0
        equity=100000.;last_exit=None
        for event in evs:
            enter=event['entry_time'];exit_=event['exit_time']
            hold=(exit_-enter)/DAY
            assert hold in [1,2]
            max_hold=max(max_hold,hold*24)
            if last_exit is not None:
                idle=(enter-last_exit)/DAY
                assert idle>=1
                min_idle=idle if min_idle is None else min(idle,min_idle)
            last_exit=exit_
            q=np.array(event['quantities']);o=np.array(event['entry_prices']);c=np.array(event['exit_prices'])
            entryfee=float(np.abs(q)@o)*fee;exitfee=float(np.abs(q)@c)*fee
            assert abs(entryfee-event['entry_fee'])<1e-7
            assert abs(exitfee-event['exit_fee'])<1e-7
            assert abs(equity-entryfee-event['equity_after_entry'])<1e-7
            pnl=float(q@(c-o));fund=0.
            for name,quantity in zip(event['names'],q):
                for r in bycoin[name]:
                    t=r['fundingTime']//HOUR*HOUR
                    if enter<t<=exit_ and r['fundingTime']<END:
                        base=quantity*float(r['fundingRate'])*r['payment_mark']
                        fund+=base
                        if r['mark_source']=='official_mark_bar_open_proxy':fund+=abs(base)*adverse
            equity+=pnl-fund-entryfee-exitfee
            largest_error=max(largest_error,abs(equity-event['equity_after']))
            assert abs(equity-event['equity_after'])<1e-6
            trades+=1
        largest_error=max(largest_error,abs(equity-results[key]['final_equity']))
        assert abs(equity-results[key]['final_equity'])<1e-6
        daily=pd.read_csv(OUT/(key+'_ledger.csv'))
        recompute=daily.equity_before+daily.price_pnl-daily.funding_paid-daily.fees
        assert np.allclose(recompute,daily.equity_after,atol=1e-8,rtol=0)
    evidence=dict(status='passed',paths=len(results),completed_trades_including_stresses=trades,funding_records_verified=checked_marks,prefix_cutoffs_passed=prefix,max_cash_error_usdt=largest_error,max_hold_hours=max_hold,min_cash_only_gap_days=min_idle,limitations=['Limited preselected coin universe','Historical fee tier and lot-size filters not replayed','Daily trade-open execution and hourly settlement-order approximation','Near-time official mark-bar proxies for early funding','Adverse daily envelope is a rejection screen, not tick liquidation reconstruction'])
    save(OUT/'audit.json',evidence)
    files=[OUT.parent/'FUNDING_PANEL_SPEC.md',OUT.parent/'funding_screen.py',OUT.parent/'funding_audit.py',OUT/'inputs.npz',OUT/'returns.npz']
    save(OUT/'run_manifest.json',{str(f.relative_to(OUT.parent)):hashlib.sha256(f.read_bytes()).hexdigest() for f in files})
    print(json.dumps(evidence,indent=2))


if __name__=='__main__':main()
