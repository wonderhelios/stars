"""Paid-premium event options: conservative historical-print execution proxy.

No order book means these results are research screens, not executable results.
"""
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path
import json
import math
import numpy as np
import pandas as pd
from fetch_options import ROOT, OUT, digest
from options_prepare import ordinary, tick, round_up


def fee(price):
    return min(.0003, price*.125)


def floor_size(value, step):
    return math.floor(value/step+1e-10)*step


def simulate_entry(r, rows, cash, stress=False):
    fx = .0015 if stress else .0005
    penalty = 2 if stress else 1
    legs, meta = r['legs'], r['metadata']
    limits = {s:round_up(meta[s], t['price']*1.1) for s,t in legs.items()}
    steps = {s:float(meta[s]['min_trade_amount']) for s in legs}
    assert steps['C'] == steps['P']
    step = steps['C']
    index_limit = r['index']*1.1
    q = min(floor_size(cash*.01/((limits[s]+fee(limits[s]))*index_limit*(1+fx)), step) for s in legs)
    quantities = dict(C=0., P=0.)
    fills = []
    total_cost = 0.
    if q <= 0:
        return dict(target=q, limits=limits, quantities=quantities, fills=fills, cost=0., status='budget_below_minimum')
    names = {t['instrument_name']:s for s,t in legs.items()}
    for t in rows:
        if not r['decision'] <= t['timestamp'] < r['decision']+900000:
            continue
        if not ordinary(t) or t['direction'] != 'buy' or t['instrument_name'] not in names:
            continue
        s = names[t['instrument_name']]
        px = round_up(meta[s], t['price']+penalty*tick(meta[s], t['price']))
        if px > limits[s]+1e-12 or t['index_price'] > index_limit:
            continue
        qty = min(floor_size(t['amount']*.1, step), round(q-quantities[s], 10))
        if qty < step-1e-10:
            continue
        premium = qty*px*t['index_price']
        tradefee = qty*fee(px)*t['index_price']
        conversion = (premium+tradefee)*fx
        cost = premium+tradefee+conversion
        quantities[s] += qty
        total_cost += cost
        fills.append(dict(side=s, instrument=t['instrument_name'], trade_id=t['trade_id'], timestamp=t['timestamp'], original_price=t['price'], proxy_price=px, printed_amount=t['amount'], quantity=qty, index=t['index_price'], premium_usd=premium, fee_usd=tradefee, conversion_usd=conversion, cost_usd=cost))
    assert total_cost <= cash*.02+1e-7
    for s in legs:
        assert sum(t['cost_usd'] for t in fills if t['side']==s) <= cash*.01+1e-7
        assert quantities[s] <= q+1e-10
    status = 'no_fill' if not fills else 'balanced' if abs(quantities['C']-quantities['P'])<1e-9 else 'imbalanced'
    return dict(target=q, limits=limits, quantities=quantities, fills=fills, cost=total_cost, status=status)


def metrics(y):
    y = np.asarray(y)
    nav = np.cumprod(1+y)
    ann = float(y.mean()*365)
    vol = float(y.std(ddof=1)*np.sqrt(365))
    return dict(annual_arithmetic=ann, annual_geometric=float(nav[-1]**(365/len(y))-1), volatility=vol, sharpe=ann/vol if vol>1e-14 else None, max_drawdown=float(np.min(nav/np.maximum.accumulate(np.r_[1,nav])[1:]-1)), total_return=float(nav[-1]-1))


def run_path(selections, coin, hold, hour, stress, deliveries):
    dates = pd.date_range('2023-06-01','2026-10-08',freq='D',tz='UTC')+pd.Timedelta(hours=8)
    nav = np.full(len(dates), np.nan)
    cash = 100000.
    ledger = []
    invalid = []
    last_settlement = dates[0]
    for r in selections:
        if (r['coin'],r['hold'],r['hour']) != (coin,hold,hour):
            continue
        decision = pd.Timestamp(r['decision'],unit='ms',tz='UTC')
        start = decision.normalize()+pd.Timedelta(hours=8)
        nav[(dates>=last_settlement)&(dates<=start)] = cash
        event = {k:r[k] for k in ['event','coin','hold','hour','decision','expiry','status']}
        event.update(cash_before=cash, cost=0., net_settlement=0., pnl=0.)
        if r['status'] != 'selected':
            if r['status'] in ['source_missing','metadata_or_mark_source_error']:
                invalid.append((r['event'],r['status']))
            event['no_trade_reason'] = r['status']
            ledger.append(event)
            last_settlement = start
            continue
        rows = json.loads((OUT/f"{r['event']}_{coin}_entry.json").read_text())
        entry = simulate_entry(r, rows, cash, stress)
        event.update(entry)
        event.update(strike=r['strike'], expiry_date=pd.Timestamp(r['expiry'],unit='ms',tz='UTC').date().isoformat())
        cash -= entry['cost']
        settlement = deliveries.get(event['expiry_date'])
        if settlement is None and entry['fills']:
            raise ValueError('Missing actual delivery price: '+str(event))
        receipts = []
        fx = .0015 if stress else .0005
        for side,qty in entry['quantities'].items():
            if not qty:
                continue
            payoff = qty*max(0, (settlement-r['strike'])*(1 if side=='C' else -1))
            deliveryfee = min(qty*.00015*settlement, payoff*.125)
            conversion = (payoff-deliveryfee)*fx
            receipts.append(dict(side=side, quantity=qty, index=settlement, gross_usd=payoff, gross_coin=payoff/settlement, delivery_fee_usd=deliveryfee, conversion_usd=conversion, net_usd=payoff-deliveryfee-conversion))
        if hold==2:
            value = cash
            marks = {}
            for side,qty in entry['quantities'].items():
                if not qty:
                    continue
                mark = r.get('interim_marks',{}).get(side)
                if mark is None:
                    invalid.append((r['event'],'missing_interim_mark_'+side))
                    value = np.nan
                else:
                    value += qty*mark['mark_price']*mark['index_price']
                    marks[side] = {'timestamp':mark['timestamp'], 'value':qty*mark['mark_price']*mark['index_price'], 'age_ms':r['expiry']-86400000-mark['timestamp']}
            nav[dates==start+pd.Timedelta(days=1)] = value
            event['interim_marks'] = marks
        receipt = sum(p['net_usd'] for p in receipts)
        cash += receipt
        last_settlement = pd.Timestamp(r['expiry'],unit='ms',tz='UTC')
        nav[dates==last_settlement] = cash
        event.update(receipts=receipts, net_settlement=receipt, pnl=receipt-entry['cost'], cash_after=cash, settlement_regime='two_step_same_net' if event['expiry_date']>='2026-08-01' else 'cash')
        ledger.append(event)
    nav[dates>=last_settlement] = cash
    # Missing marks stay missing; no forward filling or zero substitution.
    y = nav[1:]/nav[:-1]-1
    records = [e for e in ledger if e.get('fills')]
    result = dict(status='incomplete_proxy' if invalid else 'complete_proxy', invalid=invalid, events=len(ledger), events_filled=len(records), imbalanced_events=sum(e['status']=='imbalanced' for e in records), capital_final=cash, cash_pnl=sum(e['pnl'] for e in ledger), max_event_premium_fraction=max([e['cost']/e['cash_before'] for e in ledger]+[0]), total_fills=sum(len(e.get('fills',[])) for e in ledger), statuses=dict(Counter(e['status'] for e in ledger)), metrics={})
    if not invalid and np.isfinite(y).all():
        for part,mask in [('full',np.ones(len(y),bool)),('2023-24',dates[1:].year<=2024),('2025-26',dates[1:].year>=2025)]:
            result['metrics'][part]=metrics(y[mask])
    return dates,nav,y,ledger,result


def main():
    selections = json.loads((OUT/'selections.json').read_text())
    assert len(selections)==27*2*2*3, 'Selection pass must finish before performance read'
    assert all(r['status']!='source_missing' for r in selections), 'Raw fetch incomplete: finish it first'
    ledger_all = {}
    results = {}
    arrays = {}
    for coin in ['BTC','ETH']:
        source = ROOT.parent/'multisleeve_20261009'/f'delivery_{coin}.json'
        deliveries = {d['date']:d['delivery_price'] for d in json.loads(source.read_text())}
        for hold in [1,2]:
            for phase,hour in enumerate([8,9,10]):
                for stress in [False,True]:
                    name=f'fomc_long_{coin}_h{hold}_p{phase}_fee{15 if stress else 5}'
                    dates,nav,y,ledger,result=run_path(selections,coin,hold,hour,stress,deliveries)
                    arrays[name]=y
                    arrays['dates']=dates[1:].tz_localize(None).to_numpy()
                    results[name]=result
                    ledger_all[name]=ledger
                    pd.DataFrame({'time':dates,'nav_proxy':nav}).to_csv(OUT/(name+'_nav.csv'),index=False)
    np.savez_compressed(OUT/'returns.npz',**arrays)
    (OUT/'results.json').write_text(json.dumps(results,indent=2,allow_nan=False))
    (OUT/'ledgers.json').write_text(json.dumps(ledger_all,indent=2,allow_nan=False))
    (OUT/'run_manifest.json').write_text(json.dumps({'spec_sha256':digest(ROOT/'OPTIONS_SPEC.md'),'selection_sha256':digest(OUT/'selections.json'),'code_sha256':digest(Path(__file__)),'historical_quotes':False,'execution':'historical buy-print participation proxy','performance_tested':True},indent=2))
    for name,result in results.items():
        if name.endswith('fee5'):
            print(name,result,flush=True)


if __name__=='__main__':
    main()
