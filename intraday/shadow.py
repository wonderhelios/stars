#!/usr/bin/env python3
"""币安日内趋势前向影子：只读公开行情，独立模拟账本，绝不下单。"""
from pathlib import Path
import fcntl, json, math, os, sys, time, traceback, urllib.parse, urllib.request
from datetime import datetime, timezone
import lightgbm as lgb
import numpy as np
import pandas as pd

ROOT=Path(__file__).resolve().parent
STATE_PATH=Path(os.environ.get('STARS_INTRADAY','/var/lib/stars/intraday_shadow.json'))
CAPITAL=float(os.environ.get('STARS_INTRADAY_CAPITAL','2500'))
FEE=0.0007
THRESHOLD=0.542786741423357  # 净胜率模型：阈值仅在2026-10验证段确定
BASE='https://fapi.binance.com'
FEATURES=json.loads((ROOT/'features.json').read_text())
UNIVERSE=json.loads((ROOT/'universe.json').read_text())
MODEL=lgb.Booster(model_file=str(ROOT/'model.txt'))


def now_ms():return int(datetime.now(timezone.utc).timestamp()*1000)

def get(path,params=None,timeout=15):
    url=BASE+path+('?' + urllib.parse.urlencode(params) if params else '')
    req=urllib.request.Request(url,headers={'User-Agent':'stars-intraday-shadow/1.0'})
    with urllib.request.urlopen(req,timeout=timeout) as r:return json.loads(r.read())

def finite(v):return v is not None and math.isfinite(float(v))

def initial_state():
    return {'running':True,'capital':CAPITAL,'balance':CAPITAL,'equity':CAPITAL,'started_at':now_ms(),
      'positions':[],'trades':[],'signals':[],'history':[[now_ms(),CAPITAL]],'cooldowns':{},
      'realized_pnl':0.0,'total_cost':0.0,'last_update':now_ms(),'last_signal_time':None,'last_error':None}

def load_state():
    try:return json.loads(STATE_PATH.read_text())
    except FileNotFoundError:return initial_state()
    # 损坏的账本必须人工处理，不能静默重置并覆盖历史。

def save_state(state):
    STATE_PATH.parent.mkdir(parents=True,exist_ok=True)
    tmp=STATE_PATH.with_suffix('.tmp');tmp.write_text(json.dumps(state,ensure_ascii=False,separators=(',',':')));tmp.replace(STATE_PATH)

def kline_frame(symbol):
    rows=get('/fapi/v1/klines',{'symbol':symbol,'interval':'15m','limit':300})
    cols=['open_time','open','high','low','close','volume','close_time','quote_volume','count',
          'taker_buy_volume','taker_buy_quote_volume','ignore']
    x=pd.DataFrame(rows,columns=cols)
    for c in cols:x[c]=pd.to_numeric(x[c],errors='coerce')
    x['signal_time']=x.close_time+1
    return x

def features_at(x):
    """返回最后已收盘bar的特征；不读取尚未收盘bar的任何价格。"""
    now=now_ms();closed=x[x.close_time<now].copy()
    if len(closed)<193:return None
    c=closed.close;prev=c.shift(1)
    tr=pd.concat([closed.high-closed.low,(closed.high-prev).abs(),(closed.low-prev).abs()],axis=1).max(axis=1)
    closed['atr_pct']=tr.rolling(96,min_periods=96).mean()/c
    for bars in [4,16,48,96]:closed[f'ret_{bars}']=c/c.shift(bars)-1
    for span in [16,64,192]:
        closed[f'ema_{span}']=c.ewm(span=span,adjust=False,min_periods=span).mean()
        closed[f'ema_dist_{span}']=c/closed[f'ema_{span}']-1
    closed['ema16_slope4']=closed.ema_16/closed.ema_16.shift(4)-1
    closed['ema64_slope16']=closed.ema_64/closed.ema_64.shift(16)-1
    closed['volume_ratio']=closed.quote_volume/closed.quote_volume.rolling(96,min_periods=96).mean()
    closed['quote24']=closed.quote_volume.rolling(96,min_periods=96).sum()
    closed['taker_buy_ratio']=closed.taker_buy_quote_volume/closed.quote_volume
    closed['taker_buy_ratio_4']=closed.taker_buy_quote_volume.rolling(4).sum()/closed.quote_volume.rolling(4).sum()
    closed['range']=(closed.high-closed.low)/c
    closed['close_location']=(2*c-closed.high-closed.low)/(closed.high-closed.low)
    closed['upper_wick']=(closed.high-np.maximum(closed.open,c))/c
    closed['lower_wick']=(np.minimum(closed.open,c)-closed.low)/c
    closed['volatility24']=c.pct_change().rolling(96).std()
    closed['high24_dist']=c/closed.high.rolling(96).max()-1
    closed['low24_dist']=c/closed.low.rolling(96).min()-1
    r=closed.iloc[-1]
    entry=x[x.open_time==int(r.signal_time)]
    if entry.empty:return None
    return r.to_dict(),float(entry.iloc[0].open)

def metric_features(symbol,signal_time):
    oi=get('/futures/data/openInterestHist',{'symbol':symbol,'period':'5m','limit':500})
    top_pos=get('/futures/data/topLongShortPositionRatio',{'symbol':symbol,'period':'5m','limit':2})
    top_acct=get('/futures/data/topLongShortAccountRatio',{'symbol':symbol,'period':'5m','limit':2})
    retail=get('/futures/data/globalLongShortAccountRatio',{'symbol':symbol,'period':'5m','limit':2})
    taker=get('/futures/data/takerlongshortRatio',{'symbol':symbol,'period':'5m','limit':50})
    oi=[r for r in oi if int(r['timestamp'])<=signal_time]
    taker=[r for r in taker if int(r['timestamp'])<=signal_time]
    if len(oi)<289 or signal_time-int(oi[-1]['timestamp'])>6*3600000:raise ValueError('OI数据过期或不完整')
    if len(taker)<50:raise ValueError('主动成交数据不足4小时')
    vals=[float(r['sumOpenInterestValue']) for r in oi]
    latest=lambda rows,key:float([r for r in rows if int(r['timestamp'])<=signal_time][-1][key])
    taker_now=latest(taker,'buySellRatio')
    return {'oi_chg_1h':vals[-1]/vals[-13]-1,'oi_chg_4h':vals[-1]/vals[-49]-1,
      'oi_chg_24h':vals[-1]/vals[-289]-1,'sum_toptrader_long_short_ratio':latest(top_pos,'longShortRatio'),
      'count_toptrader_long_short_ratio':latest(top_acct,'longShortRatio'),
      'count_long_short_ratio':latest(retail,'longShortRatio'),
      'sum_taker_long_short_vol_ratio':taker_now,'oi_value':vals[-1],
      'taker_change_4h':taker_now-float(taker[-49]['buySellRatio'])}


def add_interaction_features(f):
    f['side_oi_1h']=f['side']*f['oi_chg_1h'];f['side_oi_4h']=f['side']*f['oi_chg_4h']
    f['side_oi_24h']=f['side']*f['oi_chg_24h'];f['price_oi_1h']=f['ret_4']*f['oi_chg_1h']
    f['price_oi_4h']=f['ret_16']*f['oi_chg_4h']
    f['top_position_retail_spread']=f['sum_toptrader_long_short_ratio']-f['count_long_short_ratio']
    f['top_account_retail_spread']=f['count_toptrader_long_short_ratio']-f['count_long_short_ratio']
    f['oi_to_volume24']=f['oi_value']/f['quote24']
    return f

def scan_candidates():
    frames={};entries={};errors=[]
    for s in UNIVERSE:
        try:
            x=kline_frame(s);result=features_at(x)
            if result is not None:
                f,entry=result;frames[s]=f;entries[s]=entry
        except Exception as e:errors.append(f'{s}: {e}')
        time.sleep(.06)
    # 大面积失败时排名不完整，本轮必须放弃，不能用残余币池制造信号。
    if len(frames)<len(UNIVERSE)*.9:raise RuntimeError(f'K线覆盖不足 {len(frames)}/{len(UNIVERSE)}；'+('; '.join(errors[:3])))
    signal_times={int(f['signal_time']) for f in frames.values()}
    if len(signal_times)!=1:raise RuntimeError('各币最新已收盘K线不一致')
    signal_time=signal_times.pop()
    rets={s:f['ret_96'] for s,f in frames.items() if finite(f['ret_96'])}
    high=set(sorted(rets,key=rets.get,reverse=True)[:5]);low=set(sorted(rets,key=rets.get)[:5])
    out=[]
    for s,f in frames.items():
        long=s in high and f['ema_16']>f['ema_64'] and f['ema64_slope16']>0
        short=s in low and f['ema_16']<f['ema_64'] and f['ema64_slope16']<0
        if not ((long or short) and f['quote24']>=5_000_000 and finite(f['atr_pct'])):continue
        try:
            f=f.copy();f['side']=1 if long else -1;f['log_quote24']=math.log(max(f['quote24'],1));f.update(metric_features(s,signal_time))
            f=add_interaction_features(f)
            f['probability']=float(MODEL.predict(pd.DataFrame([f])[FEATURES])[0]);f['symbol']=s;f['entry']=entries[s];out.append(f)
        except Exception as e:
            # 单币OI不可用只放弃这个机会，不能中断其他候选。
            errors.append(f'{s} OI: {e}')
    return signal_time,sorted(out,key=lambda x:x['probability'],reverse=True),errors

def features_at_entry(x,f):
    rows=x[x.open_time==int(f['signal_time'])]
    if rows.empty:raise ValueError('下一根开盘尚未产生')
    return float(rows.iloc[0].open)

def snapshot(state,marks=None):
    marks=marks or {};unreal=0.;gross=0.;net=0.
    for p in state['positions']:
        mark=marks.get(p['coin'],p.get('mark_price',p['entry_price']));p['mark_price']=mark
        remaining=p.get('remaining_fraction',1.0);p['remaining_notional']=p['notional']*remaining
        p['unrealized_pnl']=p['remaining_notional']*p['side']*(mark/p['entry_price']-1)
        unreal+=p['unrealized_pnl'];gross+=p['remaining_notional'];net+=p['remaining_notional']*p['side']
    state['equity']=state['balance']+unreal;state['unrealized_pnl']=unreal;state['pnl']=state['equity']-state['capital']
    state['pnl_pct']=100*state['pnl']/state['capital'];state['gross_notional']=gross;state['net_notional']=net
    state['days_elapsed']=(now_ms()-state['started_at'])/86400000
    wins=sum(1 for t in state['trades'] if t['pnl_usd']>0);state['win_rate']=100*wins/len(state['trades']) if state['trades'] else 0
    state['last_update']=now_ms()
    state['config']={'fee_per_side':FEE,'threshold':THRESHOLD,'position_fraction':.25,'max_positions':4,'gross_leverage':1,
      'cooldown_hours':0,'max_holding_hours':6,'partial_fraction':.5,'partial_target_atr':.5,'move_remainder_stop_to_entry':True}
    return state

def process_exits(state,marks):
    now=now_ms()
    for p in list(state['positions']):
        mark=marks.get(p['coin'])
        if not finite(mark):continue
        # 升级前已存在的仓位补齐字段，保留原净值与持仓，不重建账本。
        p.setdefault('remaining_fraction',1.0);p.setdefault('partial_done',False)
        p.setdefault('partial_price',p['entry_price']+(p['target_price']-p['entry_price'])*.5)
        hit_tp=mark>=p['target_price'] if p['side']==1 else mark<=p['target_price']
        hit_sl=mark<=p['stop_price'] if p['side']==1 else mark>=p['stop_price']
        hit_partial=(not p['partial_done'] and (mark>=p['partial_price'] if p['side']==1 else mark<=p['partial_price']))
        # 同一次刷新若同时满足多个条件，优先执行止损，保持保守口径。
        if hit_sl:reason='保本退出' if p['partial_done'] and p['stop_price']==p['entry_price'] else '止损';fraction=p['remaining_fraction']
        elif hit_tp:reason='止盈';fraction=p['remaining_fraction']
        elif hit_partial:reason='半仓止盈';fraction=.5*p['remaining_fraction'] if p['remaining_fraction']<.99 else .5
        elif now>=p['expiry_ts']:reason='超时退出';fraction=p['remaining_fraction']
        else:continue
        fraction=min(fraction,p['remaining_fraction']);notional=p['notional']*fraction
        exit_fee=notional*FEE;gross=notional*p['side']*(mark/p['entry_price']-1);net=gross-exit_fee-p['entry_fee']*fraction
        state['balance']+=gross-exit_fee;state['total_cost']+=exit_fee;state['realized_pnl']+=net
        state['trades'].append({'exit_ts':now,'entry_ts':p['entry_ts'],'coin':p['coin'],'side':'long' if p['side']==1 else 'short',
          'exit_price':mark,'entry_price':p['entry_price'],'notional':notional,'pnl_usd':net,'reason':reason,'probability':p['probability']})
        p['remaining_fraction']-=fraction
        if reason=='半仓止盈':
            p['partial_done']=True;p['stop_price']=p['entry_price']
        if p['remaining_fraction']<=1e-12:state['positions'].remove(p)

def process_entries(state,signal_time,candidates):
    if state.get('last_signal_time')==signal_time:return
    accepted=set()
    for c in candidates:
        s=c['symbol'];reason=''
        if c['probability']<THRESHOLD:reason='评分低于阈值'
        elif any(p['coin']==s for p in state['positions']):reason='已有同币仓位'
        elif len(state['positions'])>=4:reason='已达4仓上限'
        else:
            notional=.25*state['equity'];entry=c['entry'];fee=notional*FEE;side=c['side'];atr=c['atr_pct']
            state['balance']-=fee;state['total_cost']+=fee
            state['positions'].append({'coin':s,'side':side,'entry_ts':signal_time,'entry_price':entry,'mark_price':entry,
              'notional':notional,'entry_fee':fee,'probability':c['probability'],'target_price':entry*(1+side*atr),
              'partial_price':entry*(1+side*.5*atr),'partial_done':False,'remaining_fraction':1.0,
              'stop_price':entry*(1-side*atr),'expiry_ts':signal_time+6*3600000,'unrealized_pnl':0.0})
            accepted.add(s);reason='模拟开仓'
        state['signals'].append({'ts':signal_time,'coin':s,'side':'long' if c['side']==1 else 'short',
          'probability':c['probability'],'accepted':s in accepted,'reason':reason,'ret24':c['ret_96'],'oi_chg_4h':c['oi_chg_4h']})
    state['last_signal_time']=signal_time;state['signals']=state['signals'][-100:]

def run_once():
    state=load_state();marks={}
    try:
        marks={r['symbol']:float(r['markPrice']) for r in get('/fapi/v1/premiumIndex')}
        process_exits(state,marks);snapshot(state,marks)
        now=now_ms();current_hour=now//3600000*3600000
        if state.get('last_signal_time')!=current_hour:
            if current_hour+45000<=now<current_hour+5*60000:
                signal_time,candidates,warnings=scan_candidates()
                if signal_time==current_hour:
                    process_entries(state,signal_time,candidates);snapshot(state,marks)
                    state['last_error']='；'.join(warnings[:3]) if warnings else None
            elif now>=current_hour+5*60000:
                # 重启或停机后不得按一小时前的open补造入场。
                state['last_signal_time']=current_hour
                state['last_error']='错过整点入场窗口，本轮空仓，等待下一小时'
        hist=state['history'];bucket=now//900000*900000
        if not hist or hist[-1][0]!=bucket:hist.append([bucket,state['equity']])
        else:hist[-1]=[bucket,state['equity']]
        state['history']=hist[-2880:];state['trades']=state['trades'][-500:];state['running']=True
    except Exception as e:
        state['last_error']=f'{type(e).__name__}: {e}';state['running']=True;traceback.print_exc()
    save_state(snapshot(state,marks))

def main():
    lock=open(STATE_PATH.with_suffix('.lock'),'w')
    try:fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
    except BlockingIOError:print('日内影子已有进程运行',flush=True);return
    print(f'日内影子启动，状态={STATE_PATH}，只读行情，不下单',flush=True)
    while True:
        # 每15秒只批量读取一次premiumIndex并检查持仓；110币K线和OI仍仅整点扫描。
        run_once();time.sleep(15)

if __name__=='__main__':main()
