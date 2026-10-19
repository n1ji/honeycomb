import { api } from "../api";
import { store } from "../store";
import { $, h } from "../util";

/** Resizable regions. Sizes live in settings as pixels; an empty value means the default. */
const SIZES = {
  "layout.sidebar": { cssVar: "--sidebar-w", def: 210, min: 170, max: () => window.innerWidth * 0.35 },
  "layout.nowplaying": { cssVar: "--np-w", def: 260, min: 200, max: () => window.innerWidth * 0.4 },
  "layout.browser": { cssVar: "--browser-h", def: 180, min: 90, max: () => window.innerHeight * 0.6 },
};
type SizeKey = keyof typeof SIZES;

const BROWSER_COLS = "layout.browserCols";

function sizeOf(key: SizeKey): number {
  const v = Number(store.settings[key]);
  return v > 0 ? Math.max(SIZES[key].min, v) : SIZES[key].def;
}

/** Relative widths of the Genre / Artist / Album panes. */
function browserCols(): number[] {
  const v = (store.settings[BROWSER_COLS] || "").split(" ").map(Number);
  return v.length === 3 && v.every((n) => n > 0) ? v : [1, 1, 1];
}

export function applyLayout() {
  const root = document.documentElement.style;
  for (const key of Object.keys(SIZES) as SizeKey[]) {
    root.setProperty(SIZES[key].cssVar, `${sizeOf(key)}px`);
  }
  root.setProperty("--browser-cols", browserCols().map((n) => `minmax(0, ${n}fr)`).join(" "));
  const d = document.documentElement.dataset;
  d.sidebar = sidebarSide();
  d.np = npSide();
  d.playerbar = store.settings["layout.playerBar"] === "top" ? "top" : "bottom";
}

/** Which side of the window the sidebar and the Now Playing panel sit on. */
export const sidebarSide = (): "left" | "right" => (store.settings["layout.sidebarSide"] === "right" ? "right" : "left");
export const npSide = (): "left" | "right" => (store.settings["layout.npSide"] === "left" ? "left" : "right");

export async function saveSetting(key: string, value: string) {
  store.settings[key] = value;
  await api.setSetting(key, value);
}

/**
 * Track a pointer drag from `handle`. `onMove` gets the distance moved since
 * the drag started; `onEnd` runs once on release.
 */
export function dragFrom(handle: HTMLElement, cursor: string, onMove: (dx: number, dy: number) => void, onEnd: () => void) {
  handle.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const [x0, y0] = [e.clientX, e.clientY];
    handle.setPointerCapture(e.pointerId);
    handle.classList.add("active");
    document.body.classList.add("resizing");
    document.body.style.cursor = cursor;
    const move = (ev: PointerEvent) => onMove(ev.clientX - x0, ev.clientY - y0);
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.classList.remove("active");
      document.body.classList.remove("resizing");
      document.body.style.cursor = "";
      onEnd();
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up, { once: true });
    handle.addEventListener("pointercancel", up, { once: true });
  });
  // Clicks on a handle never reach what's underneath (e.g. column sorting).
  handle.addEventListener("click", (e) => e.stopPropagation());
}

/** `edge()` is the side the handle sits on; panels can move, so it's read on each drag. */
function edgeSplitter(target: HTMLElement, edge: () => "left" | "right" | "bottom", key: SizeKey) {
  const s = SIZES[key];
  const vertical = edge() === "bottom";
  const handle = h("div", { class: `splitter ${edge()}`, title: "Drag to resize, double-click to reset" });
  target.append(handle);
  let start = 0;
  let current = 0;
  handle.addEventListener("pointerdown", () => (start = current = sizeOf(key)), { capture: true });
  dragFrom(
    handle,
    vertical ? "row-resize" : "col-resize",
    (dx, dy) => {
      const delta = vertical ? dy : edge() === "left" ? -dx : dx;
      current = Math.round(Math.min(s.max(), Math.max(s.min, start + delta)));
      document.documentElement.style.setProperty(s.cssVar, `${current}px`);
    },
    () => void saveSetting(key, String(current)),
  );
  handle.addEventListener("dblclick", () => {
    void saveSetting(key, "");
    applyLayout();
  });
}

/** Handles between the column browser's panes; dragging trades width between neighbours. */
function browserSplitters() {
  const panes = [...$("#browser").querySelectorAll<HTMLElement>(":scope > .pane")];
  panes.slice(0, -1).forEach((pane, i) => {
    const handle = h("div", { class: "splitter right", title: "Drag to resize, double-click to reset" });
    pane.append(handle);
    let widths: number[] = [];
    dragFrom(
      handle,
      "col-resize",
      (dx) => {
        if (!widths.length) widths = panes.map((p) => p.getBoundingClientRect().width);
        const min = 80;
        const d = Math.max(min - widths[i], Math.min(widths[i + 1] - min, dx));
        const next = widths.map((w, j) => (j === i ? w + d : j === i + 1 ? w - d : w));
        store.settings[BROWSER_COLS] = next.map((w) => w.toFixed(1)).join(" ");
        applyLayout();
      },
      () => {
        widths = [];
        void saveSetting(BROWSER_COLS, store.settings[BROWSER_COLS] ?? "");
      },
    );
    handle.addEventListener("dblclick", () => {
      void saveSetting(BROWSER_COLS, "");
      applyLayout();
    });
  });
}

export function mountSplitters() {
  edgeSplitter($("#sidebar"), () => (sidebarSide() === "left" ? "right" : "left"), "layout.sidebar");
  edgeSplitter($("#nowplaying"), () => (npSide() === "right" ? "left" : "right"), "layout.nowplaying");
  edgeSplitter($("#browser"), () => "bottom", "layout.browser");
  browserSplitters();
}
