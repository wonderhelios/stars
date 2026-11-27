import { apiGet, apiPost, showToast } from "../core/api.js";
import { esc, filterRange, fmtBj, fmtNum, fmtPct, fmtPx, fmtSignedUsd, fmtUsd, signCls } from "../core/fmt.js";
import { renderLineChart } from "../core/chart.js";
import { bindPager, paginate, renderPagerHtml } from "../core/pager.js";

function normalizeHistory(raw) {
  if (!Array.isArray(raw)) return [];
  return raw
    .map((p) => {
      if (Array.isArray(p)) return { ts: Number(p[0]), val: Number(p[1]) };
      return { ts: Number(p.ts ?? p.t), val: Number(p.equity ?? p.val ?? p.v) };
    })
    .filter((p) => Number.isFinite(p.ts) && Number.isFinite(p.val));
}

export function createArchiveView(kind, label) {
  let root = null;
  let data = null;
  let range = "all";
  let pgList = 1;
  let extraOut = "";

  const endpoint = {
    hlcarry: "/api/hlcarry",
    intraday: "/api/intraday",
    tradfi: "/api/tradfi",
    fastswing: "/api/fastswing",
  }[kind];

  async function load() {
    try {
      data = await apiGet(endpoint);
      const foot = document.getElementById("foot-updated");
      if (foot) foot.textContent = fmtBj(Date.now());
      render();
    } catch (e) {
      if (root) root.innerHTML = `<div class="banner warn">归档模块「${esc(label)}」未运行或不可用：${esc(e.message)}</div>`;
    }
  }

  function render() {
    if (!root || !data) return;
    if (kind === "hlcarry") renderHlCarry(root, data);
    else renderGenericShadow(root, data);
  }

  function renderHlCarry(el, d) {
    const st = d.state || {};
    const legs = st.legs || [];
    const logsPage = paginate((st.log || []).slice().reverse(), pgList, 8);
    pgList = logsPage.page;
    const startEq = Number(st.start_equity) || 3000;
    const eq = Number(d.equity) || startEq;
    const pnl = eq - startEq;
    const pnlPct = startEq > 0 ? (pnl / startEq) * 100 : 0;

    el.innerHTML = `
      <div class="banner info">
        <span>基差影子</span>
        <div style="display:flex;gap:8px">
          <button type="button" class="btn sm" id="hc-step">手动步进</button>
          <button type="button" class="btn warn sm" id="hc-open">重置建仓 ($3,000)</button>
        </div>
      </div>

      <div class="grid-4">
        <div class="kpi"><div class="kpi-top"><span>模拟净值</span><small>初始 ${fmtUsd(startEq, 0)}</small></div><div class="kpi-val">${fmtUsd(eq)}</div><div class="kpi-sub"><span class="${signCls(pnl)}">${fmtSignedUsd(pnl)} (${fmtPct(pnlPct, 2, true)})</span></div></div>
        <div class="kpi"><div class="kpi-top"><span>年化收益</span></div><div class="kpi-val ${signCls(d.annualized_pct)}">${fmtPct(d.annualized_pct, 2, true)}</div><div class="kpi-sub"><span>资金费 ${fmtSignedUsd(d.funding_total)}</span><span>基差 ${fmtSignedUsd(d.basis_total)}</span></div></div>
        <div class="kpi"><div class="kpi-top"><span>活跃套利腿</span></div><div class="kpi-val">${legs.length}</div><div class="kpi-sub"><span>最近检查 ${fmtBj(st.last_check_ms)}</span></div></div>
        <div class="kpi"><div class="kpi-top"><span>告警</span></div><div class="kpi-val">${(d.warnings || []).length}</div><div class="kpi-sub"><span>${esc((d.warnings || [])[0] || "无告警")}</span></div></div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title-group"><span class="eyebrow">CARRY EQUITY</span><h2 class="card-title">基差影子净值曲线</h2></div>
          <div class="seg" id="ar-range">
            <button type="button" data-r="24h" class="${range === "24h" ? "active" : ""}">24H</button>
            <button type="button" data-r="7d" class="${range === "7d" ? "active" : ""}">7D</button>
            <button type="button" data-r="30d" class="${range === "30d" ? "active" : ""}">30D</button>
            <button type="button" data-r="all" class="${range === "all" ? "active" : ""}">ALL</button>
          </div>
        </div>
        <div class="chart-box sm" id="ar-chart"></div>
      </div>

      <div class="grid-2">
        <div class="card">
          <div class="card-head"><div class="card-title-group"><span class="eyebrow">ACTIVE LEGS</span><h2 class="card-title">当前持仓腿（${legs.length}）</h2></div></div>
          ${legs.length ? `
            <div class="table-wrap"><table class="tbl">
              <thead><tr><th>标的</th><th class="num">名义</th><th class="num">年化费率</th><th class="num">基差(bp)</th><th class="num">累计资金费</th><th class="num">腿盈亏</th></tr></thead>
              <tbody>
                ${legs.map((l) => `
                  <tr>
                    <td><b>${esc(l.coin)}</b> <span class="muted">${esc(l.spot || "")}</span></td>
                    <td class="num">${fmtUsd(l.notional, 0)}</td>
                    <td class="num">${fmtPct(l.funding_ann, 1)}</td>
                    <td class="num">${fmtNum(l.basis_bp, 1)}</td>
                    <td class="num ${signCls(l.funding_accrued)}">${fmtSignedUsd(l.funding_accrued)}</td>
                    <td class="num ${signCls(l.leg_pnl)}"><b>${fmtSignedUsd(l.leg_pnl)}</b></td>
                  </tr>
                `).join("")}
              </tbody>
            </table></div>
          ` : `<div class="empty">当前空仓等待满足资金费/基差门槛的标的。</div>`}
        </div>
        <div class="card">
          <div class="card-head"><div class="card-title-group"><span class="eyebrow">EVENT LOG</span><h2 class="card-title">最近事件</h2></div></div>
          ${logsPage.rows.length ? `
            <div class="feed">
              ${logsPage.rows.map((x) => `<div class="feed-item"><span class="feed-ts">${fmtBj(x.ts)}</span><span class="pill">LOG</span><span>${esc(x.msg)}</span></div>`).join("")}
            </div>
            ${renderPagerHtml("ar-pg-log", logsPage.page, logsPage.totalPages, logsPage.total)}
          ` : `<div class="empty">暂无日志。</div>`}
        </div>
      </div>
    `;

    bindPager(el, "ar-pg-log", (p) => { pgList = p; render(); });

    el.querySelectorAll("#ar-range button").forEach((b) => {
      b.addEventListener("click", () => {
        range = b.dataset.r;
        render();
      });
    });
    document.getElementById("hc-step")?.addEventListener("click", async () => {
      try {
        await apiPost("/api/hlcarry/step", {});
        showToast("基差影子已步进", "pos");
        await load();
      } catch (e) {
        showToast(`步进失败：${e.message}`, "neg");
      }
    });
    document.getElementById("hc-open")?.addEventListener("click", async () => {
      if (!confirm("以 $3,000 重置基差影子账本？")) return;
      try {
        await apiPost("/api/hlcarry/open", { capital: 3000 });
        showToast("基差影子已重置", "pos");
        await load();
      } catch (e) {
        showToast(`重置失败：${e.message}`, "neg");
      }
    });

    const pts = filterRange(normalizeHistory(st.history), range);
    renderLineChart(document.getElementById("ar-chart"), pts, {
      zeroLine: startEq,
      formatY: (v) => `$${Math.round(v).toLocaleString("en-US")}`,
      formatTip: (v) => fmtUsd(v),
    });
  }

  function renderGenericShadow(el, d) {
    const cap = Number(d.capital) || Number(d.start_equity) || 0;
    const eq = Number(d.equity) || Number(d.balance) || cap;
    const pnl = eq - cap;
    const pnlPct = cap > 0 ? (pnl / cap) * 100 : 0;
    const posObj = d.positions || {};
    const posList = Array.isArray(posObj) ? posObj : Object.entries(posObj).map(([sym, v]) => ({ symbol: sym, ...v }));
    const tradesPage = paginate((d.trades || []).slice().reverse(), pgList, 8);
    pgList = tradesPage.page;

    el.innerHTML = `
      ${kind === "tradfi" ? `
        <div class="banner info">
          <span>${esc(label)}</span>
          <div style="display:flex;gap:6px;flex-wrap:wrap">
            <button type="button" class="btn sm" id="tf-rebal">影子调仓</button>
            <button type="button" class="btn sm" id="tf-plan">计划预览</button>
            <button type="button" class="btn danger sm" id="tf-exec">执行实盘</button>
            <button type="button" class="btn danger sm" id="tf-flat">一键平仓</button>
          </div>
        </div>
      ` : ""}
      ${d.last_error ? `<div class="banner neg">${esc(d.last_error)}</div>` : ""}

      <div class="grid-4">
        <div class="kpi"><div class="kpi-top"><span>当前净值</span><small>本金 ${fmtUsd(cap, 0)}</small></div><div class="kpi-val">${fmtUsd(eq)}</div><div class="kpi-sub"><span class="${signCls(pnl)}">${fmtSignedUsd(pnl)} (${fmtPct(pnlPct, 2, true)})</span></div></div>
        <div class="kpi"><div class="kpi-top"><span>活跃持仓</span></div><div class="kpi-val">${posList.length}</div><div class="kpi-sub"><span>总名义 ${fmtUsd(d.gross_notional || 0, 0)}</span></div></div>
        <div class="kpi"><div class="kpi-top"><span>已平仓笔数</span></div><div class="kpi-val">${(d.trades || []).length}</div><div class="kpi-sub"><span>运行 ${fmtNum(d.days_elapsed || 0, 1)} 天</span></div></div>
        <div class="kpi"><div class="kpi-top"><span>最近更新</span></div><div class="kpi-val" style="font-size:15px">${fmtBj(d.last_update || d.updated_at || d.last_scan_ts)}</div><div class="kpi-sub"><span>${d.breaker_tripped ? "熔断中" : "正常"}</span></div></div>
      </div>

      ${extraOut ? `<pre class="code-box">${esc(extraOut)}</pre>` : ""}

      <div class="card">
        <div class="card-head">
          <div class="card-title-group"><span class="eyebrow">PERFORMANCE</span><h2 class="card-title">${esc(label)} 净值曲线</h2></div>
          <div class="seg" id="ar-range">
            <button type="button" data-r="24h" class="${range === "24h" ? "active" : ""}">24H</button>
            <button type="button" data-r="7d" class="${range === "7d" ? "active" : ""}">7D</button>
            <button type="button" data-r="30d" class="${range === "30d" ? "active" : ""}">30D</button>
            <button type="button" data-r="all" class="${range === "all" ? "active" : ""}">ALL</button>
          </div>
        </div>
        <div class="chart-box sm" id="ar-chart"></div>
      </div>

      <div class="grid-2">
        <div class="card">
          <div class="card-head"><div class="card-title-group"><span class="eyebrow">POSITIONS</span><h2 class="card-title">当前持仓（${posList.length}）</h2></div></div>
          ${posList.length ? `
            <div class="table-wrap"><table class="tbl">
              <thead><tr><th>标的</th><th>方向</th><th class="num">入场</th><th class="num">现价</th><th class="num">名义</th><th class="num">浮盈</th></tr></thead>
              <tbody>
                ${posList.map((p) => {
                  const side = p.side === 1 || p.side === "long" || p.side === "LONG" ? "LONG" : "SHORT";
                  const up = Number(p.unreal_pnl ?? p.unrealized_pnl ?? p.pnl ?? 0);
                  return `
                    <tr>
                      <td><b>${esc(p.symbol || p.coin)}</b></td>
                      <td><span class="pill ${side === "LONG" ? "pos" : "neg"}">${side}</span></td>
                      <td class="num">${fmtPx(p.entry_px ?? p.entry_price ?? p.entry)}</td>
                      <td class="num">${fmtPx(p.mark_px ?? p.mark_price ?? p.price)}</td>
                      <td class="num">${fmtUsd(p.notional || 0, 0)}</td>
                      <td class="num ${signCls(up)}">${fmtSignedUsd(up)}</td>
                    </tr>
                  `;
                }).join("")}
              </tbody>
            </table></div>
          ` : `<div class="empty">当前空仓。</div>`}
        </div>

        <div class="card">
          <div class="card-head"><div class="card-title-group"><span class="eyebrow">TRADES</span><h2 class="card-title">平仓记录</h2></div></div>
          ${tradesPage.rows.length ? `
            <div class="table-wrap"><table class="tbl">
              <thead><tr><th>时间</th><th>标的</th><th class="num">净盈亏</th><th>原因</th></tr></thead>
              <tbody>
                ${tradesPage.rows.map((t) => {
                  const pnlV = Number(t.net_pnl ?? t.pnl ?? 0);
                  return `
                    <tr>
                      <td class="mono muted">${fmtBj(t.exit_ts ?? t.ts ?? t.close_ts)}</td>
                      <td><b>${esc(t.symbol || t.coin)}</b></td>
                      <td class="num ${signCls(pnlV)}"><b>${fmtSignedUsd(pnlV)}</b></td>
                      <td class="muted">${esc(t.reason || t.exit_reason || "—")}</td>
                    </tr>
                  `;
                }).join("")}
              </tbody>
            </table></div>
            ${renderPagerHtml("ar-pg-tr", tradesPage.page, tradesPage.totalPages, tradesPage.total)}
          ` : `<div class="empty">暂无平仓记录。</div>`}
        </div>
      </div>
    `;

    bindPager(el, "ar-pg-tr", (p) => { pgList = p; render(); });

    el.querySelectorAll("#ar-range button").forEach((b) => {
      b.addEventListener("click", () => {
        range = b.dataset.r;
        render();
      });
    });

    if (kind === "tradfi") {
      document.getElementById("tf-rebal")?.addEventListener("click", async () => {
        try {
          await apiPost("/api/tradfi/shadow/rebalance", {});
          showToast("TradFi 影子已调仓", "pos");
          await load();
        } catch (e) { showToast(e.message, "neg"); }
      });
      document.getElementById("tf-plan")?.addEventListener("click", async () => {
        try {
          const res = await apiPost("/api/tradfi/plan", {});
          extraOut = JSON.stringify(res, null, 2);
          render();
        } catch (e) { showToast(e.message, "neg"); }
      });
      document.getElementById("tf-exec")?.addEventListener("click", async () => {
        if (!confirm("确认执行 TradFi 实盘调仓？")) return;
        try {
          const res = await apiPost("/api/tradfi/execute", {});
          extraOut = JSON.stringify(res, null, 2);
          showToast("TradFi 调仓已执行", "pos");
          await load();
        } catch (e) { showToast(e.message, "neg"); }
      });
      document.getElementById("tf-flat")?.addEventListener("click", async () => {
        if (!confirm("确认一键平掉所有 TradFi 仓位？")) return;
        try {
          const res = await apiPost("/api/tradfi/flatten", {});
          extraOut = JSON.stringify(res, null, 2);
          showToast("TradFi 已发送全平", "warn");
          await load();
        } catch (e) { showToast(e.message, "neg"); }
      });
    }

    const pts = filterRange(normalizeHistory(d.history), range);
    renderLineChart(document.getElementById("ar-chart"), pts, {
      zeroLine: cap || undefined,
      formatY: (v) => `$${Math.round(v).toLocaleString("en-US")}`,
      formatTip: (v) => fmtUsd(v),
    });
  }

  return {
    title: label,
    subtitle: "",
    pollMs: 60_000,
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
