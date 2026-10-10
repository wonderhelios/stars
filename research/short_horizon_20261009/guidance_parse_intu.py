"""Intuit original revenue, fiscal guidance, growth-rate units and interim gates."""
import hashlib
import json
import re
import pandas as pd
from bs4 import BeautifulSoup
from screen import ROOT

OUT=ROOT/'guidance_expansion/INTU'
QS={'first':1,'second':2,'third':3,'fourth':4}
QPAT=r'(first|second|third|fourth)[ -]quarter(?: of)? fiscal(?: year)? (20\d{2})'
YPAT=r'(?:full fiscal year|full year fiscal) (20\d{2})'


def norm(t):return re.sub(r'\s+',' ',t.replace('\xa0',' ')).strip()


def money(t):
    vals=[float(v.replace(',','')) for v in re.findall(r'\$\s*([\d,]+(?:\.\d+)?)',t)]
    assert len(vals)==2 and vals[0]<=vals[1],t
    scale=1000 if 'billion' in t.lower() else 1
    return [v*scale for v in vals]


def parse(item,quarterly):
    p=OUT/item['file'];s=BeautifulSoup(p.read_text(),'html.parser');body=s.select_one('.main-content');assert body
    t=norm(body.get_text(' ',strip=True));stamp=body.find('time',datetime=True);assert stamp
    pub=pd.Timestamp(stamp['datetime']).tz_localize('America/New_York')
    assert pub==pd.Timestamp(item['listing_time']).tz_localize('America/New_York')
    assert pub.tzname() in stamp.get_text() and pub.tzname() in item['listing_evidence']
    row=dict(symbol='INTU',source=item['url'],source_file='INTU/'+item['file'],sha256=hashlib.sha256(p.read_bytes()).hexdigest(),
        title=item['title'],published=str(pub),modified=None,available_after=str(pub),
        publication_basis='Issuer article visible time and separately listed archive time agree; no modification metadata available')
    if item['url'] not in quarterly:
        row.update(kind='interim_notice',source_text=t.split('About Intuit')[0])
        return row
    m=re.search(r'financial results for the (first|second|third|fourth) quarter (?:and full |of )?fiscal(?: year)? (20\d{2})',t,re.I)
    assert m,'No explicit reported fiscal quarter/year'
    q=QS[m[1].lower()];fy=int(m[2]);actual=[]
    for table in body.find_all('table'):
        tt=norm(table.get_text(' ',strip=True))
        prev=table.find_previous(['p','table'])
        heading=tt if 'GAAP CONSOLIDATED STATEMENTS OF OPERATIONS' in tt else (norm(prev.get_text(' ',strip=True)) if prev else '')
        if not ('GAAP CONSOLIDATED STATEMENTS OF OPERATIONS' in heading and 'Three Months Ended' in tt and 'In millions' in heading):continue
        for tr in table.find_all('tr'):
            cells=[norm(c.get_text(' ',strip=True)) for c in tr.find_all(['th','td'],recursive=False)]
            if cells and cells[0].lower()=='total net revenue':
                vals=[float(v.replace(',','')) for c in cells[1:] for v in re.findall(r'\b\d[\d,]*(?:\.\d+)?\b',c)]
                actual.append((vals[0],cells,(heading[:180]+' '+tt[:350])))
    assert len(actual)==1,(item['file'],len(actual))
    row.update(kind='quarterly',reported_quarter=q,reported_fiscal_year=fy,actual_revenue_million=actual[0][0],
        actual_source_row=actual[0][1],actual_table_header=actual[0][2],fiscal_year=None,next_quarter=None,
        annual_range_million=None,quarter_range_million=None,quarter_growth_percent=None,guidance_source=[])
    begin=re.search(r'Forward.looking Guidance',t,re.I)
    if not begin:
        row['guidance_issue']='No standard forward-guidance section; retain actual only';return row
    section=t[begin.end():].split('Conference Call Details')[0]
    # Format changed in FY2026 Q4: separately labelled annual and quarterly tables.
    if item['file']=='release_1320.html':
        for label,key in [('Full Year Fiscal 2027 Guidance','annual_range_million'),('First Quarter Fiscal 2027 Guidance','quarter_range_million')]:
            part=section.split(label,1)[1];match=re.search(r'Total Revenue (\$[\d,]+ to \$[\d,]+)',part);assert match
            row[key]=money(match[1]);row['guidance_source'].extend([label,match[0]])
        row.update(fiscal_year=2027,next_quarter=1);return row
    contexts=[(m.start(),'annual',int(m[1]),None) for m in re.finditer(YPAT,section,re.I)]
    contexts += [(m.start(),'quarter',int(m[2]),QS[m[1].lower()]) for m in re.finditer(QPAT,section,re.I)]
    contexts.sort();found={}
    for li in body.find_all('li'):
        bullet=norm(li.get_text(' ',strip=True))
        if not re.match(r'^Revenue\b',bullet,re.I):continue
        pos=section.find(bullet)
        if pos<0:continue
        known=[c for c in contexts if c[0]<pos]
        assert known,bullet
        _,kind,year,nq=known[-1]
        assert kind not in found,(item['file'],kind,bullet)
        rec=dict(year=year,quarter=nq,text=bullet)
        if re.match(r'Revenue (?:of |between )?\$',bullet,re.I):rec['range_million']=money(bullet)
        else:
            growth=re.search(r'Revenue (?:growth of|to grow|to decline|decline of)(?: approximately)? (\d+(?:\.\d+)?) to (\d+(?:\.\d+)?) percent',bullet,re.I)
            if not growth:
                growth=re.search(r'Revenue (?:growth of|to grow|to decline|decline of)(?: approximately)? (\d+(?:\.\d+)?) percent',bullet,re.I)
            assert growth,bullet
            vals=[float(growth[1]),float(growth[2]) if growth.lastindex==2 else float(growth[1])]
            if 'decline' in growth[0].lower():vals=sorted(-v for v in vals)
            rec['growth_percent']=vals
        found[kind]=rec;row['guidance_source'].append(bullet)
    if 'annual' in found:
        a=found['annual'];row['fiscal_year']=a['year'];row['annual_range_million']=a.get('range_million')
        assert row['annual_range_million'] is not None
    if 'quarter' in found:
        a=found['quarter'];row['next_quarter']=a['quarter'];row['quarter_guidance_year']=a['year']
        row['quarter_range_million']=a.get('range_million');row['quarter_growth_percent']=a.get('growth_percent')
        if row['fiscal_year'] is not None:assert row['fiscal_year']==a['year']
    if not found:row['guidance_issue']='No numerical quarterly/annual revenue guidance parsed; no invented forecast'
    return row


def main():
    items=json.loads((OUT/'downloaded.json').read_text());quarterly={x['href'] for x in json.loads((OUT/'quarterly_directory_links.json').read_text())}
    rows=[];interim=[];issues=[]
    for item in items:
        try:
            r=parse(item,quarterly);(rows if r['kind']=='quarterly' else interim).append(r)
        except Exception as e:issues.append(dict(file=item['file'],source=item['url'],issue=str(e),type=type(e).__name__))
    rows.sort(key=lambda r:pd.Timestamp(r['published']));byperiod={(r['reported_fiscal_year'],r['reported_quarter']):r for r in rows}
    assert len(byperiod)==len(rows)
    conversions=[]
    for r in rows:
        if r['quarter_growth_percent'] is None:continue
        r['quarter_range_basis']='proxy_from_rounded_approximate_growth_not_exact_dollar_forecast'
        r['quarter_forecast_verified_for_trading']=False
        base=byperiod.get((r['quarter_guidance_year']-1,r['next_quarter']))
        if base is None:
            r['quarter_conversion_issue']='Prior-year target-quarter original revenue not yet in saved coverage';continue
        assert pd.Timestamp(base['published'])<pd.Timestamp(r['published'])
        r['quarter_range_million']=[base['actual_revenue_million']*(1+g/100) for g in r['quarter_growth_percent']]
        r['quarter_conversion_basis']=dict(source=base['source'],published=base['published'],revenue_million=base['actual_revenue_million'])
        conversions.append(dict(source=r['source'],base=r['quarter_conversion_basis'],range_million=r['quarter_range_million']))
    for r in rows:
        if r['quarter_growth_percent'] is None and r['quarter_range_million'] is not None:
            r['quarter_range_basis']='explicit_original_dollar_range'
            r['quarter_forecast_verified_for_trading']=True
    (OUT/'parsed_guidance.json').write_text(json.dumps(rows,indent=2));(OUT/'interim_notices.json').write_text(json.dumps(interim,indent=2))
    (OUT/'parse_issues.json').write_text(json.dumps(issues,indent=2));(OUT/'growth_conversions.json').write_text(json.dumps(conversions,indent=2))
    periods=sorted(fy*4+q-1 for fy,q in byperiod)
    missing=[dict(fiscal_year=x//4,quarter=x%4+1) for x in range(min(periods),max(periods)+1) if x not in periods]
    summary=dict(downloaded=len(items),quarterly=len(rows),interim=len(interim),issues=issues,missing_quarters=missing,
        growth_conversions=len(conversions),modified_timestamp_unavailable=len(items),market_performance_read=False,
        preliminary_status='Quarterly values only; interim/scope gates and independent SEC checks required before returns')
    (OUT/'parse_summary.json').write_text(json.dumps(summary,indent=2));print(json.dumps(summary,indent=2))


if __name__=='__main__':main()
