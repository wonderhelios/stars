"""Collect ex-ante earnings scheduling notices; no price access."""
import json
import re
import shutil
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from bs4 import BeautifulSoup
from guidance_collect import fetch

ROOT=Path(__file__).resolve().parent
OUT=ROOT/'earnings_calendar'
OUT.mkdir(exist_ok=True)


def relevant(title):
    return bool(re.search(r'earnings|(?:financial|quarter|year).{0,30}results',title,re.I) and
        re.search(r'to (?:announce|report|release)|announces?.*(?:date|conference call)|extends invitation|reschedul|postpon|delay',title,re.I))


def main():
    items={}
    for sym in ['MDB','ADSK','SNPS','WDAY','DDOG_PRN','DDOG']:
        for row in json.loads((ROOT/'guidance_expansion'/sym/'catalogue.json').read_text()):
            if relevant(row['title']):items[row['url']]=dict(row,kind='prn' if 'prnewswire' in row['url'] else 'globe')
    # Reuse the full saved Veeva catalogue, not its results-only subset.
    for file in sorted((ROOT/'guidance_probe/catalogue').glob('veeva_*.html')):
        for a in BeautifulSoup(file.read_text(),'html.parser').select('a.newsreleaseconsolidatelink'):
            title=a.h3.get_text(' ',strip=True) if a.h3 else a.get_text(' ',strip=True)
            if not relevant(title):continue
            url='https://www.prnewswire.com'+a['href']
            items[url]=dict(symbol='VEEV',title=title,url=url,listing_date=a.find('small').get_text(' ',strip=True),kind='prn')
    # Source snapshots acquired earlier: only actually matching notices qualify.
    api=json.loads((ROOT/'guidance_expansion/CRM/api_results_01.json').read_text())
    crm=[r for r in api if relevant(r['title']['rendered'])]
    (OUT/'crm_existing_api_notices.json').write_text(json.dumps(crm,indent=2))
    dated=[]
    for row in items.values():
        date=row.get('listing_date') or row.get('date')
        if date and re.search(r'20\d{2}',date) and int(re.search(r'20\d{2}',date).group())<2020:continue
        dated.append(row)
    (OUT/'notice_catalogue.json').write_text(json.dumps(dated,indent=2))
    # Reuse successful probes to avoid unnecessary requests.
    saved={json.loads(p.read_text())['url']:p for p in OUT.glob('*_probe.meta.json')}
    work=[]
    for i,row in enumerate(sorted(dated,key=lambda r:(r['symbol'],r.get('listing_date','')))):
        ident=re.search(r'(\d+)\.html',row['url'])
        name=row['symbol']+'_'+(ident[1] if ident else str(i))+'.html'
        dest=OUT/name
        if row['url'] in saved and not dest.exists():
            meta=saved[row['url']];shutil.copyfile(meta.with_suffix('.html').with_name(meta.name.replace('.meta.json','.html')),dest)
            shutil.copyfile(meta,dest.with_suffix('.meta.json'))
        work.append((row,dest))
    def one(pair):
        row,dest=pair
        try:
            fetch(row['url'],dest)
            return dict(row,file=dest.name),None
        except RuntimeError as e:
            meta=json.loads(dest.with_suffix('.meta.json').read_text())
            return None,dict(source=row['url'],symbol=row['symbol'],error=str(e),status=meta['status'])
    done=[];errors=[];denied=set()
    # Bounded independent requests; never retry a refused or exhausted URL.
    # An ordinary TLS failure at one URL does not imply absent issuer notices.
    with ThreadPoolExecutor(max_workers=3) as pool:
        for start in range(0,len(work),3):
            batch=[p for p in work[start:start+3] if p[0]['kind'] not in denied]
            for good,error in pool.map(one,batch):
                if error:
                    errors.append(error)
                    if error['status'] in [403,429]:
                        denied.add('prn' if 'prnewswire' in error['source'] else 'globe')
                    print('ERROR',error['source'],error['status'],flush=True)
                else:
                    done.append(good);print(len(done),len(dated),good['symbol'],good['title'],flush=True)
            (OUT/'downloaded.json').write_text(json.dumps(done,indent=2))
            (OUT/'collection_status.json').write_text(json.dumps(dict(catalogued=len(dated),downloaded=len(done),errors=errors,denied_providers=sorted(denied),complete_download=False),indent=2))
    (OUT/'collection_status.json').write_text(json.dumps(dict(catalogued=len(dated),downloaded=len(done),errors=errors,
        crm_existing_api_notices=len(crm),complete_download=len(done)==len(dated)),indent=2))


if __name__=='__main__':main()
