import { apiGet, apiPost, showToast } from "../core/api.js";
import {
  esc,
  filterRange,
  fmtBj,
  fmtNum,
  fmtPct,
  fmtPx,
  fmtSignedUsd,
  fmtUsd,
  isDustPosition,
  shortAddr,
  signCls,
} from "../core/fmt.js";
import { renderLineChart } from "../core/chart.js";
import { bindPager, paginate, renderPagerHtml } from "../core/pager.js";

function twrSummary(idxHist) {
  if (!Array.isArray(idxHist) || idxHist.length < 2) return { ret: null, curDd: null, maxDd: null };
  let peak = -Infinity;
  let maxDd = 0;
  for (const [, v] of idxHist) {
    if (!Number.isFinite(v) || v <= 0) continue;
    if (v > peak) peak = v;
    if (peak > 0) maxDd = Math.max(maxDd, ((peak - v) / peak) * 100);
  }
  const last = idxHist[idxHist.length - 1][1];
  const first = idxHist[0][1];
  return {
    ret: first > 0 ? ((last / first) - 1) * 100 : null,
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

function mergeTwrSeries(momIdx, mlIdx, momW, mlW) {
  const a = (momIdx || []).filter((p) => Number.isFinite(p[0]) && Number.isFinite(p[1]));
  const b = (mlIdx || []).filter((p) => Number.isFinite(p[0]) && Number.isFinite(p[1]));
  if (!a.length) return b.map(([ts, v]) => ({ ts, val: (v - 1) * 100 }));
  let j = 0;
  return a.map(([ts, mv]) => {
    while (j + 1 < b.length && b[j + 1][0] <= ts) j++;
    const hasMl = b.length && b[0][0] <= ts;
    const mlv = hasMl ? b[j][1] : 1.0;
    const wSum = hasMl ? (momW + mlW) : momW;
    const idx = wSum > 0 ? ((hasMl ? momW * mv + mlW * mlv : momW * mv) / wSum) : mv;
    return { ts, val: (idx - 1) * 100 };
  });
}

export function createLiveView() {
  let root = null;
  let data = null;
  let subtab = "monitor"; // monitor | positions | ledger | settings
  let scope = "all"; // all | mom | ml
  let chartUnit = "usd"; // usd (+/- $) | pct (% TWR)
  let range = "all";
  let showDust = false;
  let mlPlanOut = "";
  let busy = false;
  let pgFills = 1;
  let pgFlows = 1;
  let pgDaily = 1;

  async function load() {
    data = await apiGet("/api/live");
    const foot = document.getElementById("foot-updated");
    if (foot) foot.textContent = fmtBj(Date.now());
    render();
  }

  function render() {
    if (!root || !data) return;
    const d = data;
    const cfg = d.config || {};
    const statusPill = d.error
      ? `<span class="pill neg"><span class="pill-dot"></span>异常</span>`
      : cfg.armed
        ? `<span class="pill pos"><span class="pill-dot"></span>${cfg.auto_run ? "自动实盘" : "手动实盘"}</span>`
        : `<span class="pill warn"><span class="pill-dot"></span>仅计划模式</span>`;

    root.innerHTML = `
      <div style="display:flex;align-items:center;justify-content:space-between;gap:10px;flex-wrap:wrap">
        <div class="subtabs" id="lv-subtabs">
          <button type="button" data-t="monitor" class="${subtab === "monitor" ? "active" : ""}">监控与健康</button>
          <button type="button" data-t="positions" class="${subtab === "positions" ? "active" : ""}">持仓与挂单</button>
          <button type="button" data-t="ledger" class="${subtab === "ledger" ? "active" : ""}">账务与成交</button>
          <button type="button" data-t="settings" class="${subtab === "settings" ? "active" : ""}">参数与控制</button>
        </div>
        <div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap">
          ${subtab !== "settings" ? `
            <div class="seg" id="lv-scope">
              <button type="button" data-s="all" class="${scope === "all" ? "active" : ""}">全部</button>
              <button type="button" data-s="mom" class="${scope === "mom" ? "active" : ""}">动量</button>
              <button type="button" data-s="ml" class="${scope === "ml" ? "active" : ""}">ML</button>
            </div>
          ` : ""}
          ${statusPill}
        </div>
      </div>
      ${d.error ? `<div class="banner neg">${esc(d.error)}</div>` : ""}
      ${d.unpriced?.length ? `<div class="banner warn">以下仓位缺少行情估值：${esc(d.unpriced.join(", "))}</div>` : ""}
      <div id="lv-body" style="display:flex;flex-direction:column;gap:12px"></div>
    `;

    root.querySelectorAll("#lv-subtabs button").forEach((b) => {
      b.addEventListener("click", () => {
        subtab = b.dataset.t;
        render();
      });
    });
    root.querySelectorAll("#lv-scope button").forEach((b) => {
      b.addEventListener("click", () => {
        scope = b.dataset.s;
        render();
      });
    });

    const body = document.getElementById("lv-body");
    if (subtab === "monitor") renderMonitor(body, d);
    else if (subtab === "positions") renderPositions(body, d);
    else if (subtab === "ledger") renderLedger(body, d);
    else renderSettings(body, d);
  }

  function renderMonitor(body, d) {
    const momEq = Number(d.equity) || 0;
    const mlEq = Number(d.ml_account?.equity) || 0;
    const momPos = (d.positions || []).filter((p) => !isDustPosition(p));
    const mlPos = (d.ml_account?.positions || []).filter((p) => !isDustPosition(p));
    const momGross = Number(d.gross_notional) || 0;
    const mlGross = mlPos.reduce((s, p) => s + Math.abs(Number(p.notional) || 0), 0);
    const mlNet = mlPos.reduce((s, p) => s + (p.side === "long" ? 1 : -1) * Math.abs(Number(p.notional) || 0), 0);
    const mlUpnl = mlPos.reduce((s, p) => s + (Number(p.unrealized) || 0), 0);

    const momTwr = twrSummary(d.mom_index_history);
    const mlTwr = twrSummary(d.ml_index_history);
    const momPnlArr = d.mom_pnl_history || [];
    const mlPnlArr = d.ml_pnl_history || [];
    const momStratPnl = momPnlArr.length ? momPnlArr[momPnlArr.length - 1][1] : null;
    const mlStratPnl = mlPnlArr.length ? mlPnlArr[mlPnlArr.length - 1][1] : null;
    const combStratPnl = (momStratPnl ?? 0) + (mlStratPnl ?? 0);
    const momBase = (Number(d.start_equity) || 0) + (Number(d.net_deposit) || 0);

    const totEq = momEq + mlEq;
    const momShare = totEq > 0 ? (momEq / totEq) * 100 : 100;
    const mlShare = totEq > 0 ? (mlEq / totEq) * 100 : 0;

    let heroHtml = "";
    if (scope === "all") {
      const totGross = momGross + mlGross;
      const totNet = (Number(d.net_notional) || 0) + mlNet;
      heroHtml = `
        <section class="hero-banner">
          <div class="hero-primary">
            <span class="hero-label">ALL LIVE ACCOUNTS · 动量 + ML</span>
            <div class="hero-value">${fmtUsd(totEq)}</div>
            <div class="hero-delta">
              <span class="pill ${signCls(combStratPnl)}">累计盈亏 ${fmtSignedUsd(combStratPnl)}</span>
            </div>
          </div>
          <div class="hero-cell">
            <span class="hero-label">动量 TWR / 盈亏</span>
            <div class="hero-cell-val ${signCls(momTwr.ret)}">${fmtPct(momTwr.ret, 2, true)}</div>
            <div class="hero-cell-sub"><span>${fmtUsd(momEq, 0)}</span><span class="${signCls(momStratPnl)}">${momStratPnl != null ? fmtSignedUsd(momStratPnl, 0) : "—"}</span></div>
          </div>
          <div class="hero-cell">
            <span class="hero-label">ML TWR / 盈亏</span>
            <div class="hero-cell-val ${signCls(mlTwr.ret)}">${fmtPct(mlTwr.ret, 2, true)}</div>
            <div class="hero-cell-sub"><span>${fmtUsd(mlEq, 0)}</span><span class="${signCls(mlStratPnl)}">${mlStratPnl != null ? fmtSignedUsd(mlStratPnl, 0) : "—"}</span></div>
          </div>
          <div class="hero-cell">
            <span class="hero-label">合并杠杆 / 回撤</span>
            <div class="hero-cell-val">${totEq > 0 ? `${fmtNum(totGross / totEq, 2)}x` : "—"}</div>
            <div class="hero-cell-sub"><span>净 ${fmtSignedUsd(totNet, 0)}</span><span>当前回撤 ${fmtPct(momTwr.curDd, 2)}</span></div>
          </div>
          <div class="hero-alloc">
            <div class="bar-track">
              <div class="bar-fill" style="width:${momShare.toFixed(1)}%;background:linear-gradient(90deg,#059669,#34d399)"></div>
              <div class="bar-fill" style="width:${mlShare.toFixed(1)}%;background:linear-gradient(90deg,#0284c7,#38bdf8)"></div>
            </div>
            <div class="hero-alloc-legend">
              <span>动量 ${momShare.toFixed(1)}% (${momPos.length} 仓)</span>
              <span>ML ${mlShare.toFixed(1)}% (${mlPos.length} 仓)</span>
            </div>
          </div>
        </section>
      `;
    } else if (scope === "mom") {
      heroHtml = `
        <section class="hero-banner">
          <div class="hero-primary">
            <span class="hero-label">MOMENTUM ACCOUNT · ${shortAddr(d.account)}</span>
            <div class="hero-value">${fmtUsd(momEq)}</div>
            <div class="hero-delta">
              <span class="pill ${signCls(momStratPnl)}">策略盈亏 ${momStratPnl != null ? fmtSignedUsd(momStratPnl) : "—"}</span>
              <span class="${signCls(momTwr.ret)}">TWR ${fmtPct(momTwr.ret, 2, true)}</span>
            </div>
          </div>
          <div class="hero-cell">
            <span class="hero-label">持仓浮盈 / 净投入</span>
            <div class="hero-cell-val ${signCls(d.cumulative_pnl)}">${fmtSignedUsd(d.cumulative_pnl)}</div>
            <div class="hero-cell-sub"><span>本金+净入 ${fmtUsd(momBase, 0)}</span><span>日胜率 ${d.win_rate_days != null ? `${fmtNum(d.win_rate_days, 1)}%` : "—"}</span></div>
          </div>
          <div class="hero-cell">
            <span class="hero-label">杠杆与日波动</span>
            <div class="hero-cell-val">${momEq > 0 ? `${fmtNum(momGross / momEq, 2)}x` : "—"}</div>
            <div class="hero-cell-sub"><span>${momPos.length} 仓 · 净 ${fmtSignedUsd(d.net_notional, 0)}</span><span>波动 ${d.vol_now?.pct != null ? `${fmtNum(d.vol_now.pct, 2)}%` : "—"}</span></div>
          </div>
          <div class="hero-cell">
            <span class="hero-label">当前 / 最大回撤</span>
            <div class="hero-cell-val">${fmtPct(momTwr.curDd, 2)}</div>
            <div class="hero-cell-sub"><span>最大 ${fmtPct(momTwr.maxDd, 2)}</span><span>缓冲 ${fmtPct(d.liq_buffer_pct, 1)}</span></div>
          </div>
        </section>
      `;
    } else {
      heroHtml = `
        <section class="hero-banner">
          <div class="hero-primary">
            <span class="hero-label">ML SUB-ACCOUNT · ${shortAddr(d.ml_account?.account)}</span>
            <div class="hero-value">${fmtUsd(mlEq)}</div>
            <div class="hero-delta">
              <span class="pill ${signCls(mlStratPnl)}">策略盈亏 ${mlStratPnl != null ? fmtSignedUsd(mlStratPnl) : "—"}</span>
              <span class="${signCls(mlTwr.ret)}">TWR ${fmtPct(mlTwr.ret, 2, true)}</span>
            </div>
          </div>
          <div class="hero-cell">
            <span class="hero-label">当前持仓浮盈</span>
            <div class="hero-cell-val ${signCls(mlUpnl)}">${fmtSignedUsd(mlUpnl)}</div>
            <div class="hero-cell-sub"><span>已登记净入 ${fmtSignedUsd(d.ml_net_deposit || 0, 0)}</span></div>
          </div>
          <div class="hero-cell">
            <span class="hero-label">杠杆与净敞口</span>
            <div class="hero-cell-val">${mlEq > 0 ? `${fmtNum(mlGross / mlEq, 2)}x` : "—"}</div>
            <div class="hero-cell-sub"><span>${mlPos.length} 活跃仓</span><span>净 ${fmtSignedUsd(mlNet, 0)}</span></div>
          </div>
          <div class="hero-cell">
            <span class="hero-label">当前 / 最大回撤</span>
            <div class="hero-cell-val">${fmtPct(mlTwr.curDd, 2)}</div>
            <div class="hero-cell-sub"><span>历史最大 ${fmtPct(mlTwr.maxDd, 2)}</span></div>
          </div>
        </section>
      `;
    }

    const activeHeadline =
      chartUnit === "usd"
        ? (scope === "all" ? combStratPnl : scope === "mom" ? momStratPnl : mlStratPnl)
        : (scope === "all"
            ? (momTwr.ret != null && mlTwr.ret != null && totEq > 0 ? (momEq * momTwr.ret + mlEq * mlTwr.ret) / totEq : momTwr.ret)
            : scope === "mom" ? momTwr.ret : mlTwr.ret);
    const activeHeadlineTxt = chartUnit === "usd" ? fmtSignedUsd(activeHeadline) : fmtPct(activeHeadline, 2, true);

    const sc = Array.isArray(d.scorecard) ? d.scorecard : [];
    const scHtml = sc
      .map((item) => {
        const cls = item.status === "green" ? "pos" : item.status === "yellow" ? "warn" : item.status === "red" ? "neg" : "";
        let valText = "—";
        if (item.value != null && Number.isFinite(Number(item.value))) {
          valText = item.key === "ic90" ? `${item.value >= 0 ? "+" : ""}${Number(item.value).toFixed(4)}` : `${Number(item.value).toFixed(2)}${item.unit || ""}`;
        }
        let sub = item.good || "";
        if (item.key === "dd" && item.max_dd != null) {
          sub = `历史最大 ${Number(item.max_dd).toFixed(2)}%`;
        } else if (item.key === "gross" && item.fee != null) {
          sub = `费用 ${fmtUsd(item.fee, 2)} (${item.fee_pct != null ? `${Number(item.fee_pct).toFixed(2)}%` : "—"})`;
        }
        return `
          <div class="score-item">
            <div class="score-item-head"><span>${esc(item.name)}</span><span class="pill ${cls}">${esc(item.status || "—")}</span></div>
            <div class="score-item-val ${cls}">${valText}</div>
            <div class="score-item-meta">${esc(sub)}</div>
          </div>
        `;
      })
      .join("");

    const planLines = Array.isArray(d.last_plan) ? d.last_plan : [];
    const chartTitle =
      scope === "all"
        ? (chartUnit === "usd" ? "全部账户盈亏" : "全部账户 TWR")
        : scope === "mom"
          ? (chartUnit === "usd" ? "动量策略盈亏" : "动量 TWR")
          : (chartUnit === "usd" ? "ML 策略盈亏" : "ML TWR");

    body.innerHTML = `
      ${heroHtml}

      <div class="card">
        <div class="card-head">
          <div class="card-title-group">
            <span class="eyebrow">${chartUnit === "usd" ? "STRATEGY PNL" : "TWR RETURN"}</span>
            <h2 class="card-title">${chartTitle}</h2>
            <div class="chart-headline">
              <span class="chart-big-num ${signCls(activeHeadline)}">${activeHeadlineTxt}</span>
            </div>
          </div>
          <div style="display:flex;gap:8px;flex-wrap:wrap;align-items:center">
            <div class="seg" id="lv-unit">
              <button type="button" data-u="usd" class="${chartUnit === "usd" ? "active" : ""}">± 盈亏 ($)</button>
              <button type="button" data-u="pct" class="${chartUnit === "pct" ? "active" : ""}">TWR (%)</button>
            </div>
            <div class="seg" id="lv-range">
              <button type="button" data-r="24h" class="${range === "24h" ? "active" : ""}">24H</button>
              <button type="button" data-r="7d" class="${range === "7d" ? "active" : ""}">7D</button>
              <button type="button" data-r="30d" class="${range === "30d" ? "active" : ""}">30D</button>
              <button type="button" data-r="all" class="${range === "all" ? "active" : ""}">ALL</button>
            </div>
          </div>
        </div>
        <div class="chart-box" id="lv-chart"></div>
      </div>

      ${sc.length ? `
        <div class="card">
          <div class="card-head">
            <div class="card-title-group">
              <span class="eyebrow">HEALTH</span>
              <h2 class="card-title">策略体检</h2>
            </div>
            <span class="muted mono" style="font-size:11.5px">上次运行 ${fmtBj(d.last_run_at)}</span>
          </div>
          <div class="score-grid">${scHtml}</div>
        </div>
      ` : ""}

      <div class="card">
        <div class="card-head">
          <div class="card-title-group">
            <span class="eyebrow">EXECUTION LOG</span>
            <h2 class="card-title">最近调仓计划</h2>
          </div>
          <div style="display:flex;gap:8px">
            <button type="button" class="btn sm" id="lv-quick-mom-plan">预览动量计划</button>
            <button type="button" class="btn sm" id="lv-quick-ml-plan">预览 ML 计划</button>
          </div>
        </div>
        <pre class="code-box">${planLines.length ? esc(planLines.join("\n")) : "尚无运行记录。"}</pre>
        ${mlPlanOut ? `<pre class="code-box" style="margin-top:8px;border-color:var(--info-bd)">${esc(mlPlanOut)}</pre>` : ""}
      </div>
    `;

    body.querySelectorAll("#lv-unit button").forEach((b) => {
      b.addEventListener("click", () => {
        chartUnit = b.dataset.u;
        renderMonitor(body, d);
      });
    });
    body.querySelectorAll("#lv-range button").forEach((b) => {
      b.addEventListener("click", () => {
        range = b.dataset.r;
        renderMonitor(body, d);
      });
    });

    const chartEl = document.getElementById("lv-chart");
    if (chartUnit === "usd") {
      let raw = [];
      if (scope === "all") raw = mergePnlSeries(d.mom_pnl_history, d.ml_pnl_history);
      else if (scope === "mom") raw = (d.mom_pnl_history || []).map(([ts, val]) => ({ ts, val }));
      else raw = (d.ml_pnl_history || []).map(([ts, val]) => ({ ts, val }));
      renderLineChart(chartEl, filterRange(raw, range), {
        zeroLine: 0,
        formatY: (v) => fmtSignedUsd(v, 0),
        formatTip: (v) => `策略盈亏 ${fmtSignedUsd(v)}`,
      });
    } else {
      let raw = [];
      if (scope === "all") raw = mergeTwrSeries(d.mom_index_history, d.ml_index_history, momEq, mlEq);
      else if (scope === "mom") raw = (d.mom_index_history || []).map(([ts, v]) => ({ ts, val: (v - 1) * 100 }));
      else raw = (d.ml_index_history || []).map(([ts, v]) => ({ ts, val: (v - 1) * 100 }));
      renderLineChart(chartEl, filterRange(raw, range), {
        zeroLine: 0,
        formatY: (v) => `${v >= 0 ? "+" : ""}${v.toFixed(1)}%`,
        formatTip: (v) => `TWR ${v >= 0 ? "+" : ""}${v.toFixed(2)}%`,
      });
    }

    document.getElementById("lv-quick-mom-plan")?.addEventListener("click", () => runMomAction(false));
    document.getElementById("lv-quick-ml-plan")?.addEventListener("click", previewMlPlan);
  }

  function renderPositions(body, d) {
    const momAll = (d.positions || []).map((p) => ({ ...p, _src: "动量" }));
    const mlAll = (d.ml_account?.positions || []).map((p) => ({ ...p, _src: "ML" }));

    let combined = scope === "mom" ? momAll : scope === "ml" ? mlAll : [...momAll, ...mlAll];
    const dust = combined.filter((p) => isDustPosition(p));
    const active = combined.filter((p) => !isDustPosition(p));
    const rows = (showDust ? combined : active).sort((a, b) => Math.abs(b.notional) - Math.abs(a.notional));

    // Overlap detection
    const momSides = new Map(momAll.filter((p) => !isDustPosition(p)).map((p) => [p.coin, p.side]));
    const oppose = [];
    const same = [];
    for (const p of mlAll.filter((x) => !isDustPosition(x))) {
      const ms = momSides.get(p.coin);
      if (ms) {
        if (ms === p.side) same.push(p.coin);
        else oppose.push(p.coin);
      }
    }

    const tpOrders = d.tp_orders || [];

    body.innerHTML = `
      ${(oppose.length || same.length) ? `
        <div class="banner ${oppose.length ? "warn" : "info"}">
          <span>跨策略持仓交叉：${oppose.length ? `反向对冲 ${oppose.length} 币 (${esc(oppose.join(", "))})` : "无反向冲突"}${same.length ? ` · 同向共振 ${same.length} 币 (${esc(same.join(", "))})` : ""}</span>
          <span class="mono">动量 ${momAll.filter((p) => !isDustPosition(p)).length} 仓 · ML ${mlAll.filter((p) => !isDustPosition(p)).length} 仓</span>
        </div>
      ` : ""}

      <div class="card">
        <div class="card-head">
          <div class="card-title-group">
            <span class="eyebrow">OPEN POSITIONS</span>
            <h2 class="card-title">当前活跃持仓（${active.length} 仓）</h2>
          </div>
          ${dust.length ? `
            <button type="button" class="btn ghost sm" id="lv-toggle-dust">
              ${showDust ? `隐藏 ${dust.length} 笔 <$1 碎仓` : `展开 ${dust.length} 笔 <$1 碎仓`}
            </button>
          ` : ""}
        </div>
        ${rows.length ? `
          <div class="table-wrap">
            <table class="tbl">
              <thead>
                <tr>
                  <th>策略</th>
                  <th>标的</th>
                  <th>方向</th>
                  <th class="num">数量</th>
                  <th class="num">开仓均价</th>
                  <th class="num">标记价</th>
                  <th class="num">名义价值</th>
                  <th class="num">未实现盈亏</th>
                  <th class="num">距强平</th>
                  <th>保证金</th>
                </tr>
              </thead>
              <tbody>
                ${rows.map((p) => `
                  <tr style="${isDustPosition(p) ? "opacity:0.55" : ""}">
                    <td><span class="pill ${p._src === "ML" ? "info" : ""}">${p._src}</span></td>
                    <td><b>${esc(p.coin)}</b>${isDustPosition(p) ? ' <span class="muted" style="font-size:10px">碎仓</span>' : ""}</td>
                    <td><span class="pill ${p.side === "long" ? "pos" : "neg"}">${p.side === "long" ? "LONG" : "SHORT"}</span></td>
                    <td class="num">${fmtNum(Math.abs(p.size), 4)}</td>
                    <td class="num">${fmtPx(p.entry_px)}</td>
                    <td class="num">${fmtPx(p.mark_px)}</td>
                    <td class="num">${fmtUsd(p.notional)}</td>
                    <td class="num ${signCls(p.unrealized)}"><b>${fmtSignedUsd(p.unrealized)}</b></td>
                    <td class="num ${p.dist_pct != null && p.dist_pct < 15 ? "warn" : "muted"}">${p.dist_pct != null ? fmtPct(p.dist_pct, 1) : "—"}</td>
                    <td class="mono muted">${p.is_cross ? "全仓" : "逐仓"} ${p.leverage || ""}x</td>
                  </tr>
                `).join("")}
              </tbody>
            </table>
          </div>
        ` : `<div class="empty">当前范围暂无活跃持仓。</div>`}
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title-group">
            <span class="eyebrow">RESTING ORDERS</span>
            <h2 class="card-title">交易所活跃挂单 / 止盈单（${tpOrders.length} 笔）</h2>
          </div>
          <button type="button" class="btn sm" id="lv-btn-tp">刷新挂止盈</button>
        </div>
        ${tpOrders.length ? `
          <div class="table-wrap">
            <table class="tbl">
              <thead><tr><th>标的</th><th>方向</th><th class="num">价格</th><th class="num">数量</th><th>类型</th><th>OID</th></tr></thead>
              <tbody>
                ${tpOrders.map((o) => `
                  <tr>
                    <td><b>${esc(o.coin)}</b></td>
                    <td><span class="pill ${o.side === "B" ? "pos" : "neg"}">${o.side === "B" ? "买" : "卖"}</span></td>
                    <td class="num">${fmtPx(o.limit_px || o.limitPx)}</td>
                    <td class="num">${fmtNum(o.sz, 4)}</td>
                    <td class="muted">${o.reduce_only || o.reduceOnly ? "只减仓" : "开仓"}</td>
                    <td class="mono muted">${esc(o.oid)}</td>
                  </tr>
                `).join("")}
              </tbody>
            </table>
          </div>
        ` : `<div class="empty">当前盘口无活跃挂单。</div>`}
      </div>
    `;

    document.getElementById("lv-toggle-dust")?.addEventListener("click", () => {
      showDust = !showDust;
      renderPositions(body, d);
    });
    document.getElementById("lv-btn-tp")?.addEventListener("click", async () => {
      try {
        await apiPost("/api/live/tp", {});
        showToast("已刷新止盈挂单", "pos");
        await load();
      } catch (e) {
        showToast(`刷新止盈失败：${e.message}`, "neg");
      }
    });
  }

  function renderLedger(body, d) {
    const momFlows = (d.capital_flows || []).map((f, idx) => ({ ...f, _idx: idx, _acct: "momentum" }));
    const mlFlows = (d.ml_capital_flows || []).map((f, idx) => ({ ...f, _idx: idx, _acct: "ml" }));
    const flows = (scope === "mom" ? momFlows : scope === "ml" ? mlFlows : [...momFlows, ...mlFlows])
      .sort((a, b) => (b.effective_ts || b.ts) - (a.effective_ts || a.ts));

    const dailyAll = (d.exec_daily || []).slice().reverse();
    const recordsAll = (d.records || []).slice().reverse();
    const flowsPage = paginate(flows, pgFlows, 8);
    pgFlows = flowsPage.page;
    const dailyPage = paginate(dailyAll, pgDaily, 8);
    pgDaily = dailyPage.page;
    const fillsPage = paginate(recordsAll, pgFills, 8);
    pgFills = fillsPage.page;

    body.innerHTML = `
      <div class="grid-2">
        <div class="card">
          <div class="card-head">
            <div class="card-title-group">
              <span class="eyebrow">CAPITAL FLOWS</span>
              <h2 class="card-title">出入金流水</h2>
            </div>
            <span class="mono muted" style="font-size:11px">动量 ${fmtSignedUsd(d.net_deposit || 0, 0)} · ML ${fmtSignedUsd(d.ml_net_deposit || 0, 0)}</span>
          </div>
          <div class="form-grid" style="grid-template-columns:repeat(auto-fill,minmax(130px,1fr));margin-bottom:10px">
            <div class="field">
              <label>账户</label>
              <select id="cf-acct">
                <option value="momentum" ${scope === "ml" ? "" : "selected"}>动量账户</option>
                <option value="ml" ${scope === "ml" ? "selected" : ""}>ML 子账户</option>
              </select>
            </div>
            <div class="field">
              <label>金额 (入+/出-)</label>
              <input id="cf-amount" type="number" step="10" placeholder="如 -630" />
            </div>
            <div class="field">
              <label>到账时间 (可选)</label>
              <input id="cf-time" type="datetime-local" />
            </div>
            <div class="field">
              <label>备注</label>
              <input id="cf-note" type="text" placeholder="备注" />
            </div>
          </div>
          <div style="display:flex;gap:8px;margin-bottom:12px">
            <button type="button" class="btn primary sm" id="cf-add">记录流水</button>
          </div>
          ${flowsPage.rows.length ? `
            <div class="table-wrap">
              <table class="tbl">
                <thead><tr><th>账户</th><th>时间</th><th class="num">金额</th><th>备注</th><th></th></tr></thead>
                <tbody>
                  ${flowsPage.rows.map((f) => `
                    <tr>
                      <td><span class="pill ${f._acct === "ml" ? "info" : ""}">${f._acct === "ml" ? "ML" : "动量"}</span></td>
                      <td class="mono muted">${fmtBj(f.effective_ts || f.ts)}</td>
                      <td class="num ${signCls(f.amount)}"><b>${fmtSignedUsd(f.amount)}</b></td>
                      <td class="muted">${esc(f.note || "—")}</td>
                      <td><button type="button" class="btn ghost sm" data-del-flow="${f._idx}" data-del-acct="${f._acct}">删除</button></td>
                    </tr>
                  `).join("")}
                </tbody>
              </table>
            </div>
            ${renderPagerHtml("pg-flows", flowsPage.page, flowsPage.totalPages, flowsPage.total)}
          ` : `<div class="empty">暂无出入金记录。</div>`}
        </div>

        <div class="card">
          <div class="card-head">
            <div class="card-title-group">
              <span class="eyebrow">EXECUTION</span>
              <h2 class="card-title">每日执行统计</h2>
            </div>
          </div>
          ${dailyPage.rows.length ? `
            <div class="table-wrap">
              <table class="tbl">
                <thead><tr><th>日期</th><th class="num">笔数</th><th class="num">成交名义</th><th class="num">换手</th><th class="num">均滑点</th></tr></thead>
                <tbody>
                  ${dailyPage.rows.map((r) => `
                    <tr>
                      <td class="mono">${fmtBj(r.day * 86_400_000).slice(0, 5)}</td>
                      <td class="num">${r.orders}</td>
                      <td class="num">${fmtUsd(r.notional, 0)}</td>
                      <td class="num">${fmtNum(r.turnover, 2)}x</td>
                      <td class="num ${r.avg_slip_pct != null && r.avg_slip_pct > 0.05 ? "warn" : ""}">${r.avg_slip_pct != null ? `${r.avg_slip_pct >= 0 ? "+" : ""}${r.avg_slip_pct.toFixed(3)}%` : "—"}</td>
                    </tr>
                  `).join("")}
                </tbody>
              </table>
            </div>
            ${renderPagerHtml("pg-daily", dailyPage.page, dailyPage.totalPages, dailyPage.total)}
          ` : `<div class="empty">暂无执行统计。</div>`}
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title-group">
            <span class="eyebrow">FILLS</span>
            <h2 class="card-title">成交流水</h2>
          </div>
          <button type="button" class="btn danger sm" id="lv-clear-records">清空记录</button>
        </div>
        ${fillsPage.rows.length ? `
          <div class="table-wrap">
            <table class="tbl">
              <thead><tr><th>时间</th><th>标的</th><th>方向</th><th class="num">名义</th><th class="num">参考价</th><th class="num">已实现盈亏</th><th>执行结果</th></tr></thead>
              <tbody>
                ${fillsPage.rows.map((r) => {
                  const isMaker = /Maker成交/i.test(r.result || "");
                  return `
                    <tr>
                      <td class="mono muted">${fmtBj(r.ts)}</td>
                      <td><b>${esc(r.coin)}</b></td>
                      <td><span class="pill ${r.side?.startsWith("买") ? "pos" : "neg"}">${esc(r.side)}</span></td>
                      <td class="num">${fmtUsd(r.notional)}</td>
                      <td class="num">${fmtPx(r.price)}</td>
                      <td class="num ${signCls(r.pnl)}">${r.pnl != null ? fmtSignedUsd(r.pnl) : "—"}</td>
                      <td>${isMaker ? '<span class="pill pos" style="margin-right:4px">MAKER</span>' : ""}<span class="muted">${esc(r.result)}</span></td>
                    </tr>
                  `;
                }).join("")}
              </tbody>
            </table>
          </div>
          ${renderPagerHtml("pg-fills", fillsPage.page, fillsPage.totalPages, fillsPage.total)}
        ` : `<div class="empty">暂无成交流水。</div>`}
      </div>
    `;

    bindPager(body, "pg-flows", (p) => { pgFlows = p; renderLedger(body, d); });
    bindPager(body, "pg-daily", (p) => { pgDaily = p; renderLedger(body, d); });
    bindPager(body, "pg-fills", (p) => { pgFills = p; renderLedger(body, d); });

    document.getElementById("cf-add")?.addEventListener("click", async () => {
      const amount = Number(document.getElementById("cf-amount")?.value);
      const account_type = document.getElementById("cf-acct")?.value || "momentum";
      const note = document.getElementById("cf-note")?.value || "";
      const timeStr = document.getElementById("cf-time")?.value;
      const effective_ts = timeStr ? new Date(timeStr).getTime() : null;
      if (!Number.isFinite(amount) || amount === 0) {
        showToast("请输入非零金额", "warn");
        return;
      }
      try {
        await apiPost("/api/live/capital", { amount, effective_ts, note, account_type });
        showToast("资金流水已记录", "pos");
        await load();
      } catch (e) {
        showToast(`记录失败：${e.message}`, "neg");
      }
    });

    body.querySelectorAll("[data-del-flow]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        if (!confirm("确定删除该条资金流水？")) return;
        try {
          await apiPost("/api/live/capital/delete", {
            index: Number(btn.dataset.delFlow),
            account_type: btn.dataset.delAcct,
          });
          showToast("已删除流水", "pos");
          await load();
        } catch (e) {
          showToast(`删除失败：${e.message}`, "neg");
        }
      });
    });

    document.getElementById("lv-clear-records")?.addEventListener("click", async () => {
      if (!confirm("仅清空本地成交展示记录（不影响真实持仓），确认继续？")) return;
      try {
        await apiPost("/api/live/records/clear", {});
        showToast("记录已清空", "pos");
        await load();
      } catch (e) {
        showToast(`清空失败：${e.message}`, "neg");
      }
    });
  }

  function renderSettings(body, d) {
    const c = d.config || {};
    body.innerHTML = `
      <div class="grid-main-side">
        <div style="display:flex;flex-direction:column;gap:12px">
          <div class="card">
            <div class="card-head">
              <div class="card-title-group">
                <span class="eyebrow">01 · PORTFOLIO &amp; SIGNAL (MOMENTUM C3)</span>
                <h2 class="card-title">动量信号与平滑调仓参数</h2>
              </div>
              <button type="button" class="btn ghost sm" id="lv-fill-c3">一键填入 C3 推荐值</button>
            </div>
            <div class="form-grid">
              <div class="field"><label>每侧持仓数</label><input id="f-positions" type="number" min="1" max="30" value="${c.target_positions ?? 10}" /></div>
              <div class="field"><label>动量回看 (天)</label><input id="f-lookback" type="number" min="2" max="120" value="${c.lookback ?? 14}" /></div>
              <div class="field"><label>选币比例 <small>C3=0.30</small></label><input id="f-top" type="number" step="0.05" min="0.02" max="0.5" value="${c.top_frac ?? 0.3}" /></div>
              <div class="field">
                <label>最低日成交额</label>
                <select id="f-minvol">
                  <option value="2000000" ${c.min_vol_usd === 2000000 ? "selected" : ""}>$2M</option>
                  <option value="5000000" ${c.min_vol_usd === 5000000 ? "selected" : ""}>$5M</option>
                  <option value="10000000" ${(c.min_vol_usd ?? 10000000) === 10000000 ? "selected" : ""}>$10M</option>
                </select>
              </div>
              <div class="field"><label>错峰切片数 <small>C3=1</small></label><input id="f-slices" type="number" min="1" max="10" value="${c.rebalance_slices ?? 1}" /></div>
              <div class="field"><label>EMA 平滑系数 <small>0.1~1</small></label><input id="f-ema" type="number" step="0.05" min="0.1" max="1" value="${c.ema_alpha ?? 0.5}" /></div>
              <div class="field"><label>调仓死区 (%) <small>C3=5%</small></label><input id="f-band" type="number" step="1" min="0" max="50" value="${Math.round((c.rebal_band ?? 0.05) * 100)}" /></div>
              <div class="field"><label>动量权重 (%)</label><input id="f-momw" type="number" step="5" min="0" max="100" value="${Math.round((c.momentum_weight ?? 1.0) * 100)}" /></div>
              <div class="field"><label>DD14 权重</label><input id="f-ddw" type="number" step="0.1" min="0" max="2" value="${c.dd_weight ?? 0.5}" /></div>
              <div class="field"><label>UpFrac14 权重</label><input id="f-upw" type="number" step="0.1" min="0" max="2" value="${c.upfrac_weight ?? 0.5}" /></div>
              <div class="field"><label>目标日波动 (%) <small>0关闭</small></label><input id="f-voltarget" type="number" step="0.1" min="0" max="10" value="${Number(((c.vol_target ?? 0) * 100).toFixed(2))}" /></div>
              <div class="field"><label>单币止损 (%) <small>推荐0</small></label><input id="f-stop" type="number" step="1" min="0" max="90" value="${Math.round((c.position_stop_pct ?? 0) * 100)}" /></div>
              <div class="field"><label>止盈比例 (%) <small>0关闭</small></label><input id="f-tp" type="number" step="1" min="0" max="50" value="${Math.round((c.take_profit_pct ?? 0) * 100)}" /></div>
              <div class="field"><label>止损冷却 (天)</label><input id="f-cool" type="number" min="0" max="60" value="${c.stop_cooldown_days ?? 5}" /></div>
            </div>
          </div>

          <div class="card">
            <div class="card-head">
              <div class="card-title-group">
                <span class="eyebrow">02 · RISK, LEVERAGE &amp; ACCOUNTS</span>
                <h2 class="card-title">杠杆上限、成交约束与账户地址</h2>
              </div>
            </div>
            <div class="form-grid">
              <div class="field"><label>目标杠杆 (x)</label><input id="f-lev" type="number" step="0.5" min="1" max="20" value="${c.leverage ?? 3}" /></div>
              <div class="field"><label>硬杠杆上限 (x) <small>默认2.8</small></label><input id="f-hardcap" type="number" step="0.1" min="0.5" max="5" value="${c.hard_gross_cap ?? 2.8}" /></div>
              <div class="field"><label>保证金缓冲 (%)</label><input id="f-buf" type="number" min="30" max="100" value="${Math.round((c.margin_buffer ?? 0.9) * 100)}" /></div>
              <div class="field"><label>单币权重上限 (%)</label><input id="f-maxsingle" type="number" step="1" min="0" max="100" value="${Math.round((c.max_single_weight ?? 0.2) * 100)}" /></div>
              <div class="field"><label>滑点容忍 (%)</label><input id="f-slip" type="number" step="0.05" min="0.05" max="5" value="${Number(((c.slippage ?? 0.005) * 100).toFixed(2))}" /></div>
              <div class="field"><label>最小下单/净值 (%)</label><input id="f-minpct" type="number" step="0.1" min="0" max="5" value="${Number(((c.min_order_pct ?? 0.005) * 100).toFixed(2))}" /></div>
              <div class="field"><label>主账户地址</label><input id="f-account" type="text" value="${esc(c.account || "")}" /></div>
              <div class="field"><label>动量子账户 (Vault)</label><input id="f-vault" type="text" value="${esc(c.vault_address || "")}" /></div>
              <div class="field"><label>ML 子账户 (Vault)</label><input id="f-ml-vault" type="text" value="${esc(c.ml_vault_address || "")}" /></div>
              <div class="field"><label>API 密钥路径</label><input id="f-key" type="text" value="${esc(c.key_path || "")}" /></div>
              <div class="field"><label>ML Python 路径</label><input id="f-ml-py" type="text" value="${esc(c.ml_python || "")}" /></div>
              <div class="field"><label>ML 脚本目录</label><input id="f-ml-dir" type="text" value="${esc(c.ml_dir || "")}" /></div>
            </div>
            <div style="margin-top:14px;display:flex;justify-content:flex-end;gap:8px">
              <button type="button" class="btn primary" id="lv-save-btn">保存全部配置</button>
            </div>
          </div>
        </div>

        <div style="display:flex;flex-direction:column;gap:12px">
          <div class="card" style="display:flex;flex-direction:column;gap:10px">
            <div class="card-head" style="margin-bottom:0">
              <div class="card-title-group">
                <span class="eyebrow">EXECUTION GATES</span>
                <h2 class="card-title">实盘启停开关</h2>
              </div>
            </div>
            <label class="check-row">
              <div><strong>启用实盘发单 (Armed)</strong><small>关闭时仅生成计划，不向交易所发单</small></div>
              <input id="f-armed" type="checkbox" ${c.armed ? "checked" : ""} />
            </label>
            <label class="check-row">
              <div><strong>每日自动调仓 (UTC 00:05)</strong><small>收盘后自动计算并执行动量调仓</small></div>
              <input id="f-auto" type="checkbox" ${c.auto_run ? "checked" : ""} />
            </label>
          </div>

          <div class="card" style="display:flex;flex-direction:column;gap:8px">
            <div class="card-head" style="margin-bottom:2px">
              <div class="card-title-group">
                <span class="eyebrow">MANUAL OPERATIONS</span>
                <h2 class="card-title">手动计划与执行</h2>
              </div>
            </div>
            <button type="button" class="btn" id="op-mom-plan">动量计划预览（不下单）</button>
            <button type="button" class="btn danger" id="op-mom-exec">动量执行实盘调仓</button>
            <button type="button" class="btn" id="op-ml-plan">ML 计划预览（不下单）</button>
            <button type="button" class="btn danger" id="op-ml-exec">ML 执行实盘调仓</button>
            <hr style="border:none;border-top:1px solid var(--border);margin:4px 0" />
            <button type="button" class="btn warn sm" id="op-reset-start">重置动量记账起点（自动备份）</button>
            <button type="button" class="btn warn sm" id="op-ml-reset">重置 ML 净值历史基准</button>
          </div>

          <div class="card">
            <div class="card-head" style="margin-bottom:6px">
              <div class="card-title-group">
                <span class="eyebrow">OUTPUT</span>
                <h2 class="card-title">命令返回</h2>
              </div>
            </div>
            <pre class="code-box" id="op-out">${esc(mlPlanOut || (d.last_plan || []).join("\n") || "点击上方按钮预览计划。")}</pre>
          </div>
        </div>
      </div>
    `;

    document.getElementById("lv-fill-c3")?.addEventListener("click", () => {
      document.getElementById("f-top").value = "0.3";
      document.getElementById("f-slices").value = "1";
      document.getElementById("f-ema").value = "0.5";
      document.getElementById("f-band").value = "5";
      document.getElementById("f-hardcap").value = "2.8";
      document.getElementById("f-minpct").value = "0.5";
      document.getElementById("f-stop").value = "0";
      showToast("已填入动量 C3 推荐参数，请点击「保存全部配置」生效", "info");
    });

    document.getElementById("lv-save-btn")?.addEventListener("click", saveLiveConfig);
    document.getElementById("op-mom-plan")?.addEventListener("click", () => runMomAction(false));
    document.getElementById("op-mom-exec")?.addEventListener("click", () => {
      if (confirm("确认立即执行动量实盘调仓并发送真实订单？")) runMomAction(true);
    });
    document.getElementById("op-ml-plan")?.addEventListener("click", previewMlPlan);
    document.getElementById("op-ml-exec")?.addEventListener("click", async () => {
      if (!confirm("确认立即执行 ML 实盘调仓（先平后开 · Maker 优先）？")) return;
      try {
        const res = await apiPost("/api/ml/execute", {});
        mlPlanOut = JSON.stringify(res, null, 2);
        showToast("ML 调仓执行完成", "pos");
        await load();
      } catch (e) {
        showToast(`ML 执行失败：${e.message}`, "neg");
      }
    });
    document.getElementById("op-reset-start")?.addEventListener("click", async () => {
      if (!confirm("将动量记账起点重置为当前净值（旧记录自动备份，真实资金不变），确认？")) return;
      try {
        await apiPost("/api/live/reset-start", {});
        showToast("动量记账起点已重置", "pos");
        await load();
      } catch (e) {
        showToast(`重置失败：${e.message}`, "neg");
      }
    });
    document.getElementById("op-ml-reset")?.addEventListener("click", async () => {
      if (!confirm("清空 ML 净值历史并以当前净值为新基准，确认？")) return;
      try {
        const res = await apiPost("/api/ml/reset", {});
        showToast(res.note || "ML 基准已重置", "pos");
        await load();
      } catch (e) {
        showToast(`重置失败：${e.message}`, "neg");
      }
    });
  }

  async function saveLiveConfig() {
    if (busy) return;
    busy = true;
    try {
      const val = (id) => document.getElementById(id)?.value;
      const num = (id) => Number(val(id));
      const payload = {
        target_positions: Math.round(num("f-positions")),
        lookback: Math.round(num("f-lookback")),
        top_frac: num("f-top"),
        min_vol_usd: num("f-minvol"),
        rebalance_slices: Math.round(num("f-slices")),
        ema_alpha: num("f-ema"),
        rebal_band: num("f-band") / 100,
        momentum_weight: num("f-momw") / 100,
        dd_weight: num("f-ddw"),
        upfrac_weight: num("f-upw"),
        vol_target: num("f-voltarget") / 100,
        position_stop_pct: num("f-stop") / 100,
        take_profit_pct: num("f-tp") / 100,
        stop_cooldown_days: Math.round(num("f-cool")),
        leverage: num("f-lev"),
        hard_gross_cap: num("f-hardcap"),
        margin_buffer: num("f-buf") / 100,
        max_single_weight: num("f-maxsingle") / 100,
        slippage: num("f-slip") / 100,
        min_order_pct: num("f-minpct") / 100,
        account: (val("f-account") || "").trim(),
        vault_address: (val("f-vault") || "").trim(),
        ml_vault_address: (val("f-ml-vault") || "").trim(),
        key_path: (val("f-key") || "").trim(),
        ml_python: (val("f-ml-py") || "").trim(),
        ml_dir: (val("f-ml-dir") || "").trim(),
        armed: !!document.getElementById("f-armed")?.checked,
        auto_run: !!document.getElementById("f-auto")?.checked,
      };
      await apiPost("/api/live/config", payload);
      showToast("实盘配置已保存", "pos");
      await load();
    } catch (e) {
      showToast(`保存失败：${e.message}`, "neg");
    } finally {
      busy = false;
    }
  }

  async function runMomAction(live) {
    try {
      const res = await apiPost("/api/live/run", { live });
      showToast(res.message || (live ? "动量调仓已启动" : "动量计划计算中…"), "info");
      setTimeout(() => load().catch(() => {}), 2500);
    } catch (e) {
      showToast(`动量请求失败：${e.message}`, "neg");
    }
  }

  async function previewMlPlan() {
    showToast("正在计算 ML 目标计划…", "info", 2000);
    try {
      const res = await apiGet("/api/ml/plan");
      const p = res.plan || {};
      const t = p.target || {};
      const orders = p.orders || [];
      const lines = [
        `ML 计划预览 · 模型 ${t.model || "—"} · 日期 ${t.date || "—"}`,
        `因果 rv20=${t.rv20 != null ? (t.rv20 * 100).toFixed(2) + "%" : "—"} | lev_scale=${t.lev_scale != null ? Number(t.lev_scale).toFixed(3) : "—"} | 有效杠杆=${t.gross_leverage != null ? Number(t.gross_leverage).toFixed(2) + "x" : "—"} | buffer=${t.buffer ?? "—"}`,
        `多头 (${(t.long || []).length}): ${(t.long || []).join(", ")}`,
        `空头 (${(t.short || []).length}): ${(t.short || []).join(", ")}`,
        `待执行订单 (${orders.length} 笔):`,
        ...orders.map((o) => `  [${(o.action || "").toUpperCase()}] ${o.buy ? "买入" : "卖出"} ${o.coin} ${o.size} @ ${o.price} (${fmtUsd(o.delta_usd)}) ${o.reduce_only ? "只减仓" : ""}`),
      ];
      mlPlanOut = lines.join("\n");
      render();
      showToast(`ML 计划完成（${orders.length} 笔订单）`, "pos");
    } catch (e) {
      showToast(`ML 计划失败：${e.message}`, "neg");
    }
  }

  return {
    title: "实盘",
    subtitle: "动量 + ML",
    pollMs: 25_000,
    async mount(el) {
      root = el;
      await load();
    },
    async refresh() {
      if (subtab !== "settings") await load();
    },
    unmount() {
      root = null;
    },
  };
}
