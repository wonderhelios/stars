"""Issuer-specific guidance extraction and financial-source audit, no prices."""
import hashlib
import json
import re
from pathlib import Path
import pandas as pd
from bs4 import BeautifulSoup
from guidance_extract import walk_json

ROOT=Path(__file__).resolve().parent/'guidance_expansion/MDB'
QUARTERS={'first':1,'second':2,'third':3,'fourth':4}


def number_range(text):
    values=[float(n.replace(',','')) for n in re.findall(r'\$\s*([\d,]+(?:\.\d+)?)',text)]
    assert len(values)==2 and values[1]>=values[0], text
    assert not ('million' in text.lower() and 'billion' in text.lower()),text
    if 'billion' in text.lower():values=[v*1000 for v in values]
    else:assert 'million' in text.lower(),text
    return values


def parse(item):
    f=ROOT/item['file'];s=BeautifulSoup(f.read_text(),'html.parser')
    records=[]
    for script in s.find_all('script',type='application/ld+json'):
        try:records.extend(walk_json(json.loads(script.get_text())))
        except json.JSONDecodeError:continue
    records=[r for r in records if r.get('@type') in ['NewsArticle','Article'] and r.get('datePublished')]
    assert len(records)==1
    meta=records[0];title=meta['headline']
    fiscal=int(re.search(r'fiscal(?: year)? (20\d{2})',title,re.I).group(1))
    quarter=QUARTERS[re.search(r'(first|second|third|fourth) quarter',title,re.I).group(1).lower()]
    pub=pd.Timestamp(meta['datePublished']);mod=pd.Timestamp(meta.get('dateModified',meta['datePublished']))
    assert pub.tzinfo is not None and mod.tzinfo is not None
    listed=pd.Timestamp(item['listing_date'].replace(' ET','')).tz_localize('America/New_York')
    assert pub==listed
    guidances=[];actual=[];split_guidance={}
    for table in s.find_all('table'):
        text=table.get_text(' ',strip=True)
        cells=[[c.get_text(' ',strip=True) for c in tr.find_all(['td','th'],recursive=False)] for tr in table.find_all('tr')]
        revenue=[c for c in cells if c and c[0].lower()=='revenue']
        new_revenue=[c for c in cells if c and c[0].lower().startswith('revenues are expected to be in the range')]
        if len(new_revenue)==1:
            rv=new_revenue[0];assert len(rv)==2
            context=' '.join(p.get_text(' ',strip=True).replace('\xa0',' ') for p in table.find_all_previous(['p','h2','h3','h4'],limit=2))
            qmatch=re.search(r'(First|Second|Third|Fourth) Quarter Fiscal(?: Year)? (20\d{2}) Guidance',context,re.I)
            ymatch=re.search(r'Full Year Fiscal(?: Year)? (20\d{2}) Guidance',context,re.I)
            assert bool(qmatch)!=bool(ymatch),(f.name,context)
            key='quarter' if qmatch else 'annual'
            assert key not in split_guidance
            split_guidance[key]=dict(match=(qmatch or ymatch).groups(),range=number_range(rv[1]),source_header=context,revenue_source_row=rv)
        if re.search(r'Full Year Fiscal(?: Year)? 20\d{2}',text,re.I) and len(revenue)==1 and len(text)<3000:
            rv=revenue[0];assert len(rv)==3,(f.name,rv)
            headers=[c for c in cells if any(re.search('Quarter Fiscal',v,re.I) for v in c)]
            assert len(headers)==1 and len(headers[0])==3
            qh,yh=headers[0][1:]
            qnum=QUARTERS[re.search(r'(First|Second|Third|Fourth) Quarter',qh,re.I).group(1).lower()]
            qyear=int(re.search(r'20\d{2}',qh).group());year=int(re.search(r'20\d{2}',yh).group())
            assert qyear==year
            eps=[c for c in cells if c and 'per share' in c[0].lower()]
            guidances.append(dict(fiscal_year=year,next_quarter=qnum,quarter_range_million=number_range(rv[1]),
                annual_range_million=number_range(rv[2]),source_header=headers[0],revenue_source_row=rv,eps_source_row=eps))
        # GAAP statement table in thousands, first amount is the current quarter.
        if 'CONSOLIDATED STATEMENTS' in text.upper() and 'thousands' in text.lower():
            rs=[c for c in cells if c and c[0].strip().lower()=='total revenue']
            if len(rs)==1:
                nums=[float(n.replace(',','')) for c in rs[0][1:] for n in re.findall(r'\b\d[\d,]*(?:\.\d+)?\b',c)]
                assert nums
                actual.append(dict(million=nums[0]/1000,source_row=rs[0]))
    if split_guidance:
        assert not guidances and set(split_guidance)=={'quarter','annual'}
        q,y=split_guidance['quarter'],split_guidance['annual']
        assert int(q['match'][1])==int(y['match'][0])
        guidances.append(dict(fiscal_year=int(y['match'][0]),next_quarter=QUARTERS[q['match'][0].lower()],
            quarter_range_million=q['range'],annual_range_million=y['range'],
            source_header=[q['source_header'],y['source_header']],revenue_source_row=[q['revenue_source_row'],y['revenue_source_row']],eps_source_row=None))
    assert len(guidances)==1 and len(actual)==1,(f.name,len(guidances),len(actual))
    text=s.get_text(' ',strip=True)
    rounded=re.search(r'Total revenue (?:for the .*?quarter(?: (?:of )?fiscal(?: year)? 20\d{2})? )?was\s*\$([\d,.]+)\s*million',text,re.I)
    assert rounded, f.name
    rounded_number=float(rounded.group(1).replace(',',''))
    assert abs(rounded_number-actual[0]['million']) <= .051,(f.name,rounded_number,actual[0])
    return dict(symbol='MDB',source=item['url'],source_file=item['file'],sha256=hashlib.sha256(f.read_bytes()).hexdigest(),
        published=str(pub),modified=str(mod),available_after=str(max(pub,mod)),title=title,
        reported_fiscal_year=fiscal,reported_quarter=quarter,actual_revenue_million=actual[0]['million'],
        actual_source_row=actual[0]['source_row'],actual_rounded_million=rounded_number,**guidances[0])


def main():
    downloads=json.loads((ROOT/'downloaded.json').read_text())
    rows=[];errors=[]
    for item in downloads:
        try: rows.append(parse(item))
        except Exception as e:errors.append(dict(file=item['file'],source=item['url'],error=str(e),error_type=type(e).__name__))
    rows.sort(key=lambda r:r['published'])
    old=None
    for row in rows:
        row['remaining_year_revision_million']=None;row['previous_source']=None
        if old and old['reported_fiscal_year']*4+old['reported_quarter']+1==row['reported_fiscal_year']*4+row['reported_quarter'] and old['fiscal_year']==row['fiscal_year']==row['reported_fiscal_year'] and old['next_quarter']==row['reported_quarter']:
            surprise=row['actual_revenue_million']-sum(old['quarter_range_million'])/2
            change=sum(row['annual_range_million'])/2-sum(old['annual_range_million'])/2
            row.update(remaining_year_revision_million=change-surprise,realized_surprise_million=surprise,previous_source=old['source'])
        old=row
    (ROOT/'parsed_guidance.json').write_text(json.dumps(rows,indent=2))
    (ROOT/'parse_errors.json').write_text(json.dumps(errors,indent=2))
    sequence=[r['reported_fiscal_year']*4+r['reported_quarter'] for r in rows]
    gaps=[dict(after=rows[i-1]['title'],before=rows[i]['title']) for i in range(1,len(rows)) if sequence[i]-sequence[i-1]!=1]
    audit=dict(releases=len(downloads),parsed=len(rows),errors=len(errors),quarter_gaps=gaps,
        actual_vs_rounded_max_difference=max(abs(r['actual_revenue_million']-r['actual_rounded_million']) for r in rows) if rows else None,
        modification_lag_max_seconds=max((pd.Timestamp(r['modified'])-pd.Timestamp(r['published'])).total_seconds() for r in rows) if rows else None,
        comparable_pairs=sum(r['remaining_year_revision_million'] is not None for r in rows),
        independent_official_directory_status='normal request HTTP403; available earlier search snapshot not a complete archived directory',
        accounting_scope_and_interim_guidance_review='pending',market_performance_read=False)
    (ROOT/'source_parse_audit.json').write_text(json.dumps(audit,indent=2))
    print(json.dumps(audit,indent=2));print(json.dumps(errors,indent=2))


if __name__=='__main__':main()
