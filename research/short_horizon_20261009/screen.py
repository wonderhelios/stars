"""Frozen short-horizon hypotheses; actual quantities, cash and sale settlement.

Yahoo daily opening/closing auction proxies are not execution validation.
All surviving-universe results are exploratory, including later date splits.
"""
import json, hashlib
from pathlib import Path
import numpy as np
import pandas as pd
import exchange_calendars as xc
from fetch import ROOT, RAW, STOCKS, ETFS

CAL = xc.get_calendar('XNYS', start='2013-01-01', end='2026-11-30')
SCHED = CAL.schedule.loc['2013-01-01':'2026-10-07']
DATES = SCHED.index
DAILY = pd.date_range('2015-01-01', '2026-10-07', freq='D')

def load():
    data, issues = {}, []
    for sym in STOCKS + ETFS:
        f = RAW / f'{sym}.json'
        if not f.exists():
            issues.append(dict(symbol=sym, issue='missing entire history'))
            continue
        x = json.loads(f.read_text())['chart']['result'][0]
        assert x['meta']['currency'] == 'USD'
        idx = pd.to_datetime(x['timestamp'], unit='s', utc=True).tz_convert('America/New_York').tz_localize(None).normalize()
        df = pd.DataFrame(x['indicators']['quote'][0], index=idx)
        df = df[~df.index.duplicated()].reindex(DATES)
        df['div'] = 0.
        for v in x.get('events', {}).get('dividends', {}).values():
            d = pd.Timestamp(v['date'], unit='s', tz='UTC').tz_convert('America/New_York').tz_localize(None).normalize()
            if d in df.index:
                df.loc[d, 'div'] += v['amount']
        ok = df[['open', 'high', 'low', 'close', 'volume']].notna().all(axis=1) & (df.volume > 0)
        valid = df.index[ok]
        holes = df.loc[valid.min():valid.max()].index[~ok.loc[valid.min():valid.max()]] if len(valid) else []
        issues.append(dict(symbol=sym, first=str(valid.min()), last=str(valid.max()), rows=int(ok.sum()), interior_holes=[str(d) for d in holes]))
        df.loc[~ok, ['open','high','low','close','volume']] = np.nan
        df['ret'] = (df.close + df['div']) / df.close.shift(1) - 1
        data[sym] = df
    return data, issues

def signals(data):
    sigs = {k: {} for k in ['stock_gap_up','stock_trend_dip','stock_residual_drop','stock_gap_down','etf_trend_dip','etf_shock_rebound']}
    qqq = data['QQQ']['ret']
    for sym, d in data.items():
        r = d.ret
        vol = r.rolling(20, min_periods=20).std().shift(1)
        residual = r - qqq
        rvol = residual.rolling(20, min_periods=20).std().shift(1)
        liq = (d.close * d.volume).rolling(20, min_periods=20).mean().shift(1) >= 5e6
        gap = (d.open + d['div']) / d.close.shift(1) - 1
        where = (d.close - d.low) / (d.high - d.low).replace(0, np.nan)
        volume = d.volume / d.volume.rolling(20, min_periods=20).mean().shift(1)
        trend = d.close > d.close.rolling(63, min_periods=63).mean().shift(1)
        r3 = (1 + r).rolling(3).apply(np.prod, raw=True) - 1
        resid3 = residual.rolling(3).sum()
        relative63 = residual.rolling(63).sum()
        cond = {
            'stock_gap_up': (gap > vol) & (where >= .75) & (volume >= 1.5),
            'stock_trend_dip': trend & (relative63 > 0) & (resid3 < -vol),
            'stock_residual_drop': residual < -2*rvol,
            'stock_gap_down': (gap < -vol) & (where <= .25) & (volume >= 1.5),
            'etf_trend_dip': trend & (r3 < -vol),
            'etf_shock_rebound': (r < -2*vol) & (where >= .5),
        }
        for name, s in cond.items():
            if (name.startswith('stock') and sym in STOCKS) or (name.startswith('etf') and sym in ETFS):
                sigs[name][sym] = (s & liq).fillna(False).shift(1, fill_value=False).to_numpy()
    return sigs

def stat(r):
    r = np.asarray(r, float)
    sd = r.std(ddof=1)
    eq = np.cumprod(1+r)
    return dict(n=len(r), annual_return=float(r.mean()*365), vol=float(sd*np.sqrt(365)), sharpe=float(r.mean()/sd*np.sqrt(365)) if sd else 0., cagr=float(eq[-1]**(365/len(r))-1), max_drawdown=float(np.min(eq/np.maximum.accumulate(np.r_[1,eq])[1:]-1)))

def simulate(data, signal, hold, phase=0, fee=.0005, gross=1., until=None, record=False, max_hold_hours=48., phase_filter=True, per_name_cap=.2):
    syms = sorted(signal)
    n = len(DATES) if until is None else until
    arrays = {s: data[s][['open','high','low','close','div']].to_numpy() for s in syms}
    cash = 100000.
    positions = {}
    receivables = []
    lastmark = {}
    prevtime = pd.Timestamp('2014-12-31 21:00:00', tz='UTC')
    prevnav = 100000.
    out, trades, accounting = {}, [], []
    fees = finance = borrow = 0.
    traded = stale = marginbreach = 0
    maxhold = 0.
    grosssum = 0.
    active_days = 0
    reconerr = 0.
    rawpnl = divpnl = 0.
    for i in range(n):
        day = DATES[i]
        if day < pd.Timestamp('2015-01-01'):
            continue
        op, cl = SCHED.iloc[i][['open','close']]
        # Prior-close to current-open interest: sale receivables settle at next open,
        # dividend cash held for conservative 30-calendar-day payment proxy.
        dt = (op-prevtime).total_seconds()/86400
        restricted = sum(p['entry_notional'] for p in positions.values() if p['qty'] < 0)
        interest = max(0., restricted-cash)*.06*dt/365
        shortval = sum(abs(p['qty'])*lastmark[s] for s,p in positions.items() if p['qty']<0)
        b = shortval*.03*dt/365
        cash -= interest+b; finance += interest; borrow += b
        pending = []
        for due, amount in receivables:
            if due <= op: cash += amount
            else: pending.append((due, amount))
        receivables = pending
        # Overnight ex-dividend entitlement; dividend receivable contributes NAV,
        # not spendable cash, until the payment proxy date.
        for s,p in positions.items():
            row = arrays[s][i]
            if not np.isfinite(row[0]):
                stale += 1
                raise ValueError(f'Missing held opening mark {s} {day}; no fabricated exit')
            d = p['qty']*row[4]
            if d:
                receivables.append((op+pd.Timedelta(days=30), d)); divpnl += d
            rawpnl += p['qty']*(row[0]-lastmark[s]); lastmark[s] = row[0]
        nav = cash + sum(v for _,v in receivables) + sum(p['qty']*lastmark[s] for s,p in positions.items())
        assert nav > 0
        exitidx = i+hold-1
        admissible = exitidx < len(SCHED) and (SCHED.iloc[exitidx]['close']-op).total_seconds() <= max_hold_hours*3600
        phase_ok = not phase_filter or hold == 1 or i%2 == phase
        names = [s for s in syms if signal[s][i] and s not in positions and np.isfinite(arrays[s][i,0])]
        if names and admissible and phase_ok:
            existing = sum(abs(p['qty'])*lastmark[s] for s,p in positions.items())
            per = min(per_name_cap*nav, max(0.,gross*nav-existing)/(len(names)*(1+fee)))
            for s in names:
                sign = -1. if 'gap_down' in signal_name(signal) else 1.
                # caller sets a metadata-free direction via the signal mapping subclass.
                qty = sign*per/arrays[s][i,0]
                if per <= 0: continue
                cash -= qty*arrays[s][i,0] + per*fee
                fees += per*fee
                positions[s] = dict(qty=qty, entry=op, entry_day=str(day.date()), entry_price=arrays[s][i,0], exitidx=exitidx, entry_notional=per, entry_fee=per*fee)
                lastmark[s] = arrays[s][i,0]
                traded += 1
        intraday_gross = sum(abs(p['qty'])*lastmark[s] for s,p in positions.items())
        grosssum += intraday_gross/nav
        active_days += bool(positions)
        # A daily adverse-price envelope, not an exact intraday margin replay.
        adverse = nav-sum(abs(p['qty'])*(lastmark[s]-(arrays[s][i,2] if p['qty']>0 else 2*lastmark[s]-arrays[s][i,1])) for s,p in positions.items())
        marginbreach += bool(intraday_gross and adverse/intraday_gross < .25)
        hours = (cl-op).total_seconds()/3600
        restricted = sum(p['entry_notional'] for p in positions.values() if p['qty']<0)
        interest = max(0., restricted-cash)*.06*hours/24/365
        b = sum(abs(p['qty'])*lastmark[s] for s,p in positions.items() if p['qty']<0)*.03*hours/24/365
        cash -= interest+b; finance += interest; borrow += b
        for s in list(positions):
            p = positions[s]; close = arrays[s][i,3]
            if not np.isfinite(close): raise ValueError(f'Missing held closing mark {s} {day}')
            rawpnl += p['qty']*(close-lastmark[s]); lastmark[s] = close
            if p['exitidx'] == i:
                value = p['qty']*close
                if value > 0:
                    lag = 1 if day >= pd.Timestamp('2024-05-28') else (2 if day >= pd.Timestamp('2017-09-05') else 3)
                    due = CAL.schedule.iloc[CAL.sessions.get_loc(day)+lag]['open']
                    receivables.append((due, value))
                else:
                    cash += value
                cash -= abs(value)*fee; fees += abs(value)*fee
                duration = (cl-p['entry']).total_seconds()/3600
                maxhold = max(maxhold, duration)
                if record:
                    trades.append(dict(symbol=s,entry=str(p['entry']),exit=str(cl),qty=p['qty'],entry_price=p['entry_price'],exit_price=close,hours=duration,price_pnl=p['qty']*(close-p['entry_price']),fees=p['entry_fee']+abs(value)*fee))
                del positions[s]
        nav = cash + sum(v for _,v in receivables) + sum(p['qty']*lastmark[s] for s,p in positions.items())
        theoretical = 100000+rawpnl+divpnl-fees-finance-borrow
        reconerr = max(reconerr, abs(nav-theoretical))
        assert abs(nav-theoretical) < max(1e-5,abs(nav)*1e-10)
        out[day] = nav/prevnav-1
        if record:
            accounting.append(dict(date=str(day.date()),nav=nav,cash=cash,receivables=sum(v for _,v in receivables),market_value=sum(p['qty']*lastmark[s] for s,p in positions.items()),price_pnl=rawpnl,dividends=divpnl,fees=fees,finance=finance,borrow=borrow))
        prevnav=nav; prevtime=cl
    # Charges over market holidays are booked at next open. This allocation of
    # financing P&L is disclosed; weekly aggregate comparisons avoid stale closes.
    r=pd.Series(out,dtype=float).reindex(DAILY[DAILY<=DATES[n-1]],fill_value=0.)
    diag=dict(trades=traded,fees=fees,finance=finance,borrow=borrow,nav=prevnav,max_hold_hours=maxhold,active_sessions=active_days,mean_open_gross=grosssum/max(1,sum(DATES[:n]>=pd.Timestamp('2015-01-01'))),stale_marks=stale,adverse_margin_flags=marginbreach,reconciliation_max_dollars=reconerr,open_positions=len(positions),cash_available_final=cash,unsettled_final=sum(v for _,v in receivables))
    return r,diag,trades,accounting

class Signal(dict):
    pass

def signal_name(signal):
    return getattr(signal,'name','')

def main():
    data, issues = load()
    sigs = signals(data)
    (ROOT/'coverage.json').write_text(json.dumps(issues,indent=2))
    results={}; returns={}; alltrades=[]
    for name, s in sigs.items():
        signal=Signal(s);signal.name=name
        for hold in [1,2]:
            for phase in range(hold):
                for fee in [.0005,.0015]:
                    key=f'{name}_h{hold}_p{phase}_fee{int(fee*10000)}'
                    r,diag,trades,ledger=simulate(data,signal,hold,phase,fee,record=fee==.0005)
                    parts={'full':r,'2015-22':r.loc[:'2022-12-31'],'2023-24':r.loc['2023-01-01':'2024-12-31'],'2025-26':r.loc['2025-01-01':]}
                    results[key]=dict(metrics={k:stat(v) for k,v in parts.items()},diagnostics=diag)
                    returns[key]=r.to_numpy()
                    for t in trades: t['strategy']=key
                    alltrades.extend(trades)
                    if ledger: pd.DataFrame(ledger).to_csv(ROOT/f'{key}_ledger.csv',index=False)
                    print(key,round(results[key]['metrics']['full']['sharpe'],3),round(results[key]['metrics']['2025-26']['annual_return'],4),diag['trades'],flush=True)
    pd.DataFrame(alltrades).to_csv(ROOT/'trades.csv',index=False)
    np.savez_compressed(ROOT/'returns.npz',dates=DAILY.to_numpy(dtype='datetime64[D]'),**returns)
    (ROOT/'results.json').write_text(json.dumps(results,indent=2))
    rows=[]
    for name,z in results.items():
        for part,m in z['metrics'].items(): rows.append(dict(strategy=name,part=part,**m))
    pd.DataFrame(rows).to_csv(ROOT/'metrics.csv',index=False)
    # Historical truncation must preserve every already-completed daily return.
    cut=2500
    checks={}
    for name in ['stock_trend_dip','stock_gap_down','etf_trend_dip']:
        sig=Signal(sigs[name]);sig.name=name
        a,*_=simulate(data,sig,2,0,until=cut)
        b=returns[f'{name}_h2_p0_fee5'][:len(a)]
        checks[name]=float(np.max(np.abs(a.to_numpy()-b)))
        assert checks[name]<1e-12
    (ROOT/'checks.json').write_text(json.dumps(dict(prefix_return_max_error=checks,source_hashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in RAW.glob('*.json')}),indent=2))

if __name__=='__main__': main()
