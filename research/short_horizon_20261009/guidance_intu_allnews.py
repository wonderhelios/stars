"""Audit the unfiltered issuer archive for omitted guidance/interim notices.

Follows observed pagination, retains all titles, reuses existing release files,
and stops on access failure. No prices or additional trading hypotheses.
"""
import json
import re
from urllib.parse import urljoin
from bs4 import BeautifulSoup
from guidance_collect import fetch
from guidance_parse_intu import OUT


def main():
    url = 'https://investors.intuit.com/news-events/press-releases'
    seen, items, errors = set(), {}, []
    end = 'page_cap'
    for page in range(1, 51):
        file = OUT / f'allnews_{page:02}.html'
        try:
            soup = BeautifulSoup(fetch(url, file), 'html.parser')
        except RuntimeError as exc:
            errors.append(dict(url=url, error=str(exc))); end = 'fetch_failure'; break
        rows = []
        for card in soup.select('.media-body'):
            a = card.find('a', href=lambda h: h and '/press-releases/detail/' in h)
            if not a:
                continue
            date = card.find('time', datetime=True)
            assert date
            rows.append(dict(url=urljoin(url, a['href']), title=a.get_text(' ', strip=True),
                             listing_time=date['datetime'], listing_evidence=date.get_text(' ', strip=True),
                             catalogue_url=url, catalogue_file=file.name))
        assert rows
        signature = tuple(x['url'] for x in rows)
        if signature in seen:
            end = 'repeated_page'; break
        seen.add(signature)
        items.update({x['url']: x for x in rows})
        (OUT / 'allnews_catalogue.json').write_text(json.dumps(list(items.values()), indent=2))
        print('page', page, 'items', len(items), rows[-1]['listing_time'], flush=True)
        if all(x['listing_time'] < '2020-01-01' for x in rows):
            end = 'before_2020'; break
        links = {urljoin(url, a['href']) for a in soup.find_all('a', href=True) if 'Next Page' in a.get_text(' ', strip=True)}
        if len(links) != 1:
            end = 'no_unique_next'; break
        url = links.pop()
    selected = [x for x in items.values() if '2020-01-01' <= x['listing_time'][:10] <= '2026-10-07'
                and re.search(r'guidance|outlook|investor day|acquir|acquisition|preliminary', x['title'], re.I)]
    (OUT / 'allnews_review_candidates.json').write_text(json.dumps(selected, indent=2))
    downloaded = []
    if not errors:
        for x in selected:
            ident = re.search(r'/detail/(\d+)/', x['url'])[1]
            file = OUT / f'release_{ident}.html'
            try:
                fetch(x['url'], file)
            except RuntimeError as exc:
                errors.append(dict(url=x['url'], error=str(exc))); break
            downloaded.append(dict(x, file=file.name))
            (OUT / 'allnews_downloaded.json').write_text(json.dumps(downloaded, indent=2))
    status = dict(catalogued=len(items), selected=len(selected), downloaded=len(downloaded),
                  archive_termination=end, errors=errors, market_performance_read=False,
                  title_scan_not_full_content_audit=True)
    (OUT / 'allnews_status.json').write_text(json.dumps(status, indent=2))
    print(json.dumps(status, indent=2), flush=True)


if __name__ == '__main__':
    main()
