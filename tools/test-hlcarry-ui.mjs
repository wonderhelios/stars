// hlcarry 影子 tab 的前端回归测试。
//
// 只用 node:vm，不引入 jsdom：把 app.js 里「HL 单账户 carry 影子」那一段切出来，
// 放进一个**只暴露允许全局**的沙盒执行。任何未声明的标识符都会直接抛
// ReferenceError —— 之前 `money is not defined` 就是靠不住这一点才漏出去的。
//
// 跑法：node tools/test-hlcarry-ui.mjs
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';

const src = readFileSync(new URL('../static/app.js', import.meta.url), 'utf8');
const start = src.indexOf('// ---------- HL 单账户 carry 影子');
assert.ok(start > 0, 'app.js 里找不到 hlcarry 前端块');
const block = src.slice(start);

// ---- 最小 DOM 桩 ----
const els = new Map();
const listeners = [];
const alerts = [];
let confirmAnswer = true;
const confirmMessages = [];
const fetchCalls = [];
function mkEl(id) {
  return {
    id,
    innerHTML: '',
    value: '',
    disabled: false,
    focused: 0,
    focus() {
      this.focused += 1;
    },
    addEventListener(type, fn) {
      listeners.push({id, type, fn});
    },
  };
}
function el(id) {
  if (!els.has(id)) els.set(id, mkEl(id));
  return els.get(id);
}

// ---- 沙盒：只给这些全局。其余一律未定义。 ----
const ctx = vm.createContext({
  $: el,
  fmt: (v, d = 2) => Number(v).toFixed(d),
  metric: (k, v, cls) =>
    `<div class="metric"><div class="k">${k}</div><div class="v ${cls || ''}">${v}</div></div>`,
  moneySigned: (v) => (v >= 0 ? '+' : '−') + '$' + Math.abs(v).toFixed(2),
  escapeHTML: (s) =>
    String(s).replace(/[&<>"]/g, (c) => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;'}[c])),
  fetchJSONRetry: async (url, opts) => {
    fetchCalls.push({url, opts});
    if (String(url).includes('/api/hlcarry/open')) return {ok: true};
    if (String(url).includes('/api/hlcarry')) return lastPayload;
    return {};
  },
  document: {
    getElementById: el,
    querySelectorAll: () => [],
    activeElement: null,
  },
  window: {},
  alert: (m) => alerts.push(String(m)),
  confirm: (m) => {
    confirmMessages.push(String(m));
    return confirmAnswer;
  },
  prompt: () => null,
  console,
});

// 执行整段：任何未定义引用在这里就会炸。
vm.runInContext(block, ctx, {filename: 'app.js#hlcarry'});
assert.equal(typeof ctx.renderHlCarry, 'function', 'renderHlCarry 必须导出到上下文');
assert.equal(typeof ctx.refreshHlCarry, 'function', 'refreshHlCarry 必须导出到上下文');

let lastPayload = null;

// ---- 用例 1：正常持仓 ----
const payload = {
  equity: 1012.34,
  annualized_pct: 21.5,
  warnings: ['XMR 资金费偏低（2.10%/年）'],
  state: {
    start_equity: 1000,
    budget_usd: 1000,
    legs: [
      {
        coin: 'BTC', spot: '@142', notional: 300, funding_ann: 10.95, basis_bp: 8.4,
        funding_accrued: 0.31, basis_pnl: 0.05, fee_paid: 0.33, leg_pnl: 0.03,
      },
      {
        coin: 'XMR', spot: '@260', notional: 200, funding_ann: 41.82, basis_bp: -12.0,
        funding_accrued: 0.95, basis_pnl: -0.2, fee_paid: 0.22, leg_pnl: 0.53,
      },
    ],
    log: [{ts: 1761520000000, msg: '开仓：2 腿，总名义 $1000'}],
  },
};

lastPayload = payload;
ctx.renderHlCarry(payload);

const metrics = el('hc-metrics').innerHTML;
assert.match(metrics, /影子权益/, '汇总缺少「影子权益」');
assert.match(metrics, /1,012\.34/, '影子权益金额未格式化（期望千分位）');
assert.match(metrics, /影子预算/, '汇总缺少「影子预算」——预算是必须可见的');
assert.match(metrics, /1,000\.00/, '预算金额未显示');
assert.match(metrics, /资金费/, '汇总缺少资金费一项');
assert.match(metrics, /基差/, '汇总缺少基差一项');
assert.match(metrics, /21\.5%/, '年化未显示');

const warn = el('hc-warn').innerHTML;
assert.match(warn, /资金费偏低/, '告警未渲染');

const legs = el('hc-legs').innerHTML;
assert.equal(
  (legs.slice(legs.indexOf('<tbody>')).match(/<tr>/g) || []).length,
  2,
  '持仓表数据行应为 2（表头不计）',
);
assert.match(legs, /BTC/, '缺少 BTC 行');
assert.match(legs, /XMR/, '缺少 XMR 行');
assert.match(legs, /41\.82/, 'XMR 资金费未显示');
assert.match(legs, /-12\.0 bp|-12 bp/, '基差未显示');

const log = el('hc-log').innerHTML;
assert.match(log, /开仓/, '事件日志未渲染');

// ---- 用例 2：空仓（未设预算） ----
els.clear();
ctx.renderHlCarry({equity: 0, annualized_pct: null, warnings: [], state: {legs: [], log: []}});
const empty = el('hc-metrics').innerHTML;
assert.match(empty, /还没有影子持仓/, '空仓时缺少引导文案');
assert.match(empty, /按预算重新开仓/, '空仓时应提示如何开仓，而不是自动用默认金额');
assert.equal(el('hc-warn').innerHTML, '', '无告警时不应渲染告警卡片');

// ---- 用例 3：字段缺失不能抛 ----
ctx.renderHlCarry({});
assert.ok(true, '空对象也必须能渲染');

// ---- 用例 4：负数与零的显示 ----
els.clear();
ctx.renderHlCarry({
  equity: 980,
  annualized_pct: -8.3,
  warnings: [],
  state: {
    start_equity: 1000,
    legs: [{coin: 'SOL', spot: '@156', notional: 150, funding_ann: -1.2, basis_bp: -40,
            funding_accrued: -0.5, basis_pnl: -0.1, fee_paid: 0.16, leg_pnl: -0.76}],
    log: [],
  },
});
const neg = el('hc-metrics').innerHTML;
assert.match(neg, /−\$20\.00|-\$20\.00/, '负收益应带负号');
assert.match(neg, /−8\.3%|-8\.3%/, '负年化应带负号');
assert.match(el('hc-legs').innerHTML, /neg/, '负的资金费应标 red 类');

// ---- 用例 5：按钮已接线 ----
const wired = new Set(listeners.map((l) => l.id));
assert.ok(wired.has('hc-refresh'), '#hc-refresh 未绑定事件');
assert.ok(wired.has('hc-step'), '#hc-step 未绑定事件');
assert.ok(wired.has('hc-open'), '#hc-open 未绑定事件');

// ---- 用例 6：预算可手动调整 ----
// 先用「有预算」的载荷渲染，确认输入框被回填
lastPayload = payload;
ctx.renderHlCarry(payload);
const budgetEl = el('hc-budget');
assert.equal(budgetEl.value, '1000', '预算输入框应回填当前预算');

const openBtn = listeners.find((l) => l.id === 'hc-open');
assert.ok(openBtn, '#hc-open 未绑定');
const callsBefore = fetchCalls.length;

// 空 / 负数 / 超上限：都不许发请求，并聚焦输入框
for (const bad of ['', '-5', '0', '99999999']) {
  budgetEl.value = bad;
  alerts.length = 0;
  budgetEl.focused = 0;
  await openBtn.fn();
  assert.equal(fetchCalls.length, callsBefore, `预算 "${bad}" 不应发起请求`);
  assert.ok(alerts.length > 0, `预算 "${bad}" 应有提示`);
  assert.ok(budgetEl.focused > 0, `预算 "${bad}" 应聚焦输入框`);
}

// 合法预算：取消确认则不提交，确认后才提交，且金额正确
budgetEl.value = '2500';
confirmAnswer = false;
confirmMessages.length = 0;
await openBtn.fn();
assert.equal(fetchCalls.length, callsBefore, '取消确认后不应提交');
assert.match(confirmMessages[0] || '', /清空/, '确认文案必须说明会清空持仓与曲线');

confirmAnswer = true;
const beforeCalls = fetchCalls.length;
await openBtn.fn();
// 提交后会再刷一次 /api/hlcarry，所以要在新增的调用里找 open 那一条
const openCall = fetchCalls.slice(beforeCalls).find((c) => String(c.url).includes('/api/hlcarry/open'));
assert.ok(openCall, '应调用 /api/hlcarry/open');
assert.equal(JSON.parse(openCall.opts.body).budget_usd, 2500, '提交的预算应为输入值');
assert.ok(
  fetchCalls.slice(beforeCalls).some((c) => String(c.url) === '/api/hlcarry'),
  '提交后应重新拉取影子状态',
);

console.log('✓ hlcarry 影子 tab：无未定义引用；四种状态渲染正确；预算可手动调整（回填/校验/确认/提交）');
