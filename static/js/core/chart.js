import { fmtBj, fmtNum } from "./fmt.js";

/**
 * Interactive institutional SVG line chart with hover crosshair & tooltip.
 * @param {HTMLElement} container
 * @param {Array<{ts:number, val:number, val2?:number}>} points
 * @param {object} opts
 */
export function renderLineChart(container, points, opts = {}) {
  if (!container) return;
  const data = (points || []).filter((p) => Number.isFinite(p.ts) && Number.isFinite(p.val));
  if (data.length < 2) {
    container.innerHTML = `<div class="empty">${opts.emptyText || "历史点不足，等待下一个采样点…"}</div>`;
    return;
  }

  const w = Math.max(container.clientWidth || 640, 320);
  const h = Math.max(container.clientHeight || 220, 160);
  const padL = 12;
  const padR = 64;
  const padT = 14;
  const padB = 26;
  const plotW = w - padL - padR;
  const plotH = h - padT - padB;

  const minTs = data[0].ts;
  const maxTs = Math.max(data[data.length - 1].ts, minTs + 1);

  let minV = Math.min(...data.map((d) => d.val));
  let maxV = Math.max(...data.map((d) => d.val));
  if (opts.zeroLine !== undefined && Number.isFinite(opts.zeroLine)) {
    minV = Math.min(minV, opts.zeroLine);
    maxV = Math.max(maxV, opts.zeroLine);
  }
  if (Math.abs(maxV - minV) < 1e-9) {
    const d = Math.abs(maxV) * 0.02 || 1;
    minV -= d;
    maxV += d;
  } else {
    const pad = (maxV - minV) * 0.08;
    minV -= pad;
    maxV += pad;
  }

  const xOf = (ts) => padL + ((ts - minTs) / (maxTs - minTs)) * plotW;
  const yOf = (v) => padT + (1 - (v - minV) / (maxV - minV)) * plotH;

  const firstVal = data[0].val;
  const lastVal = data[data.length - 1].val;
  const lineColor =
    opts.color ||
    (opts.zeroLine !== undefined
      ? lastVal >= opts.zeroLine ? "#22c55e" : "#ef4444"
      : lastVal >= firstVal ? "#22c55e" : "#ef4444");

  const fmtY = opts.formatY || ((v) => fmtNum(v, 2));
  const fmtTip = opts.formatTip || fmtY;

  // Primary path
  const linePts = data.map((d) => `${xOf(d.ts).toFixed(1)},${yOf(d.val).toFixed(1)}`).join(" ");
  const areaPts = `${xOf(data[0].ts).toFixed(1)},${(padT + plotH).toFixed(1)} ${linePts} ${xOf(data[data.length - 1].ts).toFixed(1)},${(padT + plotH).toFixed(1)}`;

  // Optional secondary bar/line series (e.g. leverage ribbon)
  let secSvg = "";
  const hasSec = opts.secondary && data.some((d) => Number.isFinite(d.val2));
  if (hasSec) {
    const secMax = Math.max(1, ...data.map((d) => Math.abs(d.val2 || 0))) * 1.15;
    const secH = plotH * 0.28;
    const y2 = (v) => padT + plotH - (Math.max(0, v) / secMax) * secH;
    const secPts = data.map((d) => `${xOf(d.ts).toFixed(1)},${y2(d.val2 || 0).toFixed(1)}`).join(" ");
    const secArea = `${xOf(data[0].ts).toFixed(1)},${(padT + plotH).toFixed(1)} ${secPts} ${xOf(data[data.length - 1].ts).toFixed(1)},${(padT + plotH).toFixed(1)}`;
    secSvg = `
      <polygon points="${secArea}" fill="rgba(56,189,248,0.10)"></polygon>
      <polyline points="${secPts}" fill="none" stroke="rgba(56,189,248,0.55)" stroke-width="1.1"></polyline>
    `;
  }

  // Horizontal grid + Y labels
  const gridTicks = 4;
  let gridSvg = "";
  for (let i = 0; i <= gridTicks; i++) {
    const frac = i / gridTicks;
    const v = maxV - frac * (maxV - minV);
    const y = padT + frac * plotH;
    gridSvg += `<line x1="${padL}" y1="${y.toFixed(1)}" x2="${(padL + plotW).toFixed(1)}" y2="${y.toFixed(1)}" stroke="rgba(255,255,255,0.055)" stroke-width="1"/>`;
    gridSvg += `<text x="${(padL + plotW + 6).toFixed(1)}" y="${(y + 3.5).toFixed(1)}" fill="#64748b" font-family="var(--font-mono)" font-size="10">${fmtY(v)}</text>`;
  }

  // Zero/baseline dashed line
  let zeroSvg = "";
  if (opts.zeroLine !== undefined && opts.zeroLine >= minV && opts.zeroLine <= maxV) {
    const zy = yOf(opts.zeroLine);
    zeroSvg = `<line x1="${padL}" y1="${zy.toFixed(1)}" x2="${(padL + plotW).toFixed(1)}" y2="${zy.toFixed(1)}" stroke="rgba(255,255,255,0.22)" stroke-dasharray="3 3" stroke-width="1"/>`;
  }

  // X ticks
  const xTicks = 4;
  let xSvg = "";
  for (let i = 0; i <= xTicks; i++) {
    const ts = minTs + (i / xTicks) * (maxTs - minTs);
    const x = xOf(ts);
    const anchor = i === 0 ? "start" : i === xTicks ? "end" : "middle";
    xSvg += `<text x="${x.toFixed(1)}" y="${(h - 7).toFixed(1)}" text-anchor="${anchor}" fill="#64748b" font-family="var(--font-mono)" font-size="10">${fmtBj(ts)}</text>`;
  }

  const gradId = `cg-${Math.random().toString(36).slice(2, 8)}`;
  container.innerHTML = `
    <svg viewBox="0 0 ${w} ${h}" preserveAspectRatio="none">
      <defs>
        <linearGradient id="${gradId}" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stop-color="${lineColor}" stop-opacity="0.18"/>
          <stop offset="100%" stop-color="${lineColor}" stop-opacity="0.0"/>
        </linearGradient>
      </defs>
      ${gridSvg}
      ${zeroSvg}
      ${secSvg}
      <polygon points="${areaPts}" fill="url(#${gradId})"/>
      <polyline points="${linePts}" fill="none" stroke="${lineColor}" stroke-width="1.75" stroke-linejoin="round" stroke-linecap="round"/>
      <circle cx="${xOf(data[data.length - 1].ts).toFixed(1)}" cy="${yOf(lastVal).toFixed(1)}" r="2.8" fill="${lineColor}"/>
      ${xSvg}
      <line class="ch-x" x1="0" y1="${padT}" x2="0" y2="${padT + plotH}" stroke="rgba(255,255,255,0.28)" stroke-dasharray="2 2" stroke-width="1" style="display:none"/>
      <circle class="ch-dot" cx="0" cy="0" r="3.5" fill="${lineColor}" stroke="#0a0c10" stroke-width="1.5" style="display:none"/>
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
    if (lo > 0 && Math.abs(data[lo - 1].ts - ts) < Math.abs(data[lo].ts - ts)) {
      return data[lo - 1];
    }
    return data[lo];
  };

  hit.addEventListener("mousemove", (ev) => {
    const rect = svg.getBoundingClientRect();
    const rx = ((ev.clientX - rect.left) / Math.max(rect.width, 1)) * w;
    const clamped = Math.max(padL, Math.min(padL + plotW, rx));
    const ts = minTs + ((clamped - padL) / plotW) * (maxTs - minTs);
    const pt = nearest(ts);
    const px = xOf(pt.ts);
    const py = yOf(pt.val);

    chX.setAttribute("x1", px.toFixed(1));
    chX.setAttribute("x2", px.toFixed(1));
    chX.style.display = "";
    chDot.setAttribute("cx", px.toFixed(1));
    chDot.setAttribute("cy", py.toFixed(1));
    chDot.style.display = "";

    let extra = "";
    if (hasSec && Number.isFinite(pt.val2)) {
      const secLbl = opts.secondaryLabel || "杠杆";
      const secFmt = opts.formatSecondary || ((v) => `${fmtNum(v, 2)}x`);
      extra = `<div style="color:var(--info)">${secLbl}: ${secFmt(pt.val2)}</div>`;
    }
    tip.innerHTML = `<div class="chart-tip-ts">${fmtBj(pt.ts)}</div><div><b>${fmtTip(pt.val)}</b></div>${extra}`;
    tip.hidden = false;

    const tipW = tip.offsetWidth || 120;
    const leftCss = (px / w) * rect.width;
    const topCss = Math.max(6, (py / h) * rect.height - 46);
    tip.style.left = `${leftCss + tipW + 16 > rect.width ? Math.max(6, leftCss - tipW - 12) : leftCss + 12}px`;
    tip.style.top = `${topCss}px`;
  });

  hit.addEventListener("mouseleave", () => {
    chX.style.display = "none";
    chDot.style.display = "none";
    tip.hidden = true;
  });
}
