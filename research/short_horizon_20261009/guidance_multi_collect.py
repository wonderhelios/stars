"""Original issuer releases, systematic catalogues, no return data access."""
import json
import re
import sys
from pathlib import Path
from bs4 import BeautifulSoup
from guidance_collect import fetch

ROOT = Path(__file__).resolve().parent/'guidance_expansion'
CONFIG = {
    'MDB': ['https://www.prnewswire.com/news/mongodb%2C-inc./'],
    'ADSK': ['https://www.prnewswire.com/news/autodesk%2C-inc./'],
    'WDAY': ['https://www.prnewswire.com/news/workday-inc./', 'https://www.prnewswire.com/news/workday%2C-inc./'],
    'SNPS': ['https://www.prnewswire.com/news/synopsys%2C-inc./'],
    'DDOG_PRN': ['https://www.prnewswire.com/news/datadog%2C-inc./'],
}


def collect(symbol):
    root=ROOT/symbol;root.mkdir(exist_ok=True)
    catalogue={};releases={};profile_status=[]
    for pidx,base in enumerate(CONFIG[symbol]):
        seen=set();termination='max_pages'
        for page in range(1,41):
            url=f'{base}?page={page}&pagesize=100'
            try:
                soup=BeautifulSoup(fetch(url,root/f'catalogue_{pidx}_{page:02}.html'),'html.parser')
            except RuntimeError as e:
                print(symbol,'catalogue error',e,flush=True)
                termination='fetch_failed';break
            cards=soup.select('a.newsreleaseconsolidatelink')
            signature=tuple(a.get('href') for a in cards)
            if not cards or signature in seen:
                termination='end_or_repeated';break
            seen.add(signature);dates=[]
            for a in cards:
                href=a.get('href','')
                title=a.h3.get_text(' ',strip=True) if a.h3 else a.get_text(' ',strip=True)
                date=a.find('small').get_text(' ',strip=True) if a.find('small') else None
                dates.append(date)
                item=dict(symbol=symbol.split('_')[0],title=title,listing_date=date,profile=base,page=page,url='https://www.prnewswire.com'+href)
                catalogue[href]=item
                if (('result' in href and re.search('quarter|full.year|fiscal.year',href)) or re.search('preliminary|updates?.*(?:guidance|outlook)',href)) and not re.search('to-(?:announce|report|release)|announces?-date|sets?-date',href):
                    if not date or not re.search(r'20\d{2}',date) or int(re.search(r'20\d{2}',date).group())>=2020:
                        releases[href]=item
            (root/'catalogue.json').write_text(json.dumps(list(catalogue.values()),indent=2))
            (root/'releases.json').write_text(json.dumps(list(releases.values()),indent=2))
            print(symbol,pidx,page,len(cards),dates[-1],'releases',len(releases),flush=True)
            dated=[d for d in dates if d and re.search(r'20\d{2}',d)]
            if dated and all(int(re.search(r'20\d{2}',d).group())<2020 for d in dated):
                termination='before_2020';break
        profile_status.append(dict(profile=base,termination=termination,pages=page))
        (root/'profile_status.json').write_text(json.dumps(profile_status,indent=2))
        if termination=='fetch_failed':
            # Do not probe another spelling or profile after source denial.
            return
    successful=[]
    for item in releases.values():
        ident=item['url'].split('-')[-1].replace('.html','')
        file=root/f'release_{ident}.html'
        try:
            fetch(item['url'],file)
        except RuntimeError as e:
            print(symbol,'release error',e,flush=True);break
        successful.append(dict(**item,file=file.name))
        (root/'downloaded.json').write_text(json.dumps(successful,indent=2))
        print(symbol,'download',item['title'],flush=True)
    (root/'collection_status.json').write_text(json.dumps(dict(catalogued_releases=len(releases),downloaded=len(successful),
        complete_download=len(successful)==len(releases),profiles=profile_status),indent=2))


if __name__=='__main__':
    for symbol in sys.argv[1:] or list(CONFIG):
        collect(symbol)
