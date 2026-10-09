"""Case audit, not a statistical strategy from four announcements."""
import json
import pandas as pd
from screen import load, SCHED, ROOT

EVENTS=[
 ('Gemini 1','2023-12-06','https://blog.google/innovation-and-ai/technology/ai/google-gemini-ai/'),
 ('Gemini 2','2024-12-11','https://blog.google/innovation-and-ai/models-and-research/google-deepmind/google-gemini-ai-update-december-2024/'),
 ('Gemini 3','2025-11-18','https://blog.google/products-and-platforms/products/gemini/gemini-3/'),
 ('Gemini 4','2026-09-30','https://blog.google/innovation-and-ai/models-and-research/gemini-models/gemini-4-argon/'),
]

def main():
 data,_=load();rows=[]
 for name,date,url in EVENTS:
  i=SCHED.index.searchsorted(pd.Timestamp(date),side='right')
  for hold in [1,2,5,20]:
   j=i+hold-1
   rec=dict(event=name,date=date,source=url,hold_sessions=hold,entry=str(SCHED.iloc[i]['open']))
   if j>=len(SCHED):
    rec['status']='window incomplete at frozen cutoff';rows.append(rec);continue
   rec.update(status='case study only',exit=str(SCHED.iloc[j]['close']),hours=(SCHED.iloc[j]['close']-SCHED.iloc[i]['open']).total_seconds()/3600)
   rec['within_48h']=rec['hours']<=48
   for symbol in ['GOOGL','QQQ']:
    d=data[symbol];pe=d.iloc[i]['open'];px=d.iloc[j]['close'];div=d.iloc[i+1:j+1]['div'].sum()
    rec[symbol+'_gross']=(px+div)/pe-1
    rec[symbol+'_net_5bp']=(px*(1-.0005)+div)/(pe*(1+.0005))-1
   rec['relative_net']=rec['GOOGL_net_5bp']-rec['QQQ_net_5bp']
   rows.append(rec)
 pd.DataFrame(rows).to_csv(ROOT/'gemini_events.csv',index=False)
 (ROOT/'gemini_events.json').write_text(json.dumps(rows,indent=2))
 print(pd.DataFrame(rows).to_string(index=False))

if __name__=='__main__':main()
