"""Follow observed public archive pagination; no trading inputs."""
import json
import re
import sys
from pathlib import Path
from urllib.parse import urljoin
from bs4 import BeautifulSoup
from guidance_collect import fetch

ROOT=Path(__file__).resolve().parent/'guidance_expansion'
STARTS={'DDOG':'https://www.globenewswire.com/en/search/organization/Datadogδ%2520Inc§',
        'CRM':'https://www.salesforce.com/news/topics/earnings/'}


def collect(symbol):
    root=ROOT/symbol;root.mkdir(exist_ok=True)
    url=STARTS[symbol];seen=set();signatures=set();allitems={};releases={};end='page_cap'
    for page in range(1,61):
        if url in seen:
            end='repeated_url';break
        seen.add(url)
        try: soup=BeautifulSoup(fetch(url,root/f'catalogue_{page:02}.html'),'html.parser')
        except RuntimeError as e:
            print(symbol,e,flush=True);return
        current=[]
        for a in soup.find_all('a',href=True):
            href=urljoin(url,a['href']);title=a.get_text(' ',strip=True)
            if not title:continue
            if symbol=='DDOG':
                valid=href.startswith('https://www.globenewswire.com/news-release/')
                date=re.search(r'/news-release/(20\d{2})/(\d{2})/(\d{2})/',href)
            else:
                valid=href.startswith('https://www.salesforce.com/news/press-releases/')
                date=re.search(r'/press-releases/(20\d{2})/(\d{2})/(\d{2})/',href)
            if not valid or not date:continue
            item=dict(symbol=symbol,title=title,url=href,date='/'.join(date.groups()),page=page)
            allitems[href]=item;current.append(item)
            if int(date.group(1))>=2020 and re.search(r'results|preliminary|updates?.*(?:guidance|outlook)',title,re.I) and not re.search('to (?:announce|report)|announces date',title,re.I):
                releases[href]=item
        (root/'catalogue.json').write_text(json.dumps(list(allitems.values()),indent=2))
        (root/'releases.json').write_text(json.dumps(list(releases.values()),indent=2))
        print(symbol,page,'items',len(current),'financial',len(releases),flush=True)
        signature=tuple(sorted(r['url'] for r in current))
        if signature in signatures:
            end='repeated_article_set';break
        signatures.add(signature)
        if not current:
            end='unparsed_or_empty_page';break
        if all(int(r['date'][:4])<2020 for r in current):
            end='before_2020';break
        nexts=[a for a in soup.find_all('a',href=True) if a.get_text(' ',strip=True).lower() in ['next page','older posts']]
        if len(nexts)!=1:
            end='no_unique_next';break
        url=urljoin(url,nexts[0]['href'])
    (root/'profile_status.json').write_text(json.dumps(dict(termination=end,pages=page,profile=STARTS[symbol]),indent=2))
    downloaded=[]
    for i,item in enumerate(sorted(releases.values(),key=lambda r:r['date'])):
        file=root/f'release_{i:03}.html'
        try: fetch(item['url'],file)
        except RuntimeError as e:
            print(symbol,e,flush=True);break
        downloaded.append(dict(**item,file=file.name))
        (root/'downloaded.json').write_text(json.dumps(downloaded,indent=2))
        print(symbol,'download',item['date'],item['title'],flush=True)
    (root/'collection_status.json').write_text(json.dumps(dict(catalogued_releases=len(releases),downloaded=len(downloaded),
        complete_download=len(downloaded)==len(releases),archive_termination=end),indent=2))


if __name__=='__main__':
    collect(sys.argv[1])
