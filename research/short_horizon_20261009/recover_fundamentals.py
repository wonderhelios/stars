"""One smaller, field-specific public API attempt for failed bulk responses."""
import json, subprocess, hashlib, time
from datetime import datetime, timezone
from fetch_fundamentals import OUT, ISSUERS
from quality import TAGS

def main():
    path=OUT/'manifest.json';manifest=json.loads(path.read_text())
    assert len(manifest)==30,'Wait for initial downloader to finish.'
    backup=OUT/'initial_manifest.json'
    if not backup.exists():backup.write_text(path.read_text())
    for rec in manifest:
        if rec.get('name_verified') or rec.get('recovery_attempted'):continue
        rec['recovery_attempted']=True
        sym=rec['symbol'];cik,expected=ISSUERS[sym]
        facts={};sources=[];verified=False;name=None
        for tag in TAGS+['OperatingIncomeLoss']:
            url=f'https://data.sec.gov/api/xbrl/companyconcept/CIK{cik:010d}/us-gaap/{tag}.json'
            cache=OUT/f'{sym}_{tag}.json'
            if not cache.exists():
                p=subprocess.run(['curl','--fail','--silent','--show-error','--max-time','30','-A','stars-research/1.0 (public financial research)',url,'-o',str(cache)],capture_output=True,text=True)
                if p.returncode:
                    sources.append(dict(url=url,error=p.stderr[:180]))
                    if cache.exists():cache.unlink()
                    continue
                time.sleep(1)
            try:
                d=json.loads(cache.read_text());assert int(d['cik'])==cik
                assert expected in d['entityName'].lower()
                verified=True;name=d['entityName']
                facts[tag]=dict(label=d.get('label',tag),description=d.get('description',''),units=d['units'])
                sources.append(dict(url=url,sha256=hashlib.sha256(cache.read_bytes()).hexdigest()))
            except Exception as exc:
                sources.append(dict(url=url,error=str(exc)))
        rec['recovery_sources']=sources
        if verified and 'OperatingIncomeLoss' in facts and any(t in facts for t in TAGS):
            p=OUT/f'{sym}.json';p.write_text(json.dumps(dict(cik=cik,entityName=name,facts={'us-gaap':facts})))
            rec.update(cik=cik,name=name,name_verified=True,source='SEC companyconcept, merged only requested tags',downloaded_utc=datetime.now(timezone.utc).isoformat(),sha256=hashlib.sha256(p.read_bytes()).hexdigest())
            rec['initial_error']=rec.pop('error')
        path.write_text(json.dumps(manifest,indent=2))
        print(sym,'recovered' if rec.get('name_verified') else 'still unavailable',flush=True)

if __name__=='__main__':main()
