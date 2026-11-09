import json,collections,datetime,shutil
from pathlib import Path
R=Path(__file__).resolve().parent;S=Path('/tmp/stars-audit-20261017');(R/'evidence').mkdir(exist_ok=True)
for f in ['live_latest.json','userFillsByTime.json','funding_all.json','userNonFundingLedgerUpdates.json','summary.json']:
 shutil.copy2(S/f,R/'evidence'/f)
x=json.loads((S/'live_latest.json').read_text()); fills=json.loads((S/'userFillsByTime.json').read_text());fund=json.loads((S/'funding_all.json').read_text())
coins=collections.defaultdict(lambda:dict(realized=0.,fee=0.,funding=0.,unrealized=0.,volume=0.,fills=0))
for r in fills:
 c=coins[r['coin']];c['realized']+=float(r['closedPnl']);c['fee']+=float(r['fee']);c['volume']+=float(r['px'])*float(r['sz']);c['fills']+=1
for r in fund:coins[r['delta']['coin']]['funding']+=float(r['delta']['usdc'])
for p in x['positions']:coins[p['coin']]['unrealized']=p['unrealized']
for c in coins.values():c['subtotal']=c['realized']-c['fee']+c['funding']+c['unrealized']
rows=sorted([dict(coin=k,**v) for k,v in coins.items()],key=lambda x:x['subtotal'])
# Daily ending snapshots. No claims of exact midnight returns.
hist={}
for p in x['history']:hist[datetime.datetime.fromtimestamp(p['ts']/1000,datetime.timezone.utc).strftime('%Y-%m-%d')]=p
flows=[(1791654639144,2498.443151),(1791766766391,2699.8)]
prev=x['history'][0];daily=[]
for day,p in hist.items():
 netflows=sum(a for t,a in flows if prev['ts']<t<=p['ts']);daily.append(dict(utc_day=day,ts=p['ts'],equity=p['equity'],net_deposit=netflows,pnl=p['equity']-prev['equity']-netflows));prev=p
out={'note':'Coin subtotal does not allocate the initial retained unrealized PnL of -10.612746. Daily bins end at last retained snapshot, not necessarily midnight. Config history not available; current rules cannot be asserted as rules used all week.','coins':rows,'daily':daily}
(R/'attribution.json').write_text(json.dumps(out,indent=2))
print(json.dumps(rows[:7],indent=2));print('daily',[(r['utc_day'],round(r['pnl'],2)) for r in daily])
