// Exercise the wallet flow without browser extensions, private keys, or real submissions.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const source = readFileSync(new URL("../static/txflow-agent.js", import.meta.url), "utf8");
const html = readFileSync(new URL("../static/txflow.html", import.meta.url), "utf8");
for (const [, id] of source.matchAll(/getElementById\("([\w-]+)"\)/g)) {
  assert.ok(html.includes(`id="${id}"`), `Missing element ${id}`);
}
const account = "0x1111111111111111111111111111111111111111";
const agent = "0x2222222222222222222222222222222222222222";
function harness(hasWallet = true) {
  const elements = new Map();
  for (const id of ["tx-wallet", "tx-agent-prepare", "tx-agent-approve", "tx-agent-status", "lv-account"]) {
    elements.set(id, {
      value: "", disabled: id === "tx-agent-approve", textContent: "", options: [{ textContent: "" }], listeners: {},
      addEventListener(name, fn) { this.listeners[name] = fn; },
      replaceChildren() { this.options = []; },
      append(option) { this.options.push(option); if (!this.value) this.value = option.value; },
    });
  }
  const state = { account, chain: "0xa4b1", rejectSign: false, signCount: 0, calls: [], saved: null, failSave: false };
  const provider = { async request({ method, params }) {
    if (method === "eth_requestAccounts" || method === "eth_accounts") return [state.account];
    if (method === "eth_chainId") return state.chain;
    if (method === "eth_signTypedData_v4") {
      state.signCount++;
      const data = JSON.parse(params[1]);
      assert.equal(params[0], account);
      assert.equal(data.primaryType, "ApproveAgent");
      assert.equal(data.message.agentAddress, agent);
      if (state.rejectSign) throw new Error("用户拒绝签名");
      return "0xTEST_SIGNATURE";
    }
    throw new Error(`Unexpected RPC ${method}`);
  } };
  const challenge = {
    ok: true, id: "test-id", account, agent_address: agent, expires_at: Date.now() + 600000,
    typed_data: { domain: { chainId: 42161 }, primaryType: "ApproveAgent", message: { agentAddress: agent } },
  };
  const context = {
    document: { getElementById: id => elements.get(id), createElement: () => ({}) },
    window: { ethereum: hasWallet ? provider : null, addEventListener() {}, dispatchEvent() {} },
    Event: class {},
    fetch: async (url, options) => {
      const body = JSON.parse(options.body);
      state.calls.push({ url, body });
      if (url.endsWith("prepare")) return { ok: true, json: async () => challenge };
      assert.deepEqual(body, { id: "test-id", signature: "0xTEST_SIGNATURE" });
      if (state.failSave) return { ok: false, json: async () => ({ ok: false, error: "保存配置失败" }) };
      return { ok: true, json: async () => ({ ok: true, agent_address: agent, config: { account, key_path: "/server/agent.key", armed: false, auto_run: false } }) };
    },
    lvConfigInto: ({ config }) => { state.saved = config; },
  };
  vm.runInNewContext(source, context);
  return { state, elements, click: id => elements.get(id).listeners.click() };
}

const ready = harness();
await ready.click("tx-agent-prepare");
assert.equal(ready.elements.get("tx-agent-approve").disabled, false);
assert.deepEqual(ready.state.calls[0].body, { account, signature_chain_id: 42161 });
await ready.click("tx-agent-approve");
assert.equal(ready.state.signCount, 1);
assert.equal(ready.state.saved.armed, false);
assert.equal(ready.state.saved.auto_run, false);
assert.equal(ready.elements.get("tx-agent-approve").disabled, true);
assert.match(ready.elements.get("tx-agent-status").textContent, /授权成功/);

const retry = harness();
await retry.click("tx-agent-prepare");
retry.state.rejectSign = true;
await retry.click("tx-agent-approve");
assert.equal(retry.state.calls.length, 1); // Refused signature is never submitted.
retry.state.rejectSign = false;
retry.state.failSave = true;
await retry.click("tx-agent-approve");
assert.equal(retry.state.signCount, 2);
retry.state.failSave = false;
await retry.click("tx-agent-approve");
assert.equal(retry.state.signCount, 2); // Retrying a receipt/save reuses the exact signature.
assert.ok(retry.state.saved);

const switched = harness();
await switched.click("tx-agent-prepare");
switched.state.account = agent;
await switched.click("tx-agent-approve");
assert.equal(switched.state.signCount, 0);
assert.equal(switched.state.calls.length, 1);
assert.match(switched.elements.get("tx-agent-status").textContent, /账户已切换/);
switched.state.account = account;
switched.state.chain = "0x1";
await switched.click("tx-agent-approve");
assert.equal(switched.state.signCount, 0);
assert.match(switched.elements.get("tx-agent-status").textContent, /网络已切换/);

const mismatch = harness();
mismatch.elements.get("lv-account").value = agent;
await mismatch.click("tx-agent-prepare");
assert.equal(mismatch.state.calls.length, 0);
assert.equal(mismatch.elements.get("tx-agent-approve").disabled, true);

const absent = harness(false);
await absent.click("tx-agent-prepare");
assert.equal(absent.state.calls.length, 0);
assert.match(absent.elements.get("tx-agent-status").textContent, /钱包扩展/);
console.log("✓ TxFlow wallet flow: authorization, rejection, retry, account/network changes, and missing wallet");
