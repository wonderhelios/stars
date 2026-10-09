"""Fully collateralized inverse quarterly futures basis, exact coin-denominated payoff."""
from pathlib import Path
import json
import numpy as np
import pandas as pd
from carry import OUT, inputs, stats


def run(coin, d, extra_slip=0.):
    contracts = sorted([x for x in json.loads((OUT/'dated_manifest.json').read_text()) if x['coin']==coin], key=lambda x:x['begin'])
    assert all(x.get('status')=='ok' for x in contracts)
    quotes = {}
    for contract in contracts:
        r = json.loads((OUT/'raw'/contract['file']).read_text())['result']
        quotes[contract['instrument']] = {int(t):(float(r['open'][i]), float(r['volume'][i])) for i,t in enumerate(r['ticks'])}
    deliveries = {x['date']:float(x['delivery_price']) for x in json.loads((OUT/f'delivery_{coin}.json').read_text())}
    j = ['BTC','ETH','SOL'].index(coin)
    spot = d['spot_o'][:,j]/d['fx_o']
    # Buy collateral once; it stays at Deribit, with the inverse short offsetting its price risk.
    E0 = 100000.
    spot_fee = .0013+extra_slip
    fx_fee = .0013+extra_slip
    future_fee = .0008+extra_slip  # 5bp taker assumption + 3bp slippage.
    btc = (E0-5.)/spot[0]/(1+spot_fee)/(1+fx_fee)
    step, entry = 0, quotes[contracts[0]['instrument']][int(d['times'][0])][0]
    lot = 10 if coin=='BTC' else 1
    N = np.floor(btc*entry/(1+future_fee)/lot)*lot
    btc -= N/entry*future_fee
    eqs, returns, fees, basis = [], [], [], []
    audit = dict(zero_volume_marks=0, zero_volume_entries=0, rolls=[], minimum_collateral_fraction=1.)
    if quotes[contracts[0]['instrument']][int(d['times'][0])][1] <= 0:
        audit['zero_volume_entries'] += 1
    previous = E0
    for i,t in enumerate(d['times']):
        fee_today = 0.
        while contracts[step]['expiry'] < t:
            expiry = contracts[step]['expiry']
            ds = pd.to_datetime(expiry, unit='ms', utc=True).strftime('%Y-%m-%d')
            delivery = deliveries[ds]
            settled = btc+N*(1/delivery-1/entry)
            fee_coin = N/delivery*.00025
            settled -= fee_coin
            step += 1
            entry, volume = quotes[contracts[step]['instrument']][expiry]
            assert entry > 0
            if volume <= 0:
                audit['zero_volume_entries'] += 1
            N = np.floor(settled*entry/(1+future_fee)/lot)*lot
            fee_coin += N/entry*future_fee
            btc = settled-N/entry*future_fee
            fee_today += fee_coin*spot[i]  # attribution proxy only, equity accounting is exact in coin.
            audit['rolls'].append(dict(expiry=expiry, delivery=delivery, next=contracts[step]['instrument'], next_entry=entry))
        price, volume = quotes[contracts[step]['instrument']][int(t)]
        if volume <= 0:
            audit['zero_volume_marks'] += 1
        marked_coin = btc+N*(1/price-1/entry)
        eq = marked_coin*spot[i]
        audit['minimum_collateral_fraction'] = min(audit['minimum_collateral_fraction'], marked_coin/(N/price))
        assert eq > 0 and marked_coin/(N/price) > .5
        if i==0:
            fee_today = E0-eq
        if i==len(d['times'])-1:
            close_coin = marked_coin-N/price*future_fee
            eq = close_coin*spot[i]*(1-spot_fee)*(1-fx_fee)-5.
            fee_today += marked_coin*spot[i]-eq
        eqs.append(eq)
        returns.append(eq/previous-1)
        fees.append(fee_today/previous)
        basis.append(price/spot[i]-1)
        previous = eq
    r = np.array(returns)
    assert np.isfinite(r).all()
    return dict(r=r, equity=np.array(eqs), fee=np.array(fees), basis=np.array(basis)), audit


def main():
    d = inputs()
    dates = pd.to_datetime(d['times'], unit='ms', utc=True)
    masks = {'full':np.ones(len(dates),bool), '2023-24':dates.year<=2024, '2025-26':dates.year>=2025}
    results, returns, normal = {}, {}, {}
    for extra,label in [(0.,''),(.0003,'_slip6bp')]:
        ledgers=[]
        for coin in ['BTC','ETH']:
            v,audit=run(coin,d,extra)
            name='dated_'+coin+label
            results[name]=dict(periods={p:dict(**stats(v['r'][m]),annual_fees=float(v['fee'][m].mean()*365)) for p,m in masks.items()},audit=audit)
            returns[name]=v['r']
            ledgers.append(v)
            pd.DataFrame(dict(date=dates,**v)).to_csv(OUT/(name+'_ledger.csv'),index=False)
        eq=np.mean([v['equity'] for v in ledgers],axis=0)
        r=eq/np.r_[100000.,eq[:-1]]-1
        name='dated_equal'+label
        results[name]=dict(periods={p:stats(r[m]) for p,m in masks.items()},note='Initial capital 50/50 BTC/ETH, no free daily rebalancing')
        returns[name]=r
    (OUT/'dated_results.json').write_text(json.dumps(results,indent=2))
    np.savez_compressed(OUT/'dated_returns.npz',times=d['times'],**returns)
    for name,v in results.items():
        print(name,v['periods'],v.get('audit',{}).get('zero_volume_entries'),flush=True)


if __name__=='__main__':
    main()
