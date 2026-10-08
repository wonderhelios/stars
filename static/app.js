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

function metricGroup(title, html, cls) {
  return `<div class="metric-group${cls ? " " + cls : ""}"><div class="g-title">${title}</div><div class="metrics">${html}</div></div>`;
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

// 从下单记录里还原真实成交滑点（正 = 成本增加）
// 汇总下单记录：已实现盈亏 + 实测滑点（都用同一套解析，避免口径不一致）
function chronRecords(d) {
  const recs = d.records || [];
  const hints = entryHints(recs);
  let realized = 0;
  let n = 0;
  const slips = [];
  recs.forEach((r, i) => {
    // 后端对账补入的被动成交（止盈单）直接带 pnl，用它更准；
    // 我们自己发的调仓单则要从成交回执里解析。
    if (r.pnl != null) {
      realized += r.pnl;
      n++;
      return;
    }
    const f = parseFill(r, hints[i]);
    if (f.kind === "filled") {
      if (f.pnl != null) {
        realized += f.pnl;
        n++;
      }
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
    if (entry > 0) {
      // 卖 = 平多；买 = 平空
      pnl = r.side === "卖" ? (px - entry) * r.size : (entry - px) * r.size;
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

function renderMonitor(d) {
  lastLiveData = d;
  if (!d) return;
  lastLiveData = d;
  const c = d.config || {};
  const pos = d.positions || [];
  const hist = d.history || [];
  const eq = d.equity || 0;
  // 盈亏必须用「成交已实现 + 当前未实现」算，不能用「净值 − 历史首点」：
  // 后者会把入金/出金算成盈利。之前就是这样显示成 +15.52% 的，其实账户只是
  // 从别的银行转进来了钱。
  const parsedAcct = chronRecords(d);
  const unrealized = pos.reduce((a, p) => a + (p.unrealized || 0), 0);
  const pnl = parsedAcct.realized + unrealized;
  // 收益率的基数 = 当前净值 − 累计盈亏（即策略开始时的本金）
  const base = eq - pnl;
  const valid = eq > 0;
  const pnlPct = valid && base > 0 ? (pnl / base) * 100 : 0;
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
  if (c.armed && keyed && holding) {
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
      "账户",
      metric("净值", "$" + fmt(eq, 2)) +
        metric("累计盈亏", valid ? (pnl >= 0 ? "+" : "") + "$" + fmt(pnl, 2) : "—", valid && pnl >= 0 ? "pos" : "neg") +
        metric("收益率", valid ? pct(pnlPct) : "—", valid && pnlPct >= 0 ? "pos" : "neg") +
        metric("持仓数", nPos)
    ) +
    metricGroup(
      "风险",
      metric("账户强平缓冲", fmt(buffer, 1) + "%", buffer < 40 ? "neg" : "pos") +
        metric("净敞口", "$" + fmt(d.net_notional || 0, 2), Math.abs(d.net_notional || 0) < 5 ? "pos" : "neg") +
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
      "执行",
      metric("实测平均滑点", slip ? slip.avg.toFixed(3) + "%" : "—", slip && slip.avg > 0.15 ? "neg" : "pos") +
        metric("已实现盈亏", parsed.n ? (realized >= 0 ? "+" : "") + "$" + fmt(realized, 2) : "—",
          realized >= 0 ? "pos" : "neg") +
        metric("总名义敞口", "$" + fmt(d.gross_notional || 0, 0)) +
        metric("止盈挂单", (d.tp_orders || []).length + " / " + nPos, (d.tp_orders || []).length >= nPos ? "pos" : "") +
        metric("最后调仓", d.last_run_at ? `<span class="sm">${ts2m(d.last_run_at)}</span>` : "—")
    ) +
    // TxFlow 的业绩快照。以前页面只显示未实现盈亏，一个赚了钱的账户看起来像在亏，
    // 因为已实现和手续费根本没进页面。这几个数全部来自交易所流水。
    (d.tx_fills
      ? metricGroup(
          "业绩（交易所口径）",
          metric(
            "真实总盈亏",
            (d.tx_total_pnl >= 0 ? "+" : "") + "$" + fmt(d.tx_total_pnl, 2),
            d.tx_total_pnl >= 0 ? "pos" : "neg"
          ) +
            metric(
              "已实现盈亏",
              (d.tx_realized >= 0 ? "+" : "") + "$" + fmt(d.tx_realized, 2),
              d.tx_realized >= 0 ? "pos" : "neg"
            ) +
            metric("手续费", "$" + fmt(d.tx_fees, 2), "neg") +
            metric("累计成交量", "$" + fmt(d.tx_volume, 0)) +
            metric("净入金", "$" + fmt(d.tx_net_deposit, 2)) +
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

  const err = d.error ? `<div class="note neg">${d.error}</div>` : "";
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
          <td class="${cls}">${p.unrealized >= 0 ? "+" : ""}$${fmt(p.unrealized, 2)}</td>
          <td>${p.liq_px == null ? "—" : fmtCompact(p.liq_px)}</td>
          <td class="${p.dist_pct != null && p.dist_pct < 20 ? "neg" : "muted"}">${dist}</td>
          <td>${p.is_cross ? '<span class="badge-mini ok">全仓</span>' : '<span class="badge-mini err">逐仓</span>'}</td>
        </tr>`;
      })
      .join("");
    $("mo-positions").innerHTML = err + `<div class="table-scroll"><table><thead><tr>
      <th>币</th><th>方向</th><th>数量</th><th>入场价</th><th>当前价</th>
      <th>名义</th><th>未实现盈亏</th><th>爆仓价</th><th>距爆仓</th>
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
    : '<div class="empty">当前没有挂单（价格碰到止盈价会自动成交）</div>';

  const recs = chron.map((r, i) => ({ r, hint: hints[i] })).reverse();
  const pages = Math.max(1, Math.ceil(recs.length / LIVE_PAGE_SIZE));
  if (livePage > pages - 1) livePage = 0;
  const pageItems = recs.slice(livePage * LIVE_PAGE_SIZE, (livePage + 1) * LIVE_PAGE_SIZE);
  $("mo-records").innerHTML = recs.length
    ? `<div class="table-scroll"><table><thead><tr>
        <th>时间</th><th>币</th><th>方向</th><th>动作</th><th>数量</th>
        <th>计划价</th><th>成交价</th><th>滑点</th><th>盈亏</th><th>状态</th>
      </tr></thead><tbody>${pageItems
        .map(({ r, hint }) => {
          const f = parseFill(r, hint);
          let pxCell = '<span class="muted">—</span>';
          let slipCell = '<span class="muted">—</span>';
          let pnlCell = '<span class="muted">—</span>';
          let status = '<span class="badge-mini mute">—</span>';
          // 对账补入的被动成交（止盈单）：交易所回执里直接带成交价和盈亏，
          // 但没有我们自己下单时的「计划价 vs 成交价」结构，所以 parseFill
          // 认不出来 —— 必须在解析之前单独处理，否则这几列全是空的。
          if (r.pnl != null) {
            status = '<span class="badge-mini ok">成交</span>';
            pxCell = r.price ? fmtCompact(r.price) : '<span class="muted">—</span>';
            const cls = r.pnl >= 0 ? "pos" : "neg";
            pnlCell = `<span class="${cls}">${r.pnl >= 0 ? "+" : ""}$${fmt(r.pnl, 2)}</span>`;
            return `<tr>
              <td class="muted">${ts2m(r.ts)}</td>
              <td>${r.coin}</td>
              <td>${sideLabel(r.side === "买" ? "long" : "short", true)}</td>
              <td class="muted">${r.action}</td>
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
              pnlCell = `<b class="${f.pnl >= 0 ? "pos" : "neg"}">${f.pnl >= 0 ? "+" : ""}$${fmt(f.pnl, 2)}</b>`;
            }
          } else if (f.kind === "unfilled") {
            status = '<span class="badge-mini warn">未成交</span>';
          } else if (f.kind === "failed") {
            status = `<span class="badge-mini err" title="${f.msg}">失败</span>`;
          } else if (f.kind === "plan") {
            status = '<span class="badge-mini mute">计划</span>';
          }
          // 方向要显示「持仓方向」：平仓时下单方向与持仓方向相反
          const closing = r.reduce_only || /平仓|减仓/.test(r.action || "");
          const posLong = r.side === "买" ? !closing : closing;
          const note =
            f.kind === "failed" && f.msg
              ? `<div class="muted" style="font-size:11px;max-width:220px;white-space:normal">${f.msg}</div>`
              : "";
          return `<tr>
            <td class="muted">${ts2m(r.ts)}</td>
            <td>${r.coin}</td>
            <td>${sideLabel(posLong ? "long" : "short", closing)}</td>
            <td class="muted">${r.action}</td>
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

function renderLiveChart(history, shadow) {
  const el = $("mo-chart");
  if (!history || history.length < 2) {
    el.innerHTML = '<div class="empty">还没有足够的数据点（每天自动记录一次净值）</div>';
    return;
  }
  const W = 1080, H = 300;
  const padL = 78, padR = 84, padT = 16, padB = 34;
  const vals = history.map((p) => p.equity);
  const base = vals[0];
  const ax = niceAxis(Math.min(...vals, base), Math.max(...vals, base), 5);
  const span = ax.hi - ax.lo || 1;
  const n = history.length;
  const X = (i) => padL + (i / (n - 1)) * (W - padL - padR);
  const Y = (v) => padT + (1 - (v - ax.lo) / span) * (H - padT - padB);
  const money = (v) => "$" + Math.round(v).toLocaleString();

  const grid = ax.lines
    .map((v) => `<line x1="${padL}" y1="${Y(v)}" x2="${W - padR}" y2="${Y(v)}" stroke="#eef1f6"/>
      <text x="${padL - 8}" y="${Y(v) + 4}" text-anchor="end" font-size="11" fill="#768297">${money(v)}</text>`)
    .join("");
  // 时间跨度不足两天时只显示时分，否则三个标签会全是同一天
  const spanMs = history[n - 1].ts - history[0].ts;
  const xfmt = spanMs < 2 * 86_400_000 ? (ms) => new Date(ms).toISOString().slice(11, 16) : ts2d;
  const xlabels = [0, Math.floor((n - 1) / 2), n - 1]
    .map((i) => {
      const anchor = i === 0 ? "start" : i === n - 1 ? "end" : "middle";
      return `<text x="${X(i)}" y="${H - 10}" text-anchor="${anchor}" font-size="11" fill="#768297">${xfmt(history[i].ts)}</text>`;
    })
    .join("");
  const baseY = Y(base);
  const endY = Y(vals[n - 1]);
  const line = `<polyline points="${vals.map((v, i) => `${X(i)},${Y(v)}`).join(" ")}" fill="none" stroke="#2d6df6" stroke-width="2.2"/>`;
  // 影子回测：把它的净值和实盘对齐到同一个起点，画成虚线对照。
  // 实盘明显低于虚线，说明差额来自执行而不是信号。
  let shadowLine = "";
  if (shadow && shadow.length > 1 && base > 0) {
    const byDay = new Map();
    for (const p of shadow) byDay.set(Math.floor(p.ts / 86400000), p.equity);
    const pts = [];
    let scale = null;
    for (let i = 0; i < n; i++) {
      const key = Math.floor(history[i].ts / 86400000);
      const v = byDay.get(key);
      if (v == null) continue;
      if (scale == null) scale = history[i].equity / v; // 对齐起点
      pts.push(`${X(i)},${Y(v * scale)}`);
    }
    if (pts.length > 1) {
      shadowLine = `<polyline points="${pts.join(" ")}" fill="none" stroke="#9aa6b8" stroke-width="1.8" stroke-dasharray="6 4"/>`;
    }
  }
  // 曲线平直时起点与终点重合，两个标签叠一起会糊 —— 只在分得开时才画起点
  const baseLabel =
    Math.abs(endY - baseY) >= 15
      ? `<text x="${W - padR + 6}" y="${baseY + 4}" font-size="11" fill="#9aa6b8" paint-order="stroke" stroke="#fff" stroke-width="3">起点</text>`
      : "";

  el.innerHTML = `
    <svg viewBox="0 0 ${W} ${H}" role="img" aria-label="实盘净值曲线">
      ${grid}
      <line x1="${padL}" y1="${baseY}" x2="${W - padR}" y2="${baseY}" stroke="#c9d3e3" stroke-width="1" stroke-dasharray="4 4"/>
      ${baseLabel}
      ${line}${shadowLine}
      <text x="${W - padR + 6}" y="${endY + 4}" font-size="12" font-weight="600" fill="#2d6df6" paint-order="stroke" stroke="#fff" stroke-width="3">${money(vals[n - 1])}</text>
      ${xlabels}
    </svg>
    <div class="legend">
      <span><span style="color:#2d6df6">━</span> 实盘账户净值</span>
      <span>起点 ${money(base)} · ${ts2d(history[0].ts)} → ${ts2d(history[n - 1].ts)}</span>
    </div>`;
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
  if (!confirm("清空所有下单记录，并把净值曲线从现在重新开始？配置会保留。")) return;
  try {
    const res = await fetch("/api/live/records/clear", { method: "POST" });
    const d = await res.json();
    if (d.ok) refreshMonitor(true);
  } catch (e) {
    alert("清空失败：" + e);
  }
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
