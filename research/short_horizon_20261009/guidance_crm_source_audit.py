"""Audit Salesforce's public newsroom archive without accessing returns."""
import hashlib
import html
import json
import re
from pathlib import Path
import pandas as pd
from bs4 import BeautifulSoup

ROOT=Path(__file__).resolve().parent/'guidance_expansion/CRM'
QS={'first':1,'second':2,'third':3,'fourth':4}


def main():
    source=ROOT/'api_results_01.json'
    all_rows=json.loads(source.read_text())
    rows=json.loads((ROOT/'api_financial_candidates.json').read_text())
    assert len({r['id'] for r in rows})==len(rows)
    audit=[]
    for row in sorted(rows,key=lambda r:r['date_gmt']):
        title=html.unescape(row['title']['rendered'])
        match=re.search(r'(First|Second|Third|Fourth) Quarter(?: and (?:Full )?(?:Fiscal )?(?:Year )?)?\s*Fiscal(?: Year)? (20\d{2})',title,re.I)
        # Some titles say "Fourth Quarter and Fiscal Year 2025".
        if not match:match=re.search(r'(First|Second|Third|Fourth) Quarter and (?:Full )?Fiscal Year (20\d{2})',title,re.I)
        assert match,title
        q=QS[match[1].lower()];fy=int(match[2])
        s=BeautifulSoup(row['content']['rendered'],'html.parser')
        text=s.get_text(' ',strip=True).replace('\xa0',' ')
        pub=pd.Timestamp(row['date_gmt'],tz='UTC');mod=pd.Timestamp(row['modified_gmt'],tz='UTC')
        local=pd.Timestamp(row['date']).tz_localize('America/Los_Angeles')
        assert local==pub,(row['id'],pub,local)
        notes=[p.get_text(' ',strip=True) for p in s.find_all(['p','li'])
            if re.search(r"Editor[’']s [Nn]ote",p.get_text())]
        images=list(dict.fromkeys(im['src'] for im in s.find_all('img',src=True)
            if '/news/wp-content/' in im['src']))
        # Preserve every explicit revenue-guidance paragraph for subsequent
        # financial extraction. Nothing from a later SEC fact is substituted.
        guides=[p.get_text(' ',strip=True) for p in s.find_all(['p','li'])
            if re.search('revenue',p.get_text(),re.I) and re.search('guidance',p.get_text(),re.I)
            and len(p.get_text())<1800]
        audit.append(dict(id=row['id'],title=title,source=row['link'],reported_fiscal_year=fy,reported_quarter=q,
            published_utc=str(pub),modified_utc=str(mod),available_after_utc=str(max(pub,mod)),
            modified_delay_hours=(mod-pub).total_seconds()/3600,
            body_sha256=hashlib.sha256(row['content']['rendered'].encode()).hexdigest(),
            editor_notes=notes,financial_image_sources=images,table_count=len(s.find_all('table')),
            guidance_paragraphs=list(dict.fromkeys(guides)),
            eligibility='pending exact quarterly actuals, image-table extraction, source-version and acquisition-scope verification'))
    seq=[r['reported_fiscal_year']*4+r['reported_quarter'] for r in audit]
    assert all(seq[i]==seq[i-1]+1 for i in range(1,len(seq)))
    result=dict(api_records=len(all_rows),financial_releases=len(audit),consecutive_quarters=True,
        first=audit[0]['title'],last=audit[-1]['title'],modified_more_than_24h=sum(r['modified_delay_hours']>24 for r in audit),
        releases_with_html_tables=sum(r['table_count']>0 for r in audit),
        source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
        market_performance_read=False,
        limitation='Archive continuity is not proof of original text versions or complete interim guidance. Rounded headline revenue must not be treated as exact realized quarterly revenue.')
    (ROOT/'api_source_audit.json').write_text(json.dumps(result,indent=2))
    (ROOT/'api_release_audit.json').write_text(json.dumps(audit,indent=2))
    print(json.dumps(result,indent=2))


if __name__=='__main__':main()
