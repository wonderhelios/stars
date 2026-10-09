"""Frozen remaining-year guidance pilot. Single-company exploration only."""
import hashlib
import json
from pathlib import Path
import numpy as np
import pandas as pd
from screen import ROOT, DATES, SCHED, Signal, load, simulate, stat

OUT = ROOT / 'guidance'
OUT.mkdir(exist_ok=True)


def make_signals(data, rows, coverage, cutoff=None):
    n = len(data['VEEV'])
    opens = SCHED['open'].iloc[:n]
    closes = SCHED['close'].iloc[:n]
    residual = data['VEEV'].ret-data['QQQ'].ret
    sigma = residual.rolling(20, min_periods=20).std().shift(1)
    signals = {k: Signal({'VEEV': np.zeros(n, dtype=bool)}) for k in ['guidance_immediate','guidance_pullback']}
    publications = sorted({pd.Timestamp(r['published']).tz_convert('UTC') for r in coverage if 'published' in r})
    trace = []
    for row in rows:
        pub = pd.Timestamp(row['available_after']).tz_convert('UTC')
        if cutoff is not None and pub > cutoff:
            continue
        revision = row.get('remaining_year_revision_million')
        if revision is None or revision <= 0:
            continue
        first = opens.searchsorted(pub+pd.Timedelta(hours=1), side='left')
        if first >= n:
            continue
        signals['guidance_immediate']['VEEV'][first] = True
        nextpub = next((d for d in publications if d > pub), None)
        pullback = None
        for i in range(first, min(first+20, n-1)):
            if nextpub is not None and opens.iloc[i+1] >= nextpub:
                break
            if residual.iloc[i] < -sigma.iloc[i]:
                pullback = i+1
                signals['guidance_pullback']['VEEV'][pullback] = True
                break
        trace.append(dict(source=row['source'], published=row['published'],
            available_after=row['available_after'],
            remaining_year_revision_million=revision,
            immediate_entry=str(opens.iloc[first]),
            pullback_observed=str(closes.iloc[pullback-1]) if pullback is not None else None,
            pullback_entry=str(opens.iloc[pullback]) if pullback is not None else None))
    for name, sig in signals.items():
        sig.name = name
    return signals, trace


def main():
    rows = json.loads((ROOT/'guidance_probe/guidance_rows.json').read_text())
    coverage = json.loads((ROOT/'guidance_probe/guidance_coverage.json').read_text())
    assert len(coverage) == 22 and all('issue' not in c and c['annual_blocks'] >= 1 for c in coverage)
    data, _ = load()
    sigs, trace = make_signals(data, rows, coverage)
    (OUT/'signal_trace.json').write_text(json.dumps(trace, indent=2))
    earliest = min(pd.Timestamp(r['published']).tz_convert('UTC') for r in coverage)
    start = DATES[SCHED['open'].searchsorted(earliest+pd.Timedelta(hours=1))]
    results, returns, trades = {}, {}, []
    for name, sig in sigs.items():
        for hold in [1,2,5]:
            for fee in [.0005,.0015]:
                key = f'{name}_h{hold}_fee{int(fee*10000)}'
                r, diag, ts, ledger = simulate(data,sig,hold,fee=fee,record=True,
                    max_hold_hours=48 if hold < 5 else 192, phase_filter=False)
                r = r.loc[start:]
                metrics = {k: stat(v) for k,v in {'full':r,'2021-22':r.loc[:'2022-12-31'],
                    '2023-24':r.loc['2023-01-01':'2024-12-31'],'2025-26':r.loc['2025-01-01':]}.items()}
                # Sharpe is undefined for a cash-only segment.
                for m in metrics.values():
                    if m['vol'] == 0:
                        m['sharpe'] = None
                results[key] = dict(metrics=metrics, diagnostics=diag)
                returns[key] = r.to_numpy()
                for t in ts:
                    t['strategy'] = key
                trades.extend(ts)
                df = pd.DataFrame(ledger)
                df[df.date >= str(start.date())].to_csv(OUT/f'{key}_ledger.csv', index=False)
                print(key, metrics['full']['annual_return'], metrics['full']['sharpe'], diag['trades'], flush=True)
    pd.DataFrame(trades).to_csv(OUT/'trades.csv', index=False)
    np.savez_compressed(OUT/'returns.npz',dates=r.index.to_numpy(dtype='datetime64[D]'),**returns)
    (OUT/'results.json').write_text(json.dumps(results, indent=2, allow_nan=False))
    checks = {}
    for cutdate in ['2023-12-31','2025-06-30']:
        n = DATES.searchsorted(pd.Timestamp(cutdate), side='right')
        sub = {s:d.iloc[:n].copy() for s,d in data.items()}
        cutoff = SCHED['close'].iloc[n-1]
        past_coverage = [r for r in coverage if pd.Timestamp(r['published']) <= cutoff]
        partial,_ = make_signals(sub,rows,past_coverage,cutoff=cutoff)
        for name in sigs:
            assert np.array_equal(partial[name]['VEEV'],sigs[name]['VEEV'][:n])
        checks[cutdate] = True
    (OUT/'prefix_checks.json').write_text(json.dumps(checks,indent=2))
    (OUT/'manifest.json').write_text(json.dumps(dict(start=str(start.date()),end=str(r.index[-1].date()),
        source_count=len(coverage),positive_revision_events=len(trace),
        spec_sha256=hashlib.sha256((ROOT/'GUIDANCE_SPEC.md').read_bytes()).hexdigest(),
        code_sha256={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in [ROOT/'guidance_extract.py',ROOT/'guidance_screen.py',ROOT/'screen.py']},
        data_sha256={s:hashlib.sha256((ROOT/'raw'/f'{s}.json').read_bytes()).hexdigest() for s in ['VEEV','QQQ']}),indent=2))


if __name__ == '__main__':
    main()
