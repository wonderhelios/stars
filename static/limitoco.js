/* ==========================================================================
   BINANCE A+B LIMIT-OCO · MONITOR & SETTINGS CONTROLLER (Pure Vanilla JS)
   ========================================================================== */
(function () {
  "use strict";

  const $id = (id) => document.getElementById(id);
  let loState = null;
  let loRange = "all";
  let loTradeFilter = "all";
  let loSettingsDirty = false;
  let loSettingsLoaded = false;
  let loBusy = false;

  const fmtNum = (v, d = 2) => (Number.isFinite(Number(v)) ? Number(v).toFixed(d) : "—");
  const fmtMoney = (v) => {
    const n = Number(v);
    if (!Number.isFinite(n)) return "—";
    return "$" + n.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  };
  const fmtMoneySigned = (v) => {
    const n = Number(v);
    if (!Number.isFinite(n)) return "—";
    return (n >= 0 ? "+" : "−") + "$" + Math.abs(n).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  };
  const fmtPx = (v) => {
    const n = Number(v);
    if (!Number.isFinite(n)) return "—";
    if (n >= 1000) return n.toFixed(2);
    if (n >= 10) return n.toFixed(3);
    if (n >= 1) return n.toFixed(4);
    return n.toFixed(6).replace(/0+$/, "").replace(/\.$/, "");
  };
  const fmtBj = (ms) => {
    if (!ms) return "—";
    const iso = new Date(ms + 8 * 3600_000).toISOString();
    return iso.slice(5, 10) + " " + iso.slice(11, 16);
  };
  const esc = (s) =>
    String(s ?? "")
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");

  async function apiJson(url, opts) {
    const res = await fetch(url, Object.assign({ cache: "no-store" }, opts || {}));
    const text = await res.text();
    let data = {};
    try { data = JSON.parse(text); } catch (_) { data = { ok: false, error: text || "HTTP " + res.status }; }
    if (!res.ok) throw new Error(data.error || "HTTP " + res.status);
    return data;
  }

  function showToast(boxId, msg, kind) {
    const el = $id(boxId);
    if (!el) return;
    if (!msg) {
      el.hidden = true;
      return;
    }
    el.hidden = false;
    el.className = "lo-banner" + (kind ? " " + kind : "");
    el.innerHTML = `<span>${esc(msg)}</span><button type="button" class="lo-btn" style="padding:3px 9px;font-size:11px" onclick="this.parentElement.hidden=true">关闭</button>`;
  }

  // ---------- Countdown to next 1h bar close ----------
  function updateCountdown() {
    const el = $id("lo-countdown");
    if (!el) return;
    const now = Date.now();
    const nextHour = (Math.floor(now / 3600_000) + 1) * 3600_000;
    const rem = Math.max(0, Math.floor((nextHour - now) / 1000));
    const m = String(Math.floor(rem / 60)).padStart(2, "0");
    const s = String(rem % 60).padStart(2, "0");
    el.textContent = `距下根1h收盘 ${m}:${s}`;
  }
  setInterval(updateCountdown, 1000);

  // ---------- Equity + Leverage Dual-Layer SVG Chart ----------
  function renderChart(state) {
    const box = $id("lo-chart");
    if (!box) return;
    const raw = (state.history || []).filter((p) => Array.isArray(p) && Number.isFinite(p[0]) && Number.isFinite(p[1]));
    const pts = raw.slice();
    const now = Date.now();
    if (Number.isFinite(state.equity) && (!pts.length || now - pts[pts.length - 1][0] > 60_000)) {
      pts.push([now, state.equity, state.gross_leverage || 0]);
    }
    const cutoffMap = { day: 86400_000, week: 7 * 86400_000, month: 30 * 86400_000, all: Infinity };
    const cut = cutoffMap[loRange] || Infinity;
    const filtered = cut === Infinity ? pts : pts.filter((p) => now - p[0] <= cut);
    const data = filtered.length >= 2 ? filtered : pts;

    if (data.length < 2) {
      box.innerHTML = `<div class="lo-empty" style="height:100%;display:flex;align-items:center;justify-content:center;border:none">启动影子引擎并完成首次扫描后，此处实时绘制净值与杠杆双层曲线。</div>`;
      return;
    }

    const W = 780, H = 260, PL = 62, PR = 18, PT = 20, PB = 34;
    const cap = Number(state.capital) || data[0][1];
    const ys = data.map((p) => p[1]).concat([cap]);
    let yMin = Math.min(...ys), yMax = Math.max(...ys);
    if (yMax - yMin < Math.max(10, cap * 0.005)) {
      const pad = Math.max(25, cap * 0.01);
      yMin -= pad; yMax += pad;
    } else {
      const pad = (yMax - yMin) * 0.14;
      yMin -= pad; yMax += pad;
    }
    const t0 = data[0][0], t1 = Math.max(data[data.length - 1][0], t0 + 1);
    const xOf = (t) => PL + ((t - t0) / (t1 - t0)) * (W - PL - PR);
    const yOf = (y) => PT + (1 - (y - yMin) / (yMax - yMin)) * (H - PT - PB);

    let grid = "";
    for (let i = 0; i <= 4; i++) {
      const val = yMin + ((yMax - yMin) * i) / 4;
      const yy = yOf(val);
      grid += `<line x1="${PL}" y1="${yy.toFixed(1)}" x2="${W - PR}" y2="${yy.toFixed(1)}" stroke="rgba(255,255,255,0.06)" stroke-dasharray="3 4"/>`;
      grid += `<text x="${PL - 8}" y="${(yy + 3.5).toFixed(1)}" text-anchor="end" fill="#64748b" font-family="JetBrains Mono, monospace" font-size="10">$${val.toFixed(0)}</text>`;
    }

    const baseY = yOf(cap);
    const baseLine = `<line x1="${PL}" y1="${baseY.toFixed(1)}" x2="${W - PR}" y2="${baseY.toFixed(1)}" stroke="rgba(245,185,66,0.35)" stroke-dasharray="5 5"/>`;

    // Subtle leverage histogram along bottom
    const maxLev = Math.max(1, ...data.map((p) => Number(p[2]) || 0));
    const barW = Math.max(2, Math.min(10, (W - PL - PR) / Math.max(data.length, 1) * 0.65));
    let levBars = "";
    for (let i = 0; i < data.length; i++) {
      const lv = Number(data[i][2]) || 0;
      if (lv <= 0) continue;
      const bh = (lv / maxLev) * 34;
      const bx = xOf(data[i][0]) - barW / 2;
      const by = H - PB - bh;
      levBars += `<rect x="${bx.toFixed(1)}" y="${by.toFixed(1)}" width="${barW.toFixed(1)}" height="${bh.toFixed(1)}" rx="1.5" fill="rgba(34,211,238,0.18)"/>`;
    }

    const coords = data.map((p) => `${xOf(p[0]).toFixed(1)},${yOf(p[1]).toFixed(1)}`);
    const linePath = "M" + coords.join(" L");
    const areaPath = `${linePath} L${xOf(data[data.length - 1][0]).toFixed(1)},${(H - PB).toFixed(1)} L${xOf(data[0][0]).toFixed(1)},${(H - PB).toFixed(1)} Z`;

    const last = data[data.length - 1];
    const lx = xOf(last[0]), ly = yOf(last[1]);
    const isPos = last[1] >= cap;
    const strokeCol = isPos ? "#f5b942" : "#ff5a78";

    box.innerHTML = `
      <svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" id="lo-chart-svg">
        <defs>
          <linearGradient id="loAreaGrad" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stop-color="${strokeCol}" stop-opacity="0.30"/>
            <stop offset="100%" stop-color="${strokeCol}" stop-opacity="0.01"/>
          </linearGradient>
        </defs>
        ${grid}
        ${baseLine}
        ${levBars}
        <path d="${areaPath}" fill="url(#loAreaGrad)"/>
        <path d="${linePath}" fill="none" stroke="${strokeCol}" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/>
        <circle cx="${lx.toFixed(1)}" cy="${ly.toFixed(1)}" r="4.5" fill="${strokeCol}" stroke="#05070c" stroke-width="2"/>
        <line id="lo-xhair" x1="0" y1="${PT}" x2="0" y2="${H - PB}" stroke="rgba(255,255,255,0.28)" stroke-dasharray="3 3" visibility="hidden"/>
        <circle id="lo-xdot" cx="0" cy="0" r="4.5" fill="#22d3ee" stroke="#05070c" stroke-width="2" visibility="hidden"/>
        <text x="${PL}" y="${H - 10}" fill="#64748b" font-family="JetBrains Mono, monospace" font-size="10">${fmtBj(t0)}</text>
        <text x="${W - PR}" y="${H - 10}" text-anchor="end" fill="#64748b" font-family="JetBrains Mono, monospace" font-size="10">${fmtBj(t1)}</text>
      </svg>
      <div class="lo-chart-tip" id="lo-chart-tip"></div>
    `;

    const svg = $id("lo-chart-svg");
    const tip = $id("lo-chart-tip");
    const xhair = $id("lo-xhair");
    const xdot = $id("lo-xdot");
    if (svg && tip) {
      svg.addEventListener("mousemove", (ev) => {
        const rect = svg.getBoundingClientRect();
        const relX = ((ev.clientX - rect.left) / rect.width) * W;
        const ratio = Math.max(0, Math.min(1, (relX - PL) / (W - PL - PR)));
        const targetT = t0 + ratio * (t1 - t0);
        let best = data[0], bestD = Infinity;
        for (const p of data) {
          const d = Math.abs(p[0] - targetT);
          if (d < bestD) { bestD = d; best = p; }
        }
        const bx = xOf(best[0]), by = yOf(best[1]);
        xhair.setAttribute("x1", bx); xhair.setAttribute("x2", bx); xhair.setAttribute("visibility", "visible");
        xdot.setAttribute("cx", bx); xdot.setAttribute("cy", by); xdot.setAttribute("visibility", "visible");
        const dpct = ((best[1] / cap - 1) * 100);
        tip.style.display = "block";
        tip.innerHTML = `<div style="color:#9aa7bf">${fmtBj(best[0])}</div>
          <div style="font-size:13px;font-weight:700;margin:2px 0">${fmtMoney(best[1])} <span class="${dpct >= 0 ? "lo-pos" : "lo-neg"}">(${dpct >= 0 ? "+" : ""}${dpct.toFixed(2)}%)</span></div>
          <div style="color:#22d3ee">杠杆 ${fmtNum(best[2] || 0, 2)}x</div>`;
        const leftPx = Math.min(rect.width - 175, Math.max(10, (bx / W) * rect.width + 12));
        tip.style.left = leftPx + "px";
        tip.style.top = "14px";
      });
      svg.addEventListener("mouseleave", () => {
        tip.style.display = "none";
        xhair.setAttribute("visibility", "hidden");
        xdot.setAttribute("visibility", "hidden");
      });
    }
  }

  // ---------- Render Monitor Tab ----------
  function renderMonitor(d) {
    if (!d) return;
    const cfg = d.config || {};
    const orb = $id("lo-orb-pill");
    if (orb) {
      const cls = d.last_error ? "is-error" : (d.running ? "is-live" : "");
      orb.className = "lo-orb-pill " + cls;
      orb.innerHTML = `<span class="lo-orb"></span><span>${esc(d.status_text || (d.running ? "运行中" : "待启动"))}</span>`;
    }

    if (d.last_error) {
      showToast("lo-mon-banner", `行情警告：${d.last_error}（如在国内/受限网络，请到「限价设置」填写代理如 http://127.0.0.1:7897）`, "err");
    } else if (d.daily_breaker_active) {
      showToast("lo-mon-banner", `日内熔断保护已触发：今日已实现 ${fmtNum(d.daily_r_pnl, 2)}R，暂停新开仓至次日 UTC 00:00`, "err");
    } else {
      showToast("lo-mon-banner", "", "");
    }

    // 5 KPI cards
    const pnl = Number(d.pnl) || 0;
    const pnlPct = Number(d.pnl_pct) || 0;
    const lev = Number(d.gross_leverage) || 0;
    const maxLev = Number(cfg.max_total_gross_lev) || 6.0;
    const levRatio = Math.min(100, (lev / Math.max(maxLev, 0.1)) * 100);
    const brkLim = Number(cfg.daily_loss_limit_r) || 3.0;
    const dailyR = Number(d.daily_r_pnl) || 0;
    const brkUsed = dailyR < 0 && brkLim > 0 ? Math.min(100, (Math.abs(dailyR) / brkLim) * 100) : 0;
    const btc = d.btc_regime || {};
    const btcCls = btc.bull_ok ? "lo-pos" : (btc.short_ok ? "lo-neg" : "lo-gold");

    const kpiEl = $id("lo-kpis");
    if (kpiEl) {
      kpiEl.innerHTML = `
        <div class="lo-kpi featured">
          <div class="lo-kpi-label"><span>账户实时净值</span><span class="lo-gold">${esc(cfg.risk_tier || "1.5x")} · ${((cfg.risk_per_trade || 0.012) * 100).toFixed(1)}%R</span></div>
          <div class="lo-kpi-val">${fmtMoney(d.equity)}</div>
          <div class="lo-kpi-sub">
            <span class="${pnl >= 0 ? "lo-pos" : "lo-neg"}">${fmtMoneySigned(pnl)} (${pnlPct >= 0 ? "+" : ""}${pnlPct.toFixed(2)}%)</span>
            <span>· 本金 ${fmtMoney(d.capital)}</span>
          </div>
        </div>
        <div class="lo-kpi">
          <div class="lo-kpi-label"><span>累计落袋绩效</span><span>${(d.trades || []).length} 笔</span></div>
          <div class="lo-kpi-val ${(d.total_r || 0) >= 0 ? "lo-pos" : "lo-neg"}">${(d.total_r || 0) >= 0 ? "+" : ""}${fmtNum(d.total_r || 0, 2)}R</div>
          <div class="lo-kpi-sub">
            <span>胜率 ${fmtNum(d.win_rate || 0, 1)}%</span>
            <span>· PF ${fmtNum(d.profit_factor || 0, 2)}</span>
            <span>· 浮盈 ${fmtMoneySigned(d.unrealized_pnl || 0)}</span>
          </div>
        </div>
        <div class="lo-kpi">
          <div class="lo-kpi-label"><span>实时杠杆 &amp; 敞口</span><span>${Object.keys(d.positions || {}).length}/${cfg.max_total_positions || 5} 仓</span></div>
          <div class="lo-kpi-val">${fmtNum(lev, 2)}x <span style="font-size:13px;color:var(--lo-muted)">/ ${fmtNum(maxLev, 1)}x</span></div>
          <div class="lo-mini-bar"><i style="width:${levRatio.toFixed(1)}%"></i></div>
          <div class="lo-kpi-sub"><span>总名义 ${fmtMoney(d.gross_notional || 0)}</span><span>· 净 ${fmtMoneySigned(d.net_notional || 0)}</span></div>
        </div>
        <div class="lo-kpi">
          <div class="lo-kpi-label"><span>最大回撤 &amp; 日内熔断</span><span>UTC ${esc(d.daily_utc_day || "—")}</span></div>
          <div class="lo-kpi-val ${(d.max_dd_pct || 0) > 15 ? "lo-neg" : ""}">-${fmtNum(d.max_dd_pct || 0, 2)}%</div>
          <div class="lo-mini-bar warn"><i style="width:${brkUsed.toFixed(1)}%"></i></div>
          <div class="lo-kpi-sub"><span>今日 ${dailyR >= 0 ? "+" : ""}${fmtNum(dailyR, 2)}R</span><span>· 熔断线 -${fmtNum(brkLim, 1)}R</span></div>
        </div>
        <div class="lo-kpi">
          <div class="lo-kpi-label"><span>BTC 宏观罗盘</span><span>费率 ${fmtNum((btc.funding_rate || 0) * 10000, 2)}bp</span></div>
          <div class="lo-kpi-val" style="font-size:20px">${btc.price ? "$" + Number(btc.price).toLocaleString("en-US", { maximumFractionDigits: 0 }) : "待扫描"}</div>
          <div class="lo-kpi-sub"><span class="${btcCls}" style="font-weight:700">${esc(btc.label || "等待首次扫描")}</span></div>
          <div class="lo-kpi-sub" style="font-size:10.5px">EMA168 ${btc.ema168 ? "$" + Number(btc.ema168).toFixed(0) : "—"} · EMA336 ${btc.ema336 ? "$" + Number(btc.ema336).toFixed(0) : "—"}</div>
        </div>
      `;
    }

    renderChart(d);

    // Sleeve A & B cards
    const slvEl = $id("lo-sleeves");
    if (slvEl) {
      const a = (d.sleeve_stats || {}).A || { open: 0, max: 3, pending: 0, trades: 0, win_rate: 0, sum_r: 0, net_pnl: 0, pf: 0 };
      const b = (d.sleeve_stats || {}).B || { open: 0, max: 2, pending: 0, trades: 0, win_rate: 0, sum_r: 0, net_pnl: 0, pf: 0 };
      const pips = (open, max, cls) => Array.from({ length: Math.max(1, max) }, (_, i) => `<i class="lo-pip ${i < open ? cls : ""}"></i>`).join("");
      slvEl.innerHTML = `
        <div class="lo-sleeve-card sleeve-a">
          <div class="lo-sleeve-top">
            <span class="lo-sleeve-tag a">SLEEVE A · 7d 趋势突破</span>
            <div class="lo-slot-pips" title="持仓槽位 ${a.open}/${a.max}">${pips(a.open, a.max, "on-a")}</div>
          </div>
          <div style="font-size:11.5px;color:var(--lo-ink-2)">168h 放量突破 · BTC &gt; EMA336 做多护栏 · 止损 2.8ATR / 止盈 1.2R · 挂单 ${a.pending} 笔</div>
          <div class="lo-sleeve-metrics">
            <div class="lo-sm-item"><span>持仓/上限</span><strong>${a.open}/${a.max}</strong></div>
            <div class="lo-sm-item"><span>净贡献 R</span><strong class="${a.sum_r >= 0 ? "lo-pos" : "lo-neg"}">${a.sum_r >= 0 ? "+" : ""}${fmtNum(a.sum_r, 2)}R</strong></div>
            <div class="lo-sm-item"><span>胜率</span><strong>${fmtNum(a.win_rate, 1)}%</strong></div>
            <div class="lo-sm-item"><span>PF</span><strong>${fmtNum(a.pf, 2)}</strong></div>
          </div>
        </div>
        <div class="lo-sleeve-card sleeve-b">
          <div class="lo-sleeve-top">
            <span class="lo-sleeve-tag b">SLEEVE B · 3d 反弹急拉做空</span>
            <div class="lo-slot-pips" title="持仓槽位 ${b.open}/${b.max}">${pips(b.open, b.max, "on-b")}</div>
          </div>
          <div style="font-size:11.5px;color:var(--lo-ink-2)">空头趋势 (&lt; EMA336) 72h +6% 急拉 · 日RSI2≥90 · 止损 2.7ATR / 止盈 0.9R · 挂单 ${b.pending} 笔</div>
          <div class="lo-sleeve-metrics">
            <div class="lo-sm-item"><span>持仓/上限</span><strong>${b.open}/${b.max}</strong></div>
            <div class="lo-sm-item"><span>净贡献 R</span><strong class="${b.sum_r >= 0 ? "lo-pos" : "lo-neg"}">${b.sum_r >= 0 ? "+" : ""}${fmtNum(b.sum_r, 2)}R</strong></div>
            <div class="lo-sm-item"><span>胜率</span><strong>${fmtNum(b.win_rate, 1)}%</strong></div>
            <div class="lo-sm-item"><span>PF</span><strong>${fmtNum(b.pf, 2)}</strong></div>
          </div>
        </div>
      `;
    }

    // Active Positions with Visual OCO Rails
    const posList = Object.values(d.positions || {});
    const posCount = $id("lo-pos-count");
    if (posCount) posCount.textContent = `${posList.length} / ${cfg.max_total_positions || 5} 活跃仓位 · 手续费 ${fmtMoney(d.total_fees || 0)} · 资金费 ${fmtMoneySigned(d.total_funding || 0)}`;
    const posEl = $id("lo-positions");
    if (posEl) {
      if (!posList.length) {
        posEl.innerHTML = `<div class="lo-empty">当前空仓等待中。每个整点收盘自动扫描 Top-${cfg.top_n_liquid || 80} 合约，满足条件后自动挂出纯 Maker GTX 限价单。</div>`;
      } else {
        posEl.innerHTML = `<div class="lo-pos-grid">${posList.map((p) => {
          const prog = Math.max(-100, Math.min(100, Number(p.progress_pct) || 0));
          const markerPct = 50 + prog * 0.5;
          const fillLeft = prog >= 0 ? 50 : markerPct;
          const fillW = Math.abs(prog) * 0.5;
          const col = prog >= 0 ? "var(--lo-jade)" : "var(--lo-rose)";
          const up = Number(p.unreal_pnl) || 0;
          const ur = Number(p.unreal_r) || 0;
          return `
            <div class="lo-pos-card">
              <div class="lo-pos-head">
                <div class="lo-sym">
                  <span class="lo-sleeve-tag ${p.sleeve === "A" ? "a" : "b"}">${esc(p.sleeve)}</span>
                  <span>${esc(p.symbol)}</span>
                  <span class="lo-side-pill ${p.side === 1 ? "long" : "short"}">${p.side === 1 ? "LONG" : "SHORT"}</span>
                </div>
                <div style="text-align:right;font-family:var(--lo-mono)">
                  <div class="${up >= 0 ? "lo-pos" : "lo-neg"}" style="font-size:15px;font-weight:700">${fmtMoneySigned(up)}</div>
                  <div style="font-size:11px;color:var(--lo-ink-2)">${ur >= 0 ? "+" : ""}${fmtNum(ur, 2)}R · 持仓 ${p.hold_hours}h/${p.max_hold_hours}h</div>
                </div>
              </div>
              <div class="lo-rail-box">
                <div class="lo-rail-track">
                  <div class="lo-rail-center"></div>
                  <div class="lo-rail-fill" style="left:${fillLeft.toFixed(1)}%;width:${fillW.toFixed(1)}%;background:${col}"></div>
                  <div class="lo-rail-marker" style="left:${markerPct.toFixed(1)}%;background:${col};color:${col}"></div>
                </div>
                <div class="lo-rail-labels">
                  <span class="lo-neg">SL ${fmtPx(p.sl_px)} (-${fmtNum(p.sl_pct, 1)}%)</span>
                  <span>入 ${fmtPx(p.entry_px)} → 现 ${fmtPx(p.mark_px)}</span>
                  <span class="lo-pos">TP ${fmtPx(p.tp_px)} (+${fmtNum(p.tp_pct, 1)}%)</span>
                </div>
              </div>
              <div style="display:flex;justify-content:space-between;font-family:var(--lo-mono);font-size:11px;color:var(--lo-muted)">
                <span>名义 ${fmtMoney(p.notional)} · 1R=${fmtMoney(p.risk_usd)}</span>
                <span>资金费 ${fmtMoneySigned(p.funding_usd || 0)}</span>
              </div>
            </div>
          `;
        }).join("")}</div>`;
      }
    }

    // Pending GTX Limit Queue
    const pendList = Object.values(d.pending || {});
    const pendCount = $id("lo-pend-count");
    if (pendCount) pendCount.textContent = `${pendList.length} 笔待成交挂单`;
    const pendEl = $id("lo-pending");
    if (pendEl) {
      if (!pendList.length) {
        pendEl.innerHTML = `<div class="lo-empty">暂无挂单队列。触发 A/B 信号后会在此展示纯 Maker GTX 挂单价、距成交距离与 12h 倒计时。</div>`;
      } else {
        pendEl.innerHTML = `<div class="lo-table-wrap"><table class="lo-table"><thead><tr>
          <th>策略/标的</th><th>方向</th><th>GTX 挂单价</th><th>现价</th><th>距成交</th><th>止损 SL</th><th>止盈 TP</th><th>有效期</th><th>触发依据</th>
        </tr></thead><tbody>${pendList.map((o) => `
          <tr>
            <td><span class="lo-sleeve-tag ${o.sleeve === "A" ? "a" : "b"}">${esc(o.sleeve)}</span> <b>${esc(o.symbol)}</b></td>
            <td><span class="lo-side-pill ${o.side === 1 ? "long" : "short"}">${o.side === 1 ? "做多" : "做空"}</span></td>
            <td class="lo-cyan">${fmtPx(o.limit_px)}</td>
            <td>${fmtPx(o.mark_px)}</td>
            <td class="lo-gold">${fmtNum(o.dist_to_fill_pct, 2)}%</td>
            <td class="lo-neg">${fmtPx(o.sl_px)} (-${fmtNum(o.sl_pct, 1)}%)</td>
            <td class="lo-pos">${fmtPx(o.tp_px)} (+${fmtNum(o.tp_pct, 1)}%)</td>
            <td>${o.age_hours}h / ${o.expire_hours}h</td>
            <td style="color:var(--lo-ink-2);max-width:260px;overflow:hidden;text-overflow:ellipsis">${esc(o.reason)}</td>
          </tr>
        `).join("")}</tbody></table></div>`;
      }
    }

    // Universe & Funnel
    const uni = d.universe || {};
    const uniMeta = $id("lo-uni-meta");
    if (uniMeta) {
      uniMeta.textContent = `全市场 ${uni.total_perps || 0} 永续 · 已剔除美股 ${uni.excluded_stocks || 0} · 合格池 Top-${uni.liquid_count || 0} · 扫描耗时 ${d.scan_duration_ms || 0}ms`;
    }
    const tickerEl = $id("lo-tickers");
    if (tickerEl) {
      const prev = uni.top80_preview || [];
      if (!prev.length) {
        tickerEl.innerHTML = `<div class="lo-empty" style="width:100%">等待首次整点扫描填充 Top-80 流动性标的池…</div>`;
      } else {
        tickerEl.innerHTML = prev.map((r) => `
          <span class="lo-ticker-chip ${r.is_commodity ? "comm" : ""}">
            <b>${esc(r.symbol.replace("USDT", ""))}</b>
            <span class="${r.ret24h >= 0 ? "lo-pos" : "lo-neg"}">${r.ret24h >= 0 ? "+" : ""}${fmtNum(r.ret24h, 1)}%</span>
            <span style="color:var(--lo-muted)">$${fmtNum(r.daily_qv7_m, 0)}M</span>
          </span>
        `).join("");
      }
    }

    const fn = d.funnel || {};
    const fnEl = $id("lo-funnel");
    if (fnEl) {
      const box = (k, v) => `<div class="lo-fn-box"><span>${k}</span><strong>${v ?? 0}</strong></div>`;
      fnEl.innerHTML =
        box("已扫描币×小时", fn.bars_scanned) +
        box("A 突破/挂单", `${fn.a_raw_cross || 0} / ${fn.a_placed || 0}`) +
        box("B 急拉/挂单", `${fn.b_raw_rip || 0} / ${fn.b_placed || 0}`) +
        box("成交 / 过期逃逸", `${fn.filled || 0} / ${(fn.expired || 0) + (fn.runaway || 0)}`) +
        box("BTC 趋势拦截", fn.a_btc_blocked) +
        box("资金费过滤", (fn.a_fund_blocked || 0) + (fn.b_fund_blocked || 0)) +
        box("SL 超限(>14%)", (fn.a_sl_rejected || 0) + (fn.b_sl_rejected || 0)) +
        box("冷却中合约", Object.keys(d.cooldowns || {}).length);
    }

    // Event Feed
    const feedEl = $id("lo-events");
    if (feedEl) {
      const evs = (d.events || []).slice().reverse().slice(0, 40);
      if (!evs.length) {
        feedEl.innerHTML = `<div class="lo-empty">尚无运行事件。</div>`;
      } else {
        feedEl.innerHTML = evs.map((e) => `
          <div class="lo-feed-item">
            <span class="lo-feed-ts">${fmtBj(e.ts)}</span>
            <span class="lo-feed-tag ${esc(e.kind)}">${esc(e.kind)}</span>
            <span style="color:var(--lo-ink)">${esc(e.msg)}</span>
          </div>
        `).join("");
      }
    }

    // Closed Trades Ledger
    const trEl = $id("lo-trades");
    if (trEl) {
      let trs = (d.trades || []).slice().reverse();
      if (loTradeFilter !== "all") trs = trs.filter((t) => t.sleeve === loTradeFilter);
      if (!trs.length) {
        trEl.innerHTML = `<div class="lo-empty">暂无已平仓落袋成交。</div>`;
      } else {
        trEl.innerHTML = `<div class="lo-table-wrap"><table class="lo-table"><thead><tr>
          <th>平仓时间</th><th>策略/标的</th><th>方向</th><th>入场 → 平仓</th><th>SL / TP</th><th>持仓</th><th>净盈亏</th><th>R 倍数</th><th>退出方式</th>
        </tr></thead><tbody>${trs.slice(0, 60).map((t) => `
          <tr>
            <td style="color:var(--lo-ink-2)">${fmtBj(t.exit_ts)}</td>
            <td><span class="lo-sleeve-tag ${t.sleeve === "A" ? "a" : "b"}">${esc(t.sleeve)}</span> <b>${esc(t.symbol)}</b></td>
            <td><span class="lo-side-pill ${t.side === 1 ? "long" : "short"}">${t.side === 1 ? "多" : "空"}</span></td>
            <td>${fmtPx(t.entry_px)} → ${fmtPx(t.exit_px)}</td>
            <td style="color:var(--lo-ink-2)">-${fmtNum(t.sl_pct, 1)}% / +${fmtNum(t.tp_pct, 1)}%</td>
            <td>${t.hold_hours}h</td>
            <td class="${t.net_pnl >= 0 ? "lo-pos" : "lo-neg"}" style="font-weight:700">${fmtMoneySigned(t.net_pnl)}</td>
            <td class="${t.r_mult >= 0 ? "lo-pos" : "lo-neg"}">${t.r_mult >= 0 ? "+" : ""}${fmtNum(t.r_mult, 2)}R</td>
            <td><span class="lo-reason-pill ${esc(t.reason)}">${esc(t.reason)}</span></td>
          </tr>
        `).join("")}</tbody></table></div>`;
      }
    }
  }

  // ---------- Render Settings Tab ----------
  function renderSettings(d, forceForm) {
    if (!d) return;
    const cfg = d.config || {};
    const presets = d.tier_presets || [];

    // Ignition Deck Status
    const stOrb = $id("lo-set-orb");
    if (stOrb) {
      stOrb.className = "lo-orb-pill " + (d.last_error ? "is-error" : (d.running ? "is-live" : ""));
      stOrb.innerHTML = `<span class="lo-orb"></span><span>${esc(d.status_text || (d.running ? "运行中" : "已暂停"))}</span>`;
    }
    const startBtn = $id("lo-btn-start");
    const stopBtn = $id("lo-btn-stop");
    if (startBtn) startBtn.hidden = !!d.running;
    if (stopBtn) stopBtn.hidden = !d.running;

    // Tier Gallery Cards
    const tierEl = $id("lo-tiers");
    if (tierEl && presets.length) {
      const activeTier = ($id("lo-f-tier") && loSettingsLoaded && !forceForm) ? $id("lo-f-tier").value : (cfg.risk_tier || "1.5x");
      tierEl.innerHTML = presets.map((p) => `
        <button type="button" class="lo-tier-card ${activeTier === p.id ? "active" : ""}" data-lo-tier="${esc(p.id)}">
          <div class="lo-tier-head">
            <span class="lo-tier-name">${esc(p.label)}</span>
            ${p.id === "1.5x" ? '<span class="lo-tier-badge">推荐默认</span>' : ""}
          </div>
          <div class="lo-tier-r">${(p.risk_per_trade * 100).toFixed(1)}%R <span style="font-size:12px;color:var(--lo-ink-2);font-weight:500">/ 单仓 ${p.max_pos_lev}x · 总 ${p.max_total_gross_lev}x</span></div>
          <div class="lo-tier-stats">
            <span>35月收益 <b>+${p.bt_ret_35m_pct ?? p.bt_ret_23m_pct}%</b></span>
            <span>年化 <b>+${p.bt_ann_pct}%</b></span>
            <span>最大回撤 <b>-${p.bt_max_dd_pct}%</b></span>
            <span>最差单日 <b>${p.bt_worst_day_pct}%</b></span>
          </div>
        </button>
      `).join("");
      tierEl.querySelectorAll("[data-lo-tier]").forEach((btn) => {
        btn.addEventListener("click", () => {
          const p = presets.find((x) => x.id === btn.dataset.loTier);
          if (!p) return;
          if ($id("lo-f-tier")) $id("lo-f-tier").value = p.id;
          if ($id("lo-f-rpt")) $id("lo-f-rpt").value = (p.risk_per_trade * 100).toFixed(2);
          if ($id("lo-f-poslev")) $id("lo-f-poslev").value = p.max_pos_lev;
          if ($id("lo-f-grosslev")) $id("lo-f-grosslev").value = p.max_total_gross_lev;
          loSettingsDirty = true;
          tierEl.querySelectorAll(".lo-tier-card").forEach((c) => c.classList.toggle("active", c === btn));
          updateRiskSimulator();
        });
      });
    }

    if (!loSettingsLoaded || forceForm) {
      const setVal = (id, val) => { const el = $id(id); if (el) el.value = val; };
      const setChk = (id, val) => { const el = $id(id); if (el) el.checked = !!val; };
      setVal("lo-f-tier", cfg.risk_tier || "1.5x");
      setVal("lo-f-capital", cfg.capital ?? 10000);
      setVal("lo-f-rpt", ((cfg.risk_per_trade ?? 0.012) * 100).toFixed(2));
      setVal("lo-f-poslev", cfg.max_pos_lev ?? 2.2);
      setVal("lo-f-grosslev", cfg.max_total_gross_lev ?? 6.0);
      setVal("lo-f-maxsl", ((cfg.max_sl_pct ?? 0.14) * 100).toFixed(1));
      setVal("lo-f-cool", cfg.cooldown_hours ?? 72);
      setVal("lo-f-breaker", cfg.daily_loss_limit_r ?? 3.0);
      setVal("lo-f-maxpos", cfg.max_total_positions ?? 5);
      setVal("lo-f-topn", cfg.top_n_liquid ?? 80);
      setVal("lo-f-minvol", ((cfg.min_daily_vol_usd ?? 15_000_000) / 1e6).toFixed(0));
      setVal("lo-f-proxy", cfg.proxy_url || "");
      setChk("lo-f-compound", cfg.compounding !== false);
      setChk("lo-f-ena", cfg.enabled_a !== false);
      setChk("lo-f-enb", cfg.enabled_b !== false);
      loSettingsLoaded = true;
      loSettingsDirty = false;
    }
    updateRiskSimulator();
  }

  function updateRiskSimulator() {
    const el = $id("lo-sim-strip");
    if (!el) return;
    const cap = Math.max(100, Number($id("lo-f-capital")?.value) || 10000);
    const rpt = Math.max(0.2, Number($id("lo-f-rpt")?.value) || 1.2) / 100;
    const posLev = Math.max(0.5, Number($id("lo-f-poslev")?.value) || 2.2);
    const grossLev = Math.max(1.0, Number($id("lo-f-grosslev")?.value) || 6.0);
    const brkR = Math.max(0, Number($id("lo-f-breaker")?.value) || 3.0);
    const oneR = cap * rpt;
    el.innerHTML = `
      <div class="lo-sim-cell"><span>单笔止损额 (1R)</span><strong class="lo-gold">${fmtMoney(oneR)}</strong></div>
      <div class="lo-sim-cell"><span>策略A 止盈目标 (1.2R)</span><strong class="lo-pos">+${fmtMoney(oneR * 1.2)}</strong></div>
      <div class="lo-sim-cell"><span>策略B 止盈目标 (0.9R)</span><strong class="lo-cyan">+${fmtMoney(oneR * 0.9)}</strong></div>
      <div class="lo-sim-cell"><span>单仓 / 总名义上限</span><strong>${fmtMoney(cap * posLev)} / ${fmtMoney(cap * grossLev)}</strong></div>
      <div class="lo-sim-cell"><span>日内熔断线 (-${fmtNum(brkR, 1)}R)</span><strong class="lo-neg">-${fmtMoney(oneR * brkR)}</strong></div>
    `;
  }

  function collectSettingsPayload() {
    return {
      risk_tier: $id("lo-f-tier")?.value || "custom",
      capital: Number($id("lo-f-capital")?.value) || 10000,
      risk_per_trade: (Number($id("lo-f-rpt")?.value) || 1.2) / 100.0,
      max_pos_lev: Number($id("lo-f-poslev")?.value) || 2.2,
      max_total_gross_lev: Number($id("lo-f-grosslev")?.value) || 6.0,
      compounding: !!$id("lo-f-compound")?.checked,
      max_sl_pct: (Number($id("lo-f-maxsl")?.value) || 14.0) / 100.0,
      cooldown_hours: parseInt($id("lo-f-cool")?.value || "72", 10),
      daily_loss_limit_r: Number($id("lo-f-breaker")?.value) || 3.0,
      max_total_positions: parseInt($id("lo-f-maxpos")?.value || "5", 10),
      top_n_liquid: parseInt($id("lo-f-topn")?.value || "80", 10),
      min_daily_vol_usd: (Number($id("lo-f-minvol")?.value) || 15) * 1_000_000,
      enabled_a: !!$id("lo-f-ena")?.checked,
      enabled_b: !!$id("lo-f-enb")?.checked,
      proxy_url: ($id("lo-f-proxy")?.value || "").trim(),
    };
  }

  async function refreshLimitOco(forceForm) {
    try {
      const d = await apiJson("/api/limitoco");
      loState = d;
      renderMonitor(d);
      renderSettings(d, !!forceForm);
    } catch (err) {
      showToast("lo-mon-banner", "读取 Limit-OCO 状态失败：" + err.message, "err");
    }
  }

  async function runAction(btn, fn, okMsg) {
    if (loBusy) return;
    loBusy = true;
    const oldText = btn ? btn.textContent : "";
    if (btn) { btn.disabled = true; btn.textContent = "执行中…"; }
    try {
      const res = await fn();
      const st = res.state || res;
      if (st && st.strategy) {
        loState = st;
        renderMonitor(st);
        renderSettings(st, true);
      } else {
        await refreshLimitOco(true);
      }
      if (okMsg) showToast("lo-set-banner", okMsg, "ok");
    } catch (err) {
      showToast("lo-set-banner", err.message || "操作失败", "err");
      showToast("lo-mon-banner", err.message || "操作失败", "err");
    } finally {
      loBusy = false;
      if (btn) { btn.disabled = false; btn.textContent = oldText; }
    }
  }

  function bindEvents() {
    document.querySelectorAll('.nav button[data-view="limitoco-monitor"], .nav button[data-view="limitoco-settings"]').forEach((b) => {
      b.addEventListener("click", () => refreshLimitOco(false));
    });

    // Range tabs
    const rTabs = $id("lo-chart-tabs");
    if (rTabs) {
      rTabs.addEventListener("click", (ev) => {
        const btn = ev.target.closest("button[data-range]");
        if (!btn) return;
        loRange = btn.dataset.range;
        rTabs.querySelectorAll("button").forEach((b) => b.setAttribute("aria-pressed", String(b === btn)));
        if (loState) renderChart(loState);
      });
    }

    // Trade sleeve filter
    const tTabs = $id("lo-trade-tabs");
    if (tTabs) {
      tTabs.addEventListener("click", (ev) => {
        const btn = ev.target.closest("button[data-slv]");
        if (!btn) return;
        loTradeFilter = btn.dataset.slv;
        tTabs.querySelectorAll("button").forEach((b) => b.setAttribute("aria-pressed", String(b === btn)));
        if (loState) renderMonitor(loState);
      });
    }

    // Live input listeners for simulator + custom tier detection
    ["lo-f-capital", "lo-f-rpt", "lo-f-poslev", "lo-f-grosslev", "lo-f-breaker"].forEach((id) => {
      const el = $id(id);
      if (el) {
        el.addEventListener("input", () => {
          loSettingsDirty = true;
          if (id === "lo-f-rpt" || id === "lo-f-poslev" || id === "lo-f-grosslev") {
            if ($id("lo-f-tier")) $id("lo-f-tier").value = "custom";
            document.querySelectorAll(".lo-tier-card").forEach((c) => c.classList.remove("active"));
          }
          updateRiskSimulator();
        });
      }
    });

    // Monitor buttons
    $id("lo-mon-step")?.addEventListener("click", (e) =>
      runAction(e.currentTarget, () => apiJson("/api/limitoco/step", { method: "POST" }), "")
    );
    $id("lo-mon-refresh")?.addEventListener("click", () => refreshLimitOco(false));

    // Settings buttons
    $id("lo-btn-save")?.addEventListener("click", (e) =>
      runAction(
        e.currentTarget,
        () => apiJson("/api/limitoco/config", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(collectSettingsPayload()) }),
        "配置已保存至 Rust 后端并同步生效。"
      )
    );

    $id("lo-btn-start")?.addEventListener("click", (e) =>
      runAction(
        e.currentTarget,
        async () => {
          // Save any unsaved edits first, then start + run immediate Binance scan
          await apiJson("/api/limitoco/config", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(collectSettingsPayload()) });
          return apiJson("/api/limitoco/start", { method: "POST" });
        },
        "影子引擎已启动并完成首次币安 Top-80 行情扫描！"
      )
    );

    $id("lo-btn-stop")?.addEventListener("click", (e) =>
      runAction(e.currentTarget, () => apiJson("/api/limitoco/stop", { method: "POST" }), "影子引擎已暂停（持仓与账本完整保留）。")
    );

    $id("lo-btn-step")?.addEventListener("click", (e) =>
      runAction(e.currentTarget, () => apiJson("/api/limitoco/step", { method: "POST" }), "已完成一次币安实时行情与信号扫描。")
    );

    $id("lo-btn-reset")?.addEventListener("click", (e) => {
      if (!window.confirm("确认按当前「初始本金」清空影子持仓、挂单与历史记录，重新开始前向记录吗？")) return;
      runAction(
        e.currentTarget,
        async () => {
          await apiJson("/api/limitoco/config", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(collectSettingsPayload()) });
          return apiJson("/api/limitoco/reset", { method: "POST" });
        },
        "影子账本已按新本金与参数重置完成。"
      );
    });

    $id("lo-btn-defaults")?.addEventListener("click", () => {
      if ($id("lo-f-tier")) $id("lo-f-tier").value = "1.5x";
      if ($id("lo-f-capital")) $id("lo-f-capital").value = 10000;
      if ($id("lo-f-rpt")) $id("lo-f-rpt").value = "1.20";
      if ($id("lo-f-poslev")) $id("lo-f-poslev").value = 2.2;
      if ($id("lo-f-grosslev")) $id("lo-f-grosslev").value = 6.0;
      if ($id("lo-f-maxsl")) $id("lo-f-maxsl").value = "14.0";
      if ($id("lo-f-cool")) $id("lo-f-cool").value = 72;
      if ($id("lo-f-breaker")) $id("lo-f-breaker").value = 3.0;
      if ($id("lo-f-maxpos")) $id("lo-f-maxpos").value = 5;
      if ($id("lo-f-topn")) $id("lo-f-topn").value = 80;
      if ($id("lo-f-minvol")) $id("lo-f-minvol").value = 15;
      if ($id("lo-f-compound")) $id("lo-f-compound").checked = true;
      if ($id("lo-f-ena")) $id("lo-f-ena").checked = true;
      if ($id("lo-f-enb")) $id("lo-f-enb").checked = true;
      document.querySelectorAll(".lo-tier-card").forEach((c) => c.classList.toggle("active", c.dataset.loTier === "1.5x"));
      updateRiskSimulator();
      showToast("lo-set-banner", "已填回 1.5x 推荐默认参数，点击「保存策略配置」生效。", "");
    });
  }

  document.addEventListener("DOMContentLoaded", () => {
    bindEvents();
    updateCountdown();
    refreshLimitOco(true);
    setInterval(() => {
      const m = $id("view-limitoco-monitor");
      const s = $id("view-limitoco-settings");
      if ((m && m.classList.contains("active")) || (s && s.classList.contains("active"))) {
        refreshLimitOco(false);
      }
    }, 15000);
  });
})();
