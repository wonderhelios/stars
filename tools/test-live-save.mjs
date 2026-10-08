import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const source = readFileSync(new URL("../static/app.js", import.meta.url), "utf8");
const start = source.indexOf('$("lv-save").addEventListener');
const end = source.indexOf("async function runLive", start);
assert.ok(start >= 0 && end > start);
const elements = new Map();
const get = id => {
  if (!elements.has(id)) elements.set(id, { value: "", textContent: "", disabled: false, addEventListener(_, fn) { this.click = fn; } });
  return elements.get(id);
};
get("lv-minvol").value = "1000000";
get("lv-armed").value = "false";
get("lv-auto").value = "false";
let response = { ok: false, status: 409, json: async () => ({ ok: false, error: "订单操作正在执行，请稍后保存配置" }) };
let sent;
vm.runInNewContext(source.slice(start, end), {
  $: get, fetch: async (_, options) => { sent = JSON.parse(options.body); return response; }, setTimeout() {},
});
await get("lv-save").click();
assert.equal(get("lv-save-status").textContent, "订单操作正在执行，请稍后保存配置");
assert.equal(get("lv-save").disabled, false);
assert.equal(sent.min_vol_usd, 1000000);
assert.equal(sent.armed, false);
assert.equal(sent.auto_run, false);
response = { ok: true, json: async () => ({ ok: true }) };
await get("lv-save").click();
assert.equal(get("lv-save-status").textContent, "配置已保存。");
assert.equal(get("lv-save").disabled, false);
console.log("✓ Config saving displays the server error, preserves settings, and allows retry");
