"""Forward schedules from original minutes, available only after publication."""
import hashlib
import json
import re
from pathlib import Path
import pandas as pd
from bs4 import BeautifulSoup

ROOT=Path(__file__).resolve().parent
OUT=ROOT/'macro_calendar'
MONTH='January|February|March|April|May|June|July|August|September|October|November|December'


def normalize(t):
    return re.sub(r'\s+',' ',t.replace('\xa0',' ').replace('\u200d','').replace('\u200b','')).strip()


def main():
    downloaded=json.loads((OUT/'downloaded.json').read_text());rows=[];issues=[]
    for item in downloaded:
        try:
            file=OUT/item['file'];s=BeautifulSoup(file.read_text(),'html.parser');t=normalize(s.get_text(' ',strip=True))
            m=re.search(r'(?:It was agreed|The Committee agreed) that the next meeting of the Committee would be held on ([^.]+)\.',t)
            assert m, 'No explicit next scheduled meeting sentence'
            dates=list(re.finditer(rf'({MONTH})\s+(\d{{1,2}})(?:\s*[–—-]\s*(\d{{1,2}}))?',m[1]))
            year=re.findall(r'20\d{2}',m[1]);assert dates and len(set(year))==1
            last=dates[-1];day=pd.Timestamp(f'{last[1]} {last[3] or last[2]}, {year[-1]}')
            stamp=re.search(rf'Last Update:\s*(({MONTH}) \d{{1,2}}, 20\d{{2}})',t)
            assert stamp,'Missing original-page last update date'
            updated=pd.Timestamp(stamp[1]);published=pd.Timestamp(item['published_date'])
            meeting=pd.Timestamp(item['meeting_id']);assert meeting<published<day and (day-meeting).days<100
            known=(max(updated,published)+pd.Timedelta(days=1,hours=1)).tz_localize('America/New_York')
            rows.append(dict(source=item['url'],source_file=item['file'],source_meeting_date=str(meeting.date()),
                published_date=str(published.date()),last_update_date=str(updated.date()),available_after=str(known),
                planned_date=str(day.date()),schedule_evidence=m[0],update_evidence=stamp[0],
                sha256=hashlib.sha256(file.read_bytes()).hexdigest(),late_update_beyond_planned_event=known.date()>day.date()))
        except Exception as e:issues.append(dict(source=item['url'],file=item['file'],issue=str(e),type=type(e).__name__))
    rows.sort(key=lambda r:pd.Timestamp(r['available_after']))
    (OUT/'planned_events.json').write_text(json.dumps(rows,indent=2))
    (OUT/'parse_issues.json').write_text(json.dumps(issues,indent=2))
    result=dict(downloaded=len(downloaded),parsed=len(rows),issues=issues,late_updates=sum(r['late_update_beyond_planned_event'] for r in rows),
        performance_read_at_initial_parse=False)
    (OUT/'parse_summary.json').write_text(json.dumps(result,indent=2));print(json.dumps(result,indent=2))


if __name__=='__main__':main()
