import unittest
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

    def test_long_take_profit_and_cooldown(self):
        s=initial_state();process_entries(s,1000,[candidate('BTCUSDT')]);p=s['positions'][0]
        process_exits(s,{'BTCUSDT':p['target_price']})
        self.assertEqual(len(s['positions']),0);self.assertEqual(len(s['trades']),1)
        self.assertEqual(s['trades'][0]['reason'],'止盈');self.assertGreater(s['trades'][0]['pnl_usd'],0)
        self.assertGreater(s['cooldowns']['BTCUSDT'],1000)

    def test_short_stop_and_accounting(self):
        s=initial_state();process_entries(s,1000,[candidate('ETHUSDT',side=-1)]);p=s['positions'][0]
        process_exits(s,{'ETHUSDT':p['stop_price']});snapshot(s,{})
        self.assertEqual(s['trades'][0]['reason'],'止损');self.assertLess(s['trades'][0]['pnl_usd'],0)
        self.assertAlmostEqual(s['equity'],s['balance']);self.assertLess(s['equity'],s['capital'])

if __name__=='__main__':unittest.main()
