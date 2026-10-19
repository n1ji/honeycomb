import { api, Track } from "../api";
import { SortKey, store } from "../store";
import { $, fmtTime, h, modKey, stars } from "../util";
import { openTrackMenu, removeSelected } from "./actions";
import { dragFrom, saveSetting } from "./layout";

const rowH = () => (store.settings.density === "compact" ? 22 : 26);
const OVERSCAN = 8;

interface Col {
  key: SortKey;
  label: string;
  width: string;
  cls?: string;
  render?: (t: Track, row: number) => string;
}

const COLS: Col[] = [
  { key: "trackNo", label: "#", width: "44px", cls: "num" },
  { key: "title", label: "Title", width: "minmax(160px, 2fr)" },
  { key: "artist", label: "Artist", width: "minmax(110px, 1.3fr)" },
  { key: "album", label: "Album", width: "minmax(110px, 1.3fr)" },
  { key: "year", label: "Year", width: "52px", cls: "num", render: (t) => (t.year ? String(t.year) : "") },
  { key: "genre", label: "Genre", width: "minmax(70px, 0.7fr)" },
  { key: "duration", label: "Time", width: "56px", cls: "num", render: (t) => fmtTime(t.duration) },
  { key: "rating", label: "Rating", width: "86px" },
  { key: "playCount", label: "Plays", width: "50px", cls: "num", render: (t) => (t.playCount ? String(t.playCount) : "") },
];

const COL_WIDTHS = "layout.columns";

/** Columns the user resized, in pixels. Others keep their flexible default. */
function colWidths(): Record<string, number> {
  try {
    return JSON.parse(store.settings[COL_WIDTHS] || "{}");
  } catch {
    return {};
  }
}

/** Header and rows share one grid template through a CSS variable. */
function applyColumns(widths = colWidths()) {
  const template = COLS.map((c) => (widths[c.key] ? `${widths[c.key]}px` : c.width)).join(" ");
  $("#tracklist").style.setProperty("--tl-cols", template);
}

let scroller: HTMLElement;
let spacer: HTMLElement;
let rowsEl: HTMLElement;
let header: HTMLElement;
let empty: HTMLElement;
let frame = 0;

/** In playlists and the queue the first column is the row's position, not the track number. */
function positional(): boolean {
  return store.view.kind === "queue" || (store.view.kind === "playlist" && store.currentPlaylist?.kind === "manual");
}

export function mountTrackList() {
  const root = $("#tracklist");
  header = h("div", { class: "tl-header" });
  applyColumns();
  scroller = h("div", { class: "tl-scroll", tabindex: 0 });
  spacer = h("div", { class: "tl-spacer" });
  rowsEl = h("div", { class: "tl-rows" });
  empty = h("div", { class: "tl-empty" });
  spacer.append(rowsEl);
  scroller.append(spacer, empty);
  root.append(header, scroller);

  renderHeader();
  scroller.addEventListener("scroll", schedule);
  new ResizeObserver(schedule).observe(scroller);

  rowsEl.addEventListener("mousedown", onRowMouseDown);
  rowsEl.addEventListener("dblclick", (e) => {
    const i = rowIndex(e);
    if (i >= 0 && !(e.target as HTMLElement).dataset.star) store.playVisible(i);
  });
  rowsEl.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    const i = rowIndex(e);
    if (i < 0) return;
    if (!store.selection.has(i)) select(i, false, false);
    openTrackMenu(e.clientX, e.clientY);
  });
  rowsEl.addEventListener("dragstart", (e) => {
    const i = rowIndex(e);
    if (i < 0 || !e.dataTransfer) return;
    if (!store.selection.has(i)) select(i, false, false);
    e.dataTransfer.setData("application/x-honeycomb-tracks", JSON.stringify(store.selectedIds()));
    e.dataTransfer.effectAllowed = "copy";
  });
  scroller.addEventListener("keydown", onKey);

  store.on("visible", () => {
    renderHeader();
    if (store.listReset) {
      cursor = -1;
      scroller.scrollTop = 0;
    }
    schedule();
  });
  store.on("selection", schedule);
  store.on("tracks", schedule);
  store.on("queue", schedule);
  store.on("settings", schedule);
  let lastTrack: number | null = null;
  store.on("tick", () => {
    if (store.tick.trackId !== lastTrack) {
      lastTrack = store.tick.trackId;
      schedule();
    }
  });
}

function renderHeader() {
  header.replaceChildren(
    ...COLS.map((c) => {
      const sorted = store.sort?.key === c.key;
      const label = c.key === "trackNo" && positional() ? "#" : c.label;
      const cell = h(
        "div",
        { class: `tl-cell ${c.cls ?? ""} ${sorted ? "sorted" : ""}`, onclick: () => store.setSort(c.key) },
        label,
        sorted ? h("span", { class: "arrow" }, store.sort!.desc ? "▼" : "▲") : null,
      );
      cell.append(resizeHandle(c, cell));
      return cell;
    }),
  );
}

/** Drag a header cell's right edge to set that column's width; double-click resets it. */
function resizeHandle(c: Col, cell: HTMLElement): HTMLElement {
  const handle = h("span", { class: "col-resize", title: "Drag to resize, double-click to reset" });
  let start = 0;
  let widths: Record<string, number> = {};
  handle.addEventListener(
    "pointerdown",
    () => {
      start = cell.getBoundingClientRect().width;
      widths = colWidths();
    },
    { capture: true },
  );
  dragFrom(
    handle,
    "col-resize",
    (dx) => {
      widths[c.key] = Math.round(Math.max(36, start + dx));
      applyColumns(widths);
    },
    () => void saveSetting(COL_WIDTHS, JSON.stringify(widths)),
  );
  handle.addEventListener("dblclick", (e) => {
    e.stopPropagation();
    const w = colWidths();
    delete w[c.key];
    applyColumns(w);
    void saveSetting(COL_WIDTHS, JSON.stringify(w));
  });
  return handle;
}

function schedule() {
  if (!frame) frame = requestAnimationFrame(render);
}

function render() {
  frame = 0;
  const rows = store.visible;
  spacer.style.height = `${rows.length * rowH()}px`;
  empty.textContent = rows.length
    ? ""
    : store.tracks.size === 0
      ? "Your library is empty. Open Settings (⚙) and add a music folder."
      : store.search
        ? "No tracks match your search."
        : "Nothing here yet.";

  const first = Math.max(0, Math.floor(scroller.scrollTop / rowH()) - OVERSCAN);
  const last = Math.min(rows.length, Math.ceil((scroller.scrollTop + scroller.clientHeight) / rowH()) + OVERSCAN);

  const playingId = store.tick.trackId;
  const inQueue = store.view.kind === "queue";
  const pos = positional();
  const out: HTMLElement[] = [];
  for (let i = first; i < last; i++) {
    const t = rows[i];
    const playing = inQueue ? store.visibleSource[i] === store.queue.current : t.id === playingId;
    const row = h("div", {
      class: `tl-row${store.selection.has(i) ? " selected" : ""}${playing ? " playing" : ""}${i % 2 ? " odd" : ""}`,
      "data-index": i,
      draggable: "true",
    });
    row.style.transform = `translateY(${i * rowH()}px)`;
    row.style.height = `${rowH()}px`;
    for (const c of COLS) {
      let cell: HTMLElement;
      if (c.key === "rating") {
        cell = h("div", { class: "tl-cell rating" }, stars(t.rating));
      } else if (c.key === "trackNo") {
        const n = pos ? String(store.visibleSource[i] + 1) : t.trackNo ? String(t.trackNo) : "";
        cell = h("div", { class: "tl-cell num" }, playing ? h("span", { class: "eq" }, "♪") : n);
      } else {
        const text = c.render ? c.render(t, i) : String(t[c.key] ?? "");
        cell = h("div", { class: `tl-cell ${c.cls ?? ""}`, title: text }, text);
      }
      row.append(cell);
    }
    out.push(row);
  }
  rowsEl.replaceChildren(...out);
}

function rowIndex(e: Event): number {
  const row = (e.target as HTMLElement).closest<HTMLElement>(".tl-row");
  return row ? Number(row.dataset.index) : -1;
}

/** Keyboard focus row; `store.anchor` is the fixed end of a shift-range. */
let cursor = -1;

function select(i: number, toggle: boolean, range: boolean) {
  const sel = store.selection;
  cursor = i;
  if (range && store.anchor >= 0) {
    if (!toggle) sel.clear();
    const [a, b] = [Math.min(store.anchor, i), Math.max(store.anchor, i)];
    for (let k = a; k <= b; k++) sel.add(k);
  } else if (toggle) {
    sel.has(i) ? sel.delete(i) : sel.add(i);
    store.anchor = i;
  } else {
    sel.clear();
    sel.add(i);
    store.anchor = i;
  }
  store.emit("selection");
}

async function onRowMouseDown(e: MouseEvent) {
  const i = rowIndex(e);
  if (i < 0) return;
  const starEl = e.target as HTMLElement;
  const star = starEl.dataset.star;
  if (star && e.button === 0) {
    const t = store.visible[i];
    // The left half of a star gives a half-star. Clicking the current rating clears it.
    const box = starEl.getBoundingClientRect();
    const picked = Number(star) * 2 - (e.clientX < box.left + box.width / 2 ? 1 : 0);
    const rating = picked === t.rating ? 0 : picked;
    const ids = store.selection.has(i) ? store.selectedIds() : [t.id];
    await api.setRating(ids, rating);
    store.updateTracks(ids.map((id) => ({ ...store.tracks.get(id)!, rating })));
    return;
  }
  if (e.button === 2 && store.selection.has(i)) return;
  // Keep a multi-selection intact when starting a drag.
  if (e.button === 0 && store.selection.has(i) && !modKey(e) && !e.shiftKey) {
    rowsEl.addEventListener("mouseup", () => select(i, false, false), { once: true });
    return;
  }
  select(i, modKey(e), e.shiftKey);
}

function onKey(e: KeyboardEvent) {
  const n = store.visible.length;
  if (!n) return;
  const cur = cursor < n ? cursor : -1;
  const move = (to: number) => {
    to = Math.max(0, Math.min(n - 1, to));
    select(to, false, e.shiftKey);
    ensureVisible(to);
  };
  const page = Math.max(1, Math.floor(scroller.clientHeight / rowH()) - 1);
  switch (e.key) {
    case "ArrowDown":
      move(cur + 1);
      break;
    case "ArrowUp":
      move(cur - 1);
      break;
    case "PageDown":
      move(cur + page);
      break;
    case "PageUp":
      move(cur - page);
      break;
    case "Home":
      move(0);
      break;
    case "End":
      move(n - 1);
      break;
    case "Enter":
      if (cur >= 0) store.playVisible(cur);
      break;
    case "Delete":
    case "Backspace":
      removeSelected();
      break;
    case "a":
      if (!modKey(e)) return;
      store.visible.forEach((_, i) => store.selection.add(i));
      store.emit("selection");
      break;
    default:
      return;
  }
  e.preventDefault();
}

function ensureVisible(i: number) {
  const top = i * rowH();
  if (top < scroller.scrollTop) scroller.scrollTop = top;
  else if (top + rowH() > scroller.scrollTop + scroller.clientHeight) {
    scroller.scrollTop = top + rowH() - scroller.clientHeight;
  }
}

/** Scroll to and select the playing track, if it is in the current list. */
export function revealPlaying() {
  const id = store.tick.trackId;
  const i = store.visible.findIndex((t) => t.id === id);
  if (i < 0) return;
  select(i, false, false);
  scroller.scrollTop = Math.max(0, i * rowH() - scroller.clientHeight / 2);
  scroller.focus();
}
