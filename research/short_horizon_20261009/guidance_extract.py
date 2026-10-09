"""Extract source-anchored forward guidance; no market-return inputs."""
import hashlib
import json
import re
from pathlib import Path
from bs4 import BeautifulSoup

ROOT = Path(__file__).resolve().parent / 'guidance_probe'


def walk_json(x):
    if isinstance(x, dict):
        yield x
        for v in x.values():
            yield from walk_json(v)
    elif isinstance(x, list):
        for v in x:
            yield from walk_json(v)


def dollar_range(text):
    vals = [float(v.replace(',', '')) for v in re.findall(r'\$\s*([\d,]+(?:\.\d+)?)', text)]
    if 'at least' in text.lower() or not vals or len(vals) > 2:
        return None
    if len(vals) == 1:
        vals *= 2
    if vals[0] > vals[1]:
        raise ValueError(text)
    return vals


def guidance_bullets(p):
    result = []
    for sibling in p.next_siblings:
        if getattr(sibling, 'name', None) is None:
            continue
        if sibling.name != 'ul':
            break
        result.extend(li.get_text(' ', strip=True).replace('\xa0', ' ') for li in sibling.find_all('li', recursive=False))
    return result


def extract(item, file):
    soup = BeautifulSoup(file.read_text(), 'html.parser')
    nodes = []
    for script in soup.find_all('script', type='application/ld+json'):
        try:
            nodes.extend(walk_json(json.loads(script.string or script.get_text())))
        except json.JSONDecodeError:
            continue
    articles = [n for n in nodes if n.get('@type') in ['NewsArticle', 'Article'] and n.get('datePublished')]
    if len(articles) != 1:
        raise ValueError(f'{file.name}: publication metadata {len(articles)}')
    article = articles[0]
    assert 'Veeva' in article.get('headline', '')
    report_year = int(re.search(r'fiscal(?: year)? (20\d{2})', article['headline'], re.I).group(1))
    actual = []
    next_quarter = []
    for p in soup.find_all(['p', 'li']):
        text = p.get_text(' ', strip=True).replace('\xa0', ' ')
        m = re.search(r'Total revenues for the (first|second|third|fourth) quarter were\s*\$([\d,.]+)\s*million', text, re.I)
        if m:
            actual.append(dict(quarter=m.group(1).lower(), revenue_million=float(m.group(2).replace(',', '')), source_text=text))
        if p.name == 'p' and re.search(r'providing.*guidance.*fiscal (?:first|second|third|fourth) quarter ending', text, re.I):
            m = re.search(r'fiscal (first|second|third|fourth) quarter ending\s+([A-Za-z]+\s+\d{1,2},?\s+20\d{2})', text, re.I)
            ul = p.find_next('ul')
            rev = [li.get_text(' ', strip=True) for li in ul.find_all('li', recursive=False) if re.match(r'Total revenues?\b', li.get_text(' ', strip=True), re.I)] if ul else []
            if not m or len(rev) != 1:
                raise ValueError(f'Unparsed next quarter guidance: {file.name}: {text}')
            numbers = dollar_range(rev[0])
            if numbers and 'billion' in rev[0].lower():
                numbers = [v*1000 for v in numbers]
            elif numbers and 'million' not in rev[0].lower():
                numbers = None
            next_quarter.append(dict(quarter=m.group(1).lower(), ending=m.group(2), revenue_range_million=numbers, source_text=text+' '+rev[0]))
    # A nested tag can expose the same paragraph twice; deduplicate exact facts.
    actual = list({(v['quarter'], v['revenue_million']): v for v in actual}.values())
    if len(actual) > 1 or len(next_quarter) > 1:
        raise ValueError(f'Ambiguous quarter observations: {file.name}')
    rows = []
    for p in soup.find_all('p'):
        text = p.get_text(' ', strip=True).replace('\xa0', ' ')
        if not re.search(r'(?:providing|issuing).*guidance.*fiscal year ending', text, re.I):
            continue
        fiscal = re.search(r'fiscal year ending\s+([A-Za-z]+\s+\d{1,2},?\s+20\d{2})', text, re.I)
        if not fiscal:
            raise ValueError(f'Unparsed annual date: {text}')
        bullets = guidance_bullets(p)
        if not bullets:
            raise ValueError(f'Unparsed annual bullets: {file.name}')
        revenue = [b for b in bullets if re.match(r'Total revenues?\b', b, re.I)]
        eps = [b for b in bullets if re.match(r'Non.GAAP', b, re.I) and re.search(r'(?:per share|earnings per)', b, re.I)]
        row = dict(symbol='VEEV', source=item['url'], source_file=file.name,
            sha256=hashlib.sha256(file.read_bytes()).hexdigest(), title=article['headline'],
            published=article['datePublished'], modified=article.get('dateModified'),
            available_after=max(article['datePublished'], article.get('dateModified') or article['datePublished']),
            fiscal_year_end=fiscal.group(1), reported_fiscal_year=report_year, introduction=text, bullets=bullets,
            revenue_range_million=None, eps_range=None,
            actual_quarter=actual[0] if actual else None,
            next_quarter_guidance=next_quarter[0] if next_quarter else None)
        if len(revenue) == 1:
            numbers = dollar_range(revenue[0])
            if numbers and 'billion' in revenue[0].lower():
                numbers = [v*1000 for v in numbers]
            elif numbers and 'million' not in revenue[0].lower():
                numbers = None
            row['revenue_range_million'] = numbers
        if len(eps) == 1:
            row['eps_range'] = dollar_range(eps[0])
        rows.append(row)
    return rows, dict(source=item['url'], file=file.name, published=article['datePublished'],
                     title=article.get('headline'), annual_blocks=len(rows))


def main():
    catalogue = json.loads((ROOT/'veeva_release_catalogue.json').read_text())
    rows, coverage = [], []
    for item in catalogue:
        ident = item['url'].split('-')[-1].replace('.html', '')
        file = ROOT / f'VEEV_PRN_{ident}.html'
        meta = file.with_suffix('.meta.json')
        if not meta.exists() or json.loads(meta.read_text()).get('status') != 200:
            coverage.append(dict(source=item['url'], issue='not_downloaded'))
            continue
        parsed, c = extract(item, file)
        rows.extend(parsed); coverage.append(c)
    rows.sort(key=lambda r: r['published'])
    previous = {}
    for row in rows:
        key = row['fiscal_year_end']
        prior = previous.get(key)
        row['previous_source'] = prior['source'] if prior else None
        row['previous_publication'] = prior['published'] if prior else None
        for field, output in [('revenue_range_million', 'revenue_midpoint_revision'), ('eps_range', 'eps_midpoint_revision')]:
            old = prior.get(field) if prior else None
            new = row.get(field)
            row[output] = (sum(new)/sum(old)-1) if old and new and sum(old) > 0 else None
        row['remaining_year_revision_million'] = None
        row['realized_quarter_surprise_million'] = None
        aq = row['actual_quarter']
        nq = prior['next_quarter_guidance'] if prior else None
        current_fiscal_year = int(re.search(r'20\d{2}', row['fiscal_year_end']).group()) == row['reported_fiscal_year']
        if current_fiscal_year and prior and aq and nq and aq['quarter'] == nq['quarter'] and nq['revenue_range_million'] and prior['revenue_range_million'] and row['revenue_range_million']:
            surprise = aq['revenue_million'] - sum(nq['revenue_range_million'])/2
            row['realized_quarter_surprise_million'] = surprise
            row['remaining_year_revision_million'] = sum(row['revenue_range_million'])/2-sum(prior['revenue_range_million'])/2-surprise
        previous[key] = row
    (ROOT/'guidance_rows.json').write_text(json.dumps(rows, indent=2))
    (ROOT/'guidance_coverage.json').write_text(json.dumps(coverage, indent=2))
    for r in rows:
        print(r['published'][:10], r['fiscal_year_end'], r['revenue_range_million'], r['eps_range'], r['revenue_midpoint_revision'], 'remaining', r['remaining_year_revision_million'], flush=True)
    print('releases', len(coverage), 'annual blocks', len(rows), 'missing', sum('issue' in r for r in coverage), flush=True)


if __name__ == '__main__':
    main()
