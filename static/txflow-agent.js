// No wallet secrets enter this page. EIP-1193 signs a server-issued ApproveAgent only.
(() => {
  const picker = document.getElementById("tx-wallet");
  const prepare = document.getElementById("tx-agent-prepare");
  const approve = document.getElementById("tx-agent-approve");
  const status = document.getElementById("tx-agent-status");
  const providers = new Map();
  let challenge = null;
  let wallet = null;
  let signed = null;
  let busy = false;
  function addWallet(id, name, provider) {
    if (!provider || typeof provider.request !== "function" || providers.has(id)) return;
    if (providers.size === 0) picker.replaceChildren();
    providers.set(id, provider);
    const option = document.createElement("option");
    option.value = id;
    option.textContent = name;
    picker.append(option);
  }
  window.addEventListener("eip6963:announceProvider", (event) => {
    const { info, provider } = event.detail || {};
    if (info?.uuid) addWallet(info.uuid, info.name || "浏览器钱包", provider);
  });
  window.dispatchEvent(new Event("eip6963:requestProvider"));
  if (window.ethereum) addWallet("injected", "默认浏览器钱包", window.ethereum);
  if (!providers.size) picker.options[0].textContent = "未检测到钱包，请在装有钱包的浏览器打开";

  function setBusy(value) {
    busy = value;
    prepare.disabled = value;
    picker.disabled = value;
    approve.disabled = value || !challenge;
  }
  async function post(path, body) {
    const response = await fetch(`/api/txflow/agent/${path}`, {
      method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body), cache: "no-store",
    });
    const result = await response.json();
    if (!response.ok || !result.ok) throw new Error(result.error || `HTTP ${response.status}`);
    return result;
  }
  async function connectedAccount(provider, method) {
    const accounts = await provider.request({ method });
    if (!Array.isArray(accounts) || !/^0x[0-9a-f]{40}$/i.test(accounts[0] || "")) {
      throw new Error("钱包未返回有效账户，请连接你在 TxFlow 使用的主账户。");
    }
    return accounts[0];
  }
  async function chainId(provider) {
    const chain = Number(await provider.request({ method: "eth_chainId" }));
    if (!Number.isSafeInteger(chain) || chain <= 0) throw new Error("无法识别钱包当前网络。");
    return chain;
  }
  prepare.addEventListener("click", async () => {
    if (busy) return;
    setBusy(true);
    challenge = null;
    signed = null;
    approve.textContent = "签名授权 Agent";
    try {
      wallet = providers.get(picker.value);
      if (!wallet) throw new Error("请在装有 MetaMask 等钱包扩展的浏览器打开此页面。");
      status.textContent = "等待连接主钱包…";
      const account = await connectedAccount(wallet, "eth_requestAccounts");
      const current = document.getElementById("lv-account").value.trim();
      if (current && current.toLowerCase() !== account.toLowerCase()) {
        throw new Error(`连接的账户 ${account} 与页面主账户不同。请切换钱包账户，或清空账户字段后重试。`);
      }
      challenge = await post("prepare", { account, signature_chain_id: await chainId(wallet) });
      status.textContent = `主账户：${challenge.account}。专用 Agent：${challenge.agent_address}。密钥已保存在服务器。请在 10 分钟内点击“签名授权 Agent”；钱包签名应为 ApproveAgent，名称 StarsStrategy，Agent 地址与此处一致。`;
    } catch (error) {
      status.textContent = error.message || String(error);
    } finally { setBusy(false); }
  });
  approve.addEventListener("click", async () => {
    if (busy || !challenge) return;
    setBusy(true);
    try {
      if (!signed) {
        if (Date.now() > challenge.expires_at) throw new Error("授权已过期，请重新连接并生成 Agent。");
        const account = await connectedAccount(wallet, "eth_accounts");
        if (account.toLowerCase() !== challenge.account.toLowerCase()) throw new Error("钱包账户已切换，请切回准备授权时的主账户。");
        if (await chainId(wallet) !== challenge.typed_data.domain.chainId) throw new Error("钱包网络已切换，请切回原网络或重新生成 Agent。");
        status.textContent = `等待主钱包签名，授权 Agent ${challenge.agent_address}…`;
        signed = await wallet.request({ method: "eth_signTypedData_v4", params: [account, JSON.stringify(challenge.typed_data)] });
      }
      status.textContent = "正在提交授权并保存配置…";
      const result = await post("approve", { id: challenge.id, signature: signed });
      lvConfigInto({ config: result.config });
      status.textContent = `授权成功。Agent：${result.agent_address}。主账户和密钥路径已保存，实盘与自动调仓均关闭。现在可以生成计划核对账户。`;
      challenge = null;
      signed = null;
      approve.textContent = "已授权";
    } catch (error) {
      status.textContent = error.message || String(error);
      if (signed) approve.textContent = "重试获取授权结果";
    } finally { setBusy(false); }
  });
})();
