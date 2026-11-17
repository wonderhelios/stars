import unittest
import tempfile
from pathlib import Path
from unittest.mock import patch
import shadow
from shadow import initial_state,process_entries,process_exits,snapshot,FEE


def candidate(symbol,probability=.60,side=1):
    return {'symbol':symbol,'probability':probability,'entry':100.,'side':side,'atr_pct':.02,
            'ret_96':.2*side,'oi_chg_4h':.1}

class ShadowBookTests(unittest.TestCase):
    def test_opportunities_max_four_and_idempotent(self):
        s=initial_state();s['equity']=2500
        cs=[candidate(f'C{i}USDT') for i in range(6)]
        process_entries(s,1000,cs)
        self.assertEqual(len(s['positions']),4)
        self.assertEqual(s['last_signal_time'],1000)
        balance=s['balance'];process_entries(s,1000,cs)
        self.assertEqual(len(s['positions']),4);self.assertEqual(s['balance'],balance)
        self.assertTrue(all(p['notional']==625 for p in s['positions']))
        self.assertAlmostEqual(s['total_cost'],4*625*FEE)

    def test_low_probability_does_not_trade(self):
        s=initial_state();process_entries(s,1000,[candidate('BTCUSDT',.54)])
        self.assertEqual(len(s['positions']),0);self.assertEqual(s['signals'][0]['reason'],'概率低于阈值')

    def test_long_partial_take_profit_then_breakeven_without_cooldown(self):
        s=initial_state();process_entries(s,1000,[candidate('BTCUSDT')]);p=s['positions'][0]
        original_notional=p['notional']
        process_exits(s,{'BTCUSDT':p['partial_price']})
        self.assertEqual(len(s['positions']),1);self.assertEqual(len(s['trades']),1)
        self.assertEqual(s['trades'][0]['reason'],'半仓止盈')
        self.assertAlmostEqual(p['remaining_fraction'],.5)
        self.assertEqual(p['stop_price'],p['entry_price'])
        snapshot(s,{'BTCUSDT':p['entry_price']})
        self.assertAlmostEqual(p['remaining_notional'],original_notional*.5)
        process_exits(s,{'BTCUSDT':p['entry_price']})
        self.assertEqual(len(s['positions']),0);self.assertEqual(s['trades'][-1]['reason'],'保本退出')
        self.assertNotIn('BTCUSDT',s['cooldowns'])
        self.assertAlmostEqual(sum(t['pnl_usd'] for t in s['trades']),s['realized_pnl'])

    def test_short_stop_and_accounting(self):
        s=initial_state();process_entries(s,1000,[candidate('ETHUSDT',side=-1)]);p=s['positions'][0]
        process_exits(s,{'ETHUSDT':p['stop_price']});snapshot(s,{})
        self.assertEqual(s['trades'][0]['reason'],'止损');self.assertLess(s['trades'][0]['pnl_usd'],0)
        self.assertAlmostEqual(s['equity'],s['balance']);self.assertLess(s['equity'],s['capital'])

class MetricWindowTests(unittest.TestCase):
    @patch('shadow.get')
    def test_future_5m_rows_are_filtered_before_24h_window(self,mock_get):
        signal=1_800_000_000_000
        oi=[{'timestamp':signal-295*300000+i*300000,'sumOpenInterestValue':str(1000+i)} for i in range(301)]
        def fake_get(path,params=None,timeout=15):
            self.assertNotEqual(params.get('limit'),289)
            if path.endswith('openInterestHist'):return oi
            key='buySellRatio' if path.endswith('takerlongshortRatio') else 'longShortRatio'
            return [{'timestamp':signal,key:'1.2'}]
        mock_get.side_effect=fake_get
        f=shadow.metric_features('BTCUSDT',signal)
        self.assertAlmostEqual(f['oi_chg_24h'],(1000+295)/(1000+7)-1)
        self.assertEqual(f['sum_taker_long_short_vol_ratio'],1.2)

class RestartWindowTests(unittest.TestCase):
    @patch('shadow.scan_candidates')
    def test_late_restart_never_backfills_an_hour_old_entry(self,scan):
        with tempfile.TemporaryDirectory() as tmp, patch.object(shadow,'STATE_PATH',Path(tmp)/'state.json'), \
             patch.object(shadow,'now_ms',return_value=4_200_000), patch.object(shadow,'get',return_value=[]):
            shadow.run_once()
            state=shadow.load_state()
        scan.assert_not_called()
        self.assertEqual(state['last_signal_time'],3_600_000)
        self.assertEqual(state['positions'],[])

if __name__=='__main__':unittest.main()
