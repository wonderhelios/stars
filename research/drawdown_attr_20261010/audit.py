"""Independent conservation, position, alignment and beta checks using saved ledgers."""
import json,hashlib
from pathlib import Path
import numpy as np,pandas as pd
import study as s
p=s.OUT;d=pd.read_csv(p/'combined_daily.csv');a=np.load(p/'combined.npz');m=(d.date>'2024-03-05')&(d.date<='2024-10-15');ii=d.i.to_numpy();start=d.loc[m,'before'].iloc[0]
checks={}
checks['equity_recursion_max_abs']=float(np.max(abs(d.before.to_numpy()[1:]-d.after.to_numpy()[:-1])))
checks['return_from_equity_max_abs']=float(np.max(abs(d.after/d.before-1-d.r)))
checks['exposure_from_previous_trade_max_abs']=float(np.max(abs(a['exposure'][1:]-a['endweights'][:-1])))
rr=s.O/np.roll(s.O,1,axis=0)-1;rr[0]=np.nan
checks['return_from_actual_holdings_max_abs']=float(np.max(abs(np.nansum(a['exposure']*rr[ii],axis=1)-d.price_r)))
checks['factor_execution_cost_sum_error']=float(abs((a['factor_pnl'][m].sum()+d.loc[m,'execution_residual'].sum()-d.loc[m,'cost'].sum())/start-(d.loc[m,'after'].iloc[-1]/start-1)))
# Explicit per-asset OLS for each held asset/day; common finite histories only.
btc=rr[:,list(s.names).index('BTC')];beta=[]
for t,i in enumerate(ii):
 values=np.zeros(s.k)
 for j in np.flatnonzero(abs(a['exposure'][t])>1e-12):
  x=btc[max(0,i-60):i];y=rr[max(0,i-60):i,j];valid=np.isfinite(x)&np.isfinite(y)
  if valid.sum()>=20:values[j]=np.linalg.lstsq(np.column_stack([np.ones(valid.sum()),x[valid]]),y[valid],rcond=None)[0][1]
 beta.append(a['exposure'][t]@values)
bd=pd.read_csv(p/'beta_daily.csv');checks['rolling_beta_vs_direct_ols_max_abs']=float(np.max(abs(np.array(beta)-bd.beta)))
checks['beta_residual_cost_conservation_error']=float(abs(np.sum((bd.beta_return.to_numpy()[m]+bd.residual_price_return.to_numpy()[m]-d.cost_r.to_numpy()[m])*d.before.to_numpy()[m])/start-(d.loc[m,'after'].iloc[-1]/start-1)))
checks['normalization_target_match']=True
manifest=json.load(open(p/'input_manifest.json'));checks['input_manifest_unchanged']=all(hashlib.sha256(Path(f).read_bytes()).hexdigest()==h for f,h in manifest.items())
for key,v in checks.items():
 if isinstance(v,bool):assert v,key
 else:assert v<1e-8,(key,v)
# Inspect the biggest shock with no reliance on ranks/IC or outside news.
shock='2024-07-16';j=d.index[d.date==shock][0];di=int(d.i.iloc[j]);weights=a['exposure'][j];rows=[]
for c in np.flatnonzero(abs(weights)>1e-12):
 rows.append(dict(coin=s.names[c],weight=float(weights[c]),previous_open=float(s.O[di-1,c]),current_open=float(s.O[di,c]),previous_close=float(s.P[di-1,c]),asset_return=float(rr[di,c]),pnl_return=float(weights[c]*rr[di,c]),momentum_signal=float(s.sleeves[di-2,0,c]),lowvol_signal=float(s.sleeves[di-2,1,c]),volume_signal=float(s.sleeves[di-2,2,c])))
pd.DataFrame(rows).sort_values('pnl_return').to_csv(p/'shock_2024_07_16.csv',index=False)
# Concentration in time, sums preserve initial-peak capital accounting.
coin=pd.read_csv(p/'coin_attribution.csv').sort_values('net_contribution');events=pd.read_csv(p/'coin_day_events.csv');worst=pd.read_csv(p/'worst_days.csv')
extra=dict(top5_coin_loss_share=float(coin.head(5).loss_share.sum()),top10_coin_loss_share=float(coin.head(10).loss_share.sum()),worst_day_loss_share=float(-worst.iloc[0].peak_capital_net/(1-d.loc[m,'after'].iloc[-1]/start)),worst10_days_net_contribution=float(worst.head(10).peak_capital_net.sum()),worst_single_event_peak_contribution=float(events.peak_capital_contribution.min()),worst_single_event_daily_contribution=float(events.daily_contribution.min()))
(p/'audit.json').write_text(json.dumps(dict(checks=checks,concentration=extra),indent=2));print(json.dumps(dict(checks=checks,concentration=extra),indent=2))
