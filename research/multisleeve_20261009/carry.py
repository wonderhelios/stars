"""Independent cash-and-carry ledger with actual funding and two venue cash accounts."""
from pathlib import Path
import json
import numpy as np
import pandas as pd

OUT = Path(__file__).resolve().parent
DAY = 86400000
COINS = ['BTC', 'ETH', 'SOL']


def inputs():
    src = dict(np.load(OUT.parent / 'portfolio_20261009/input.npz'))
    times = src['times']
    mask = (times >= pd.Timestamp('2023-06-01', tz='UTC').timestamp()*1000) & (times < 1791417600000)
    times = times[mask]  # Last funding settlement at 2026-10-08 00:00 not returned in frozen request.
    names = list(src['names'])
    d = {'times': times}
    for z in ['o', 'h', 'l', 'c']:
        d['perp_' + z] = src[z][mask][:, [names.index(c) for c in COINS]]
    for suffix, col in [('o', 1), ('h', 2), ('l', 3), ('c', 4)]:
        x = []
        for coin in COINS + ['USDC']:
            rows = json.loads((OUT / f'spot_{coin}USDT.json').read_text())
            lookup = {int(r[0]): float(r[col]) for r in rows}
            x.append([lookup.get(int(t), np.nan) for t in times])
        d['spot_' + suffix] = np.array(x[:3]).T
        d['fx_' + suffix] = np.array(x[3])
    d['rates'] = np.zeros((len(times), 3))
    d['positive_rates'] = np.zeros((len(times), 3))
    d['negative_rates'] = np.zeros((len(times), 3))
    d['counts'] = np.zeros((len(times), 3), int)
    di = {t: i for i, t in enumerate(times)}
    gaps = []
    for j, coin in enumerate(COINS):
        rows = json.loads((OUT / f'funding_{coin}.json').read_text())
        hs = [int(r['time']) // 3600000 * 3600000 for r in rows]
        for a, b in zip(hs, hs[1:]):
            if b-a not in [3600000, 8*3600000]:
                gaps.append({'coin': coin, 'before': a, 'after': b, 'missing_hours': int((b-a)//3600000-1)})
        for row, hour in zip(rows, hs):
            # Settlement at midnight belongs to the position held BEFORE that midnight.
            day = ((hour + DAY - 1) // DAY) * DAY
            if day in di:
                i = di[day]
                d['rates'][i, j] += float(row['fundingRate'])
                d['positive_rates'][i,j] += max(0., float(row['fundingRate']))
                d['negative_rates'][i,j] += min(0., float(row['fundingRate']))
                d['counts'][i, j] += 1
    d['gaps'] = gaps
    assert all(np.isfinite(d[z]).all() for z in d if z not in ['gaps'])
    assert np.all(np.diff(times) == DAY)
    return d


def stats(r):
    r = np.asarray(r)
    eq = np.r_[1., np.cumprod(1+r)]
    sd = r.std(ddof=1)
    return dict(sharpe=float(r.mean()/sd*np.sqrt(365)) if sd else 0.,
                annual_return=float(r.mean()*365), vol=float(sd*np.sqrt(365)),
                cagr=float(eq[-1]**(365/len(r))-1),
                max_drawdown=float(np.min(eq/np.maximum.accumulate(eq)-1)))


def run(d, gated=False, stress=False, transfer_delay=1, transfer_fee=5., slippage=.0003, initial_equity=100000.):
    """USDC equity. Spot BTC paid in USDT; perpetual variation margin in USDC.

    Transfers leave one venue immediately and are unavailable until next daily mark.
    All trades share the SAME coin quantity on the long spot and short perp legs.
    """
    E0 = initial_equity
    spot_fee, perp_fee, fx_fee = .001+slippage, .00045+slippage, .001+slippage
    spot_cash = .5*E0*d['fx_o'][0]*(1-fx_fee)
    perp_cash = .5*E0
    qty = np.zeros(3)
    active = np.ones(3, bool) if not gated else np.zeros(3, bool)
    pending = []
    out = {k: [] for k in ['r', 'equity', 'funding', 'fees', 'basis_pnl', 'cash_fx_pnl', 'turnover', 'gross', 'perp_margin_ratio']}
    audits = {'transfers': 0, 'transfer_volume': 0., 'open_margin_breaches': 0,
              'intraday_adverse_envelope_breaches': 0, 'constrained_buys': 0,
              'missing_funding_hours': sum(x['missing_hours'] for x in d['gaps'])}
    prev_E, previous_spot_usdc = E0, d['spot_o'][0]/d['fx_o'][0]
    for i, t in enumerate(d['times']):
        fx = d['fx_o'][i]
        s = d['spot_o'][i]/fx
        p = d['perp_o'][i]
        fees = .5*E0*fx_fee if i == 0 else 0.
        fund, basis, cash_fx = 0., 0., 0.
        if i:
            daily_rates = d['rates'][i].copy()
            oracle = (previous_spot_usdc+s)/2
            payments = oracle*daily_rates
            if stress:
                # Deliberately adverse oracle within the day's spot range for every signed payment.
                low = d['spot_l'][i-1]/d['fx_h'][i-1]
                high = d['spot_h'][i-1]/d['fx_l'][i-1]
                payments = low*d['positive_rates'][i]+high*d['negative_rates'][i]
                # Unavailable hours are explicitly charged the trailing week's largest daily rate / 24.
                for gap in d['gaps']:
                    day = ((gap['after']+DAY-1)//DAY)*DAY
                    if day == t:
                        j = COINS.index(gap['coin'])
                        payments[j] -= high[j]*np.max(np.abs(d['rates'][max(0, i-7):i, j]))/24*gap['missing_hours']
            fund = float(np.sum(qty*payments))
            perp_pnl = float(-qty@(p-d['perp_o'][i-1]))
            basis = float(qty@(s-previous_spot_usdc))+perp_pnl
            cash_fx = spot_cash/fx-spot_cash/d['fx_o'][i-1]
            perp_cash += perp_pnl+fund
            high_loss = float(qty@(d['perp_h'][i-1]-d['perp_o'][i-1]))
            # Non-simultaneous highs: diagnostic conservative envelope, not a realized liquidation claim.
            if previous_perp_cash-high_loss < .05*float(qty@d['perp_h'][i-1]):
                audits['intraday_adverse_envelope_breaches'] += 1
        remaining = []
        for arrival, dest, amount in pending:
            if arrival <= i:
                if dest == 'perp':
                    perp_cash += amount
                else:
                    spot_cash += amount*fx*(1-fx_fee)
                    fees += amount*fx_fee
            else:
                remaining.append((arrival, dest, amount))
        pending = remaining
        eq = perp_cash+spot_cash/fx+float(qty@s)+sum(x[2] for x in pending)
        assert eq > 0
        short_notional = float(qty@p)
        if short_notional and perp_cash < .05*short_notional:
            audits['open_margin_breaches'] += 1
        if gated and i >= 7:
            # At the 00:05 execution proxy, midnight settlement is observable.
            forecast = np.sum(d['rates'][i-6:i+1], axis=0)/7*365
            active = np.where(active, forecast >= 0., forecast > .10)
        target = .45*eq/3/s*active
        delta = target-qty
        delta[np.abs(delta) < .20*np.maximum(np.abs(target), 1e-12)] = 0.
        # Execute sales before purchases; never invent immediately accessible transferred capital.
        sell = np.minimum(delta, 0.)
        spot_cash -= float(sell@d['spot_o'][i])+float(np.abs(sell)@d['spot_o'][i])*spot_fee
        perp_cash -= float(np.abs(sell)@p)*perp_fee
        cost = float(np.abs(sell)@s)*spot_fee+float(np.abs(sell)@p)*perp_fee
        qty += sell
        buy = np.maximum(delta, 0.)
        need = float(buy@d['spot_o'][i])*(1+spot_fee)
        if need > spot_cash:
            buy *= max(0., spot_cash)/need
            audits['constrained_buys'] += 1
        spot_cash -= float(buy@d['spot_o'][i])*(1+spot_fee)
        perp_cash -= float(buy@p)*perp_fee
        cost += float(buy@s)*spot_fee+float(buy@p)*perp_fee
        qty += buy
        fees += cost
        assert spot_cash >= -1e-6 and (qty >= -1e-12).all()
        turnover = float((np.abs(sell)+buy)@(s+p))
        # Rebalance the two venue capital pools only when the mismatch exceeds 2% of total equity.
        eq = perp_cash+spot_cash/fx+float(qty@s)+sum(x[2] for x in pending)
        spot_value = spot_cash/fx+float(qty@s)
        if not pending:
            excess = spot_value-.5*eq
            amount, dest = 0., None
            if excess > .02*eq:
                amount = min(excess, max(0., spot_cash/fx-transfer_fee)/(1+fx_fee))
                if amount > .02*eq:
                    spot_cash -= (amount*(1+fx_fee)+transfer_fee)*fx
                    fees += amount*fx_fee+transfer_fee
                    dest = 'perp'
            elif excess < -.02*eq:
                amount = min(-excess, max(0., perp_cash-.25*float(qty@p)-transfer_fee))
                if amount > .02*eq:
                    perp_cash -= amount+transfer_fee
                    fees += transfer_fee
                    dest = 'spot'
            if dest:
                audits['transfers'] += 1
                audits['transfer_volume'] += amount
                pending.append((i+max(1, transfer_delay), dest, amount))
        eq = perp_cash+spot_cash/fx+float(qty@s)+sum(x[2] for x in pending)
        if i == len(d['times'])-1:
            # Both hedge legs closed, and all available venue cash converted to USDC.
            terminal = float(qty@s)*spot_fee+float(qty@p)*perp_fee
            usdt_value = spot_cash/fx+float(qty@s)*(1-spot_fee)
            terminal += usdt_value*fx_fee+transfer_fee
            fees += terminal
            eq -= terminal
            turnover += float(qty@(s+p))
        values = dict(r=eq/prev_E-1, equity=eq, funding=fund/prev_E, fees=fees/prev_E,
                      basis_pnl=basis/prev_E, cash_fx_pnl=cash_fx/prev_E,
                      turnover=turnover/prev_E, gross=float(qty@(s+p))/eq,
                      perp_margin_ratio=perp_cash/max(float(qty@p), 1e-12))
        for z, value in values.items():
            out[z].append(value)
        # Equity change reconciles to price basis + funding + currency + ALL fees.
        assert abs((eq-prev_E)-(basis+fund+cash_fx-fees)) < 1e-6, (i, eq-prev_E, basis+fund+cash_fx-fees)
        prev_E, previous_spot_usdc, previous_perp_cash = eq, s, perp_cash
    out = {k: np.array(v) for k, v in out.items()}
    assert np.isfinite(out['r']).all()
    return out, audits


def main():
    d = inputs()
    dates = pd.to_datetime(d['times'], unit='ms', utc=True)
    masks = {'full': np.ones(len(dates), bool), '2023-24': dates.year <= 2024, '2025-26': dates.year >= 2025}
    results, returns = {}, {}
    for label, kwargs in [('carry_always', {}), ('carry_gated', {'gated': True}),
                          ('always_adverse', {'stress': True}), ('gated_adverse', {'gated': True, 'stress': True}),
                          ('always_slip6bp', {'slippage': .0006}), ('always_transfer3d', {'transfer_delay': 3})]:
        v, audit = run(d, **kwargs)
        results[label] = {'periods': {p: dict(**stats(v['r'][m]), annual_fees=float(v['fees'][m].mean()*365),
                                             annual_funding=float(v['funding'][m].mean()*365),
                                             annual_basis_pnl=float(v['basis_pnl'][m].mean()*365),
                                             annual_cash_fx=float(v['cash_fx_pnl'][m].mean()*365)) for p, m in masks.items()}, 'audit': audit}
        returns[label] = v['r']
        print(label, results[label], flush=True)
        pd.DataFrame({'date': dates, **v}).to_csv(OUT / f'{label}_ledger.csv', index=False)
    (OUT / 'carry_results.json').write_text(json.dumps({'strategies': results, 'funding_gaps': d['gaps']}, indent=2))
    np.savez_compressed(OUT / 'carry_returns.npz', times=d['times'], **returns)


if __name__ == '__main__':
    main()
