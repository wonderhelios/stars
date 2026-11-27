import { initAuthModal, openAuthModal, setAuthSuccessHandler } from "./core/api.js";
import { Router } from "./core/router.js";
import { createOverviewView } from "./views/overview.js";
import { createLiveView } from "./views/live.js";
import { createLimitOcoView } from "./views/limitoco.js";
import { createArchiveView } from "./views/archive.js";

const routes = {
  overview: createOverviewView(),
  live: createLiveView(),
  limitoco: createLimitOcoView(),
  "archive/hlcarry": createArchiveView("hlcarry", "基差影子"),
  "archive/intraday": createArchiveView("intraday", "日内影子"),
  "archive/tradfi": createArchiveView("tradfi", "美股商品"),
  "archive/fastswing": createArchiveView("fastswing", "快速波段"),
};

const sidebar = document.getElementById("sidebar");
const backdrop = document.getElementById("sidebar-backdrop");
const setDrawerOpen = (open) => {
  if (sidebar) sidebar.classList.toggle("open", open);
  if (backdrop) backdrop.hidden = !open;
};

const router = new Router(routes, (entry) => {
  const titleEl = document.getElementById("crumb-title");
  const metaEl = document.getElementById("crumb-meta");
  if (titleEl) titleEl.textContent = entry.title;
  if (metaEl) metaEl.textContent = entry.subtitle || "";
  setDrawerOpen(false);
});

window.__starsNavigate = (key) => router.navigate(key, true);

// Sidebar + mobile bottom nav route buttons
document.querySelectorAll("[data-route]").forEach((btn) => {
  btn.addEventListener("click", () => router.navigate(btn.dataset.route, true));
});

// Collapsible archive group
const archToggle = document.getElementById("nav-archive-toggle");
const archList = document.getElementById("nav-archive-list");
if (archToggle && archList) {
  archToggle.addEventListener("click", () => {
    const open = archToggle.getAttribute("aria-expanded") === "true";
    archToggle.setAttribute("aria-expanded", String(!open));
    archList.hidden = open;
  });
}

// Mobile drawer toggles
document.getElementById("btn-mobile-menu")?.addEventListener("click", (e) => {
  e.stopPropagation();
  setDrawerOpen(!sidebar?.classList.contains("open"));
});
document.getElementById("btn-mobile-more")?.addEventListener("click", () => {
  if (archToggle && archList) {
    archToggle.setAttribute("aria-expanded", "true");
    archList.hidden = false;
  }
  setDrawerOpen(true);
});
backdrop?.addEventListener("click", () => setDrawerOpen(false));

// Global refresh + auth modal
const refreshBtn = document.getElementById("btn-global-refresh");
if (refreshBtn) {
  refreshBtn.addEventListener("click", async () => {
    refreshBtn.disabled = true;
    try {
      await router.refreshCurrent();
    } finally {
      refreshBtn.disabled = false;
    }
  });
}

document.getElementById("btn-auth-open")?.addEventListener("click", () => {
  setDrawerOpen(false);
  openAuthModal("");
});

initAuthModal();
setAuthSuccessHandler(() => router.refreshCurrent());
router.start();
