// 无浏览器依赖的 ML 曲线回归测试：node tests/test_ml_chart.js
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const source = fs.readFileSync('static/app.js', 'utf8');
function loadFunction(name) {
  const start = source.indexOf(`function ${name}(`);
  assert.ok(start >= 0, `找不到 ${name}`);
  const open = source.indexOf('{', start);
  let depth = 0;
  for (let i = open; i < source.length; i++) {
    if (source[i] === '{') depth++;
    if (source[i] === '}') depth--;
    if (depth === 0) {
      vm.runInThisContext(source.slice(start, i + 1));
      return;
    }
  }
  throw new Error(`${name} 函数未闭合`);
}
for (const name of ['isTransferFlow', 'splitCapitalFlows', 'strategyHistory', 'equityWindow', 'mergeHistory', 'renderLiveChart']) loadFunction(name);

const now = Date.now();
const firstTs = now - 2 * 3600000;
const badTs = now - 3600000;
const history = [
  { ts: firstTs, equity: 2500.000004 },
  { ts: badTs, equity: 635.087057 },
];
const element = { innerHTML: '', clientWidth: 960, querySelector: () => null };
Object.assign(globalThis, {
  currentStrategy: 'ml',
  chartRange: 'day',
  liveFetchedAt: now,
  lastLiveData: {
    account: 'ml-account',
    ml_history: history,
    ml_account: { account: 'ml-account', equity: 2524.780226, error: null },
  },
  $: id => {
    assert.equal(id, 'mo-chart');
    return element;
  },
  ts2m: ts => new Date(ts).toISOString(),
  niceAxis: (lo, hi) => ({ lo, hi, lines: [lo, hi] }),
});

renderLiveChart(history, false);
assert.ok(
  element.innerHTML.includes('<strong>+$24.78</strong>'),
  'ML 曲线末端必须使用当前净值，而不是小时历史中的异常旧值',
);
assert.ok(element.innerHTML.includes('策略盈亏曲线'));
console.log('ML chart current-equity regression: OK (+$24.78)');

// 入金发生在同小时的两个异步采样点之间：旧动量 + 新 ML 不能用 ML 的较晚时间扣款。
const hour = 3600000;
const slot = Math.floor(now / hour) * hour;
const flowTs = slot + 30 * 60000;
const momentum = [
  { ts: slot - 2 * hour + 5 * 60000, equity: 4500 },
  { ts: slot - hour + 5 * 60000, equity: 5000 },
  { ts: slot + 5 * 60000, equity: 5374.47691 },
];
const ml = [
  { ts: momentum[0].ts, equity: 2500.000004 },
  { ts: slot - hour + 6 * 60000, equity: 2500 },
  { ts: slot + 55 * 60000, equity: 1699.659045 },
];
const merged = mergeHistory(momentum, ml);
assert.equal(merged.at(-1).ts, momentum.at(-1).ts, '合并点必须保留动量账本自己的采样时间');
const combined = strategyHistory(merged, {
  account: 'momentum-account',
  capital_baseline: { account: 'momentum-account', equity: 7000.000004 },
  capital_flows: [{ ts: flowTs, amount: 6000 }],
  net_deposit: 6000,
});
const expected = momentum.at(-1).equity + ml.at(-1).equity - 7000.000004;
assert.ok(
  Math.abs(combined.history.at(-1).equity - expected) < 1e-6,
  '入金前的动量采样点不能因 ML 时间较晚而被多扣 $6,000',
);
console.log('Combined chart asynchronous-deposit regression: OK');
