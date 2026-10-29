import { api, Track } from "../api";
import { SortKey, store } from "../store";
import { $, fmtDate, fmtTime, h, modKey, stars } from "../util";
import { openTrackMenu, removeSelected } from "./actions";
import { openColumnsCustomizer, setLayoutSetting } from "./customize";
import { dragFrom, saveSetting } from "./layout";
import { MenuItem, openMenu, SEP } from "./menu";

const rowH = () => (store.settings.density === "compact" ? 22 : 26);
const OVERSCAN = 8;

interface Col {
  key: SortKey;
  label: string;
  /** Fixed width; columns without one stretch (`grow`) and shrink down to `min` pixels. */
  width?: string;
  min?: number;
  grow?: number;
  /** Name in menus when the header label is too short to say what it is. */
  menuLabel?: string;
  cls?: string;
  render?: (t: Track, row: number) => string;
}

/** Every column the list can show; `layout.columnsShown` picks which ones and their order. */
export const COLS: Col[] = [
  { key: "trackNo", label: "#", menuLabel: "Track number", width: "44px", cls: "num" },
  { key: "title", label: "Title", min: 80, grow: 2 },
  { key: "artist", label: "Artist", min: 60, grow: 1.3 },
  { key: "album", label: "Album", min: 60, grow: 1.3 },
  { key: "year", label: "Year", width: "52px", cls: "num", render: (t) => (t.year ? String(t.year) : "") },
  { key: "genre", label: "Genre", min: 44, grow: 0.7 },
  { key: "duration", label: "Time", width: "56px", cls: "num", render: (t) => fmtTime(t.duration) },
  { key: "rating", label: "Rating", width: "90px" },
  { key: "playCount", label: "Plays", width: "50px", cls: "num", render: (t) => (t.playCount ? String(t.playCount) : "") },
  { key: "albumArtist", label: "Album Artist", min: 60, grow: 1.2 },
  { key: "composer", label: "Composer", min: 60, grow: 1 },
  { key: "discNo", label: "Disc", width: "44px", cls: "num", render: (t) => (t.discNo ? String(t.discNo) : "") },
  { key: "format", label: "Format", width: "64px" },
  { key: "bitrate", label: "Bitrate", width: "74px", cls: "num", render: (t) => (t.bitrate ? `${t.bitrate} kbps` : "") },
  {
    key: "sampleRate",
    label: "Sample Rate",
    width: "84px",
    cls: "num",
    render: (t) => (t.sampleRate ? `${(t.sampleRate / 1000).toFixed(1)} kHz` : ""),
  },
  { key: "dateAdded", label: "Added", menuLabel: "Date added", width: "92px", cls: "num", render: (t) => fmtDate(t.dateAdded) },
  { key: "lastPlayed", label: "Last Played", width: "92px", cls: "num", render: (t) => fmtDate(t.lastPlayed) },
  { key: "comment", label: "Comment", min: 60, grow: 1 },
];

export const DEFAULT_COLUMNS = ["trackNo", "title", "artist", "album", "year", "genre", "duration", "rating", "playCount"];
const COLS_SHOWN = "layout.columnsShown";
const BY_KEY = new Map(COLS.map((c) => [c.key as string, c]));

/** Keys of the shown columns, in order. Title is always shown. */
export function shownColumns(): string[] {
  const keys = (store.settings[COLS_SHOWN] ?? "").split(" ").filter((k) => BY_KEY.has(k));
  if (!keys.length) return DEFAULT_COLUMNS;
  return keys.includes("title") ? keys : ["title", ...keys];
}

const shown = (): Col[] => shownColumns().map((k) => BY_KEY.get(k)!);

const saveColumns = (keys: string[]) => setLayoutSetting(COLS_SHOWN, keys.join(" "));

const COL_WIDTHS = "layout.columns";

/** Columns the user resized, in pixels. Others keep their flexible default. */
function colWidths(): Record<string, number> {
  try {
    return JSON.parse(store.settings[COL_WIDTHS] || "{}");
  } catch {
    return {};
  }
}

/** A column's grid track. A resized stretchy column keeps its width but still shrinks in narrow windows. */
function track(c: Col, saved: number | undefined): string {
  if (c.width) return saved ? `${saved}px` : c.width;
  return `minmax(${c.min}px, ${saved ? `${saved}px` : `${c.grow}fr`})`;
}

/** Header and rows share one grid template through a CSS variable. */
function applyColumns(widths = colWidths()) {
  const template = shown().map((c) => track(c, widths[c.key])).join(" ");
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
  store.on("settings", () => {
    applyColumns();
    renderHeader();
    schedule();
  });
  header.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    openColumnsMenu(e.clientX, e.clientY);
  });
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
    ...shown().map((c) => {
      const sorted = store.sort?.key === c.key;
      const cell = h(
        "div",
        {
          class: `tl-cell ${c.cls ?? ""} ${sorted ? "sorted" : ""}`,
          title: c.menuLabel ?? "",
          onclick: () => {
            if (!justDragged) store.setSort(c.key);
          },
        },
        c.label,
        sorted ? h("span", { class: "arrow" }, store.sort!.desc ? "▼" : "▲") : null,
      );
      cell.addEventListener("pointerdown", (e) => startColumnDrag(e, c.key, cell));
      cell.append(resizeHandle(c, cell));
      return cell;
    }),
  );
}

/** Right-click on the header: tick columns on and off. */
function openColumnsMenu(x: number, y: number) {
  const keys = shownColumns();
  const items: MenuItem[] = COLS.map((c) => {
    const on = keys.includes(c.key);
    return {
      label: `${on ? "✓" : "\u2003"}  ${c.menuLabel ?? c.label}`,
      disabled: c.key === "title",
      action: () => void saveColumns(on ? keys.filter((k) => k !== c.key) : [...keys, c.key]),
    };
  });
  openMenu(x, y, [
    ...items,
    SEP,
    { label: "Customize columns…", action: openColumnsCustomizer },
    { label: "Reset columns", action: () => void saveColumns([]) },
  ]);
}

/** Set after a header drag so the click that ends it doesn't also sort. */
let justDragged = false;

/** Drag a header cell sideways to move its column. A plain click still sorts. */
function startColumnDrag(e: PointerEvent, key: string, cell: HTMLElement) {
  if (e.button !== 0) return;
  const x0 = e.clientX;
  const cells = [...header.children] as HTMLElement[];
  const marker = h("div", { class: "col-drop" });
  let dragging = false;
  let slot = -1;
  const move = (ev: PointerEvent) => {
    if (!dragging) {
      if (Math.abs(ev.clientX - x0) < 6) return;
      dragging = true;
      cell.classList.add("dragging");
      header.append(marker);
      document.body.classList.add("resizing");
      document.body.style.cursor = "grabbing";
    }
    // The gap nearest the pointer: before the first cell whose middle is to its right.
    const rects = cells.map((c) => c.getBoundingClientRect());
    slot = rects.findIndex((r) => ev.clientX < r.left + r.width / 2);
    if (slot < 0) slot = cells.length;
    const x = slot < cells.length ? rects[slot].left : rects[rects.length - 1].right;
    marker.style.left = `${x - header.getBoundingClientRect().left - 1}px`;
  };
  const up = () => {
    window.removeEventListener("pointermove", move);
    if (!dragging) return;
    cell.classList.remove("dragging");
    marker.remove();
    document.body.classList.remove("resizing");
    document.body.style.cursor = "";
    justDragged = true;
    setTimeout(() => (justDragged = false));
    const keys = [...shownColumns()];
    const from = keys.indexOf(key);
    const to = slot > from ? slot - 1 : slot;
    if (from < 0 || to === from) return;
    keys.splice(to, 0, keys.splice(from, 1)[0]);
    void saveColumns(keys);
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up, { once: true });
  window.addEventListener("pointercancel", up, { once: true });
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
  const cols = shown();
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
    for (const c of cols) {
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
