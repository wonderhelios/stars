import { fmtBj, fmtNum } from "./fmt.js";

/**
 * High-craft pixel-exact interactive SVG financial chart.
 * - Uses true container pixel dimensions (NO preserveAspectRatio="none" distortion!)
 * - Split emerald/coral-rose gradient anchored precisely at zeroLine (0% / $0)
 * - Crisp area fill towards zeroLine, glowing endpoint halo, interactive crosshair tooltip
 */
export function renderLineChart(container, points, opts = {}) {
  if (!container) return;
  const data = (points || []).filter((p) => Number.isFinite(p.ts) && Number.isFinite(p.val));
  if (data.length < 2) {
    container.innerHTML = `<div class="empty">${opts.emptyText || "历史点不足，等待下一个采样点…"}</div>`;
    return;
  }

  const draw = () => {
    const rect = container.getBoundingClientRect();
    const w = Math.max(Math.round(rect.width) || container.clientWidth || 720, 260);
    const h = Math.max(Math.round(rect.height) || container.clientHeight || 250, 170);
    const compact = w < 480;
    const padL = compact ? 6 : 12;
    const padR = compact ? 58 : 72;
    const padT = 16;
    const padB = 26;
    const plotW = Math.max(80, w - padL - padR);
    const plotH = Math.max(80, h - padT - padB);

    const minTs = data[0].ts;
    const maxTs = Math.max(data[data.length - 1].ts, minTs + 1);

    let minV = Math.min(...data.map((d) => d.val));
    let maxV = Math.max(...data.map((d) => d.val));
    const hasZero = opts.zeroLine !== undefined && Number.isFinite(opts.zeroLine);
    if (hasZero) {
      minV = Math.min(minV, opts.zeroLine);
      maxV = Math.max(maxV, opts.zeroLine);
    }
    if (Math.abs(maxV - minV) < 1e-9) {
      const d = Math.abs(maxV) * 0.03 || 1;
      minV -= d;
      maxV += d;
    } else {
      const pad = (maxV - minV) * 0.11;
      minV -= pad;
      maxV += pad;
    }

    const xOf = (ts) => padL + ((ts - minTs) / (maxTs - minTs)) * plotW;
    const yOf = (v) => padT + (1 - (v - minV) / (maxV - minV)) * plotH;

    const firstVal = data[0].val;
    const lastVal = data[data.length - 1].val;
    const posCol = opts.posColor || "#34d399";
    const negCol = opts.negColor || "#fb7185";
    const fixedCol = opts.color || null;
    const endCol = fixedCol || (hasZero ? (lastVal >= opts.zeroLine ? posCol : negCol) : (lastVal >= firstVal ? posCol : negCol));

    const fmtY = opts.formatY || ((v) => fmtNum(v, 2));
    const fmtTip = opts.formatTip || fmtY;

    const uid = Math.random().toString(36).slice(2, 8);
    const baseY = hasZero ? yOf(opts.zeroLine) : padT + plotH;
    const zeroFrac = Math.max(0, Math.min(1, (baseY - padT) / plotH));

    // Build path string
    const lineD = data
      .map((d, i) => `${i === 0 ? "M" : "L"}${xOf(d.ts).toFixed(1)},${yOf(d.val).toFixed(1)}`)
      .join(" ");
    const areaD = `${lineD} L${xOf(data[data.length - 1].ts).toFixed(1)},${baseY.toFixed(1)} L${xOf(data[0].ts).toFixed(1)},${baseY.toFixed(1)} Z`;

    // Stroke & area gradients: split cleanly at zeroLine when hasZero && !fixedCol
    let defsSvg = "";
    let strokeRef = fixedCol || endCol;
    let fillRef = `url(#ag-${uid})`;
    if (hasZero && !fixedCol) {
      const pct = (zeroFrac * 100).toFixed(2);
      defsSvg = `
        <linearGradient id="sg-${uid}" x1="0" y1="${padT}" x2="0" y2="${padT + plotH}" gradientUnits="userSpaceOnUse">
          <stop offset="0%" stop-color="${posCol}"/>
          <stop offset="${pct}%" stop-color="${posCol}"/>
          <stop offset="${pct}%" stop-color="${negCol}"/>
          <stop offset="100%" stop-color="${negCol}"/>
        </linearGradient>
        <linearGradient id="ag-${uid}" x1="0" y1="${padT}" x2="0" y2="${padT + plotH}" gradientUnits="userSpaceOnUse">
          <stop offset="0%" stop-color="${posCol}" stop-opacity="0.24"/>
          <stop offset="${pct}%" stop-color="${posCol}" stop-opacity="0.02"/>
          <stop offset="${pct}%" stop-color="${negCol}" stop-opacity="0.02"/>
          <stop offset="100%" stop-color="${negCol}" stop-opacity="0.24"/>
        </linearGradient>
      `;
      strokeRef = `url(#sg-${uid})`;
    } else {
      defsSvg = `
        <linearGradient id="ag-${uid}" x1="0" y1="${padT}" x2="0" y2="${padT + plotH}" gradientUnits="userSpaceOnUse">
          <stop offset="0%" stop-color="${endCol}" stop-opacity="0.22"/>
          <stop offset="100%" stop-color="${endCol}" stop-opacity="0.0"/>
        </linearGradient>
      `;
    }

    // Optional secondary ribbon (e.g., leverage)
    let secSvg = "";
    const hasSec = opts.secondary && data.some((d) => Number.isFinite(d.val2));
    if (hasSec) {
      const secMax = Math.max(1, ...data.map((d) => Math.abs(d.val2 || 0))) * 1.15;
      const secH = plotH * 0.25;
      const y2 = (v) => padT + plotH - (Math.max(0, v) / secMax) * secH;
      const sLine = data.map((d, i) => `${i === 0 ? "M" : "L"}${xOf(d.ts).toFixed(1)},${y2(d.val2 || 0).toFixed(1)}`).join(" ");
      const sArea = `${sLine} L${xOf(data[data.length - 1].ts).toFixed(1)},${(padT + plotH).toFixed(1)} L${xOf(data[0].ts).toFixed(1)},${(padT + plotH).toFixed(1)} Z`;
      secSvg = `
        <path d="${sArea}" fill="rgba(56,189,248,0.09)"/>
        <path d="${sLine}" fill="none" stroke="rgba(56,189,248,0.52)" stroke-width="1.2"/>
      `;
    }

    // Grid + Y labels
    const gridTicks = 4;
    let gridSvg = "";
    for (let i = 0; i <= gridTicks; i++) {
      const frac = i / gridTicks;
      const v = maxV - frac * (maxV - minV);
      const y = padT + frac * plotH;
      gridSvg += `<line x1="${padL}" y1="${y.toFixed(1)}" x2="${(padL + plotW).toFixed(1)}" y2="${y.toFixed(1)}" stroke="rgba(148,163,184,0.08)" stroke-width="1"/>`;
      gridSvg += `<text x="${(padL + plotW + 8).toFixed(1)}" y="${(y + 3.5).toFixed(1)}" fill="#64748b" font-family="var(--font-mono)" font-size="10.5">${fmtY(v)}</text>`;
    }

    // Zero baseline
    let zeroSvg = "";
    if (hasZero && opts.zeroLine >= minV && opts.zeroLine <= maxV) {
      zeroSvg = `<line x1="${padL}" y1="${baseY.toFixed(1)}" x2="${(padL + plotW).toFixed(1)}" y2="${baseY.toFixed(1)}" stroke="rgba(148,163,184,0.32)" stroke-dasharray="4 4" stroke-width="1"/>`;
    }

    // X ticks
    const xTicks = compact ? 2 : 4;
    let xSvg = "";
    for (let i = 0; i <= xTicks; i++) {
      const ts = minTs + (i / xTicks) * (maxTs - minTs);
      const x = xOf(ts);
      const anchor = i === 0 ? "start" : i === xTicks ? "end" : "middle";
      xSvg += `<text x="${x.toFixed(1)}" y="${(h - 8).toFixed(1)}" text-anchor="${anchor}" fill="#64748b" font-family="var(--font-mono)" font-size="10.5">${fmtBj(ts)}</text>`;
    }

    const lastX = xOf(data[data.length - 1].ts);
    const lastY = yOf(lastVal);

    container.innerHTML = `
      <svg width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">
        <defs>${defsSvg}</defs>
        ${gridSvg}
        ${zeroSvg}
        ${secSvg}
        <path d="${areaD}" fill="${fillRef}"/>
        <path d="${lineD}" fill="none" stroke="${strokeRef}" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>
        <circle cx="${lastX.toFixed(1)}" cy="${lastY.toFixed(1)}" r="6" fill="${endCol}" fill-opacity="0.22"/>
        <circle cx="${lastX.toFixed(1)}" cy="${lastY.toFixed(1)}" r="3.2" fill="${endCol}" stroke="#0d1320" stroke-width="1.5"/>
        ${xSvg}
        <line class="ch-x" x1="0" y1="${padT}" x2="0" y2="${padT + plotH}" stroke="rgba(226,232,240,0.28)" stroke-dasharray="3 3" stroke-width="1" style="display:none"/>
        <circle class="ch-dot" cx="0" cy="0" r="4.2" fill="${endCol}" stroke="#0a0e16" stroke-width="2" style="display:none"/>
        <rect class="ch-hit" x="${padL}" y="${padT}" width="${plotW}" height="${plotH}" fill="transparent" style="cursor:crosshair"/>
      </svg>
      <div class="chart-tip" hidden></div>
    `;

    const svg = container.querySelector("svg");
    const hit = svg.querySelector(".ch-hit");
    const chX = svg.querySelector(".ch-x");
    const chDot = svg.querySelector(".ch-dot");
    const tip = container.querySelector(".chart-tip");

    const nearest = (ts) => {
      let lo = 0;
      let hi = data.length - 1;
      while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (data[mid].ts < ts) lo = mid + 1;
        else hi = mid;
      }
      if (lo > 0 && Math.abs(data[lo - 1].ts - ts) < Math.abs(data[lo].ts - ts)) return data[lo - 1];
      return data[lo];
    };

    const showAtClientX = (clientX) => {
      const r = svg.getBoundingClientRect();
      const rx = clientX - r.left;
      const clamped = Math.max(padL, Math.min(padL + plotW, rx));
      const ts = minTs + ((clamped - padL) / plotW) * (maxTs - minTs);
      const pt = nearest(ts);
      const px = xOf(pt.ts);
      const py = yOf(pt.val);
      const dotCol = fixedCol || (hasZero ? (pt.val >= opts.zeroLine ? posCol : negCol) : endCol);

      chX.setAttribute("x1", px.toFixed(1));
      chX.setAttribute("x2", px.toFixed(1));
      chX.style.display = "";
      chDot.setAttribute("cx", px.toFixed(1));
      chDot.setAttribute("cy", py.toFixed(1));
      chDot.setAttribute("fill", dotCol);
      chDot.style.display = "";

      let extra = "";
      if (hasSec && Number.isFinite(pt.val2)) {
        const secLbl = opts.secondaryLabel || "杠杆";
        const secFmt = opts.formatSecondary || ((v) => `${fmtNum(v, 2)}x`);
        extra = `<div style="color:var(--info);margin-top:2px">${secLbl}: ${secFmt(pt.val2)}</div>`;
      }
      tip.innerHTML = `<div class="chart-tip-ts">${fmtBj(pt.ts)}</div><div style="color:${dotCol};font-weight:700">${fmtTip(pt.val)}</div>${extra}`;
      tip.hidden = false;

      const tipW = tip.offsetWidth || 130;
      tip.style.left = `${px + tipW + 18 > w ? Math.max(8, px - tipW - 14) : px + 14}px`;
      tip.style.top = `${Math.max(8, py - 48)}px`;
    };

    const hideTip = () => {
      chX.style.display = "none";
      chDot.style.display = "none";
      tip.hidden = true;
    };

    hit.addEventListener("mousemove", (ev) => showAtClientX(ev.clientX));
    hit.addEventListener("mouseleave", hideTip);
    hit.addEventListener("touchstart", (ev) => {
      if (ev.touches?.[0]) showAtClientX(ev.touches[0].clientX);
    }, { passive: true });
    hit.addEventListener("touchmove", (ev) => {
      if (ev.touches?.[0]) showAtClientX(ev.touches[0].clientX);
    }, { passive: true });
    hit.addEventListener("touchend", hideTip, { passive: true });
  };

  draw();
  if (container._ro) container._ro.disconnect();
  if (typeof ResizeObserver !== "undefined") {
    let lastW = container.clientWidth;
    const ro = new ResizeObserver(() => {
      if (Math.abs(container.clientWidth - lastW) > 6) {
        lastW = container.clientWidth;
        draw();
      }
    });
    ro.observe(container);
    container._ro = ro;
  }
}
