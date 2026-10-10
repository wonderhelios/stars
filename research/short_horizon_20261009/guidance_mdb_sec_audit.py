"""Cross-check original news revenue against separately cached SEC filings.

Used as later verification of the numbers, never as announcement-time input.
"""
import hashlib
import json
import sys
from pathlib import Path
import pandas as pd

ROOT=Path(__file__).resolve().parent
OUT=ROOT/'guidance_expansion/MDB'


def main(symbol='MDB'):
    assert symbol in ['MDB','ADSK','DDOG','INTU']
    out=ROOT/'guidance_expansion'/symbol
    file=ROOT/'fundamentals'/f'{symbol}.json'
    cik,entity={'MDB':(1441816,'MONGODB'),'ADSK':(769397,'AUTODESK'),'DDOG':(1561550,'DATADOG'),'INTU':(896878,'INTUIT')}[symbol]
    doc=json.loads(file.read_text());assert doc['cik']==cik and entity in doc['entityName'].upper()
    facts=doc['facts']['us-gaap']['RevenueFromContractWithCustomerExcludingAssessedTax']['units']['USD']
    if symbol=='INTU':
        # Original FY2020 10-K uses Revenues; its original Q3 uses the ASC606 tag.
        # Both describe the total net revenue explicitly verified in the release.
        facts=[dict(v,xbrl_tag=tag) for tag in ['RevenueFromContractWithCustomerExcludingAssessedTax','Revenues']
            for v in doc['facts']['us-gaap'][tag]['units']['USD']]
    allrows=json.loads((out/'parsed_guidance.json').read_text())
    rows=[r for r in allrows if r.get('actual_revenue_million') is not None]
    verified=[];issues=[]
    for row in rows:
        fy=row['reported_fiscal_year'];q=row['reported_quarter'];pub=pd.Timestamp(row['published']).tz_localize(None).normalize()
        fiscal_start=pd.Timestamp(f'{fy}-01-01') if symbol=='DDOG' else pd.Timestamp(f'{fy-1}-08-01' if symbol=='INTU' else f'{fy-1}-02-01')
        end=(fiscal_start+pd.DateOffset(months=3*q))-pd.Timedelta(days=1)
        start=fiscal_start+pd.DateOffset(months=3*(q-1))
        expected=round(row['actual_revenue_million']*1e6)
        # Verify contemporaneous filings, not later annual restatements.
        direct=[v for v in facts if v.get('start')==str(start.date()) and v.get('end')==str(end.date())
            and abs(v['val']-expected)<1 and pub<=pd.Timestamp(v['filed'])<=pub+pd.Timedelta(days=15)]
        check=dict(source=row['source'],quarter_end=str(end.date()),reported_revenue_dollars=expected)
        if symbol=='ADSK' and str(end.date())=='2024-04-30' and not direct:
            direct=[v for v in facts if v.get('accn')=='0000769397-24-000091' and v.get('start')==str(start.date())
                and v.get('end')==str(end.date()) and v.get('filed')=='2024-06-10' and abs(v['val']-expected)<1]
            check['timing_exception']='10-Q filed 2024-06-10, before the 2024-06-11 final news release. Final release is a repeated-quarter event and generates no new signal.'
        if direct:
            chosen=min(direct,key=lambda v:v['filed'])
            check.update(method='reported_quarter',filings=[chosen],error_dollars=0)
        elif q==4:
            year=[v for v in facts if v.get('start')==str(fiscal_start.date()) and v.get('end')==str(end.date())
                and v.get('fy')==fy and v.get('fp')=='FY'
                and pub<=pd.Timestamp(v['filed'])<=pub+pd.Timedelta(days=30)]
            if symbol=='ADSK' and str(end.date())=='2024-01-31' and not year:
                year=[v for v in facts if v.get('accn')=='0000769397-24-000090' and v.get('start')==str(fiscal_start.date())
                    and v.get('end')==str(end.date()) and v.get('filed')=='2024-06-10']
                check['timing_exception']='Delayed original FY2024 annual filing on 2024-06-10 after audit-committee review; used only to check unchanged original quarterly amount, never backdated as a source.'
            nine_end=fiscal_start+pd.DateOffset(months=9)-pd.Timedelta(days=1)
            nine=[v for v in facts if v.get('start')==str(fiscal_start.date()) and v.get('end')==str(nine_end.date())
                and pd.Timestamp(v['filed'])<=pub]
            pairs=[(y,n) for y in year for n in nine if abs(y['val']-n['val']-expected)<1]
            if not pairs:
                issues.append(dict(**check,issue='annual minus contemporaneous nine-month amount did not match'));continue
            y,n=min(pairs,key=lambda p:(p[0]['filed'],p[1]['filed']))
            check.update(method='annual_minus_original_nine_month',filings=[y,n],error_dollars=abs(y['val']-n['val']-expected))
        else:
            issues.append(dict(**check,issue='no matching contemporaneous quarter filing'));continue
        verified.append(check)
    result=dict(symbol=symbol,verified=len(verified),expected=len(rows),issues=issues,preliminary_no_exact_value=len(allrows)-len(rows),
        audit_timing_note='Quarter checks use filings within 15 days of the release; annual checks use the same FY/FP=FY first annual filing within 30 days. MDB FY2025 annual filing was 16 days later. This is later verification only, not signal availability.',
        source_sha256=hashlib.sha256(file.read_bytes()).hexdigest(),
        independent_filing_accessions=sorted({v['accn'] for r in verified for v in r['filings']}),
        records=verified,scope='Independent reported financial values and completed-quarter filing references, not an exhaustive interim-guidance news catalogue.')
    (out/'sec_crosscheck.json').write_text(json.dumps(result,indent=2))
    print(json.dumps({k:v for k,v in result.items() if k not in ['records','independent_filing_accessions']},indent=2))


if __name__=='__main__':main(sys.argv[1] if len(sys.argv)>1 else 'MDB')
