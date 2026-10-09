"""Explain historical baseline changes without treating diagnostics as alpha."""
import json
import numpy as np
import pandas as pd
from run import OUT, ROOT, freeze, targets, simulate, summarize

d = freeze()
old = np.load(ROOT / 'research/distribution_stagger_20261008/panel.npz')
stored = json.loads((ROOT / 'research/distribution_stagger_20261008/results.json').read_text())
ti = {int(t): i for i, t in enumerate(d['times'])}
ni = {name: i for i, name in enumerate(d['names'])}
inds = np.ix_([ti[int(x)] for x in old['times']], [ni[n] for n in old['names']])
aligned = {z: d[z][inds] for z in ['o', 'c', 'h', 'l', 'v']}
aligned.update(times=old['times'], names=old['names'])
masked = {z: np.where(np.isfinite(old['O']), aligned[z], np.nan) for z in ['o', 'c', 'h', 'l', 'v']}
masked.update(times=old['times'], names=old['names'])
masked_target, _ = targets(masked)
full_target, _ = targets(d)
aligned_target, _ = targets(aligned)
old_end = old['times'][-2]

def cut(data):
    return {z: a[data['times'] <= old_end] if z != 'names' else a for z, a in data.items()}

cases = [
    ('old_frozen_targets_old_opens', masked, old['base']),
    ('old_coverage_rebuilt_targets', masked, masked_target['baseline']),
    ('new_coverage_same_calendar_warmup', aligned, aligned_target['baseline']),
    ('new_coverage_full_warmup', d, full_target['baseline']),
]
results = {}
for label, data, target in cases:
    runs = [simulate(cut(data), target[data['times'] <= old_end], phase=p) for p in range(3)]
    dates = pd.to_datetime(cut(data)['times'], unit='ms', utc=True)
    dates = dates[dates >= pd.Timestamp('2023-08-14', tz='UTC')]
    masks = {'full': np.ones(len(dates), bool), '2023-24': dates.year <= 2024, '2025-26': dates.year >= 2025}
    results[label] = {z: summarize(runs, runs, m) for z, m in masks.items()}
    print(label, [results[label][z]['sharpe'] for z in masks])

extra = np.isfinite(aligned['o']) & ~np.isfinite(old['O'])
traded_signal = (old['times'] >= old['times'][45]) & (old['times'] < old_end)
overlap_target_error = float(np.max(np.abs(masked_target['baseline'] - old['base'])[traded_signal]))
baseline_error = abs(results['old_frozen_targets_old_opens']['full']['sharpe'] - stored['summary']['baseline']['full']['sharpe'])
assert baseline_error < 1e-10
result = dict(cases=results, frozen_baseline_sharpe_error=baseline_error,
              old_coverage_target_max_error=overlap_target_error,
              added_positive_volume_rows=int(np.sum(extra & (aligned['v'] > 0))),
              added_zero_volume_rows=int(np.sum(extra & (aligned['v'] == 0))),
              extra_rows_by_coin={str(name): int(extra[:, j].sum()) for j, name in enumerate(old['names']) if extra[:, j].any()},
              note='Mask uses old open availability. Closes/volumes from new cache may still differ, so only frozen-target case exactly reproduces original input.')
(OUT / 'reconciliation.json').write_text(json.dumps(result, indent=2))
print('old masked target max error', overlap_target_error)
