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
for (const name of ['strategyHistory', 'equityWindow', 'renderLiveChart']) loadFunction(name);

const now = Date.now();
const firstTs = now - 2 * 3600000;
const badTs = now - 3600000;
const history = [
  { ts: firstTs, equity: 2500.000004 },
  { ts: badTs, equity: 635.087057 },
];
const element = { innerHTML: '', clientWidth: 960 };
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
