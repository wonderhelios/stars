const TOKEN_KEY = "stars_token";

export function getToken() {
  try {
    return localStorage.getItem(TOKEN_KEY) || "";
  } catch {
    return "";
  }
}

export function setToken(val) {
  try {
    if (val) localStorage.setItem(TOKEN_KEY, val.trim());
    else localStorage.removeItem(TOKEN_KEY);
  } catch {
    // ignore storage errors
  }
}

export function showToast(msg, kind = "info", durationMs = 3600) {
  let stack = document.getElementById("toast-stack");
  if (!stack) {
    stack = document.createElement("div");
    stack.id = "toast-stack";
    stack.className = "toast-stack";
    document.body.appendChild(stack);
  }
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = msg;
  stack.appendChild(el);
  setTimeout(() => {
    el.style.opacity = "0";
    el.style.transition = "opacity 0.2s";
    setTimeout(() => el.remove(), 220);
  }, durationMs);
}

let onAuthSuccessCb = null;
export function setAuthSuccessHandler(fn) {
  onAuthSuccessCb = fn;
}

export function openAuthModal(reason = "") {
  const overlay = document.getElementById("auth-overlay");
  if (!overlay) return;
  overlay.hidden = false;
  const st = document.getElementById("auth-status");
  if (st) st.textContent = reason;
  const input = document.getElementById("auth-token");
  if (input) {
    input.value = getToken();
    input.focus();
  }
}

export function initAuthModal() {
  const submit = document.getElementById("auth-submit");
  const input = document.getElementById("auth-token");
  const overlay = document.getElementById("auth-overlay");
  const st = document.getElementById("auth-status");
  if (!submit || !input || !overlay) return;

  const verify = async () => {
    const tok = input.value.trim();
    if (!tok) {
      if (st) st.textContent = "请输入访问令牌";
      return;
    }
    setToken(tok);
    if (st) st.textContent = "正在验证…";
    try {
      const res = await fetch("/api/live", {
        headers: { Authorization: `Bearer ${tok}` },
        cache: "no-store",
      });
      if (res.status === 401) {
        if (st) st.textContent = "令牌不正确，请重新输入";
        return;
      }
      overlay.hidden = true;
      if (st) st.textContent = "";
      showToast("鉴权通过", "pos");
      if (onAuthSuccessCb) onAuthSuccessCb();
    } catch (e) {
      if (st) st.textContent = `连接失败：${e.message}`;
    }
  };
  submit.addEventListener("click", verify);
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") verify();
  });
}

export async function apiRequest(url, opts = {}) {
  const headers = { ...(opts.headers || {}) };
  const tok = getToken();
  if (tok) headers.Authorization = `Bearer ${tok}`;
  if (opts.body !== undefined && !headers["Content-Type"]) {
    headers["Content-Type"] = "application/json";
  }
  const res = await fetch(url, {
    ...opts,
    headers,
    cache: "no-store",
  });
  if (res.status === 401) {
    openAuthModal("服务端启用了 STARS_TOKEN 鉴权，请输入访问令牌");
    throw new Error("未授权 (401)");
  }
  const text = await res.text();
  let data = {};
  try {
    data = text ? JSON.parse(text) : {};
  } catch {
    data = { error: text || `HTTP ${res.status}` };
  }
  if (!res.ok || data?.ok === false) {
    throw new Error(data?.error || data?.message || `请求失败 (${res.status})`);
  }
  return data;
}

export function apiGet(url) {
  return apiRequest(url, { method: "GET" });
}

export function apiPost(url, payload) {
  return apiRequest(url, {
    method: "POST",
    body: payload !== undefined ? JSON.stringify(payload) : undefined,
  });
}
