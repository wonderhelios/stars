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
        `已缓存 ${d.cached_coins} 币 · 流动性 ${d.universe_liquid} · ${d.exchange === "txflow" ? "TxFlow 日线就绪" : d.paper?.running ? "纸交易运行中" : "纸交易未启动"}`;
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
    const m = /成交\s+([\d.]+)@([\d.]+)/.exec(r.result || "");
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
  const m = /成交\s+([\d.]+)@([\d.]+)/.exec(t);
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
    return { kind: "filled", px, slip, pnl };
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

// TxFlow 业绩单独、低频拉取（60 秒一次），不放在状态轮询里 ——
// 否则每次轮询都要打两次交易所接口，在限速下会把状态请求拖到 502。
let txPnl = null;
let txPnlAt = 0;
async function fetchTxPnl(account) {
  if (LIVE_EXCHANGE !== "TxFlow") return;
  const key = (account || "").toLowerCase();
  if (txPnl && (txPnl.account || "").toLowerCase() !== key) { txPnl = null; txPnlAt = 0; }
  if (Date.now() - txPnlAt < 60000) return;
  txPnlAt = Date.now();
  try {
    // TxFlow's server rewrites the common /api/ prefix exactly once.
    const r = await fetch("/api/pnl", { cache: "no-store" });
    const j = await r.json();
    txPnl = r.ok && j.ok && (j.account || "").toLowerCase() === key ? j : null;
  } catch (e) { txPnl = null; }
}

function capitalPerformance(d) {
  const baseline = d.capital_baseline;
  const base = baseline && baseline.equity;
  const valid = !d.error && Number.isFinite(d.equity) && d.equity >= 0 &&
    Number.isFinite(base) && base > 0 &&
    (baseline.account || "").toLowerCase() === (d.account || "").toLowerCase();
  return { valid, base, change: valid ? d.equity - base : null,
    percent: valid ? (d.equity / base - 1) * 100 : null };
}

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

function renderMonitor(d) {
  if (!d) return;
  if (txPnl && (txPnl.account || "").toLowerCase() === (d.account || "").toLowerCase()) {
    d.tx_realized = txPnl.realized;
    d.tx_fees = txPnl.fees;
    d.tx_volume = txPnl.volume;
    d.tx_fills = txPnl.fills;
    d.tx_net_deposit = txPnl.net_deposit;
  }
  lastLiveData = d;
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
  const moneySigned = v => (v >= 0 ? "+" : "−") + "$" + fmt(Math.abs(v), 2);

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
      metric("记录起点资金", capital.base > 0 ? "$" + fmt(capital.base, 2) : "—") +
        metric("起点以来资金增减", valid ? (pnl >= 0 ? "+" : "") + "$" + fmt(pnl, 2) : "—", valid && pnl >= 0 ? "pos" : "neg") +
        metric("资金变化率", valid ? pct(pnlPct) : "—", valid ? (pnlPct >= 0 ? "pos" : "neg") : "") +
        metric("记录起点 · UTC", d.capital_baseline?.ts ? `<span class="sm">${ts2m(d.capital_baseline.ts)}</span>` : "时间未知"),
      "capital-account",
      '<details class="metric-definition"><summary>资金口径说明 · 包含出入金</summary><p>资金增减 = 当前净值 − 记录起点资金，已体现手续费与资金费，也包含出入金。无后续出入金时才等于净收益。旧账户的最早留存记录可能晚于系统首次启动。</p></details>'
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
    ) +
    // Returned exchange records may be truncated; never infer total account profit from them.
    (d.tx_fills
      ? metricGroup(
          "交易所返回记录（可能截断，非完整净收益）",
            metric(
              "已实现（未扣费）",
              (d.tx_realized >= 0 ? "+" : "") + "$" + fmt(d.tx_realized, 2),
              d.tx_realized >= 0 ? "pos" : "neg"
            ) +
            metric("手续费", "$" + fmt(d.tx_fees, 2), "neg") +
            metric("累计成交量", "$" + fmt(d.tx_volume, 0)) +
            metric("返回流水净入金", "$" + fmt(d.tx_net_deposit, 2)) +
            metric("成交笔数", String(d.tx_fills)),
          "tx-perf"
        )
      : "");

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
            status = '<span class="badge-mini ok">成交</span>';
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

// Window selection and time aggregation are independent of the drawing code.
function equityWindow(history, range, now) {
  const hours = { day: 24, week: 168, month: 720 };
  const cutoff = hours[range] ? now - hours[range] * 3600000 : -Infinity;
  const unique = new Map();
  for (const p of history || []) {
    if (Number.isFinite(p.ts) && Number.isFinite(p.equity) && p.equity >= 0 && p.ts <= now && p.ts >= cutoff) unique.set(p.ts, p);
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
  const { raw, points } = equityWindow(observations, chartRange, now);
  const names = { day: "最近 24 小时", week: "最近 7 天", month: "最近 30 天", all: "全部留存记录" };
  if (!raw.length) {
    el.innerHTML = `<div class="empty">${names[chartRange]}暂无净值记录。可以切换更长区间查看。</div>`;
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
  const money = v => "$" + v.toFixed(2);
  const grid = ax.lines.map(v => `<line x1="${padL}" y1="${Y(v)}" x2="${W-padR}" y2="${Y(v)}" stroke="#eaf0f0"/><text x="${padL-10}" y="${Y(v)+4}" text-anchor="end" font-size="11" fill="#86949d">$${v.toFixed(span < 10 ? 2 : 0)}</text>`).join("");
  const label = ts => chartRange === "day" ? new Date(ts).toISOString().slice(11,16) : ts2m(ts).slice(0,5);
  const ticks = last.ts === first.ts ? [first.ts] : [first.ts, first.ts + (last.ts-first.ts)/2, last.ts];
  const xlabels = ticks.map((ts,i) => `<text x="${X(ts)}" y="${H-10}" text-anchor="${i === 0 ? "start" : i === ticks.length-1 ? "end" : "middle"}" font-size="11" fill="#86949d">${label(ts)}</text>`).join("");
  const line = points.map(p => `${X(p.ts)},${Y(p.equity)}`).join(" ");
  const circles = points.map(p => `<circle cx="${X(p.ts)}" cy="${Y(p.equity)}" r="6" fill="transparent" class="equity-point"><title>${ts2m(p.ts)} UTC · ${money(p.equity)}</title></circle>`).join("");
  el.innerHTML = `<div class="chart-summary"><div><span class="chart-eyebrow">${names[chartRange]} · 末值</span><strong>${money(last.equity)}</strong></div><div><span class="chart-eyebrow">区间记录变化</span><strong class="${change >= 0 ? "pos" : "neg"}">${change >= 0 ? "+" : "−"}$${Math.abs(change).toFixed(2)}</strong></div><div class="chart-coverage"><span class="chart-eyebrow">实际记录区间 · UTC</span>${ts2m(first.ts)} — ${ts2m(last.ts)}</div></div>
    <svg viewBox="0 0 ${W} ${H}" role="img" aria-label="${names[chartRange]}账户净值曲线">
      <defs><linearGradient id="equity-area" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="#168473" stop-opacity="0.14"/><stop offset="100%" stop-color="#168473" stop-opacity="0"/></linearGradient></defs>
      ${grid}
      ${points.length > 1 ? `<polygon points="${X(first.ts)},${H-padB} ${line} ${X(last.ts)},${H-padB}" fill="url(#equity-area)"/>` : ""}
      <line x1="${padL}" y1="${Y(first.equity)}" x2="${W-padR}" y2="${Y(first.equity)}" stroke="#bacac8" stroke-dasharray="4 5"/>
      <polyline points="${line}" fill="none" stroke="#168473" stroke-width="2.5" stroke-linejoin="round"/>
      <circle cx="${X(last.ts)}" cy="${Y(last.equity)}" r="4" fill="#168473" stroke="white" stroke-width="2"/>
      ${circles}${xlabels}
    </svg><div class="chart-caption"><span><i></i>账户净值 <span class="muted">· 虚线为区间记录起点</span></span><span>${raw.length === 1 ? "仅一条记录，等待下一次采样" : "悬停查看采样值"} · 含出入金，非纯策略收益</span></div>`;
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
    // 先拿业绩（有 60 秒缓存，不会每次都请求），再渲染
    await fetchTxPnl(d.account);
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
