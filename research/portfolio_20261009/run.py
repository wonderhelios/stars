"""Frozen, reproducible portfolio comparisons; never connects to trading APIs."""
from pathlib import Path
import hashlib
import json
import math
import warnings
import numpy as np
import pandas as pd

warnings.filterwarnings('ignore', category=RuntimeWarning)
OUT = Path(__file__).resolve().parent
ROOT = OUT.parent.parent
DAY = 86400000
CUTOFF = int(pd.Timestamp('2026-10-09', tz='UTC').timestamp() * 1000)
START = int(pd.Timestamp('2023-08-14', tz='UTC').timestamp() * 1000)


def freeze():
    dest = OUT / 'input.npz'
    if dest.exists():
        meta = json.loads((OUT / 'input_manifest.json').read_text())
        assert hashlib.sha256(dest.read_bytes()).hexdigest() == meta['snapshot_sha256'], 'Frozen input changed'
        return dict(np.load(dest, allow_pickle=False))
    files = sorted(Path('/tmp/hl-daily-full').glob('*.json'))
    assert files, 'No source data'
    raw, manifest, audit = {}, {}, {}
    for f in files:
        payload = f.read_bytes()
        rows = json.loads(payload)
        rows = [r for r in rows if r['t'] + DAY <= CUTOFF and float(r['c']) > 0]
        assert len({r['t'] for r in rows}) == len(rows), f'Duplicate {f}'
        raw[f.stem] = rows
        manifest[f.name] = hashlib.sha256(payload).hexdigest()
        tt = sorted(r['t'] for r in rows)
        audit[f.stem] = dict(rows=len(tt), start=tt[0] if tt else None,
                            end=tt[-1] if tt else None,
                            internal_gaps=int(np.sum(np.diff(tt) != DAY)))
    names = sorted(raw)
    times = np.array(sorted({r['t'] for rr in raw.values() for r in rr}))
    assert np.all(np.diff(times) == DAY)
    ti = {t: i for i, t in enumerate(times)}
    data = {z: np.full((len(times), len(names)), np.nan) for z in ['o', 'c', 'h', 'l', 'v']}
    for j, name in enumerate(names):
        for row in raw[name]:
            for z in data:
                data[z][ti[row['t']], j] = float(row[z])
    for z in ['o', 'c', 'h', 'l']:
        assert (data[z][np.isfinite(data[z])] > 0).all()
    assert (data['v'][np.isfinite(data['v'])] >= 0).all()
    data.update(times=times, names=np.array(names))
    np.savez_compressed(dest, **data)
    old = json.loads((ROOT / 'research/distribution_stagger_20261008/results.json').read_text())['meta']['manifest']
    meta = dict(source='/tmp/hl-daily-full', cutoff_ms=CUTOFF, files=manifest, coverage=audit,
                old_files_identical=sum(old.get(k) == h for k, h in manifest.items()),
                snapshot_sha256=hashlib.sha256(dest.read_bytes()).hexdigest(),
                provenance='Local Hyperliquid candle cache; no new retrieval in this study. Historical exchange availability not independently certified.')
    (OUT / 'input_manifest.json').write_text(json.dumps(meta, indent=2))
    return data


def book(names, ids, score):
    w = np.zeros(len(names))
    ids = ids[np.isfinite(score[ids])]
    if len(ids) < 8:
        return w
    cap = min(max(1, int(np.floor(len(ids) * .2 + .5))), 5, len(ids) // 2)
    order = ids[np.lexsort((names[ids], score[ids]))]
    w[order[:cap]] = -.5 / cap
    w[order[-cap:]] = .5 / cap
    return w


def targets(d):
    p, names = d['c'], d['names']
    q = p * d['v']
    r = pd.DataFrame(p).pct_change(fill_method=None)
    v = pd.DataFrame(q).rolling(30, min_periods=5).mean().shift().values
    sd = r.rolling(20, min_periods=5).std().values
    mom = p / pd.DataFrame(p).shift(14).values - 1
    btc = r.iloc[:, list(names).index('BTC')]
    betas = np.full_like(p, np.nan)
    for j in range(len(names)):
        # Pairwise observations, so missing alt history does not change the BTC variance window.
        x = btc.where(r[j].notna())
        betas[:, j] = (r[j].rolling(60, min_periods=40).cov(x) /
                        x.rolling(60, min_periods=40).var()).shift().values
    resid = r.values - betas * btc.values[:, None]
    residual_score = (pd.DataFrame(resid).rolling(14, min_periods=14).sum().values /
                      pd.DataFrame(resid).rolling(20, min_periods=20).std().values)
    high_score = p / pd.DataFrame(p).rolling(90, min_periods=60).max().values
    trend = np.sign(p / pd.DataFrame(p).shift(63).values - 1)
    eligible = ((v >= 5e6) & np.isfinite(p) & np.isfinite(mom) & np.isfinite(sd) &
                np.array([':' not in name for name in names])[None, :])
    t = {z: np.zeros_like(p) for z in ['baseline', 'residual_momentum', 'near_high', 'btc_eth_trend']}
    for i in range(32, len(p)):
        ids = np.flatnonzero(eligible[i])
        if len(ids) >= 8:
            w = sum(book(names, ids, s) for s in [mom[i] / np.maximum(sd[i], 1e-9), -sd[i], q[i] / v[i]]) / 3
            g = np.abs(w).sum()
            t['baseline'][i] = w / g if g > 1e-12 else w
            t['residual_momentum'][i] = book(names, ids, residual_score[i])
            t['near_high'][i] = book(names, ids, high_score[i])
        for name in ['BTC', 'ETH']:
            j = list(names).index(name)
            if eligible[i, j] and np.isfinite(trend[i, j]):
                t['btc_eth_trend'][i, j] = .5 * trend[i, j]
    for z in ['residual_momentum', 'near_high', 'btc_eth_trend']:
        t['blend_' + z] = .8 * t['baseline'] + .2 * t[z]
    t['blend_all'] = .8 * t['baseline'] + sum(t[z] for z in ['residual_momentum', 'near_high', 'btc_eth_trend']) * (.2 / 3)
    assert all(np.isfinite(a).all() and np.max(np.abs(a).sum(axis=1)) <= 1 + 1e-10 for a in t.values())
    assert all(np.max(np.abs(t[z].sum(axis=1))) < 1e-10 for z in ['baseline', 'residual_momentum', 'near_high'])
    return t, eligible


def simulate(d, target, phase=0, fee=.00075, period=3, lev=2.7, penalty=0):
    """Notional weights, actual price/equity drift, close-known -> next open."""
    op = pd.DataFrame(d['o']).ffill().values
    ks = np.flatnonzero(d['times'] >= START)
    ks = ks[ks > 32]
    w = np.zeros(len(d['names']))
    hashes = np.array([sum(name.encode()) for name in d['names']])
    out = {z: [] for z in ['r', 'turn', 'gross', 'net', 'missing', 'cost']}
    for step, i in enumerate(ks):
        gain = 0.
        if step:
            rr = np.nan_to_num(op[i] / op[i - 1] - 1)
            gain = w @ rr
            assert gain > -1, 'Bankruptcy in open-to-open mark'
            w = w * (1 + rr) / (1 + gain)
        tgt = target[i - 1] * lev
        valid = np.isfinite(d['o'][i])
        slot = ((d['times'][i] // DAY + hashes + phase) % period) == 0
        delta = tgt - w
        use = slot & valid & ((np.abs(delta) >= .02 * np.abs(tgt)) | (tgt * w <= 0))
        dw = np.where(use, delta, 0.)
        missing = np.abs(w[~valid]).sum()
        dw[~valid] = -w[~valid]
        turn = np.abs(dw).sum()
        cost = turn * fee + missing * penalty
        assert cost < 1
        w = (w + dw) / (1 - cost)
        for z, value in dict(r=(1 + gain) * (1 - cost) - 1, turn=turn,
                             gross=np.abs(w).sum(), net=w.sum(), missing=missing, cost=cost).items():
            out[z].append(value)
    terminal = np.abs(w).sum()
    out['r'][-1] = (1 + out['r'][-1]) * (1 - terminal * fee) - 1
    out['turn'][-1] += terminal
    out['cost'][-1] += terminal * fee
    result = {z: np.array(v) for z, v in out.items()}
    assert np.isfinite(result['r']).all() and (result['r'] > -1).all()
    return result


def sharpe(r):
    sd = np.std(r, ddof=1)
    return float(np.mean(r) / sd * np.sqrt(365)) if sd > 1e-12 else 0.


def stat(r):
    eq = np.r_[1., np.cumprod(1 + r)]
    return dict(sharpe=sharpe(r), annual_arithmetic=float(r.mean() * 365),
                cagr=float(eq[-1] ** (365 / len(r)) - 1),
                max_drawdown=float((eq / np.maximum.accumulate(eq) - 1).min()))


def summarize(rr, base, mask):
    ss = [stat(v['r'][mask]) for v in rr]
    result = {z: float(np.mean([v[z] for v in ss])) for z in ss[0]}
    result['phase_sharpes'] = [v['sharpe'] for v in ss]
    result['phase_sd'] = float(np.std(result['phase_sharpes']))
    result['phase_drawdowns'] = [v['max_drawdown'] for v in ss]
    for source, label in [('turn', 'daily_turnover'), ('gross', 'mean_gross'), ('cost', 'annual_cost')]:
        result[label] = float(np.mean([v[source][mask].mean() for v in rr])) * (365 if source == 'cost' else 1)
    result['mean_abs_net'] = float(np.mean([np.abs(v['net'][mask]).mean() for v in rr]))
    result['correlation_baseline'] = float(np.mean([np.corrcoef(v['r'][mask], base[i % len(base)]['r'][mask])[0, 1] for i, v in enumerate(rr)]))
    result['delta_phases'] = [sharpe(v['r'][mask]) - sharpe(base[i % len(base)]['r'][mask]) for i, v in enumerate(rr)]
    result['delta_phase_sd'] = float(np.std(result['delta_phases']))
    return result


def influence(r):
    mu, sd = r.mean(), r.std(ddof=1)
    return np.sqrt(365) * ((r - mu) / sd - mu * ((r - mu) ** 2 - sd ** 2) / (2 * sd ** 3))


def bootstrap(runs, mask, B=9999):
    names = [z for z in runs if z != 'baseline']
    base = runs['baseline']
    bi = np.mean([influence(v['r'][mask]) for v in base], axis=0)
    bs = np.mean([sharpe(v['r'][mask]) for v in base])
    delta = np.array([np.mean([sharpe(v['r'][mask]) for v in runs[z]]) - bs for z in names])
    D = np.array([np.mean([influence(v['r'][mask]) for v in runs[z]], axis=0) - bi for z in names]).T
    D -= D.mean(axis=0)
    N = len(D)
    result = {}
    for L in [15, 30, 60]:
        rng = np.random.default_rng(20261009 + L)
        draws = []
        for j in range(0, B, 200):
            size = min(200, B - j)
            starts = rng.integers(N, size=(size, math.ceil(N / L)))
            indices = ((starts[:, :, None] + np.arange(L)) % N).reshape(size, -1)[:, :N]
            counts = np.array([np.bincount(ii, minlength=N) for ii in indices])
            draws.append(counts @ D / N)
        draws = np.concatenate(draws)
        se = np.maximum(draws.std(axis=0, ddof=1), 1e-12)
        maximum = np.max(draws / se, axis=1)
        result[str(L)] = {z: dict(delta_sharpe=float(delta[j]),
                                  ci95=(delta[j] + np.quantile(draws[:, j], [.025, .975])).tolist(),
                                  p_fwer=float((1 + np.sum(maximum >= delta[j] / se[j])) / (B + 1))) for j, z in enumerate(names)}
    return result


def verify(d, t, rr):
    checks = {}
    # Reconstruct signals using only a prefix: all previous targets must match bit-for-bit.
    cut = int(np.searchsorted(d['times'], pd.Timestamp('2025-01-01', tz='UTC').timestamp() * 1000))
    sub = {k: v[:cut] if k != 'names' else v for k, v in d.items()}
    pt, _ = targets(sub)
    checks['prefix_target_max_error'] = max(float(np.max(np.abs(pt[z] - t[z][:cut]))) for z in t)
    assert checks['prefix_target_max_error'] < 1e-10
    # Independent quantity/equity ledger for phase 0 baseline, including exit assumptions.
    qty = np.zeros(len(d['names']))
    op = pd.DataFrame(d['o']).ffill().values
    eq, last, results = 1e6, None, []
    hashes = np.array([sum(name.encode()) for name in d['names']])
    for i in np.flatnonzero(d['times'] >= START):
        before = eq
        px = op[i]
        if last is not None:
            eq += np.nansum(qty * (px - last))
        want = np.divide(t['baseline'][i - 1] * eq * 2.7, px, out=np.zeros_like(px), where=np.isfinite(px))
        valid = np.isfinite(d['o'][i])
        slot = ((d['times'][i] // DAY + hashes) % 3) == 0
        delta = want - qty
        use = slot & valid & ((np.abs(delta) >= .02 * np.abs(want)) | (want * qty <= 0))
        trade = np.where(use, delta, 0.)
        trade[~valid] = -qty[~valid]
        eq -= np.nansum(np.abs(trade) * px) * .00075
        qty += trade
        results.append(eq / before - 1)
        last = px
    terminal = np.nansum(np.abs(qty) * last) / eq
    results[-1] = (1 + results[-1]) * (1 - terminal * .00075) - 1
    checks['independent_accounting_max_error'] = float(np.max(np.abs(np.array(results) - rr['baseline'][0]['r'])))
    assert checks['independent_accounting_max_error'] < 1e-10
    return checks


def main():
    d = freeze()
    t, eligible = targets(d)
    dates = pd.to_datetime(d['times'][d['times'] >= START], unit='ms', utc=True)
    masks = {'full': np.ones(len(dates), bool), '2023-24': dates.year <= 2024, '2025-26': dates.year >= 2025}
    masks.update({str(y): dates.year == y for y in sorted(set(dates.year))})
    rr = {z: [simulate(d, a, phase=p) for p in range(3)] for z, a in t.items()}
    summary = {z: {period: summarize(v, rr['baseline'], mask) for period, mask in masks.items()} for z, v in rr.items()}
    for z in summary:
        print(z, [round(summary[z][p]['sharpe'], 4) for p in ['full', '2023-24', '2025-26']], flush=True)
    diagnostics = {'daily_rebalance': [simulate(d, t['baseline'], period=1)]}
    for z in t:
        if z.startswith('blend_'):
            ratio = summary[z]['full']['mean_gross'] / summary['baseline']['full']['mean_gross']
            diagnostics['matched_gross_' + z] = [simulate(d, t['baseline'], phase=p, lev=2.7 * ratio) for p in range(3)]
    diagnostic_summary = {z: {p: summarize(v, rr['baseline'], m) for p, m in masks.items()} for z, v in diagnostics.items()}
    checks = verify(d, t, rr)
    boot = {p: bootstrap(rr, masks[p]) for p in ['full', '2023-24', '2025-26']}
    stress = {}
    for label, fee, penalty in [('slippage6bp', .00105, 0), ('maker_cost_only', .00045, 0), ('missing5pct', .00075, .05)]:
        sr = {z: [simulate(d, a, phase=p, fee=fee, penalty=penalty) for p in range(3)] for z, a in t.items()}
        stress[label] = {z: {part: summarize(v, sr['baseline'], masks[part]) for part in ['full', '2023-24', '2025-26']} for z, v in sr.items()}
    old = np.load(ROOT / 'research/distribution_stagger_20261008/panel.npz')
    di = {int(x): i for i, x in enumerate(d['times'])}
    ni = {name: i for i, name in enumerate(d['names'])}
    new_open = d['o'][np.ix_([di[int(x)] for x in old['times']], [ni[name] for name in old['names']])]
    same = np.isfinite(old['O']) & np.isfinite(new_open)
    checks['historical_open_comparison'] = dict(common_cells=int(same.sum()),
                                               changed_cells=int(np.sum(np.abs(new_open[same] - old['O'][same]) > 1e-10)),
                                               old_missing_now_present=int(np.sum(~np.isfinite(old['O']) & np.isfinite(new_open))))
    source_hash = {str(f.relative_to(ROOT)): hashlib.sha256(f.read_bytes()).hexdigest() for f in [Path(__file__), OUT / 'SPEC.md', ROOT / 'src/trader.rs']}
    results = dict(meta=dict(start=str(dates[0]), end=str(dates[-1]), periods={p: int(m.sum()) for p, m in masks.items()},
                            names=len(d['names']), candidate_tests=7, blend_tests=4, seed=20261009, bootstrap_replicates=9999,
                            funding_included=False, independent_holdout=False, source_hash=source_hash,
                            max_missing_exposure={z: float(max(v['missing'].max() for v in r)) for z, r in rr.items()}),
                   summary=summary, diagnostics=diagnostic_summary, bootstrap=boot, stress=stress, checks=checks)
    (OUT / 'results.json').write_text(json.dumps(results, indent=2))
    np.savez_compressed(OUT / 'returns.npz', dates=np.array(dates.astype(str)), **{z: np.array([v['r'] for v in r]) for z, r in rr.items()})
    pd.DataFrame([dict(strategy=z, period=p, **v) for z, q in summary.items() for p, v in q.items()]).to_csv(OUT / 'metrics.csv', index=False)
    pd.DataFrame([dict(date=str(date), strategy=z, phase=phase, **{key: float(a[i]) for key, a in v.items()}) for z, r in rr.items() for phase, v in enumerate(r) for i, date in enumerate(dates)]).to_csv(OUT / 'daily_returns.csv', index=False)
    print('CHECKS', checks, flush=True)
    print('MAXT', boot['full']['30'], flush=True)


if __name__ == '__main__':
    main()
