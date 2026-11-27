import { apiGet } from "../core/api.js";
import { esc, filterRange, fmtBj, fmtNum, fmtPct, fmtSignedUsd, fmtUsd, isDustPosition, signCls } from "../core/fmt.js";
import { renderLineChart } from "../core/chart.js";

function twrStats(idxHist) {
  if (!Array.isArray(idxHist) || idxHist.length < 2) return { retPct: null, curDd: null, maxDd: null };
  let peak = -Infinity;
  let maxDd = 0;
  for (const [, v] of idxHist) {
    if (!Number.isFinite(v) || v <= 0) continue;
    if (v > peak) peak = v;
    if (peak > 0) maxDd = Math.max(maxDd, ((peak - v) / peak) * 100);
  }
  const first = idxHist[0][1];
  const last = idxHist[idxHist.length - 1][1];
  return {
    retPct: first > 0 ? ((last / first) - 1) * 100 : null,
    curDd: peak > 0 ? Math.max(0, ((peak - last) / peak) * 100) : null,
    maxDd,
  };
}

function mergePnlSeries(momPnl, mlPnl) {
  const a = (momPnl || []).filter((p) => Number.isFinite(p[0]) && Number.isFinite(p[1]));
  const b = (mlPnl || []).filter((p) => Number.isFinite(p[0]) && Number.isFinite(p[1]));
  if (!a.length) return b.map(([ts, val]) => ({ ts, val }));
  let j = 0;
  return a.map(([ts, mv]) => {
    while (j + 1 < b.length && b[j + 1][0] <= ts) j++;
    const mlv = b.length && b[0][0] <= ts ? b[j][1] : 0;
    return { ts, val: mv + mlv };
  });
}

function setSidebarDot(id, cls) {
  const el = document.getElementById(id);
  if (el) el.className = `nav-dot ${cls || ""}`;
}

export function createOverviewView() {
  let root = null;
  let state = { live: null, lo: null, range: "all", curve: "all_pnl" };

  async function load() {
    const [liveRes, loRes] = await Promise.allSettled([apiGet("/api/live"), apiGet("/api/limitoco")]);
    state.live = liveRes.status === "fulfilled" ? liveRes.value : null;
    state.lo = loRes.status === "fulfilled" ? loRes.value : null;

    const foot = document.getElementById("foot-updated");
    if (foot) foot.textContent = fmtBj(Date.now());

    if (state.live) {
      setSidebarDot("dot-live", state.live.error ? "err" : state.live.config?.armed ? "live" : "warn");
      const pill = document.getElementById("top-health-pill");
      const h = state.live.health;
      if (pill && h) {
        const cls = h.status === "green" ? "pos" : h.status === "yellow" ? "warn" : "neg";
        pill.className = `pill ${cls}`;
        pill.innerHTML = `<span class="pill-dot"></span>健康 ${h.status.toUpperCase()}${h.ic90 != null ? ` · IC ${h.ic90 >= 0 ? "+" : ""}${h.ic90.toFixed(3)}` : ""}`;
        pill.hidden = false;
      }
    }
    if (state.lo) {
      setSidebarDot("dot-limitoco", state.lo.last_error ? "err" : state.lo.running ? "live" : "warn");
    }

    render();
  }

  function render() {
    if (!root) return;
    const lv = state.live;
    const lo = state.lo;

    const momEq = Number(lv?.equity) || 0;
    const mlEq = Number(lv?.ml_account?.equity) || 0;
    const totalLiveEq = momEq + mlEq;
    const momShare = totalLiveEq > 0 ? (momEq / totalLiveEq) * 100 : 0;
    const mlShare = totalLiveEq > 0 ? (mlEq / totalLiveEq) * 100 : 0;

    const momPnlArr = lv?.mom_pnl_history || [];
    const mlPnlArr = lv?.ml_pnl_history || [];
    const momPnl = momPnlArr.length ? momPnlArr[momPnlArr.length - 1][1] : null;
    const mlPnl = mlPnlArr.length ? mlPnlArr[mlPnlArr.length - 1][1] : null;
    const combPnl = (momPnl ?? 0) + (mlPnl ?? 0);

    const momTwr = twrStats(lv?.mom_index_history);
    const mlTwr = twrStats(lv?.ml_index_history);

    const momPos = (lv?.positions || []).filter((p) => !isDustPosition(p));
    const mlPos = (lv?.ml_account?.positions || []).filter((p) => !isDustPosition(p));
    const momGross = Number(lv?.gross_notional) || 0;
    const mlGross = mlPos.reduce((s, p) => s + Math.abs(Number(p.notional) || 0), 0);
    const combLev = totalLiveEq > 0 ? (momGross + mlGross) / totalLiveEq : 0;

    const momMap = new Map(momPos.map((p) => [p.coin, p.side]));
    let opposeCount = 0;
    let sameCount = 0;
    for (const p of mlPos) {
      const ms = momMap.get(p.coin);
      if (ms) {
        if (ms === p.side) sameCount++;
        else opposeCount++;
      }
    }

    const health = lv?.health;
    const hCls = !health ? "" : health.status === "green" ? "pos" : health.status === "yellow" ? "warn" : "neg";

    // Headline stat for active chart
    let chartValText = "—";
    let chartValCls = "";
    if (state.curve === "all_pnl") {
      chartValText = fmtSignedUsd(combPnl);
      chartValCls = signCls(combPnl);
    } else if (state.curve === "mom_twr") {
      chartValText = fmtPct(momTwr.retPct, 2, true);
      chartValCls = signCls(momTwr.retPct);
    } else if (state.curve === "ml_twr") {
      chartValText = fmtPct(mlTwr.retPct, 2, true);
      chartValCls = signCls(mlTwr.retPct);
    } else if (lo) {
      chartValText = `${fmtSignedUsd(lo.pnl)} (${fmtPct(lo.pnl_pct, 2, true)})`;
      chartValCls = signCls(lo.pnl);
    }

    root.innerHTML = `
      <section class="hero-banner">
        <div class="hero-primary">
          <span class="hero-label">TOTAL LIVE PORTFOLIO · HYPERLIQUID</span>
          <div class="hero-value">${fmtUsd(totalLiveEq)}</div>
          <div class="hero-delta">
            <span class="pill ${signCls(combPnl)}">策略累计盈亏 ${fmtSignedUsd(combPnl)}</span>
            <span class="muted" style="font-size:11.5px;font-weight:500">已剔除全部出入金与划转</span>
          </div>
        </div>

        <div class="hero-cell">
          <span class="hero-label">动量策略 · ${momShare.toFixed(0)}%</span>
          <div class="hero-cell-val">${fmtUsd(momEq)}</div>
          <div class="hero-cell-sub">
            <span class="${signCls(momTwr.retPct)}">TWR ${fmtPct(momTwr.retPct, 2, true)}</span>
            <span>回撤 ${fmtPct(momTwr.curDd, 2)}</span>
          </div>
        </div>

        <div class="hero-cell">
          <span class="hero-label">ML 策略 · ${mlShare.toFixed(0)}%</span>
          <div class="hero-cell-val">${fmtUsd(mlEq)}</div>
          <div class="hero-cell-sub">
            <span class="${signCls(mlTwr.retPct)}">TWR ${fmtPct(mlTwr.retPct, 2, true)}</span>
            <span>${mlPos.length} 活跃仓</span>
          </div>
        </div>

        <div class="hero-cell">
          <span class="hero-label">限价 V2 影子 · BINANCE</span>
          <div class="hero-cell-val">${lo ? fmtUsd(lo.equity) : "—"}</div>
          <div class="hero-cell-sub">
            <span class="${signCls(lo?.pnl_pct)}">${lo ? fmtPct(lo.pnl_pct, 2, true) : "—"}</span>
            <span>胜率 ${lo ? fmtPct(lo.win_rate, 1) : "—"}</span>
          </div>
        </div>

        <div class="hero-alloc">
          <div class="bar-track">
            <div class="bar-fill" style="width:${momShare.toFixed(1)}%;background:linear-gradient(90deg,#059669,#34d399)"></div>
            <div class="bar-fill" style="width:${mlShare.toFixed(1)}%;background:linear-gradient(90deg,#0284c7,#38bdf8)"></div>
          </div>
          <div class="hero-alloc-legend">
            <span>合并总杠杆 <b>${fmtNum(combLev, 2)}x</b> · 名义 ${fmtUsd(momGross + mlGross, 0)} · ${momPos.length + mlPos.length} 仓</span>
            <span class="${opposeCount > 0 ? "warn" : ""}">跨策略交叉：反向 ${opposeCount} / 同向 ${sameCount}</span>
          </div>
        </div>
      </section>

      <section class="grid-main-side">
        <div class="card">
          <div class="card-head">
            <div class="card-title-group">
              <span class="eyebrow">REAL STRATEGY PNL · ZERO DEPOSIT DISTORTION</span>
              <h2 class="card-title">策略真实收益曲线（± 盈亏 / TWR）</h2>
              <div class="chart-headline">
                <span class="chart-big-num ${chartValCls}">${chartValText}</span>
              </div>
            </div>
            <div style="display:flex;gap:8px;flex-wrap:wrap;align-items:center">
              <div class="seg" id="ov-curve-seg">
                <button type="button" data-c="all_pnl" class="${state.curve === "all_pnl" ? "active" : ""}">合并 ±$</button>
                <button type="button" data-c="mom_twr" class="${state.curve === "mom_twr" ? "active" : ""}">动量 %</button>
                <button type="button" data-c="ml_twr" class="${state.curve === "ml_twr" ? "active" : ""}">ML %</button>
                <button type="button" data-c="lo_pnl" class="${state.curve === "lo_pnl" ? "active" : ""}">限价 V2 ±$</button>
              </div>
              <div class="seg" id="ov-range-seg">
                <button type="button" data-r="24h" class="${state.range === "24h" ? "active" : ""}">24H</button>
                <button type="button" data-r="7d" class="${state.range === "7d" ? "active" : ""}">7D</button>
                <button type="button" data-r="30d" class="${state.range === "30d" ? "active" : ""}">30D</button>
                <button type="button" data-r="all" class="${state.range === "all" ? "active" : ""}">ALL</button>
              </div>
            </div>
          </div>
          <div class="chart-box" id="ov-chart"></div>
        </div>

        <div class="card" style="display:flex;flex-direction:column;gap:12px">
          <div class="card-head" style="margin-bottom:0">
            <div class="card-title-group">
              <span class="eyebrow">STRATEGY HEALTH</span>
              <h2 class="card-title">实盘 3 因子与风控状态</h2>
            </div>
            ${health ? `<span class="pill ${hCls}"><span class="pill-dot"></span>${esc(health.status.toUpperCase())}</span>` : ""}
          </div>
          <div style="font-size:12.5px;color:var(--ink-2)">${esc(health?.reason || "等待健康度样本…")}</div>
          <div class="grid-2" style="gap:10px">
            <div class="score-item">
              <div class="score-item-head"><span>因子口径</span></div>
              <div class="mono" style="font-size:12.5px;font-weight:600">${esc(health?.factor_label || "—")}</div>
              <div class="score-item-meta">有效样本 ${health?.days ?? 0} 天</div>
            </div>
            <div class="score-item">
              <div class="score-item-head"><span>90 日 IC / 60 日</span></div>
              <div class="score-item-val ${signCls(health?.ic90)}">${health?.ic90 != null ? (health.ic90 >= 0 ? "+" : "") + health.ic90.toFixed(4) : "—"}</div>
              <div class="score-item-meta">60d ${health?.ic60 != null ? (health.ic60 >= 0 ? "+" : "") + health.ic60.toFixed(4) : "—"}</div>
            </div>
            <div class="score-item">
              <div class="score-item-head"><span>90 日多空价差</span></div>
              <div class="score-item-val ${signCls(health?.spread90)}">${health?.spread90 != null ? `${health.spread90 >= 0 ? "+" : ""}${health.spread90.toFixed(2)}%` : "—"}</div>
              <div class="score-item-meta">日均税前多空差</div>
            </div>
            <div class="score-item">
              <div class="score-item-head"><span>实盘日波动</span></div>
              <div class="score-item-val">${lv?.vol_now?.pct != null ? `${fmtNum(lv.vol_now.pct, 2)}%` : "—"}</div>
              <div class="score-item-meta">目标 ${lv?.vol_now?.target_pct ? `${fmtNum(lv.vol_now.target_pct, 1)}%` : "关闭"}</div>
            </div>
          </div>
          <div style="margin-top:auto;display:flex;gap:10px;flex-wrap:wrap">
            <button type="button" class="btn primary sm" data-jump="live">打开实盘控制台</button>
            <button type="button" class="btn sm" data-jump="limitoco">打开限价 V2</button>
          </div>
        </div>
      </section>

      <section class="card">
        <div class="card-head">
          <div class="card-title-group">
            <span class="eyebrow">ACTIVE ENGINES MATRIX</span>
            <h2 class="card-title">主力引擎核心对比</h2>
          </div>
          <span class="muted mono" style="font-size:11.5px">4 个老影子实验已收纳至左侧「ARCHIVE」按需加载</span>
        </div>
        <div class="table-wrap">
          <table class="tbl">
            <thead>
              <tr>
                <th>引擎</th>
                <th>类型 / 交易所</th>
                <th>状态</th>
                <th class="num">当前净值</th>
                <th class="num">真实策略盈亏</th>
                <th class="num">TWR 收益率</th>
                <th class="num">当前 / 最大回撤</th>
                <th class="num">活跃仓 / 杠杆</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <td><b>动量 3-Factor</b></td>
                <td class="muted">实盘 · Hyperliquid</td>
                <td><span class="pill ${lv?.config?.armed ? "pos" : "warn"}">${lv?.config?.armed ? (lv?.config?.auto_run ? "自动实盘" : "手动实盘") : "仅计划"}</span></td>
                <td class="num">${fmtUsd(momEq)}</td>
                <td class="num ${signCls(momPnl)}"><b>${momPnl != null ? fmtSignedUsd(momPnl) : "—"}</b></td>
                <td class="num ${signCls(momTwr.retPct)}">${fmtPct(momTwr.retPct, 2, true)}</td>
                <td class="num">${fmtPct(momTwr.curDd, 2)} <span class="muted">/ ${fmtPct(momTwr.maxDd, 1)}</span></td>
                <td class="num">${momPos.length} · ${momEq > 0 ? `${fmtNum(momGross / momEq, 2)}x` : "—"}</td>
                <td><button type="button" class="btn sm" data-jump="live">进入</button></td>
              </tr>
              <tr>
                <td><b>ML H1357 + VT30</b></td>
                <td class="muted">实盘 · Hyperliquid 子账户</td>
                <td><span class="pill ${lv?.ml_account?.error ? "neg" : "info"}">${lv?.ml_account?.error ? "异常" : "定时 00:10 UTC"}</span></td>
                <td class="num">${fmtUsd(mlEq)}</td>
                <td class="num ${signCls(mlPnl)}"><b>${mlPnl != null ? fmtSignedUsd(mlPnl) : "—"}</b></td>
                <td class="num ${signCls(mlTwr.retPct)}">${fmtPct(mlTwr.retPct, 2, true)}</td>
                <td class="num">${fmtPct(mlTwr.curDd, 2)} <span class="muted">/ ${fmtPct(mlTwr.maxDd, 1)}</span></td>
                <td class="num">${mlPos.length} · ${mlEq > 0 ? `${fmtNum(mlGross / mlEq, 2)}x` : "—"}</td>
                <td><button type="button" class="btn sm" data-jump="live">进入</button></td>
              </tr>
              <tr>
                <td><b>Limit-OCO V2 UltraWR</b></td>
                <td class="muted">纯 Rust 影子 · Binance USD-M</td>
                <td><span class="pill ${lo?.last_error ? "neg" : lo?.running ? "pos" : "warn"}">${esc(lo?.status_text || (lo?.running ? "运行中" : "已暂停"))}</span></td>
                <td class="num">${lo ? fmtUsd(lo.equity) : "—"}</td>
                <td class="num ${signCls(lo?.pnl)}"><b>${lo ? fmtSignedUsd(lo.pnl) : "—"}</b></td>
                <td class="num ${signCls(lo?.pnl_pct)}">${lo ? fmtPct(lo.pnl_pct, 2, true) : "—"}</td>
                <td class="num">${lo ? fmtPct(lo.max_dd_pct, 2) : "—"}</td>
                <td class="num">${Object.keys(lo?.positions || {}).length}/${lo?.config?.max_total_positions || 5} · ${lo ? `${fmtNum(lo.gross_leverage, 2)}x` : "—"}</td>
                <td><button type="button" class="btn sm" data-jump="limitoco">进入</button></td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>
    `;

    root.querySelectorAll("[data-jump]").forEach((b) => {
      b.addEventListener("click", () => window.__starsNavigate(b.dataset.jump));
    });
    root.querySelectorAll("#ov-curve-seg button").forEach((b) => {
      b.addEventListener("click", () => {
        state.curve = b.dataset.c;
        render();
      });
    });
    root.querySelectorAll("#ov-range-seg button").forEach((b) => {
      b.addEventListener("click", () => {
        state.range = b.dataset.r;
        render();
      });
    });

    renderOverviewChart();
  }

  function renderOverviewChart() {
    const el = document.getElementById("ov-chart");
    if (!el) return;
    if (state.curve === "all_pnl") {
      const pts = filterRange(mergePnlSeries(state.live?.mom_pnl_history, state.live?.ml_pnl_history), state.range);
      renderLineChart(el, pts, {
        zeroLine: 0,
        formatY: (v) => fmtSignedUsd(v, 0),
        formatTip: (v) => `合并盈亏 ${fmtSignedUsd(v)}`,
      });
    } else if (state.curve === "mom_twr") {
      const raw = (state.live?.mom_index_history || []).map(([ts, v]) => ({ ts, val: (v - 1) * 100 }));
      renderLineChart(el, filterRange(raw, state.range), {
        zeroLine: 0,
        formatY: (v) => `${v >= 0 ? "+" : ""}${v.toFixed(1)}%`,
        formatTip: (v) => `动量 TWR ${v >= 0 ? "+" : ""}${v.toFixed(2)}%`,
      });
    } else if (state.curve === "ml_twr") {
      const raw = (state.live?.ml_index_history || []).map(([ts, v]) => ({ ts, val: (v - 1) * 100 }));
      renderLineChart(el, filterRange(raw, state.range), {
        zeroLine: 0,
        formatY: (v) => `${v >= 0 ? "+" : ""}${v.toFixed(1)}%`,
        formatTip: (v) => `ML TWR ${v >= 0 ? "+" : ""}${v.toFixed(2)}%`,
      });
    } else {
      const cap = Number(state.lo?.capital) || 10000;
      const raw = (state.lo?.history || []).map(([ts, eq, lev]) => ({ ts, val: eq - cap, val2: lev }));
      renderLineChart(el, filterRange(raw, state.range), {
        zeroLine: 0,
        secondary: true,
        secondaryLabel: "总杠杆",
        formatY: (v) => fmtSignedUsd(v, 0),
        formatTip: (v) => `限价 V2 盈亏 ${fmtSignedUsd(v)}`,
      });
    }
  }

  return {
    title: "总览",
    subtitle: "Hyperliquid 实盘双策略 + Binance 限价 V2 指挥台",
    pollMs: 30_000,
    async mount(el) {
      root = el;
      await load();
    },
    async refresh() {
      await load();
    },
    unmount() {
      root = null;
    },
  };
}
