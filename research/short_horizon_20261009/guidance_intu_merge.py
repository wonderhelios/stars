"""Merge source-verified dollar forecasts without reading any market returns.

Keep the press growth proxies intact. This output is a source reconstruction,
not a released signal dataset: historical publication/version and interim-news
coverage remain separate gates. No change to the six registered hypotheses.
"""
import hashlib
import json
import math
from datetime import datetime, timedelta
from pathlib import Path

OUT = Path(__file__).resolve().parent / 'guidance_expansion/INTU'


def read(name):
    return json.loads((OUT / name).read_text())


def mid(values):
    return sum(values) / 2


def build(press, facts, reviews, gates, html):
    facts = {x['release_source']: x for x in facts}
    html = {x['source']: x for x in html}
    reviews = {x['file']: x for x in reviews['fact_sheets']}
    rules = {x['source']: x for x in gates['rules']}
    rows = []
    for p in sorted(press, key=lambda x: datetime.fromisoformat(x['published'])):
        f = facts[p['source']]
        h = html[p['source']]
        assert h['sha256'] == p['sha256']
        assert h['quarter_range_million'] == f['quarter_range_million']
        assert h['annual_range_million'] == f['annual_range_million']
        v = reviews[f['file']]
        assert f['sha256'] == v['sha256']
        assert v['table_verified'] and v['dated_cover_verified'] and v['footnotes_reviewed']
        assert v['cover_date'] == p['published'][:10]
        assert (f['annual_range_million'] is None) == (p['annual_range_million'] is None)
        if f['annual_range_million']:
            assert all(math.isclose(a, b, rel_tol=0, abs_tol=1e-7)
                       for a, b in zip(f['annual_range_million'], p['annual_range_million']))
        if p.get('quarter_range_basis') == 'explicit_original_dollar_range':
            assert p['quarter_range_million'] == f['quarter_range_million']
        # Both original records stay intact. available_after is source time,
        # signal_not_before additionally applies the registered 60-minute lag.
        # Original HTML Table E supplies the signal values. The independently
        # checked PDF may be a later version, so retain its time but do not
        # conflate its metadata with the separate HTML publication evidence.
        available = datetime.fromisoformat(h['available_after'])
        r = dict(p)
        r.update(
            original_press_quarter_range_million=p['quarter_range_million'],
            original_press_quarter_range_basis=p.get('quarter_range_basis'),
            annual_range_million=f['annual_range_million'],
            quarter_range_million=f['quarter_range_million'],
            quarter_range_basis='original_html_table_e_dollar_range' if h['quarter_range_million'] else 'not_provided',
            quarter_forecast_verified_for_trading=False,
            forecast_numeric_source_verified=f['quarter_range_million'] is not None,
            forecast_file=h['source_file'], forecast_source=h['source'], forecast_sha256=h['sha256'],
            forecast_table=h.get('table_text'),
            corroborating_fact_sheet=dict(file=f['file'], source=f['source'], sha256=f['sha256'],
                                         page=f['page'], conservative_source_time=f['conservative_source_time']),
            available_after=available.isoformat(),
            signal_not_before=(available + timedelta(hours=1)).isoformat(),
            pdf_later_version=f['modified_after_release'],
            metadata_timezone_unknown=f['metadata_timezone_unknown'],
            publication_version_verified=False, interim_coverage_complete=False,
            source_gate=rules.get(p['source']), trading_ready=False,
        )
        rows.append(r)
    pairs = []
    # Use only adjacent original quarterly releases; missing guidance breaks
    # the chain rather than silently jumping to a more convenient older row.
    for prev, cur in zip(rows, rows[1:]):
        reason = None
        consecutive = cur['reported_fiscal_year'] * 4 + cur['reported_quarter'] == prev['reported_fiscal_year'] * 4 + prev['reported_quarter'] + 1
        if not consecutive:
            reason = 'nonconsecutive_quarter'
        elif not all([prev['annual_range_million'], cur['annual_range_million'], prev['quarter_range_million']]):
            reason = 'missing_guidance'
        elif not (prev['fiscal_year'] == cur['fiscal_year'] == cur['reported_fiscal_year']):
            reason = 'different_forecast_year'
        elif not (prev['next_quarter'] == cur['reported_quarter'] and prev['quarter_guidance_year'] == cur['reported_fiscal_year']):
            reason = 'quarter_forecast_target_mismatch'
        elif datetime.fromisoformat(prev['available_after']) >= datetime.fromisoformat(cur['published']):
            reason = 'benchmark_version_not_available_before_results'
        pair = dict(previous_source=prev['source'], source=cur['source'], published=cur['published'],
                    numeric_comparison_valid=reason is None, invalid_reason=reason,
                    source_gate=cur['source_gate'], trading_ready=False)
        if reason is None:
            annual = mid(cur['annual_range_million']) - mid(prev['annual_range_million'])
            beat = cur['actual_revenue_million'] - mid(prev['quarter_range_million'])
            value = annual - beat
            proxy = prev['original_press_quarter_range_million']
            proxy_value = annual - (cur['actual_revenue_million'] - mid(proxy)) if proxy else None
            pair.update(annual_revision_million=annual, realized_quarter_beat_million=beat,
                        remaining_year_revision_million=value,
                        rounded_proxy_revision_million=proxy_value,
                        proxy_changed_positive_sign=proxy_value is not None and (value > 0) != (proxy_value > 0),
                        passes_known_scope_gates=cur['source_gate'] is None,
                        signal_not_before=cur['signal_not_before'])
        pairs.append(pair)
    return rows, pairs


def main():
    press, facts, reviews, gates = [read(x) for x in ['parsed_guidance.json', 'facts_parsed.json', 'visual_review_progress.json', 'source_review_gates.json']]
    sec = read('sec_crosscheck.json')
    assert sec['verified'] == len(press) == len(facts) == 27 and not sec['issues']
    for p in press:
        assert hashlib.sha256((OUT.parent / p['source_file']).read_bytes()).hexdigest() == p['sha256']
    for f in facts:
        assert hashlib.sha256((OUT / f['file']).read_bytes()).hexdigest() == f['sha256']
    html = read('html_exact_guidance.json')
    rows, pairs = build(press, facts, reviews, gates, html)
    # Every historical source prefix produces exactly the same already-known
    # rows and comparisons: future fact sheets cannot revise past benchmarks.
    for n in range(1, len(press) + 1):
        sources = {r['source'] for r in rows[:n]}
        prefix_rows, prefix_pairs = build([x for x in press if x['source'] in sources],
            [x for x in facts if x['release_source'] in sources], reviews, gates,
            [x for x in html if x['source'] in sources])
        assert prefix_rows == rows[:n] and prefix_pairs == pairs[:max(n - 1, 0)]
    valid = [x for x in pairs if x['numeric_comparison_valid']]
    known = [x for x in valid if x['passes_known_scope_gates']]
    summary = dict(quarterly_rows=len(rows), exact_quarter_ranges=sum(x['forecast_numeric_source_verified'] for x in rows),
                   numeric_comparisons=len(valid), known_gate_exclusions=len(valid)-len(known),
                   positive_before_pending_audits=sum(x['remaining_year_revision_million'] > 0 for x in known),
                   proxy_sign_changes=sum(x['proxy_changed_positive_sign'] for x in valid),
                   source_prefix_checks=len(press), all_hash_checks_passed=True,
                   market_performance_read=False, trading_ready=False,
                   exact_numeric_basis='Original HTML Table E; PDFs crosschecked independently, later PDF timestamps retained separately.',
                   limitations=['Original issuer HTML lacks an independently archived modification history; dated PDFs do not prove first web availability.',
                                'Interim investor-day/call and all-news coverage still incomplete.',
                                'Numeric source verification is not strategy validation or a clean out-of-sample result.'])
    for name, data in [('exact_guidance.json', rows), ('exact_pairs.json', pairs), ('exact_merge_audit.json', summary)]:
        (OUT / name).write_text(json.dumps(data, indent=2))
    print(json.dumps(summary, indent=2))


if __name__ == '__main__':
    main()
