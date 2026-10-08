import json,math,requests,os
from pathlib import Path
O=Path(__file__).resolve().parent
os.environ.update(https_proxy='http://127.0.0.1:7897',http_proxy='http://127.0.0.1:7897',all_proxy='socks5://127.0.0.1:7897')
now=json.load(open(O/'raw/metadata.json'))['server_ms'];out={}
for coin in ['BTC','ETH']:
 ins=json.load(open(O/f'raw/{coin}_instruments.json'))['result'];su={x['instrument_name']:x for x in json.load(open(O/f'raw/{coin}_summary.json'))['result']}
 exp=min({x['expiration_timestamp'] for x in ins},key=lambda t:abs(t-now-30*86400000));a=[]
 for x in ins:
  q=su.get(x['instrument_name'],{});iv=q.get('mark_iv');spot=q.get('underlying_price')
  if x['expiration_timestamp']!=exp or not iv or not spot:continue
  T=(exp-now)/86400000/365;v=iv/100;d1=(math.log(spot/x['strike'])+.5*v*v*T)/(v*math.sqrt(T));delta=.5*(1+math.erf(d1/math.sqrt(2)))-(x['option_type']=='put')
  a.append((x,delta))
 rows=[]
 for typ,target in [('call',.25),('put',-.25)]:
  x,de=min([x for x in a if x[0]['option_type']==typ],key=lambda x:abs(x[1]-target));name=x['instrument_name']
  j=requests.get('https://www.deribit.com/api/v2/public/get_order_book',params={'instrument_name':name,'depth':20},timeout=40).json();json.dump(j,open(O/f'raw/{name}_skew.json','w'));q=j['result']
  rows.append({'instrument':name,'approx_selection_delta':de,'actual_delta':q['greeks']['delta'],'mark_iv':q['mark_iv'],'bid_iv':q.get('bid_iv'),'ask_iv':q.get('ask_iv')})
 out[coin]={'legs':rows,'call_minus_put_mark_iv_points':rows[0]['mark_iv']-rows[1]['mark_iv'],'warning':'single-date mark snapshot; not predictive evidence'}
json.dump(out,open(O/'skew_snapshot.json','w'),indent=2)
