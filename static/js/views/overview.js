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
  const retPct = first > 0 ? ((last / first) - 1) * 100 : null;
  const curDd = peak > 0 ? Math.max(0, ((peak - last) / peak) * 100) : null;
  return { retPct, curDd, maxDd };
}

function setSidebarDot(id, cls) {
  const el = document.getElementById(id);
  if (el) el.className = `nav-dot ${cls || ""}`;
}

export function createOverviewView() {
  let root = null;
  let state = { live: null, lo: null, range: "all", curve: "mom" };

  async function load() {
    const [liveRes, loRes] = await Promise.allSettled([apiGet("/api/live"), apiGet("/api/limitoco")]);
    state.live = liveRes.status === "fulfilled" ? liveRes.value : null;
    state.lo = loRes.status === "fulfilled" ? loRes.value : null;

    const foot = document.getElementById("foot-updated");
    if (foot) foot.textContent = fmtBj(Date.now());

    // Update sidebar status dots
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

    const momBase = (Number(lv?.start_equity) || 0) + (Number(lv?.net_deposit) || 0);
    const momPnl = momBase > 0 ? momEq - momBase : null;
    const momTwr = twrStats(lv?.mom_index_history);
    const mlTwr = twrStats(lv?.ml_index_history);

    const momPos = (lv?.positions || []).filter((p) => !isDustPosition(p));
    const mlPos = (lv?.ml_account?.positions || []).filter((p) => !isDustPosition(p));
    const momGross = Number(lv?.gross_notional) || 0;
    const mlGross = mlPos.reduce((s, p) => s + Math.abs(Number(p.notional) || 0), 0);
    const combLev = totalLiveEq > 0 ? (momGross + mlGross) / totalLiveEq : 0;

    // Cross-strategy overlap check
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

    root.innerHTML = `
      <section class="grid-4">
        <div class="kpi">
          <div class="kpi-top"><span>实盘总净值 (HL)</span><small>动量 ${momShare.toFixed(0)}% / ML ${mlShare.toFixed(0)}%</small></div>
          <div class="kpi-val">${fmtUsd(totalLiveEq)}</div>
          <div class="bar-track">
            <div class="bar-fill" style="width:${momShare.toFixed(1)}%;background:var(--pos)"></div>
            <div class="bar-fill" style="width:${mlShare.toFixed(1)}%;background:var(--info)"></div>
          </div>
          <div class="kpi-sub"><span>动量 ${fmtUsd(momEq, 0)}</span><span>ML ${fmtUsd(mlEq, 0)}</span></div>
        </div>

        <div class="kpi">
          <div class="kpi-top"><span>动量 TWR / 回撤</span><small>${lv?.config?.rebalance_slices === 1 ? "C3 EMA" : `切片 ${lv?.config?.rebalance_slices || 3}`}</small></div>
          <div class="kpi-val ${signCls(momTwr.retPct)}">${fmtPct(momTwr.retPct, 2, true)}</div>
          <div class="kpi-sub">
            <span>当前回撤 ${fmtPct(momTwr.curDd, 2)}</span>
            <span>最大 ${fmtPct(momTwr.maxDd, 2)}</span>
          </div>
        </div>

        <div class="kpi">
          <div class="kpi-top"><span>实盘总敞口 &amp; 重叠</span><small>${momPos.length + mlPos.length} 活跃仓位</small></div>
          <div class="kpi-val">${fmtNum(combLev, 2)}x</div>
          <div class="kpi-sub">
            <span>总名义 ${fmtUsd(momGross + mlGross, 0)}</span>
            <span class="${opposeCount > 0 ? "warn" : ""}">反向 ${opposeCount} / 同向 ${sameCount}</span>
          </div>
        </div>

        <div class="kpi">
          <div class="kpi-top"><span>限价 V2 影子 (Binance)</span><small>${lo?.running ? "运行中" : "已暂停"}</small></div>
          <div class="kpi-val ${signCls(lo?.pnl_pct)}">${lo ? fmtUsd(lo.equity) : "—"}</div>
          <div class="kpi-sub">
            <span class="${signCls(lo?.pnl_pct)}">${lo ? fmtPct(lo.pnl_pct, 2, true) : "—"}</span>
            <span>胜率 ${lo ? fmtPct(lo.win_rate, 1) : "—"} · ${Object.keys(lo?.positions || {}).length} 仓</span>
          </div>
        </div>
      </section>

      <section class="grid-main-side">
        <div class="card">
          <div class="card-head">
            <div class="card-title-group">
              <span class="eyebrow">TIME-WEIGHTED RETURN · 剔除划转</span>
              <h2 class="card-title">主力策略 TWR 收益曲线</h2>
            </div>
            <div style="display:flex;gap:8px;flex-wrap:wrap">
              <div class="seg" id="ov-curve-seg">
                <button type="button" data-c="mom" class="${state.curve === "mom" ? "active" : ""}">动量 TWR</button>
                <button type="button" data-c="ml" class="${state.curve === "ml" ? "active" : ""}">ML TWR</button>
                <button type="button" data-c="lo" class="${state.curve === "lo" ? "active" : ""}">限价 V2</button>
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

        <div class="card" style="display:flex;flex-direction:column;gap:10px">
          <div class="card-head" style="margin-bottom:0">
            <div class="card-title-group">
              <span class="eyebrow">STRATEGY HEALTH</span>
              <h2 class="card-title">实盘因子与风控状态</h2>
            </div>
            ${health ? `<span class="pill ${hCls}"><span class="pill-dot"></span>${esc(health.status.toUpperCase())}</span>` : ""}
          </div>
          <div style="font-size:12px;color:var(--ink-2)">${esc(health?.reason || "等待健康度样本…")}</div>
          <div class="grid-2" style="gap:8px">
            <div class="score-item">
              <div class="score-item-head"><span>因子口径</span></div>
              <div class="mono" style="font-size:12px;font-weight:600">${esc(health?.factor_label || "—")}</div>
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
          <div style="margin-top:auto;display:flex;gap:8px;flex-wrap:wrap">
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
          <span class="muted mono" style="font-size:11px">4 个老影子实验已收纳至左侧「ARCHIVE」按需加载</span>
        </div>
        <div class="table-wrap">
          <table class="tbl">
            <thead>
              <tr>
                <th>引擎</th>
                <th>类型 / 交易所</th>
                <th>运行状态</th>
                <th class="num">当前净值</th>
                <th class="num">TWR / 收益率</th>
                <th class="num">当前回撤</th>
                <th class="num">活跃仓位</th>
                <th class="num">总杠杆</th>
                <th>操作</th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <td><b>动量 3-Factor</b></td>
                <td class="muted">实盘 · Hyperliquid</td>
                <td><span class="pill ${lv?.config?.armed ? "pos" : "warn"}">${lv?.config?.armed ? (lv?.config?.auto_run ? "自动实盘" : "手动实盘") : "仅计划"}</span></td>
                <td class="num">${fmtUsd(momEq)}</td>
                <td class="num ${signCls(momTwr.retPct)}">${fmtPct(momTwr.retPct, 2, true)}${momPnl != null ? ` <span class="muted">(${fmtSignedUsd(momPnl, 0)})</span>` : ""}</td>
                <td class="num">${fmtPct(momTwr.curDd, 2)} <span class="muted">/ max ${fmtPct(momTwr.maxDd, 1)}</span></td>
                <td class="num">${momPos.length}</td>
                <td class="num">${momEq > 0 ? `${fmtNum(momGross / momEq, 2)}x` : "—"}</td>
                <td><button type="button" class="btn sm" data-jump="live">查看</button></td>
              </tr>
              <tr>
                <td><b>ML H1357 + VT30</b></td>
                <td class="muted">实盘 · Hyperliquid 子账户</td>
                <td><span class="pill ${lv?.ml_account?.error ? "neg" : "info"}">${lv?.ml_account?.error ? "异常" : "定时 00:10 UTC"}</span></td>
                <td class="num">${fmtUsd(mlEq)}</td>
                <td class="num ${signCls(mlTwr.retPct)}">${fmtPct(mlTwr.retPct, 2, true)}</td>
                <td class="num">${fmtPct(mlTwr.curDd, 2)} <span class="muted">/ max ${fmtPct(mlTwr.maxDd, 1)}</span></td>
                <td class="num">${mlPos.length}</td>
                <td class="num">${mlEq > 0 ? `${fmtNum(mlGross / mlEq, 2)}x` : "—"}</td>
                <td><button type="button" class="btn sm" data-jump="live">查看</button></td>
              </tr>
              <tr>
                <td><b>Limit-OCO V2 UltraWR</b></td>
                <td class="muted">纯 Rust 影子 · Binance USD-M</td>
                <td><span class="pill ${lo?.last_error ? "neg" : lo?.running ? "pos" : "warn"}">${esc(lo?.status_text || (lo?.running ? "运行中" : "已暂停"))}</span></td>
                <td class="num">${lo ? fmtUsd(lo.equity) : "—"}</td>
                <td class="num ${signCls(lo?.pnl_pct)}">${lo ? `${fmtPct(lo.pnl_pct, 2, true)} (${fmtSignedUsd(lo.pnl, 0)})` : "—"}</td>
                <td class="num">${lo ? fmtPct(lo.max_dd_pct, 2) : "—"}</td>
                <td class="num">${Object.keys(lo?.positions || {}).length} / ${lo?.config?.max_total_positions || 5}</td>
                <td class="num">${lo ? `${fmtNum(lo.gross_leverage, 2)}x` : "—"}</td>
                <td><button type="button" class="btn sm" data-jump="limitoco">查看</button></td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>
    `;

    // Wire events
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
    if (state.curve === "mom") {
      const raw = (state.live?.mom_index_history || []).map(([ts, v]) => ({ ts, val: (v - 1) * 100 }));
      renderLineChart(el, filterRange(raw, state.range), {
        zeroLine: 0,
        formatY: (v) => `${v >= 0 ? "+" : ""}${v.toFixed(1)}%`,
        formatTip: (v) => `动量 TWR ${v >= 0 ? "+" : ""}${v.toFixed(2)}%`,
      });
    } else if (state.curve === "ml") {
      const raw = (state.live?.ml_index_history || []).map(([ts, v]) => ({ ts, val: (v - 1) * 100 }));
      renderLineChart(el, filterRange(raw, state.range), {
        zeroLine: 0,
        color: "#38bdf8",
        formatY: (v) => `${v >= 0 ? "+" : ""}${v.toFixed(1)}%`,
        formatTip: (v) => `ML TWR ${v >= 0 ? "+" : ""}${v.toFixed(2)}%`,
      });
    } else {
      const raw = (state.lo?.history || []).map(([ts, eq, lev]) => ({ ts, val: eq, val2: lev }));
      const cap = Number(state.lo?.capital) || 10000;
      renderLineChart(el, filterRange(raw, state.range), {
        zeroLine: cap,
        secondary: true,
        secondaryLabel: "总杠杆",
        formatY: (v) => `$${Math.round(v).toLocaleString("en-US")}`,
        formatTip: (v) => fmtUsd(v),
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
