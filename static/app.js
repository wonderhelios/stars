// stars · 动量 alpha 研究台 — 前端逻辑
const $ = (id) => document.getElementById(id);

// ---------- 工具 ----------
const pct = (v, d = 2) => `${(v >= 0 ? "+" : "")}${v.toFixed(d)}%`;
const signed = (v, d = 2) => `${(v >= 0 ? "+" : "")}${v.toFixed(d)}`;
const fmt = (v, d = 2) => v.toFixed(d);
const ts2d = (ms) => new Date(ms).toISOString().slice(0, 10);

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

  $("bt-years").innerHTML = `<table><thead><tr><th>年份</th><th>天数</th><th>alpha 年化</th><th>t 统计</th></tr></thead>
    <tbody>${d.per_year.map((y) => `<tr><td>${y.year}</td><td>${y.days}</td>
      <td class="${y.alpha_annual >= 0 ? "pos" : "neg"}">${pct(y.alpha_annual * 100)}</td>
      <td>${fmt(y.t_stat, 2)}</td></tr>`).join("")}</tbody></table>`;

  $("bt-cost").innerHTML = `<table><thead><tr><th>单边费率</th><th>净年化 alpha</th></tr></thead>
    <tbody>${d.cost_sensitivity.map((c) => `<tr><td>${(c.fee * 100).toFixed(3)}%</td>
      <td class="${c.net_annual >= 0 ? "pos" : "neg"}">${pct(c.net_annual * 100)}</td></tr>`).join("")}</tbody></table>`;

  $("bt-coins").innerHTML = `<table><thead><tr><th>币</th><th>入选次数</th></tr></thead>
    <tbody>${d.top_coins.map(([c, n]) => `<tr><td>${c}</td><td>${n}</td></tr>`).join("")}</tbody></table>`;
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
});

$("pp-step").addEventListener("click", async () => {
  const res = await fetch("/api/paper/step", { method: "POST" });
  const d = await res.json();
  if (!res.ok) $("pp-error").innerHTML = `<div class="error">${d.error || "步进失败"}</div>`;
  await refreshPaper();
});

$("pp-reset").addEventListener("click", async () => {
  await fetch("/api/paper/reset", { method: "POST" });
  await refreshPaper();
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
    renderPositions(d.positions);
    renderTrades(d.trades, d.config);
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
  $("pp-positions").innerHTML = `<table><thead><tr>
    <th>币</th><th>方向</th><th>名义</th><th>均价</th><th>当前价</th>
    <th>未实现盈亏</th><th>爆仓价</th><th>距爆仓</th>
  </tr></thead><tbody>${rows}</tbody></table>`;
}

function renderTrades(trades, config) {
  if (!trades || !trades.length) {
    $("pp-trades").innerHTML =
      '<div class="empty">还没有平仓记录（首次换仓后开始出现）</div>';
    return;
  }
  const list = trades.slice().reverse();
  const realized = list.reduce((s, t) => s + t.pnl_usd, 0);
  const wins = list.filter((t) => t.pnl_usd > 0).length;
  const rows = list
    .slice(0, 100)
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
    <table><thead><tr>
      <th>币</th><th>方向</th><th>均价</th><th>出场价</th><th>盈亏</th><th>盈亏%</th><th>持有</th><th>原因</th>
    </tr></thead><tbody>${rows}</tbody></table>`;
}

function renderChart(history) {
  if (!history || history.length < 2) {
    $("pp-chart").innerHTML = '<div class="empty">数据不足，曲线需要至少 2 个点</div>';
    return;
  }
  const W = 1080, H = 220, pad = 30;
  const all = history.flatMap((p) => [p.equity, p.market * (history[0].equity || 1)]);
  const lo = Math.min(...all), hi = Math.max(...all);
  const span = hi - lo || 1;
  const n = history.length;
  const X = (i) => pad + (i / (n - 1)) * (W - pad * 2);
  const Y = (v) => H - pad - ((v - lo) / span) * (H - pad * 2);
  const cap = history[0].equity;
  const stratLine = history.map((p, i) => `${X(i)},${Y(p.equity)}`).join(" ");
  const mktLine = history.map((p, i) => `${X(i)},${Y(p.market * cap)}`).join(" ");
  $("pp-chart").innerHTML = `
    <svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="none">
      <line x1="${pad}" y1="${H - pad}" x2="${W - pad}" y2="${H - pad}" stroke="#e7ebf1"/>
      <line x1="${pad}" y1="${pad}" x2="${pad}" y2="${H - pad}" stroke="#e7ebf1"/>
      <polyline points="${mktLine}" fill="none" stroke="#768297" stroke-width="1.5"/>
      <polyline points="${stratLine}" fill="none" stroke="#2d6df6" stroke-width="2"/>
    </svg>
    <div class="note" style="display:flex;gap:16px">
      <span><span style="color:#2d6df6">━</span> 策略净值（多空前 20%，市场中性）</span>
      <span><span style="color:#768297">━</span> 等权市场（若无对冲会拿到的）</span>
      <span>${ts2d(history[0].ts)} → ${ts2d(history[history.length - 1].ts)}</span>
    </div>`;
}

// ---------- 启动 ----------
refreshStatus();
setInterval(refreshStatus, 5000);
setInterval(refreshPaper, 10000);
refreshPaper();
