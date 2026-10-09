"""Read-only SEC public fundamentals. Curl uses the system TLS stack."""
import json, subprocess, time, hashlib
from datetime import datetime, timezone
from pathlib import Path
from fetch import ROOT, STOCKS

OUT=ROOT/'fundamentals'
# Issuer identifier candidates, verified against each returned legal entity name.
# The ticker-directory URL returned HTTP 403; do not retry or bypass that file.
ISSUERS={
 'GOOGL':(1652044,'alphabet'),'MSFT':(789019,'microsoft'),'AAPL':(320193,'apple'),
 'AMZN':(1018724,'amazon'),'META':(1326801,'meta'),'NVDA':(1045810,'nvidia'),
 'AMD':(2488,'advanced micro'),'INTC':(50863,'intel'),'ORCL':(1341439,'oracle'),
 'CRM':(1108524,'salesforce'),'ADBE':(796343,'adobe'),'INTU':(896878,'intuit'),
 'NOW':(1373715,'servicenow'),'VEEV':(1393052,'veeva'),'GWRE':(1528396,'guidewire'),
 'TYL':(860731,'tyler'),'ADSK':(769397,'autodesk'),'ANSS':(1013462,'ansys'),
 'CDNS':(813672,'cadence'),'SNPS':(883241,'synopsys'),'PAYC':(1590955,'paycom'),
 'PCTY':(1591698,'paylocity'),'WDAY':(1327811,'workday'),'TEAM':(1650372,'atlassian'),
 'DDOG':(1561550,'datadog'),'ZS':(1713683,'zscaler'),'CRWD':(1535527,'crowdstrike'),
 'NET':(1477333,'cloudflare'),'MDB':(1441816,'mongodb'),'HUBS':(1404655,'hubspot')}

def get(url,path):
    if path.exists():
        return json.loads(path.read_text())
    tmp=path.with_suffix('.partial')
    result=subprocess.run(['curl','--fail','--silent','--show-error','--max-time','45','-A','stars-research/1.0 (public financial research)',url,'-o',str(tmp)],capture_output=True,text=True)
    if result.returncode:
        if tmp.exists():tmp.unlink()
        raise RuntimeError(result.stderr[:200])
    data=json.loads(tmp.read_text());tmp.replace(path)
    time.sleep(1)
    return data

def main():
    OUT.mkdir(exist_ok=True)
    probe=ROOT/'sec_probe.json'
    if probe.exists():
        d=json.loads(probe.read_text());assert int(d['cik'])==1393052
        (OUT/'VEEV.json').write_text(probe.read_text())
    manifest=[]
    for s in STOCKS:
        rec=dict(symbol=s,downloaded_utc=datetime.now(timezone.utc).isoformat())
        try:
            cik,expected=ISSUERS[s];url=f'https://data.sec.gov/api/xbrl/companyfacts/CIK{cik:010d}.json'
            p=OUT/f'{s}.json';d=get(url,p)
            assert int(d['cik'])==cik
            assert expected in d['entityName'].lower(), (s,d['entityName'])
            rec.update(cik=cik,url=url,name=d['entityName'],name_verified=True,sha256=hashlib.sha256(p.read_bytes()).hexdigest())
        except Exception as exc:
            rec['error']=str(exc)
        manifest.append(rec)
        (OUT/'manifest.json').write_text(json.dumps(manifest,indent=2))
        print(s,rec.get('name',rec.get('error')),flush=True)

if __name__=='__main__':main()
