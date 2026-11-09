import json,collections
from pathlib import Path
R=Path(__file__).resolve().parent;E=R.parent/'evidence'
fills=sorted(json.loads((E/'userFillsByTime.json').read_text()),key=lambda r:r['time']);snap=json.loads((E/'live_latest.json').read_text());active={};episodes=[];lastclose={};expected={};chain_breaks=[]
for f in fills:
 c=f['coin'];pre=float(f['startPosition']);delta=float(f['sz'])*(1 if f['side']=='B' else -1);post=pre+delta;fee=float(f['fee']);closed=float(f['closedPnl']);t=f['time']
 if c not in active:
  active[c]={'coin':c,'start':t,'end':None,'left_censored':abs(pre)>1e-10,'side':'long' if (pre if abs(pre)>1e-10 else delta)>0 else 'short','realized':0.,'fees':0.,'funding':0.,'fills':0,'reentry_hours':(t-lastclose[c])/3600000 if c in lastclose else None}
 if c in expected and abs(pre-expected[c])>max(1e-8,abs(pre)*1e-8):chain_breaks.append({'coin':c,'time':t,'expected':expected[c],'actual':pre})
 expected[c]=post
 ep=active[c];ep['realized']+=closed;ep['fills']+=1
 flip=pre*post<0
 closefee=fee*abs(pre)/abs(delta) if flip else fee;ep['fees']+=closefee
 if abs(post)<1e-9 or flip:
  ep['end']=t;ep['hours']=(t-ep['start'])/3600000;episodes.append(ep);del active[c];lastclose[c]=t
 if flip:
  active[c]={'coin':c,'start':t,'end':None,'left_censored':False,'side':'long' if post>0 else 'short','realized':0.,'fees':fee-closefee,'funding':0.,'fills':1,'reentry_hours':0.}
for c,ep in active.items():
 ep['unrealized']=sum(p['unrealized'] for p in snap['positions'] if p['coin']==c);ep['hours_to_last_history']=(snap['history'][-1]['ts']-ep['start'])/3600000;episodes.append(ep)
for f in json.loads((E/'funding_all.json').read_text()):
 for ep in episodes:
  if ep['coin']==f['delta']['coin'] and ep['start']<=f['time'] and (ep['end'] is None or f['time']<ep['end']):ep['funding']+=float(f['delta']['usdc']);break
for ep in episodes:ep['net']=ep['realized']-ep['fees']+ep['funding']+ep.get('unrealized',0.)
closed=[e for e in episodes if e['end'] and not e['left_censored']]
res={'note':'Positions at first retrieved fill are left-censored. Funding before first fill and initial PnL not allocated. Durations for open positions end at last timestamped account history, not exact snapshot fetch time.','position_chain_breaks':chain_breaks,'episodes':sorted(episodes,key=lambda e:e['net']),'summary':{'closed_uncensored':len(closed),'closed_win_rate':sum(e['net']>0 for e in closed)/max(1,len(closed)),'reentries_within24h':sum(e['reentry_hours'] is not None and e['reentry_hours']<=24 for e in episodes)}}
(R/'live_episodes.json').write_text(json.dumps(res,indent=2));print(json.dumps(res['summary']));print(json.dumps(res['episodes'][:8],indent=2))
