import assert from 'node:assert/strict';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const elements=new Map();
const el=id=>{if(!elements.has(id))elements.set(id,{value:'',textContent:'',innerHTML:'',checked:false,disabled:false,addEventListener:(type,fn)=>{el(id)[type]=fn;},reset:()=>{el('tx-points-trader').value='';}});return elements.get(id);};
const week=Date.parse('2026-10-05T00:00:00Z');let wallet='a',calls=[],poll;
const context=vm.createContext({document:{hidden:false,getElementById:el,addEventListener:()=>{}},Date,Number,Set,Error,
 setInterval:fn=>{poll=fn;},fetch:async(url,opts)=>{
 calls.push({url,opts});return {ok:true,json:async()=>opts?.method==='POST'?{ok:true}:{ok:true,account:wallet,week_start:week,entries:[],activity:{weeks:[]}}};
}});
vm.runInContext(readFileSync(new URL('../static/txflow-points.js',import.meta.url),'utf8'),context);
await new Promise(r=>setImmediate(r));
assert.match(el('tx-points-rows').innerHTML,/待登记/);
assert.equal(el('tx-points-week').value,'2026-10-05');
el('tx-points-trader').value='123';el('tx-points-referral').value='4';
await el('tx-points-form').submit({preventDefault(){}});
const body=JSON.parse(calls.find(c=>c.opts?.method==='POST').opts.body);
assert.deepEqual(body,{account:'a',week_start:week,trader_points:123,referral_points:4,final_allocation:false});
assert.ok(calls.every(c=>c.url==='/api/txflow/points'),'never sends orders or changes trading settings');
wallet='b';await poll();assert.equal(el('tx-points-trader').value,'');
assert.match(el('tx-points-summary').textContent,/积分不计入账户收益/);
console.log('✓ Points form saves official values by account/week; account changes clear inputs; no trading endpoint calls');
