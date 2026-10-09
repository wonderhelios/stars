"""Extract publicly planned earnings dates; never infer them from actual releases."""
import hashlib
import json
import re
from collections import Counter
from pathlib import Path
import pandas as pd
from bs4 import BeautifulSoup
from guidance_extract import walk_json

ROOT=Path(__file__).resolve().parent
OUT=ROOT/'earnings_calendar'
QS={'first':1,'second':2,'third':3,'fourth':4}
MONTH=r'(?:Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|Jun(?:e)?|Jul(?:y)?|Aug(?:ust)?|Sep(?:tember|t)?|Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)'
DATE=rf'\b{MONTH}\.?\s+\d{{1,2}}(?:st|nd|rd|th)?(?:\s*,?\s*20\d{{2}})?\b'


def parse(item):
    path=OUT/item['file'];s=BeautifulSoup(path.read_text(),'html.parser');ld=[]
    for script in s.find_all('script',type='application/ld+json'):
        try:ld.extend(walk_json(json.loads(script.get_text())))
        except json.JSONDecodeError:continue
    ld=[x for x in ld if x.get('@type') in ['NewsArticle','Article'] and x.get('datePublished')]
    assert len(ld)==1,(path.name,'article metadata')
    meta=ld[0];pub=pd.Timestamp(meta['datePublished']);mod=pd.Timestamp(meta.get('dateModified',meta['datePublished']))
    assert pub.tzinfo and mod.tzinfo
    if item['kind']=='prn':
        assert pub==pd.Timestamp(item['listing_date'].replace(' ET','')).tz_localize('America/New_York')
    else:
        assert pub.tz_convert('America/New_York').strftime('%Y/%m/%d')==item['date']
        time=s.find('time',datetime=True)
        if time:assert pub==pd.Timestamp(time['datetime'])
    body=s.select_one('.release-body') or s
    text=body.get_text(' ',strip=True).replace('\xa0',' ')
    title=meta['headline']
    # Conference-call time alone cannot establish the financial release time.
    match=re.search(r'\b(?:will|plans to)\s+(?:report|announce|release|publish)\b',text,re.I)
    assert match,(path.name,'no explicit future financial release clause')
    clause=re.split(r'\.\s+(?=[A-Z])',text[match.start():match.start()+1000],maxsplit=1)[0]
    assert re.search(r'results|earnings',clause,re.I),(path.name,clause)
    after=re.search(r'after\s+(?:the\s+)?(?:U\.S\.\s+)?(?:financial\s+)?markets?\s+clos(?:e|es|ing)',clause,re.I)
    before=re.search(r'before\s+(?:the\s+)?(?:U\.S\.\s+)?(?:financial\s+)?markets?\s+open',clause,re.I)
    assert bool(after)!=bool(before),(path.name,'release time ambiguous',clause)
    phase='after_close' if after else 'before_open'
    parsed=[]
    pubday=pub.tz_convert('America/New_York').tz_localize(None).normalize()
    for m in re.finditer(DATE,clause,re.I):
        # The financial period can end on the notice date itself; it is not
        # another scheduled release date (e.g. Veeva's Jan 31, 2023 notice).
        if re.search(r'\b(?:ended|ending)\s*$',clause[:m.start()],re.I):continue
        t=re.sub(r'(\d)(st|nd|rd|th)',r'\1',m.group(),flags=re.I).replace('.','')
        explicit=bool(re.search(r'20\d{2}',t))
        options=[pd.Timestamp(t)] if explicit else [pd.Timestamp(t+f', {year}') for year in [pubday.year,pubday.year+1]]
        options=[d for d in options if 0<=(d-pubday).days<=120]
        if len(options)==1:parsed.append((options[0],m.group(),explicit))
    days={v[0] for v in parsed};assert len(days)==1,(path.name,'non-unique future date',clause,parsed)
    day=next(iter(days));identity=title+' '+clause
    qm=re.search(r'(First|Second|Third|Fourth) Quarter',identity,re.I)
    fy=re.search(r'fiscal(?: year)?\s*(20\d{2})',identity,re.I)
    assert qm and fy,(path.name,'missing explicit fiscal quarter/year',identity)
    return dict(symbol=item['symbol'],source=item['url'],source_file=item['file'],title=title,
        published=str(pub),modified=str(mod),available_after=str(max(pub,mod)+pd.Timedelta(hours=1)),
        fiscal_year=int(fy[1]),quarter=QS[qm[1].lower()],planned_date=str(day.date()),phase=phase,
        planned_date_evidence=[dict(text=t,explicit_year=e) for d,t,e in parsed],release_clause=clause,
        sha256=hashlib.sha256(path.read_bytes()).hexdigest())


def main():
    items=json.loads((OUT/'downloaded.json').read_text());rows=[];issues=[]
    for item in items:
        try:rows.append(parse(item))
        except Exception as e:issues.append(dict(symbol=item['symbol'],source=item['url'],file=item['file'],issue=str(e),type=type(e).__name__))
    rows.sort(key=lambda r:pd.Timestamp(r['available_after']))
    updates=[];old={}
    for row in rows:
        key=(row['symbol'],row['fiscal_year'],row['quarter'])
        if key in old:
            updates.append(dict(symbol=row['symbol'],fiscal_year=row['fiscal_year'],quarter=row['quarter'],
                previous=old[key],current=row))
        old[key]=row
    (OUT/'parsed_notices.json').write_text(json.dumps(rows,indent=2))
    (OUT/'parse_issues.json').write_text(json.dumps(issues,indent=2))
    (OUT/'schedule_updates.json').write_text(json.dumps(updates,indent=2))
    summary=dict(downloaded=len(items),parsed=len(rows),issues=len(issues),schedule_updates=len(updates),
        parsed_by_symbol=dict(Counter(r['symbol'] for r in rows)),issues_by_symbol=dict(Counter(r['symbol'] for r in issues)),
        market_performance_read=False)
    (OUT/'parse_summary.json').write_text(json.dumps(summary,indent=2));print(json.dumps(summary,indent=2))


if __name__=='__main__':main()
