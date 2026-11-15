import { h } from "../util";

export interface VGridOptions {
  /** A grid of square tiles at least `min` wide with `extra` pixels of text under each. Without it: full-width rows of `rowH`. */
  tile?: { min: number; extra: number; gap: number };
  rowH?: number;
  /** Scrolls away above the items. */
  header?: HTMLElement;
  count: () => number;
  render: (i: number) => HTMLElement;
  empty?: () => string;
}

export interface VGrid {
  el: HTMLElement;
  /** Redraw after the data changed. */
  refresh: () => void;
  /** Redraw at a scroll position (going back to a page). */
  restore: (top: number) => void;
  /** Draw the mounted items again where they are, for small changes (a dot, a count). */
  repaint: () => void;
}

const OVERSCAN = 3;

/**
 * A scroller that only keeps the items near the viewport in the DOM. Items
 * stay mounted while they're in range, so covers don't reload on every scroll
 * frame; `<img data-src>` inside them is loaded once fast scrolling settles.
 */
export function vgrid(o: VGridOptions): VGrid {
  const spacer = h("div", { class: "vg-spacer" });
  const empty = h("div", { class: "ln-empty muted" });
  const el = h("div", { class: "vg", tabindex: 0 }, o.header, spacer, empty);
  const live = new Map<number, HTMLElement>();
  let frame = 0;
  let cols = 1;
  let cellW = 0;
  let cellH = o.rowH ?? 0;
  let lastTop = 0;
  let imgTimer = 0;

  const clear = () => {
    for (const node of live.values()) node.remove();
    live.clear();
  };

  const loadImages = () => {
    for (const img of spacer.querySelectorAll<HTMLImageElement>("img[data-src]")) {
      img.src = img.dataset.src!;
      delete img.dataset.src;
      if (img.complete && img.naturalWidth) img.classList.add("ready", "instant");
    }
  };

  const render = () => {
    frame = 0;
    if (!el.clientHeight) return;
    const n = o.count();
    const width = spacer.clientWidth;
    if (o.tile) {
      const { min, extra, gap } = o.tile;
      const c = Math.max(1, Math.floor((width + gap) / (min + gap)));
      const w = (width - gap * (c - 1)) / c;
      if (c !== cols || w !== cellW) clear();
      cols = c;
      cellW = w;
      cellH = w + extra + gap;
    } else {
      if (width !== cellW) clear();
      cellW = width;
    }
    spacer.style.height = `${Math.ceil(n / cols) * cellH}px`;
    empty.textContent = n ? "" : (o.empty?.() ?? "");

    const top = el.scrollTop - spacer.offsetTop;
    const first = Math.max(0, Math.floor(top / cellH) - OVERSCAN) * cols;
    const last = Math.min(n, (Math.ceil((top + el.clientHeight) / cellH) + OVERSCAN) * cols);
    for (const [i, node] of live) {
      if (i < first || i >= last) {
        node.remove();
        live.delete(i);
      }
    }
    const gap = o.tile?.gap ?? 0;
    for (let i = first; i < last; i++) {
      if (live.has(i)) continue;
      const node = o.render(i);
      node.style.width = `${cellW}px`;
      node.style.height = `${cellH - gap}px`;
      node.style.transform = `translate(${(i % cols) * (cellW + gap)}px, ${Math.floor(i / cols) * cellH}px)`;
      live.set(i, node);
      spacer.append(node);
    }

    const fast = Math.abs(el.scrollTop - lastTop) > cellH * 2;
    lastTop = el.scrollTop;
    clearTimeout(imgTimer);
    if (fast) imgTimer = window.setTimeout(loadImages, 120);
    else loadImages();
  };

  const schedule = () => {
    if (!frame) frame = requestAnimationFrame(render);
  };
  el.addEventListener("scroll", schedule, { passive: true });
  new ResizeObserver(schedule).observe(el);

  return {
    el,
    refresh() {
      clear();
      render();
    },
    repaint() {
      for (const [i, old] of live) {
        const node = o.render(i);
        node.style.cssText = old.style.cssText;
        old.replaceWith(node);
        live.set(i, node);
      }
      loadImages();
    },
    restore(top) {
      clear();
      render();
      el.scrollTop = top;
      lastTop = el.scrollTop;
      render();
    },
  };
}
