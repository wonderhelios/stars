"""Synopsys original targets, actual revenue and explicit fiscal-week footnotes."""
import hashlib
import json
import re
from pathlib import Path
import pandas as pd
from bs4 import BeautifulSoup
from guidance_extract import walk_json

ROOT=Path(__file__).resolve().parent/'guidance_expansion/SNPS'
QS={'first':1,'second':2,'third':3,'fourth':4}
DATE=r'[A-Z][a-z]+ \d{1,2}, 20\d{2}'


def parse(item):
    f=ROOT/item['file'];s=BeautifulSoup(f.read_text(),'html.parser');records=[]
    for script in s.find_all('script',type='application/ld+json'):
        try:records.extend(walk_json(json.loads(script.get_text())))
        except json.JSONDecodeError:continue
    records=[x for x in records if x.get('@type') in ['NewsArticle','Article'] and x.get('datePublished')]
    assert len(records)==1
    x=records[0];title=x['headline'];pub=pd.Timestamp(x['datePublished']);mod=pd.Timestamp(x.get('dateModified',x['datePublished']))
    assert pub==pd.Timestamp(item['listing_date'].replace(' ET','')).tz_localize('America/New_York')
    year=int(re.search(r'Fiscal Year (20\d{2})',title,re.I).group(1))
    qword=re.search(r'(First|Second|Third|Fourth) Quarter',title,re.I).group(1).lower();q=QS[qword]
    text=s.get_text(' ',strip=True).replace('\xa0',' ')
    footnote=re.search(rf"Synopsys['’] {qword} quarter of fiscal year {year}(?: and 20\d{{2}})? ended on ({DATE})[^.]*\.",text,re.I)
    nominal=pd.Timestamp(f'{year-1}-11-01')+pd.DateOffset(months=q*3)-pd.Timedelta(days=1)
    if footnote:end=pd.Timestamp(footnote.group(1));period_basis=footnote.group()
    else:
        assert year>=2026, (f.name,'missing actual fiscal calendar footnote')
        end=nominal;period_basis='Calendar fiscal quarters after FY2025 change; independently verify against original SEC period.'
    presentation_basis=re.search(r'For presentation purposes, we refer to the closest calendar month end\.',text,re.I)
    assert end==nominal or presentation_basis,(f.name,'no evidence for nominal/actual endpoint mapping')
    guidance=[];actual=[];scope=[]
    for table in s.find_all('table'):
        t=table.get_text(' ',strip=True).replace('\xa0',' ')
        cells=[[td.get_text(' ',strip=True).replace('\xa0',' ') for td in tr.find_all(['td','th'],recursive=False)] for tr in table.find_all('tr')]
        if 'Financial Targets' in t and 'in millions' in t.lower() and len(t)<8000:
            rs=[c for c in cells if c and re.fullmatch(r'Revenue(?: \(\d\))?',c[0])]
            assert len(rs)==1,(f.name,rs)
            values=[float(v.replace(',','')) for c in rs[0][1:] for v in re.findall(r'\$\s*([\d,]+(?:\.\d+)?)',c)]
            assert len(values)==4 and values[1]>=values[0] and values[3]>=values[2],(f.name,rs)
            target_year=year+(q==4);assert str(target_year) in t
            guidance.append(dict(fiscal_year=target_year,next_quarter=q%4+1,quarter_range_million=values[:2],annual_range_million=values[2:],guidance_source_row=rs[0],guidance_context=t))
        if re.search(r'Statements of (?:Income|Operations)',t,re.I) and 'in thousands' in t.lower():
            rs=[c for c in cells if c and c[0].lower()=='total revenue']
            if not rs:continue
            assert len(rs)==1
            values=[float(v.replace(',','')) for c in rs[0][1:] for v in re.findall(r'\b\d[\d,]*(?:\.\d+)?\b',c)]
            actual.append(dict(actual_revenue_million=values[0]/1000,actual_source_row=rs[0]))
    assert len(guidance)==1 and len(actual)==1,(f.name,len(guidance),len(actual))
    # Capture explicit accounting/scope statements, not navigation keywords.
    for p in s.find_all(['p','li','td']):
        t=p.get_text(' ',strip=True).replace('\xa0',' ')
        if 60<len(t)<2000 and re.search(r'guidance|targets|continuing operations|fiscal year',t,re.I) and re.search(r'Ansys|Software Integrity|divest|53.week|changed.*fiscal|fiscal.*chang',t,re.I):scope.append(t)
    return dict(symbol='SNPS',source=item['url'],source_file=item['file'],sha256=hashlib.sha256(f.read_bytes()).hexdigest(),
        title=title,published=str(pub),modified=str(mod),available_after=str(max(pub,mod)),reported_fiscal_year=year,reported_quarter=q,
        reported_quarter_end=str(end.date()),presented_calendar_end=str(nominal.date()),period_basis=period_basis,
        calendar_presentation_basis=presentation_basis.group() if presentation_basis else None,
        scope_review_excerpts=list(dict.fromkeys(scope)),**guidance[0],**actual[0])


def main():
    items=json.loads((ROOT/'downloaded.json').read_text());rows=[];errors=[]
    for item in items:
        try:rows.append(parse(item))
        except Exception as e:errors.append(dict(file=item['file'],error=str(e),type=type(e).__name__))
    rows.sort(key=lambda r:r['published']);old=None
    for row in rows:
        row['remaining_year_revision_million']=None;row['previous_source']=None
        if old and row['reported_fiscal_year']*4+row['reported_quarter']==old['reported_fiscal_year']*4+old['reported_quarter']+1 and old['fiscal_year']==row['fiscal_year']==row['reported_fiscal_year']:
            assert old['next_quarter']==row['reported_quarter']
            surprise=row['actual_revenue_million']-sum(old['quarter_range_million'])/2
            row.update(remaining_year_revision_million=sum(row['annual_range_million'])/2-sum(old['annual_range_million'])/2-surprise,
                realized_surprise_million=surprise,previous_source=old['source'])
        old=row
    (ROOT/'parsed_guidance.json').write_text(json.dumps(rows,indent=2));(ROOT/'parse_errors.json').write_text(json.dumps(errors,indent=2))
    seq=[r['reported_fiscal_year']*4+r['reported_quarter'] for r in rows]
    audit=dict(releases=len(items),parsed=len(rows),errors=errors,consecutive=all(seq[i]==seq[i-1]+1 for i in range(1,len(seq))),
        non_calendar_period_ends=sum(r['reported_quarter_end']!=r['presented_calendar_end'] for r in rows),
        scope_and_period_change_review='pending; numeric revisions are not eligible signals yet',sec_crosscheck='pending',market_performance_read=False)
    (ROOT/'source_parse_audit.json').write_text(json.dumps(audit,indent=2));print(json.dumps(audit,indent=2))


if __name__=='__main__':main()
