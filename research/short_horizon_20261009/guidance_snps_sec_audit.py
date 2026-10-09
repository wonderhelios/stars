"""Verify true fiscal-week endpoints and original, not restated, revenue."""
import hashlib
import json
from pathlib import Path
import pandas as pd

ROOT=Path(__file__).resolve().parent
OUT=ROOT/'guidance_expansion/SNPS'


def main():
    file=ROOT/'fundamentals/SNPS.json';doc=json.loads(file.read_text())
    assert doc['cik']==883241 and 'SYNOPSYS' in doc['entityName']
    facts=[dict(v,concept=tag) for tag in ['RevenueFromContractWithCustomerExcludingAssessedTax','Revenues']
        for v in doc['facts']['us-gaap'][tag]['units']['USD']]
    rows=json.loads((OUT/'parsed_guidance.json').read_text());verified=[];issues=[]
    for row in rows:
        pub=pd.Timestamp(row['published']).tz_localize(None).normalize();end=row['reported_quarter_end'];fy=row['reported_fiscal_year'];q=row['reported_quarter']
        # SNPS explicitly presents calendar month ends even when its fiscal weeks
        # end a few days apart. XBRL uses that presentation date. Preserve both;
        # accept only the two source-observed endpoints, never a fuzzy date window.
        reported_end=row['presented_calendar_end']
        allowed_ends={end,reported_end}
        expected=round(row['actual_revenue_million']*1e6)
        direct=[v for v in facts if v.get('start') and v.get('end') in allowed_ends and 70<=(pd.Timestamp(v['end'])-pd.Timestamp(v['start'])).days<=110
            and v['val']==expected and pub<=pd.Timestamp(v['filed'])<=pub+pd.Timedelta(days=15)]
        record=dict(source=row['source'],actual_fiscal_end=end,presented_calendar_end=reported_end,
            period_basis=row['period_basis'],expected_dollars=expected)
        if direct:
            v=min(direct,key=lambda v:v['filed']);record.update(method='original_quarter',filings=[v],error_dollars=0)
        elif q==4:
            year=[v for v in facts if v.get('end') in allowed_ends and v.get('start') and 320<=(pd.Timestamp(v['end'])-pd.Timestamp(v['start'])).days<=380
                and v.get('fy')==fy and v.get('fp')=='FY' and pub<=pd.Timestamp(v['filed'])<=pub+pd.Timedelta(days=30)]
            prior=next((r for r in rows if r['reported_fiscal_year']==fy and r['reported_quarter']==3),None)
            nine=[v for v in facts if prior and v.get('end') in {prior['reported_quarter_end'],prior['presented_calendar_end']} and v.get('start') and 240<=(pd.Timestamp(v['end'])-pd.Timestamp(v['start'])).days<=300 and pd.Timestamp(v['filed'])<=pub]
            pairs=[(y,n) for y in year for n in nine if y['start']==n['start'] and y['val']-n['val']==expected]
            if not pairs:issues.append(dict(**record,issue='no exact same-scope annual-minus-nine-month original filing'));continue
            y,n=min(pairs,key=lambda x:(x[0]['filed'],x[1]['filed']));record.update(method='annual_minus_original_nine_month',filings=[y,n],error_dollars=0)
        else:
            issues.append(dict(**record,issue='no exact original quarter value at source-defined presentation/fiscal endpoints'));continue
        verified.append(record)
    result=dict(expected=len(rows),verified=len(verified),issues=issues,records=verified,
        source_sha256=hashlib.sha256(file.read_bytes()).hexdigest(),
        scope='Later independent verification of announced actual values and true financial periods; not an input to announcement-time signals.')
    (OUT/'sec_crosscheck.json').write_text(json.dumps(result,indent=2))
    print(json.dumps({k:v for k,v in result.items() if k!='records'},indent=2))


if __name__=='__main__':main()
