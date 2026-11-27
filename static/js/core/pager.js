/**
 * Lightweight 8-items-per-page pagination helper.
 */
export function paginate(items, page, pageSize = 8) {
  const list = Array.isArray(items) ? items : [];
  const total = list.length;
  const totalPages = Math.max(1, Math.ceil(total / pageSize));
  const cur = Math.max(1, Math.min(totalPages, Number(page) || 1));
  const start = (cur - 1) * pageSize;
  return {
    rows: list.slice(start, start + pageSize),
    page: cur,
    totalPages,
    total,
  };
}

export function renderPagerHtml(id, page, totalPages, total) {
  if (total <= 0 || totalPages <= 1) return "";
  return `
    <div class="pager" id="${id}">
      <span class="pager-info mono">共 ${total} 条 · ${page} / ${totalPages} 页</span>
      <div class="pager-btns">
        <button type="button" class="btn sm" data-pg="${page - 1}" ${page <= 1 ? "disabled" : ""}>上一页</button>
        <button type="button" class="btn sm" data-pg="${page + 1}" ${page >= totalPages ? "disabled" : ""}>下一页</button>
      </div>
    </div>
  `;
}

export function bindPager(root, id, onPage) {
  const el = root.querySelector(`#${id}`);
  if (!el) return;
  el.querySelectorAll("button[data-pg]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const next = Number(btn.dataset.pg);
      if (Number.isFinite(next) && next >= 1) onPage(next);
    });
  });
}
