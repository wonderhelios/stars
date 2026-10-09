"""Collect a complete date-ordered company release catalogue, without returns.

Independent public distribution site; never retry access-denied responses.
"""
import hashlib
import json
import re
import subprocess
import time
from pathlib import Path
from bs4 import BeautifulSoup

ROOT = Path(__file__).resolve().parent / 'guidance_probe'
CAT = ROOT / 'catalogue'
CAT.mkdir(exist_ok=True)


def fetch(url, dest):
    meta = dest.with_suffix('.meta.json')
    attempts = []
    if meta.exists():
        m = json.loads(meta.read_text())
        if m['status'] == 200:
            return dest.read_text()
        attempts = m.get('attempts', [m])
        if m['status'] != 0 or len(attempts) >= 2:
            raise RuntimeError(f'Previous failure: {url}: {m}')
    r = subprocess.run(['curl', '-L', '--max-time', '60', '-sS', '-o', str(dest), '-w', '%{http_code}', url], capture_output=True, text=True)
    status = int(r.stdout) if r.stdout.isdigit() else 0
    m = dict(url=url, status=status, exit_code=r.returncode, error=r.stderr,
             sha256=hashlib.sha256(dest.read_bytes()).hexdigest() if dest.exists() else None,
             retrieved_utc=time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()))
    attempts.append(m.copy())
    m['attempts'] = attempts
    meta.write_text(json.dumps(m, indent=2))
    if status == 0 and len(attempts) < 2:
        time.sleep(3)
        return fetch(url, dest)
    if status != 200 or r.returncode:
        raise RuntimeError(f'Fetch failed: {m}')
    time.sleep(1)
    return dest.read_text()


def main():
    releases = {}
    seen_pages = set()
    for page in range(1, 41):
        url = f'https://www.prnewswire.com/news/veeva-systems/?page={page}&pagesize=100'
        html = fetch(url, CAT / f'veeva_{page:02}.html')
        soup = BeautifulSoup(html, 'html.parser')
        cards = soup.select('a.newsreleaseconsolidatelink')
        signature = tuple(a.get('href') for a in cards)
        if not cards or signature in seen_pages:
            print('Catalogue ended/repeated', page, flush=True)
            break
        seen_pages.add(signature)
        dates = []
        for a in cards:
            href = a.get('href', '')
            title = a.h3.get_text(' ', strip=True) if a.h3 else a.get_text(' ', strip=True)
            date = a.find('small').get_text(' ', strip=True) if a.find('small') else None
            dates.append(date)
            # Only company earnings releases, not conference announcements or translations.
            if re.search(r'veeva-announces-(?:fiscal-\d{4}-(?:first|second|third)-quarter|fourth-quarter-and-fiscal-year-\d{4})-results-', href):
                releases[href] = dict(symbol='VEEV', title=title, listing_date=date, catalogue_page=page,
                    url='https://www.prnewswire.com'+href)
        (ROOT / 'veeva_release_catalogue.json').write_text(json.dumps(list(releases.values()), indent=2))
        print(page, len(cards), dates[0], dates[-1], 'earnings', len(releases), flush=True)
        dated = [d for d in dates if d and re.search(r'20\d{2}', d)]
        if dated and all(int(re.search(r'20\d{2}', d).group()) < 2020 for d in dated):
            break
    for item in releases.values():
        year = int(re.search(r'(20\d{2})', item['listing_date']).group()) if item['listing_date'] and re.search(r'20\d{2}', item['listing_date']) else 2026
        if year < 2019:
            continue
        ident = item['url'].split('-')[-1].replace('.html', '')
        dest = ROOT / f'VEEV_PRN_{ident}.html'
        fetch(item['url'], dest)
        print('download', item['title'], flush=True)


if __name__ == '__main__':
    main()
