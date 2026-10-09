"""Official minutes with forward meeting schedules; no price access."""
import json
import re
import shutil
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from bs4 import BeautifulSoup
from guidance_collect import fetch

ROOT=Path(__file__).resolve().parent
OUT=ROOT/'macro_calendar'


def main():
    s=BeautifulSoup((ROOT/'options_events/raw/fomccalendars.html').read_text(),'html.parser')
    items={}
    for block in s.select('.fomc-meeting'):
        if 'notation vote' in block.get_text(' ',strip=True).lower():continue
        for a in block.find_all('a',href=True):
            match=re.fullmatch(r'/monetarypolicy/fomcminutes(\d{8})\.htm',a['href'])
            if not match:continue
            released=re.search(r'\(Released ([A-Za-z]+ \d{1,2}, 20\d{2})\)',block.get_text(' ',strip=True))
            assert released
            items[match[1]]=dict(meeting_id=match[1],url='https://www.federalreserve.gov'+a['href'],published_date=released[1],publication_evidence=block.get_text(' ',strip=True))
    items['20201216']=dict(meeting_id='20201216',url='https://www.federalreserve.gov/monetarypolicy/fomcminutes20201216.htm',published_date='January 06, 2021',publication_evidence='Official December 2020 press-conference page: Released January 06, 2021 at 2:00 p.m.',publication_source='https://www.federalreserve.gov/monetarypolicy/fomcpresconf20201216.htm')
    (OUT/'minutes_catalogue.json').write_text(json.dumps(list(items.values()),indent=2))
    # Existing successful sample, with its original acquisition metadata.
    dest=OUT/'minutes_20231101.html'
    if not dest.exists():
        shutil.copyfile(OUT/'fomc_probe.html',dest);shutil.copyfile(OUT/'fomc_probe.meta.json',dest.with_suffix('.meta.json'))
    def one(item):
        dest=OUT/('minutes_'+item['meeting_id']+'.html')
        try:fetch(item['url'],dest);return dict(item,file=dest.name),None
        except RuntimeError as e:
            meta=json.loads(dest.with_suffix('.meta.json').read_text())
            return None,dict(url=item['url'],error=str(e),status=meta['status'])
    done=[];errors=[];values=sorted(items.values(),key=lambda r:r['meeting_id'])
    with ThreadPoolExecutor(2) as pool:
        for i in range(0,len(values),2):
            for good,error in pool.map(one,values[i:i+2]):
                if error:errors.append(error);print('ERROR',error['url'],error['status'],flush=True)
                else:done.append(good);print(len(done),len(values),good['meeting_id'],flush=True)
            (OUT/'downloaded.json').write_text(json.dumps(done,indent=2))
            if any(e['status'] in [403,429] for e in errors):break
    (OUT/'collection_status.json').write_text(json.dumps(dict(catalogued=len(values),downloaded=len(done),errors=errors,finished=True),indent=2))


if __name__=='__main__':main()
