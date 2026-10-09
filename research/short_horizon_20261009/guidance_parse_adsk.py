"""Autodesk source-specific extraction. Preserve preliminary vs final releases."""
import hashlib
import json
import re
from pathlib import Path
import pandas as pd
from bs4 import BeautifulSoup
from guidance_extract import walk_json

ROOT=Path(__file__).resolve().parent/'guidance_expansion/ADSK'
QUARTERS={'first':1,'second':2,'third':3,'fourth':4}


def parse(item):
    f=ROOT/item['file'];s=BeautifulSoup(f.read_text(),'html.parser');articles=[]
    for script in s.find_all('script',type='application/ld+json'):
        try: articles.extend(walk_json(json.loads(script.get_text())))
        except json.JSONDecodeError:continue
    articles=[a for a in articles if a.get('@type') in ['NewsArticle','Article'] and a.get('datePublished')]
    assert len(articles)==1
    a=articles[0];title=a['headline'];preliminary='preliminary' in title.lower()
    pub=pd.Timestamp(a['datePublished']);mod=pd.Timestamp(a.get('dateModified',a['datePublished']))
    assert pub==pd.Timestamp(item['listing_date'].replace(' ET','')).tz_localize('America/New_York')
    forecast={};actual=[];prelim=[];scope=[]
    for table in s.find_all('table'):
        t=table.get_text(' ',strip=True)
        cells=[[c.get_text(' ',strip=True) for c in tr.find_all(['td','th'],recursive=False)] for tr in table.find_all('tr')]
        cells=[[c for c in row if c] for row in cells]
        rs=[c for c in cells if c and re.match(r'Revenue\s*\(in millions\)',c[0],re.I)]
        if rs and len(t)<5000:
            assert len(rs)==1 and len(rs[0])==2
            hs=[c for c in cells if c and 'Guidance Metrics' in c[0]]
            assert len(hs)==1 and len(hs[0])==2
            header=hs[0][1]
            year=2000+int(re.search(r'FY(\d{2})',header).group(1))
            q=re.search(r'Q(\d)',header)
            key='quarter' if q else 'annual';assert key not in forecast
            values=[float(v.replace(',','')) for v in re.findall(r'\$\s*([\d,]+(?:\.\d+)?)',rs[0][1])]
            assert len(values)==2 and values[1]>=values[0]
            end=re.search(r'ending (\w+ \d{1,2}, 20\d{2})',header).group(1)
            forecast[key]=dict(fiscal_year=year,quarter=int(q.group(1)) if q else None,
                ending=str(pd.Timestamp(end).date()),range_million=values,header=header,source_row=rs[0])
        if 'STATEMENTS OF OPERATIONS' in t.upper() and 'In millions' in t:
            rs=[c for c in cells if c and c[0].lower()=='total net revenue']
            assert len(rs)==1
            numbers=[float(v.replace(',','')) for c in rs[0][1:] for v in re.findall(r'\b\d[\d,]*(?:\.\d+)?\b',c)]
            assert numbers
            actual.append(dict(revenue_million=numbers[0],source_row=rs[0]))
        if preliminary and re.search(r'Q\d FY\d{2}',t) and 'Guidance Metrics' not in t and len(t)<1000:
            h=re.search(r'Q(\d) FY(\d{2}) \(ending (\w+ \d{1,2}, 20\d{2})\)',t)
            if h:
                rs=[c for c in cells if c and c[0]=='Revenue'];assert len(rs)==1
                prelim.append(dict(reported_fiscal_year=2000+int(h.group(2)),reported_quarter=int(h.group(1)),
                    actual_revenue_million=None,approximate_actual_text=rs[0][1],reported_quarter_end=str(pd.Timestamp(h.group(3)).date())))
    assert set(forecast)=={'quarter','annual'} and forecast['quarter']['fiscal_year']==forecast['annual']['fiscal_year']
    if preliminary:
        assert not actual and len(prelim)==1
        reported=prelim[0];actual_row=None
    else:
        assert len(actual)==1
        year=int(re.search(r'Fiscal (20\d{2})',title,re.I).group(1));quarter=QUARTERS[re.search(r'(first|second|third|fourth) quarter',title,re.I).group(1).lower()]
        end=pd.Timestamp(f'{year-1}-02-01')+pd.DateOffset(months=3*quarter)-pd.Timedelta(days=1)
        reported=dict(reported_fiscal_year=year,reported_quarter=quarter,reported_quarter_end=str(end.date()),actual_revenue_million=actual[0]['revenue_million'])
        actual_row=actual[0]['source_row']
    for p in s.find_all(['p','li']):
        t=p.get_text(' ',strip=True)
        if len(t)<1800 and re.search(r'new (?:transaction|buying) model|acquisition|accounting investigation|restat',t,re.I):scope.append(t)
    return dict(symbol='ADSK',source=item['url'],source_file=item['file'],sha256=hashlib.sha256(f.read_bytes()).hexdigest(),
        published=str(pub),modified=str(mod),available_after=str(max(pub,mod)),title=title,preliminary=preliminary,
        actual_source_row=actual_row,scope_review_excerpts=list(dict.fromkeys(scope)),**reported,
        fiscal_year=forecast['annual']['fiscal_year'],next_quarter=forecast['quarter']['quarter'],
        next_quarter_end=forecast['quarter']['ending'],quarter_range_million=forecast['quarter']['range_million'],
        annual_range_million=forecast['annual']['range_million'],guidance_source=forecast)


def main():
    items=json.loads((ROOT/'downloaded.json').read_text());rows=[];errors=[]
    for item in items:
        try:rows.append(parse(item))
        except Exception as e:errors.append(dict(file=item['file'],error=str(e),type=type(e).__name__))
    rows.sort(key=lambda r:r['published']);old=None
    for row in rows:
        row['remaining_year_revision_million']=None;row['previous_source']=old['source'] if old else None
        row['comparison_status']='initial_or_different_year'
        if row['preliminary']:
            row['comparison_status']='preliminary_actual_only_approximate_no_signal'
        elif old and old['reported_fiscal_year']*4+old['reported_quarter']==row['reported_fiscal_year']*4+row['reported_quarter']:
            row['comparison_status']='repeated_reported_quarter_no_signal'
        elif old and old['reported_fiscal_year']*4+old['reported_quarter']+1==row['reported_fiscal_year']*4+row['reported_quarter'] and old['fiscal_year']==row['fiscal_year']==row['reported_fiscal_year'] and old['next_quarter']==row['reported_quarter']:
            assert old['next_quarter_end']==row['reported_quarter_end']
            surprise=row['actual_revenue_million']-sum(old['quarter_range_million'])/2
            change=sum(row['annual_range_million'])/2-sum(old['annual_range_million'])/2
            row.update(remaining_year_revision_million=change-surprise,realized_surprise_million=surprise,comparison_status='numerically_comparable_scope_review_pending')
        old=row
    (ROOT/'parsed_guidance.json').write_text(json.dumps(rows,indent=2))
    (ROOT/'parse_errors.json').write_text(json.dumps(errors,indent=2))
    seq=[r['reported_fiscal_year']*4+r['reported_quarter'] for r in rows if not r['preliminary']]
    audit=dict(releases=len(items),parsed=len(rows),errors=errors,preliminary_releases=sum(r['preliminary'] for r in rows),
        final_quarters_consecutive=all(seq[i]==seq[i-1]+1 for i in range(1,len(seq))),
        computed_comparisons=sum(r['remaining_year_revision_million'] is not None for r in rows),
        source_scope_review='pending',sec_value_crosscheck='pending',market_performance_read=False)
    (ROOT/'source_parse_audit.json').write_text(json.dumps(audit,indent=2));print(json.dumps(audit,indent=2))


if __name__=='__main__':main()
