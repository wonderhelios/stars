"""Independent cashflow, raw-print, and point-in-time invariants."""
import json
from collections import Counter
import numpy as np
import pandas as pd
from fetch_options import OUT, ROOT, digest
from options_prepare import select, ordinary


def main():
    selections=json.loads((OUT/'selections.json').read_text())
    results=json.loads((OUT/'results.json').read_text())
    ledgers=json.loads((OUT/'ledgers.json').read_text())
    npz=np.load(OUT/'returns.npz')
    source_cache={}
    max_cash_error=0.
    max_return_error=0.
    max_hold=0.
    fills_checked=0
    prefix_checks=0
    partials=0
    for r in selections:
        if r['status']!='selected':
            continue
        key=(r['event'],r['coin'])
        if key not in source_cache:
            source_cache[key]=json.loads((OUT/f'{key[0]}_{key[1]}_entry.json').read_text())
        rows=source_cache[key]
        full=select(rows,r['decision'],r['expiry'])
        prefix=select([t for t in rows if t['timestamp']<r['decision']],r['decision'],r['expiry'])
        assert full==prefix
        assert full['legs']==r['legs']
        assert all(t['timestamp']<r['decision'] for t in r['legs'].values())
        prefix_checks+=1
    for name,ledger in ledgers.items():
        cash=100000.
        fx=.0015 if name.endswith('fee15') else .0005
        for e in ledger:
            assert abs(cash-e['cash_before'])<1e-7
            debit=0.
            receipt=0.
            if e.get('fills'):
                raw={str(t['trade_id']):t for t in source_cache[(e['event'],e['coin'])]}
                quantities={'C':0.,'P':0.}
                ids=[]
                for t in e['fills']:
                    source=raw[str(t['trade_id'])]
                    assert ordinary(source) and source['direction']=='buy'
                    assert source['instrument_name']==t['instrument']
                    assert source['timestamp']==t['timestamp']
                    assert e['decision']<=t['timestamp']<e['decision']+900000
                    assert t['quantity']<=source['amount']*.1+1e-10
                    assert t['proxy_price']>source['price']
                    assert t['proxy_price']<=e['limits'][t['side']]+1e-12
                    premium_coin=t['quantity']*t['proxy_price']
                    fee_coin=min(t['quantity']*.0003,premium_coin*.125)
                    independent=(premium_coin+fee_coin)*source['index_price']*(1+fx)
                    assert abs(independent-t['cost_usd'])<1e-8
                    debit+=independent
                    quantities[t['side']]+=t['quantity']
                    max_hold=max(max_hold,(e['expiry']-t['timestamp'])/3600000)
                    ids.append(str(t['trade_id']))
                    fills_checked+=1
                assert len(ids)==len(set(ids))
                for side,qty in quantities.items():
                    assert abs(qty-e['quantities'][side])<1e-9
                    assert qty<=e['target']+1e-9
                partials+=int(abs(quantities['C']-quantities['P'])>1e-9)
                for t in e['receipts']:
                    s=t['index'];k=e['strike'];q=t['quantity']
                    payoff_coin=q*max(0.,(1-k/s) if t['side']=='C' else (k/s-1))
                    fee_coin=min(q*.00015,payoff_coin*.125)
                    independent=(payoff_coin-fee_coin)*s*(1-fx)
                    assert abs(independent-t['net_usd'])<1e-7
                    receipt+=independent
            assert debit<=cash*.02+1e-7
            assert abs(debit-e['cost'])<1e-7
            assert abs(receipt-e['net_settlement'])<1e-7
            cash+=receipt-debit
        max_cash_error=max(max_cash_error,abs(cash-results[name]['capital_final']))
        assert abs(cash-results[name]['capital_final'])<1e-7
        nav=pd.read_csv(OUT/(name+'_nav.csv'))['nav_proxy'].to_numpy()
        y=nav[1:]/nav[:-1]-1
        assert np.array_equal(np.isnan(y),np.isnan(npz[name]))
        max_return_error=max(max_return_error,float(np.nanmax(np.abs(y-npz[name]))))
        assert np.allclose(y,npz[name],equal_nan=True,atol=1e-13)
        assert abs(nav[-1]-cash)<1e-7
    assert max_hold<=48
    audit=dict(status='passed',paths=len(results),point_in_time_checks=prefix_checks,fills_checked=fills_checked,imbalanced_events_including_stress=partials,max_hold_hours=max_hold,max_cash_error_usd=max_cash_error,max_daily_return_error=max_return_error,execution_quotes_verified=False,remaining_limitations=['Historical prints do not prove fill availability','Interim mark timestamps may precede valuation by up to one hour','Conversion at delivery TWAP plus fixed spread is a proxy','Event schedule is reconstructed from current official historical calendar','No original-strategy synchronized portfolio test'],input_sha256=digest(OUT/'ledgers.json'))
    (OUT/'audit.json').write_text(json.dumps(audit,indent=2))
    print(json.dumps(audit,indent=2))


if __name__=='__main__':
    main()
