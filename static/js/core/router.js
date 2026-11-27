export class Router {
  constructor(routes, onRouteMeta) {
    this.routes = routes;
    this.onRouteMeta = onRouteMeta;
    this.currentKey = null;
    this.currentView = null;
    this.timer = null;
  }

  resolve(hash) {
    const clean = (hash || "").replace(/^#\/?/, "").trim();
    if (this.routes[clean]) return clean;
    return "overview";
  }

  async navigate(key, pushHash = true) {
    const target = this.routes[key] ? key : "overview";
    if (pushHash && location.hash !== `#${target}`) {
      history.replaceState(null, "", `#${target}`);
    }
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
    if (this.currentView && typeof this.currentView.unmount === "function") {
      this.currentView.unmount();
    }

    this.currentKey = target;
    const entry = this.routes[target];
    this.currentView = entry;

    // Highlight sidebar buttons
    document.querySelectorAll(".nav-btn[data-route]").forEach((btn) => {
      btn.classList.toggle("active", btn.dataset.route === target);
    });
    if (target.startsWith("archive/")) {
      const archToggle = document.getElementById("nav-archive-toggle");
      const archList = document.getElementById("nav-archive-list");
      if (archToggle && archList) {
        archToggle.setAttribute("aria-expanded", "true");
        archList.hidden = false;
      }
    }
    // Close mobile sidebar drawer
    const sb = document.getElementById("sidebar");
    if (sb) sb.classList.remove("open");

    if (this.onRouteMeta) this.onRouteMeta(entry);

    const root = document.getElementById("workspace");
    if (!root) return;
    root.innerHTML = `<div class="empty">正在加载 ${entry.title}…</div>`;

    await entry.mount(root);
    if (entry.pollMs && entry.pollMs > 0) {
      this.timer = setInterval(() => {
        if (!document.hidden && this.currentKey === target && typeof entry.refresh === "function") {
          entry.refresh();
        }
      }, entry.pollMs);
    }
  }

  async refreshCurrent() {
    if (this.currentView && typeof this.currentView.refresh === "function") {
      await this.currentView.refresh(true);
    }
  }

  start() {
    window.addEventListener("hashchange", () => {
      const key = this.resolve(location.hash);
      if (key !== this.currentKey) this.navigate(key, false);
    });
    this.navigate(this.resolve(location.hash), false);
  }
}
