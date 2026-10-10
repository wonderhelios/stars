"""Issuer financial archive, including interim revisions/reaffirmations; no prices."""
import json
import re
import shutil
from urllib.parse import urljoin
from bs4 import BeautifulSoup
from guidance_collect import fetch
from screen import ROOT

OUT=ROOT/'guidance_expansion/INTU'
START='https://investors.intuit.com/news-events/press-releases?category=financial&page=1'


def main():
    if not (OUT/'catalogue_01.html').exists():
        shutil.copyfile(OUT/'probe.html',OUT/'catalogue_01.html')
        shutil.copyfile(OUT/'probe.meta.json',OUT/'catalogue_01.meta.json')
    url=START;seen=set();items={};end='page_cap';errors=[]
    for page in range(1,31):
        try:s=BeautifulSoup(fetch(url,OUT/f'catalogue_{page:02}.html'),'html.parser')
        except RuntimeError as e:
            errors.append(dict(url=url,error=str(e)));end='fetch_failure';break
        rows=[]
        for card in s.select('.media-body'):
            a=card.find('a',href=lambda h:h and '/press-releases/detail/' in h);time=card.find('time',datetime=True)
            if not a:continue
            assert time
            item=dict(symbol='INTU',title=a.get_text(' ',strip=True),url=a['href'],listing_time=time['datetime'],
                listing_evidence=time.get_text(' ',strip=True),catalogue_url=url,catalogue_file=f'catalogue_{page:02}.html')
            rows.append(item);items[item['url']]=item
        assert rows,'No parsed original financial archive cards'
        signature=tuple(x['url'] for x in rows)
        if signature in seen:end='repeated_set';break
        seen.add(signature)
        (OUT/'catalogue.json').write_text(json.dumps(list(items.values()),indent=2))
        print('catalogue',page,len(items),rows[-1]['listing_time'],flush=True)
        if all(r['listing_time']<'2020-01-01' for r in rows):end='before_2020';break
        nexts={urljoin(url,a['href']) for a in s.find_all('a',href=True) if 'Next Page' in a.get_text(' ',strip=True)}
        if len(nexts)!=1:end='no_unique_next';break
        url=nexts.pop()
    quarterly={r['href'] for r in json.loads((OUT/'quarterly_directory_links.json').read_text())}
    selected=[x for x in items.values() if '2020-01-01'<=x['listing_time'][:10]<='2026-10-07'
        and (x['url'] in quarterly or re.search('reports|guidance|outlook|updates|reaffirms',x['title'],re.I))
        and not re.search('to announce',x['title'],re.I)]
    (OUT/'releases.json').write_text(json.dumps(selected,indent=2));downloaded=[]
    if not errors:
        for item in sorted(selected,key=lambda x:x['listing_time']):
            ident=re.search(r'/detail/(\d+)/',item['url'])[1];p=OUT/f'release_{ident}.html'
            try:fetch(item['url'],p)
            except RuntimeError as e:
                errors.append(dict(url=item['url'],error=str(e)));break
            downloaded.append(dict(item,file=p.name));(OUT/'downloaded.json').write_text(json.dumps(downloaded,indent=2))
            print('download',len(downloaded),len(selected),item['title'],flush=True)
    (OUT/'collection_status.json').write_text(json.dumps(dict(catalogued=len(items),selected=len(selected),downloaded=len(downloaded),
        archive_termination=end,errors=errors,complete_download=len(downloaded)==len(selected) and not errors),indent=2))


if __name__=='__main__':main()
