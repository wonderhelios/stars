"""Independently recover exact guidance from original HTML Table E.

The earlier prose parser missed these reconciliation tables. Retain its proxy
output as an audit trail, and compare every exact value against the fact sheet.
"""
import hashlib
import json
import re
from datetime import datetime
from pathlib import Path
from bs4 import BeautifulSoup

OUT = Path(__file__).resolve().parent / 'guidance_expansion/INTU'


def main():
    press = json.loads((OUT / 'parsed_guidance.json').read_text())
    facts = {x['release_source']: x for x in json.loads((OUT / 'facts_parsed.json').read_text())}
    rows = []
    for p in press:
        raw = (OUT.parent / p['source_file']).read_bytes()
        assert hashlib.sha256(raw).hexdigest() == p['sha256']
        soup = BeautifulSoup(raw, 'html.parser')
        tables = [t for t in soup.find_all('table') if 'Forward-Looking Guidance' in t.get_text(' ', strip=True)]
        assert len(tables) <= 1
        f = facts[p['source']]
        row = dict(source=p['source'], source_file=p['source_file'], sha256=p['sha256'],
                   available_after=p['available_after'], table_found=bool(tables))
        if tables:
            t = tables[0]
            text = re.sub(r'\s+', ' ', t.get_text(' ', strip=True))
            header = text[:500]
            if 'RECONCILIATION OF FORWARD-LOOKING GUIDANCE' not in header:
                # One original release puts the table heading in preceding p's.
                preceding = ' '.join(x.get_text(' ', strip=True) for x in list(t.find_previous_siblings())[:5])
                header = preceding + ' ' + header
            assert 'RECONCILIATION OF FORWARD-LOOKING GUIDANCE' in header
            assert 'In millions' in header and re.search(r'GAAP R\s*ange of Estimate', text)
            values = re.findall(r'(Three|Twelve) Months Ending (.+?) Revenue \$ ([\d,]+) \$ ([\d,]+)', text)
            assert len(values) == 2 and [x[0] for x in values] == ['Three', 'Twelve']
            q, year = values
            qend = datetime.strptime(q[1], '%B %d, %Y').date()
            yend = datetime.strptime(year[1], '%B %d, %Y').date()
            expected_month = {1: 10, 2: 1, 3: 4, 4: 7}[p['next_quarter']]
            assert qend.month == expected_month and qend.year == p['fiscal_year'] - (p['next_quarter'] == 1)
            assert (yend.year, yend.month, yend.day) == (p['fiscal_year'], 7, 31)
            qr = [float(x.replace(',', '')) for x in q[2:]]
            yr = [float(x.replace(',', '')) for x in year[2:]]
            assert qr == f['quarter_range_million'] and yr == f['annual_range_million']
            row.update(quarter_range_million=qr, annual_range_million=yr,
                       quarter_ending=str(qend), fiscal_year_ending=str(yend),
                       table_header=header, table_text=text, fact_sheet_match=True)
        else:
            assert f['quarter_range_million'] is None and f['annual_range_million'] is None
            row.update(quarter_range_million=None, annual_range_million=None, fact_sheet_match=True)
        rows.append(row)
    summary = dict(releases=len(rows), exact_tables=sum(x['table_found'] for x in rows),
                   all_fact_sheet_matches=True, market_performance_read=False,
                   correction='Exact dollar guidance was present in original HTML appendices; prose-only extraction missed it. PDF is independent crosscheck, not the only numeric source.',
                   limitation='Current original issuer HTML still lacks a historical modification log; no claim of independently archived point-in-time versions.')
    (OUT / 'html_exact_guidance.json').write_text(json.dumps(rows, indent=2))
    (OUT / 'html_table_audit.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary, indent=2))


if __name__ == '__main__':
    main()
