// stars · 动量 alpha 研究台 — 前端逻辑
const $ = (id) => document.getElementById(id);

// ---------- 工具 ----------
const pct = (v, d = 2) => `${(v >= 0 ? "+" : "")}${v.toFixed(d)}%`;
const signed = (v, d = 2) => `${(v >= 0 ? "+" : "")}${v.toFixed(d)}`;
const fmt = (v, d = 2) => v.toFixed(d);
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
let feePrefilled = false;
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
    } else if (d.cached_coins > 0) {
      led.className = "led";
      $("statusText").textContent =
        `已缓存 ${d.cached_coins} 币 · 流动性 ${d.universe_liquid} · ${d.paper?.running ? "纸交易运行中" : "纸交易未启动"}`;
    } else {
      led.className = "led off";
      $("statusText").textContent = "等待数据";
    }
    // Prefill the real Hyperliquid taker fee once.
    if (!feePrefilled && d.fee_taker > 0) {
      $("pp-fee").value = (d.fee_taker * 100).toFixed(4);
      feePrefilled = true;
    }
    if (d.fee_taker > 0) {
      $("pp-fee").title = `Hyperliquid 实时费率：taker ${(d.fee_taker * 100).toFixed(4)}% / maker ${(d.fee_maker * 100).toFixed(4)}%`;
    }
  } catch (e) {
    $("led").className = "led off";
    $("statusText").textContent = "连接中断";
  }
}

// ---------- 回测 ----------
$("bt-run").addEventListener("click", async () => {
  $("bt-error").innerHTML = "";
  $("bt-run").disabled = true;
  $("bt-run").textContent = "回测中…";
  try {
    const body = {
      lookback: parseInt($("bt-lookback").value) || 14,
      top_frac: parseFloat($("bt-top").value) || 0.2,
      min_vol_usd: parseFloat($("bt-minvol").value),
      hedge: $("bt-hedge").value,
    };
    const res = await fetch("/api/backtest", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    const d = await res.json();
    if (!res.ok) throw new Error(d.error || "HTTP " + res.status);
    renderBacktest(d);
  } catch (e) {
    $("bt-error").innerHTML = `<div class="error">${e.message}</div>`;
  } finally {
    $("bt-run").disabled = false;
    $("bt-run").textContent = "运行回测";
  }
});

function metric(k, v, cls) {
  return `<div class="metric"><div class="k">${k}</div><div class="v ${cls || ""}">${v}</div></div>`;
}

function renderBacktest(d) {
  $("bt-result").classList.remove("hidden");
  const cls = d.alpha_annual >= 0 ? "pos" : "neg";
  $("bt-metrics").innerHTML =
    metric("纯 alpha 年化", pct(d.alpha_annual * 100), cls) +
    metric("Sharpe", fmt(d.sharpe, 2), cls) +
    metric("t 统计", fmt(d.t_stat, 2), cls) +
    metric("胜率", fmt(d.win_rate * 100, 1) + "%") +
    metric("beta", fmt(d.beta, 2)) +
    metric("与 BTC 相关性", fmt(d.corr_btc, 2)) +
    metric("多头腿年化", pct(d.long_leg_daily * 365 * 100)) +
    metric("换手/天", fmt(d.turnover_daily * 100, 0) + "%") +
    metric("样本天数", d.days) +
    metric("参与币数", d.coins_traded);

  $("bt-years").innerHTML = `<div class="table-scroll"><table><thead><tr><th>年份</th><th>天数</th><th>alpha 年化</th><th>t 统计</th></tr></thead>
    <tbody>${d.per_year.map((y) => `<tr><td>${y.year}</td><td>${y.days}</td>
      <td class="${y.alpha_annual >= 0 ? "pos" : "neg"}">${pct(y.alpha_annual * 100)}</td>
      <td>${fmt(y.t_stat, 2)}</td></tr>`).join("")}</tbody></table></div>`;

  $("bt-cost").innerHTML = `<div class="table-scroll"><table><thead><tr><th>单边费率</th><th>净年化 alpha</th></tr></thead>
    <tbody>${d.cost_sensitivity.map((c) => `<tr><td>${(c.fee * 100).toFixed(3)}%</td>
      <td class="${c.net_annual >= 0 ? "pos" : "neg"}">${pct(c.net_annual * 100)}</td></tr>`).join("")}</tbody></table></div>`;

  $("bt-coins").innerHTML = `<div class="table-scroll"><table><thead><tr><th>币</th><th>入选次数</th></tr></thead>
    <tbody>${d.top_coins.map(([c, n]) => `<tr><td>${c}</td><td>${n}</td></tr>`).join("")}</tbody></table></div>`;
}

// ---------- 纸交易 ----------
let paperRunning = false;

$("pp-toggle").addEventListener("click", async () => {
  $("pp-error").innerHTML = "";
  if (paperRunning) {
    await fetch("/api/paper/stop", { method: "POST" });
  } else {
    const body = {
      lookback: parseInt($("pp-lookback").value) || 14,
      top_frac: parseFloat($("pp-top").value) || 0.2,
      min_vol_usd: parseFloat($("pp-minvol").value),
      fee: parseFloat($("pp-fee").value) / 100 || 0.00045,
      capital: parseFloat($("pp-capital").value) || 2000,
      leverage: parseFloat($("pp-leverage").value) || 3,
      target_positions: parseInt($("pp-target").value) || 8,
      replay_days: 90,
    };
    const res = await fetch("/api/paper/start", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    const d = await res.json();
    if (!res.ok) {
      $("pp-error").innerHTML = `<div class="error">${d.error || "启动失败"}</div>`;
      return;
    }
  }
  await refreshPaper();
// 实盘接口按次计费（Hyperliquid 有限流），所以低频轮询。
refreshLiveConfig();
refreshMonitor();
setInterval(refreshMonitor, 60000);
});

$("pp-step").addEventListener("click", async () => {
  const res = await fetch("/api/paper/step", { method: "POST" });
  const d = await res.json();
  if (!res.ok) $("pp-error").innerHTML = `<div class="error">${d.error || "步进失败"}</div>`;
  await refreshPaper();
// 实盘接口按次计费（Hyperliquid 有限流），所以低频轮询。
refreshLiveConfig();
refreshMonitor();
setInterval(refreshMonitor, 60000);
});

$("pp-reset").addEventListener("click", async () => {
  await fetch("/api/paper/reset", { method: "POST" });
  await refreshPaper();
// 实盘接口按次计费（Hyperliquid 有限流），所以低频轮询。
refreshLiveConfig();
refreshMonitor();
setInterval(refreshMonitor, 60000);
});

async function refreshPaper() {
  try {
    const res = await fetch("/api/paper", { cache: "no-store" });
    const d = await res.json();
    paperRunning = !!d.running;
    $("pp-toggle").textContent = d.running ? "停止纸交易" : "启动纸交易";
    $("pp-toggle").classList.toggle("danger", d.running);

    if (!d.config) {
      $("pp-metrics").innerHTML = '<div class="empty">尚未启动纸交易。设置参数后点「启动纸交易」。</div>';
      $("pp-chart").innerHTML = "";
      $("pp-positions").innerHTML = "";
      $("pp-trades").innerHTML = "";
      return;
    }

    const pnlCls = d.pnl >= 0 ? "pos" : "neg";
    const liq = d.nearest_liq_pct == null ? "—" : fmt(d.nearest_liq_pct, 1) + "%";
    $("pp-metrics").innerHTML =
      metric("账户净值", "$" + fmt(d.equity, 2)) +
      metric("累计盈亏", `${d.pnl >= 0 ? "+" : ""}$${fmt(d.pnl, 2)}`, pnlCls) +
      metric("收益率", pct(d.pnl_pct), pnlCls) +
      metric("已实现盈亏", `${d.realized_pnl >= 0 ? "+" : ""}$${fmt(d.realized_pnl, 2)}`, d.realized_pnl >= 0 ? "pos" : "neg") +
      metric("未实现盈亏", `${d.unrealized_pnl >= 0 ? "+" : ""}$${fmt(d.unrealized_pnl, 2)}`, d.unrealized_pnl >= 0 ? "pos" : "neg") +
      metric("平仓胜率", fmt(d.win_rate, 1) + "%") +
      metric("爆仓次数", d.liquidations, d.liquidations > 0 ? "neg" : "pos") +
      metric("杠杆", fmt(d.leverage, 0) + "×") +
      metric("总名义敞口", "$" + fmt(d.gross_notional, 0)) +
      metric("净敞口（市场中性）", "$" + fmt(d.net_notional, 0), Math.abs(d.net_notional) < 1 ? "pos" : "") +
      metric("账户强平缓冲", fmt(d.liq_buffer_pct, 1) + "%", d.liq_buffer_pct < 40 ? "neg" : "pos") +
      metric("强平净值线", "$" + fmt(d.liq_equity, 0)) +
      metric("维持保证金", "$" + fmt(d.maintenance_margin, 0)) +
      metric("初始保证金占用", "$" + fmt(d.margin_used, 0) + ` (${fmt(d.margin_usage_pct, 0)}%)`) +
      metric("逐仓距爆仓(最小)", liq, d.nearest_liq_pct != null && d.nearest_liq_pct < 20 ? "neg" : "") +
      metric("同期等权市场（参考）", pct(d.market_pct), d.market_pct >= 0 ? "pos" : "neg") +
      metric("已运行天数", d.days_elapsed) +
      metric("累计成本", "$" + fmt(d.total_cost, 2));

    renderChart(d.history);
    renderDailyPnl(d.history, d.config ? d.config.capital : 1);
    renderPositions(d.positions);
    renderTrades(d.trades, d.config);
    renderHolding(d.trades);
  } catch (e) {
    $("pp-error").innerHTML = `<div class="error">${e.message}</div>`;
  }
}

function sideLabel(s) {
  return s === "long"
    ? '<span class="badge ok">多</span>'
    : '<span class="badge neg-badge">空</span>';
}

function renderPositions(positions) {
  if (!positions || !positions.length) {
    $("pp-positions").innerHTML = '<div class="empty">暂无持仓</div>';
    return;
  }
  const rows = positions
    .slice()
    .sort((a, b) => (a.side === b.side ? b.notional - a.notional : a.side === "long" ? -1 : 1))
    .map((p) => {
      const cls = p.unrealized_pnl >= 0 ? "pos" : "neg";
      const dist =
        p.side === "long"
          ? ((p.mark_price - p.liq_price) / p.mark_price) * 100
          : ((p.liq_price - p.mark_price) / p.mark_price) * 100;
      return `<tr>
        <td>${p.coin}</td>
        <td>${sideLabel(p.side)}</td>
        <td>$${fmt(p.notional, 0)}</td>
        <td>${fmt(p.basis_price || p.entry_price, 6)}</td>
        <td>${fmt(p.mark_price, 6)}</td>
        <td class="${cls}">${p.unrealized_pnl >= 0 ? "+" : ""}$${fmt(p.unrealized_pnl, 2)}</td>
        <td>${fmt(p.liq_price, 6)}</td>
        <td class="${dist < 20 ? "neg" : "muted"}">${fmt(dist, 1)}%</td>
      </tr>`;
    })
    .join("");
  $("pp-positions").innerHTML = `<div class="table-scroll"><table><thead><tr>
    <th>币</th><th>方向</th><th>名义</th><th>均价</th><th>当前价</th>
    <th>未实现盈亏</th><th>爆仓价</th><th>距爆仓</th>
  </tr></thead><tbody>${rows}</tbody></table></div>`;
}

function renderTrades(trades, config) {
  lastTrades = trades || [];
  lastConfig = config;
  if (!trades || !trades.length) {
    $("pp-trades").innerHTML =
      '<div class="empty">还没有平仓记录（首次换仓后开始出现）</div>';
    return;
  }
  const list = trades.slice().reverse();
  const realized = list.reduce((s, t) => s + t.pnl_usd, 0);
  const wins = list.filter((t) => t.pnl_usd > 0).length;
  const pages = Math.max(1, Math.ceil(list.length / PAGE_SIZE));
  if (tradesPage > pages - 1) tradesPage = 0;
  const pageItems = list.slice(tradesPage * PAGE_SIZE, (tradesPage + 1) * PAGE_SIZE);
  const rows = pageItems
    .map((t) => {
      const cls = t.pnl_usd >= 0 ? "pos" : "neg";
      const d1 = ts2d(t.entry_ts);
      const d2 = ts2d(t.exit_ts);
      return `<tr>
        <td>${t.coin}</td>
        <td>${sideLabel(t.side)}</td>
        <td>${fmt(t.avg_entry || t.entry_price, 6)}</td>
        <td>${fmt(t.exit_price, 6)}</td>
        <td class="${cls}">${t.pnl_usd >= 0 ? "+" : ""}$${fmt(t.pnl_usd, 2)}</td>
        <td class="${cls}">${pct(t.pnl_pct)}</td>
        <td class="muted">${d1} → ${d2}</td>
        <td class="muted">${t.reason === "liquidated" ? "爆仓强平" : "换仓"}</td>
      </tr>`;
    })
    .join("");
  $("pp-trades").innerHTML = `<div class="note">
      共 ${list.length} 笔平仓 · 已实现盈亏 <b class="${realized >= 0 ? "pos" : "neg"}">${realized >= 0 ? "+" : ""}$${fmt(realized, 2)}</b>
      · 胜率 ${fmt((wins / list.length) * 100, 1)}%
    </div>
    <div class="table-scroll"><table><thead><tr>
      <th>币</th><th>方向</th><th>均价</th><th>出场价</th><th>盈亏</th><th>盈亏%</th><th>持有</th><th>原因</th>
    </tr></thead><tbody>${rows}</tbody></table></div>
    ${pager("trades", list.length, tradesPage)}`;
}



// ---------- 分页 ----------
const PAGE_SIZE = 10;
const LIVE_PAGE_SIZE = 5;
let dailyPage = 0;
let tradesPage = 0;
let lastDaily = [];
let lastTrades = [];
let lastConfig = null;
let lastCapital = 1;

// 通用分页条；点击由全局委托处理
function pager(key, total, page, size) {
  const per = size || PAGE_SIZE;
  const pages = Math.max(1, Math.ceil(total / per));
  const p = Math.min(Math.max(0, page), pages - 1);
  return `<div class="pager">
    <button class="btn ghost" data-pager="${key}" data-to="${p - 1}" ${p <= 0 ? "disabled" : ""}>← 上一页</button>
    <span class="info">第 ${p + 1} / ${pages} 页 · 共 ${total} 条</span>
    <button class="btn ghost" data-pager="${key}" data-to="${p + 1}" ${p >= pages - 1 ? "disabled" : ""}>下一页 →</button>
  </div>`;
}

document.addEventListener("click", (e) => {
  const btn = e.target.closest("[data-pager]");
  if (!btn) return;
  const key = btn.dataset.pager;
  const to = Number(btn.dataset.to);
  if (key === "daily") {
    dailyPage = to;
    renderDailyPnl(lastDaily, lastCapital);
  } else if (key === "trades") {
    tradesPage = to;
    renderTrades(lastTrades, lastConfig);
  } else if (key === "live") {
    livePage = to;
    renderMonitor(lastLiveData);
  }
});

// 每日盈亏：从净值曲线上取相邻两点的差
function renderDailyPnl(history, capital) {
  lastDaily = history || [];
  lastCapital = capital;
  if (!history || history.length < 2) {
    $("pp-daily").innerHTML = '<div class="empty">还没有完整交易日（需要至少 2 天）</div>';
    return;
  }
  const rows = [];
  for (let i = 1; i < history.length; i++) {
    const prev = history[i - 1], cur = history[i];
    const pnl = cur.equity - prev.equity;
    const pct = prev.equity > 0 ? (pnl / prev.equity) * 100 : 0;
    const mkt = prev.market > 0 ? ((cur.market / prev.market) - 1) * 100 : 0;
    rows.push({ ts: cur.ts, equity: cur.equity, pnl, pct, mkt });
  }
  const list = rows.slice().reverse();
  const pages = Math.max(1, Math.ceil(list.length / PAGE_SIZE));
  if (dailyPage > pages - 1) dailyPage = 0;
  const show = list.slice(dailyPage * PAGE_SIZE, (dailyPage + 1) * PAGE_SIZE);
  const wins = rows.filter((r) => r.pnl > 0).length;
  const total = rows.reduce((s, r) => s + r.pnl, 0);
  const best = rows.reduce((a, b) => (b.pnl > a.pnl ? b : a), rows[0]);
  const worst = rows.reduce((a, b) => (b.pnl < a.pnl ? b : a), rows[0]);
  const body = show
    .map((r) => {
      const cls = r.pnl >= 0 ? "pos" : "neg";
      return `<tr>
        <td>${ts2d(r.ts)}</td>
        <td>$${fmt(r.equity, 2)}</td>
        <td class="${cls}">${r.pnl >= 0 ? "+" : ""}$${fmt(r.pnl, 2)}</td>
        <td class="${cls}">${pct(r.pct)}</td>
        <td class="muted">${pct(r.mkt)}</td>
      </tr>`;
    })
    .join("");
  $("pp-daily").innerHTML = `<div class="note">
      共 ${rows.length} 个交易日 · 上涨 ${wins} 天 / 下跌 ${rows.length - wins} 天（${fmt((wins / rows.length) * 100, 1)}% 胜率）
      · 累计 ${total >= 0 ? "+" : ""}$${fmt(total, 2)}
      · 最好 ${ts2d(best.ts)} +$${fmt(best.pnl, 2)}
      · 最差 ${ts2d(worst.ts)} $${fmt(worst.pnl, 2)}
    </div>
    <div class="table-scroll"><table><thead><tr>
      <th>日期(UTC)</th><th>净值</th><th>当日盈亏</th><th>当日%</th><th>同期市场%</th>
    </tr></thead><tbody>${body}</tbody></table></div>
    ${pager("daily", list.length, dailyPage)}`;
}

// 持有期分析：赚的钱来自长仓还是短仓
function renderHolding(trades) {
  if (!trades || !trades.length) {
    $("pp-holding").innerHTML = '<div class="empty">还没有平仓记录</div>';
    return;
  }
  const buckets = [
    ["1 天内", 0, 1],
    ["2-3 天", 1, 3],
    ["4-7 天", 3, 7],
    ["8-14 天", 7, 14],
    ["15 天以上", 14, 1e9],
  ];
  const rows = buckets.map(([label, lo, hi]) => {
    const v = trades.filter(
      (t) => lo < (t.exit_ts - t.entry_ts) / 86400000 && (t.exit_ts - t.entry_ts) / 86400000 <= hi
    );
    if (!v.length) return { label, n: 0, avg: 0, win: 0, sum: 0 };
    const sum = v.reduce((s, t) => s + t.pnl_usd, 0);
    return {
      label,
      n: v.length,
      avg: sum / v.length,
      win: (v.filter((t) => t.pnl_usd > 0).length / v.length) * 100,
      sum,
    };
  });
  const all = trades.map((t) => (t.exit_ts - t.entry_ts) / 86400000);
  const avgHold = all.reduce((a, b) => a + b, 0) / all.length;
  const sorted = all.slice().sort((a, b) => a - b);
  const medHold = sorted[Math.floor(sorted.length / 2)];

  $("pp-holding").innerHTML = `<div class="note">
      平均持有 <b>${fmt(avgHold, 1)} 天</b> · 中位 <b>${fmt(medHold, 0)} 天</b>
      · 规律：<b>持有越久越赚钱</b>，短命仓位是亏损来源（这也是为什么不该加止损）
    </div>
    <div class="table-scroll"><table><thead><tr>
      <th>持有期</th><th>笔数</th><th>占比</th><th>平均盈亏</th><th>合计盈亏</th><th>胜率</th>
    </tr></thead><tbody>${rows
      .map((r) => {
        const cls = r.avg >= 0 ? "pos" : "neg";
        return `<tr>
          <td>${r.label}</td><td>${r.n}</td>
          <td>${fmt((r.n / trades.length) * 100, 1)}%</td>
          <td class="${cls}">${r.avg >= 0 ? "+" : ""}$${fmt(r.avg, 2)}</td>
          <td class="${cls}">${r.sum >= 0 ? "+" : ""}$${fmt(r.sum, 2)}</td>
          <td>${fmt(r.win, 0)}%</td>
        </tr>`;
      })
      .join("")}</tbody></table></div>`;
}

// 选一组好看的坐标轴刻度
function niceAxis(lo, hi, ticks) {
  if (!(hi > lo)) {
    // 完全平直时给一个 ±1% 的窗口，否则整张图只有一根网格线
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

function renderChart(history) {
  if (!history || history.length < 2) {
    $("pp-chart").innerHTML = '<div class="empty">数据不足，曲线需要至少 2 个点</div>';
    return;
  }
  const W = 1080, H = 300;
  const padL = 78, padR = 84, padT = 16, padB = 34;
  const cap = history[0].equity || 1;
  const strat = history.map((p) => p.equity);
  const mkt = history.map((p) => p.market * cap);
  const ax = niceAxis(Math.min(...strat.concat(mkt)), Math.max(...strat.concat(mkt)), 5);
  const span = ax.hi - ax.lo || 1;
  const n = history.length;
  const X = (i) => padL + (i / (n - 1)) * (W - padL - padR);
  const Y = (v) => padT + (1 - (v - ax.lo) / span) * (H - padT - padB);
  const money = (v) => "$" + Math.round(v).toLocaleString();

  const grid = ax.lines
    .map((v) => {
      const y = Y(v);
      return `<line x1="${padL}" y1="${y}" x2="${W - padR}" y2="${y}" stroke="#eef1f6"/>
        <text x="${padL - 8}" y="${y + 4}" text-anchor="end" font-size="11" fill="#768297">${money(v)}</text>`;
    })
    .join("");

  const poly = (arr, color, w) =>
    `<polyline points="${arr.map((v, i) => `${X(i)},${Y(v)}`).join(" ")}" fill="none" stroke="${color}" stroke-width="${w}"/>`;

  const xlabels = [0, Math.floor((n - 1) / 2), n - 1]
    .map((i) => {
      const anchor = i === 0 ? "start" : i === n - 1 ? "end" : "middle";
      return `<text x="${X(i)}" y="${H - 10}" text-anchor="${anchor}" font-size="11" fill="#768297">${ts2d(history[i].ts)}</text>`;
    })
    .join("");

  const baseY = Y(cap);
  const lastS = strat[n - 1], lastM = mkt[n - 1];
  const yS = Y(lastS), yM = Y(lastM);
  // 端点标签互相挨太近会叠成糊的，错开或省略
  const baseVisible =
    cap >= ax.lo && cap <= ax.hi && Math.abs(yS - baseY) >= 15 && Math.abs(yM - baseY) >= 15;
  const baseline =
    cap >= ax.lo && cap <= ax.hi
      ? `<line x1="${padL}" y1="${baseY}" x2="${W - padR}" y2="${baseY}" stroke="#c9d3e3" stroke-width="1" stroke-dasharray="4 4"/>` +
        (baseVisible
          ? `<text x="${W - padR + 6}" y="${baseY + 4}" font-size="11" fill="#9aa6b8" paint-order="stroke" stroke="#fff" stroke-width="3">本金</text>`
          : "")
      : "";
  const marketLabel =
    Math.abs(yS - yM) >= 14
      ? `<text x="${W - padR + 6}" y="${yM + 4}" font-size="12" fill="#768297" paint-order="stroke" stroke="#fff" stroke-width="3">${money(lastM)}</text>`
      : "";
  const endLabels = `
    <text x="${W - padR + 6}" y="${yS + 4}" font-size="12" font-weight="600" fill="#2d6df6" paint-order="stroke" stroke="#fff" stroke-width="3">${money(lastS)}</text>
    ${marketLabel}`;

  $("pp-chart").innerHTML = `
    <svg viewBox="0 0 ${W} ${H}" role="img" aria-label="净值曲线">
      ${grid}
      ${baseline}
      ${poly(mkt, "#768297", 1.6)}
      ${poly(strat, "#2d6df6", 2.2)}
      ${endLabels}
      ${xlabels}
    </svg>
    <div class="legend">
      <span><span style="color:#2d6df6">━</span> 策略净值（多空前 20%，市场中性）</span>
      <span><span style="color:#768297">━</span> 等权市场（若无对冲会拿到的）</span>
      <span>本金 ${money(cap)} · ${ts2d(history[0].ts)} → ${ts2d(history[n - 1].ts)}</span>
    </div>`;
}


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
    lookback: Number($("lv-lookback").value),
    top_frac: Number($("lv-top").value),
    min_vol_usd: Number($("lv-minvol").value),
    armed: $("lv-armed").value === "true",
    auto_run: $("lv-auto").value === "true",
  };
  $("lv-save").textContent = "保存中…";
  try {
    const res = await fetch("/api/live/config", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    const d = await res.json();
    if (!d.ok) throw new Error(d.error || "保存失败");
    $("lv-save").textContent = "已保存 ✓";
  } catch (e) {
    $("lv-save").textContent = "保存失败";
  }
  setTimeout(() => ($("lv-save").textContent = "保存配置"), 1500);
});

async function runLive(live) {
  if (live && $("lv-armed").value !== "true") {
    alert("请先把「启用实盘」设为开启并保存配置，否则不会发送任何订单。");
    return;
  }
  if (live && !confirm("确认执行真实调仓？会对 Hyperliquid 账户发送真实订单。")) return;
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
    const r = d.result;
    const extra = r.executed && r.executed.length ? "\n\n执行结果:\n" + r.executed.join("\n") : "";
    $("lv-plan-out").innerHTML = `<pre class="logbox">${(r.plan_lines || []).join("\n")}${extra}</pre>`;
  } catch (e) {
    $("lv-plan-out").innerHTML = `<pre class="logbox neg">${e}</pre>`;
  }
  btn.textContent = label;
  btn.disabled = false;
  liveBusy = false;
  refreshMonitor();
}

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
let lastLiveData = null;

// 从下单记录里还原真实成交滑点（正 = 成本增加）
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
function parseFill(r) {
  const t = r.result || "";
  const m = /成交\s+([\d.]+)@([\d.]+)/.exec(t);
  if (m) {
    const px = parseFloat(m[2]);
    let slip = r.price > 0 ? ((px - r.price) / r.price) * 100 : 0;
    if (r.side === "卖") slip = -slip; // 卖出成交价低于计划 = 成本增加
    return { kind: "filled", px, slip };
  }
  if (/未成交|挂单/.test(t)) return { kind: "unfilled" };
  if (/失败|错误|invalid|rejected/i.test(t)) {
    return { kind: "failed", msg: t.replace(/^[^:：]*失败[:：]\s*/, "").slice(0, 60) };
  }
  if (t === "计划") return { kind: "plan" };
  return { kind: "other", msg: t };
}

function renderMonitor(d) {
  if (!d) return;
  lastLiveData = d;
  const c = d.config || {};
  const pos = d.positions || [];
  const hist = d.history || [];
  const eq = d.equity || 0;
  const base = hist.length ? hist[0].equity : 0;
  const valid = eq > 0 && base > 0;
  const pnl = valid ? eq - base : 0;
  const pnlPct = valid ? (pnl / base) * 100 : 0;
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
  const autoText = auto
    ? `<span>每日自动调仓：<b>已开启</b>，下次 <b>${nextRun} UTC</b>（北京时间 08:05）</span>`
    : `<span>每日自动调仓：<b>已关闭</b>（需手动点「执行调仓」）</span>`;
  const lastText = d.last_run_at
    ? `<span>上次调仓 <b>${ts2m(d.last_run_at)}</b>${d.last_live ? "（真实下单）" : "（仅生成计划）"}</span>`
    : "<span>还没有调仓记录</span>";
  $("mo-status").innerHTML = `<div class="live-status ${cls}">
    <span class="tag">${tag}</span><span>${text}</span>${autoText}${lastText}
  </div>`;

  $("mo-metrics").innerHTML =
    metric("实盘状态", holding ? "运行中" : c.armed ? "待建仓" : "未启用",
      holding ? "pos" : c.armed ? "" : "neg") +
    metric("账户净值", "$" + fmt(eq, 2)) +
    metric("累计盈亏", valid ? (pnl >= 0 ? "+" : "") + "$" + fmt(pnl, 2) : "—", valid && pnl >= 0 ? "pos" : "neg") +
    metric("收益率", valid ? pct(pnlPct) : "—", valid && pnlPct >= 0 ? "pos" : "neg") +
    metric("持仓数", pos.length) +
    metric("杠杆", fmt(c.leverage || 0, 1) + "×") +
    metric("总名义敞口", "$" + fmt(d.gross_notional || 0, 0)) +
    metric("净敞口（市场中性）", "$" + fmt(d.net_notional || 0, 2), Math.abs(d.net_notional || 0) < 1 ? "pos" : "") +
    metric("账户强平缓冲", fmt(buffer, 1) + "%", buffer < 40 ? "neg" : "pos") +
    metric("强平净值线", "$" + fmt(d.maintenance_margin || 0, 0)) +
    metric("维持保证金", "$" + fmt(d.maintenance_margin || 0, 0)) +
    metric("逐仓距爆仓(最小)", d.nearest_liq_pct == null ? "—" : fmt(d.nearest_liq_pct, 1) + "%",
      d.nearest_liq_pct != null && d.nearest_liq_pct < 20 ? "neg" : "") +
    metric("最后调仓", d.last_run_at ? `<span class="sm">${ts2m(d.last_run_at)}</span>` : "—") +
    (() => {
      const sl = slippageStats(d.records);
      if (!sl) return metric("实测平均滑点", "—");
      const cls = sl.avg > 0.15 ? "neg" : "pos";
      const v = `<span class="sm">${sl.avg >= 0 ? "+" : ""}${sl.avg.toFixed(3)}%</span>`;
      return metric(`实测平均滑点 (${sl.n}笔)`, v, cls);
    })();

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
    const rows = pos
      .map((p) => {
        const cls = p.unrealized >= 0 ? "pos" : "neg";
        const dist = p.dist_pct == null ? "—" : fmt(p.dist_pct, 1) + "%";
        return `<tr>
          <td>${p.coin}</td>
          <td>${sideLabel(p.side)}</td>
          <td>${fmt(p.size, 6)}</td>
          <td>${fmt(p.entry_px, 6)}</td>
          <td>${fmt(p.mark_px, 6)}</td>
          <td>$${fmt(p.notional, 0)}</td>
          <td class="${cls}">${p.unrealized >= 0 ? "+" : ""}$${fmt(p.unrealized, 2)}</td>
          <td>${p.liq_px == null ? "—" : fmt(p.liq_px, 6)}</td>
          <td class="${p.dist_pct != null && p.dist_pct < 20 ? "neg" : "muted"}">${dist}</td>
        </tr>`;
      })
      .join("");
    $("mo-positions").innerHTML = err + `<div class="table-scroll"><table><thead><tr>
      <th>币</th><th>方向</th><th>数量</th><th>入场价</th><th>当前价</th>
      <th>名义</th><th>未实现盈亏</th><th>爆仓价</th><th>距爆仓</th>
    </tr></thead><tbody>${rows}</tbody></table></div>`;
  }

  renderLiveChart(hist);

  const recs = (d.records || []).slice().reverse();
  const pages = Math.max(1, Math.ceil(recs.length / LIVE_PAGE_SIZE));
  if (livePage > pages - 1) livePage = 0;
  const pageItems = recs.slice(livePage * LIVE_PAGE_SIZE, (livePage + 1) * LIVE_PAGE_SIZE);
  $("mo-records").innerHTML = recs.length
    ? `<div class="table-scroll"><table><thead><tr>
        <th>时间</th><th>币</th><th>方向</th><th>动作</th><th>数量</th>
        <th>计划价</th><th>成交价</th><th>滑点</th><th>状态</th>
      </tr></thead><tbody>${pageItems
        .map((r) => {
          const f = parseFill(r);
          let pxCell = '<span class="muted">—</span>';
          let slipCell = '<span class="muted">—</span>';
          let status = '<span class="badge-mini mute">—</span>';
          if (f.kind === "filled") {
            status = '<span class="badge-mini ok">成交</span>';
            pxCell = fmt(f.px, 6);
            const cls = f.slip > 0.03 ? "neg" : f.slip < -0.03 ? "pos" : "muted";
            slipCell = `<span class="${cls}">${f.slip >= 0 ? "+" : ""}${f.slip.toFixed(3)}%</span>`;
          } else if (f.kind === "unfilled") {
            status = '<span class="badge-mini warn">未成交</span>';
          } else if (f.kind === "failed") {
            status = `<span class="badge-mini err" title="${f.msg}">失败</span>`;
          } else if (f.kind === "plan") {
            status = '<span class="badge-mini mute">计划</span>';
          }
          const note =
            f.kind === "failed" && f.msg
              ? `<div class="muted" style="font-size:11px;max-width:220px;white-space:normal">${f.msg}</div>`
              : "";
          return `<tr>
            <td class="muted">${ts2m(r.ts)}</td>
            <td>${r.coin}</td>
            <td>${sideLabel(r.side === "买" ? "long" : "short")}</td>
            <td class="muted">${r.action}</td>
            <td>${fmt(r.size, 6)}</td>
            <td class="muted">${fmt(r.price, 6)}</td>
            <td>${pxCell}</td>
            <td>${slipCell}</td>
            <td>${status}${note}</td>
          </tr>`;
        })
        .join("")}</tbody></table></div>
      ${pager("live", recs.length, livePage, LIVE_PAGE_SIZE)}`
    : '<div class="empty">还没有下单记录</div>';
}

function renderLiveChart(history) {
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
      ${line}
      <text x="${W - padR + 6}" y="${endY + 4}" font-size="12" font-weight="600" fill="#2d6df6" paint-order="stroke" stroke="#fff" stroke-width="3">${money(vals[n - 1])}</text>
      ${xlabels}
    </svg>
    <div class="legend">
      <span><span style="color:#2d6df6">━</span> 实盘账户净值</span>
      <span>起点 ${money(base)} · ${ts2d(history[0].ts)} → ${ts2d(history[n - 1].ts)}</span>
    </div>`;
}

async function refreshMonitor() {
  if (liveBusy) return;
  try {
    const d = await fetchLive();
    if (!liveLoaded) {
      lvConfigInto(d);
      liveLoaded = true;
    }
    renderMonitor(d);
  } catch (e) {
    $("mo-metrics").innerHTML = `<div class="note neg">读取实盘状态失败：${e}</div>`;
  }
}

$("mo-refresh").addEventListener("click", refreshMonitor);

// ---------- 启动 ----------
refreshStatus();
setInterval(refreshStatus, 5000);
setInterval(refreshPaper, 10000);
refreshPaper();
// 实盘接口按次计费（Hyperliquid 有限流），所以低频轮询。
refreshLiveConfig();
refreshMonitor();
setInterval(refreshMonitor, 60000);
