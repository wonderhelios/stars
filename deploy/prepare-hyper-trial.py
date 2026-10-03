#!/usr/bin/env python3
"""Replay and save a candidate on the same server. Never activates or resumes live trading."""
import argparse
import json
import sys
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import Request, urlopen


def request(base, path, body=None):
    req = Request(base.rstrip('/') + path, data=None if body is None else json.dumps(body).encode(), headers={'Content-Type':'application/json'})
    try:
        with urlopen(req, timeout=180) as response:
            return json.load(response)
    except HTTPError as error:
        raise RuntimeError(error.read().decode(errors='replace')) from error


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--research-url', default='http://127.0.0.1:3000')
    args = parser.parse_args()
    strategy = json.loads(Path(__file__).with_name('hyper-trial-all8.json').read_text())
    handoff = request(args.research_url, '/api/research/handoff')
    if handoff.get('compatible_rules') is not True:
        raise RuntimeError('两项目规则不一致或服务未升级，请先部署并重启两个项目')
    cycles = request(args.research_url, '/api/research/cycles')
    result = request(args.research_url, '/api/research/execution', {'strategy':strategy,'capital':500,'cycle':cycles['active_cycle']})
    saved = request(args.research_url, '/api/research/strategies', result['strategy'])
    if not saved.get('saved'):
        raise RuntimeError(json.dumps(saved, ensure_ascii=False))
    report = result['report']
    print('已回放并保存到同服务器 Hyper Fly 策略库；没有切换或恢复实盘。')
    print('候选：' + strategy['name'])
    print('模拟收益 %.2f%%；最大回撤 %.2f%%' % (report['return_pct'], report['max_drawdown_pct']))
    print('回放状态：' + ('通过' if report['deployable'] else '未通过'))
    for reason in report.get('blockers', []):
        print('  - ' + reason)
    print('在 Hyper Fly 暂停新开仓，空仓后选择此候选，确认人工试运行并切换；切换后保持暂停。')


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print('准备失败：' + str(error), file=sys.stderr)
        sys.exit(1)
