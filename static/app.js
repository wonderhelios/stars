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

// ---------- 导航 ----------
document.querySelectorAll(".nav button").forEach((b) => {
  b.addEventListener("click", () => {
    document.querySelectorAll(".nav button").forEach((x) => x.classList.remove("active"));
    b.classList.add("active");
    document.querySelectorAll(".view").forEach((v) => v.classList.remove("active"));
    $("view-" + b.dataset.view).classList.add("active");
  });
});

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
        `已缓存 ${d.cached_coins} 币 · 流动性 ${d.universe_liquid} · ${d.paper?.running ? "纸交易运行中" : "纸交易未启动"}`;
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
const LIVE_PAGE_SIZE = 5;

// 通用分页条；点击由全局委托处理

document.addEventListener("click", (e) => {
  const btn = e.target.closest("[data-pager]");
  if (!btn) return;
  const key = btn.dataset.pager;
  const to = Number(btn.dataset.to);
  if (key === "live") {
    livePage = to;
    renderMonitor(lastLiveData);
  } else if (key === "pos") {
    posPage = to;
    renderMonitor(lastLiveData);
  } else if (key === "order") {
    orderPage = to;
    renderMonitor(lastLiveData);
  }
});

// 每日盈亏：从净值曲线上取相邻两点的差

// 持有期分析：赚的钱来自长仓还是短仓

// 选一组好看的坐标轴刻度



// ==================== 实盘设置 ====================
let liveLoaded = false;
let liveBusy = false;

function lvConfigInto(d) {
  const c = d.config || {};
  $("lv-account").value = c.account || "";
  $("lv-key").value = c.key_path || "";
  $("lv-positions").value = c.target_positions ?? 8;
  $("lv-leverage").value = c.leverage ?? 3;
  $("lv-buffer").value = Math.round((c.margin_buffer ?? 0.9) * 100);
  $("lv-slippage").value = ((c.slippage ?? 0.005) * 100).toFixed(2);
  $("lv-slices").value = c.rebalance_slices ?? 3;
  $("lv-tp").value = Math.round((c.take_profit_pct ?? 0.1) * 100);
  if ($("lv-momw")) $("lv-momw").value = Math.round((c.momentum_weight ?? 0.5) * 100);
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
    target_positions: Number($("lv-positions").value),
    leverage: Number($("lv-leverage").value),
    margin_buffer: Number($("lv-buffer").value) / 100,
    slippage: Number($("lv-slippage").value) / 100,
    rebalance_slices: Number($("lv-slices").value),
    take_profit_pct: Number($("lv-tp").value) / 100,
    momentum_weight: ($("lv-momw") ? Number($("lv-momw").value) : 50) / 100,
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

function renderCapitalFlows(d) {
  const el = $("lv-flow-history");
  if (!el) return;
  const key = JSON.stringify([d.account, d.capital_flows]);
  if (el.flowKey === key) return; // Polling must not overwrite an in-progress time correction.
  el.flowKey = key;
  const flows = (d.capital_flows || []).map((f,index) => ({...f,index})).reverse();
  el.innerHTML = flows.length ? flows.map(f => `<div class="flow-entry"><div class="flow-row"><time>${ts2m(f.effective_ts ?? f.ts)} UTC${f.effective_ts == null ? " · 登记时间" : " · 到账时间"}</time><b>${f.amount >= 0 ? "+" : "−"}$${fmt(Math.abs(f.amount), 2)}</b></div><details><summary>校正到账时间</summary><input aria-label="到账时间 UTC" type="datetime-local" step="1" value="${new Date(f.effective_ts ?? f.ts).toISOString().slice(0,19)}"><button type="button" class="btn ghost" data-flow-index="${f.index}" data-flow-ts="${f.ts}" data-flow-account="${escapeHTML(d.account)}">保存时间</button></details></div>`).join("") : '<p>暂无登记流水</p>';

}

function setupCapitalFlow() {
  const btn = $("lv-flow-btn");
  if (!btn) return;
  btn.addEventListener("click", async () => {
    const input = $("lv-flow");
    const amount = Number(input && input.value);
    if (!amount || !Number.isFinite(amount)) { alert("请填非零金额（正 = 入金，负 = 出金）"); return; }
    btn.textContent = "记录中…"; btn.disabled = true;
    try {
      const res = await fetch("/api/live/capital", {
        method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ amount, effective_ts: $("lv-flow-time")?.value ? Date.parse($("lv-flow-time").value + "Z") : null })
      });
      const j = await res.json();
      if (res.ok && j.ok) {
        if (input) input.value = "";
        $("lv-flow-status").textContent = "流水已保存，正在刷新资金指标…";
        try { const latest = await fetchLive(); liveFetchedAt = Date.now(); renderMonitor(latest); $("lv-flow-status").textContent = "流水已保存，资金指标已更新。"; }
        catch (_) { $("lv-flow-status").textContent = "流水已保存；账户刷新失败，请点击监控页刷新，无需重复记录。"; }
      }
      else { alert(j.error || ("HTTP " + res.status)); }
    } catch (e) { alert("请求失败: " + e); }
    finally { btn.textContent = "记录"; btn.disabled = false; }
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

function renderMonitor(d) {
  if (!d) return;
  lastLiveData = d;
  renderCapitalFlows(d);
  renderCapitalAlloc(d.capital_alloc);
  const c = d.config || {};
  const pos = d.positions || [];
  const hist = d.history || [];
  const eq = d.equity || 0;
  const capital = capitalPerformance(d);
  const valid = capital.valid;
  const pnl = capital.change;
  const pnlPct = capital.percent;
  const buffer = d.liq_buffer_pct || 0;

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
  if (d.error) {
    cls = "idle";
    tag = "● 账户数据不可用";
    text = escapeHTML(d.error);
  } else if (c.armed && keyed && holding) {
    cls = "run";
    tag = "● 运行中";
    text = `<b>实盘已启动</b> · 持有 <b>${pos.length}</b> 个仓位 · 总名义 $${fmt(d.gross_notional || 0, 0)}`;
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
  const isoN = d.isolated_count || 0;
  if (isoN > 0) {
    text += `<span class="neg"><b>⚠ ${isoN} 个仓位仍是逐仓</b>（Hyperliquid 不允许持仓时切换模式，需用「转为全仓」逐币处理）</span>`;
  }
  const autoText = auto
    ? `<span>每日自动调仓：<b>已开启</b>，下次 <b>${nextRun} UTC</b>（北京时间 08:05）</span>`
    : `<span>每日自动调仓：<b>已关闭</b>（需手动点「执行调仓」）</span>`;
  const lastText = d.last_run_at
    ? `<span>上次调仓 <b>${ts2m(d.last_run_at)}</b>${d.last_live ? "（真实下单）" : "（仅生成计划）"}</span>`
    : "<span>还没有调仓记录</span>";
  const ago = liveFetchedAt ? Math.round((Date.now() - liveFetchedAt) / 1000) : null;
  const freshness = ago == null ? "" : `<span class="muted" id="mo-ago">数据更新于 ${ago} 秒前</span>`;
  $("mo-status").innerHTML = `<div class="live-status ${cls}">
    <span class="tag">${tag}</span><span>${text}</span>${autoText}${lastText}${freshness}
  </div>`;

  const parsed = chronRecords(d);
  const realized = parsed.realized;
  const slip = parsed.slip;
  const targetLev = (c.leverage || 0) * (c.margin_buffer || 1);
  const actualLev = eq > 0 ? (d.gross_notional || 0) / eq : 0;
  const nPos = pos.length;
  const nCross = nPos - (d.isolated_count || 0);
  const currentValid = !d.error && Number.isFinite(d.equity) && pos.every(p => Number.isFinite(p.unrealized));
  const unrealized = pos.reduce((sum, p) => sum + (Number.isFinite(p.unrealized) ? p.unrealized : 0), 0);

  // 拿不到行情的持仓：必须显眼提示，否则净值看着正常但风险是隐形的
  const unp = d.unpriced || [];
  $("mo-warn").innerHTML = unp.length
    ? `<div class="warn-banner">⚠ ${unp.length} 个持仓拿不到行情（${unp
        .slice(0, 8)
        .join(" ")}${unp.length > 8 ? " …" : ""}）—— 已改用交易所市值估值，
        但这些仓位的强平距离无法计算，请检查是否已退市。</div>`
    : "";

  $("mo-metrics").innerHTML =
    metricGroup(
      "实时概览 · 当前账户与持仓",
      metric("账户净值", currentValid ? "$" + fmt(eq, 2) : "—") +
      metric("未实现盈亏 · PNL", currentValid ? moneySigned(unrealized) : "—", currentValid ? (unrealized >= 0 ? "pos" : "neg") : "") +
      metric("持仓数量", currentValid ? nPos + '<span class="metric-unit"> 个</span>' : "—") +
      metric("实际杠杆", currentValid ? fmt(actualLev, 2) + '<span class="metric-unit"> x</span>' : "—"),
      "live-overview",
      '<div class="metric-footnote"><span class="live-dot"></span>未实现盈亏为当前持仓浮盈浮亏，不与历史已实现盈亏相加。</div>'
    ) +
    metricGroup(
      "资金表现 · 相对记录起点",
      metric("净投入本金", valid ? "$" + fmt(capital.principal, 2) : "—") +
        metric("起点后净入金", valid ? moneySigned(capital.netDep) : "—") +
        metric("扣除出入金后净收益", valid ? moneySigned(pnl) : "—", valid && pnl >= 0 ? "pos" : "neg") +
        metric("净投入收益率", pnlPct != null ? pct(pnlPct) : "—", pnlPct != null ? (pnlPct >= 0 ? "pos" : "neg") : ""),
      "capital-account",
      `<details class="metric-definition"><summary>原始起点 ${valid ? "$" + fmt(capital.base, 2) : "—"} · ${d.capital_baseline?.ts ? ts2m(d.capital_baseline.ts) + " UTC" : "时间未知"} · 查看口径</summary><p>净投入本金 = 记录起点资金 + 起点后净入金；净收益 = 当前净值 − 净投入本金；净投入收益率 = 净收益 ÷ 净投入本金。本金不大于零时不显示收益率。这是简单本金收益率，不是时间加权或年化收益率。已体现账户手续费与资金费，准确性依赖出入金记录完整。记录入金不修改历史起点；旧账户的记录起点可能晚于首次启动。</p></details>`
    ) +
    metricGroup(
      "风险与仓位",
      metric("账户强平缓冲", currentValid ? fmt(buffer, 1) + "%" : "—", buffer < 40 ? "neg" : "pos") +
        metric("净敞口 · 多减空", currentValid ? moneySigned(d.net_notional || 0) : "—", Math.abs(d.net_notional || 0) <= eq * 0.1 ? "" : "neg") +
        metric("实际 / 目标杠杆", fmt(actualLev, 2) + "x / " + fmt(targetLev, 2) + "x",
          actualLev < targetLev * 0.9 ? "neg" : "pos") +
        metric(
          "保证金模式",
          // 全是全仓时只说「全仓」。逐仓是早期一个真实 bug（is_cross 传了 false），
          // 现已修复；把「0 逐仓」挂在卡片上只会让人以为还有问题。
          nPos
            ? (d.isolated_count || 0) > 0
              ? nCross + " 全仓 / " + d.isolated_count + " 逐仓"
              : nCross + " 全仓"
            : "—",
          (d.isolated_count || 0) > 0 ? "neg" : "pos"
        )
    ) +
    metricGroup(
      "执行质量与回执",
      metric("实测平均滑点", slip ? slip.avg.toFixed(3) + "%" : "—", slip && slip.avg > 0.15 ? "neg" : "pos") +
        metric("补录已实现 · 未扣费", parsed.n ? moneySigned(realized) : "—",
          realized >= 0 ? "pos" : "neg") +
        metric("总名义敞口", "$" + fmt(d.gross_notional || 0, 0)) +
        metric("当前挂单", currentValid ? (d.tp_orders || []).length + " 笔" : "—") +
        metric("最后调仓", d.last_run_at ? `<span class="sm">${ts2m(d.last_run_at)}</span>` : "—")
    );

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
    const posPages = Math.max(1, Math.ceil(pos.length / LIVE_PAGE_SIZE));
    if (posPage > posPages - 1) posPage = 0;
    const rows = pos
      .slice(posPage * LIVE_PAGE_SIZE, (posPage + 1) * LIVE_PAGE_SIZE)
      .map((p) => {
        const cls = p.unrealized >= 0 ? "pos" : "neg";
        const dist = p.dist_pct == null ? "—" : fmt(p.dist_pct, 1) + "%";
        return `<tr>
          <td>${p.coin}</td>
          <td>${sideLabel(p.side)}</td>
          <td>${fmtCompact(p.size)}</td>
          <td>${fmtCompact(p.entry_px)}</td>
          <td>${fmtCompact(p.mark_px)}</td>
          <td>$${fmt(p.notional, 0)}</td>
          <td class="${cls}">${moneySigned(p.unrealized)}</td>
          <td>${p.liq_px == null ? "—" : fmtCompact(p.liq_px)}</td>
          <td class="${p.dist_pct != null && p.dist_pct < 20 ? "neg" : "muted"}">${dist}</td>
          <td>${p.is_cross ? '<span class="badge-mini ok">全仓</span>' : '<span class="badge-mini err">逐仓</span>'}</td>
        </tr>`;
      })
      .join("");
    $("mo-positions").innerHTML = err + `<div class="table-scroll"><table><thead><tr>
      <th>币</th><th>方向</th><th>数量</th><th>入场价</th><th>当前价</th>
      <th>名义</th><th>未实现盈亏</th><th>强平价</th><th>距强平</th><th>保证金模式</th>
    </tr></thead><tbody>${rows}</tbody></table></div>` + pager("pos", pos.length, posPage, LIVE_PAGE_SIZE);
  }

  renderExecDaily(d);
  renderLiveChart(hist, d.shadow);

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
              <td class="muted">${escapeHTML(recordAction(r))}${r.tid != null ? '<span class="record-origin">成交回执 · 意图未确认</span>' : '<span class="record-origin">系统订单</span>'}</td>
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
            <td class="muted">${escapeHTML(recordAction(r))}${r.tid != null ? '<span class="record-origin">成交回执 · 意图未确认</span>' : '<span class="record-origin">系统订单</span>'}</td>
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

function renderLiveChart(history, shadow) {
  const el = $("mo-chart");
  const now = Date.now();
  const observations = [...(history || [])];
  if (lastLiveData && !lastLiveData.error && Number.isFinite(lastLiveData.equity) && liveFetchedAt > 0) {
    observations.push({ ts: liveFetchedAt, equity: lastLiveData.equity });
  }
  const adjusted = strategyHistory(observations, lastLiveData || {});
  if (adjusted.error) { el.innerHTML = `<div class="empty">${escapeHTML(adjusted.error)}</div>`; return; }
  const { raw, points } = equityWindow(adjusted.history, chartRange, now);
  const names = { day: "最近 24 小时", week: "最近 7 天", month: "最近 30 天", all: "全部留存记录" };
  if (!raw.length) {
    el.innerHTML = `<div class="empty">${names[chartRange]}暂无策略盈亏记录。可以切换更长区间查看。</div>`;
    return;
  }
  const first = raw[0], last = raw.at(-1), change = last.equity - first.equity;
  const W = Math.max(360, el.clientWidth || 960), H = 280;
  const padL = 64, padR = 22, padT = 24, padB = 36;
  const values = raw.map(p => p.equity);
  const ax = niceAxis(Math.min(...values), Math.max(...values), 4);
  const span = ax.hi - ax.lo || 1;
  const X = ts => padL + (last.ts === first.ts ? 0.5 : (ts - first.ts) / (last.ts - first.ts)) * (W - padL - padR);
  const Y = v => padT + (1 - (v - ax.lo) / span) * (H - padT - padB);
  const money = v => (v < 0 ? "−" : "+") + "$" + Math.abs(v).toFixed(2);
  const grid = ax.lines.map(v => `<line x1="${padL}" y1="${Y(v)}" x2="${W-padR}" y2="${Y(v)}" stroke="#eaf0f0"/><text x="${padL-10}" y="${Y(v)+4}" text-anchor="end" font-size="11" fill="#86949d">$${v.toFixed(span < 10 ? 2 : 0)}</text>`).join("");
  const label = ts => chartRange === "day" ? new Date(ts).toISOString().slice(11,16) : ts2m(ts).slice(0,5);
  const ticks = last.ts === first.ts ? [first.ts] : [first.ts, first.ts + (last.ts-first.ts)/2, last.ts];
  const xlabels = ticks.map((ts,i) => `<text x="${X(ts)}" y="${H-10}" text-anchor="${i === 0 ? "start" : i === ticks.length-1 ? "end" : "middle"}" font-size="11" fill="#86949d">${label(ts)}</text>`).join("");
  const line = points.map(p => `${X(p.ts)},${Y(p.equity)}`).join(" ");
  const circles = points.map(p => `<circle cx="${X(p.ts)}" cy="${Y(p.equity)}" r="6" fill="transparent" class="equity-point"><title>${ts2m(p.ts)} UTC · ${money(p.equity)}</title></circle>`).join("");
  el.innerHTML = `<div class="chart-summary"><div><span class="chart-eyebrow">累计策略盈亏 · 区间末值</span><strong>${money(last.equity)}</strong></div><div><span class="chart-eyebrow">区间策略盈亏</span><strong class="${change >= 0 ? "pos" : "neg"}">${change >= 0 ? "+" : "−"}$${Math.abs(change).toFixed(2)}</strong></div><div class="chart-coverage"><span class="chart-eyebrow">实际记录区间 · UTC</span>${ts2m(first.ts)} — ${ts2m(last.ts)}</div></div>
    <svg viewBox="0 0 ${W} ${H}" role="img" aria-label="${names[chartRange]}策略盈亏曲线">
      <defs><linearGradient id="equity-area" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="#168473" stop-opacity="0.14"/><stop offset="100%" stop-color="#168473" stop-opacity="0"/></linearGradient></defs>
      ${grid}
      ${points.length > 1 ? `<polygon points="${X(first.ts)},${H-padB} ${line} ${X(last.ts)},${H-padB}" fill="url(#equity-area)"/>` : ""}
      <line x1="${padL}" y1="${Y(first.equity)}" x2="${W-padR}" y2="${Y(first.equity)}" stroke="#bacac8" stroke-dasharray="4 5"/>
      <polyline points="${line}" fill="none" stroke="#168473" stroke-width="2.5" stroke-linejoin="round"/>
      <circle cx="${X(last.ts)}" cy="${Y(last.equity)}" r="4" fill="#168473" stroke="white" stroke-width="2"/>
      ${circles}${xlabels}
    </svg><div class="chart-caption"><span><i></i>策略盈亏 <span class="muted">· 虚线为区间起点</span></span><span>${raw.length === 1 ? "仅一条记录，等待下一次采样" : "悬停查看采样值"} · 已扣登记出入金，含手续费与资金费</span></div>${adjusted.estimated ? '<p class="note">旧流水按登记时间估算；如为事后补记，请在资金流水中校正到账时间，避免历史曲线出现错位跳变。</p>' : ""}`;
}


let liveFetchedAt = 0;

// 手机端省电：页面切到后台就停止轮询，切回来立刻拉一次
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) refreshMonitor();
});

async function refreshMonitor(force) {
  if (liveBusy) return;
  if (document.hidden && !force) return;
  try {
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
$("mo-clear").addEventListener("click", async () => {
  if (!confirm("清空订单与成交记录？净值曲线、记录起点资金和配置均保留。")) return;
  try {
    const res = await fetch("/api/live/records/clear", { method: "POST" });
    const d = await res.json();
    if (d.ok) refreshMonitor(true);
  } catch (e) {
    alert("清空失败：" + e);
  }
});





$("mo-chart-tabs").addEventListener("click", event => {
  const button = event.target.closest("button[data-range]");
  if (!button) return;
  chartRange = button.dataset.range;
  $("mo-chart-tabs").querySelectorAll("button").forEach(b => b.setAttribute("aria-pressed", String(b === button)));
  if (lastLiveData) renderLiveChart(lastLiveData.history, lastLiveData.shadow);
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
    if (!res.ok) throw new Error("HTTP " + res.status);
    return await res.json();
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

let lastFundingData = null;
let lastCarryData = null;

const annPct = (v) => Number.isFinite(v) ? (v * 100).toFixed(1) + "%" : "—";
const annSigned = (v) => Number.isFinite(v) ? (v >= 0 ? "+" : "−") + Math.abs(v * 100).toFixed(1) + "%" : "—";
const usdM = (v) => Number.isFinite(v) ? (v >= 1e6 ? "$" + (v / 1e6).toFixed(1) + "M" : "$" + (v / 1e3).toFixed(0) + "K") : "—";

// 资金费差：只显示摘要 + 前 5 个机会，全表折叠。
function renderFunding(d) {
  const up = $("fd-updated");
  if (up) {
    up.textContent = d && d.updated
      ? "数据更新于 " + ts2m(d.updated) + " UTC" + (d.bn_error ? " · ⚠ 币安：" + d.bn_error : "")
      : "暂无数据";
  }
  const pairsEl = $("fd-pairs");
  if (pairsEl) {
    const pairs = (d && d.pairs) || [];
    if (!pairs.length) {
      pairsEl.innerHTML = '<div class="empty">还没有数据（等后台刷新）</div>';
    } else {
      const avg = pairs.reduce((a, p) => a + Math.abs(p.diff_ann), 0) / pairs.length;
      const row = (p) => `<tr>
          <td>${escapeHTML(p.coin)}</td>
          <td>${annPct(p.hl_ann)}</td>
          <td>${annPct(p.bn_ann)}</td>
          <td class="${p.diff_ann >= 0 ? "pos" : "neg"}">${annSigned(p.diff_ann)}</td>
          <td class="muted">${p.diff_ann >= 0 ? "空 HL · 多币安" : "多 HL · 空币安"}</td>
          <td class="muted">${usdM(p.hl_vol)} / ${usdM(p.bn_vol)}</td>
        </tr>`;
      const head = `<tr><th>币</th><th>HL</th><th>币安</th><th>差</th><th>方向</th><th>成交额 HL/币安</th></tr>`;
      const top = pairs.slice(0, 5);
      let html = `<p class="note">合格 <b>${pairs.length}</b> 个币 · 平均 |差| <b>${(avg * 100).toFixed(2)}%</b> · 最大 <b>${escapeHTML(pairs[0].coin)} ${annSigned(pairs[0].diff_ann)}</b></p>`;
      html += `<div class="table-scroll"><table><thead>${head}</thead><tbody>${top.map(row).join("")}</tbody></table></div>`;
      if (pairs.length > 5) {
        html += `<details class="setup-details"><summary>展开全部 ${pairs.length} 个</summary>
          <div class="table-scroll"><table><thead>${head}</thead><tbody>${pairs.map(row).join("")}</tbody></table></div></details>`;
      }
      pairsEl.innerHTML = html;
    }
  }
  const spikesEl = $("fd-spikes");
  if (spikesEl) {
    const spikes = (d && d.spikes) || [];
    if (!spikes.length) {
      spikesEl.innerHTML = '<div class="empty">当前没有 ≥40% 的尖峰</div>';
    } else {
      const pos = spikes.filter((s) => s.ann > 0).length;
      const row = (s) => `<tr>
          <td>${escapeHTML(s.market)}</td>
          <td class="${s.ann >= 0 ? "pos" : "neg"}">${annSigned(s.ann)}</td>
          <td>${usdM(s.vol)}</td>
          <td class="muted">${s.ann >= 0 ? "空头收钱" : "多头收钱"}</td>
        </tr>`;
      const head = `<tr><th>市场</th><th>年化资金费</th><th>成交额</th><th>收钱方向</th></tr>`;
      let html = `<p class="note">共 <b>${spikes.length}</b> 个尖峰 · 正资金费 <b>${pos}</b>（做空收钱）/ 负 <b>${spikes.length - pos}</b>（做多收钱）</p>`;
      html += `<div class="table-scroll"><table><thead>${head}</thead><tbody>${spikes.slice(0, 5).map(row).join("")}</tbody></table></div>`;
      if (spikes.length > 5) {
        html += `<details class="setup-details"><summary>展开全部 ${spikes.length} 个</summary>
          <div class="table-scroll"><table><thead>${head}</thead><tbody>${spikes.map(row).join("")}</tbody></table></div></details>`;
      }
      spikesEl.innerHTML = html;
    }
  }
}

async function refreshFunding(force) {
  const viewEl = $("view-funding");
  const viewActive = viewEl && viewEl.classList.contains("active");
  if (!force && !viewActive && lastFundingData) return; // 非激活视图不刷（省接口）
  try {
    lastFundingData = await fetchJSONRetry(force ? "/api/funding/refresh" : "/api/funding", {
      method: force ? "POST" : "GET",
    }, force ? 45000 : 8000);
    renderFunding(lastFundingData);
  } catch (e) {
    // 静默失败，等下一轮
  }
}

function renderCapitalAlloc(alloc) {
  alloc = alloc || {};
  const m = $("fd-cap-momentum"), c = $("fd-cap-carry"), h = $("fd-cap-hunting");
  // 用户正在编辑时不覆盖输入（renderMonitor 每 20 秒会重入一次）。
  const editing = [m, c, h].some((el) => el && document.activeElement === el);
  if (!editing) {
    if (m) m.value = Number.isFinite(alloc.momentum_usd) ? alloc.momentum_usd : 0;
    if (c) c.value = Number.isFinite(alloc.carry_usd) ? alloc.carry_usd : 0;
    if (h) h.value = Number.isFinite(alloc.hunting_usd) ? alloc.hunting_usd : 0;
  }
  const total = (alloc.momentum_usd || 0) + (alloc.carry_usd || 0) + (alloc.hunting_usd || 0);
  const s = $("fd-cap-summary");
  if (s) {
    s.textContent = total > 0
      ? `合计 $${fmt(total, 0)} · 动量 ${fmt((alloc.momentum_usd || 0) / total * 100, 0)}% · carry ${fmt((alloc.carry_usd || 0) / total * 100, 0)}% · 尖峰 ${fmt((alloc.hunting_usd || 0) / total * 100, 0)}%`
      : "还没有配置资金矩阵";
  }
}

if ($("fd-cap-save")) {
  $("fd-cap-save").addEventListener("click", async () => {
    const body = {
      momentum_usd: Number($("fd-cap-momentum").value) || 0,
      carry_usd: Number($("fd-cap-carry").value) || 0,
      hunting_usd: Number($("fd-cap-hunting").value) || 0,
    };
    const st = $("fd-cap-status");
    try {
      const res = await fetch("/api/capital", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      const out = await res.json();
      if (!res.ok || !out.ok) {
        st.textContent = "保存失败：" + (out.error || ("HTTP " + res.status));
        return;
      }
      st.textContent = "已保存。";
      renderCapitalAlloc(body);
    } catch (e) {
      st.textContent = "保存失败：" + e;
    }
  });
}

if ($("fd-refresh")) {
  $("fd-refresh").addEventListener("click", () => refreshFunding(true));
}

// 切到资金费套利视图时刷新（资金矩阵已并入 renderMonitor 的 20 秒轮询）。
// 进入套利监控 / 套利设置时刷新对应数据
document.querySelectorAll('.nav button[data-view="funding"], .nav button[data-view="funding-setup"]').forEach((b) => {
  b.addEventListener("click", () => {
    refreshFunding();
    refreshCarry();
    if (b.dataset.view === "funding-setup") {
      fetchJSONRetry("/api/live", {}, 8000)
        .then((d) => renderCapitalAlloc(d.capital_alloc))
        .catch(() => {});
    }
  });
});

// 启动：首次拉取 + 每 60 秒轮询。
refreshFunding();
refreshCarry();
setInterval(() => refreshFunding(false), 60000);

// ---------- 纸面 carry 策略 ----------

// 纸面 carry：摘要 + 权益曲线 + 两腿汇总，明细折叠。
function renderCarry(d) {
  d = d || {};
  const pairs = d.pairs || [];
  const open = pairs.reduce((a, p) => a + p.funding_accrued + p.basis_pnl - p.entry_fee, 0);
  const closed = d.closed_pnl || 0;
  const total = closed + open;
  const base = d.start_equity || 0;
  const eq = base + total;
  const sum = $("cy-summary");
  if (sum) {
    sum.innerHTML =
      `<span>预算 <b>$${fmt(d.budget_usd || 0, 0)}</b></span> · ` +
      `<span>权益 <b>$${fmt(eq, 2)}</b> ` +
      `<b class="${total >= 0 ? "pos" : "neg"}">${base > 0 ? (total / base * 100).toFixed(2) + "%" : "—"}</b></span> · ` +
      `<span>已实现 <b class="${closed >= 0 ? "pos" : "neg"}">${moneySigned(closed)}</b></span> · ` +
      `<span>未实现 <b class="${open >= 0 ? "pos" : "neg"}">${moneySigned(open)}</b></span> · ` +
      `<span>持仓 <b>${pairs.length}</b> 对</span>`;
  }
  renderCarryChart(d.history || []);
  const legs = $("cy-legs");
  if (legs) {
    const agg = (dir) => {
      const g = pairs.filter((p) => (dir > 0 ? p.direction > 0 : p.direction < 0));
      return {
        n: g.length,
        notional: g.reduce((a, p) => a + p.notional, 0),
        funding: g.reduce((a, p) => a + p.funding_accrued, 0),
        basis: g.reduce((a, p) => a + p.basis_pnl, 0),
        net: g.reduce((a, p) => a + p.funding_accrued + p.basis_pnl - p.entry_fee, 0),
      };
    };
    const box = (title, a) => `<div class="content-card" style="padding:14px 16px">
        <div class="panel-head" style="margin:0"><span class="section-eyebrow">${title}</span><b>${a.n} 对</b></div>
        <p class="note" style="margin:6px 0 0">名义 $${fmt(a.notional, 0)} · 资金费 <b class="${a.funding >= 0 ? "pos" : "neg"}">${moneySigned(a.funding)}</b> · 基差 <b class="${a.basis >= 0 ? "pos" : "neg"}">${moneySigned(a.basis)}</b> · 净 <b class="${a.net >= 0 ? "pos" : "neg"}">${moneySigned(a.net)}</b></p></div>`;
    legs.innerHTML = pairs.length
      ? `<div class="grid-2">${box("空 HL · 多币安", agg(1))}${box("多 HL · 空币安", agg(-1))}</div>`
      : "";
  }
  const posEl = $("cy-positions");
  if (posEl) {
    posEl.innerHTML = pairs.length
      ? `<div class="table-scroll"><table><thead><tr><th>币</th><th>方向</th><th>每腿名义</th><th>进场差</th><th>资金费</th><th>基差</th><th>净</th><th>已持</th></tr></thead><tbody>${
          pairs.map((p) => {
            const net = p.funding_accrued + p.basis_pnl - p.entry_fee;
            const h = Math.max(0, (Date.now() - p.entry_ts) / 3600000).toFixed(0);
            return `<tr><td>${escapeHTML(p.coin)}</td>
              <td class="muted">${p.direction > 0 ? "空HL·多BN" : "多HL·空BN"}</td>
              <td>$${fmt(p.notional, 0)}</td>
              <td>${(p.entry_diff * 100).toFixed(1)}%</td>
              <td class="${p.funding_accrued >= 0 ? "pos" : "neg"}">${moneySigned(p.funding_accrued)}</td>
              <td class="${p.basis_pnl >= 0 ? "pos" : "neg"}">${moneySigned(p.basis_pnl)}</td>
              <td class="${net >= 0 ? "pos" : "neg"}">${moneySigned(net)}</td>
              <td class="muted">${h}h</td></tr>`;
          }).join("")
        }</tbody></table></div>`
      : '<div class="empty">没有纸面持仓。预算 &gt; 0 且某币 |价差| ≥ 开仓阈值时自动开。</div>';
  }
  const logEl = $("cy-log");
  if (logEl) {
    const logs = (d.log || []).slice(-40).reverse();
    logEl.innerHTML = logs.length
      ? logs.map((l) => `<div class="note">${ts2m(l.ts)} · ${escapeHTML(l.msg)}</div>`).join("")
      : '<div class="note">还没有操作。</div>';
  }
}

// 纸面 carry 权益曲线（每小时一个点）。
function renderCarryChart(history) {
  const el = $("cy-chart");
  if (!el) return;
  const pts = (history || [])
    .filter((p) => Array.isArray(p) && Number.isFinite(p[0]) && Number.isFinite(p[1]))
    .map((p) => ({ ts: p[0], eq: p[1] }));
  if (pts.length < 2) {
    el.innerHTML = '<div class="empty">还没有足够的权益点（每小时一个，跑两小时就能画线）</div>';
    return;
  }
  const W = Math.max(340, el.clientWidth || 900), H = 240;
  const padL = 64, padR = 20, padT = 20, padB = 30;
  const first = pts[0], last = pts[pts.length - 1];
  const change = last.eq - first.eq;
  const vals = pts.map((p) => p.eq);
  const ax = niceAxis(Math.min(...vals), Math.max(...vals), 4);
  const span = ax.hi - ax.lo || 1;
  const X = (ts) => padL + (last.ts === first.ts ? 0.5 : (ts - first.ts) / (last.ts - first.ts)) * (W - padL - padR);
  const Y = (v) => padT + (1 - (v - ax.lo) / span) * (H - padT - padB);
  const grid = ax.lines.map((v) => `<line x1="${padL}" y1="${Y(v)}" x2="${W - padR}" y2="${Y(v)}" stroke="#eaf0f0"/><text x="${padL - 10}" y="${Y(v) + 4}" text-anchor="end" font-size="11" fill="#86949d">$${v.toFixed(span < 10 ? 2 : 0)}</text>`).join("");
  const line = pts.map((p) => `${X(p.ts)},${Y(p.eq)}`).join(" ");
  const ticks = [first.ts, first.ts + (last.ts - first.ts) / 2, last.ts];
  const xlabels = ticks.map((ts, i) => `<text x="${X(ts)}" y="${H - 8}" text-anchor="${i === 0 ? "start" : i === ticks.length - 1 ? "end" : "middle"}" font-size="11" fill="#86949d">${ts2m(ts)}</text>`).join("");
  el.innerHTML = `<div class="chart-summary">
      <div><span class="chart-eyebrow">纸面权益 · 最新</span><strong>$${fmt(last.eq, 2)}</strong></div>
      <div><span class="chart-eyebrow">区间变化</span><strong class="${change >= 0 ? "pos" : "neg"}">${moneySigned(change)}</strong></div>
      <div class="chart-coverage"><span class="chart-eyebrow">点数</span>${pts.length} · ${ts2m(first.ts)} — ${ts2m(last.ts)}</div>
    </div>
    <svg viewBox="0 0 ${W} ${H}" role="img" aria-label="纸面 carry 权益曲线">
      <defs><linearGradient id="carry-area" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="#168473" stop-opacity="0.14"/><stop offset="100%" stop-color="#168473" stop-opacity="0"/></linearGradient></defs>
      ${grid}
      <polygon points="${X(first.ts)},${H - padB} ${line} ${X(last.ts)},${H - padB}" fill="url(#carry-area)"/>
      <line x1="${padL}" y1="${Y(first.eq)}" x2="${W - padR}" y2="${Y(first.eq)}" stroke="#bacac8" stroke-dasharray="4 5"/>
      <polyline points="${line}" fill="none" stroke="#168473" stroke-width="2.5" stroke-linejoin="round"/>
      <circle cx="${X(last.ts)}" cy="${Y(last.eq)}" r="4" fill="#168473" stroke="white" stroke-width="2"/>
      ${xlabels}
    </svg>`;
}

function renderCarryConfig(cfg) {
  cfg = cfg || {};
  const entry = $("cy-entry"), exit = $("cy-exit"), pairs = $("cy-pairs"), hours = $("cy-hours"), stop = $("cy-stop");
  // 60 秒轮询会重入：正在编辑的输入框不能被覆盖。
  if ([entry, exit, pairs, hours, stop].some((el) => el && document.activeElement === el)) return;
  if (entry) entry.value = Math.round(cfg.entry_threshold * 1000) / 10;
  if (exit) exit.value = Math.round(cfg.exit_threshold * 1000) / 10;
  if (pairs) pairs.value = cfg.max_pairs;
  if (hours) hours.value = cfg.max_holding_hours;
  if (stop) stop.value = Math.round(cfg.stop_loss_pct * 1000) / 10;
}

async function refreshCarry() {
  try {
    lastCarryData = await fetchJSONRetry("/api/carry", {}, 8000);
    renderCarry(lastCarryData);
    renderCarryConfig(lastCarryData.config);
  } catch (e) {
    // 服务器偶尔被写盘卡住：8 秒超时 + 重试，避免参数框一直空白
  }
}

if ($("cy-save")) {
  $("cy-save").addEventListener("click", async () => {
    const body = {
      entry_threshold: (Number($("cy-entry").value) || 0) / 100,
      exit_threshold: (Number($("cy-exit").value) || 0) / 100,
      max_pairs: Number($("cy-pairs").value) || 0,
      max_holding_hours: Number($("cy-hours").value) || 0,
      stop_loss_pct: (Number($("cy-stop").value) || 0) / 100,
    };
    const st = $("cy-status");
    try {
      const res = await fetch("/api/carry/config", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      const out = await res.json();
      if (!res.ok || !out.ok) {
        st.textContent = "保存失败：" + (out.error || ("HTTP " + res.status));
        return;
      }
      st.textContent = "参数已保存。";
      lastCarryData = null;
      refreshCarry();
    } catch (e) {
      st.textContent = "保存失败：" + e;
    }
  });
}

if ($("cy-step")) {
  $("cy-step").addEventListener("click", async () => {
    const st = $("cy-status");
    try {
      const res = await fetch("/api/carry/step", { method: "POST", cache: "no-store" });
      if (!res.ok) throw new Error("HTTP " + res.status);
      lastCarryData = await res.json();
      renderCarry(lastCarryData);
      st.textContent = "已推进。";
    } catch (e) {
      st.textContent = "推进失败：" + e;
    }
  });
}

if ($("cy-reset")) {
  $("cy-reset").addEventListener("click", async () => {
    if (!confirm("重置纸面 carry：清空全部持仓、已实现盈亏与曲线（保留参数）。继续？")) return;
    const st = $("cy-status");
    try {
      const res = await fetch("/api/carry/reset", { method: "POST", cache: "no-store" });
      if (!res.ok) throw new Error("HTTP " + res.status);
      lastCarryData = await res.json();
      renderCarry(lastCarryData);
      st.textContent = "已重置。";
    } catch (e) {
      st.textContent = "重置失败：" + e;
    }
  });
}

setInterval(refreshCarry, 60000);
