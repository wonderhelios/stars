"""Merge issuer releases across its two publishers; preserve coverage gaps."""
import hashlib
import json
import re
from pathlib import Path
import pandas as pd
from bs4 import BeautifulSoup
from guidance_extract import walk_json

ROOT=Path(__file__).resolve().parent/'guidance_expansion'
OUT=ROOT/'DDOG'
QS={'first':1,'second':2,'third':3,'fourth':4}


def amounts(text):
    values=[float(v.replace(',','')) for v in re.findall(r'\$\s*([\d,]+(?:\.\d+)?)',text)]
    assert len(values)==2 and values[1]>=values[0],text
    if 'billion' in text.lower():
        assert 'million' not in text.lower();values=[v*1000 for v in values]
    else:assert 'million' in text.lower()
    return values


def parse(item,folder):
    f=folder/item['file'];s=BeautifulSoup(f.read_text(),'html.parser');records=[]
    for script in s.find_all('script',type='application/ld+json'):
        try:records.extend(walk_json(json.loads(script.get_text())))
        except json.JSONDecodeError:continue
    records=[x for x in records if x.get('@type') in ['NewsArticle','Article'] and x.get('datePublished')]
    if not records and folder.name=='DDOG_NEWSFILE':
        visible=s.get_text(' ',strip=True).replace('\xa0',' ')
        stamp=re.search(r'([A-Za-z]+ \d{1,2}, 20\d{2} \d{1,2}:\d{2} [AP]M) (EST|EDT) \| Source: Datadog, Inc\.',visible)
        assert stamp and s.find('meta',attrs={'name':'author'}).get('content')=='Datadog, Inc.'
        date=pd.Timestamp(stamp.group(1)).tz_localize('America/New_York');assert date.tzname()==stamp.group(2)
        records=[dict(headline=s.find('meta',attrs={'property':'og:title'})['content'],datePublished=str(date),publication_basis='visible Newsfile issuer timestamp, also matched official issuer listing',modified_time_unavailable=True)]
    assert len(records)==1
    x=records[0];title=x['headline'];pub=pd.Timestamp(x['datePublished']);mod=pd.Timestamp(x.get('dateModified',x['datePublished']))
    assert 'Datadog' in title and pub.tzinfo is not None
    time=s.find('time',datetime=True)
    if time:assert pub==pd.Timestamp(time['datetime'])
    else:assert pub==pd.Timestamp(item['listing_date'].replace(' ET','')).tz_localize('America/New_York')
    quarter=QS[re.search(r'(First|Second|Third|Fourth) Quarter',title,re.I).group(1).lower()]
    year_match=re.search(r'20\d{2}',title)
    year=int(year_match.group()) if year_match else pub.tz_convert('America/New_York').year-(quarter==4)
    text=s.get_text(' ',strip=True).replace('\xa0',' ')
    qmatches=list(re.finditer(r'(First|Second|Third|Fourth) Quarter (20\d{2}) Outlook:\s*[•○◦\s]*Revenue between ([^.]+(?:\.[\d]+[^.]+)*?)\.(?=\s|$)',text,re.I))
    ymatches=list(re.finditer(r'(?:Full|Fiscal) Year (20\d{2}) Outlook:\s*[•○◦\s]*Revenue between ([^.]+(?:\.[\d]+[^.]+)*?)\.(?=\s|$)',text,re.I))
    assert len(qmatches)==1 and len(ymatches)==1,(f.name,len(qmatches),len(ymatches))
    q,y=qmatches[0],ymatches[0];assert q.group(2)==y.group(1)
    actual=[]
    for table in s.find_all('table'):
        tx=table.get_text(' ',strip=True).lower()
        if not ('gross profit' in tx and 'cost of revenue' in tx):continue
        rs=[]
        for tr in table.find_all('tr'):
            c=[td.get_text(' ',strip=True) for td in tr.find_all(['td','th'],recursive=False)]
            if c and c[0].lower()=='revenue':rs.append(c)
        if not rs:continue
        assert len(rs)==1
        values=[float(v.replace(',','')) for c in rs[0][1:] for v in re.findall(r'\b\d[\d,]*(?:\.\d+)?\b',c)]
        assert values;actual.append(dict(revenue_million=values[0]/1000,source_row=rs[0]))
    assert len(actual)==1,(f.name,len(actual))
    rounded=re.search(r'Revenue was\s*\$([\d,.]+)\s*(million|billion)',text,re.I)
    assert rounded,f.name
    value=float(rounded.group(1).replace(',',''))*(1000 if rounded.group(2).lower()=='billion' else 1)
    precision=len(rounded.group(1).split('.')[1]) if '.' in rounded.group(1) else 0
    tolerance=.5*10**(-precision)*(1000 if rounded.group(2).lower()=='billion' else 1)+1e-6
    assert abs(value-actual[0]['revenue_million'])<=tolerance,(f.name,value,actual[0])
    return dict(symbol='DDOG',source=item['url'],source_file=str(f.relative_to(ROOT)),sha256=hashlib.sha256(f.read_bytes()).hexdigest(),
        title=title,published=str(pub),modified=None if x.get('modified_time_unavailable') else str(mod),available_after=str(max(pub,mod)),
        publication_basis=x.get('publication_basis','JSON-LD and visible timestamp/listing'),
        reported_fiscal_year=year,reported_quarter=quarter,actual_revenue_million=actual[0]['revenue_million'],actual_source_row=actual[0]['source_row'],
        fiscal_year=int(y.group(1)),next_quarter=QS[q.group(1).lower()],quarter_range_million=amounts(q.group(3)),annual_range_million=amounts(y.group(2)),
        guidance_source=[q.group(),y.group()],actual_rounded_value_million=value,rounding_tolerance_million=tolerance)


def main():
    rows=[];errors=[]
    for folder in [ROOT/'DDOG',ROOT/'DDOG_PRN',ROOT/'DDOG_NEWSFILE']:
        for item in json.loads((folder/'downloaded.json').read_text()):
            try:rows.append(parse(item,folder))
            except Exception as e:errors.append(dict(folder=folder.name,file=item['file'],source=item['url'],error=str(e),type=type(e).__name__))
    rows.sort(key=lambda x:x['published']);seen={};dedup=[]
    for row in rows:
        key=(row['reported_fiscal_year'],row['reported_quarter'])
        if key in seen:
            prior=seen[key]
            assert all(row[f]==prior[f] for f in ['actual_revenue_million','quarter_range_million','annual_range_million'])
            prior.setdefault('duplicate_sources',[]).append(row['source'])
        else:seen[key]=row;dedup.append(row)
    old=None;gaps=[]
    for row in dedup:
        row['remaining_year_revision_million']=None;row['previous_source']=None
        if old:
            delta=row['reported_fiscal_year']*4+row['reported_quarter']-(old['reported_fiscal_year']*4+old['reported_quarter'])
            if delta!=1:gaps.append(dict(after=old['title'],before=row['title']))
            if delta==1 and old['fiscal_year']==row['fiscal_year']==row['reported_fiscal_year'] and old['next_quarter']==row['reported_quarter']:
                surprise=row['actual_revenue_million']-sum(old['quarter_range_million'])/2
                row.update(remaining_year_revision_million=sum(row['annual_range_million'])/2-sum(old['annual_range_million'])/2-surprise,
                    realized_surprise_million=surprise,previous_source=old['source'])
        old=row
    (OUT/'parsed_guidance.json').write_text(json.dumps(dedup,indent=2));(OUT/'parse_errors.json').write_text(json.dumps(errors,indent=2))
    audit=dict(parsed=len(rows),unique_quarters=len(dedup),errors=errors,gaps=gaps,comparable_pairs=sum(r['remaining_year_revision_million'] is not None for r in dedup),
        sec_crosscheck='pending',interim_guidance_and_scope_review='pending',market_performance_read=False)
    (OUT/'source_parse_audit.json').write_text(json.dumps(audit,indent=2));print(json.dumps(audit,indent=2))


if __name__=='__main__':main()
