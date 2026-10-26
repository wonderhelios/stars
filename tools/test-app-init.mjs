// app.js 初始化冒烟测试：整份文件按**真实执行顺序**跑一遍，不能抛错、不能渲染成错误态。
//
// 为什么需要它：
//   `hcMoney` 用 const 声明在 1521 行，而启动块第 1107 行就调 refreshHlCarry()。
//   TDZ 只在"整个文件按顺序执行"时才出现，因此
//     - node --check 看不见（语法合法）
//     - lint-undefined 看不见（名字确实声明过）
//     - 单块切片测试也看不见（切出来的片段里声明顺序是对的）
//   只有真的把 app.js 从头跑一遍才抓得住。
//
// 断言方式：给 fetch 喂一份真实的 /api/hlcarry 载荷，跑完等异步落定，然后要求
//   #hc-metrics 里出现「影子权益」而**不是**「渲染失败」/「读取失败」。
//
// 跑法：node tools/test-app-init.mjs

import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';

const src = readFileSync(new URL('../static/app.js', import.meta.url), 'utf8');

// ---------- DOM 桩 ----------
const elements = new Map();
function stubEl(id = '') {
  const el = {
    id,
    innerHTML: '',
    textContent: '',
    value: '',
    checked: false,
    disabled: false,
    dataset: {},
    style: {},
    children: [],
    childNodes: [],
    classList: {add() {}, remove() {}, toggle() {}, contains: () => false},
    __listeners: {},
    addEventListener(type, fn) {
      (this.__listeners[type] ||= []).push(fn);
    },
    removeEventListener() {},
    __fire(type, ev) {
      const fns = this.__listeners[type] || [];
      let threw = null;
      for (const fn of fns) {
        try {
          const r = fn(ev || {type, preventDefault() {}, stopPropagation() {}, target: this});
          if (r && typeof r.then === 'function') return r.catch((e) => { threw = e; });
        } catch (e) {
          threw = e;
        }
      }
      if (threw) throw threw;
      return Promise.resolve();
    },
    appendChild: (c) => c,
    removeChild: (c) => c,
    append() {},
    prepend() {},
    insertBefore: (n) => n,
    setAttribute() {},
    getAttribute: () => null,
    removeAttribute() {},
    querySelector: () => stubEl(),
    querySelectorAll: () => [],
    getContext: () => ctx2d,
    getBoundingClientRect: () => ({width: 600, height: 300, top: 0, left: 0, right: 600, bottom: 300}),
    focus() {},
    blur() {},
    click() {},
    scrollTo() {},
    closest: () => null,
    matches: () => false,
    offsetWidth: 600,
    offsetHeight: 300,
    clientWidth: 600,
    clientHeight: 300,
    scrollHeight: 300,
    scrollTop: 0,
  };
  return el;
}
// canvas 2d 上下文：任何方法都是空操作，任何属性都能读写
const ctx2d = new Proxy(
  {},
  {
    get: (t, k) => {
      if (k === 'measureText') return () => ({width: 12});
      if (k === 'canvas') return stubEl();
      if (k in t) return t[k];
      return () => {};
    },
    set: (t, k, v) => {
      t[k] = v;
      return true;
    },
  },
);

function el(id) {
  if (!elements.has(id)) elements.set(id, stubEl(id));
  return elements.get(id);
}

const document = {
  getElementById: el,
  querySelector: () => stubEl(),
  querySelectorAll: () => [],
  createElement: () => stubEl(),
  createTextNode: () => stubEl(),
  addEventListener() {},
  removeEventListener() {},
  activeElement: null,
  body: stubEl('body'),
  documentElement: stubEl('html'),
  head: stubEl('head'),
};

// ---------- 网络桩：只为 /api/hlcarry 返回真实结构 ----------
const hlcarryPayload = {
  equity: 1012.34,
  annualized_pct: 21.5,
  warnings: [],
  state: {
    start_equity: 1000,
    budget_usd: 1000,
    basket: [{coin: 'BTC', spot: '@142', weight: 1}],
    legs: [
      {
        coin: 'BTC', spot: '@142', notional: 1000, funding_ann: 10.95, basis_bp: 8.4,
        funding_accrued: 12.5, basis_pnl: 0.4, fee_paid: 1.09, leg_pnl: 11.81,
      },
    ],
    log: [{ts: 1761520000000, msg: '开仓：1 腿，总名义 $1000'}],
    history: [],
    params: {top_k: 5, min_funding_ann_pct: 3, min_spot_volume_usd: 1000000, max_leg_weight: 0.4, payback_days: 30},
    rebalances: [],
    daily: [{date: '2026-10-27', equity_start: 1000, equity_end: 1012.34, funding: 13.5, basis: 0, fees: 1.09, rebalances: 0}],
  },
};

function makeResponse(body) {
  return {
    ok: true,
    status: 200,
    headers: {get: () => null},
    json: async () => body,
    text: async () => JSON.stringify(body),
  };
}

const fetchLog = [];
const fetchStub = async (url, opts) => {
  const u = String(url);
  fetchLog.push({url: u, opts});
  if (u.includes('/api/hlcarry')) return makeResponse(hlcarryPayload);
  return makeResponse({});
};

// ---------- 沙盒 ----------
const ctx = vm.createContext({
  document,
  // app.js 第 22 行就是 window.fetch.bind(window)，所以 window 必须有 fetch
  window: {
    fetch: fetchStub,
    document,
    location: {href: 'http://127.0.0.1:3000/', search: '', hash: '', pathname: '/'},
    localStorage: {getItem: () => null, setItem() {}, removeItem() {}},
    navigator: {userAgent: 'node'},
    __listeners: {},
    addEventListener(type, fn) {
      (this.__listeners[type] ||= []).push(fn);
    },
    removeEventListener() {},
    __fire(type, ev) {
      const fns = this.__listeners[type] || [];
      let threw = null;
      for (const fn of fns) {
        try {
          const r = fn(ev || {type, preventDefault() {}, stopPropagation() {}, target: this});
          if (r && typeof r.then === 'function') return r.catch((e) => { threw = e; });
        } catch (e) {
          threw = e;
        }
      }
      if (threw) throw threw;
      return Promise.resolve();
    },
    matchMedia: () => ({matches: false, addEventListener() {}, removeEventListener() {}}),
    getComputedStyle: () => ({getPropertyValue: () => ''}),
    requestAnimationFrame: () => 0,
    innerWidth: 1440,
    innerHeight: 900,
    devicePixelRatio: 1,
  },
  fetch: fetchStub,
  AbortController: class {
    constructor() {
      this.signal = {aborted: false, addEventListener() {}};
    }
    abort() {
      this.signal.aborted = true;
    }
  },
  setTimeout: (fn) => {
    // 不真的调度，避免测试被定时器拖住；只记录
    return 0;
  },
  clearTimeout() {},
  setInterval: () => 0,
  clearInterval() {},
  requestAnimationFrame: () => 0,
  cancelAnimationFrame() {},
  localStorage: {getItem: () => null, setItem() {}, removeItem() {}},
  getComputedStyle: () => ({getPropertyValue: () => ''}),
  matchMedia: () => ({matches: false, addEventListener() {}, removeEventListener() {}}),
  alert() {},
  prompt: () => null,
  confirm: () => false,
  console,
});

// ---------- 跑整份 app.js ----------
try {
  vm.runInContext(src, ctx, {filename: 'app.js'});
} catch (e) {
  console.error(`✗ app.js 初始化就抛错了：${e.message}`);
  console.error('  这正是 TDZ / 未定义函数会在浏览器里炸掉的位置。');
  process.exit(1);
}

// 让 refreshHlCarry 的 await 落定（fetch 是立即 resolve 的，几轮微任务足够）
for (let i = 0; i < 20; i++) await Promise.resolve();
await new Promise((r) => setImmediate(r));
await new Promise((r) => setImmediate(r));

// ---------- 断言 ----------
const metrics = el('hc-metrics').innerHTML || '';

assert.ok(metrics.length > 0, '初始化后 #hc-metrics 仍是空的：refreshHlCarry 没有跑到渲染');
assert.doesNotMatch(
  metrics,
  /渲染失败|读取失败|ReferenceError|is not defined|before initialization/,
  `初始化渲染报错：${metrics.replace(/<[^>]+>/g, ' ').trim().slice(0, 120)}`,
);
assert.match(metrics, /影子权益/, '#hc-metrics 里没有「影子权益」');
assert.match(metrics, /1,012\.34/, '权益金额未渲染');

const legs = el('hc-legs').innerHTML || '';
assert.match(legs, /BTC/, '持仓表没有渲染出 BTC');

assert.match(el('hc-daily').innerHTML || '', /2026-10-27/, '每日汇总没有渲染');
assert.match(el('hc-rebalances').innerHTML || '', /还没有换过仓/, '换仓卡片没有渲染');

// ---------- 真点一次「保存预算并重新开仓」 ----------
const budgetEl = el('hc-budget');
const openBtn = el('hc-open');
assert.ok(openBtn.__listeners.click && openBtn.__listeners.click.length > 0,
  '#hc-open 上没有任何 click 监听器 —— 按钮点了当然没反应');

// 已经在用页内两步确认，不依赖 window.confirm（浏览器会静默屏蔽）
budgetEl.value = '5000';
const before = fetchLog.length;

// 第一次点击：只进入"待确认"，不发请求
await openBtn.__fire('click');
assert.equal(
  fetchLog.length, before,
  '第一次点击不应直接提交——必须先确认',
);
assert.match(el('hc-budget-status').textContent, /再点一次/, '第一次点击应提示再点一次确认');
assert.match(el('hc-budget-status').textContent, /5,000/, '提示里应写明新预算');
assert.match(openBtn.textContent, /确认/, '按钮文字应变成确认态');

// 第二次点击：真正提交
await openBtn.__fire('click');
const openCall = fetchLog.slice(before).find((c) => c.url.includes('/api/hlcarry/open'));
assert.ok(openCall, '确认后没有请求 /api/hlcarry/open');
assert.equal(JSON.parse(openCall.opts.body).budget_usd, 5000, '提交的预算应为输入框里的 5000');
assert.match(el('hc-budget-status').textContent, /已按/, '提交后应有成功提示');

// 非法值：第一次点击就拦下，且提示可见
budgetEl.value = '0';
const before2 = fetchLog.length;
await openBtn.__fire('click');
assert.equal(fetchLog.length, before2, '非法预算不应发请求');
assert.match(el('hc-budget-status').textContent, /正数/, '非法预算要有可见提示');

console.log('✓ app.js 按真实顺序初始化：渲染正常、无 TDZ、预算按钮两步确认且提交成功');
