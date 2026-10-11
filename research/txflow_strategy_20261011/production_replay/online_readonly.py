from pathlib import Path
import urllib.request,json,time
P=Path(__file__).resolve().parent
out={'read_time':time.time(),'requests':'GET only; private account identifiers and keys omitted'}
for name,path in [('live','/api/txflow/live'),('status','/api/txflow/status')]:
 try:
  d=json.load(urllib.request.urlopen('http://8.219.113.154:3000'+path,timeout=20))
  if name=='live':
   out[name]={k:d.get(k) for k in ['equity','gross_notional','net_notional','margin_used','last_run_at','error','tx_fees','tx_volume','tx_fills','unpriced']}
   out[name]['config']={k:d.get('config',{}).get(k) for k in ['armed','auto_run','target_positions','leverage','margin_buffer','min_order_pct','rebalance_slices','lookback','top_frac','min_vol_usd','take_profit_pct']}
   records=d.get('records',[]);out[name]['records_total']=len(records)
   out[name]['record_field_names']=sorted(records[0]) if records else []
   out[name]['last_plan']=d.get('last_plan',[])
  else:out[name]={k:d.get(k) for k in ['exchange','fee_maker','fee_taker','cached_coins','liquid_coins','latest_ts','refreshed_at']}
 except Exception as e:out[name]={'fetch_error':str(e)}
(P/'online_sanitized.json').write_text(json.dumps(out,ensure_ascii=False,indent=2));print(json.dumps(out,ensure_ascii=False)[:6500])
