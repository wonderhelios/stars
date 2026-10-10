"""Fetch observed issuer fact-sheet PDFs only; preserve source versions and failures."""
import hashlib
import json
import re
import subprocess
import time
from pathlib import Path
from bs4 import BeautifulSoup

OUT=Path(__file__).resolve().parent/'guidance_expansion/INTU'


def main():
    directory=BeautifulSoup((OUT/'quarterly_directory.html').read_text(),'html.parser')
    rows=json.loads((OUT/'parsed_guidance.json').read_text());done=[];errors=[]
    for row in rows:
        id=re.search(r'/detail/(\d+)/',row['source'])[1]
        a=directory.find('a',href=row['source']);links=[a['href'] for a in a.parent.parent.find_all('a',href=True) if a.get_text(' ',strip=True)=='Fact Sheet PDF']
        if len(links)!=1:errors.append(dict(release=row['source'],issue='No unique fact sheet link'));continue
        url=links[0];p=OUT/f'original_facts_{id}.pdf';meta=p.with_suffix('.meta.json')
        if meta.exists():
            m=json.loads(meta.read_text())
            if m['status']!=200:errors.append(m);break
            assert hashlib.sha256(p.read_bytes()).hexdigest()==m['sha256']
        else:
            attempts=[]
            for attempt in range(2):
                r=subprocess.run(['curl','-L','--max-time','60','-sS','-o',str(p),'-w','%{http_code}',url],capture_output=True,text=True)
                m=dict(url=url,status=int(r.stdout) if r.stdout.isdigit() else 0,exit_code=r.returncode,error=r.stderr,
                    retrieved_utc=time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),sha256=hashlib.sha256(p.read_bytes()).hexdigest() if p.exists() else None)
                attempts.append(m.copy());m['attempts']=attempts;meta.write_text(json.dumps(m,indent=2))
                if m['status']!=0 or attempt==1:break
                time.sleep(2)
            if m['status']!=200 or m['exit_code']:
                errors.append(m)
                if m['status'] in [403,429]:break
                continue
            time.sleep(1)
        assert p.read_bytes().startswith(b'%PDF-')
        done.append(dict(release_id=id,release_source=row['source'],release_published=row['published'],
            reported_fiscal_year=row['reported_fiscal_year'],reported_quarter=row['reported_quarter'],file=p.name,**m))
        (OUT/'facts_downloaded.json').write_text(json.dumps(done,indent=2));print(len(done),len(rows),p.name,flush=True)
    summary=dict(expected=len(rows),downloaded=len(done),errors=errors,complete_download=len(done)==len(rows),market_performance_read=False)
    (OUT/'facts_collection_status.json').write_text(json.dumps(summary,indent=2));print(json.dumps(summary,indent=2))


if __name__=='__main__':main()
