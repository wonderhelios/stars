"""Verify unfiltered catalogue evidence and six investor-day reaffirmations."""
import hashlib
import json
import re
from datetime import datetime
from zoneinfo import ZoneInfo
from bs4 import BeautifulSoup
from guidance_intu_merge import OUT, read


def main():
    status = read('allnews_status.json')
    assert not status['errors'] and status['archive_termination'] == 'before_2020'
    catalog = {x['url']: x for x in read('allnews_catalogue.json')}
    press = read('exact_guidance.json')
    original = read('downloaded.json')
    items = read('allnews_downloaded.json')
    index, reaffirmations = [], []
    for item in items:
        file = OUT / item['file']
        raw = file.read_bytes()
        meta = json.loads(file.with_suffix('.meta.json').read_text())
        assert meta['status'] == 200 and meta['sha256'] == hashlib.sha256(raw).hexdigest()
        soup = BeautifulSoup(raw, 'html.parser')
        body = soup.select_one('.main-content')
        dt = body.find('time', datetime=True)
        assert dt['datetime'] == catalog[item['url']]['listing_time']
        pub = datetime.fromisoformat(dt['datetime']).replace(tzinfo=ZoneInfo('America/New_York'))
        assert pub.tzname() in dt.get_text(' ', strip=True)
        text = re.sub(r'\s+', ' ', body.get_text(' ', strip=True))
        index.append(dict(source=item['url'], file=item['file'], sha256=meta['sha256'],
                          published=pub.isoformat(), title=item['title'],
                          full_content_semantic_audit_complete=False))
        if 'Investor Day' not in item['title'] or 'Reaffirms' not in item['title']:
            continue
        prior = max([x for x in press if datetime.fromisoformat(x['published']) < pub],
                    key=lambda x: datetime.fromisoformat(x['published']))
        tables = [t.get_text(' ', strip=True) for t in body.find_all('table')
                  if 'Forward-Looking Guidance' in t.get_text(' ', strip=True)]
        if tables:
            assert len(tables) == 1
            pairs = re.findall(r'(Three|Twelve) Months Ending (.+?) Revenue \$ ([\d,]+) \$ ([\d,]+)',
                               re.sub(r'\s+', ' ', tables[0]))
            assert len(pairs) == 2 and [x[0] for x in pairs] == ['Three', 'Twelve']
            quarter, annual = [[float(s.replace(',', '')) for s in x[2:]] for x in pairs]
            quote = tables[0]
        else:
            assert 'Fiscal 2027 Guidance' in item['title']
            pairs = re.findall(r'Total Revenue \$([\d,]+) to \$([\d,]+)', text)
            assert len(pairs) == 2
            annual, quarter = [[float(s.replace(',', '')) for s in x] for x in pairs]
            quote = text[text.index('Full Year Fiscal 2027 Guidance'):text.index('Investor Day: How to Participate')]
        assert quarter == prior['quarter_range_million'] and annual == prior['annual_range_million']
        reaffirmations.append(dict(source=item['url'], sha256=meta['sha256'], published=pub.isoformat(),
                                   prior_quarter_source=prior['source'], fiscal_year=prior['fiscal_year'],
                                   annual_range_million=annual, quarter_range_million=quarter,
                                   exact_reaffirmation=True, new_revision_signal=False, evidence=quote))
    assert len(reaffirmations) == 6
    for x in press:
        assert catalog[x['source']]['listing_time'].replace('T', ' ') == x['published'][:19]
    summary = dict(catalogued_titles=len(catalog), selected_originals=len(items),
                   new_originals=len({x['url'] for x in items} - {x['url'] for x in original}),
                   union_originals=len({x['url'] for x in items} | {x['url'] for x in original}),
                   quarterly_time_matches=len(press), selected_hash_time_matches=len(index),
                   exact_investor_day_reaffirmations=len(reaffirmations), market_performance_read=False,
                   remaining=['Title-filtered candidate set is not a full content review of all 390 releases.',
                              'GoCo/SeedFi/Zendrive closing and revenue-scope treatment need filing/call evidence.',
                              'Investor-day decks and unscripted call remarks not exhaustively archived.',
                              'Historical web modification versions still not independently verified.'])
    for name, data in [('allnews_notice_index.json', index), ('interim_reaffirmation_audit.json', reaffirmations),
                       ('allnews_audit.json', summary)]:
        (OUT / name).write_text(json.dumps(data, indent=2))
    print(json.dumps(summary, indent=2))


if __name__ == '__main__':
    main()
