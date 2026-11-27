import { apiGet, apiPost, showToast } from "../core/api.js";
import { esc, filterRange, fmtBj, fmtNum, fmtPct, fmtPx, fmtSignedUsd, fmtUsd, signCls } from "../core/fmt.js";
import { renderLineChart } from "../core/chart.js";

export function createLimitOcoView() {
  let root = null;
  let data = null;
  let subtab = "monitor"; // monitor | settings
  let range = "all";
  let tradeFilter = "all";
  let cdTimer = null;
  let busy = false;

  async function load() {
    data = await apiGet("/api/limitoco");
    const foot = document.getElementById("foot-updated");
    if (foot) foot.textContent = fmtBj(Date.now());
    const dot = document.getElementById("dot-limitoco");
    if (dot) dot.className = `nav-dot ${data.last_error ? "err" : data.running ? "live" : "warn"}`;
    render();
  }

  function updateCountdown() {
    const el = document.getElementById("lo-countdown");
    if (!el) return;
    const rem = Math.max(0, Math.floor((Math.ceil(Date.now() / 3_600_000) * 3_600_000 - Date.now()) / 1000));
    const mm = String(Math.floor(rem / 60)).padStart(2, "0");
    const ss = String(rem % 60).padStart(2, "0");
    el.textContent = `下根1h收盘 ${mm}:${ss}`;
  }

  function render() {
    if (!root || !data) return;
    const d = data;
    const stCls = d.last_error ? "neg" : d.running ? "pos" : "warn";

    root.innerHTML = `
      <div style="display:flex;align-items:center;justify-content:space-between;gap:10px;flex-wrap:wrap">
        <div class="subtabs" id="lo-subtabs">
          <button type="button" data-t="monitor" class="${subtab === "monitor" ? "active" : ""}">实时观测台</button>
          <button type="button" data-t="settings" class="${subtab === "settings" ? "active" : ""}">策略设置与启停</button>
        </div>
        <div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap">
          <span class="pill ${stCls}"><span class="pill-dot"></span>${esc(d.status_text || (d.running ? "运行中" : "已暂停"))}</span>
          <span class="pill mono" id="lo-countdown">下根1h收盘 --:--</span>
          <button type="button" class="btn sm" id="lo-top-step">立即扫描</button>
        </div>
      </div>
      ${d.last_error ? `<div class="banner neg">${esc(d.last_error)}</div>` : ""}
      <div id="lo-body" style="display:flex;flex-direction:column;gap:12px"></div>
    `;

    updateCountdown();
    root.querySelectorAll("#lo-subtabs button").forEach((b) => {
      b.addEventListener("click", () => {
        subtab = b.dataset.t;
        render();
      });
    });
    document.getElementById("lo-top-step")?.addEventListener("click", () => runAction("/api/limitoco/step", {}, "已完成一次行情扫描与盯市"));

    const body = document.getElementById("lo-body");
    if (subtab === "monitor") renderMonitor(body, d);
    else renderSettings(body, d);
  }

  function renderMonitor(body, d) {
    const cfg = d.config || {};
    const btc = d.btc_regime || {};
    const posList = Object.values(d.positions || {});
    const pendList = Object.values(d.pending || {});
    const a = d.sleeve_stats?.A || { open: 0, max: 3, pending: 0, trades: 0, win_rate: 0, sum_r: 0, pf: 0 };
    const b = d.sleeve_stats?.B || { open: 0, max: 2, pending: 0, trades: 0, win_rate: 0, sum_r: 0, pf: 0 };
    const uni = d.universe || {};
    const fn = d.funnel || {};
    const dailyR = Number(d.daily_r_pnl) || 0;
    const brkLim = Number(cfg.daily_loss_limit_r) || 3;

    let trades = (d.trades || []).slice().reverse();
    if (tradeFilter !== "all") trades = trades.filter((t) => t.sleeve === tradeFilter);
    const events = (d.events || []).slice().reverse().slice(0, 35);

    body.innerHTML = `
      <div class="grid-4">
        <div class="kpi">
          <div class="kpi-top"><span>影子净值 (${esc(cfg.risk_tier || "1.5x")})</span><small>${fmtNum((cfg.risk_per_trade || 0.012) * 100, 1)}%R</small></div>
          <div class="kpi-val">${fmtUsd(d.equity)}</div>
          <div class="kpi-sub"><span class="${signCls(d.pnl)}">${fmtSignedUsd(d.pnl)} (${fmtPct(d.pnl_pct, 2, true)})</span><span>本金 ${fmtUsd(d.capital, 0)}</span></div>
        </div>
        <div class="kpi">
          <div class="kpi-top"><span>胜率 &amp; 盈亏比</span><small>${(d.trades || []).length} 笔已平</small></div>
          <div class="kpi-val">${fmtPct(d.win_rate, 1)}</div>
          <div class="kpi-sub"><span>PF ${fmtNum(d.profit_factor, 2)}</span><span class="${signCls(d.total_r)}">累计 ${d.total_r >= 0 ? "+" : ""}${fmtNum(d.total_r, 2)}R</span></div>
        </div>
        <div class="kpi">
          <div class="kpi-top"><span>仓位 &amp; 杠杆</span><small>${posList.length}/${cfg.max_total_positions || 5} 仓 · ${pendList.length} 挂单</small></div>
          <div class="kpi-val">${fmtNum(d.gross_leverage, 2)}x</div>
          <div class="kpi-sub"><span>名义 ${fmtUsd(d.gross_notional, 0)}</span><span>费 ${fmtUsd(d.total_fees, 2)} · 资 ${fmtSignedUsd(d.total_funding, 2)}</span></div>
        </div>
        <div class="kpi">
          <div class="kpi-top"><span>回撤 &amp; BTC 罗盘</span><small>UTC ${esc(d.daily_utc_day || "—")}</small></div>
          <div class="kpi-val">${btc.price ? fmtUsd(btc.price, 0) : "待扫描"}</div>
          <div class="kpi-sub"><span class="${btc.bull_ok ? "pos" : btc.short_ok ? "neg" : "warn"}">${esc(btc.label || "等待扫描")}</span><span>今日 ${dailyR >= 0 ? "+" : ""}${fmtNum(dailyR, 2)}R / -${fmtNum(brkLim, 1)}R</span></div>
        </div>
      </div>

      <div class="grid-main-side">
        <div class="card">
          <div class="card-head">
            <div class="card-title-group">
              <span class="eyebrow">EQUITY &amp; LEVERAGE RIBBON · V2 ULTRA-WR</span>
              <h2 class="card-title">实时净值与杠杆双层曲线</h2>
            </div>
            <div class="seg" id="lo-range">
              <button type="button" data-r="24h" class="${range === "24h" ? "active" : ""}">24H</button>
              <button type="button" data-r="7d" class="${range === "7d" ? "active" : ""}">7D</button>
              <button type="button" data-r="30d" class="${range === "30d" ? "active" : ""}">30D</button>
              <button type="button" data-r="all" class="${range === "all" ? "active" : ""}">ALL</button>
            </div>
          </div>
          <div class="chart-box" id="lo-chart"></div>
        </div>

        <div class="card" style="display:flex;flex-direction:column;gap:10px">
          <div class="card-head" style="margin-bottom:0">
            <div class="card-title-group">
              <span class="eyebrow">DUAL-SLEEVE TELEMETRY</span>
              <h2 class="card-title">A / B 双引擎归因</h2>
            </div>
          </div>
          <div class="sleeve-card">
            <div style="display:flex;justify-content:space-between;align-items:center">
              <span class="pill warn">SLEEVE A · 7d 趋势突破</span>
              <span class="mono muted" style="font-size:11px">挂单 ${a.pending} 笔</span>
            </div>
            <div class="muted" style="font-size:11.5px">168h 放量突破 · BTC &gt; EMA336 做多护栏 · 止损 2.8ATR / 止盈 1.2R</div>
            <div class="sleeve-metrics">
              <div class="sleeve-metric"><span>持仓</span><strong>${a.open}/${a.max}</strong></div>
              <div class="sleeve-metric"><span>净 R</span><strong class="${signCls(a.sum_r)}">${a.sum_r >= 0 ? "+" : ""}${fmtNum(a.sum_r, 2)}R</strong></div>
              <div class="sleeve-metric"><span>胜率</span><strong>${fmtPct(a.win_rate, 1)}</strong></div>
              <div class="sleeve-metric"><span>PF</span><strong>${fmtNum(a.pf, 2)}</strong></div>
            </div>
          </div>
          <div class="sleeve-card">
            <div style="display:flex;justify-content:space-between;align-items:center">
              <span class="pill info">SLEEVE B · 3d 反弹急拉做空</span>
              <span class="mono muted" style="font-size:11px">挂单 ${b.pending} 笔</span>
            </div>
            <div class="muted" style="font-size:11.5px">空头 (&lt; EMA336) 72h +6% 急拉 · 日RSI2≥90 · 止损 2.7ATR / 止盈 0.9R</div>
            <div class="sleeve-metrics">
              <div class="sleeve-metric"><span>持仓</span><strong>${b.open}/${b.max}</strong></div>
              <div class="sleeve-metric"><span>净 R</span><strong class="${signCls(b.sum_r)}">${b.sum_r >= 0 ? "+" : ""}${fmtNum(b.sum_r, 2)}R</strong></div>
              <div class="sleeve-metric"><span>胜率</span><strong>${fmtPct(b.win_rate, 1)}</strong></div>
              <div class="sleeve-metric"><span>PF</span><strong>${fmtNum(b.pf, 2)}</strong></div>
            </div>
          </div>
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title-group">
            <span class="eyebrow">ACTIVE POSITIONS · OCO RAILS</span>
            <h2 class="card-title">当前持仓与止盈止损轨道（${posList.length} / ${cfg.max_total_positions || 5}）</h2>
          </div>
        </div>
        ${posList.length ? `
          <div class="oco-grid">
            ${posList.map((p) => {
              const prog = Math.max(-100, Math.min(100, Number(p.progress_pct) || 0));
              const markerPct = 50 + prog * 0.5;
              const fillLeft = prog >= 0 ? 50 : markerPct;
              const fillW = Math.abs(prog) * 0.5;
              const col = prog >= 0 ? "var(--pos)" : "var(--neg)";
              return `
                <div class="oco-card">
                  <div style="display:flex;justify-content:space-between;align-items:center">
                    <div style="display:flex;align-items:center;gap:6px">
                      <span class="pill ${p.sleeve === "A" ? "warn" : "info"}">${esc(p.sleeve)}</span>
                      <b>${esc(p.symbol)}</b>
                      <span class="pill ${p.side === 1 ? "pos" : "neg"}">${p.side === 1 ? "LONG" : "SHORT"}</span>
                    </div>
                    <div class="mono" style="text-align:right">
                      <div class="${signCls(p.unreal_pnl)}" style="font-weight:700">${fmtSignedUsd(p.unreal_pnl)}</div>
                      <div class="muted" style="font-size:10.5px">${p.unreal_r >= 0 ? "+" : ""}${fmtNum(p.unreal_r, 2)}R · ${p.hold_hours}h/${p.max_hold_hours}h</div>
                    </div>
                  </div>
                  <div class="oco-rail">
                    <div class="oco-center"></div>
                    <div class="oco-fill" style="left:${fillLeft.toFixed(1)}%;width:${fillW.toFixed(1)}%;background:${col}"></div>
                    <div class="oco-marker" style="left:${markerPct.toFixed(1)}%;background:${col}"></div>
                  </div>
                  <div class="oco-labels">
                    <span class="neg">SL ${fmtPx(p.sl_px)} (-${fmtNum(p.sl_pct, 1)}%)</span>
                    <span>${fmtPx(p.entry_px)} → ${fmtPx(p.mark_px)}</span>
                    <span class="pos">TP ${fmtPx(p.tp_px)} (+${fmtNum(p.tp_pct, 1)}%)</span>
                  </div>
                </div>
              `;
            }).join("")}
          </div>
        ` : `<div class="empty">当前空仓等待中。每整点收盘扫描 Top-${cfg.top_n_liquid || 80} 并自动挂纯 Maker GTX 单。</div>`}
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title-group">
            <span class="eyebrow">RESTING MAKER QUEUE · GTX POST-ONLY</span>
            <h2 class="card-title">待成交限价挂单（${pendList.length} 笔 · 12h 有效）</h2>
          </div>
        </div>
        ${pendList.length ? `
          <div class="table-wrap">
            <table class="tbl">
              <thead><tr><th>策略 / 标的</th><th>方向</th><th class="num">GTX 挂单价</th><th class="num">现价</th><th class="num">距成交</th><th class="num">SL</th><th class="num">TP</th><th>有效期</th><th>依据</th></tr></thead>
              <tbody>
                ${pendList.map((o) => `
                  <tr>
                    <td><span class="pill ${o.sleeve === "A" ? "warn" : "info"}">${esc(o.sleeve)}</span> <b>${esc(o.symbol)}</b></td>
                    <td><span class="pill ${o.side === 1 ? "pos" : "neg"}">${o.side === 1 ? "做多" : "做空"}</span></td>
                    <td class="num info">${fmtPx(o.limit_px)}</td>
                    <td class="num">${fmtPx(o.mark_px)}</td>
                    <td class="num warn">${fmtPct(o.dist_to_fill_pct, 2)}</td>
                    <td class="num neg">${fmtPx(o.sl_px)} (-${fmtNum(o.sl_pct, 1)}%)</td>
                    <td class="num pos">${fmtPx(o.tp_px)} (+${fmtNum(o.tp_pct, 1)}%)</td>
                    <td class="mono muted">${o.age_hours}h / ${o.expire_hours}h</td>
                    <td class="muted">${esc(o.reason)}</td>
                  </tr>
                `).join("")}
              </tbody>
            </table>
          </div>
        ` : `<div class="empty">暂无待成交 GTX 挂单。</div>`}
      </div>

      <div class="grid-2">
        <div class="card" style="display:flex;flex-direction:column;gap:10px">
          <div class="card-head" style="margin-bottom:0">
            <div class="card-title-group">
              <span class="eyebrow">UNIVERSE &amp; SIGNAL FUNNEL</span>
              <h2 class="card-title">Top-${uni.liquid_count || 80} 流动性池与信号漏斗</h2>
            </div>
            <span class="mono muted" style="font-size:11px">全市场 ${uni.total_perps || 0} · 剔除美股 ${uni.excluded_stocks || 0} · ${d.scan_duration_ms || 0}ms</span>
          </div>
          <div class="funnel-grid">
            <div class="funnel-box"><span>扫描币×时</span><strong>${fn.bars_scanned || 0}</strong></div>
            <div class="funnel-box"><span>A 突破/挂单</span><strong>${fn.a_raw_cross || 0}/${fn.a_placed || 0}</strong></div>
            <div class="funnel-box"><span>B 急拉/挂单</span><strong>${fn.b_raw_rip || 0}/${fn.b_placed || 0}</strong></div>
            <div class="funnel-box"><span>成交/过期</span><strong>${fn.filled || 0}/${(fn.expired || 0) + (fn.runaway || 0)}</strong></div>
            <div class="funnel-box"><span>BTC 拦截</span><strong>${fn.a_btc_blocked || 0}</strong></div>
            <div class="funnel-box"><span>费率过滤</span><strong>${(fn.a_fund_blocked || 0) + (fn.b_fund_blocked || 0)}</strong></div>
            <div class="funnel-box"><span>SL&gt;14%</span><strong>${(fn.a_sl_rejected || 0) + (fn.b_sl_rejected || 0)}</strong></div>
            <div class="funnel-box"><span>冷却中</span><strong>${Object.keys(d.cooldowns || {}).length}</strong></div>
          </div>
          <div class="chip-strip">
            ${(uni.top80_preview || []).slice(0, 20).map((r) => `
              <span class="chip ${r.is_commodity ? "comm" : ""}">
                <b>${esc(r.symbol.replace("USDT", ""))}</b>
                <span class="${signCls(r.ret24h)}">${fmtPct(r.ret24h, 1, true)}</span>
              </span>
            `).join("")}
          </div>
        </div>

        <div class="card">
          <div class="card-head">
            <div class="card-title-group">
              <span class="eyebrow">ACTIVITY STREAM</span>
              <h2 class="card-title">引擎实时日志</h2>
            </div>
          </div>
          ${events.length ? `
            <div class="feed">
              ${events.map((e) => `
                <div class="feed-item">
                  <span class="feed-ts">${fmtBj(e.ts)}</span>
                  <span><span class="pill ${e.kind === "TP" || e.kind === "FILL" ? "pos" : e.kind === "SL" ? "neg" : e.kind === "SIGNAL" ? "info" : ""}">${esc(e.kind)}</span></span>
                  <span>${esc(e.msg)}</span>
                </div>
              `).join("")}
            </div>
          ` : `<div class="empty">尚无运行事件。</div>`}
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title-group">
            <span class="eyebrow">REALIZED LEDGER</span>
            <h2 class="card-title">已平仓成交记录（${trades.length} 笔）</h2>
          </div>
          <div class="seg" id="lo-trade-filter">
            <button type="button" data-f="all" class="${tradeFilter === "all" ? "active" : ""}">全部</button>
            <button type="button" data-f="A" class="${tradeFilter === "A" ? "active" : ""}">策略 A</button>
            <button type="button" data-f="B" class="${tradeFilter === "B" ? "active" : ""}">策略 B</button>
          </div>
        </div>
        ${trades.length ? `
          <div class="table-wrap">
            <table class="tbl">
              <thead><tr><th>平仓时间</th><th>策略 / 标的</th><th>方向</th><th class="num">入场 → 平仓</th><th class="num">SL / TP</th><th class="num">持仓</th><th class="num">净盈亏</th><th class="num">R 倍数</th><th>退出</th></tr></thead>
              <tbody>
                ${trades.slice(0, 60).map((t) => `
                  <tr>
                    <td class="mono muted">${fmtBj(t.exit_ts)}</td>
                    <td><span class="pill ${t.sleeve === "A" ? "warn" : "info"}">${esc(t.sleeve)}</span> <b>${esc(t.symbol)}</b></td>
                    <td><span class="pill ${t.side === 1 ? "pos" : "neg"}">${t.side === 1 ? "多" : "空"}</span></td>
                    <td class="num">${fmtPx(t.entry_px)} → ${fmtPx(t.exit_px)}</td>
                    <td class="num muted">-${fmtNum(t.sl_pct, 1)}% / +${fmtNum(t.tp_pct, 1)}%</td>
                    <td class="num">${t.hold_hours}h</td>
                    <td class="num ${signCls(t.net_pnl)}"><b>${fmtSignedUsd(t.net_pnl)}</b></td>
                    <td class="num ${signCls(t.r_mult)}">${t.r_mult >= 0 ? "+" : ""}${fmtNum(t.r_mult, 2)}R</td>
                    <td><span class="pill ${t.reason === "TP" ? "pos" : t.reason === "SL" ? "neg" : ""}">${esc(t.reason)}</span></td>
                  </tr>
                `).join("")}
              </tbody>
            </table>
          </div>
        ` : `<div class="empty">暂无已平仓落袋成交。</div>`}
      </div>
    `;

    body.querySelectorAll("#lo-range button").forEach((b) => {
      b.addEventListener("click", () => {
        range = b.dataset.r;
        renderMonitor(body, d);
      });
    });
    body.querySelectorAll("#lo-trade-filter button").forEach((b) => {
      b.addEventListener("click", () => {
        tradeFilter = b.dataset.f;
        renderMonitor(body, d);
      });
    });

    const pts = filterRange((d.history || []).map(([ts, eq, lev]) => ({ ts, val: eq, val2: lev })), range);
    renderLineChart(document.getElementById("lo-chart"), pts, {
      zeroLine: Number(d.capital) || 10000,
      secondary: true,
      secondaryLabel: "总杠杆",
      formatY: (v) => `$${Math.round(v).toLocaleString("en-US")}`,
      formatTip: (v) => fmtUsd(v),
    });
  }

  function renderSettings(body, d) {
    const cfg = d.config || {};
    const presets = d.tier_presets || [];
    const activeTier = cfg.risk_tier || "1.5x";

    body.innerHTML = `
      <div class="card" style="display:flex;align-items:center;justify-content:space-between;gap:12px;flex-wrap:wrap">
        <div>
          <span class="eyebrow">ENGINE CONTROL</span>
          <h2 class="card-title">纯 Rust 影子引擎启停</h2>
          <p class="card-sub">启动后后台每 60s 盯市、每整点扫描 Top-80 已收盘 1h K 线并模拟 GTX 挂单与 OCO 止盈止损。</p>
        </div>
        <div style="display:flex;gap:8px;flex-wrap:wrap">
          ${d.running
            ? `<button type="button" class="btn danger" id="lo-btn-stop">⏸ 暂停引擎</button>`
            : `<button type="button" class="btn pos" id="lo-btn-start">▶ 启动引擎</button>`}
          <button type="button" class="btn" id="lo-btn-step">立即扫描一步</button>
          <button type="button" class="btn warn" id="lo-btn-reset">按新本金重置账本</button>
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title-group">
            <span class="eyebrow">RISK TIERS · 35M CROSS-CYCLE CALIBRATED (V2 ULTRA-WR)</span>
            <h2 class="card-title">风险杠杆档位（52.4% 胜率 · 12h 中位持仓 · 3 年最大连亏 8 笔）</h2>
          </div>
          <input type="hidden" id="lo-f-tier" value="${esc(activeTier)}" />
        </div>
        <div class="tier-grid" id="lo-tiers">
          ${presets.map((p) => `
            <button type="button" class="tier-card ${activeTier === p.id ? "active" : ""}" data-tier="${esc(p.id)}">
              <div style="display:flex;justify-content:space-between;align-items:center">
                <b>${esc(p.label)}</b>
                ${p.id === "1.5x" ? '<span class="pill warn">推荐</span>' : ""}
              </div>
              <div class="mono" style="font-size:15px;font-weight:700">${(p.risk_per_trade * 100).toFixed(1)}%R <span class="muted" style="font-size:11px;font-weight:400">/ ${p.max_pos_lev}x · 总 ${p.max_total_gross_lev}x</span></div>
              <div class="tier-stats">
                <span>35月 <b>+${p.bt_ret_35m_pct ?? p.bt_ret_23m_pct}%</b></span>
                <span>年化 <b>+${p.bt_ann_pct}%</b></span>
                <span>回撤 <b>-${p.bt_max_dd_pct}%</b></span>
                <span>夏普 <b>${p.bt_sharpe}</b></span>
              </div>
            </button>
          `).join("")}
        </div>
        <div class="sim-strip" id="lo-sim"></div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title-group">
            <span class="eyebrow">PRECISION PARAMETERS</span>
            <h2 class="card-title">策略参数配置</h2>
          </div>
          <div style="display:flex;gap:8px">
            <button type="button" class="btn ghost sm" id="lo-btn-def">恢复 1.5x 默认</button>
            <button type="button" class="btn primary sm" id="lo-btn-save">保存配置</button>
          </div>
        </div>
        <div class="form-grid" style="margin-bottom:12px">
          <div class="field"><label>初始本金 (USDT)</label><input id="lo-f-cap" type="number" step="500" min="100" value="${cfg.capital ?? 10000}" /></div>
          <div class="field"><label>单笔风险 1R (%)</label><input id="lo-f-rpt" type="number" step="0.1" min="0.2" max="5" value="${((cfg.risk_per_trade ?? 0.012) * 100).toFixed(2)}" /></div>
          <div class="field"><label>单仓最大杠杆 (x)</label><input id="lo-f-poslev" type="number" step="0.1" min="0.5" max="10" value="${cfg.max_pos_lev ?? 2.2}" /></div>
          <div class="field"><label>组合总杠杆上限 (x)</label><input id="lo-f-grosslev" type="number" step="0.5" min="1" max="20" value="${cfg.max_total_gross_lev ?? 6.0}" /></div>
          <div class="field"><label>最大止损上限 (%)</label><input id="lo-f-maxsl" type="number" step="1" min="4" max="25" value="${((cfg.max_sl_pct ?? 0.14) * 100).toFixed(1)}" /></div>
          <div class="field"><label>单币冷却 (h)</label><input id="lo-f-cool" type="number" step="6" min="0" max="240" value="${cfg.cooldown_hours ?? 72}" /></div>
          <div class="field"><label>UTC 日内熔断 (R)</label><input id="lo-f-brk" type="number" step="0.5" min="0" max="20" value="${cfg.daily_loss_limit_r ?? 3.0}" /></div>
          <div class="field"><label>最大并发持仓</label><input id="lo-f-maxpos" type="number" min="1" max="12" value="${cfg.max_total_positions ?? 5}" /></div>
          <div class="field"><label>Top-N 流动性池</label><input id="lo-f-topn" type="number" step="5" min="20" max="150" value="${cfg.top_n_liquid ?? 80}" /></div>
          <div class="field"><label>7日日均额门槛 ($M)</label><input id="lo-f-minvol" type="number" step="5" min="1" max="200" value="${Math.round((cfg.min_daily_vol_usd ?? 15_000_000) / 1e6)}" /></div>
          <div class="field"><label>行情代理 URL (可选)</label><input id="lo-f-proxy" type="text" value="${esc(cfg.proxy_url || "")}" placeholder="直连留空" /></div>
        </div>
        <div class="grid-3">
          <label class="check-row"><div><strong>启用复利滚存</strong><small>1R 随当前净值自动缩放</small></div><input id="lo-f-comp" type="checkbox" ${cfg.compounding !== false ? "checked" : ""} /></label>
          <label class="check-row"><div><strong>策略 A · 7d 突破</strong><small>2.8ATR / 1.2R · 最多 3 仓</small></div><input id="lo-f-ena" type="checkbox" ${cfg.enabled_a !== false ? "checked" : ""} /></label>
          <label class="check-row"><div><strong>策略 B · 3d 急拉做空</strong><small>2.7ATR / 0.9R · 最多 2 仓</small></div><input id="lo-f-enb" type="checkbox" ${cfg.enabled_b !== false ? "checked" : ""} /></label>
        </div>
      </div>
    `;

    const updateSim = () => {
      const cap = Math.max(100, Number(document.getElementById("lo-f-cap")?.value) || 10000);
      const rpt = Math.max(0.2, Number(document.getElementById("lo-f-rpt")?.value) || 1.2) / 100;
      const posLev = Math.max(0.5, Number(document.getElementById("lo-f-poslev")?.value) || 2.2);
      const grossLev = Math.max(1, Number(document.getElementById("lo-f-grosslev")?.value) || 6.0);
      const brkR = Math.max(0, Number(document.getElementById("lo-f-brk")?.value) || 3.0);
      const oneR = cap * rpt;
      const sim = document.getElementById("lo-sim");
      if (sim) {
        sim.innerHTML = `
          <div class="sim-cell"><span>单笔止损 (1R)</span><strong class="warn">${fmtUsd(oneR)}</strong></div>
          <div class="sim-cell"><span>策略A 止盈 (1.2R)</span><strong class="pos">+${fmtUsd(oneR * 1.2)}</strong></div>
          <div class="sim-cell"><span>策略B 止盈 (0.9R)</span><strong class="info">+${fmtUsd(oneR * 0.9)}</strong></div>
          <div class="sim-cell"><span>单仓 / 总名义上限</span><strong>${fmtUsd(cap * posLev, 0)} / ${fmtUsd(cap * grossLev, 0)}</strong></div>
          <div class="sim-cell"><span>日内熔断 (-${fmtNum(brkR, 1)}R)</span><strong class="neg">-${fmtUsd(oneR * brkR)}</strong></div>
        `;
      }
    };
    updateSim();

    ["lo-f-cap", "lo-f-rpt", "lo-f-poslev", "lo-f-grosslev", "lo-f-brk"].forEach((id) => {
      document.getElementById(id)?.addEventListener("input", updateSim);
    });

    body.querySelectorAll("#lo-tiers [data-tier]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const p = presets.find((x) => x.id === btn.dataset.tier);
        if (!p) return;
        document.getElementById("lo-f-tier").value = p.id;
        document.getElementById("lo-f-rpt").value = (p.risk_per_trade * 100).toFixed(2);
        document.getElementById("lo-f-poslev").value = p.max_pos_lev;
        document.getElementById("lo-f-grosslev").value = p.max_total_gross_lev;
        body.querySelectorAll("#lo-tiers .tier-card").forEach((c) => c.classList.toggle("active", c === btn));
        updateSim();
      });
    });

    document.getElementById("lo-btn-start")?.addEventListener("click", () => runAction("/api/limitoco/start", {}, "限价 V2 引擎已启动"));
    document.getElementById("lo-btn-stop")?.addEventListener("click", () => runAction("/api/limitoco/stop", {}, "限价 V2 引擎已暂停"));
    document.getElementById("lo-btn-step")?.addEventListener("click", () => runAction("/api/limitoco/step", {}, "已完成一次行情扫描"));
    document.getElementById("lo-btn-reset")?.addEventListener("click", () => {
      const cap = Math.max(100, Number(document.getElementById("lo-f-cap")?.value) || 10000);
      if (confirm(`确认以 ${fmtUsd(cap, 0)} 重置限价 V2 影子账本（清零历史成交与持仓）？`)) {
        runAction("/api/limitoco/reset", { capital: cap }, "账本已重置为干净的 V2 基线");
      }
    });
    document.getElementById("lo-btn-def")?.addEventListener("click", () => {
      const def = presets.find((x) => x.id === "1.5x");
      if (def) {
        document.getElementById("lo-f-tier").value = "1.5x";
        document.getElementById("lo-f-rpt").value = (def.risk_per_trade * 100).toFixed(2);
        document.getElementById("lo-f-poslev").value = def.max_pos_lev;
        document.getElementById("lo-f-grosslev").value = def.max_total_gross_lev;
      }
      document.getElementById("lo-f-maxsl").value = "14.0";
      document.getElementById("lo-f-cool").value = "72";
      document.getElementById("lo-f-brk").value = "3.0";
      document.getElementById("lo-f-maxpos").value = "5";
      document.getElementById("lo-f-topn").value = "80";
      document.getElementById("lo-f-minvol").value = "15";
      updateSim();
      showToast("已恢复 1.5x 默认值，请点击保存", "info");
    });
    document.getElementById("lo-btn-save")?.addEventListener("click", () => {
      const payload = {
        risk_tier: document.getElementById("lo-f-tier")?.value || "1.5x",
        capital: Number(document.getElementById("lo-f-cap")?.value) || 10000,
        risk_per_trade: (Number(document.getElementById("lo-f-rpt")?.value) || 1.2) / 100,
        max_pos_lev: Number(document.getElementById("lo-f-poslev")?.value) || 2.2,
        max_total_gross_lev: Number(document.getElementById("lo-f-grosslev")?.value) || 6.0,
        max_sl_pct: (Number(document.getElementById("lo-f-maxsl")?.value) || 14) / 100,
        cooldown_hours: parseInt(document.getElementById("lo-f-cool")?.value || "72", 10),
        daily_loss_limit_r: Number(document.getElementById("lo-f-brk")?.value) || 3.0,
        max_total_positions: parseInt(document.getElementById("lo-f-maxpos")?.value || "5", 10),
        top_n_liquid: parseInt(document.getElementById("lo-f-topn")?.value || "80", 10),
        min_daily_vol_usd: (Number(document.getElementById("lo-f-minvol")?.value) || 15) * 1_000_000,
        proxy_url: (document.getElementById("lo-f-proxy")?.value || "").trim(),
        compounding: !!document.getElementById("lo-f-comp")?.checked,
        enabled_a: !!document.getElementById("lo-f-ena")?.checked,
        enabled_b: !!document.getElementById("lo-f-enb")?.checked,
      };
      runAction("/api/limitoco/config", payload, "限价 V2 配置已保存");
    });
  }

  async function runAction(url, payload, okMsg) {
    if (busy) return;
    busy = true;
    try {
      await apiPost(url, payload);
      showToast(okMsg, "pos");
      await load();
    } catch (e) {
      showToast(`操作失败：${e.message}`, "neg");
    } finally {
      busy = false;
    }
  }

  return {
    title: "限价 V2",
    subtitle: "Binance USD-M · V2 UltraWR (52.4% WR / 12h Hold · 纯 Rust 影子)",
    pollMs: 25_000,
    async mount(el) {
      root = el;
      cdTimer = setInterval(updateCountdown, 1000);
      await load();
    },
    async refresh() {
      if (subtab !== "settings") await load();
    },
    unmount() {
      if (cdTimer) {
        clearInterval(cdTimer);
        cdTimer = null;
      }
      root = null;
    },
  };
}
