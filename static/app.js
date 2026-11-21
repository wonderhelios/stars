// stars · 动量 alpha 研究台 — 前端逻辑
const $ = (id) => document.getElementById(id);
const LIVE_EXCHANGE = "Hyperliquid";

// ---------- 工具 ----------
const pct = (v, d = 2) => `${(v >= 0 ? "+" : "")}${v.toFixed(d)}%`;
const signed = (v, d = 2) => `${(v >= 0 ? "+" : "")}${v.toFixed(d)}`;
const fmt = (v, d = 2) => v.toFixed(d);
// 价格和数量最多显示 6 位小数，去掉小数末尾的补零。
const fmtCompact = (v) => v.toFixed(6).replace(/(\.\d*?[1-9])0+$|\.0+$/, "$1");
const ts2d = (ms) => new Date(ms).toISOString().slice(0, 10);
// 短时间戳（MM-DD HH:MM），用于指标卡避免换行
const ts2m = (ms) => {
  const iso = new Date(ms).toISOString();
  return iso.slice(5, 10) + " " + iso.slice(11, 16);
};
const ts2bj = (ms) => {
  const iso = new Date(ms + 8 * 3600000).toISOString();
  return iso.slice(5, 10) + " " + iso.slice(11, 16);
};

// ---------- 鉴权（STARS_TOKEN ✗） ----------
const AUTH_KEY = "stars_token";
const authToken = () => { try { return localStorage.getItem(AUTH_KEY) || ""; } catch (_) { return ""; } };
const setAuthToken = (t) => { try { localStorage.setItem(AUTH_KEY, t); } catch (_) {} };
const rawFetch = window.fetch.bind(window);
window.fetch = function (input, init) {
  const url = typeof input === "string" ? input : (input && input.url) || "";
  if (url.indexOf("/api/") !== -1) {
    init = init || {};
    const headers = new Headers(init.headers || (input && input.headers) || {});
    if (!headers.has("Authorization") && authToken()) headers.set("Authorization", "Bearer " + authToken());
    init.headers = headers;
  }
  return rawFetch(input, init).then((res) => {
    if (res.status === 401) showAuth();
    return res;
  });
};
function showAuth(msg) {
  const el = $("auth-overlay");
  if (!el) return;
  el.hidden = false;
  const st = $("auth-status");
  if (st) st.textContent = msg || "令牌无效或未登录";
}
document.addEventListener("DOMContentLoaded", () => {
  const btn = $("auth-submit");
  if (btn) btn.addEventListener("click", () => {
    const v = (($("auth-token") || {}).value || "").trim();
    if (!v) return;
    setAuthToken(v);
    location.reload();
  });
});

// ---------- 导航 ----------
function openWorkspaceView(view) {
  const destination = $("view-" + view);
  if (!destination) return;
  const parent = view === "analysis" ? "monitor" : view;
  document.querySelectorAll(".nav button").forEach(button => {
    const selected = button.dataset.view === parent;
    button.classList.toggle("active", selected);
    if (selected) button.setAttribute("aria-current", "page");
    else button.removeAttribute("aria-current");
  });
  document.querySelectorAll(".view").forEach(section => section.classList.toggle("active", section === destination));
  window.scrollTo({top:0,behavior:"instant"});
}
document.querySelectorAll(".nav button").forEach(button => button.addEventListener("click", () => openWorkspaceView(button.dataset.view)));
document.querySelectorAll("[data-open-view]").forEach(button => button.addEventListener("click", () => openWorkspaceView(button.dataset.openView)));

// ---------- 状态轮询 ----------
async function refreshStatus() {
  try {
    const res = await fetch("/api/status", { cache: "no-store" });
    if (!res.ok) throw new Error("HTTP " + res.status);
    const d = await res.json();
    const led = $("led");
    const refresh = d.refresh || {};
    if (refresh.phase === "backfill") {
      led.className = "led warn";
      $("statusText").textContent =
        `回填数据 ${refresh.coins_done}/${refresh.coins_total} · ${refresh.current || ""}`;
    } else if ((refresh.phase || "").startsWith("error:")) {
      led.className = "led warn";
      $("statusText").textContent = "行情更新失败 · " + refresh.phase.slice(6);
    } else if (d.cached_coins > 0) {
      led.className = "led";
      $("statusText").textContent =
        `已缓存 ${d.cached_coins} 币 · 流动性 ${d.universe_liquid}`;
    } else {
      led.className = "led off";
      $("statusText").textContent = "等待数据";
    }
  } catch (e) {
    $("led").className = "led off";
    $("statusText").textContent = "连接中断";
  }
}


// ---------- 纸交易 ----------

// ---------- 渲染工具 ----------
function niceAxis(lo, hi, ticks) {
  if (!(hi > lo)) {
    const step = Math.max(1, Math.round(Math.abs(lo) * 0.01));
    return { lo: lo - step, hi: lo + step, lines: [lo - step, lo, lo + step] };
  }
  const raw = (hi - lo) / (ticks || 5);
  const mag = Math.pow(10, Math.floor(Math.log10(raw)));
  const norm = raw / mag;
  const step = (norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 5 ? 5 : 10) * mag;
  const nlo = Math.floor(lo / step) * step;
  const nhi = Math.ceil(hi / step) * step;
  const lines = [];
  for (let v = nlo; v <= nhi + step * 0.001; v += step) lines.push(v);
  return { lo: nlo, hi: nhi, lines };
}

function metric(k, v, cls) {
  return `<div class="metric"><div class="k">${k}</div><div class="v ${cls || ""}">${v}</div></div>`;
}

function metricGroup(title, html, cls, note) {
  return `<div class="metric-group${cls ? " " + cls : ""}"><div class="g-title">${title}</div><div class="metrics">${html}</div>${note || ""}</div>`;
}

function moneySigned(v) {
  return (v >= 0 ? "+" : "−") + "$" + fmt(Math.abs(v), 2);
}

// 轮询后台调仓结果（每 3 秒一次，最多 tries 次）
async function pollPlanOut(tries) {
  for (let i = 0; i < tries; i++) {
    await new Promise((r) => setTimeout(r, 3000));
    try {
      const res = await fetch("/api/live", { cache: "no-store" });
      const d = await res.json();
      const plan = d.last_plan || [];
      const busy = plan.length === 1 && plan[0].includes("执行中");
      if (!busy && plan.length) {
        $("lv-plan-out").innerHTML = `<pre class="logbox">${plan.join("\n")}</pre>`;
        refreshMonitor(true);
        return;
      }
    } catch (e) { /* 网络抖动，继续轮询 */ }
  }
  $("lv-plan-out").innerHTML =
    '<pre class="logbox neg">等待超时。请刷新页面，到「实盘监控」核对持仓数与止盈挂单数。</pre>';
}

function pager(key, total, page, size) {
  const per = size || LIVE_PAGE_SIZE;
  const pages = Math.max(1, Math.ceil(total / per));
  const p = Math.min(Math.max(0, page), pages - 1);
  return `<div class="pager">
    <button class="btn ghost" data-pager="${key}" data-to="${p - 1}" ${p <= 0 ? "disabled" : ""}>← 上一页</button>
    <span class="info">第 ${p + 1} / ${pages} 页 · 共 ${total} 条</span>
    <button class="btn ghost" data-pager="${key}" data-to="${p + 1}" ${p >= pages - 1 ? "disabled" : ""}>下一页 →</button>
  </div>`;
}

function sideLabel(s, closing = false) {
  const prefix = closing ? "平" : "";
  return s === "long"
    ? `<span class="badge direction-label ok">${prefix}多</span>`
    : `<span class="badge direction-label neg-badge">${prefix}空</span>`;
}

function orderSideLabel(s) {
  const cls = s === "买" ? "ok" : s === "卖" ? "neg-badge" : "todo";
  return `<span class="badge direction-label ${cls}">${s}</span>`;
}



// ---------- 分页 ----------
const LIVE_PAGE_SIZE = 10;
const POS_PAGE_SIZE = 10;
const FLOW_PAGE_SIZE = 5;
const CB_PAGE_SIZE = 8;
const TF_TRADE_PAGE_SIZE = 12;
let flowPage = 0;
let cbPosPage = 0;
let cbSigPage = 0;
let cbTradePage = 0;
let tfTradePage = 0;

// 通用分页条；点击由全局委托处理

document.addEventListener("click", (e) => {
  const btn = e.target.closest("[data-pager]");
  if (!btn) return;
  const key = btn.dataset.pager;
  const to = Number(btn.dataset.to);
  if (!Number.isFinite(to)) return;
  if (key === "live") {
    livePage = Math.max(0, to);
    renderMonitor(lastLiveData);
  } else if (key === "pos") {
    posPage = Math.max(0, to);
    renderMonitor(lastLiveData);
  } else if (key === "order") {
    orderPage = Math.max(0, to);
    renderMonitor(lastLiveData);
  } else if (key === "flow") {
    flowPage = Math.max(0, to);
    if (lastLiveData) renderCapitalFlows(lastLiveData);
  } else if (key === "cbpos") {
    cbPosPage = Math.max(0, to);
    if (lastCombineData) renderIntraday(lastCombineData);
  } else if (key === "cbsig") {
    cbSigPage = Math.max(0, to);
    if (lastCombineData) renderIntraday(lastCombineData);
  } else if (key === "cbtr") {
    cbTradePage = Math.max(0, to);
    if (lastCombineData) renderIntraday(lastCombineData);
  } else if (key === "tftr") {
    tfTradePage = Math.max(0, to);
    if (lastTradFiData) renderTradFi(lastTradFiData);
  }
});

// 每日盈亏：从净值曲线上取相邻两点的差

// 持有期分析：赚的钱来自长仓还是短仓

// 选一组好看的坐标轴刻度

// ==================== 实盘设置 ====================
let liveLoaded = false;
let liveBusy = false;
// 这些缓存必须在这里声明 ✗ 之前放在文件后面时 ✗
// 早期调用的 renderMonitor 会踩到 TDZ（Cannot access before initialization）✗
let lastCombineData = null;
let lastTradFiData = null;
let cbRange = "all";
let tfRange = "all";
let cbLeg = "mom";
let lastHealth = null;
let liveFetchedAt = 0;

function lvConfigInto(d) {
  const c = d.config || {};
  $("lv-account").value = c.account || "";
  $("lv-key").value = c.key_path || "";
  if ($("lv-vault")) $("lv-vault").value = c.vault_address || "";
  if ($("lv-ml-vault")) $("lv-ml-vault").value = c.ml_vault_address || "";
  if ($("lv-ml-python")) $("lv-ml-python").value = c.ml_python || "";
  if ($("lv-ml-dir")) $("lv-ml-dir").value = c.ml_dir || "";
  $("lv-positions").value = c.target_positions ?? 8;
  $("lv-leverage").value = c.leverage ?? 3;
  $("lv-buffer").value = Math.round((c.margin_buffer ?? 0.9) * 100);
  $("lv-slippage").value = ((c.slippage ?? 0.005) * 100).toFixed(2);
  $("lv-slices").value = c.rebalance_slices ?? 3;
  $("lv-tp").value = Math.round((c.take_profit_pct ?? 0.1) * 100);
  if ($("lv-momw")) $("lv-momw").value = Math.round((c.momentum_weight ?? 0.5) * 100);
  if ($("lv-ddw")) $("lv-ddw").value = c.dd_weight ?? 0;
  if ($("lv-upw")) $("lv-upw").value = c.upfrac_weight ?? 0;
  if ($("lv-voltarget")) $("lv-voltarget").value = Math.round((c.vol_target ?? 0) * 1000) / 10;
  if ($("lv-stop")) $("lv-stop").value = Math.round((c.position_stop_pct ?? 0.25) * 100);
  if ($("lv-cool")) $("lv-cool").value = c.stop_cooldown_days ?? 5;
  $("lv-lookback").value = c.lookback ?? 14;
  $("lv-top").value = c.top_frac ?? 0.2;
  $("lv-minvol").value = String(c.min_vol_usd ?? 5000000);
  $("lv-minpct").value = String((c.min_order_pct ?? 0) * 100);
  $("lv-armed").value = String(!!c.armed);
  $("lv-auto").value = String(!!c.auto_run);
}

async function fetchLive() {
  const res = await fetch("/api/live", { cache: "no-store" });
  if (!res.ok) throw new Error("HTTP " + res.status);
  return res.json();
}

$("lv-save").addEventListener("click", async () => {
  const body = {
    account: $("lv-account").value,
    key_path: $("lv-key").value,
    vault_address: $("lv-vault") ? $("lv-vault").value : "",
    ml_vault_address: $("lv-ml-vault") ? $("lv-ml-vault").value : "",
    ml_python: $("lv-ml-python") ? $("lv-ml-python").value : "",
    ml_dir: $("lv-ml-dir") ? $("lv-ml-dir").value : "",
    target_positions: Number($("lv-positions").value),
    leverage: Number($("lv-leverage").value),
    margin_buffer: Number($("lv-buffer").value) / 100,
    slippage: Number($("lv-slippage").value) / 100,
    rebalance_slices: Number($("lv-slices").value),
    take_profit_pct: Number($("lv-tp").value) / 100,
    momentum_weight: ($("lv-momw") ? Number($("lv-momw").value) : 50) / 100,
    dd_weight: $("lv-ddw") ? (Number($("lv-ddw").value) || 0) : 0,
    upfrac_weight: $("lv-upw") ? (Number($("lv-upw").value) || 0) : 0,
    vol_target: ($("lv-voltarget") ? Number($("lv-voltarget").value) : 0) / 100,
    position_stop_pct: ($("lv-stop") ? Number($("lv-stop").value) : 25) / 100,
    stop_cooldown_days: Number($("lv-cool") ? $("lv-cool").value : 5) || 0,
    lookback: Number($("lv-lookback").value),
    top_frac: Number($("lv-top").value),
    min_vol_usd: Number($("lv-minvol").value),
    min_order_pct: Number($("lv-minpct").value) / 100,
    armed: $("lv-armed").value === "true",
    auto_run: $("lv-auto").value === "true",
  };
  $("lv-save").textContent = "保存中…";
  const saveStatus = $("lv-save-status");
  saveStatus.textContent = "";
  $("lv-save").disabled = true;
  try {
    const res = await fetch("/api/live/config", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    const d = await res.json();
    if (!res.ok || !d.ok) throw new Error(d.error || "保存失败");
    $("lv-save").textContent = "已保存 ✓";
    saveStatus.textContent = "配置已保存。";
  } catch (e) {
    $("lv-save").textContent = "保存失败";
    saveStatus.textContent = e.message || String(e);
  } finally {
    $("lv-save").disabled = false;
  }
  setTimeout(() => ($("lv-save").textContent = "保存配置"), 1500);
});

async function runLive(live) {
  if (live && $("lv-armed").value !== "true") {
    alert("请先把「启用实盘」设为开启并保存配置，否则不会发送任何订单。");
    return;
  }
  if (live && !confirm(`确认执行真实调仓？会对 ${LIVE_EXCHANGE} 账户发送真实订单。`)) return;
  liveBusy = true;
  const btn = live ? $("lv-run") : $("lv-plan");
  const label = btn.textContent;
  btn.textContent = live ? "执行中…" : "计算中…";
  btn.disabled = true;
  try {
    const res = await fetch("/api/live/run", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ live }),
    });
    const d = await res.json();
    if (!d.ok) throw new Error(d.error || "失败");
    // 真实调仓在后台任务里跑（浏览器断开也不会打断它），这里改为轮询结果。
    if (d.started) {
      $("lv-plan-out").innerHTML =
        '<pre class="logbox">已开始执行，等待结果…\n（执行在服务器后台进行，关掉页面也不会中断）</pre>';
      await pollPlanOut(90);
    } else {
      const r = d.result;
      const extra = r.executed && r.executed.length ? "\n\n执行结果:\n" + r.executed.join("\n") : "";
      $("lv-plan-out").innerHTML = `<pre class="logbox">${(r.plan_lines || []).join("\n")}${extra}</pre>`;
    }
  } catch (e) {
    $("lv-plan-out").innerHTML = `<pre class="logbox neg">${e}</pre>`;
  }
  btn.textContent = label;
  btn.disabled = false;
  liveBusy = false;
  refreshMonitor();
}

$("lv-tp-btn").addEventListener("click", async () => {
  const btn = $("lv-tp-btn");
  const label = btn.textContent;
  btn.textContent = "处理中…";
  btn.disabled = true;
  try {
    const res = await fetch("/api/live/tp", { method: "POST" });
    const d = await res.json();
    if (!d.ok) throw new Error(d.error || "失败");
    if (d.started) await pollPlanOut(180);
    else $("lv-plan-out").innerHTML = `<pre class="logbox">${(d.log || []).join("\n")}</pre>`;
  } catch (e) {
    $("lv-plan-out").innerHTML = `<pre class="logbox neg">${e}</pre>`;
  }
  btn.textContent = label;
  btn.disabled = false;
  refreshMonitor(true);
});

$("lv-rebuild").addEventListener("click", async () => {
  if (
    !confirm(
      "逐币把现有仓位转成全仓？\n\n会依次「平仓 → 切全仓 → 重开」，约 16 轮，耗时 1~2 分钟。\n全程只有 1 个仓位短暂无对冲，成本约 $1~2 手续费。\n\n确认执行？"
    )
  )
    return;
  const btn = $("lv-rebuild");
  const label = btn.textContent;
  btn.textContent = "转换中…";
  btn.disabled = true;
  $("lv-plan-out").innerHTML =
    '<pre class="logbox">已提交后台执行。可以关闭页面 —— 过程不会中断，结果会写进状态。</pre>';
  try {
    const res = await fetch("/api/live/rebuild", { method: "POST" });
    const d = await res.json();
    if (!d.ok) throw new Error(d.error || "失败");
    // 后台任务：轮询状态拿结果，而不是等这个请求返回
    await pollPlanOut(6000);
  } catch (e) {
    $("lv-plan-out").innerHTML = `<pre class="logbox neg">${e}</pre>`;
  }
  btn.textContent = label;
  btn.disabled = false;
  refreshMonitor(true);
});

$("lv-plan").addEventListener("click", () => runLive(false));
$("lv-run").addEventListener("click", () => runLive(true));

async function refreshLiveConfig() {
  try {
    const d = await fetchLive();
    if (!liveLoaded) {
      lvConfigInto(d);
      liveLoaded = true;
    }
  } catch (e) {
    /* 设置页静默失败，监控页会提示 */
  }
}

// ==================== 实盘监控 ====================
let livePage = 0;
let posPage = 0;
let orderPage = 0;
let lastLiveData = null;
let chartRange = "week";
let recordScope = "all";

// 从下单记录里还原真实成交滑点（正 = 成本增加）
// 汇总下单记录：已实现盈亏 + 实测滑点（都用同一套解析，避免口径不一致）
function chronRecords(d) {
  const recs = d.records || [];
  const hints = entryHints(recs);
  let realized = 0;
  let n = 0;
  const slips = [];
  const seen = new Set();
  recs.forEach((r, i) => {
    // 只汇总带唯一成交 ID 的交易所回执；系统订单估算不混入已实现汇总。
    if (r.tid != null && seen.has(r.tid)) return;
    if (r.tid != null && Number.isFinite(r.pnl)) {
      seen.add(r.tid);
      realized += r.pnl;
      n++;
      return;
    }
    const f = parseFill(r, hints[i]);
    if (f.kind === "filled") {

      if (isFinite(f.slip)) slips.push(f.slip);
    }
  });
  const slip = slips.length
    ? { avg: slips.reduce((a, b) => a + b, 0) / slips.length, n: slips.length }
    : null;
  return { realized, n, slip };
}

function slippageStats(records) {
  const list = [];
  for (const r of records || []) {
    const m = /成交\s+([\d.eE+\-]+)@([\d.eE+\-]+)/.exec(r.result || "");
    if (!m || !r.price) continue;
    const fill = parseFloat(m[2]);
    if (!isFinite(fill) || fill <= 0) continue;
    let sl = ((fill - r.price) / r.price) * 100;
    if (r.side === "卖") sl = -sl; // 卖出成交价低于计划 = 成本增加
    list.push(sl);
  }
  if (!list.length) return null;
  return {
    n: list.length,
    avg: list.reduce((a, b) => a + b, 0) / list.length,
    worst: Math.max(...list),
    best: Math.min(...list),
  };
}

// 把交易所返回的结果文本拆成结构化字段（原来整列都是重复的一长串文字）
function parseFill(r, entryHint) {
  const t = r.result || "";
  const m = /成交\s+([\d.eE+\-]+)@([\d.eE+\-]+)/.exec(t);
  if (m) {
    const px = parseFloat(m[2]);
    let slip = r.price > 0 ? ((px - r.price) / r.price) * 100 : 0;
    if (r.side === "卖") slip = -slip; // 卖出成交价低于计划 = 成本增加
    // 平仓/减仓单：用入场价算这笔的已实现盈亏
    let pnl = null;
    const entry = r.entry_px > 0 ? r.entry_px : entryHint > 0 ? entryHint : 0;
    if (entry > 0 && recordIsClosing(r)) {
      // 卖 = 平多；买 = 平空
      pnl = r.side === "卖" ? (px - entry) * parseFloat(m[1]) : (entry - px) * parseFloat(m[1]);
    }
    const requested = /\[requested=([\d.eE+\-]+)\]/.exec(t);
    const partial = requested && Number(m[1]) < Number(requested[1]) * (1 - 1e-8);
    return { kind: "filled", px, slip, pnl, partial };
  }
  if (/未成交|挂单/.test(t)) return { kind: "unfilled" };
  if (/失败|错误|invalid|rejected/i.test(t)) {
    return { kind: "failed", msg: t.replace(/^[^:：]*失败[:：]\s*/, "").slice(0, 60) };
  }
  if (t === "计划") return { kind: "plan" };
  return { kind: "other", msg: t };
}

// 旧记录没存入场价：用该币最近一次「开仓」的成交价回推（仅缺失时兜底）
function entryHints(records) {
  const last = Object.create(null);
  const out = new Array(records.length).fill(null);
  records.forEach((r, i) => {
    const closing = /平仓|减仓/.test(r.action || "");
    out[i] = r.entry_px > 0 ? r.entry_px : closing ? last[r.coin] ?? null : null;
    const m = /成交\s+[\d.]+@([\d.]+)/.exec(r.result || "");
    if (m && !closing) last[r.coin] = parseFloat(m[1]);
  });
  return out;
}

function capitalPerformance(d) {
  const baseline = d.capital_baseline;
  const base = baseline && baseline.equity;
  const netDep = d.net_deposit == null ? 0 : Number(d.net_deposit);
  const principal = base + netDep;
  const valid = !d.error && Number.isFinite(netDep) && Number.isFinite(principal) && Number.isFinite(d.equity) && d.equity >= 0 &&
    Number.isFinite(base) && base > 0 &&
    (baseline.account || "").toLowerCase() === (d.account || "").toLowerCase();
  // 净值扣除净入金，剩下的才是策略真实赚出来的 —— 充钱/取钱不算盈亏。
  const change = valid ? d.equity - netDep - base : null;
  return { valid, base, netDep, principal, change,
    percent: valid && principal > 0 ? (change / principal) * 100 : null };
}

function isTransferFlow(f) {
  return /ML互转|互转|划转/.test((f && f.note) || "");
}

function splitCapitalFlows(d) {
  const flows = d?.capital_flows || [];
  const transferFlows = flows.filter(isTransferFlow);
  const externalFlows = flows.filter(f => !isTransferFlow(f));
  const transferNet = transferFlows.reduce((s, f) => s + (Number(f.amount) || 0), 0);
  const netDep = Number(d?.net_deposit) || 0;
  const extNetDep = netDep - transferNet;
  const mlFlows = transferFlows.map(f => ({ ...f, amount: -Number(f.amount || 0) }));
  const mlNetDep = -transferNet;
  return { externalFlows, extNetDep, mlFlows, mlNetDep };
}

function renderCapitalFlows(d) {
  const el = $("lv-flow-history");
  if (!el) return;
  const flows = (d.capital_flows || []).map((f, index) => ({ ...f, index })).reverse();
  const pages = Math.max(1, Math.ceil(flows.length / FLOW_PAGE_SIZE));
  if (flowPage > pages - 1) flowPage = 0;
  const key = JSON.stringify([d.account, d.capital_flows, flowPage]);
  if (el.flowKey === key) return; // Polling must not overwrite an in-progress time correction.
  el.flowKey = key;
  const pageRows = flows.slice(flowPage * FLOW_PAGE_SIZE, (flowPage + 1) * FLOW_PAGE_SIZE);
  el.innerHTML = flows.length
    ? pageRows.map(f => `<div class="flow-entry"><div class="flow-row"><time>${ts2m(f.effective_ts ?? f.ts)} UTC${f.effective_ts == null ? " · 登记时间" : " · 到账时间"}${isTransferFlow(f) ? " · 子账户互转" : ""}</time><b>${f.amount >= 0 ? "+" : "−"}$${fmt(Math.abs(f.amount), 2)}</b></div><details><summary>校正到账时间 / 删除</summary><input aria-label="到账时间 UTC" type="datetime-local" step="1" value="${new Date(f.effective_ts ?? f.ts).toISOString().slice(0,19)}"><button type="button" class="btn ghost" data-flow-index="${f.index}" data-flow-ts="${f.ts}" data-flow-account="${escapeHTML(d.account)}">保存时间</button><button type="button" class="btn ghost neg" data-flow-delete="${f.index}">删除</button></details></div>`).join("") + (flows.length > FLOW_PAGE_SIZE ? pager("flow", flows.length, flowPage, FLOW_PAGE_SIZE) : "")
    : '<p>暂无登记流水</p>';
}

function setupCapitalFlow() {
  const btn = $("lv-flow-btn");
  if (!btn) return;
  btn.addEventListener("click", async () => {
    const input = $("lv-flow");
    const amount = Number(input && input.value);
    if (!amount || !Number.isFinite(amount)) { alert("请填非零金额（正 = 入金，负 = 出金）"); return; }
    const isTransfer = !!($("lv-flow-transfer") && $("lv-flow-transfer").checked);
    btn.textContent = "记录中…"; btn.disabled = true;
    try {
      const res = await fetch("/api/live/capital", {
        method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ amount, effective_ts: $("lv-flow-time")?.value ? Date.parse($("lv-flow-time").value + "Z") : null, note: isTransfer ? "ML互转" : "" })
      });
      const j = await res.json();
      if (res.ok && j.ok) {
        if (input) input.value = "";
        if ($("lv-flow-transfer")) $("lv-flow-transfer").checked = false;
        $("lv-flow-status").textContent = "流水已保存，正在刷新资金指标…";
        try { const latest = await fetchLive(); liveFetchedAt = Date.now(); renderMonitor(latest); $("lv-flow-status").textContent = "流水已保存，资金指标已更新。"; }
        catch (_) { $("lv-flow-status").textContent = "流水已保存；账户刷新失败，请点击监控页刷新，无需重复记录。"; }
      }
      else { alert(j.error || ("HTTP " + res.status)); }
    } catch (e) { alert("请求失败: " + e); }
    finally { btn.textContent = "记录流水"; btn.disabled = false; }
  });
  $("lv-flow-history")?.addEventListener("click", async e => {
    const del = e.target.closest("[data-flow-delete]");
    if (!del) return;
    if (!confirm("确认删除这条资金流水？")) return;
    del.disabled = true;
    try {
      const res = await fetch("/api/live/capital/delete", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ index: Number(del.dataset.flowDelete) }) });
      const j = await res.json();
      if (!res.ok || !j.ok) throw Error(j.error || "删除失败");
      $("lv-flow-status").textContent = "流水已删除。";
      const latest = await fetchLive(); liveFetchedAt = Date.now(); renderMonitor(latest);
    } catch (err) { alert(err.message); }
    finally { del.disabled = false; }
  });
}
setupCapitalFlow();
function setupCapitalTime() {
  $("lv-flow-history")?.addEventListener("click", async e => {
    const btn = e.target.closest("[data-flow-index]");
    if (!btn) return;
    const input = btn.parentElement.querySelector("input");
    const effective_ts = Date.parse(input.value + "Z");
    if (!Number.isFinite(effective_ts)) { alert("请填写有效的 UTC 到账时间"); return; }
    btn.disabled = true;
    try {
      const res = await fetch("/api/live/capital/time", {method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({account:btn.dataset.flowAccount,index:Number(btn.dataset.flowIndex),recorded_ts:Number(btn.dataset.flowTs),effective_ts})});
      const result = await res.json();
      if (!res.ok || !result.ok) throw Error(result.error || "保存失败");
      $("lv-flow-status").textContent = "到账时间已保存。";
      try { const data = await fetchLive(); liveFetchedAt=Date.now(); renderMonitor(data); }
      catch (_) { $("lv-flow-status").textContent = "时间已保存；请刷新监控页以更新曲线。"; }
    } catch (err) { alert(err.message); }
    finally { btn.disabled=false; }
  });
}
setupCapitalTime();

function escapeHTML(value) {
  return String(value ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;", "<":"&lt;", ">":"&gt;", '"':"&quot;", "'":"&#39;"}[c]));
}

function recordAction(r) {
  return r.tid != null || /止盈\/被动成交|被动成交/.test(r.action || "") ? "交易所补录" : (r.action || "—");
}

function recordIsClosing(r) {
  return !!r.reduce_only || /平仓|减仓|止盈\/被动成交/.test(r.action || "");
}

function recordPositionSide(r) {
  return r.side === "买" ? (recordIsClosing(r) ? "short" : "long") : (recordIsClosing(r) ? "long" : "short");
}

let execDailyRange = 7;
let execDailyPage = 0;
function renderExecDaily(d) {
  const root = $("mo-exec-daily");
  if (!root) return;
  if (currentStrategy === "ml") {
    root.innerHTML = '<div class="empty">ML 子账户由 Python 定时脚本（daily.py）每 3 天独立执行调仓，当前仅接入小时级净值快照与实时持仓；逐日成交统计仅适用于动量子账户。</div>';
    return;
  }
  if (!Array.isArray(d.exec_daily)) {
    root.innerHTML = '<div class="empty">当前接口尚未提供每日执行数据，请更新服务后刷新。</div>'; return;
  }
  const today = Math.floor(Date.now() / 86400000);
  const days = d.exec_daily.filter(r => Number.isInteger(r.day) && r.day <= today && r.day > today - execDailyRange)
    .sort((a,b) => a.day - b.day);
  const valid = (key) => days.map(r=>r[key]).filter(v=>Number.isFinite(v));
  const total = key => valid(key).reduce((a,b)=>a+b,0);
  const slips = valid("avg_slip_pct"), turns = d.equity > 0 ? valid("turnover") : [];
  const value = (v,n=2) => Number.isFinite(v) ? fmt(v,n) : "—";
  const money = v => Number.isFinite(v) ? "$" + fmt(v,2) : "—";
  const dayLabel = day => new Date(day*86400000).toISOString().slice(0,10);
  const peak = Math.max(1,...valid("notional"));
  const count = days.length;
  const cellWidth = Math.max(110,...days.map(r=>money(r.notional).length*8+24));
  const bars = days.map(r=>{
    const h = Number.isFinite(r.notional) ? Math.max(0,r.notional)/peak*120 : 0;
    return `<div class="daily-exec-bar" role="listitem" aria-label="${dayLabel(r.day)} · ${money(r.notional)} · ${value(r.orders,0)} 笔">
      <span class="daily-exec-bar-value" style="bottom:${24+h+6}px">${money(r.notional)}</span>
      <span class="daily-exec-bar-fill" style="height:${h}px"></span>
      <span class="daily-exec-bar-date">${dayLabel(r.day).slice(5)}</span></div>`;
  }).join("");
  const pages=Math.max(1,Math.ceil(days.length/10));execDailyPage=Math.min(execDailyPage,pages-1);
  const rows=[...days].reverse().slice(execDailyPage*10,execDailyPage*10+10);
  root.innerHTML = `<header class="daily-exec-head"><div><span class="section-eyebrow">DAILY EXECUTION</span><h2>每日执行</h2><p>观察调仓频率与执行摩擦 · UTC 自然日</p></div>
    <div class="segmented" role="group" aria-label="每日执行时间范围">${[7,30,90].map(n=>`<button type="button" data-exec-range="${n}" aria-pressed="${execDailyRange===n}">近 ${n} 天</button>`).join("")}</div></header>
    <div class="daily-exec-kpis">${metric("区间记录笔数",days.length ? value(total("orders"),0) : "—")}${metric("区间记录名义额",days.length ? money(total("notional")) : "—")}${metric("有数据日均参考换手",turns.length ? value(turns.reduce((a,b)=>a+b,0)/turns.length)+"×" : "—")}${metric("有数据日均滑点",slips.length ? value(slips.reduce((a,b)=>a+b,0)/slips.length,3)+"%" : "—")}</div>
    ${days.length ? `<div class="daily-exec-chart"><div class="daily-exec-chart-label"><span>每日记录名义额</span><span>当日记录可能仍在更新</span></div><div class="daily-exec-plot"><div class="daily-exec-axis" style="min-width:${Math.max(70,money(peak).length*6+14)}px" aria-label="金额刻度 · USD"><span style="top:28px">${money(peak)}</span><span style="top:88px">${money(peak/2)}</span><span style="top:148px">$0</span></div><div class="daily-exec-scroll" tabindex="0" role="region" aria-label="每日金额图，可横向滚动"><div class="daily-exec-bars" role="list" style="grid-template-columns:repeat(${count},minmax(${cellWidth}px,1fr));min-width:${count*cellWidth}px">${bars}</div></div></div><div class="daily-exec-chart-label"><span>${dayLabel(days[0].day)} — ${dayLabel(days.at(-1).day)} · UTC</span><span>每柱金额精确到分 · 多日可横向滚动</span></div></div>
    <div class="table-scroll"><table><thead><tr><th>日期 · UTC</th><th>记录笔数</th><th>记录名义额</th><th>参考换手</th><th>平均不利滑点</th></tr></thead><tbody>${rows.map(r=>`<tr><td>${dayLabel(r.day)}${r.day===today ? '<span class="record-origin">今日 · 未结束</span>':''}</td><td>${value(r.orders,0)}</td><td>${money(r.notional)}</td><td>${Number.isFinite(r.turnover)&&d.equity>0 ? value(r.turnover)+"×" : "—"}</td><td class="${Number.isFinite(r.avg_slip_pct)&&r.avg_slip_pct>0 ? 'neg':''}">${Number.isFinite(r.avg_slip_pct)?value(r.avg_slip_pct,3)+"%":"—"}</td></tr>`).join("")}</tbody></table></div>
    <div class="daily-exec-pagination"><span>${days.length} 个有记录日 · ${execDailyPage+1} / ${pages} 页</span><div><button type="button" class="btn ghost" data-exec-page="prev" ${execDailyPage===0?'disabled':''}>上一页</button><button type="button" class="btn ghost" data-exec-page="next" ${execDailyPage===pages-1?'disabled':''}>下一页</button></div></div>` : '<div class="empty">所选区间暂无执行记录；不将缺失数据记为零。</div>'}
    <details class="daily-exec-definition"><summary>统计口径</summary><p>按接口 exec_daily 展示：记录可能包含失败、未执行及交易所补录，并非去重后的实际成交笔数或成交额。参考换手 = 当日记录名义额 ÷ <strong>当日净值</strong>（取该日最后一个净值点），因此入金当天会稀释该日数值；它仍不能与回测日换手直接比较。日均值仅取有数据的日期；区间滑点为每日平均值的算术平均，非逐笔或成交额加权。正滑点表示不利，缺失显示 —。此处不推算手续费、年化收益或翻倍时间。</p></details>`;
  root.onclick = event => {
    const range=event.target.closest("[data-exec-range]"),page=event.target.closest("[data-exec-page]");
    if(range) {execDailyRange=Number(range.dataset.execRange);execDailyPage=0;renderExecDaily(d);}
    else if(page&&!page.disabled) {execDailyPage+=page.dataset.execPage==='next'?1:-1;renderExecDaily(d);}
  };
}

// 实盘监控的策略切换：all=全部账户合计，momentum=动量，ml=ML。
let currentStrategy = 'all';

function renderTotal(d) {
  const el = $('mo-total');
  const band = $('mo-overview-band');
  if (!el) return;
  if (currentStrategy !== 'all') {
    el.innerHTML = '';
    if (band) band.classList.add('single-col');
    return;
  }
  const mom = Number.isFinite(d.equity) ? d.equity : 0;
  const ml = d.ml_account && !d.ml_account.error && Number.isFinite(d.ml_account.equity)
    ? d.ml_account.equity : null;
  if (ml == null) {
    el.innerHTML = '';
    if (band) band.classList.add('single-col');
    return;
  }
  if (band) band.classList.remove('single-col');
  const total = mom + ml;
  // 合计变化基准 = 两个子账户【各自的起始净值】之和（不能用合并首尾 ✗
  // ML 的初始资金会被当成收益 ✗ 之前 +2962 就是这个 bug）
  const { extNetDep, mlNetDep } = splitCapitalFlows(d);
  const momNetDep = Number(d.net_deposit) || 0;
  const aBase = (d.capital_baseline && Number.isFinite(d.capital_baseline.equity) && d.capital_baseline.equity > 0)
    ? d.capital_baseline.equity : mom;
  const mlH = d.ml_history || [];
  const bBase = mlH.length && Number.isFinite(mlH[0].equity) ? mlH[0].equity : ml;
  const baseTotal = aBase + bBase + extNetDep;
  let changeHtml = "";
  if (baseTotal > 0) {
    const diff = total - baseTotal, pct = (diff / baseTotal) * 100;
    const cls = diff >= 0 ? "pos" : "neg";
    changeHtml = `<span class="total-change ${cls}">${moneySigned(diff)}（${pct >= 0 ? "+" : ""}${fmt(pct, 1)}%）</span>`;
  }
  const momShare = total > 0 ? (mom / total) * 100 : 0;
  const mlShare = total > 0 ? (ml / total) * 100 : 0;
  const momDiff = aBase > 0 ? mom - (aBase + momNetDep) : null;
  const mlDiff = bBase > 0 ? ml - (bBase + mlNetDep) : null;
  const momSub = momDiff != null
    ? `<span class="total-sub">占比 ${fmt(momShare, 1)}% · <span class="${momDiff >= 0 ? 'pos' : 'neg'}">${moneySigned(momDiff)}</span></span>`
    : `<span class="total-sub">占比 ${fmt(momShare, 1)}%</span>`;
  const mlSub = mlDiff != null
    ? `<span class="total-sub">占比 ${fmt(mlShare, 1)}% · <span class="${mlDiff >= 0 ? 'pos' : 'neg'}">${moneySigned(mlDiff)}</span></span>`
    : `<span class="total-sub">占比 ${fmt(mlShare, 1)}%</span>`;
  el.innerHTML = `
    <div class="total-card">
      <div class="total-item total-sum"><span class="total-label">全部账户净值</span><span class="total-value">$${fmt(total, 2)}</span>${changeHtml}</div>
      <div class="total-item"><span class="total-label">动量账户</span><span class="total-value">$${fmt(mom, 2)}</span>${momSub}</div>
      <div class="total-item"><span class="total-label">ML 账户</span><span class="total-value">$${fmt(ml, 2)}</span>${mlSub}</div>
      <div class="total-alloc-bar" aria-hidden="true"><div class="total-alloc-mom" style="width:${momShare.toFixed(1)}%"></div><div class="total-alloc-ml" style="width:${mlShare.toFixed(1)}%"></div></div>
    </div>`;
}

function syncStrategyTabs() {
  for (const id of ['mo-strategy-tabs', 'an-strategy-tabs']) {
    const root = $(id);
    if (!root) continue;
    root.querySelectorAll('[data-strategy]').forEach(b => {
      b.setAttribute('aria-pressed', b.dataset.strategy === currentStrategy ? 'true' : 'false');
    });
  }
}

function bindStrategyTabs() {
  for (const id of ['mo-strategy-tabs', 'an-strategy-tabs']) {
    const root = $(id);
    if (!root || root.dataset.bound) continue;
    root.dataset.bound = '1';
    root.onclick = (event) => {
      const btn = event.target.closest('[data-strategy]');
      if (!btn) return;
      currentStrategy = btn.dataset.strategy;
      syncStrategyTabs();
      if (lastLiveData) renderMonitor(lastLiveData);
    };
  }
  syncStrategyTabs();
}

function mlMetricsSummary(d, mlAcc, mlHist) {
  const { mlFlows, mlNetDep } = splitCapitalFlows(d);
  const mlPos = mlAcc ? (mlAcc.positions || []) : [];
  const eq = Number.isFinite(mlAcc?.equity) ? mlAcc.equity : NaN;
  const base = (mlHist[0] || {}).equity;
  const baseTs = (mlHist[0] || {}).ts || null;
  const principal = Number.isFinite(base) && base > 0 ? base + mlNetDep : NaN;
  const valid = !!mlAcc && Number.isFinite(eq) && Number.isFinite(principal) && principal > 0;
  const change = valid ? eq - mlNetDep - base : null;
  const percent = valid ? (change / principal) * 100 : null;
  const unrealized = mlPos.reduce((s, p) => s + (Number.isFinite(p.unrealized) ? p.unrealized : 0), 0);
  const grossNotional = mlPos.reduce((s, p) => s + (Number.isFinite(p.notional) ? p.notional : 0), 0);
  const netNotional = mlPos.reduce((s, p) => s + (p.side === "long" ? 1 : -1) * (Number.isFinite(p.notional) ? p.notional : 0), 0);
  const dists = mlPos.map(p => p.dist_pct).filter(Number.isFinite);
  const minDist = dists.length ? Math.min(...dists) : null;
  const isoN = mlPos.filter(p => !p.is_cross).length;

  let adjPts = [];
  if (Number.isFinite(base) && base > 0 && mlHist.length >= 2) {
    const res = strategyHistory(mlHist, {
      account: mlAcc?.account || "ml",
      capital_baseline: { equity: base, ts: baseTs, account: mlAcc?.account || "ml" },
      capital_flows: mlFlows,
      net_deposit: mlNetDep,
    });
    adjPts = res.history || [];
  }
  let maxDdPct = null, annVolPct = null, winDays = 0, totalDays = 0, winMonths = 0, totalMonths = 0;
  if (adjPts.length >= 2) {
    let peak = -Infinity, maxDd = 0;
    const dayLast = new Map(), monthLast = new Map();
    const rets = [];
    for (let i = 0; i < adjPts.length; i++) {
      const p = adjPts[i];
      // strategyHistory 返回的是扣除基准和出入金后的累计盈亏（从 0 起步），加回 base 才是调整后净值
      const v = base + (Number.isFinite(p.equity) ? p.equity : 0);
      if (v > peak) peak = v;
      if (peak > 0) maxDd = Math.max(maxDd, ((peak - v) / peak) * 100);
      const iso = new Date(p.ts).toISOString();
      dayLast.set(iso.slice(0, 10), v);
      monthLast.set(iso.slice(0, 7), v);
      if (i > 0) {
        const dtHours = (adjPts[i].ts - adjPts[i - 1].ts) / 3600000;
        const prev = base + (Number.isFinite(adjPts[i - 1].equity) ? adjPts[i - 1].equity : 0);
        if (dtHours >= 0.5 && prev > 0) {
          rets.push((v / prev - 1) / Math.sqrt(dtHours));
        }
      }
    }
    maxDdPct = maxDd;
    if (rets.length >= 3) {
      const mean = rets.reduce((a, r) => a + r, 0) / rets.length;
      const var_ = rets.reduce((a, r) => a + (r - mean) ** 2, 0) / (rets.length - 1);
      annVolPct = Math.sqrt(var_) * Math.sqrt(24 * 365) * 100;
    }
    const dVals = [...dayLast.values()];
    for (let i = 1; i < dVals.length; i++) { totalDays++; if (dVals[i] > dVals[i - 1]) winDays++; }
    const mVals = [...monthLast.values()];
    for (let i = 1; i < mVals.length; i++) { totalMonths++; if (mVals[i] > mVals[i - 1]) winMonths++; }
  }
  return {
    valid, base, baseTs, netDep: mlNetDep, principal, eq, change, percent, unrealized,
    realized: valid ? change - unrealized : null,
    grossNotional, netNotional, minDist, nPos: mlPos.length, isoN, nCross: mlPos.length - isoN,
    actualLev: Number.isFinite(eq) && eq > 0 ? grossNotional / eq : 0,
    maxDdPct, annVolPct,
    winRateDays: totalDays ? (winDays / totalDays) * 100 : null, winSamplesDays: totalDays,
    winRateMonths: totalMonths ? (winMonths / totalMonths) * 100 : null, winSamplesMonths: totalMonths,
    lastTs: mlHist.length ? mlHist[mlHist.length - 1].ts : null,
    pointsCount: mlHist.length,
  };
}

// ML 计划调仓：拉取 predict 的目标持仓 + place 的下单指令（dry-run）
// 安全的 JSON 请求：空响应/非 JSON 给出可读错误，而不是 "Unexpected end of JSON input"
async function fetchJsonSafe(url, opts = {}, timeoutMs = 300000) {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), timeoutMs);
  try {
    const res = await fetch(url, { ...opts, signal: ctrl.signal });
    const text = await res.text();
    if (!text) throw new Error(`空响应（HTTP ${res.status}）✗ 可能后端超时或中断`);
    try {
      return JSON.parse(text);
    } catch {
      throw new Error(`非 JSON 响应（HTTP ${res.status}）: ${text.slice(0, 200)}`);
    }
  } catch (e) {
    if (e.name === 'AbortError') throw new Error(`请求超时（${Math.round(timeoutMs/1000)}s）`);
    throw e;
  } finally {
    clearTimeout(timer);
  }
}

async function mlPlan() {
  const view = $('lv-plan-out');
  if (!view) return;
  const btn = $('ml-plan-btn');
  const label = btn?.textContent || 'ML 计划调仓（不下单）';
  if (btn) {
    btn.disabled = true;
    btn.textContent = '计算中…';
  }
  view.innerHTML = '<div class="empty">正在计算 ML 计划调仓…</div>';
  try {
    const d = await fetchJsonSafe("/api/ml/plan", { cache: "no-store" }, 120000);
    if (!d.ok) { view.innerHTML = `<div class="note neg">${escapeHTML(d.error || '失败')}</div>`; return; }
    renderMlPlan(view, d);
  } catch (e) {
    view.innerHTML = `<div class="note neg">读取 ML 计划失败：${escapeHTML(String(e))}</div>`;
  } finally {
    if (btn) {
      btn.textContent = label;
      btn.disabled = false;
    }
  }
}

function renderMlPlan(view, d) {
  const pred = d.predict || {};
  const plan = d.plan || {};
  const orders = plan.orders || [];
  const rows = orders.map(o => {
    const side = o.buy ? '买入' : '卖出';
    const cls = o.buy ? 'pos' : 'neg';
    return `<tr><td>${escapeHTML(o.coin)}</td><td class="${cls}">${side}</td><td>${escapeHTML(o.size)}</td><td>$${fmt(Number(o.price), 4)}</td></tr>`;
  }).join("");
  const long = (pred.long || []).map(escapeHTML).join(", ") || "—";
  const short = (pred.short || []).map(escapeHTML).join(", ") || "—";
  view.innerHTML = `
    <div class="execution-result-block">
      <div class="result-heading"><b>ML 计划调仓</b><span class="badge-mini mute">未下单</span></div>
      <p class="sub result-targets">目标持仓：做多 <b>${long}</b> · 做空 <b>${short}</b></p>
      ${orders.length ? `<div class="table-scroll"><table><thead><tr><th>币</th><th>方向</th><th>数量</th><th>价格</th></tr></thead><tbody>${rows}</tbody></table></div>` : '<div class="empty">当前已符合目标，无需调仓</div>'}
    </div>`;
}

async function mlExecute() {
  const view = $('lv-plan-out');
  const btn = $('ml-execute-btn');
  if (btn) { btn.disabled = true; btn.textContent = '执行中…'; }
  if (view) view.innerHTML = '<div class="empty">正在执行 ML 调仓，等待交易所回执…</div>';
  try {
    const d = await fetchJsonSafe("/api/ml/execute", { method: "POST" }, 300000);
    if (!view) return;
    const results = d.results || [];
    const rows = results.map(r => {
      const ok = r.ok !== false;
      const side = r.buy == null ? "—" : r.buy ? "买入" : "卖出";
      const status = r.error || r.status || (ok ? "已提交" : "失败");
      return `<tr><td>${escapeHTML(r.coin || "—")}</td><td>${side}</td><td>${escapeHTML(r.size ?? "—")}</td><td class="${ok ? "pos" : "neg"}">${escapeHTML(status)}</td></tr>`;
    }).join("");
    const ok = d.ok !== false;
    const headline = d.note || d.error || (ok ? `ML 调仓完成：${results.length} 项订单` : `ML 调仓未全部成功：${Number(d.errors) || results.filter(r => r.ok === false).length} 项失败`);
    view.innerHTML = `<div class="execution-result-block">
      <div class="note ${ok ? "ok" : "neg"}">${ok ? "✅" : "⚠️"} ${escapeHTML(headline)}</div>
      ${rows ? `<div class="table-scroll"><table><thead><tr><th>币</th><th>方向</th><th>数量</th><th>交易所回执</th></tr></thead><tbody>${rows}</tbody></table></div>` : ""}
    </div>`;
  } catch (e) {
    if (view) view.innerHTML = `<div class="execution-result-block"><div class="note neg">执行失败：${escapeHTML(String(e))}</div></div>`;
  } finally {
    if (btn) { btn.disabled = false; btn.textContent = '执行调仓'; }
  }
}

function bindMlPlanButtons() {
  const planBtn = $('ml-plan-btn');
  const execBtn = $('ml-execute-btn');
  const resetBtn = $('ml-reset-btn');
  if (planBtn && !planBtn.dataset.bound) {
    planBtn.dataset.bound = '1';
    planBtn.onclick = mlPlan;
  }
  if (execBtn && !execBtn.dataset.bound) {
    execBtn.dataset.bound = '1';
    execBtn.onclick = mlExecute;
  }
  if (resetBtn && !resetBtn.dataset.bound) {
    resetBtn.dataset.bound = '1';
    resetBtn.onclick = mlReset;
  }
}

// ML 重置记录：只清空 ML 净值历史 ✗ 不动动量。平仓后调整初始资金时用。
async function mlReset() {
  const view = $('lv-plan-out');
  const btn = $('ml-reset-btn');
  if (!confirm('ML 重置记录：清空 ML 子账户的净值历史（不动动量记录）。下一次记录会用【当前净值】作为新基准。\n\n建议先平掉 ML 全部仓位、把资金调整到目标金额，再点这个。继续？')) return;
  if (btn) { btn.disabled = true; btn.textContent = '重置中…'; }
  try {
    const d = await fetchJsonSafe('/api/ml/reset', { method: 'POST' }, 30000);
    if (view) view.innerHTML = `<div class="execution-result-block"><div class="note ok">✅ ${escapeHTML(d.note || 'ML 净值历史已清空，下一次记录会用当前净值作为新基准')}</div></div>`;
    setTimeout(() => { try { refreshLive(); } catch (_) {} }, 800);
  } catch (e) {
    if (view) view.innerHTML = `<div class="execution-result-block"><div class="note neg">重置失败：${escapeHTML(String(e))}</div></div>`;
  } finally {
    if (btn) { btn.disabled = false; btn.textContent = 'ML 重置记录'; }
  }
}

function renderMonitor(d) {
  if (!d) return;
  lastLiveData = d;
  renderTotal(d);
  bindStrategyTabs();
  bindMlPlanButtons();
  renderCapitalFlows(d);
  const c = d.config || {};
  // 策略切换：ML 时看 ML 子账户的持仓/净值，否则看动量子账户。
  const mlAcc = d.ml_account && !d.ml_account.error ? d.ml_account : null;
  const isMl = currentStrategy === 'ml';
  const allView = currentStrategy === 'all' && !!mlAcc;
  const momPositions = d.positions || [];
  const mlPositions = mlAcc ? (mlAcc.positions || []) : [];
  const pos = isMl
    ? mlPositions
    : allView
      ? [...momPositions.map(p => ({ ...p, _account: '动量' })), ...mlPositions.map(p => ({ ...p, _account: 'ML' }))]
      : momPositions;
  const positionGross = positions => positions.reduce((sum, p) => sum + (Number.isFinite(p.notional) ? p.notional : 0), 0);
  const grossNow = allView
    ? (d.gross_notional || positionGross(momPositions)) + positionGross(mlPositions)
    : isMl
      ? positionGross(mlPositions)
      : (d.gross_notional || 0);
  const mlHist = d.ml_history || [];
  const hist = currentStrategy === 'all'
    ? mergeHistory(d.history || [], mlHist)
    : isMl ? mlHist : (d.history || []);
  const eq = isMl
    ? (Number.isFinite(mlAcc?.equity) ? mlAcc.equity : NaN)
    : allView
      ? (d.equity || 0) + (mlAcc.equity || 0)
      : (d.equity || 0);
  const capital = capitalPerformance(d);
  const valid = capital.valid;
  const pnl = capital.change;
  const pnlPct = capital.percent;
  const buffers = allView
    ? [d.liq_buffer_pct, mlAcc.liq_buffer_pct].filter(Number.isFinite)
    : [isMl ? mlAcc?.liq_buffer_pct : d.liq_buffer_pct].filter(Number.isFinite);
  const buffer = buffers.length ? Math.min(...buffers) : 0;

  const keyed = !!(c.key_path && c.key_path.trim());
  const ready = !!(d.configured && keyed);
  const holding = pos.length > 0;
  const auto = !!c.auto_run;
  const nextRun = (() => {
    const n = new Date();
    const t = new Date(Date.UTC(n.getUTCFullYear(), n.getUTCMonth(), n.getUTCDate(), 0, 5, 0));
    if (t <= n) t.setUTCDate(t.getUTCDate() + 1);
    return ts2m(t.getTime());
  })();

  // 状态横幅：一眼看出到底跑没跑
  let cls, tag, text;
  if (isMl && !mlAcc) {
    cls = "idle";
    tag = "● ML 账户数据暂不可用";
    text = escapeHTML(d.ml_account?.error || d.error || "暂时无法读取 ML 子账户快照，请稍后刷新。");
  } else if (d.error) {
    cls = "idle";
    tag = "● 账户数据不可用";
    text = escapeHTML(d.error);
  } else if (c.armed && keyed && holding) {
    cls = "run";
    tag = "● 运行中";
    text = `<b>实盘已启动</b> · 持有 <b>${pos.length}</b> 个仓位 · 总名义 $${fmt(grossNow, 0)}`;
  } else if (c.armed && keyed && !holding) {
    cls = "idle";
    tag = "● 已启用，尚无持仓";
    text = "点「执行调仓」建立初始仓位（或等自动调仓）";
  } else if (c.armed && !keyed) {
    cls = "err";
    tag = "● 无法下单";
    text = "<b>API 钱包密钥路径为空</b>，无法签名发单。请到「实盘设置」填好并保存。";
  } else {
    cls = "off";
    tag = "● 未启用";
    text = "当前只会生成计划，不会发送订单";
  }
  const isoN = allView
    ? (Number(d.isolated_count) || 0) + (Number(mlAcc.isolated_count) || 0)
    : isMl
      ? (Number(mlAcc?.isolated_count) || 0)
      : (d.isolated_count || 0);
  if (isoN > 0) {
    text += `<span class="neg"><b>⚠ ${isoN} 个仓位仍是逐仓</b>（Hyperliquid 不允许持仓时切换模式，需用「转为全仓」逐币处理）</span>`;
  }
  const autoText = isMl
    ? (() => {
        // ML 调仓日 = 2026-05-04 起每 3 天（UTC 00:00）—— 显示下一次 + 距离天数
        const base = Date.UTC(2026, 4, 4);
        const step = 3 * 86400000;
        const now = Date.now();
        const next = base + (Math.floor((now - base) / step) + 1) * step;
        const bj = new Date(next + 8 * 3600000 + 10 * 60000);  // UTC 00:10 = 北京 08:10（等 5min K 线闭合）
        const dstr = bj.toISOString().slice(5, 10);
        const hhmm = bj.toISOString().slice(11, 16);
        return `<span>ML 调仓：<b>每 3 天</b>（daily.py 自动 ✗ 下次 <b>${dstr} ${hhmm}</b> 北京时间）</span>`;
      })()
    : (auto
      ? `<span>每日自动调仓：<b>已开启</b>，下次 <b>${nextRun} UTC</b>（北京时间 08:05）</span>`
      : `<span>每日自动调仓：<b>已关闭</b>（需手动点「执行调仓」）</span>`);
  // ML 有自己的调仓节奏（每 3 天）✗ 不能显示动量的"上次调仓"
  const mlLastTs = (d.ml_history || []).length ? d.ml_history[d.ml_history.length - 1].ts : null;
  const lastText = isMl
    ? (mlLastTs
      ? `<span>ML 净值最后记录 <b>${ts2m(mlLastTs)}</b></span>`
      : "<span>ML 还没有净值记录</span>")
    : (d.last_run_at
      ? `<span>上次调仓 <b>${ts2m(d.last_run_at)}</b>${d.last_live ? "（真实下单）" : "（仅生成计划）"}</span>`
      : "<span>还没有调仓记录</span>");
  const ago = liveFetchedAt ? Math.round((Date.now() - liveFetchedAt) / 1000) : null;
  const freshness = ago == null ? "" : `<span class="muted" id="mo-ago">数据更新于 ${ago} 秒前</span>`;
  $("mo-status").innerHTML = `<div class="live-status ${cls}">
    <div class="live-status-left"><span class="tag">${tag}</span><span>${text}</span></div>
    <div class="live-status-right">${autoText}${lastText}${freshness}</div>
  </div>`;

  const parsed = chronRecords(d);
  const realized = parsed.realized;
  const slip = parsed.slip;
  const targetLev = (c.leverage || 0) * (c.margin_buffer || 1);
  // ML tab 用 ML 子账户自己的总名义（d.gross_notional 是动量的）
  const actualLev = eq > 0 ? grossNow / eq : 0;
  const nPos = pos.length;
  const nCross = nPos - isoN;
  const currentValid = !d.error && Number.isFinite(eq) && pos.every(p => Number.isFinite(p.unrealized));
  const unrealized = pos.reduce((sum, p) => sum + (Number.isFinite(p.unrealized) ? p.unrealized : 0), 0);

  // 拿不到行情的持仓：必须显眼提示，否则净值看着正常但风险是隐形的
  const unp = isMl ? [] : (d.unpriced || []);
  $("mo-warn").innerHTML = unp.length
    ? `<div class="warn-banner">⚠ ${unp.length} 个持仓拿不到行情（${unp
        .slice(0, 8)
        .join(" ")}${unp.length > 8 ? " …" : ""}）—— 已改用交易所市值估值，
        但这些仓位的强平距离无法计算，请检查是否已退市。</div>`
    : "";

  if (isMl) {
    const mlm = mlMetricsSummary(d, mlAcc, mlHist);
    $("mo-metrics").innerHTML =
      metricGroup(
        "ML 账户 · 当前持仓",
        metric("ML 净值", mlm.valid ? "$" + fmt(mlm.eq, 2) : "—") +
        metric("未实现盈亏 · PNL", mlm.valid ? moneySigned(mlm.unrealized) : "—", mlm.valid ? (mlm.unrealized >= 0 ? "pos" : "neg") : "") +
        metric("持仓数量", mlm.valid ? mlm.nPos + '<span class="metric-unit"> 个</span>' : "—") +
        metric("实际杠杆", mlm.valid ? fmt(mlm.actualLev, 2) + '<span class="metric-unit"> x</span>' : "—"),
        "live-overview",
        '<div class="metric-footnote"><span class="live-dot"></span>ML 子账户独立统计，仅包含 ML 持仓与净值曲线。</div>'
      ) +
      metricGroup(
        "资金表现 · ML 相对起点",
        metric("净投入本金", mlm.valid ? "$" + fmt(mlm.principal, 2) : "—") +
          metric("子账户互转净额", mlm.valid ? moneySigned(mlm.netDep) : "—") +
          metric("扣除互转后净收益", mlm.valid ? moneySigned(mlm.change) : "—", mlm.valid && mlm.change >= 0 ? "pos" : "neg") +
          metric("已实现盈亏 · 推算", mlm.valid ? moneySigned(mlm.realized) : "—", mlm.valid && mlm.realized >= 0 ? "pos" : "neg") +
          metric("净投入收益率", mlm.percent != null ? pct(mlm.percent) : "—", mlm.percent != null ? (mlm.percent >= 0 ? "pos" : "neg") : ""),
        "capital-account",
        `<details class="metric-definition"><summary>ML 记录起点 ${Number.isFinite(mlm.base) ? "$" + fmt(mlm.base, 2) : "—"} · ${mlm.baseTs ? ts2m(mlm.baseTs) + " UTC" : "时间未知"} · 查看口径</summary><p>ML 净投入本金 = ML 记录起点资金 + 子账户互转净额；净收益 = 当前 ML 净值 − 净投入本金；已实现盈亏·推算 = 净收益 − 当前未实现盈亏。</p></details>`
      ) +
      metricGroup(
        "风险与仓位 · ML",
        metric("最低强平缓冲", mlm.minDist != null ? fmt(mlm.minDist, 1) + "%" : "—", mlm.minDist == null ? "" : (mlm.minDist < 40 ? "neg" : "pos")) +
          metric("净敞口 · 多减空", mlm.valid ? moneySigned(mlm.netNotional) : "—", mlm.valid && Math.abs(mlm.netNotional) <= mlm.eq * 0.15 ? "" : "neg") +
          metric("总名义敞口", mlm.valid ? "$" + fmt(mlm.grossNotional, 0) : "—") +
          metric("保证金模式", mlm.nPos ? (mlm.isoN > 0 ? mlm.nCross + " 全仓 / " + mlm.isoN + " 逐仓" : mlm.nCross + " 全仓") : "—", mlm.isoN > 0 ? "neg" : "pos")
      ) +
      metricGroup(
        "调仓与记录 · ML",
        metric("调仓节奏", "每 3 天") +
          metric("净值样本点数", mlm.pointsCount + " 点") +
          metric("年化波动率", mlm.annVolPct != null ? fmt(mlm.annVolPct, 1) + "%" : "—") +
          metric("记录期最大回撤", mlm.maxDdPct != null ? fmt(mlm.maxDdPct, 2) + "%" : "—", mlm.maxDdPct != null && mlm.maxDdPct > 15 ? "neg" : "") +
          metric("最后净值时间", mlm.lastTs ? `<span class="sm">${ts2m(mlm.lastTs)}</span>` : "—")
      ) +
      metricGroup(
        "胜率 · ML 净值口径",
        metric("按交易日", mlm.winRateDays != null ? fmt(mlm.winRateDays, 0) + "%" : "—",
          mlm.winRateDays == null ? "" : (mlm.winRateDays >= 50 ? "pos" : "neg")) +
          metric("按自然月", mlm.winRateMonths != null ? fmt(mlm.winRateMonths, 0) + "%" : "—",
            mlm.winRateMonths == null ? "" : (mlm.winRateMonths >= 50 ? "pos" : "neg")) +
          metric("样本量", `<span class="sm">${mlm.winSamplesDays} 天 / ${mlm.winSamplesMonths} 月</span>`),
        "win-rates",
        '<details class="metric-definition"><summary>ML 胜率口径说明</summary><p>按每日/每月最后一个调整后净值点对比前一日/上一月计算（已扣除子账户互转影响）。</p></details>'
      );
  } else {
  $("mo-metrics").innerHTML =
    metricGroup(
      allView ? "账户合计 · 当前持仓" : "动量账户 · 当前持仓",
      metric("账户净值", currentValid ? "$" + fmt(eq, 2) : "—") +
      metric("未实现盈亏 · PNL", currentValid ? moneySigned(unrealized) : "—", currentValid ? (unrealized >= 0 ? "pos" : "neg") : "") +
      metric("持仓数量", currentValid ? nPos + '<span class="metric-unit"> 个</span>' : "—") +
      metric("实际杠杆", currentValid ? fmt(actualLev, 2) + '<span class="metric-unit"> x</span>' : "—"),
      "live-overview",
      '<div class="metric-footnote"><span class="live-dot"></span>未实现盈亏为当前持仓浮盈浮亏，不与历史已实现盈亏相加。' +
        "</div>"
    ) +
    metricGroup(
      "资金表现 · 相对记录起点",
      metric("净投入本金", valid ? "$" + fmt(capital.principal, 2) : "—") +
        metric("起点后净入金", valid ? moneySigned(capital.netDep) : "—") +
        metric("扣除出入金后净收益", valid ? moneySigned(pnl) : "—", valid && pnl >= 0 ? "pos" : "neg") +
        metric("已实现盈亏 · 推算", valid ? moneySigned(pnl - unrealized) : "—", valid && pnl - unrealized >= 0 ? "pos" : "neg") +
        metric("净投入收益率", pnlPct != null ? pct(pnlPct) : "—", pnlPct != null ? (pnlPct >= 0 ? "pos" : "neg") : ""),
      "capital-account",
      `<details class="metric-definition"><summary>原始起点 ${valid ? "$" + fmt(capital.base, 2) : "—"} · ${d.capital_baseline?.ts ? ts2m(d.capital_baseline.ts) + " UTC" : "时间未知"} · 查看口径</summary><p>净投入本金 = 记录起点资金 + 起点后净入金；净收益 = 当前净值 − 净投入本金；净投入收益率 = 净收益 ÷ 净投入本金。本金不大于零时不显示收益率。这是简单本金收益率，不是时间加权或年化收益率。已体现账户手续费与资金费，准确性依赖出入金记录完整。已实现盈亏·推算 = 扣除出入金后净收益 − 当前未实现盈亏，它才是与净值对得上的已实现数；下方「补录已实现·未扣费」只汇总交易所回执（不含计划调仓的平仓、也未扣手续费），两者不应相等。记录入金不修改历史起点；旧账户的记录起点可能晚于首次启动。</p></details>`
    ) +
    metricGroup(
      "风险与仓位",
      metric("账户强平缓冲", currentValid ? fmt(buffer, 1) + "%" : "—", buffer < 40 ? "neg" : "pos") +
        metric("净敞口 · 多减空", currentValid ? moneySigned((d.net_notional || 0) + (allView ? (mlAcc.net_notional || 0) : 0)) : "—", Math.abs((d.net_notional || 0) + (allView ? (mlAcc.net_notional || 0) : 0)) <= eq * 0.1 ? "" : "neg") +
        metric("实际 / 目标杠杆", fmt(actualLev, 2) + "x / " + fmt(targetLev, 2) + "x",
          actualLev < targetLev * 0.9 ? "neg" : "pos") +
        metric(
          "保证金模式",
          // 全是全仓时只说「全仓」。逐仓是早期一个真实 bug（is_cross 传了 false），
          // 现已修复；把「0 逐仓」挂在卡片上只会让人以为还有问题。
          nPos
            ? isoN > 0
              ? nCross + " 全仓 / " + isoN + " 逐仓"
              : nCross + " 全仓"
            : "—",
          isoN > 0 ? "neg" : "pos"
        )
    ) +
    metricGroup(
      "执行质量与回执",
      metric("实测平均滑点", slip ? slip.avg.toFixed(3) + "%" : "—", slip && slip.avg > 0.15 ? "neg" : "pos") +
        metric("补录已实现 · 未扣费", parsed.n ? moneySigned(realized) : "—",
          realized >= 0 ? "pos" : "neg") +
        metric("总名义敞口", "$" + fmt(grossNow, 0)) +
        metric("当前挂单", currentValid ? (d.tp_orders || []).length + " 笔" : "—") +
        metric("最后调仓", d.last_run_at ? `<span class="sm">${ts2m(d.last_run_at)}</span>` : "—")
    ) +
    metricGroup(
      "胜率 · 三种口径",
      metric("按交易日", d.win_rate_days != null ? fmt(d.win_rate_days, 0) + "%" : "—",
        d.win_rate_days == null ? "" : (d.win_rate_days >= 50 ? "pos" : "neg")) +
        metric("按自然月", d.win_rate_months != null ? fmt(d.win_rate_months, 0) + "%" : "—",
          d.win_rate_months == null ? "" : (d.win_rate_months >= 50 ? "pos" : "neg")) +
        metric("按平仓记录", d.win_rate_trades != null ? fmt(d.win_rate_trades, 0) + "%" : "—",
          d.win_rate_trades == null ? "" : (d.win_rate_trades >= 50 ? "pos" : "neg")) +
        metric("样本量", `<span class="sm">${d.win_samples_days || 0} 天 / ${d.win_samples_months || 0} 月 / ${d.win_samples_trades || 0} 笔</span>`),
      "win-rates",
      '<details class="metric-definition"><summary>三个口径分别怎么算 · 查看定义</summary><p>' +
        '<b>按交易日</b>：当日最后一个净值点 &gt; 前一日最后一个点，算赢；只有完整记录过的日子才计入。<br>' +
        '<b>按自然月</b>：当月最后一个净值点 &gt; 上月最后一个点，算赢；样本不足两个月时显示 —。<br>' +
        '<b>按平仓记录</b>：交易所回执里的已实现盈亏 &gt; 0 算赢，已含手续费；同一仓位分多次平仓会算成多笔，所以它不等于「按仓位」的胜率。<br>' +
        '三者都不是预测，只是对已发生记录的不同切法；样本很小时波动极大。</p></details>'
    );
  }
  try {
    arrangeMonitorSummary("mo-metrics");
  } catch (e) {
    console.error("arrangeMonitorSummary failed:", e);
  }

  // 检查清单按实测结果自动打勾
  const sl = slippageStats(d.records || []);
  const mark = (sid, bid, ok, okText, okDetail) => {
    const se = $(sid), be = $(bid);
    if (!se || !be) return;
    if (ok) {
      se.textContent = "✅";
      be.className = "badge ok";
      be.textContent = okText;
      if (okDetail) {
        const te = $(sid.replace("-s", "-t"));
        if (te) te.textContent = okDetail;
      }
    }
  };
  if (sl) {
    mark("ck-slip-s", "ck-slip-b", true, "已实测",
      `实测 ${sl.n} 笔成交，平均滑点 ${sl.avg >= 0 ? "+" : ""}${sl.avg.toFixed(3)}%（最差 ${sl.worst.toFixed(3)}%，最好 ${sl.best.toFixed(3)}%）；纸交易假设 0 滑点`);
  }
  mark("ck-buf-s", "ck-buf-b", buffer > 40, "已通过",
    buffer > 40 ? `当前 ${buffer.toFixed(1)}%，安全` : `当前 ${buffer.toFixed(1)}%，偏低`);

  const err = d.error ? `<div class="note neg">${escapeHTML(d.error)}</div>` : "";
  if (!pos.length) {
    $("mo-positions").innerHTML = err + '<div class="empty">账户当前没有持仓</div>';
  } else {
    const posPages = Math.max(1, Math.ceil(pos.length / POS_PAGE_SIZE));
    if (posPage > posPages - 1) posPage = 0;
    const rows = pos
      .slice(posPage * POS_PAGE_SIZE, (posPage + 1) * POS_PAGE_SIZE)
      .map((p) => {
        const cls = p.unrealized >= 0 ? "pos" : "neg";
        const dist = p.dist_pct == null ? "—" : fmt(p.dist_pct, 1) + "%";
        // 离止盈：止盈线 = 成本 × (1±tp) ✗ 显示还差多少【价格】✗
        const tp = isMl || p._account === 'ML' ? 0 : ((d.config && d.config.take_profit_pct) || 0);
        const long = p.side === "long";
        const line = tp > 0 && p.entry_px > 0 ? p.entry_px * (long ? 1 + tp : 1 - tp) : null;
        const toTp = line && p.mark_px > 0 ? (line / p.mark_px - 1) * (long ? 1 : -1) * 100 : null;
        const toTpCell = toTp == null
          ? "—"
          : `<span class="${toTp <= 0 ? "pos" : "muted"}">${toTp <= 0 ? "已到线" : fmt(toTp, 1) + "%"}</span>`;
        return `<tr>
          <td>${p.coin}</td>
          ${allView ? `<td><span class="badge-mini ${p._account === 'ML' ? 'mute' : 'ok'}">${p._account}</span></td>` : ''}
          <td>${sideLabel(p.side)}</td>
          <td>${fmtCompact(p.size)}</td>
          <td>${fmtCompact(p.entry_px)}</td>
          <td>${fmtCompact(p.mark_px)}</td>
          <td>$${fmt(p.notional, 0)}</td>
          <td class="${cls}">${moneySigned(p.unrealized)}</td>
          <td>${p.liq_px == null ? "—" : fmtCompact(p.liq_px)}</td>
          <td class="${p.dist_pct != null && p.dist_pct < 20 ? "neg" : "muted"}">${dist}</td>
          <td>${toTpCell}</td>
          <td>${p.is_cross ? '<span class="badge-mini ok">全仓</span>' : '<span class="badge-mini err">逐仓</span>'}</td>
        </tr>`;
      })
      .join("");
    $("mo-positions").innerHTML = err + `<div class="table-scroll"><table><thead><tr>
      <th>币</th>${allView ? '<th>账户</th>' : ''}<th>方向</th><th>数量</th><th>入场价</th><th>当前价</th>
      <th>名义</th><th>未实现盈亏</th><th>强平价</th><th>距强平</th><th>离止盈</th><th>保证金模式</th>
    </tr></thead><tbody>${rows}</tbody></table></div>` + pager("pos", pos.length, posPage, POS_PAGE_SIZE);
  }

  renderExecDaily(d);
  renderMonitorChart(d);

  if (isMl) {
    $("mo-orders").innerHTML = '<div class="empty">ML 策略由每日定时脚本（daily.py）每 3 天独立执行调仓，不使用动量被动止盈挂单。</div>';
    $("mo-records").innerHTML = '<div class="empty">ML 子账户暂未接入独立成交流水；请在「实盘监控」查看 ML 实时持仓与净值曲线，或切换至「动量策略」查看动量订单与成交记录。</div>';
  } else {
  const chron = d.records || [];
  const hints = entryHints(chron);
  const orders = d.tp_orders || [];
  const orderPages = Math.max(1, Math.ceil(orders.length / LIVE_PAGE_SIZE));
  if (orderPage > orderPages - 1) orderPage = 0;
  $("mo-orders").innerHTML = orders.length
    ? `<div class="table-scroll"><table><thead><tr><th>币</th><th>方向</th><th>挂单价</th><th>数量</th></tr></thead><tbody>${orders
        .slice(orderPage * LIVE_PAGE_SIZE, (orderPage + 1) * LIVE_PAGE_SIZE)
        .map((o) => `<tr><td>${o.coin}</td><td>${orderSideLabel(o.side)}</td><td>${fmtCompact(o.px)}</td><td>${fmtCompact(o.sz)}</td></tr>`)
        .join("")}</tbody></table></div>` + pager("order", orders.length, orderPage, LIVE_PAGE_SIZE)
    : '<div class="empty">当前没有未成交挂单</div>';

  const recs = chron.map((r, i) => ({ r, hint: hints[i] }))
    .filter(({ r }) => recordScope === "all" || (recordScope === "exchange" ? r.tid != null : r.tid == null))
    .sort((a, b) => b.r.ts - a.r.ts);
  const pages = Math.max(1, Math.ceil(recs.length / LIVE_PAGE_SIZE));
  if (livePage > pages - 1) livePage = 0;
  const pageItems = recs.slice(livePage * LIVE_PAGE_SIZE, (livePage + 1) * LIVE_PAGE_SIZE);
  $("mo-records").innerHTML = recs.length
    ? `<div class="table-scroll"><table><thead><tr>
        <th>时间 · UTC</th><th>币</th><th>持仓方向</th><th>动作 / 来源</th><th>数量</th>
        <th>计划价</th><th>成交价</th><th>滑点</th><th>已实现 · 未扣费</th><th>状态</th>
      </tr></thead><tbody>${pageItems
        .map(({ r, hint }) => {
          const f = parseFill(r, hint);
          let pxCell = '<span class="muted">—</span>';
          let slipCell = '<span class="muted">—</span>';
          let pnlCell = '<span class="muted">—</span>';
          let status = '<span class="badge-mini mute">—</span>';
          // 交易所补录回执直接带成交价和盈亏，无法据此判断订单意图，
          // 但没有我们自己下单时的「计划价 vs 成交价」结构，所以 parseFill
          // 认不出来 —— 必须在解析之前单独处理，否则这几列全是空的。
          if (r.pnl != null) {
            status = '<span class="badge-mini ok">成交</span>';
            pxCell = r.price ? fmtCompact(r.price) : '<span class="muted">—</span>';
            const cls = r.pnl >= 0 ? "pos" : "neg";
            pnlCell = `<span class="${cls}">${moneySigned(r.pnl)}</span>`;
            return `<tr>
              <td class="muted">${ts2m(r.ts)}</td>
              <td>${escapeHTML(r.coin)}</td>
              <td>${sideLabel(recordPositionSide(r), recordIsClosing(r))}</td>
              <td class="muted">${escapeHTML(recordAction(r))}${r.tid != null ? `<span class="record-origin">${r.action === "止盈成交" ? "止盈单成交" : "成交回执 · 意图未确认"}</span>` : '<span class="record-origin">系统订单</span>'}</td>
              <td>${fmtCompact(r.size)}</td>
              <td class="muted">—</td>
              <td>${pxCell}</td>
              <td><span class="muted">—</span></td>
              <td>${pnlCell}</td>
              <td>${status}</td>
            </tr>`;
          }
          if (f.kind === "filled") {
            status = f.partial ? '<span class="badge-mini warn">部分成交</span>' : '<span class="badge-mini ok">成交</span>';
            pxCell = fmtCompact(f.px);
            const cls = f.slip > 0.03 ? "neg" : f.slip < -0.03 ? "pos" : "muted";
            slipCell = `<span class="${cls}">${f.slip >= 0 ? "+" : ""}${f.slip.toFixed(3)}%</span>`;
            if (f.pnl != null) {
              pnlCell = `<span class="${f.pnl >= 0 ? "pos" : "neg"}">${moneySigned(f.pnl)}<span class="record-origin">估算</span></span>`;
            }
          } else if (f.kind === "unfilled") {
            status = '<span class="badge-mini warn">未成交</span>';
          } else if (f.kind === "failed") {
            status = `<span class="badge-mini err" title="${escapeHTML(f.msg)}">失败</span>`;
          } else if (f.kind === "plan") {
            status = '<span class="badge-mini mute">计划</span>';
          }
          // 方向要显示「持仓方向」：平仓时下单方向与持仓方向相反
          const closing = r.reduce_only || /平仓|减仓/.test(r.action || "");
          const posLong = r.side === "买" ? !closing : closing;
          const note =
            f.kind === "failed" && f.msg
              ? `<div class="muted" style="font-size:11px;max-width:220px;white-space:normal">${escapeHTML(f.msg)}</div>`
              : "";
          return `<tr>
            <td class="muted">${ts2m(r.ts)}</td>
            <td>${escapeHTML(r.coin)}</td>
            <td>${sideLabel(posLong ? "long" : "short", closing)}</td>
            <td class="muted">${escapeHTML(recordAction(r))}${r.tid != null ? `<span class="record-origin">${r.action === "止盈成交" ? "止盈单成交" : "成交回执 · 意图未确认"}</span>` : '<span class="record-origin">系统订单</span>'}</td>
            <td>${fmtCompact(r.size)}</td>
            <td class="muted">${fmtCompact(r.price)}</td>
            <td>${pxCell}</td>
            <td>${slipCell}</td>
            <td>${pnlCell}</td>
            <td>${status}${note}</td>
          </tr>`;
        })
        .join("")}</tbody></table></div>
      ${pager("live", recs.length, livePage, LIVE_PAGE_SIZE)}`
    : '<div class="empty">还没有下单记录</div>';
  }
}

// Convert every observation before windowing: earlier cash flows also affect a short window.
function strategyHistory(history, data) {
  const baseline = data?.capital_baseline;
  if (!baseline || !Number.isFinite(baseline.equity) || baseline.equity <= 0 ||
      (baseline.account || "").toLowerCase() !== (data.account || "").toLowerCase()) return { error: "缺少有效的账户记录起点，暂时无法计算策略盈亏。", history: [] };
  const flows = [...(data.capital_flows || [])];
  const net = data.net_deposit ?? 0;
  if (!Number.isFinite(net) || flows.some(f => !Number.isFinite(f.amount) || !Number.isFinite(f.effective_ts ?? f.ts)) ||
      Math.abs(flows.reduce((sum,f) => sum + f.amount, 0) - net) > 0.005) return { error: "出入金明细与累计金额不一致，请先核对资金流水。", history: [] };
  flows.sort((a,b) => (a.effective_ts ?? a.ts) - (b.effective_ts ?? b.ts));
  const observations = [...history].filter(p => Number.isFinite(p.ts) && Number.isFinite(p.equity) && p.equity >= 0 && (!baseline.ts || p.ts >= baseline.ts)).sort((a,b) => a.ts-b.ts);
  let i = 0, cash = 0;
  const adjusted = observations.map(p => {
    while (i < flows.length && (flows[i].effective_ts ?? flows[i].ts) <= p.ts) cash += flows[i++].amount;
    return { ts:p.ts, equity:p.equity - baseline.equity - cash };
  });
  return { history:adjusted, estimated:flows.some(f => f.effective_ts == null) };
}

// Window selection and time aggregation are independent of the drawing code.
function equityWindow(history, range, now) {
  const hours = { day: 24, week: 168, month: 720 };
  const cutoff = hours[range] ? now - hours[range] * 3600000 : -Infinity;
  const unique = new Map();
  for (const p of history || []) {
    if (Number.isFinite(p.ts) && Number.isFinite(p.equity) && p.ts <= now && p.ts >= cutoff) unique.set(p.ts, p);
  }
  const raw = [...unique.values()].sort((a, b) => a.ts - b.ts);
  if (raw.length < 2) return { raw, points: raw };
  const interval = range === "day" ? 3600000 : range === "week" ? 3 * 3600000 : range === "month" ? 6 * 3600000 : Math.max(3600000, Math.ceil((raw.at(-1).ts - raw[0].ts) / 180));
  const buckets = new Map();
  for (const p of raw) buckets.set(Math.floor(p.ts / interval), p);
  // Preserve exact first/last observed values, without filling missing periods.
  const points = [raw[0], ...buckets.values()].filter((p, i, list) => !i || p.ts !== list[i - 1].ts);
  return { raw, points };
}


// 合并两个子账户的净值历史（严格按同小时对齐并过滤跨账本异步划转毛刺）——「全部账户」曲线。
function mergeHistory(a, b) {
  const clean = arr => (arr || []).filter(p => Number.isFinite(p.ts) && Number.isFinite(p.equity)).sort((x, y) => x.ts - y.ts);
  const aS = clean(a), bS = clean(b);
  if (!aS.length) return bS;
  if (!bS.length) return aS;
  const bStartHour = Math.floor(bS[0].ts / 3600000);
  const aBuckets = new Map();
  for (const p of aS) aBuckets.set(Math.floor(p.ts / 3600000), p);
  const bBuckets = new Map();
  for (const p of bS) bBuckets.set(Math.floor(p.ts / 3600000), p);
  const out = [];
  for (const [h, ap] of [...aBuckets.entries()].sort((x, y) => x[0] - y[0])) {
    if (h < bStartHour) {
      out.push({ ts: ap.ts, equity: ap.equity + bS[0].equity, sync: true });
    } else {
      const bp = bBuckets.get(h);
      if (bp) {
        const sync = Math.abs(ap.ts - bp.ts) <= 120000;
        out.push({ ts: Math.max(ap.ts, bp.ts), equity: ap.equity + bp.equity, sync });
      }
    }
  }
  // 过滤异步采样（>2分钟）导致的孤立或连续多点划转尖刺（跳变 > $220 且在 8 个点内回落）
  const cleaned = [];
  for (let i = 0; i < out.length; i++) {
    const cur = out[i];
    if (cleaned.length > 0) {
      const prev = cleaned[cleaned.length - 1];
      const jump = cur.equity - prev.equity;
      if (Math.abs(jump) > 220) {
        let reverted = false;
        for (let j = i + 1; j < Math.min(out.length, i + 8); j++) {
          const back = out[j].equity - cur.equity;
          if (back * jump < 0 && Math.abs(out[j].equity - prev.equity) <= Math.abs(jump) * 0.45) {
            reverted = true;
            break;
          }
        }
        if (reverted || !cur.sync) continue;
      }
    }
    cleaned.push({ ts: cur.ts, equity: cur.equity });
  }
  return cleaned;
}

// 根据当前策略 tab 取曲线数据：all = A+B 合并，ml = ML，momentum = 动量
function liveHist(d) {
  if (!d) return [];
  const mlHist = d.ml_history || [];
  const isMl = currentStrategy === 'ml' && d.ml_account && !d.ml_account.error;
  if (currentStrategy === 'all') return mergeHistory(d.history || [], mlHist);
  if (isMl) return mlHist;
  return d.history || [];
}

// 统一渲染监控曲线（切换周期 / 数据刷新都走这里 ✗ 避免用错 hist）
function renderMonitorChart(d) {
  if (!d) return;
  renderLiveChart(liveHist(d), d.shadow);
}

function renderLiveChart(history, shadow) {
  const el = $("mo-chart");
  const now = Date.now();
  const observations = [...(history || [])];
  // 「全部账户」= A+B 合计；「ML」= ML 子账户自己的净值和基准
  const isAll = currentStrategy === 'all';
  const mlAccC = lastLiveData && lastLiveData.ml_account && !lastLiveData.ml_account.error ? lastLiveData.ml_account : null;
  const isMl = currentStrategy === 'ml' && !!mlAccC;
  if (isMl && liveFetchedAt > 0 && Number.isFinite(mlAccC.equity)) {
    // ML 也必须追加当前快照；否则小时历史里的一个异常点会一直作为曲线末端。
    observations.push({ ts: liveFetchedAt, equity: mlAccC.equity });
  } else if (lastLiveData && !lastLiveData.error && Number.isFinite(lastLiveData.equity) && liveFetchedAt > 0) {
    const curEq = (isAll && mlAccC && Number.isFinite(mlAccC.equity))
      ? lastLiveData.equity + mlAccC.equity
      : lastLiveData.equity;
    observations.push({ ts: liveFetchedAt, equity: curEq });
  }
  let ldForBaseline = lastLiveData || {};
  const { externalFlows, extNetDep, mlFlows, mlNetDep } = splitCapitalFlows(lastLiveData);
  if (isAll && mlAccC) {
    const aBase = (lastLiveData.capital_baseline || {}).equity || 0;
    const bBase = ((lastLiveData.ml_history || [])[0] || {}).equity || 0;
    ldForBaseline = {
      ...lastLiveData,
      capital_baseline: { ...(lastLiveData.capital_baseline || {}), equity: aBase + bBase },
      capital_flows: externalFlows,
      net_deposit: extNetDep,
    };
  } else if (isMl) {
    // ML 用自己的起始净值做基准，并扣除「子账户互转」的 ML 侧出入金
    const mlHistArr = lastLiveData.ml_history || [];
    const mlBase = (mlHistArr[0] || {}).equity || 0;
    ldForBaseline = {
      capital_baseline: { equity: mlBase, account: mlAccC.account },
      account: mlAccC.account,
      capital_flows: mlFlows,
      net_deposit: mlNetDep,
    };
  }
  const adjusted = strategyHistory(observations, ldForBaseline);
  if (adjusted.error) { el.innerHTML = `<div class="empty">${escapeHTML(adjusted.error)}</div>`; return; }
  const { raw, points } = equityWindow(adjusted.history, chartRange, now);
  const names = { day: "最近 24 小时", week: "最近 7 天", month: "最近 30 天", all: "全部留存记录" };
  if (!raw.length) {
    el.innerHTML = `<div class="empty">${names[chartRange]}暂无策略盈亏记录。可以切换更长区间查看。</div>`;
    return;
  }
  const first = raw[0], last = raw.at(-1), change = last.equity - first.equity;
  const W = Math.max(260, el.clientWidth || 960), H = 280;
  const padL = 64, padR = 22, padT = 24, padB = 36;
  const values = raw.map(p => p.equity);
  const ax = niceAxis(Math.min(...values), Math.max(...values), 4);
  const span = ax.hi - ax.lo || 1;
  const X = ts => padL + (last.ts === first.ts ? 0.5 : (ts - first.ts) / (last.ts - first.ts)) * (W - padL - padR);
  const Y = v => padT + (1 - (v - ax.lo) / span) * (H - padT - padB);
  const money = v => (v < 0 ? "−" : "+") + "$" + Math.abs(v).toFixed(2);
  const grid = ax.lines.map(v => `<line x1="${padL}" y1="${Y(v)}" x2="${W-padR}" y2="${Y(v)}" stroke="var(--grid-line, rgba(255,255,255,0.06))"/><text x="${padL-10}" y="${Y(v)+4}" text-anchor="end" font-size="11" font-family="var(--font-mono)" fill="#64748b">$${v.toFixed(span < 10 ? 2 : 0)}</text>`).join("");
  const label = ts => chartRange === "day" ? new Date(ts).toISOString().slice(11,16) : ts2m(ts).slice(0,5);
  const ticks = last.ts === first.ts ? [first.ts] : [first.ts, first.ts + (last.ts-first.ts)/2, last.ts];
  const xlabels = ticks.map((ts,i) => `<text x="${X(ts)}" y="${H-10}" text-anchor="${i === 0 ? "start" : i === ticks.length-1 ? "end" : "middle"}" font-size="11" font-family="var(--font-mono)" fill="#64748b">${label(ts)}</text>`).join("");
  const line = points.map(p => `${X(p.ts)},${Y(p.equity)}`).join(" ");
  el.innerHTML = `<div class="chart-summary"><div><span class="chart-eyebrow">累计策略盈亏 · 区间末值</span><strong>${money(last.equity)}</strong></div><div><span class="chart-eyebrow">区间策略盈亏</span><strong class="${change >= 0 ? "pos" : "neg"}">${change >= 0 ? "+" : "−"}$${Math.abs(change).toFixed(2)}</strong></div><div class="chart-coverage"><span class="chart-eyebrow">实际记录区间 · UTC</span>${ts2m(first.ts)} — ${ts2m(last.ts)}</div></div>
    <div style="position:relative">
      <svg viewBox="0 0 ${W} ${H}" role="img" aria-label="${names[chartRange]}策略盈亏曲线">
        <defs><linearGradient id="equity-area" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="#10b981" stop-opacity="0.24"/><stop offset="100%" stop-color="#10b981" stop-opacity="0"/></linearGradient></defs>
        ${grid}
        ${points.length > 1 ? `<polygon points="${X(first.ts)},${H-padB} ${line} ${X(last.ts)},${H-padB}" fill="url(#equity-area)"/>` : ""}
        <line x1="${padL}" y1="${Y(first.equity)}" x2="${W-padR}" y2="${Y(first.equity)}" stroke="rgba(148,163,184,0.3)" stroke-dasharray="4 5"/>
        <polyline points="${line}" fill="none" stroke="#10b981" stroke-width="2.2" stroke-linejoin="round"/>
        <circle cx="${X(last.ts)}" cy="${Y(last.equity)}" r="4.5" fill="#34d399" stroke="var(--surface, #0e131b)" stroke-width="2"/>
        <line class="mo-xhair" x1="0" y1="${padT}" x2="0" y2="${H-padB}" stroke="var(--xhair, rgba(255,255,255,0.22))" stroke-dasharray="3 3" style="display:none"/>
        <circle class="mo-xdot" cx="0" cy="0" r="4.5" fill="#34d399" stroke="var(--surface, #0e131b)" stroke-width="2" style="display:none"/>
        ${xlabels}
      </svg>
      <div class="chart-tooltip" style="display:none"></div>
    </div><div class="chart-caption"><span><i></i>策略盈亏 <span class="muted">· 虚线为区间起点</span></span><span>${raw.length === 1 ? "仅一条记录，等待下一次采样" : "移动鼠标查看精确采样值"} · 已扣登记出入金，含手续费与资金费</span></div>${adjusted.estimated ? '<p class="note">旧流水按登记时间估算；如为事后补记，请在资金流水中校正到账时间，避免历史曲线出现错位跳变。</p>' : ""}`;
  const svg = el.querySelector("svg");
  const tip = el.querySelector(".chart-tooltip");
  const xhair = el.querySelector(".mo-xhair");
  const xdot = el.querySelector(".mo-xdot");
  if (svg && tip && xhair && xdot && points.length) {
    svg.addEventListener("mousemove", (ev) => {
      const rect = svg.getBoundingClientRect();
      const mx = ((ev.clientX - rect.left) / rect.width) * W;
      let best = points[0], bestDist = Infinity;
      for (const p of points) {
        const d = Math.abs(X(p.ts) - mx);
        if (d < bestDist) { bestDist = d; best = p; }
      }
      const cx = X(best.ts), cy = Y(best.equity);
      xhair.setAttribute("x1", cx); xhair.setAttribute("x2", cx); xhair.style.display = "";
      xdot.setAttribute("cx", cx); xdot.setAttribute("cy", cy); xdot.style.display = "";
      tip.style.display = "block";
      tip.style.left = `${(cx / W) * 100}%`;
      tip.style.top = `${(cy / H) * 100}%`;
      tip.innerHTML = `<span class="muted">${ts2m(best.ts)} UTC</span> · <b class="${best.equity >= 0 ? 'pos' : 'neg'}">${money(best.equity)}</b>`;
    });
    svg.addEventListener("mouseleave", () => {
      xhair.style.display = "none"; xdot.style.display = "none"; tip.style.display = "none";
    });
  }
}


// 手机端省电：页面切到后台就停止轮询，切回来立刻拉一次
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) refreshMonitor();
});

async function refreshHealth() {
  try {
    lastHealth = await fetchJSONRetry("/api/strategy-health", {}, 20000);
  } catch (e) {}
}

async function refreshMonitor(force) {
  if (liveBusy) return;
  if (document.hidden && !force) return;
  try {
    // 健康度是重计算（要跑 150 天 IC ✗ 后端缓存到 5 分钟一次即可）
    if (!lastHealth || Date.now() - (lastHealth._at || 0) > 300000) {
      const h = await fetchJSONRetry("/api/strategy-health", {}, 30000).catch(() => null);
      if (h) {
        h._at = Date.now();
        lastHealth = h;
      }
    }
    const d = await fetchLive();
    liveFetchedAt = Date.now();
    if (!liveLoaded) {
      lvConfigInto(d);
      liveLoaded = true;
    }
    renderMonitor(d);
  } catch (e) {
    $("mo-metrics").innerHTML = `<div class="note neg">读取实盘状态失败：${e}</div>`;
  }
}

$("mo-refresh").addEventListener("click", () => refreshMonitor(true));
$("mo-chart-tabs").addEventListener("click", event => {
  const button = event.target.closest("button[data-range]");
  if (!button) return;
  chartRange = button.dataset.range;
  $("mo-chart-tabs").querySelectorAll("button").forEach(b => b.setAttribute("aria-pressed", String(b === button)));
  if (lastLiveData) renderMonitorChart(lastLiveData);
});
$("mo-record-tabs").addEventListener("click", event => {
  const button = event.target.closest("button[data-scope]");
  if (!button) return;
  recordScope = button.dataset.scope;
  livePage = 0;
  $("mo-record-tabs").querySelectorAll("button").forEach(b => b.setAttribute("aria-pressed", String(b === button)));
  if (lastLiveData) renderMonitor(lastLiveData);
});

// ---------- 启动 ----------
refreshStatus();
setInterval(refreshStatus, 5000);
setInterval(() => {
  const el = $("mo-ago");
  if (el && liveFetchedAt) el.textContent = `数据更新于 ${Math.round((Date.now() - liveFetchedAt) / 1000)} 秒前`;
}, 1000);
// 每次轮询只打 3 个接口（账户 + 现货 + 批量价格），20 秒足够实时且远离限流。
refreshLiveConfig();
refreshMonitor(true);
setInterval(refreshMonitor, 20000);

// ---------- 资金费套利面板 ----------
// 带超时的 fetch：服务器偶尔会被后台写盘卡住 20~30 秒，不能把 UI 一起拖死。
async function fetchJSON(url, opts = {}, timeoutMs = 8000) {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), timeoutMs);
  try {
    const res = await fetch(url, { ...opts, cache: "no-store", signal: ctrl.signal });
    let body = null;
    try { body = await res.json(); } catch {}
    if (!res.ok) {
      const detail = body && typeof body.error === "string" ? body.error : "HTTP " + res.status;
      throw new Error(detail);
    }
    return body;
  } finally {
    clearTimeout(timer);
  }
}
async function fetchJSONRetry(url, opts = {}, timeoutMs = 8000, tries = 2) {
  let last;
  for (let i = 0; i < tries; i++) {
    try {
      return await fetchJSON(url, opts, timeoutMs);
    } catch (e) {
      last = e;
      if (i + 1 < tries) await new Promise((r) => setTimeout(r, 1200));
    }
  }
  throw last;
}


const usdM = (v) => Number.isFinite(v) ? (v >= 1e6 ? "$" + (v / 1e6).toFixed(1) + "M" : "$" + (v / 1e3).toFixed(0) + "K") : "—";

// 资金费差：只显示摘要 + 前 5 个机会，全表折叠。





// 切到资金费套利视图时刷新（资金矩阵已并入 renderMonitor 的 20 秒轮询）。
// 组合策略 tab：进入时拉一次 ✗ 停留时每 60 秒刷新
document.querySelectorAll('.nav button[data-view="combine"]').forEach((b) => {
  b.addEventListener("click", () => refreshCombine());
});
document.querySelectorAll('.nav button[data-view="tradfi"]').forEach((b) => {
  b.addEventListener("click", () => refreshTradFi());
});
const cbLegTabs = $("cb-leg-tabs");
if (cbLegTabs) {
  cbLegTabs.addEventListener("click", (event) => {
    const btn = event.target.closest("button[data-leg]");
    if (!btn) return;
    cbLeg = btn.dataset.leg;
    cbLegTabs.querySelectorAll("button").forEach((b) => b.setAttribute("aria-pressed", String(b === btn)));
    if (lastCombineData) renderCombine(lastCombineData);
  });
}
const cbTabs = $("cb-chart-tabs");
if (cbTabs) {
  cbTabs.addEventListener("click", (event) => {
    const btn = event.target.closest("button[data-range]");
    if (!btn) return;
    cbRange = btn.dataset.range;
    cbTabs.querySelectorAll("button").forEach((b) => b.setAttribute("aria-pressed", String(b === btn)));
    if (lastCombineData) renderCombine(lastCombineData);
  });
}
const combineRefreshButton = $("cb-refresh");
if (combineRefreshButton) combineRefreshButton.addEventListener("click", () => refreshCombine());
setInterval(() => {
  const v = $("view-combine");
  if (v && v.classList.contains("active")) refreshCombine();
}, 15000);

// 启动：首次拉取 + 周期轮询。
refreshHlCarry();
setInterval(refreshHlCarry, 120000);

// ---------- 纸面 carry 策略 ----------

// 纸面 carry：摘要 + 权益曲线 + 两腿汇总，明细折叠。
// ---------- 跨所价差纸面策略 ----------
let lastXData = null;





// 跨所价差（OKX vs 币安）。用户当年 G/USDT 那种机会。


// 曲线是"每小时一个点"✗ 两次采样之间线会停在旧值上 ✗ 看起来像"卡住"✗
// 这里把【当前权益】补成一个点 ✓ 线就永远画到"现在"✓
function withNow(history, equityNow) {
  const pts = (history || []).filter((p) => Array.isArray(p) && Number.isFinite(p[0]) && Number.isFinite(p[1]));
  if (!Number.isFinite(equityNow)) return pts;
  const now = Date.now();
  if (pts.length && now - pts[pts.length - 1][0] < 60_000) return pts;
  return [...pts, [now, equityNow]];
}

// 通用权益曲线（每小时一个点）。
// ---------- 组合策略（paper4 ✗ 日线动量 + 12h 区间位置）----------
function renderCombine(d) {
  d = d || {};
  const h = (d.history || []).filter((p) => Number.isFinite(p[0]) && Number.isFinite(p[1]));
  const cfg = d.config || {};
  const eq = Number.isFinite(d.equity) ? d.equity : 0;
  const pnl = Number.isFinite(d.pnl_pct) ? d.pnl_pct : 0;
  const days = d.days_elapsed || Math.max(0, h.length - 1);
  const legs = d.positions || [];
  const a = d.last_a || [];
  const b = d.last_b || [];
  const m = $("cb-metrics");
  if (m) {
    const sampleDays = Math.max(0, days - (cfg.replay_days || 0));
    const kpi = (label, value, detail, cls) =>
      `<div class="combine-kpi"><div class="combine-kpi-label">${label}</div><div class="combine-kpi-value ${cls || ""}">${value}</div>${detail ? `<div class="combine-kpi-detail">${detail}</div>` : ""}</div>`;
    m.innerHTML =
      kpi("组合净值", "$" + fmt(eq, 2), "纸面账户当前权益") +
      kpi("累计收益", (pnl >= 0 ? "+" : "−") + Math.abs(pnl).toFixed(2) + "%", "自组合起点", pnl >= 0 ? "pos" : "neg") +
      kpi("运行天数", `${days}<span class="metric-unit"> 天</span>`, cfg.replay_days ? `回放 ${cfg.replay_days} 天 · 样本外 ${sampleDays} 天` : "按已记录区间统计") +
      kpi(
        "当前持仓",
        `${legs.length}<span class="metric-unit"> 币</span>`,
        `动量 ${a.length} 币 · 区间 ${b.length} 币 · 浮动 ${moneySigned(Number(d.unrealized) || 0)}`
      );
  }
  const runState = $("cb-run-state");
  if (runState) {
    runState.className = "combine-state " + (d.running ? "is-running" : "is-stopped");
    runState.innerHTML = `<i></i>${d.running ? "纸面运行中" : "已停止"}`;
  }
  const st = $("cb-status");
  if (st) {
    if (!b.length) {
      st.textContent = "区间位置腿当前没有信号，当前持仓仅反映日线动量腿。";
      st.hidden = false;
    } else {
      st.textContent = "";
      st.hidden = true;
    }
  }
  if (h.length > 1) {
    // 与实盘同一套区间口径（日/周/月/全部 ✗ equityWindow 要 {ts,equity} 对象 ✓）
    const obs = h.map((p) => ({ ts: p[0], equity: p[1] }));
    let win = equityWindow(obs, cbRange, Date.now());
    let pts = win.points.map((p) => [p.ts, p.equity]);
    // 日频数据下"近 1 天/近 7 天"可能点太少 ✗ 回退到【最近一段】而不是全部 ✓
    if (pts.length < 3 && cbRange !== "all") {
      pts = h.slice(-3);
    }
    renderEquityChart("cb-chart", pts, "组合策略权益曲线", undefined, undefined, undefined, {
      equityLabel: "组合净值 · 最新", changeLabel: "记录区间变化", legendName: "组合策略"
    });
  } else {
    const el = $("cb-chart");
    if (el) el.innerHTML = '<div class="empty">还没有数据 ✗ 等下一轮步进</div>';
  }
  const lg = $("cb-legs");
  if (lg) {
    const posTable = (arr) => {
      if (!arr.length) return '<div class="empty">这条腿当前没有持仓</div>';
      const rows = arr.slice().sort((x, y) => (y.side - x.side) || String(x.coin).localeCompare(String(y.coin)));
      const sum = rows.reduce((acc, p) => acc + (Number(p.unrealized) || 0), 0);
      return '<div class="table-scroll"><table><thead><tr>' +
        '<th>币</th><th>方向</th><th>开仓价</th><th>名义</th><th>现价</th><th>浮盈</th>' +
        '</tr></thead><tbody>' +
        rows.map((p) => {
          const long = Number(p.side) > 0;
          const u = Number(p.unrealized) || 0;
          return `<tr><td><b>${escapeHTML(p.coin)}</b></td>` +
            `<td class="${long ? "pos" : "neg"}">${long ? "多" : "空"}</td>` +
            `<td>${fmtCompact(p.entry_px)}</td>` +
            `<td>$${fmt(p.notional, 0)}</td>` +
            `<td>${fmtCompact(p.mark_px)}</td>` +
            `<td class="${u >= 0 ? "pos" : "neg"}">${moneySigned(u)}</td></tr>`;
        }).join("") +
        '</tbody></table></div>' +
        `<div class="combine-leg-total">这条腿合计浮盈 <b class="${sum >= 0 ? "pos" : "neg"}">${moneySigned(sum)}</b></div>`;
    };
    const head = (title, cadence, n) =>
      `<div class="combine-leg-head"><span class="combine-leg-title">${title}</span>` +
      `<span class="combine-leg-cadence">${cadence} · ${n} 个仓位</span></div>`;
    lg.innerHTML =
      '<article class="combine-leg">' +
      (cbLeg === "range"
        ? head("12 小时区间位置", "每 12 小时调整", (d.b_pos || []).length) + posTable(d.b_pos || [])
        : head("日线动量", "每日调整", (d.a_pos || []).length) + posTable(d.a_pos || [])) +
      "</article>";
  }
  const posCount = $("cb-position-count");
  if (posCount) posCount.textContent = `${legs.length} 个合并后币种 · 多 ${legs.filter((x) => x[1] > 0).length} / 空 ${legs.filter((x) => x[1] < 0).length}`;
  const config = $("cb-config");
  if (config) {
    const momentumWeight = cfg.momentum_weight == null ? 1 : cfg.momentum_weight;
    const bWeight = cfg.b_weight == null ? 0.5 : cfg.b_weight;
    config.innerHTML = [
      `动量权重 ${momentumWeight.toFixed(2)}`,
      `区间权重 ${bWeight.toFixed(2)}`,
      `区间每边 ${cfg.range_k || 3} 币`,
      `杠杆 ${fmt(cfg.leverage || 2.7, 2)}×`,
      `最低成交额 $${fmt((cfg.min_vol_usd || 0) / 1e6, 0)}M`
    ].map((item) => `<span>${item}</span>`).join("");
  }
}

function renderIntraday(d) {
  d = d || {};
  const cfg = d.config || {}, ps = d.positions || [];
  const isA1w = cfg.strategy === "hl_a1w_short";
  const isLongPosition = p => Number(p.side) === 1 || p.side === "long";
  const eq = Number(d.equity) || 0, pnlPct = Number(d.pnl_pct) || 0;
  const pnl = Number(d.pnl) || 0, days = Number(d.days_elapsed) || 0;
  const kpi = (label, value, detail, cls) => `<div class="combine-kpi"><div class="combine-kpi-label">${label}</div><div class="combine-kpi-value ${cls || ""}">${value}</div>${detail ? `<div class="combine-kpi-detail">${detail}</div>` : ""}</div>`;
  const m = $("cb-metrics");
  if (m) m.innerHTML =
    kpi("影子净值", "$" + fmt(eq, 2), "独立模拟账户 · 本金 $" + fmt(d.capital || 0, 0)) +
    kpi("累计收益", moneySigned(pnl), (pnlPct >= 0 ? "+" : "") + pnlPct.toFixed(2) + "%", pnl >= 0 ? "pos" : "neg") +
    kpi("已实现 / 浮盈", moneySigned(d.realized_pnl || 0),
      "浮动 " + moneySigned(d.unrealized_pnl || 0) + (isA1w ? " · 资金费 " + moneySigned(d.total_funding || 0) : ""),
      (d.realized_pnl || 0) >= 0 ? "pos" : "neg") +
    kpi("当前持仓", String(ps.length), "多 " + ps.filter(p => isLongPosition(p)).length + " · 空 " + ps.filter(p => !isLongPosition(p)).length) +
    kpi("平仓胜率", (d.win_rate || 0).toFixed(1) + "%", (d.trades || []).length + " 笔已平仓") +
    kpi("运行时间", days.toFixed(1) + " 天", isA1w ? "Hyperliquid A1w · 纯前向" : "纯前向 · 无历史回放");
  const state = $("cb-run-state");
  const fresh = Number.isFinite(d.last_update) && Date.now() - d.last_update < 180000;
  if (state) {
    state.className = "combine-state " + (d.running && fresh ? "is-running" : "is-stopped");
    state.innerHTML = `<i></i>${d.running && fresh ? "影子运行中" : "数据过期 / 已停止"}`;
  }
  const st = $("cb-status");
  if (st) {
    st.textContent = d.last_error ? "最近采集异常：" + d.last_error + "。异常期间不会补造交易。" : "";
    st.hidden = !d.last_error;
  }
  const h = (d.history || []).filter(p => Array.isArray(p) && Number.isFinite(p[0]) && Number.isFinite(p[1]));
  if (h.length) {
    const obs = h.map(p => ({ts:p[0], equity:p[1]}));
    let pts = equityWindow(obs, cbRange, Date.now()).points.map(p => [p.ts, p.equity]);
    if (pts.length < 3 && cbRange !== "all") pts = h.slice(-3);
    renderEquityChart("cb-chart", withNow(pts, eq), "日内影子权益曲线", undefined, undefined, undefined,
      {equityLabel:"影子净值 · 最新",changeLabel:"记录区间变化",legendName:isA1w?"HL A1w 影子":"日内影子",useBjTime:true});
  } else if ($("cb-chart")) $("cb-chart").innerHTML = '<div class="empty">等待第一个前向净值点</div>';
  const legs = $("cb-legs");
  if (legs) {
    const posPages = Math.max(1, Math.ceil(ps.length / CB_PAGE_SIZE));
    if (cbPosPage > posPages - 1) cbPosPage = 0;
    const posRows = ps.slice(cbPosPage * CB_PAGE_SIZE, (cbPosPage + 1) * CB_PAGE_SIZE);
    if (isA1w) {
      legs.innerHTML = ps.length ? '<div class="table-scroll"><table><thead><tr><th>币</th><th>方向</th><th>入场年化费率</th><th>名义</th><th>开仓价</th><th>现价</th><th>15% 止损价 / 到期 (UTC+8)</th><th>已付资金费</th><th>浮盈</th></tr></thead><tbody>' +
        posRows.map(p => { const long=isLongPosition(p),u=Number(p.unrealized_pnl)||0,fu=Number(p.funding_usd)||0; return `<tr><td><b>${escapeHTML(p.coin)}</b></td><td class="${long?"pos":"neg"}">${long?"多":"空"}</td><td class="neg">${fmt(p.entry_fr_ann||0,1)}%</td><td>$${fmt(p.notional,0)}</td><td>${fmtCompact(p.entry_price)}</td><td>${fmtCompact(p.mark_price)}</td><td>${fmtCompact(p.stop_price)} / ${p.expiry_ts?ts2bj(p.expiry_ts):"—"}</td><td class="${fu>=0?"pos":"neg"}">${moneySigned(fu)}</td><td class="${u>=0?"pos":"neg"}">${moneySigned(u)}</td></tr>`; }).join("") + '</tbody></table></div>' + (ps.length > CB_PAGE_SIZE ? pager("cbpos", ps.length, cbPosPage, CB_PAGE_SIZE) : "")
        : '<div class="empty">当前空仓。等待 24h 涨幅 ≥ 8% 且年化负费率 ≤ −25% 的逼空衰竭信号。</div>';
    } else {
      legs.innerHTML = ps.length ? '<div class="table-scroll"><table><thead><tr><th>币</th><th>方向</th><th>模型评分</th><th>名义</th><th>开仓价</th><th>现价</th><th>止盈 / 止损</th><th>浮盈</th></tr></thead><tbody>' +
        posRows.map(p => { const long=isLongPosition(p),u=Number(p.unrealized_pnl)||0; return `<tr><td><b>${escapeHTML(p.coin)}</b></td><td class="${long?"pos":"neg"}">${long?"多":"空"}</td><td>${(100*(p.probability||0)).toFixed(1)}%</td><td>$${fmt(p.notional,0)}</td><td>${fmtCompact(p.entry_price)}</td><td>${fmtCompact(p.mark_price)}</td><td>${fmtCompact(p.target_price)} / ${fmtCompact(p.stop_price)}</td><td class="${u>=0?"pos":"neg"}">${moneySigned(u)}</td></tr>`; }).join("") + '</tbody></table></div>' + (ps.length > CB_PAGE_SIZE ? pager("cbpos", ps.length, cbPosPage, CB_PAGE_SIZE) : "")
        : '<div class="empty">当前空仓。没有足够强的趋势延续信号就不出手。</div>';
    }
  }
  if ($("cb-position-count")) $("cb-position-count").textContent = `${ps.length} / ${cfg.max_positions||4} 仓 · 毛敞口 $${fmt(d.gross_notional||0,0)}`;
  if ($("cb-config")) {
    const chips = isA1w
      ? [
          `单仓 ${Math.round(100*(cfg.position_fraction||.15))}%`,
          `最多 ${cfg.max_positions||4} 仓`,
          `24h涨 ≥ ${Math.round(100*(cfg.ret24_min||.08))}%`,
          `年化费率 ≤ ${cfg.fr_ann_max ?? -25}%`,
          `成交额 ≥ $${fmt((cfg.min_vol_usd||2e6)/1e6,0)}M`,
          `${Math.round(100*(cfg.stop_loss_pct||.15))}% 宽止损`,
          `${cfg.cooldown_hours||24}h 单币冷却`,
          `${cfg.max_holding_hours||8}h 平仓`,
        ]
      : [
          `单仓 ${100*(cfg.position_fraction||.25)}%`,
          `最多 ${cfg.max_positions||4} 仓`,
          `模型评分 ≥ ${cfg.threshold||.55}`,
          `成本 ${10000*(cfg.fee_per_side||.0007)}bp/边`,
          `最长 ${cfg.max_holding_hours||6} 小时`,
        ];
    $("cb-config").innerHTML = chips.map(x=>`<span>${x}</span>`).join("");
  }
  const sigs = (d.signals || []).slice().reverse();
  if ($("cb-signals")) {
    const sigPages = Math.max(1, Math.ceil(sigs.length / CB_PAGE_SIZE));
    if (cbSigPage > sigPages - 1) cbSigPage = 0;
    const sigRows = sigs.slice(cbSigPage * CB_PAGE_SIZE, (cbSigPage + 1) * CB_PAGE_SIZE);
    if (isA1w) {
      $("cb-signals").innerHTML = sigs.length ? '<div class="table-scroll"><table><thead><tr><th>时间 · UTC+8</th><th>币</th><th>方向</th><th>24h涨幅</th><th>年化费率</th><th>24h成交额</th><th>处理</th></tr></thead><tbody>'+sigRows.map(s=>`<tr><td class="muted">${ts2bj(s.ts)}</td><td>${escapeHTML(s.coin)}</td><td class="${s.side==="long"?"pos":"neg"}">${s.side==="long"?"多":"空"}</td><td class="pos">+${(100*(s.ret24||0)).toFixed(1)}%</td><td class="${(s.fr_ann||0)<= -25?"neg":"muted"}">${fmt(s.fr_ann||0,1)}%</td><td>${usdM(s.vol24||0)}</td><td class="${s.accepted?"pos":"muted"}">${escapeHTML(s.reason)}</td></tr>`).join("")+'</tbody></table></div>' + pager("cbsig", sigs.length, cbSigPage, CB_PAGE_SIZE) : '<div class="empty">尚无信号记录，每小时整点扫描一次。</div>';
    } else {
      $("cb-signals").innerHTML = sigs.length ? '<div class="table-scroll"><table><thead><tr><th>时间 · UTC+8</th><th>币</th><th>方向</th><th>24h涨跌</th><th>4h OI</th><th>模型评分</th><th>处理</th></tr></thead><tbody>'+sigRows.map(s=>`<tr><td class="muted">${ts2bj(s.ts)}</td><td>${escapeHTML(s.coin)}</td><td class="${s.side==="long"?"pos":"neg"}">${s.side==="long"?"多":"空"}</td><td>${(100*(s.ret24||0)).toFixed(1)}%</td><td>${(100*(s.oi_chg_4h||0)).toFixed(1)}%</td><td>${(100*(s.probability||0)).toFixed(1)}%</td><td class="${s.accepted?"pos":"muted"}">${escapeHTML(s.reason)}</td></tr>`).join("")+'</tbody></table></div>' + pager("cbsig", sigs.length, cbSigPage, CB_PAGE_SIZE) : '<div class="empty">尚无信号记录，每小时检查一次。</div>';
    }
  }
  const groupedTrades = [];
  const tradeGroups = new Map();
  for (const t of (d.trades || [])) {
    const key = `${t.coin || ""}|${t.side || ""}|${t.entry_ts ?? t.entry_price ?? ""}`;
    let g = tradeGroups.get(key);
    if (!g) {
      g = {
        coin: t.coin,
        side: t.side,
        entry_ts: t.entry_ts,
        entry_price: t.entry_price,
        probability: t.probability,
        entry_fr_ann: t.entry_fr_ann,
        funding_usd: 0,
        exit_ts: t.exit_ts || 0,
        pnl_usd: 0,
        notional_sum: 0,
        weighted_exit: 0,
        legs: [],
      };
      tradeGroups.set(key, g);
      groupedTrades.push(g);
    }
    const n = Number(t.notional) || 0;
    const px = Number(t.exit_price) || 0;
    g.pnl_usd += Number(t.pnl_usd) || 0;
    g.funding_usd += Number(t.funding_usd) || 0;
    g.notional_sum += n;
    g.weighted_exit += px * n;
    if ((t.exit_ts || 0) >= g.exit_ts) g.exit_ts = t.exit_ts || 0;
    g.legs.push(t);
  }
  const trades = groupedTrades.sort((a, b) => (b.exit_ts || 0) - (a.exit_ts || 0));
  if ($("cb-trades")) {
    const trPages = Math.max(1, Math.ceil(trades.length / CB_PAGE_SIZE));
    if (cbTradePage > trPages - 1) cbTradePage = 0;
    const trRows = trades.slice(cbTradePage * CB_PAGE_SIZE, (cbTradePage + 1) * CB_PAGE_SIZE);
    if (isA1w) {
      $("cb-trades").innerHTML = trades.length ? '<div class="table-scroll"><table><thead><tr><th>平仓时间 · UTC+8</th><th>币</th><th>方向</th><th>开仓 / 平仓</th><th>入场费率</th><th>实付资金费</th><th>净盈亏</th><th>原因</th></tr></thead><tbody>'+trRows.map(t=>{
        const avgExit = t.notional_sum > 0 ? t.weighted_exit / t.notional_sum : (t.legs[t.legs.length - 1]?.exit_price || 0);
        const reason = t.legs[t.legs.length - 1]?.reason || "平仓";
        return `<tr><td class="muted">${ts2bj(t.exit_ts)}</td><td>${escapeHTML(t.coin)}</td><td class="${t.side==="long"?"pos":"neg"}">${t.side==="long"?"多":"空"}</td><td>${fmtCompact(t.entry_price)} / ${fmtCompact(avgExit)}</td><td class="neg">${fmt(t.entry_fr_ann||0,1)}%</td><td class="${t.funding_usd>=0?"pos":"neg"}">${moneySigned(t.funding_usd)}</td><td class="${t.pnl_usd>=0?"pos":"neg"}">${moneySigned(t.pnl_usd)}</td><td>${escapeHTML(reason)}</td></tr>`;
      }).join("")+'</tbody></table></div>' + pager("cbtr", trades.length, cbTradePage, CB_PAGE_SIZE) : '<div class="empty">暂无已平仓模拟成交。</div>';
    } else {
      $("cb-trades").innerHTML = trades.length ? '<div class="table-scroll"><table><thead><tr><th>平仓时间 · UTC+8</th><th>币</th><th>方向</th><th>开仓 / 平仓</th><th>模型评分</th><th>净盈亏</th><th>原因</th></tr></thead><tbody>'+trRows.map(t=>{
        t.legs.sort((a, b) => (a.exit_ts || 0) - (b.exit_ts || 0));
        const avgExit = t.notional_sum > 0 ? t.weighted_exit / t.notional_sum : (t.legs[t.legs.length - 1]?.exit_price || 0);
        const reasons = t.legs.map(l => l.reason || "平仓");
        const summaryReason = reasons.length > 1 ? reasons.join(" → ") : reasons[0];
        const reasonCell = t.legs.length > 1
          ? `<span class="trade-legs-wrap" tabindex="0"><span>${escapeHTML(summaryReason)}</span><span class="trade-legs-badge">${t.legs.length}笔</span><div class="trade-legs-pop">${
              (t.entry_ts ? `<div>开仓 ${ts2bj(t.entry_ts)} · 均价 ${fmtCompact(t.entry_price)}</div>` : "") +
              t.legs.map((l, idx) => `<div>${idx + 1}. ${escapeHTML(l.reason || "平仓")} · ${fmtCompact(l.exit_price)} · <span class="${(l.pnl_usd||0)>=0?"pos":"neg"}">${moneySigned(l.pnl_usd||0)}</span> · ${ts2bj(l.exit_ts)}</div>`).join("")
            }</div></span>`
          : escapeHTML(summaryReason);
        return `<tr><td class="muted">${ts2bj(t.exit_ts)}</td><td>${escapeHTML(t.coin)}</td><td class="${t.side==="long"?"pos":"neg"}">${t.side==="long"?"多":"空"}</td><td>${fmtCompact(t.entry_price)} / ${fmtCompact(avgExit)}</td><td>${(100*(t.probability||0)).toFixed(1)}%</td><td class="${t.pnl_usd>=0?"pos":"neg"}">${moneySigned(t.pnl_usd)}</td><td>${reasonCell}</td></tr>`;
      }).join("")+'</tbody></table></div>' + pager("cbtr", trades.length, cbTradePage, CB_PAGE_SIZE) : '<div class="empty">暂无已平仓模拟成交。</div>';
    }
  }
}

async function refreshCombine() {
  try {
    const d = await fetchJSONRetry("/api/intraday", {}, 15000);
    lastCombineData = d;
    renderIntraday(d);
  } catch (e) {
    const st = $("cb-status");
    if (st) {
      st.textContent = "读取日内影子失败：" + ((e && e.message) || e);
      st.hidden = false;
    }
  }
}

function renderTradFi(d) {
  d = d || {};
  const cfg = d.config || {};
  const ps = Array.isArray(d.positions_list) ? d.positions_list : Object.values(d.positions || {});
  const eq = Number(d.equity) || 0;
  const pnl = Number(d.pnl) || 0;
  const pnlPct = Number(d.pnl_pct) || 0;
  const days = Number(d.days_elapsed) || 0;
  const kpi = (label, value, detail, cls) =>
    `<div class="combine-kpi"><div class="combine-kpi-label">${label}</div><div class="combine-kpi-value ${cls || ""}">${value}</div>${detail ? `<div class="combine-kpi-detail">${detail}</div>` : ""}</div>`;
  const tfLegBadge = (leg) => {
    const s = String(leg || "");
    if (s === "stock_a4") return '<span class="badge-mini ok">美股A4</span>';
    if (s === "comm_ema") return '<span class="badge-mini warn">商品S4波段</span>';
    if (s === "vbounce") return '<span class="badge-mini" style="background:rgba(56,189,248,.14);color:var(--cyan);border:1px solid rgba(56,189,248,.28)">超跌抄底</span>';
    if (s === "stock_a4+vbounce") return '<span class="badge-mini ok">美股A4+抄底</span>';
    if (s === "comm_ema+vbounce") return '<span class="badge-mini warn">商品S4+抄底</span>';
    return '<span class="badge-mini mute">' + escapeHTML(s || "—") + '</span>';
  };
  const nStock = ps.filter(p => String(p.leg || "").startsWith("stock_a4")).length;
  const nComm = ps.filter(p => String(p.leg || "").startsWith("comm_ema")).length;
  const nVb = ps.filter(p => String(p.leg || "").includes("vbounce")).length;
  const m = $("tf-metrics");
  if (m) m.innerHTML =
    kpi("影子净值", "$" + fmt(eq, 2), "独立模拟账户 · 本金 $" + fmt(d.capital || 0, 0)) +
    kpi("累计收益", moneySigned(pnl), (pnlPct >= 0 ? "+" : "") + pnlPct.toFixed(2) + "%", pnl >= 0 ? "pos" : "neg") +
    kpi("已实现 / 浮盈", moneySigned(d.realized_pnl || 0), "浮动 " + moneySigned(d.unrealized_pnl || 0) + " · 资金费 " + moneySigned(d.total_funding || 0), (d.realized_pnl || 0) >= 0 ? "pos" : "neg") +
    kpi("当前持仓", String(ps.length), "美股 " + nStock + " · 商品 " + nComm + (nVb ? " · 抄底 " + nVb : "")) +
    kpi("毛敞口 / 净敞口", "$" + fmt(d.gross_notional || 0, 0), "净额 " + moneySigned(d.net_notional || 0) + " · 费用 −$" + fmt(d.total_fees || 0, 2)) +
    kpi("运行时间", days.toFixed(1) + " 天", "上次调仓 " + (d.last_rebalance_day || "—"));
  const state = $("tf-run-state");
  const fresh = Number.isFinite(d.last_update) && Date.now() - d.last_update < 600000;
  if (state) {
    state.className = "combine-state " + (d.running && fresh ? "is-running" : "is-stopped");
    state.innerHTML = `<i></i>${d.running && fresh ? "影子运行中" : "数据过期 / 未启动"}`;
  }
  const st = $("tf-status");
  if (st) {
    const brk = d.breaker_tripped ? "⚠ 盘中回撤熔断已触发并自动平仓（需点「按本金重置影子」恢复）。" : "";
    const coolSyms = Object.keys(d.stop_cooldowns || {});
    const cool = coolSyms.length ? "盘中止损冷却中（24h）：" + coolSyms.join(", ") + "。" : "";
    const blocked = (d.blocked_symbols || []).length ? "拆股防护已屏蔽：" + d.blocked_symbols.join(", ") + "。" : "";
    const err = d.last_error ? "最近采集异常：" + d.last_error : "";
    const msg = [brk, cool, err, blocked].filter(Boolean).join(" ");
    st.textContent = msg;
    st.hidden = !msg;
  }
  const h = (d.history || []).filter(p => Array.isArray(p) && Number.isFinite(p[0]) && Number.isFinite(p[1]));
  if (h.length) {
    const obs = h.map(p => ({ ts: p[0], equity: p[1] }));
    let pts = equityWindow(obs, tfRange, Date.now()).points.map(p => [p.ts, p.equity]);
    if (pts.length < 3 && tfRange !== "all") pts = h.slice(-3);
    renderEquityChart("tf-chart", withNow(pts, eq), "TradFi 双腿影子权益曲线", undefined, undefined, undefined,
      { equityLabel: "影子净值 · 最新", changeLabel: "记录区间变化", legendName: "TradFi 双腿", useBjTime: true });
  } else if ($("tf-chart")) {
    $("tf-chart").innerHTML = '<div class="empty">等待第一个前向净值点</div>';
  }
  const posEl = $("tf-positions");
  if (posEl) {
    posEl.innerHTML = ps.length
      ? '<div class="table-scroll"><table><thead><tr><th>标的</th><th>所属腿</th><th>方向</th><th>目标权重</th><th>名义</th><th>开仓价</th><th>现价</th><th>资金费</th><th>浮盈</th></tr></thead><tbody>' +
        ps.map(p => {
          const isLong = p.side === "long";
          const u = Number(p.unrealized_pnl) || 0;
          const fu = Number(p.funding_usd) || 0;
          const legLabel = tfLegBadge(p.leg);
          return `<tr><td><b>${escapeHTML(p.symbol)}</b></td><td>${legLabel}</td><td class="${isLong ? "pos" : "neg"}">${isLong ? "多" : "空"}</td><td>${((Number(p.target_weight) || 0) * 100).toFixed(1)}%</td><td>$${fmt(p.notional || 0, 0)}</td><td>${fmtCompact(p.entry_price || 0)}</td><td>${fmtCompact(p.mark_price || 0)}</td><td class="${fu >= 0 ? "pos" : "neg"}">${moneySigned(fu)}</td><td class="${u >= 0 ? "pos" : "neg"}">${moneySigned(u)}</td></tr>`;
        }).join("") + '</tbody></table></div>'
      : '<div class="empty">暂无持仓，等待每日 UTC 00:05 调仓或首次初始化。</div>';
  }
  if ($("tf-position-count")) $("tf-position-count").textContent = `${ps.length} 个持仓 · 毛敞口 $${fmt(d.gross_notional || 0, 0)}`;
  if ($("tf-config")) {
    const chips = [
      `总杠杆 ${fmt(cfg.gross_leverage || 3.0, 1)}x`,
      `美股A4 ${Math.round(100 * (cfg.stock_alloc ?? 0.4))}%`,
      `4核心商品双周期(日线S4+4h) ${Math.round(100 * (cfg.comm_alloc ?? 0.4))}%`,
      `超跌抄底 ${Math.round(100 * (cfg.vbounce_alloc ?? 0.2))}%（闲置回流）`,
      (cfg.vol_target_ann ?? 0.4) > 0 ? `目标年化波动 ${Math.round(100 * (cfg.vol_target_ann ?? 0.4))}%` : `目标年化波动 关闭`,
      `新币冷启动 ≥ ${cfg.stock_min_bars || 7}d`,
      `盘中止损 -${fmt(cfg.pos_stop_pct ?? 15.0, 0)}%`,
      `移动止盈 +${fmt(cfg.trail_tp_activate_pct ?? 12.0, 0)}% / 回撤${fmt(cfg.trail_tp_callback_pct ?? 5.0, 0)}%`,
      `回撤熔断 ${fmt(cfg.max_dd_pct ?? 20.0, 0)}%`,
      `费用 ${fmt(cfg.fee_bp || 2.0, 1)}bp/边`,
      `拆股防护 ≥ ${Math.round(100 * (cfg.split_ret_threshold || 0.4))}%`,
    ];
    $("tf-config").innerHTML = chips.map(x => `<span>${x}</span>`).join("");
  }
  const trades = (d.trades || []).slice().reverse();
  if ($("tf-trades")) {
    const trPages = Math.max(1, Math.ceil(trades.length / TF_TRADE_PAGE_SIZE));
    if (tfTradePage > trPages - 1) tfTradePage = 0;
    const trRows = trades.slice(tfTradePage * TF_TRADE_PAGE_SIZE, (tfTradePage + 1) * TF_TRADE_PAGE_SIZE);
    $("tf-trades").innerHTML = trades.length
      ? '<div class="table-scroll"><table><thead><tr><th>时间 · UTC+8</th><th>标的</th><th>所属腿</th><th>动作</th><th>数量</th><th>成交价</th><th>名义</th><th>手续费</th><th>已实现盈亏</th><th>原因</th></tr></thead><tbody>' +
        trRows.map(t => {
          const legLabel = tfLegBadge(t.leg);
          const actCls = /止损|熔断/.test(t.action || "") ? "neg" : /止盈/.test(t.action || "") ? "pos" : (t.side === "long" ? "pos" : "neg");
          const rp = t.realized_pnl;
          const pnlCell = rp == null ? '<span class="muted">—</span>' : `<span class="${rp >= 0 ? "pos" : "neg"}">${moneySigned(rp)}</span>`;
          return `<tr><td class="muted">${ts2bj(t.ts)}</td><td><b>${escapeHTML(t.symbol)}</b></td><td>${legLabel}</td><td class="${actCls}">${escapeHTML(t.action || "—")}</td><td>${fmtCompact(Number(t.qty) || 0)}</td><td>${fmtCompact(Number(t.price) || 0)}</td><td>$${fmt(Number(t.notional) || 0, 0)}</td><td class="muted">−$${fmt(Number(t.fee_usd) || 0, 2)}</td><td>${pnlCell}</td><td class="muted">${escapeHTML(t.reason || "—")}</td></tr>`;
        }).join("") + '</tbody></table></div>' + (trades.length > TF_TRADE_PAGE_SIZE ? pager("tftr", trades.length, tfTradePage, TF_TRADE_PAGE_SIZE) : "")
      : '<div class="empty">暂无详细交易记录，下一次调仓、止盈或止损后将自动记录。</div>';
  }
  const rbs = (d.rebalances || []).slice(-20).reverse();
  if ($("tf-rebalances")) {
    $("tf-rebalances").innerHTML = rbs.length
      ? '<div class="table-scroll"><table><thead><tr><th>时间 · UTC+8</th><th>净值</th><th>换手名义</th><th>手续费</th><th>持仓数</th><th>变动明细</th></tr></thead><tbody>' +
        rbs.map(r => `<tr><td class="muted">${ts2bj(r.ts)}</td><td>$${fmt(r.equity || 0, 2)}</td><td>$${fmt(r.turnover_usd || 0, 0)}</td><td>−$${fmt(r.fees_usd || 0, 2)}</td><td>${r.n_positions || 0}</td><td class="muted">${escapeHTML((r.changes || []).join(" ") || "—")}</td></tr>`).join("") +
        '</tbody></table></div>'
      : '<div class="empty">暂无调仓记录。</div>';
  }
}

let tradfiCfgLoaded = false;

function tfConfigInto(c) {
  c = c || {};
  const set = (id, v) => { const el = $(id); if (el && document.activeElement !== el) el.value = v; };
  set("tf-capital", c.capital ?? 2000);
  set("tf-leverage", c.gross_leverage ?? 3.0);
  set("tf-stock-alloc", Math.round((c.stock_alloc ?? 0.4) * 100));
  set("tf-vbounce-alloc", Math.round((c.vbounce_alloc ?? 0.2) * 100));
  set("tf-vol-target", Math.round((c.vol_target_ann ?? 0.4) * 100));
  set("tf-min-bars", c.stock_min_bars ?? 7);
  set("tf-max-dd", c.max_dd_pct ?? 20);
  set("tf-maker-wait", c.maker_wait_sec ?? 8);
  set("tf-taker-only", String(!!c.taker_only));
  set("tf-armed", String(!!c.armed));
  set("tf-auto", String(!!c.auto_run));
  set("tf-key-file", c.key_file || "");
  set("tf-proxy", c.proxy_url || "");
  const kb = $("tf-key-badge");
  if (kb) {
    const ready = !!(c.has_api_key && c.has_api_secret);
    kb.className = "badge-mini " + (ready ? "ok" : "mute");
    kb.textContent = ready ? ("已就绪 " + (c.api_key_masked || "")) : "未配置";
  }
}

async function refreshTradFiConfig(force = false) {
  try {
    const res = await fetchJSONRetry("/api/tradfi/config", {}, 10000);
    if (res && res.config && (!tradfiCfgLoaded || force)) {
      tfConfigInto(res.config);
      tradfiCfgLoaded = true;
    }
  } catch (_) {}
}

function renderTradFiPlanOut(title, payload) {
  const out = $("tf-plan-out");
  if (!out) return;
  const p = payload || {};
  const orders = Array.isArray(p.orders) ? p.orders : [];
  const results = Array.isArray(p.results) ? p.results : [];
  const srcLabel = p.equity_source === "binance_account" ? "币安实盘净值" : "规划本金";
  const head = `<div class="note ${p.breaker_tripped ? "neg" : "ok"}" style="margin-bottom:8px"><b>${escapeHTML(title)}</b> · ${srcLabel} $${fmt(Number(p.equity) || 0, 2)} · 杠杆 ${fmt(Number(p.gross_leverage) || 3, 1)}x · 回撤 ${fmt(Number(p.dd_pct) || 0, 2)}%${(p.blocked_symbols || []).length ? " · 屏蔽 " + escapeHTML(p.blocked_symbols.join(",")) : ""}</div>`;
  const rows = orders.map((o) => {
    const isBuy = o.side === "BUY";
    return `<tr><td><b>${escapeHTML(o.symbol)}</b></td><td class="${isBuy ? "pos" : "neg"}">${isBuy ? "买入" : "卖出"}</td><td>${fmtCompact(Number(o.qty) || 0)}</td><td>$${fmt(Number(o.notional) || 0, 0)}</td><td class="muted">${escapeHTML(o.reason || "")}</td></tr>`;
  }).join("");
  const table = orders.length
    ? `<div class="table-scroll"><table><thead><tr><th>标的</th><th>方向</th><th>数量</th><th>名义</th><th>动作</th></tr></thead><tbody>${rows}</tbody></table></div>`
    : '<div class="empty">当前无需调仓（或已空仓）。</div>';
  const resHtml = results.length
    ? `<pre class="logbox">${escapeHTML(results.map((r) => `${r.symbol}: ${r.mode || ""} ${r.status || ""}`).join("\n"))}</pre>`
    : "";
  out.innerHTML = head + table + resHtml;
}

async function refreshTradFi() {
  refreshTradFiConfig(false);
  try {
    const d = await fetchJSONRetry("/api/tradfi", {}, 15000);
    lastTradFiData = d;
    renderTradFi(d);
  } catch (e) {
    const st = $("tf-status");
    if (st) {
      st.textContent = "读取 TradFi 影子失败：" + ((e && e.message) || e);
      st.hidden = false;
    }
  }
}

async function saveTradFiConfig() {
  const body = {
    capital: Number($("tf-capital").value),
    gross_leverage: Number($("tf-leverage").value),
    stock_alloc: Number($("tf-stock-alloc").value) / 100,
    vbounce_alloc: Number($("tf-vbounce-alloc") ? $("tf-vbounce-alloc").value : 20) / 100,
    vol_target_ann: Number($("tf-vol-target") ? $("tf-vol-target").value : 40) / 100,
    stock_min_bars: Number($("tf-min-bars").value),
    max_dd_pct: Number($("tf-max-dd").value),
    maker_wait_sec: Number($("tf-maker-wait").value),
    taker_only: $("tf-taker-only").value === "true",
    armed: $("tf-armed").value === "true",
    auto_run: $("tf-auto").value === "true",
    key_file: ($("tf-key-file").value || "").trim(),
    proxy_url: ($("tf-proxy").value || "").trim(),
  };
  const res = await fetchJSON("/api/tradfi/config", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  }, 15000);
  if (res && res.config) tfConfigInto(res.config);
  return res;
}

if ($("tf-refresh")) $("tf-refresh").addEventListener("click", () => { refreshTradFiConfig(true); refreshTradFi(); });
if ($("tf-cfg-save")) $("tf-cfg-save").addEventListener("click", async () => {
  const btn = $("tf-cfg-save");
  const st = $("tf-cfg-status");
  btn.disabled = true;
  btn.textContent = "保存中…";
  try {
    await saveTradFiConfig();
    if (st) st.textContent = "配置已保存 ✓（杠杆/比例下轮自动同步，或点「立即同步影子」马上生效）";
    btn.textContent = "已保存 ✓";
  } catch (e) {
    if (st) st.textContent = "保存失败：" + ((e && e.message) || e);
    btn.textContent = "保存失败";
  } finally {
    btn.disabled = false;
    setTimeout(() => { btn.textContent = "保存配置"; }, 1600);
  }
});
if ($("tf-shadow-rebal")) $("tf-shadow-rebal").addEventListener("click", async () => {
  const btn = $("tf-shadow-rebal");
  const st = $("tf-cfg-status");
  btn.disabled = true;
  btn.textContent = "同步中…";
  if (st) st.textContent = "正在保存参数并立即按新杠杆/比例调仓影子（保留历史收益）…";
  try {
    await saveTradFiConfig();
    const res = await fetchJsonSafe("/api/tradfi/shadow/rebalance", { method: "POST" }, 90000);
    if (!res.ok) throw new Error(res.error || "失败");
    if (res.shadow) {
      lastTradFiData = res.shadow;
      renderTradFi(res.shadow);
    } else {
      await refreshTradFi();
    }
    if (st) st.textContent = "影子已按最新策略参数立即调仓 ✓";
  } catch (e) {
    if (st) st.textContent = "同步影子失败：" + ((e && e.message) || e);
  } finally {
    btn.disabled = false;
    btn.textContent = "立即同步影子";
  }
});
if ($("tf-shadow-reset")) $("tf-shadow-reset").addEventListener("click", async () => {
  const cap = Number($("tf-capital").value);
  if (!Number.isFinite(cap) || cap < 50) {
    alert("请先填写有效的规划本金（≥ $50）。");
    return;
  }
  if (!confirm(`确认按本金 $${cap.toLocaleString("en-US")} 重置美股+商品影子？\n\n会清空当前模拟历史与旧持仓，立即按新本金与当前策略参数重新建仓（不发真单）。`)) return;
  const btn = $("tf-shadow-reset");
  const st = $("tf-cfg-status");
  btn.disabled = true;
  btn.textContent = "重置中…";
  if (st) st.textContent = `正在按 $${cap.toLocaleString("en-US")} 重置影子并拉取币安行情建仓…`;
  try {
    await saveTradFiConfig();
    const res = await fetchJsonSafe("/api/tradfi/shadow/reset", { method: "POST" }, 90000);
    if (!res.ok) throw new Error(res.error || "失败");
    if (res.shadow) {
      lastTradFiData = res.shadow;
      renderTradFi(res.shadow);
    } else {
      await refreshTradFi();
    }
    if (st) st.textContent = `影子已按本金 $${cap.toLocaleString("en-US")} 重置并建仓 ✓`;
  } catch (e) {
    if (st) st.textContent = "重置影子失败：" + ((e && e.message) || e);
  } finally {
    btn.disabled = false;
    btn.textContent = "按本金重置影子";
  }
});
if ($("tf-plan-btn")) $("tf-plan-btn").addEventListener("click", async () => {
  const btn = $("tf-plan-btn");
  const out = $("tf-plan-out");
  btn.disabled = true;
  btn.textContent = "计算中…";
  if (out) out.innerHTML = '<div class="empty">正在拉取币安行情并计算调仓计划…</div>';
  try {
    const res = await fetchJsonSafe("/api/tradfi/plan", { method: "POST" }, 90000);
    if (!res.ok) throw new Error(res.error || "失败");
    renderTradFiPlanOut("预览计划（未下单）", res.plan);
  } catch (e) {
    if (out) out.innerHTML = `<div class="note neg">计算计划失败：${escapeHTML(String((e && e.message) || e))}</div>`;
  } finally {
    btn.disabled = false;
    btn.textContent = "预览调仓计划（不下单）";
  }
});
if ($("tf-exec-btn")) $("tf-exec-btn").addEventListener("click", async () => {
  if ($("tf-armed").value !== "true") {
    alert("请先把「启用币安实盘」设为开启并点「保存配置」，否则不会发送任何订单。");
    return;
  }
  if (!confirm("确认执行币安 TradFi 实盘调仓？将使用已保存的配置向币安 USDⓈ-M 合约账户发送真实订单。")) return;
  const btn = $("tf-exec-btn");
  const out = $("tf-plan-out");
  btn.disabled = true;
  btn.textContent = "执行中…";
  if (out) out.innerHTML = '<div class="empty">正在执行币安实盘调仓（GTX Maker 优先），请稍候…</div>';
  try {
    const res = await fetchJsonSafe("/api/tradfi/execute", { method: "POST" }, 300000);
    if (!res.ok) throw new Error(res.error || "失败");
    renderTradFiPlanOut("币安实盘执行完成", res.result);
    refreshTradFi();
  } catch (e) {
    if (out) out.innerHTML = `<div class="note neg">实盘执行失败：${escapeHTML(String((e && e.message) || e))}</div>`;
  } finally {
    btn.disabled = false;
    btn.textContent = "执行币安实盘调仓";
  }
});
if ($("tf-flatten-btn")) $("tf-flatten-btn").addEventListener("click", async () => {
  if ($("tf-armed").value !== "true") {
    alert("请先开启「启用币安实盘」并保存配置。");
    return;
  }
  if (!confirm("⚠ 确认一键平仓？将对币安账户内全部 TradFi（美股+金银油气）持仓发送 Reduce-Only 平仓单。")) return;
  const btn = $("tf-flatten-btn");
  const out = $("tf-plan-out");
  btn.disabled = true;
  btn.textContent = "平仓中…";
  if (out) out.innerHTML = '<div class="empty">正在发送 Reduce-Only 平仓单…</div>';
  try {
    const res = await fetchJsonSafe("/api/tradfi/flatten", { method: "POST" }, 300000);
    if (!res.ok) throw new Error(res.error || "失败");
    renderTradFiPlanOut("币安一键平仓完成", res.result);
    refreshTradFi();
  } catch (e) {
    if (out) out.innerHTML = `<div class="note neg">平仓失败：${escapeHTML(String((e && e.message) || e))}</div>`;
  } finally {
    btn.disabled = false;
    btn.textContent = "一键全部平仓";
  }
});
const tfChartTabs = $("tf-chart-tabs");
if (tfChartTabs) {
  tfChartTabs.addEventListener("click", (event) => {
    const btn = event.target.closest("button[data-range]");
    if (!btn) return;
    tfRange = btn.dataset.range;
    tfChartTabs.querySelectorAll("button").forEach(b => b.setAttribute("aria-pressed", String(b === btn)));
    if (lastTradFiData) renderTradFi(lastTradFiData);
  });
}

function renderEquityChart(id, history, aria, compare, third, fourth, summaryLabels) {
  const el = $(id);
  if (!el) return;
  const pts = (history || [])
    .filter((p) => Array.isArray(p) && Number.isFinite(p[0]) && Number.isFinite(p[1]))
    .map((p) => ({ ts: p[0], eq: p[1] }));
  if (pts.length < 2) {
    el.innerHTML = '<div class="empty">还没有足够的权益点（每小时一个，跑两小时就能画线）</div>';
    return;
  }
  const cmp = (compare || [])
    .filter((p) => Array.isArray(p) && Number.isFinite(p[0]) && Number.isFinite(p[1]))
    .map((p) => ({ ts: p[0], eq: p[1] }));
  const thd = (third || [])
    .filter((p) => Array.isArray(p) && Number.isFinite(p[0]) && Number.isFinite(p[1]))
    .map((p) => ({ ts: p[0], eq: p[1] }));
  const fth = (fourth || [])
    .filter((p) => Array.isArray(p) && Number.isFinite(p[0]) && Number.isFinite(p[1]))
    .map((p) => ({ ts: p[0], eq: p[1] }));
  const W = Math.max(260, el.clientWidth || 900), H = 240;
  const padL = 64, padR = 20, padT = 20, padB = 30;
  const first = pts[0], last = pts[pts.length - 1];
  const change = last.eq - first.eq;
  const vals = pts.map((p) => p.eq);
  const allVals = vals
    .concat(cmp.length > 1 ? cmp.map((p) => p.eq) : [])
    .concat(thd.length > 1 ? thd.map((p) => p.eq) : [])
    .concat(fth.length > 1 ? fth.map((p) => p.eq) : []);
  const ax = niceAxis(Math.min(...allVals), Math.max(...allVals), 4);
  const span = ax.hi - ax.lo || 1;
  const X = (ts) => padL + (last.ts === first.ts ? 0.5 : (ts - first.ts) / (last.ts - first.ts)) * (W - padL - padR);
  const Y = (v) => padT + (1 - (v - ax.lo) / span) * (H - padT - padB);
  const grid = ax.lines.map((v) => `<line x1="${padL}" y1="${Y(v)}" x2="${W - padR}" y2="${Y(v)}" stroke="var(--grid-line, rgba(255,255,255,0.06))"/><text x="${padL - 10}" y="${Y(v) + 4}" text-anchor="end" font-size="11" font-family="var(--font-mono)" fill="#64748b">$${v.toFixed(span < 10 ? 2 : 0)}</text>`).join("");
  const line = pts.map((p) => `${X(p.ts)},${Y(p.eq)}`).join(" ");
  const ticks = [first.ts, first.ts + (last.ts - first.ts) / 2, last.ts];
  const tfmt = summaryLabels && summaryLabels.useBjTime ? ts2bj : ts2m;
  const xlabels = ticks.map((ts, i) => `<text x="${X(ts)}" y="${H - 8}" text-anchor="${i === 0 ? "start" : i === ticks.length - 1 ? "end" : "middle"}" font-size="11" font-family="var(--font-mono)" fill="#64748b">${tfmt(ts)}</text>`).join("");
  const cmpLine = cmp.length > 1 ? cmp.map((p) => `${X(p.ts)},${Y(p.eq)}`).join(" ") : "";
  const thdLine = thd.length > 1 ? thd.map((p) => `${X(p.ts)},${Y(p.eq)}`).join(" ") : "";
  const fthLine = fth.length > 1 ? fth.map((p) => `${X(p.ts)},${Y(p.eq)}`).join(" ") : "";
  const sw = (color) => `<span style="display:inline-block;width:14px;height:3px;border-radius:2px;background:${color};margin-right:6px;vertical-align:middle"></span>`;
  const legendName = summaryLabels && summaryLabels.legendName ? summaryLabels.legendName : "影子 三因子";
  const cmpLegend = cmp.length > 1
    ? `<div class="chart-coverage">${sw("#10b981")}影子 三因子 · $${fmt(last.eq, 2)} · ${moneySigned(last.eq - first.eq)}</div>` +
      `<div class="chart-coverage">${sw("#f59e0b")}对照 纯动量 · $${fmt(cmp[cmp.length - 1].eq, 2)} · ${moneySigned(cmp[cmp.length - 1].eq - cmp[0].eq)}</div>` +
      (thd.length > 1
        ? `<div class="chart-coverage">${sw("#38bdf8")}波动目标 · $${fmt(thd[thd.length - 1].eq, 2)} · ${moneySigned(thd[thd.length - 1].eq - thd[0].eq)}</div>`
        : "") +
      (fth.length > 1
        ? `<div class="chart-coverage">${sw("#a78bfa")}组合策略（动量+12h波段） · $${fmt(fth[fth.length - 1].eq, 2)} · ${moneySigned(fth[fth.length - 1].eq - fth[0].eq)}</div>`
        : "")
    : `<div class="chart-coverage">${sw("#10b981")}${legendName} · $${fmt(last.eq, 2)} · ${moneySigned(last.eq - first.eq)}</div>`;
  el.innerHTML = `<div class="chart-summary">
      <div><span class="chart-eyebrow">${summaryLabels && summaryLabels.equityLabel ? summaryLabels.equityLabel : "影子权益 · 最新"}</span><strong>$${fmt(last.eq, 2)}</strong></div>
      <div><span class="chart-eyebrow">${summaryLabels && summaryLabels.changeLabel ? summaryLabels.changeLabel : "影子区间变化"}</span><strong class="${change >= 0 ? "pos" : "neg"}">${moneySigned(change)}</strong></div>
      ${cmpLegend}
      <div class="chart-coverage"><span class="chart-eyebrow">点数${summaryLabels && summaryLabels.useBjTime ? " · UTC+8" : ""}</span>${pts.length} · ${tfmt(first.ts)} — ${tfmt(last.ts)}</div>
    </div>
    <svg viewBox="0 0 ${W} ${H}" role="img" aria-label="${aria}">
      <defs><linearGradient id="carry-area" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="#10b981" stop-opacity="0.22"/><stop offset="100%" stop-color="#10b981" stop-opacity="0"/></linearGradient></defs>
      ${grid}
      <polygon points="${X(first.ts)},${H - padB} ${line} ${X(last.ts)},${H - padB}" fill="url(#carry-area)"/>
      <line x1="${padL}" y1="${Y(first.eq)}" x2="${W - padR}" y2="${Y(first.eq)}" stroke="rgba(148,163,184,0.3)" stroke-dasharray="4 5"/>
      ${cmpLine ? `<polyline points="${cmpLine}" fill="none" stroke="#f59e0b" stroke-width="2" stroke-linejoin="round" opacity="0.9"/>` : ""}
      ${thdLine ? `<polyline points="${thdLine}" fill="none" stroke="#38bdf8" stroke-width="2" stroke-linejoin="round" opacity="0.9"/>` : ""}
      ${fthLine ? `<polyline points="${fthLine}" fill="none" stroke="#a78bfa" stroke-width="2" stroke-linejoin="round" opacity="0.9"/>` : ""}
      <polyline points="${line}" fill="none" stroke="#10b981" stroke-width="2.2" stroke-linejoin="round"/>
      <circle cx="${X(last.ts)}" cy="${Y(last.eq)}" r="4.5" fill="#34d399" stroke="var(--surface, #0e131b)" stroke-width="2"/>
      ${xlabels}
    </svg>`;
}







// Rebuild chart coordinates when the visible container changes width (rotation,
// split view or navigation). This only redraws cached data; never requests orders.
if (typeof ResizeObserver !== "undefined") {
  const chartWidths = new WeakMap();
  const chartResize = new window.ResizeObserver(entries => {
    for (const { target, contentRect } of entries) {
      const width = Math.round(contentRect.width);
      if (!width || chartWidths.get(target) === width) continue;
      chartWidths.set(target, width);
      if (target.id === "mo-chart" && lastLiveData) renderMonitorChart(lastLiveData);
      if (target.id === "hc-chart" && lastHlCarryData) renderHlCarry(lastHlCarryData);
      if (target.id === "tf-chart" && lastTradFiData) renderTradFi(lastTradFiData);
    }
  });
  ["mo-chart", "hc-chart", "tf-chart"].forEach(id => {
    const chart = $(id);
    if (chart) chartResize.observe(chart);
  });
}

// Keep accounting calculations in the existing renderers. This layer only selects
// the first-read summary and moves all original metrics into inspectable details.
function arrangeMonitorSummary(id, shadow = false) {
  const root = $(id);
  if (!root) return;
  const groups = [...root.querySelectorAll(':scope > .metric-group')];
  if (!groups.length) return;
  const detailsHost = $(shadow ? 'sh-analysis-metrics' : 'mo-analysis-metrics');
  // 容器不存在就直接放弃整理 ✗ 让指标留在原地显示 ✗ 总比整页空白好 ✓
  if (!detailsHost) return;
  const key = id + '-details';
  let details = $(key);
  const active = details?.dataset.active || '0';
  if (!details) {
    details = document.createElement('section');
    details.id = key;
    details.className = 'account-details content-card';
    detailsHost.append(details);
  }
  const first = [...groups[0].querySelectorAll('.metric')];
  const capital = groups.find(g => g.classList.contains('capital-account'));
  root.replaceChildren();
  root.className = shadow ? 'account-summary cols-3' : 'account-summary';
  if (shadow) {
    // 影子页沿用原来的三格（净值 ✗ 累计盈亏 ✗ 浮动 ✓）
    const earnings = first[1];
    const floating = groups[1]?.querySelectorAll('.metric')[1];
    for (const [source, label] of [[first[0], '影子净值'], [earnings, '累计影子盈亏'], [floating, '当前持仓浮动盈亏']]) {
      if (!source) continue;
      const item = source.cloneNode(true);
      item.querySelector('.k').textContent = label;
      root.append(item);
    }
  } else if (lastLiveData) {
    // 实盘：把四个数按【净值 = 本金 + 已实现 + 浮动】的顺序排 ✗ 一眼能对账 ✓
    const ld = lastLiveData;
    const base = (ld.capital_baseline || {}).equity || 0;
    const dep = Number(ld.net_deposit) || 0;
    const principal = base + dep;
    const eq = Number.isFinite(ld.equity) ? ld.equity : 0;
    const unrl = Number.isFinite(ld.cumulative_pnl) ? ld.cumulative_pnl : 0;
    const realized = eq - principal - unrl;
    const money = (v) =>
      (v >= 0 ? "+" : "−") + "$" + Math.abs(v).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
    const usd = (v) => "$" + Math.abs(v).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
    const cell = (label, value, tone, caption) => {
      const item = document.createElement("div");
      item.className = "metric";
      item.innerHTML =
        `<div class="k">${label}</div><div class="v ${tone}">${value}</div>`;
      if (caption) {
        const cap = document.createElement("span");
        cap.className = "account-return";
        cap.textContent = caption;
        item.append(cap);
      }
      return item;
    };
    const pct = principal > 0 ? ((eq / principal - 1) * 100).toFixed(2) + "%" : "";
    const mlAcc2 = ld.ml_account && !ld.ml_account.error ? ld.ml_account : null;
    const { extNetDep, mlNetDep } = splitCapitalFlows(ld);
    // 「全部账户」：显示 A + B 合计（本金 = A 的基准 + B 的起始 + 外部净入金）
    if (currentStrategy === 'all' && mlAcc2) {
      const mlEq = Number.isFinite(mlAcc2.equity) ? mlAcc2.equity : 0;
      const mlBase = ((ld.ml_history || [])[0] || {}).equity || 0;
      const eqAll = eq + mlEq;
      const principalAll = base + mlBase + extNetDep;
      const unrlAll = unrl + (mlAcc2.positions || []).reduce((s2, p2) => s2 + (Number.isFinite(p2.unrealized) ? p2.unrealized : 0), 0);
      const pctAll = principalAll > 0 ? ((eqAll / principalAll - 1) * 100).toFixed(2) + "%" : "";
      const netAll = eqAll - principalAll;
      const bufs = [ld.liq_buffer_pct, mlAcc2.liq_buffer_pct].filter(Number.isFinite);
      const minBuf = bufs.length ? Math.min(...bufs) : null;
      root.append(cell("合计净收益", money(netAll), netAll >= 0 ? "pos" : "neg", pctAll ? `净投入收益率 ${Number(pctAll) >= 0 ? "+" : ""}${pctAll}` : ""));
      root.append(cell("合计本金", usd(principalAll)));
      root.append(cell("当前浮盈", money(unrlAll), unrlAll >= 0 ? "pos" : "neg"));
      root.append(cell("最低强平缓冲", minBuf == null ? "—" : minBuf.toFixed(1) + "%", minBuf == null ? "" : minBuf < 40 ? "neg" : "pos"));
    } else if (currentStrategy === 'ml') {
      // ML tab：显示 ML 子账户自己的净值/浮盈/持仓（扣除子账户互转）
      const eqMl = Number.isFinite(mlAcc2?.equity) ? mlAcc2.equity : null;
      const mlHist = ld.ml_history || [];
      const baseMl = (mlHist[0] || {}).equity || 0;
      const principalMl = baseMl + mlNetDep;
      const unrlMl = mlAcc2
        ? (mlAcc2.positions || []).reduce((s2, p2) => s2 + (Number.isFinite(p2.unrealized) ? p2.unrealized : 0), 0)
        : null;
      const pctMl = principalMl > 0 && eqMl != null ? ((eqMl / principalMl - 1) * 100).toFixed(2) + "%" : "";
      root.append(cell("ML 子账户净值", eqMl == null ? "—" : usd(eqMl), "", pctMl ? `净投入收益率 ${Number(pctMl) >= 0 ? "+" : ""}${pctMl}` : (eqMl == null ? "ML 账户数据暂不可用" : "")));
      root.append(cell("本金", principalMl > 0 ? usd(principalMl) : "—"));
      root.append(cell("当前浮盈", unrlMl == null ? "—" : money(unrlMl), unrlMl == null ? "" : unrlMl >= 0 ? "pos" : "neg"));
      // 组合预估日波动：ml_history 是小时点 ✗ 小时收益 std × sqrt(24) = 日波动
      let volPct = null;
      if (mlHist.length >= 4) {
        const rets = [];
        for (let i = 1; i < mlHist.length; i++) {
          if (mlHist[i].ts > mlHist[i-1].ts && mlHist[i-1].equity > 0) {
            rets.push(mlHist[i].equity / mlHist[i-1].equity - 1);
          }
        }
        if (rets.length >= 3) {
          const n = rets.length;
          const mean = rets.reduce((a, r) => a + r, 0) / n;
          const var_ = rets.reduce((a, r) => a + (r - mean) ** 2, 0) / (n - 1);
          const estimate = Math.sqrt(var_) * Math.sqrt(24) * 100;
          if (Number.isFinite(estimate)) volPct = estimate;
        }
      }
      const tone = volPct == null ? "" : volPct > 3 ? "neg" : volPct > 2.2 ? "warn" : "pos";
      root.append(cell("组合预估日波动", volPct == null ? "—" : volPct.toFixed(2) + "%", tone));
    } else {
      root.append(
        cell("账户净值", usd(eq), "", pct ? `净投入收益率 ${Number(pct) >= 0 ? "+" : ""}${pct}` : "")
      );
      root.append(cell("本金", usd(principal)));
      root.append(cell("当前浮盈", money(unrl), unrl >= 0 ? "pos" : "neg"));
      // 组合预估日波动（波动目标看的数 ✗ 高波动时会自动减仓 ✓）
      const vn = lastLiveData && lastLiveData.vol_now;
      if (vn && Number.isFinite(vn.pct)) {
        const pct = vn.pct;
        const target = Number(vn.target_pct) || 2;
        const tone = pct > target * 1.5 ? "neg" : pct > target * 1.1 ? "warn" : "pos";
        root.append(cell("组合预估日波动", pct.toFixed(2) + "%", tone));
      }
    }
  }
  // 策略健康度（Bento 侧栏与曲线并排 ✓ ML 与动量各自独立计算 ✓）
  const healthHost = !shadow ? $('mo-health-host') : null;
  const perfPanel = !shadow ? $('mo-perf-panel') : null;
  if (!shadow && currentStrategy === 'ml' && lastLiveData) {
    const mlAccH = lastLiveData.ml_account && !lastLiveData.ml_account.error ? lastLiveData.ml_account : null;
    const mlm = mlMetricsSummary(lastLiveData, mlAccH, lastLiveData.ml_history || []);
    const netRatio = mlm.valid && mlm.eq > 0 ? (Math.abs(mlm.netNotional) / mlm.eq) * 100 : null;
    const items = [
      {
        name: "区间净收益（扣互转）",
        val: mlm.percent != null ? `${mlm.percent >= 0 ? "+" : "−"}${Math.abs(mlm.percent).toFixed(2)}%` : "样本不足",
        status: mlm.percent == null ? "unknown" : (mlm.percent >= 0 ? "green" : mlm.percent > -8 ? "yellow" : "red"),
        hint: mlm.valid ? `净收益 ${moneySigned(mlm.change)} · 起点 $${fmt(mlm.base, 0)}` : "等待更多净值记录",
      },
      {
        name: "记录期最大回撤",
        val: mlm.maxDdPct != null ? `${mlm.maxDdPct.toFixed(2)}%` : "样本不足",
        status: mlm.maxDdPct == null ? "unknown" : (mlm.maxDdPct <= 10 ? "green" : mlm.maxDdPct <= 20 ? "yellow" : "red"),
        hint: "目标 ≤ 15% · 基于调整后净值序列",
      },
      {
        name: "年化波动率",
        val: mlm.annVolPct != null ? `${mlm.annVolPct.toFixed(1)}%` : "样本不足",
        status: mlm.annVolPct == null ? "unknown" : (mlm.annVolPct <= 25 ? "green" : mlm.annVolPct <= 40 ? "yellow" : "red"),
        hint: `共 ${mlm.pointsCount} 个小时采样点`,
      },
      {
        name: "净敞口 / 净值",
        val: netRatio != null ? `${netRatio.toFixed(1)}%` : "—",
        status: netRatio == null ? "unknown" : (netRatio <= 15 ? "green" : netRatio <= 30 ? "yellow" : "red"),
        hint: "多空净额偏移 · 目标 ≤ 15%",
      },
      {
        name: "最低强平缓冲",
        val: mlm.minDist != null ? `${mlm.minDist.toFixed(1)}%` : "—",
        status: mlm.minDist == null ? "unknown" : (mlm.minDist >= 40 ? "green" : mlm.minDist >= 25 ? "yellow" : "red"),
        hint: "安全线 > 40% · 距最近强平价",
      },
    ];
    const rows = items.map((row) => {
      const stateLabel = row.status === "green" ? "正常" : row.status === "yellow" ? "关注" : row.status === "red" ? "异常" : "样本不足";
      const dot = '<span class="score-dot ' + row.status + '" role="img" aria-label="' + stateLabel + '"></span>';
      const tone = row.status === "green" ? "pos" : row.status === "yellow" ? "warn" : row.status === "red" ? "neg" : "";
      return `<div class="score-row"><div class="score-row-top">${dot}<span class="score-name">${escapeHTML(row.name)}</span><span class="score-val ${tone}">${escapeHTML(row.val)}</span></div><span class="score-hint">${escapeHTML(row.hint)}</span></div>`;
    }).join("");
    const card = document.createElement("div");
    card.className = "metric score-metric";
    card.innerHTML = `<div class="k">策略健康度 · ML</div><div class="score-body">${rows}</div>`;
    if (healthHost && perfPanel) {
      healthHost.replaceChildren(card);
      healthHost.hidden = false;
      perfPanel.classList.add('with-health');
    } else {
      root.append(card);
    }
  } else if (!shadow && lastLiveData && Array.isArray(lastLiveData.scorecard) && lastLiveData.scorecard.length) {
    const card = document.createElement("div");
    card.className = "metric score-metric";
    const scorecard = lastLiveData.scorecard;
    const grossRow = scorecard.find((row) => row.key === "gross");
    const rows = scorecard.filter((row) => row.key !== "gross").map((row) => {
      const status = row.status || "unknown";
      const stateLabel = status === "green" ? "正常" : status === "yellow" ? "关注" : status === "red" ? "异常" : "样本不足";
      const dot = '<span class="score-dot ' + escapeHTML(status) + '" role="img" aria-label="' + stateLabel + '"></span>';
      const tone = row.status === "green" ? "pos" : row.status === "yellow" ? "warn" : row.status === "red" ? "neg" : "";
      let val = "样本不足";
      let hint = row.good || "";
      if (Number.isFinite(row.value)) {
        if (row.key === "ic90") val = row.value.toFixed(4);
        else if (row.key === "dd") val = `${row.value.toFixed(2)}%`;
        else val = `${row.value >= 0 ? "+" : ""}${row.value.toFixed(2)}%`;
      }
      // 毛收益只是净收益加回手续费，合并进区间收益说明，避免被误读成第二个收益指标。
      if (row.key === "ret30" && grossRow && Number.isFinite(grossRow.value)) {
        const gross = (grossRow.value >= 0 ? "+" : "−") + Math.abs(grossRow.value).toFixed(2) + "%";
        const net = Number.isFinite(grossRow.net)
          ? (grossRow.net >= 0 ? "+" : "−") + Math.abs(grossRow.net).toFixed(2) + "%"
          : val;
        const feePct = Number.isFinite(grossRow.fee_pct) ? grossRow.fee_pct.toFixed(2) + "%" : "—";
        const feeUsd = Number.isFinite(grossRow.fee) ? "（$" + grossRow.fee.toFixed(0) + "）" : "";
        hint = hint + " · 毛 " + gross + " = 净 " + net + " + 费用 " + feePct + feeUsd;
      }
      return `<div class="score-row"><div class="score-row-top">${dot}<span class="score-name">${escapeHTML(row.name || "")}</span><span class="score-val ${tone}">${escapeHTML(val)}</span></div><span class="score-hint">${escapeHTML(hint)}</span></div>`;
    }).join("");
    card.innerHTML = `<div class="k">策略健康度 · 动量</div><div class="score-body">${rows}</div>`;
    if (healthHost && perfPanel) {
      healthHost.replaceChildren(card);
      healthHost.hidden = false;
      perfPanel.classList.add('with-health');
    } else {
      root.append(card);
    }
  } else if (healthHost && perfPanel) {
    healthHost.replaceChildren();
    healthHost.hidden = true;
    perfPanel.classList.remove('with-health');
  }
  // 兜底：如果整理后根节点什么都没有（比如 structure 变了 ✗）
  // 就把原来的分组放回去 ✗ 宁可丑也不能空白 ✓
  if (!root.children.length) {
    groups.forEach((g) => root.append(g));
    console.warn("arrangeMonitorSummary: 摘要为空，已回退到原始分组");
    return;
  }
  details.replaceChildren();

  const nav = document.createElement('div');
  nav.className = 'account-detail-tabs';
  nav.setAttribute('role', 'group');
  nav.setAttribute('aria-label', '账户详情分类');
  details.append(nav);
  const labels = shadow ? ['账户', '盈亏', '风险', '同期对照', '因子'] : ['账户', '资金', '风险', '执行', '胜率'];
  function select(index) {
    details.dataset.active = String(index);
    groups.forEach((g, i) => { g.hidden = i !== index; nav.children[i].setAttribute('aria-pressed', String(i === index)); });
  }
  groups.forEach((group, index) => {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = labels[index] || group.querySelector('.g-title').textContent;
    button.setAttribute('aria-controls', key + '-' + index);
    group.id = key + '-' + index;
    button.addEventListener('click', () => select(index));
    nav.append(button);
    details.append(group);
  });
  select(Math.min(Number(active) || 0, groups.length - 1));
}


// ---------- HL 单账户 carry 影子
// 注意：app.js 里的 `money` 只是某些函数内的局部变量，不是全局；
// 这里用一个本块的格式化器，避免 ReferenceError。
// 必须用函数声明（会提升），不能用 const 箭头函数。
// 启动块里的 refreshHlCarry() 在第 1107 行就跑，而这段代码在 1521 行；
// 用 const 会让渲染撞上 TDZ：Cannot access 'hcMoney' before initialization。
const HC_PAGE = 10;
const HC_PAGE_2 = 15;
let hcLegPage = 0;
let hcRbPage = 0;
let hcDailyPage = 0;
let hcLogPage = 0;
let hcRange = "all";
let lastHlCarryData = null;

function hcMoney(v) {
  return Number.isFinite(v)
    ? "$" + Math.abs(v).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })
    : "—";
}

function hcSpotLabel(pair) {
  const known = {
    "@107": "HYPE/USDC",
    "@142": "UBTC/USDC · Unit",
    "@151": "UETH/USDC · Unit",
    "@260": "XMR1/USDC · Wagyu",
  };
  return known[pair] ? `${known[pair]}（${pair}）` : (pair || "—");
}

function renderHlCarry(d) {
  const s = (d && d.state) || {};
  const legs = Array.isArray(s.legs) ? s.legs : [];
  const eq = Number.isFinite(d.equity) ? d.equity : 0;
  const start = Number.isFinite(s.start_equity) && s.start_equity > 0 ? s.start_equity : 0;
  const fund = Number.isFinite(d.funding_total)
    ? d.funding_total
    : legs.reduce((a, l) => a + (Number(l.funding_accrued) || 0), 0);
  const basis = Number.isFinite(d.basis_total)
    ? d.basis_total
    : legs.reduce((a, l) => a + (Number(l.basis_pnl) || 0), 0);
  const feeToday = (Array.isArray(s.daily) && s.daily.length)
    ? Number(s.daily[s.daily.length - 1].fees) || 0
    : 0;
  const feeTotal = Number.isFinite(s.fees_paid_total) ? s.fees_paid_total : 0;
  const ann = d.annualized_pct;
  const p = s.params || {};
  const pf = [
    ["hc-p-topk", p.top_k, 0],
    ["hc-p-minfund", p.min_funding_ann_pct, 2],
    ["hc-p-minvol", p.min_spot_volume_usd, 0],
    ["hc-p-maxw", Number.isFinite(p.max_leg_weight) ? p.max_leg_weight * 100 : null, 0],
    ["hc-p-payback", p.payback_days, 0],
    ["hc-p-minhold", p.min_hold_hours, 0],
    ["hc-p-basistp", p.basis_take_profit_bp, 0],
  ];
  for (const [id, val, digits] of pf) {
    const inp = $(id);
    if (inp && document.activeElement !== inp && val != null && Number.isFinite(Number(val))) {
      inp.value = String(digits ? Number(val).toFixed(digits).replace(/\.00$/, "") : val);
    }
  }
  window.__hcBudget = Number.isFinite(s.budget_usd) ? s.budget_usd : 0;
  const budgetInput = $("hc-budget");
  if (budgetInput && document.activeElement !== budgetInput) {
    budgetInput.value = window.__hcBudget > 0 ? String(window.__hcBudget) : "";
  }

  const m = $("hc-metrics");
  if (m) {
    m.innerHTML = start > 0
      ? `<div class="hc-primary-metrics"><div class="hc-metric-label">账户表现 · 含已实现盈亏</div>${[
          metric("影子权益", hcMoney(eq)),
          metric("累计盈亏", moneySigned(eq - start), eq - start >= 0 ? "pos" : "neg"),
          metric("资金费收入", moneySigned(fund), fund >= 0 ? "pos" : "neg"),
          metric("基差盈亏", moneySigned(basis), basis >= 0 ? "pos" : "neg"),
        ].join("")}</div><div class="hc-secondary-metrics"><div class="hc-metric-label">资金与成本 · 累计口径</div>${[
          metric("累计手续费", "−" + hcMoney(feeTotal)),
          metric("今日手续费", "−" + hcMoney(feeToday)),
          metric("影子预算", hcMoney(start)),
          metric("资金利用率", fmt((legs.reduce((a, l) => a + (Number(l.notional) || 0), 0) / start) * 100, 0) + "%"),
          metric("年化", Number.isFinite(ann) ? (ann >= 0 ? "+" : "−") + Math.abs(ann).toFixed(1) + "%" : "—"),
        ].join("")}</div>` + (legs.length ? "" : '<div class="empty">当前空仓等待机会；累计盈亏和曲线会继续保留。</div>')
      : '<div class="empty">还没有影子账户。填写 USD 预算并点击「按预算重置模拟」即可开始。</div>';
  }

  const w = $("hc-warn");
  if (w) {
    const ws = Array.isArray(d.warnings) ? d.warnings : [];
    w.innerHTML = ws.length
      ? `<div class="content-card"><div class="panel-head"><h2>告警</h2></div>${ws.map((x) => `<p class="note neg">${escapeHTML(x)}</p>`).join("")}</div>`
      : "";
  }

  const tb = $("hc-legs");
  if (tb) {
    const legPages = Math.max(1, Math.ceil(legs.length / HC_PAGE));
    if (hcLegPage > legPages - 1) hcLegPage = 0;
    const legRows = legs.slice(hcLegPage * HC_PAGE, (hcLegPage + 1) * HC_PAGE);
    tb.innerHTML = legs.length
      ? `<div class="table-scroll"><table><thead><tr><th>币</th><th>现货对</th><th>名义</th><th>资金费/年</th><th>当前基差</th><th>较入场基差</th><th>资金费累计</th><th>基差盈亏</th><th>腿盈亏</th></tr></thead><tbody>${legRows
          .map((l) => {
            const basisMove = Number.isFinite(Number(l.basis_move_bp)) ? Number(l.basis_move_bp) : 0;
            return `<tr><td>${escapeHTML(l.coin || "")}</td><td>${escapeHTML(hcSpotLabel(l.spot))}</td><td>${hcMoney(l.notional)}</td><td class="${Number(l.funding_ann) >= 0 ? "pos" : "neg"}">${fmt(l.funding_ann, 2)}%</td><td>${fmt(l.basis_bp, 1)} bp</td><td class="${basisMove <= 0 ? "pos" : "neg"}">${fmt(basisMove, 1)} bp</td><td>${moneySigned(l.funding_accrued)}</td><td>${moneySigned(l.basis_pnl)}</td><td class="${Number(l.leg_pnl) >= 0 ? "pos" : "neg"}">${moneySigned(l.leg_pnl)}</td></tr>`;
          })
          .join("")}</tbody></table></div>` + (legs.length > HC_PAGE ? pager("hcleg", legs.length, hcLegPage, HC_PAGE) : "")
      : '<div class="empty">当前空仓，等待新的资金费与基差机会。</div>';
  }

  // ---- 换仓记录：桌面表格，手机逐条卡片，避免七列横向挤压 ----
  const rb = $("hc-rebalances");
  if (rb) {
    const all = (s.rebalances || []).slice().reverse();
    const pages = Math.max(1, Math.ceil(all.length / HC_PAGE));
    if (hcRbPage > pages - 1) hcRbPage = 0;
    const rows = all.slice(hcRbPage * HC_PAGE, (hcRbPage + 1) * HC_PAGE);
    const models = rows.map((r) => {
      const dt = new Date(r.ts).toLocaleString("zh-CN", { hour12: false });
      const takeProfit = r.reason === "basis_take_profit";
      const up = Number(r.funding_after) >= Number(r.funding_before);
      const reason = takeProfit ? "基差止盈" : "资金费轮动";
      const change = []
        .concat((r.added || []).map((c) => `<span class="pos">+${escapeHTML(c)}</span>`))
        .concat((r.removed || []).map((c) => `<span class="neg">−${escapeHTML(c)}</span>`))
        .concat((r.resized || []).map(([c, a, b]) => `${escapeHTML(c)} ${fmt(a * 100, 0)}%→${fmt(b * 100, 0)}%`))
        .join(" · ");
      const moves = takeProfit && (r.basis_moves || []).length
        ? `<div class="muted">${(r.basis_moves || []).map(([c, b]) => `${escapeHTML(c)} ${fmt(b, 1)}bp`).join(" · ")}</div>`
        : "";
      const funding = takeProfit ? "—" : `${fmt(r.funding_before, 2)}% → ${fmt(r.funding_after, 2)}%`;
      const threshold = `${fmt(r.threshold, takeProfit ? 0 : 2)}${takeProfit ? " bp" : "%"}`;
      return {r, dt, takeProfit, up, reason, change, moves, funding, threshold};
    });
    const table = `<div class="table-scroll"><table><thead><tr><th>时间</th><th>原因</th><th>换手</th><th>加权资金费</th><th>门槛</th><th>手续费</th><th>变动</th></tr></thead><tbody>${models
      .map(({r, dt, takeProfit, up, reason, change, moves, funding, threshold}) =>
        `<tr><td class="muted">${dt}</td><td>${reason}</td><td>${fmt(r.turnover * 100, 0)}%</td><td class="${up ? "pos" : "neg"}">${funding}</td><td class="muted">${threshold}</td><td>−${hcMoney(r.fee_usd)}</td><td>${change || "—"}${moves}</td></tr>`)
      .join("")}</tbody></table></div>`;
    const cards = `<div class="hc-rb-cards">${models.map(({r, dt, reason, change, moves, funding, threshold}) =>
      `<article class="hc-rb-card"><div class="hc-rb-head"><time>${dt}</time><strong>${reason}</strong></div><div class="hc-rb-change">${change || "—"}${moves}</div><div class="hc-rb-metrics">${[
        ["换手", fmt(r.turnover * 100, 0) + "%"],
        ["加权资金费", funding],
        ["门槛", threshold],
        ["手续费", "−" + hcMoney(r.fee_usd)],
      ].map(([k, v]) => `<div><span>${k}</span><b>${v}</b></div>`).join("")}</div></article>`).join("")}</div>`;
    rb.innerHTML = rows.length
      ? table + cards
      : '<div class="empty">还没有止盈或资金费轮动记录。</div>';
    if (rows.length) {
      rb.innerHTML += pager("hcrb", all.length, hcRbPage, HC_PAGE);
    }
  }

  // ---- 每日汇总（分页）----
  const dl = $("hc-daily");
  if (dl) {
    const allDaily = (s.daily || []).slice().reverse();
    const dPages = Math.max(1, Math.ceil(allDaily.length / HC_PAGE));
    if (hcDailyPage > dPages - 1) hcDailyPage = 0;
    const rows = allDaily.slice(hcDailyPage * HC_PAGE, (hcDailyPage + 1) * HC_PAGE);
    dl.innerHTML = allDaily.length
      ? `<div class="table-scroll"><table><thead><tr><th>日期</th><th>权益</th><th>资金费</th><th>基差</th><th>手续费</th><th>换仓</th></tr></thead><tbody>${rows
          .map(
            (d) =>
              `<tr><td>${escapeHTML(d.date)}</td><td>${hcMoney(d.equity_start)} → ${hcMoney(d.equity_end)}</td><td class="${Number(d.funding) >= 0 ? "pos" : "neg"}">${moneySigned(d.funding)}</td><td class="${Number(d.basis) >= 0 ? "pos" : "neg"}">${moneySigned(d.basis)}</td><td>−${hcMoney(d.fees)}</td><td>${d.rebalances || 0}</td></tr>`,
          )
          .join("")}</tbody></table></div>` + pager("hcdl", allDaily.length, hcDailyPage, HC_PAGE)
      : '<div class="empty">还没有满一小时的记录。</div>';
  }

  const lg = $("hc-log");
  if (lg) {
    const all = (s.log || []).slice().reverse();
    const pages = Math.max(1, Math.ceil(all.length / HC_PAGE_2));
    if (hcLogPage > pages - 1) hcLogPage = 0;
    const rows = all.slice(hcLogPage * HC_PAGE_2, (hcLogPage + 1) * HC_PAGE_2);
    lg.innerHTML = rows.length
      ? '<div class="hc-event-list">' +
        rows
          .map((r) => {
            const dt = new Date(r.ts);
            const day = dt.toLocaleDateString("zh-CN", { month: "2-digit", day: "2-digit" });
            const hm = dt.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false });
            const neg = /止损|移出|不足|失败|负/.test(r.msg || "");
            return `<div class="hc-event"><span class="hc-event-time"><b>${day}</b>${hm}</span><span class="hc-event-msg ${neg ? "neg" : ""}">${escapeHTML(r.msg || "")}</span></div>`;
          })
          .join("") +
        "</div>" +
        pager("hclog", all.length, hcLogPage, HC_PAGE_2)
      : '<div class="empty">暂无事件</div>';
  }

  // ---- 基差影子收益曲线（优先使用 s.history，不足时用 s.daily 兜底）----
  const hcChartEl = $("hc-chart");
  if (hcChartEl) {
    let rawHist = Array.isArray(s.history)
      ? s.history.filter((p) => Array.isArray(p) && Number.isFinite(p[0]) && Number.isFinite(p[1]))
      : [];
    if (rawHist.length < 2 && Array.isArray(s.daily) && s.daily.length) {
      rawHist = s.daily
        .map((row) => [Date.parse(row.date + "T23:59:59Z"), Number(row.equity_end)])
        .filter((p) => Number.isFinite(p[0]) && Number.isFinite(p[1]));
    }
    if (rawHist.length >= 1) {
      const obs = rawHist.map((p) => ({ ts: p[0], equity: p[1] }));
      let pts = equityWindow(obs, hcRange, Date.now()).points.map((p) => [p.ts, p.equity]);
      if (pts.length < 2 && hcRange !== "all") pts = rawHist.slice(-3);
      const series = Number.isFinite(eq) && eq > 0 ? withNow(pts, eq) : pts;
      if (series.length >= 2) {
        renderEquityChart("hc-chart", series, "基差影子收益曲线", undefined, undefined, undefined, {
          equityLabel: "影子权益 · 最新",
          changeLabel: "记录区间变化",
          legendName: "基差影子",
        });
      } else {
        hcChartEl.innerHTML = '<div class="empty">已记录 1 个采样点，等待下一个小时采样点形成曲线。</div>';
      }
    } else {
      hcChartEl.innerHTML = '<div class="empty">暂无曲线数据，按预算重置模拟后将每小时自动记录。</div>';
    }
  }
}

async function refreshHlCarry() {
  try {
    const d = await fetchJSONRetry("/api/hlcarry", { cache: "no-store" });
    lastHlCarryData = d;
    try {
      renderHlCarry(d);
    } catch (err) {
      const m = $("hc-metrics");
      if (m) m.innerHTML = `<div class="empty neg">渲染失败：${escapeHTML(String((err && err.message) || err))}</div>`;
      console.error("renderHlCarry", err, d);
    }
  } catch (e) {
    const m = $("hc-metrics");
    if (m) m.innerHTML = `<div class="empty neg">读取失败：${escapeHTML(String((e && e.message) || e))}</div>`;
  }
}

document.querySelectorAll("#hc-chart-tabs button").forEach((b) => {
  b.addEventListener("click", () => {
    hcRange = b.dataset.range || "all";
    document.querySelectorAll("#hc-chart-tabs button").forEach((x) => x.setAttribute("aria-pressed", String(x === b)));
    if (lastHlCarryData) renderHlCarry(lastHlCarryData);
  });
});

document.addEventListener("click", (e) => {
  const b = e.target.closest("[data-pager^='hc']");
  if (!b) return;
  const to = Number(b.dataset.to);
  if (!Number.isFinite(to)) return;
  if (b.dataset.pager === "hcleg") hcLegPage = Math.max(0, to);
  if (b.dataset.pager === "hcrb") hcRbPage = Math.max(0, to);
  if (b.dataset.pager === "hcdl") hcDailyPage = Math.max(0, to);
  if (b.dataset.pager === "hclog") hcLogPage = Math.max(0, to);
  if (lastHlCarryData) renderHlCarry(lastHlCarryData);
});
document.querySelectorAll('.nav button[data-view="hlcarry"]').forEach((b) => {
  b.addEventListener("click", () => refreshHlCarry());
});

const hcRefreshBtn = $("hc-refresh");
if (hcRefreshBtn) hcRefreshBtn.addEventListener("click", () => refreshHlCarry());

const hcStepBtn = $("hc-step");
if (hcStepBtn)
  hcStepBtn.addEventListener("click", async () => {
    hcStepBtn.disabled = true;
    try {
      await fetchJSONRetry("/api/hlcarry/step", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: "{}",
      });
      await refreshHlCarry();
    } catch (e) {
      alert("推进失败：" + ((e && e.message) || e));
    } finally {
      hcStepBtn.disabled = false;
    }
  });

// ---- 选币规则：保存 ----
const hcParamsBtn = $("hc-params-save");
if (hcParamsBtn)
  hcParamsBtn.addEventListener("click", async () => {
    const num = (id) => {
      const el = $(id);
      const v = el ? Number(el.value) : NaN;
      return Number.isFinite(v) ? v : null;
    };
    const maxw = num("hc-p-maxw");
    const body = {
      top_k: num("hc-p-topk"),
      min_funding_ann_pct: num("hc-p-minfund"),
      min_spot_volume_usd: num("hc-p-minvol"),
      max_leg_weight: maxw == null ? null : maxw / 100,
      payback_days: num("hc-p-payback"),
      min_hold_hours: num("hc-p-minhold"),
      basis_take_profit_bp: num("hc-p-basistp"),
    };
    // null 表示"没填"，服务端会保留原值
    for (const k of Object.keys(body)) if (body[k] == null) delete body[k];
    if (Object.keys(body).length === 0) {
      alert("没有可保存的参数。");
      return;
    }
    const st = $("hc-params-status");
    hcParamsBtn.disabled = true;
    try {
      const r = await fetchJSONRetry("/api/hlcarry/params", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      if (st) st.textContent = "已保存。规则在下一次评估时生效，当前持仓不动。";
      await refreshHlCarry();
      return r;
    } catch (e) {
      if (st) st.textContent = "保存失败：" + ((e && e.message) || e);
    } finally {
      hcParamsBtn.disabled = false;
    }
  });

const HC_BUDGET_MAX = 10000000;

// 不用 window.confirm：浏览器在多次弹窗后会静默屏蔽，返回 false，
// 表现就是"点了没反应"。改成页内两步确认 + 明确的状态提示。
let hcOpenArmed = false;
let hcOpenArmTimer = 0;

const hcOpenBtn = $("hc-open");
function hcBudgetStatus(msg, cls) {
  const el = $("hc-budget-status");
  if (!el) return;
  el.textContent = msg || "";
  el.className = "note" + (cls ? " " + cls : "");
}
function hcOpenDisarm() {
  hcOpenArmed = false;
  if (hcOpenArmTimer) {
    clearTimeout(hcOpenArmTimer);
    hcOpenArmTimer = 0;
  }
  if (hcOpenBtn) {
    hcOpenBtn.textContent = "按预算重置模拟";
    if (hcOpenBtn.classList) hcOpenBtn.classList.remove("is-armed");
  }
}

if (hcOpenBtn)
  hcOpenBtn.addEventListener("click", async () => {
    const input = $("hc-budget");
    const budget = Number(input ? input.value : NaN);

    // 第一步：校验
    if (!Number.isFinite(budget) || budget <= 0) {
      hcOpenDisarm();
      hcBudgetStatus("预算必须是正数（美元）。", "neg");
      if (input) input.focus();
      return;
    }
    if (budget > HC_BUDGET_MAX) {
      hcOpenDisarm();
      hcBudgetStatus("预算上限 $" + HC_BUDGET_MAX.toLocaleString("en-US") + "，请检查是否填错。", "neg");
      if (input) input.focus();
      return;
    }

    const cur = Number(window.__hcBudget) || 0;
    const fmt2 = (v) => "$" + Number(v).toLocaleString("en-US", { maximumFractionDigits: 2 });

    // 第二步：页内确认（再点一次同一个按钮）
    if (!hcOpenArmed) {
      hcOpenArmed = true;
      hcOpenBtn.textContent = "确认重置并建仓";
      if (hcOpenBtn.classList) hcOpenBtn.classList.add("is-armed");
      hcBudgetStatus(
        (cur > 0
          ? `将清空当前持仓、累计盈亏与权益曲线，预算 ${fmt2(cur)} → ${fmt2(budget)}。`
          : `将按 ${fmt2(budget)} 开始模拟建仓。`) + " 请再点一次确认执行；5 秒后自动取消。",
      );
      hcOpenArmTimer = setTimeout(() => {
        hcOpenDisarm();
        hcBudgetStatus("确认已过期，未执行。需要重置时请再次点击按钮。", "");
      }, 5000);
      return;
    }

    // 第三步：执行
    hcOpenDisarm();
    hcOpenBtn.disabled = true;
    hcBudgetStatus("正在建仓…");
    try {
      // 重置会清空影子历史，POST 不做自动重试，避免超时后重复重置账本。
      const result = await fetchJSON("/api/hlcarry/open", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ budget_usd: budget }),
      }, 20000);
      await refreshHlCarry();
      if (Number(result.legs) > 0) {
        hcBudgetStatus(`预算已重置为 ${fmt2(budget)}，模拟建仓 ${result.legs} 条腿。`, "pos");
      } else if (result.legs === 0) {
        hcBudgetStatus(`预算已保存为 ${fmt2(budget)}，但当前没有符合规则的标的，模拟账户暂时空仓。`, "");
      } else {
        hcBudgetStatus(`已按 ${fmt2(budget)} 完成模拟账户重置。`, "pos");
      }
    } catch (e) {
      const reason = e && e.name === "AbortError"
        ? "请求超时，服务器可能仍在处理"
        : ((e && e.message) || String(e));
      await refreshHlCarry();
      hcBudgetStatus("重置未获确认：" + reason + "。已刷新状态，请核对当前预算与持仓后再操作。", "neg");
    } finally {
      hcOpenBtn.disabled = false;
    }
  });

const hcBudgetInput = $("hc-budget");
if (hcBudgetInput)
  hcBudgetInput.addEventListener("input", () => {
    if (!hcOpenArmed) return;
    hcOpenDisarm();
    hcBudgetStatus("预算已修改，请重新确认后才会执行。", "");
  });

const themeToggleBtn = $("theme-toggle");
if (themeToggleBtn) {
  const syncThemeLabel = () => {
    const isLight = document.documentElement.dataset.theme === "light";
    themeToggleBtn.title = isLight ? "切换为深色风格" : "切换为浅色风格";
    themeToggleBtn.setAttribute("aria-label", themeToggleBtn.title);
  };
  syncThemeLabel();
  themeToggleBtn.addEventListener("click", () => {
    const next = document.documentElement.dataset.theme === "light" ? "dark" : "light";
    document.documentElement.dataset.theme = next;
    try { localStorage.setItem("stars_theme", next); } catch (_) {}
    syncThemeLabel();
  });
}

(() => {
  let pop = null;
  const ensurePop = () => {
    if (!pop) {
      pop = document.createElement("div");
      pop.className = "trade-legs-floating";
      pop.hidden = true;
      document.body.appendChild(pop);
    }
    return pop;
  };
  const showPop = (wrap) => {
    const src = wrap.querySelector(".trade-legs-pop");
    if (!src) return;
    const el = ensurePop();
    el.innerHTML = src.innerHTML;
    el.hidden = false;
    const rect = wrap.getBoundingClientRect();
    const pr = el.getBoundingClientRect();
    let left = Math.min(window.innerWidth - pr.width - 12, Math.max(12, rect.right - pr.width));
    let top = rect.top - pr.height - 8;
    if (top < 12) top = Math.min(window.innerHeight - pr.height - 12, rect.bottom + 8);
    el.style.left = left + "px";
    el.style.top = top + "px";
  };
  const hidePop = () => { if (pop) pop.hidden = true; };
  document.addEventListener("mouseover", (e) => {
    const wrap = e.target.closest(".trade-legs-wrap");
    if (wrap) showPop(wrap);
  });
  document.addEventListener("mouseout", (e) => {
    const wrap = e.target.closest(".trade-legs-wrap");
    if (wrap && !wrap.contains(e.relatedTarget)) hidePop();
  });
  document.addEventListener("focusin", (e) => {
    const wrap = e.target.closest(".trade-legs-wrap");
    if (wrap) showPop(wrap);
  });
  document.addEventListener("focusout", (e) => {
    const wrap = e.target.closest(".trade-legs-wrap");
    if (wrap && !wrap.contains(e.relatedTarget)) hidePop();
  });
  window.addEventListener("scroll", hidePop, true);
})();
