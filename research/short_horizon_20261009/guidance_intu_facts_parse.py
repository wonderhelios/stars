"""Extract exact published fact-sheet ranges, preserving later modification timing.

Run with bundled Python (pypdf). Kept separate from trading inputs pending visual
column/footnote audits for every quarter; never overwrite rounded press proxies.
"""
import hashlib
import json
import re
from datetime import datetime, timezone, timedelta
from pathlib import Path
from pypdf import PdfReader

OUT=Path(__file__).resolve().parent/'guidance_expansion/INTU'


def main():
    downloaded=json.loads((OUT/'facts_downloaded.json').read_text());press=json.loads((OUT/'parsed_guidance.json').read_text())
    rows=[];errors=[]
    for item in downloaded:
        try:
            file=OUT/item['file'];assert hashlib.sha256(file.read_bytes()).hexdigest()==item['sha256']
            reader=PdfReader(file);texts=[p.extract_text() for p in reader.pages]
            (OUT/(item['file']+'.txt')).write_text('\n\f\n'.join(texts))
            p=next(x for x in press if x['source']==item['release_source']);matches=[]
            for n,t in enumerate(texts):
                t=re.sub(r'\s+',' ',t)
                if not ('FINANCIAL SUMMARY' in t.upper() and 'millions' in t):continue
                m=re.search(r'Total Revenue (.+?)(?:% change YOY|GAAP Operating Income)',t,re.I);assert m,item['file']
                pairs=[[float(a.replace(',','')),float(b.replace(',',''))] for a,b in re.findall(r'\$([\d,]+)\s*[-–]\s*\$([\d,]+)',m[1])]
                assert len(pairs) in [0,2],(item['file'],pairs)
                if pairs:
                    fy=p['fiscal_year'];nq=p['next_quarter'];assert fy is not None and nq is not None
                    old=re.search(rf'Q{nq}\s*FY\s*{fy%100}\s+FY\s*{fy%100}',t[:1200])
                    modern=re.search(rf'Q{nq}\s+FY[\x27’]?{fy%100}\s+FY[\x27’]?{fy%100}',t[:1200])
                    revised=item['release_id']=='1320' and n==2 and 'FY27' in t and 'Revised View' in t
                    assert old or modern or revised,t[:500]
                    assert all(abs(x-y)<1e-7 for x,y in zip(pairs[1],p['annual_range_million']))
                    if p.get('quarter_range_basis')=='explicit_original_dollar_range':
                        assert all(abs(x-y)<1e-7 for x,y in zip(pairs[0],p['quarter_range_million']))
                elif p['quarter_range_million'] is not None:
                    # FY2026 Q4 has a historical-only page then a revised guidance view.
                    assert item['release_id'] in ['1266','1320'] and n==1
                    continue
                matches.append(dict(page=n+1,header=t[:500],total_revenue_row=m[0],quarter_range_million=pairs[0] if pairs else None,
                    annual_range_million=pairs[1] if pairs else None))
            assert len(matches)==1,(item['file'],len(matches))
            pub=datetime.fromisoformat(item['release_published']);created=reader.metadata.creation_date;modified=reader.metadata.modification_date
            unknown_zone=any(x is not None and x.tzinfo is None for x in [created,modified])
            bounds=[x if x.tzinfo else x.replace(tzinfo=timezone(timedelta(hours=-12))) for x in [created,modified] if x is not None]
            latest=max([pub]+bounds)
            rows.append(dict(file=item['file'],source=item['url'],sha256=item['sha256'],release_source=item['release_source'],
                release_published=item['release_published'],pdf_created=created.isoformat() if created else None,
                pdf_modified=modified.isoformat() if modified else None,conservative_source_time=latest.isoformat(),
                metadata_timezone_unknown=unknown_zone,unknown_timezone_bound='UTC-12 latest-instant bound, not an assertion of actual timezone' if unknown_zone else None,
                modified_after_release=latest>pub,delay_hours=(latest-pub).total_seconds()/3600,
                publication_limitation='PDF metadata is not a web publication log; preserve later version times and verify original dated cover.',
                trading_ready=False,**matches[0]))
        except Exception as e:errors.append(dict(file=item['file'],error=str(e),type=type(e).__name__))
    summary=dict(downloaded=len(downloaded),parsed=len(rows),issues=errors,late_versions=sum(x['modified_after_release'] for x in rows),
        numerical_guidance=sum(x['quarter_range_million'] is not None for x in rows),market_performance_read=False,
        requires='Visual review of each relevant table, dated cover and footnotes; remaining source/time/scope checks; then explicitly merge exact ranges')
    (OUT/'facts_parsed.json').write_text(json.dumps(rows,indent=2));(OUT/'facts_parse_summary.json').write_text(json.dumps(summary,indent=2))
    print(json.dumps(summary,indent=2))


if __name__=='__main__':main()
