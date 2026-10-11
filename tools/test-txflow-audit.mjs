import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const source = readFileSync(new URL('../static/app.js',import.meta.url),'utf8');
const ctx = vm.createContext({recordIsClosing:()=>false});
vm.runInContext(source.slice(source.indexOf('function parseFill('),source.indexOf('// 旧记录没存入场价')),ctx);
const partial=ctx.parseFill({result:'成交 2e3@4e-6 [requested=4000] [oid=42]',price:4e-6,side:'买'});
assert.equal(partial.kind,'filled'); assert.equal(partial.partial,true); assert.equal(partial.px,4e-6);
assert.equal(ctx.parseFill({result:'Success'}).kind,'other');
assert.equal(ctx.parseFill({result:'成交 4000@0.1 [requested=4000]',price:.1}).partial,false);
assert.equal(ctx.parseFill({result:'TxFlow 下单失败'}).kind,'failed');
console.log('✓ Receipt-only Success is unverified; partial fills use submitted size; scientific prices parse correctly');
const render = vm.createContext({LIVE_EXCHANGE:'TxFlow', txPnl:null, txPnlAt:0,
 metricGroup:(a,b)=>a+b, metric:(a,b)=>a+b, escapeHTML:String,
 ts2m:String,fmtCompact:String,fmt:String});
vm.runInContext(source.slice(source.indexOf('function moneySigned('),source.indexOf('// 轮询后台')),render);
vm.runInContext(source.slice(source.indexOf('function renderTxAudit('),source.indexOf('function renderMonitor(')),render);
assert.match(render.renderTxAudit('a'),/暂不可用/);
render.txPnl={account:'a',audit:{fills:2,matched_fills:0,unassigned_fills:2,known_realized:-2,known_fees_usd:1,incomplete_rows:0,limitation:'历史未核验',rows:[]}};
assert.match(render.renderTxAudit('a'),/尚未闭合/);
assert.match(render.renderTxAudit('a'),/资金费待核验/);
assert.match(render.renderTxAudit('b'),/暂不可用/);
console.log('✓ Audit UI keeps funding and attribution unknown; account changes hide stale evidence');
