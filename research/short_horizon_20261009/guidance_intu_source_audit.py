"""Audit sources, distinguish rounded growth proxies, pre-register source gates."""
import hashlib
import json
import re
import pandas as pd
from bs4 import BeautifulSoup
from guidance_parse_intu import OUT,norm


def main():
    rows=json.loads((OUT/'parsed_guidance.json').read_text());interim=json.loads((OUT/'interim_notices.json').read_text())
    catalog=json.loads((OUT/'catalogue.json').read_text());directory=BeautifulSoup((OUT/'quarterly_directory.html').read_text(),'html.parser')
    found={}
    for box in directory.select('.results-info'):
        title=box.find('h3');m=re.fullmatch(r'(Q[1-4]|FY) (20\d{2})',title.get_text(' ',strip=True)) if title else None
        if not m:continue
        a=box.parent.find('a',href=lambda h:h and '/press-releases/detail/' in h)
        if a:found[(int(m[2]),4 if m[1]=='FY' else int(m[1][-1]))]=dict(source=a['href'],label=m[0],evidence=box.get_text(' ',strip=True))
    assert len(rows)==27 and not json.loads((OUT/'parse_issues.json').read_text())
    for r in rows+interim:
        file=OUT.parent/r['source_file'];raw=file.read_bytes();assert hashlib.sha256(raw).hexdigest()==r['sha256']
        assert json.loads(file.with_suffix('.meta.json').read_text())['sha256']==r['sha256']
        item=next(x for x in catalog if x['url']==r['source'])
        listed=BeautifulSoup((OUT/item['catalogue_file']).read_text(),'html.parser')
        assert listed.find('a',href=r['source'])
        if r['kind']=='quarterly':
            entry=found[(r['reported_fiscal_year'],r['reported_quarter'])];assert entry['source']==r['source']
            body=norm(BeautifulSoup(raw,'html.parser').select_one('.main-content').get_text(' ',strip=True))
            for excerpt in r['guidance_source']:assert excerpt in body
    sec=json.loads((OUT/'sec_crosscheck.json').read_text());assert sec['verified']==27 and not sec['issues']
    # Later statements explicitly recap earlier dollar guidance. They demonstrate
    # rounding differences, but are NOT backdated to replace the earlier forecast.
    checks=[]
    for id,previous_date in [('175','2021-02-23'),('129','2021-11-18')]:
        notice=next(x for x in interim if x['source_file']==f'INTU/release_{id}.html')
        m=re.search(r'down from the prior (?:guidance )?range of \$([\d.]+) billion to \$([\d.]+) billion',notice['source_text']);assert m
        prior=next(x for x in rows if x['published'].startswith(previous_date));reported=[float(m[1])*1000,float(m[2])*1000]
        checks.append(dict(prior_source=prior['source'],later_source=notice['source'],later_available=notice['available_after'],
            growth_proxy=prior['quarter_range_million'],later_recalled_prior_dollar_range=reported,
            endpoint_difference_million=[a-b for a,b in zip(prior['quarter_range_million'],reported)],
            later_evidence_used_as_earlier_signal=False))
    rules=[]
    for date,ids,reason in [
        ('2021-02-23',['201','189'],'Intervening Credit Karma scope update and lower preliminary quarter revenue supersede the previous quarterly forecast.'),
        ('2021-05-25',['175'],'Full-year upside and lower current-quarter revenue were preannounced May 11; calendar shift in tax filing, not a first surprise on May 25.'),
        ('2021-11-18',[],'New Mailchimp scope. Release explicitly quantifies $760–770m FY contribution and ex-Mailchimp growth; the unadjusted total-revenue pair is incomparable. A bridge is possible but has not been implemented or audited; do not call it unquantified.'),
        ('2022-02-24',['129'],'Quarter revenue was preannounced lower on February 14 while annual guidance was reiterated; original quarterly benchmark is stale.'),
        ('2022-11-29',['101'],'Preliminary quarter outperformance and Credit Karma deterioration were disclosed November 1 before the formal quarterly release.')]:
        current=next(r for r in rows if r['published'].startswith(date))
        notices=[next(r for r in interim if r['source_file']==f'INTU/release_{id}.html') for id in ids]
        assert all(pd.Timestamp(r['published'])<pd.Timestamp(current['published']) for r in notices)
        rules.append(dict(symbol='INTU',published=date,source=current['source'],action='exclude_incomparable_guidance_pair',
            reason=reason,intervening_sources=[r['source'] for r in notices],registered_before_expansion_performance=True))
    gates=dict(rules=rules,quarter_forecast_precision_gate='Prose growth-derived ranges remain ineligible proxies. Original HTML Table E and fact sheets now recover 25 exact ranges; use exact_guidance.json after its remaining publication/interim gates pass, never the old prose proxies.',
        remaining_review='All interim financial news categories and conference-call versions are not complete; remaining scope audit pending.',
        notes=['FY2025 desktop timing changes already discussed in initial annual guidance: do not blanket-delete subsequent same-year total-revenue pairs.',
        'FY2027 Mailchimp segment reporting/non-GAAP changes are not automatically changes to total GAAP revenue scope.'])
    (OUT/'source_review_gates.json').write_text(json.dumps(gates,indent=2));(OUT/'growth_precision_audit.json').write_text(json.dumps(checks,indent=2))
    summary=dict(quarterly_directory_matches=len(rows),quarter_actual_SEC_verified=sec['verified'],interim_notices=len(interim),
        growth_rate_forecast_quarters=sum(r['quarter_growth_percent'] is not None for r in rows),
        converted_growth_proxy_quarters=sum(r.get('quarter_conversion_basis') is not None for r in rows),
        missing_growth_base_quarters=sum(r.get('quarter_conversion_issue') is not None for r in rows),
        explicit_dollar_forecast_quarters=sum(r.get('quarter_range_basis')=='explicit_original_dollar_range' for r in rows),
        no_quarter_guidance_in_original_release=sum(r.get('quarter_range_million') is None and r['quarter_growth_percent'] is None for r in rows),
        publication_crosschecks=36,modified_metadata_unavailable=36,source_gate_count=len(rules),
        ready_for_full_strategy_test=False,market_performance_read=False,
        limitations=['No mutation-time metadata or independently archived originals','Prose extraction alone misses exact dollar forecasts in original HTML Table E; separate html_table_audit.json and exact_merge_audit.json cover the correction',
        'Complete investor-day/call/all-news coverage still pending'])
    (OUT/'source_audit.json').write_text(json.dumps(summary,indent=2));print(json.dumps(summary,indent=2))


if __name__=='__main__':main()
