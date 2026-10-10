import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const source = readFileSync(new URL("../static/app.js", import.meta.url), "utf8");
const start = source.indexOf("let txPnl = null;");
const end = source.indexOf("function renderMonitor(d)", start);
const snippet = source.slice(start, end).replace("setupCapitalFlow();", "").replace("setupCapitalTime();", "");
const base = { account: "A", equity: 569.09, capital_baseline: { account: "a", equity: 500, ts: 1 } };
const context = vm.createContext({ LIVE_EXCHANGE: "Hyperliquid", fetch: () => { throw Error("HL must not request TxFlow PnL"); } });
vm.runInContext(snippet, context);
let p = context.capitalPerformance(base);
assert.equal(p.valid, true);
assert.ok(Math.abs(p.change - 69.09) < 1e-8);
assert.ok(Math.abs(p.percent - 13.818) < 1e-8);
assert.equal(context.capitalPerformance({ ...base, equity: 0 }).percent, -100);
assert.equal(context.capitalPerformance({ ...base, equity: 450 }).change, -50);
for (const bad of [{ ...base, capital_baseline: null }, { ...base, error: "offline" },
  { ...base, equity: NaN }, { ...base, account: "B" }, { ...base, capital_baseline: {account:"a",equity:0} }]) {
  assert.equal(context.capitalPerformance(bad).valid, false);
}
await context.fetchTxPnl("a");

// Exercise the same prefix rewrite used by the actual TxFlow route.
const rewritten = snippet.replaceAll("/api/", "/api/txflow/");
let calls = [], fails = false, account = "a";
let now = 1_000_000;
const tx = vm.createContext({ LIVE_EXCHANGE: "TxFlow", Date: { now: () => now }, fetch: async url => {
  calls.push(url);
  if (fails) throw Error("network unavailable");
  return { ok: true, json: async () => ({ok:true, account, realized:3.06, fees:1, fills:2}) };
}});
vm.runInContext(rewritten, tx);
await tx.fetchTxPnl("a");
assert.deepEqual(calls, ["/api/txflow/pnl"]);
await tx.fetchTxPnl("a"); assert.equal(calls.length, 1);
account = "b"; await tx.fetchTxPnl("b"); assert.equal(calls.length, 2);
assert.equal(vm.runInContext("txPnl.account", tx), "b");
now += 60001; fails = true; await tx.fetchTxPnl("b");
assert.equal(vm.runInContext("txPnl", tx), null);

// Render the complete monitor with an empty position list, without a browser or exchange.
const elements = new Map();
const get = id => { if (!elements.has(id)) elements.set(id, {innerHTML:"",textContent:""}); return elements.get(id); };
const renderContext = vm.createContext({ ...context, $:get, LIVE_EXCHANGE:"Hyperliquid", liveFetchedAt:0,
  sideLabel:() => "", fmtCompact:v => String(v), pager:() => "",
  lastLiveData:null, recordScope:"all", chartRange:"week", LIVE_PAGE_SIZE:20, posPage:0, livePage:0, orderPage:0, entryHints:() => [],
  chronRecords:() => ({realized:9.03, slip:null, n:1, records:[]}), slippageStats:() => null,
  ts2m:() => "10-09 00:00", fmt:(v,n) => Number(v || 0).toFixed(n), pct:v => `${v >= 0 ? "+" : ""}${v.toFixed(2)}%`,
  renderLiveChart:() => {},
});
vm.runInContext(source.slice(source.indexOf("function metric("), source.indexOf("// 轮询后台")), renderContext);
vm.runInContext(source.slice(start, source.indexOf("function renderLiveChart(")).replace("setupCapitalFlow();", "").replace("setupCapitalTime();", ""), renderContext);
renderContext.renderMonitor({ ...base, positions:[], records:[], config:{}, history:[] });
const html = get("mo-metrics").innerHTML;
assert.match(html, /\+\$69\.09/);
assert.match(html, /\+13\.82%/);
assert.match(html, /记录起点资金/);
assert.match(html, /净投入收益率/);
assert.doesNotMatch(html, /真实总盈亏|>收益率</);
console.log("✓ Account changes use persisted equity; unavailable baselines stay unknown; TxFlow route and account cache are isolated");

// Live PNL is current positions only, including losses/flat/unknown states.
for (const [positions, expected] of [[[{unrealized:12}, {unrealized:-5}], '+$7.00'], [[{unrealized:-4}], '−$4.00'], [[], '+$0.00']]) {
  renderContext.renderMonitor({...base, positions, records:[], config:{}, history:[]});
  assert.match(get('mo-metrics').innerHTML, new RegExp(expected.replace(/[+$]/g, '\\$&')));
}
renderContext.renderMonitor({...base, error:'offline', positions:[], records:[], config:{}, history:[]});
assert.match(get('mo-status').innerHTML, /账户数据不可用/);
assert.doesNotMatch(get('mo-metrics').innerHTML, /\+\$0\.00/);
assert.equal(renderContext.recordAction({tid:1,action:'止盈/被动成交'}), '交易所补录');
assert.equal(renderContext.recordPositionSide({side:'买',reduce_only:true}), 'short');
assert.equal(renderContext.recordPositionSide({side:'卖',reduce_only:true}), 'long');
const hour=3600000, endTime=2000*hour;
const history=Array.from({length:10001}, (_,i)=>({ts:endTime-(10000-i)*hour/12,equity:500+i/100}));
for (const [range,hours] of [['day',24],['week',168],['month',720],['all',Infinity]]) {
  const {raw,points}=renderContext.equityWindow([...history,{ts:endTime+1,equity:1},{ts:endTime,equity:600}],range,endTime);
  assert.ok(raw.every(p=>p.ts>=endTime-hours*hour && p.ts<=endTime));
  assert.equal(points[0].ts,raw[0].ts);
  assert.equal(points.at(-1).equity,600);
  assert.ok(points.length<=183);
}
assert.equal(renderContext.equityWindow([{ts:1,equity:1},{ts:100,equity:2}], 'all',100).points.length,2);
vm.runInContext(source.slice(source.indexOf('function chronRecords('), source.indexOf('function slippageStats(')), renderContext);
assert.equal(renderContext.chronRecords({records:[{tid:1,pnl:3},{tid:1,pnl:3},{tid:2,pnl:-1}]}).realized,2);
console.log('✓ Current PNL, unavailable accounts, neutral fill labels, close direction, bounded chart windows and receipt deduplication');
vm.runInContext(source.slice(source.indexOf('function parseFill('),source.indexOf('// 旧记录没存入场价')),renderContext);
const partial={side:'卖', action:'平仓', reduce_only:true, size:10, price:12, entry_px:10, result:'成交 2@12'};
assert.equal(renderContext.parseFill(partial).pnl,4);
assert.equal(renderContext.parseFill({...partial,reduce_only:false,action:'开仓'}).pnl,null);

const deposited = context.capitalPerformance({account:'A',equity:3084.63,net_deposit:2498.4,capital_baseline:{account:'a',equity:541.02}});
assert.ok(Math.abs(deposited.principal-3039.42)<1e-8);
assert.ok(Math.abs(deposited.change-45.21)<1e-8);
assert.ok(Math.abs(deposited.percent-1.48745484)<1e-7);
for (const amount of [-500,-600]) {
  const result=context.capitalPerformance({...base,net_deposit:amount});
  assert.equal(result.valid,true); assert.equal(result.percent,null);
}
assert.equal(context.capitalPerformance({...base,net_deposit:NaN}).valid,false);
assert.equal(context.capitalPerformance({...base,net_deposit:Infinity}).valid,false);
assert.ok(Math.abs(context.capitalPerformance({...base,net_deposit:-100}).percent-42.2725)<1e-8);
console.log('✓ Deposits/withdrawals adjust principal and simple return, nonpositive principal stays undefined');
