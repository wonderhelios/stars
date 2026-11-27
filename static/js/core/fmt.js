export function esc(s) {
  return String(s ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

export function fmtNum(v, digits = 2) {
  const n = Number(v);
  if (!Number.isFinite(n)) return "—";
  return n.toLocaleString("en-US", {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
}

export function fmtUsd(v, digits = 2) {
  const n = Number(v);
  if (!Number.isFinite(n)) return "—";
  const sign = n < 0 ? "-" : "";
  return `${sign}$${Math.abs(n).toLocaleString("en-US", {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  })}`;
}

export function fmtSignedUsd(v, digits = 2) {
  const n = Number(v);
  if (!Number.isFinite(n)) return "—";
  const sign = n > 0 ? "+" : n < 0 ? "-" : "";
  return `${sign}$${Math.abs(n).toLocaleString("en-US", {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  })}`;
}

export function fmtPct(v, digits = 2, signed = false) {
  const n = Number(v);
  if (!Number.isFinite(n)) return "—";
  const sign = signed ? (n > 0 ? "+" : n < 0 ? "-" : "") : (n < 0 ? "-" : "");
  return `${sign}${Math.abs(n).toFixed(digits)}%`;
}

export function fmtPx(v) {
  const n = Number(v);
  if (!Number.isFinite(n) || n === 0) return "—";
  const a = Math.abs(n);
  if (a >= 1000) return n.toLocaleString("en-US", { maximumFractionDigits: 2 });
  if (a >= 10) return n.toFixed(3);
  if (a >= 1) return n.toFixed(4);
  if (a >= 0.01) return n.toFixed(5);
  return n.toFixed(6);
}

export function fmtBj(tsMs, withSeconds = false) {
  const n = Number(tsMs);
  if (!Number.isFinite(n) || n <= 0) return "—";
  const d = new Date(n + 8 * 3600 * 1000);
  const pad = (x) => String(x).padStart(2, "0");
  const base = `${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}`;
  return withSeconds ? `${base}:${pad(d.getUTCSeconds())}` : base;
}

export function fmtBjDate(tsMs) {
  const n = Number(tsMs);
  if (!Number.isFinite(n) || n <= 0) return "—";
  const d = new Date(n + 8 * 3600 * 1000);
  const pad = (x) => String(x).padStart(2, "0");
  return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}`;
}

export function signCls(v) {
  const n = Number(v);
  if (!Number.isFinite(n) || Math.abs(n) < 1e-12) return "";
  return n > 0 ? "pos" : "neg";
}

/** 识别不可平碎仓（名义绝对值 < $1，如 $0.07 SUI） */
export function isDustPosition(pos, thresholdUsd = 1.0) {
  return Math.abs(Number(pos?.notional) || 0) < thresholdUsd;
}

export function filterRange(points, range) {
  if (!Array.isArray(points) || !points.length) return [];
  if (!range || range === "all") return points;
  const lastTs = points[points.length - 1].ts;
  const span = {
    "24h": 86_400_000,
    day: 86_400_000,
    "7d": 7 * 86_400_000,
    week: 7 * 86_400_000,
    "30d": 30 * 86_400_000,
    month: 30 * 86_400_000,
  }[range];
  if (!span) return points;
  const cut = lastTs - span;
  const out = points.filter((p) => p.ts >= cut);
  return out.length >= 2 ? out : points.slice(-Math.min(points.length, 24));
}

export function shortAddr(addr) {
  if (!addr || addr.length < 12) return addr || "—";
  return `${addr.slice(0, 6)}…${addr.slice(-4)}`;
}
