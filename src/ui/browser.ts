import { Track } from "../api";
import { artistOf, collator, Filters, store } from "../store";
import { $, fmtTotal, h, modKey } from "../util";
import { openTrackMenu } from "./actions";

/**
 * The three-pane column browser above the track list: Genre → Artist → Album.
 * Each pane only lists values present in the panes to its left. Click picks
 * one value, Ctrl/Cmd-click adds or removes one, Shift-click picks a range.
 */
export function mountBrowser() {
  const root = $("#browser");
  type Pane = {
    key: keyof Filters;
    label: string;
    get: (t: Track) => string;
    list: HTMLElement;
    /** Values in the order shown, for Shift-click ranges. */
    values: string[];
    /** Where the last plain or Ctrl/Cmd-click landed. */
    anchor: string | null;
  };
  const pane = (key: keyof Filters, label: string, get: (t: Track) => string): Pane => ({
    key,
    label,
    get,
    list: h("div", { class: "pane-list" }),
    values: [],
    anchor: null,
  });
  const panes = [pane("genre", "Genre", (t) => t.genre), pane("artist", "Artist", artistOf), pane("album", "Album", (t) => t.album)];
  for (const p of panes) {
    root.append(h("div", { class: "pane" }, h("div", { class: "pane-head" }, p.label), p.list));
    p.list.addEventListener("click", (e) => {
      const el = (e.target as HTMLElement).closest<HTMLElement>(".pane-item");
      if (!el) return;
      if (el.dataset.all) {
        p.anchor = null;
        store.setFilter(p.key, null);
        return;
      }
      const v = el.dataset.value ?? "";
      const picked = store.filters[p.key] ?? [];
      if (e.shiftKey && p.anchor !== null && p.values.includes(p.anchor)) {
        const [a, b] = [p.values.indexOf(p.anchor), p.values.indexOf(v)].sort((x, y) => x - y);
        store.setFilter(p.key, p.values.slice(a, b + 1));
      } else if (modKey(e)) {
        p.anchor = v;
        store.setFilter(p.key, picked.includes(v) ? picked.filter((x) => x !== v) : [...picked, v]);
      } else {
        p.anchor = v;
        store.setFilter(p.key, [v]);
      }
    });
    // Right-click: the track menu for everything in that row, e.g. a whole album.
    p.list.addEventListener("contextmenu", (e) => {
      const el = (e.target as HTMLElement).closest<HTMLElement>(".pane-item");
      if (!el) return;
      e.preventDefault();
      let rows = store.baseRows().map((r) => r.t);
      for (const q of panes) {
        if (q === p) break;
        const sel = store.filters[q.key];
        if (sel) rows = rows.filter((t) => sel.includes(q.get(t)));
      }
      if (!el.dataset.all) {
        // Right-clicking one of several picked rows covers all of them.
        const v = el.dataset.value ?? "";
        const picked = store.filters[p.key];
        const set = new Set(picked?.includes(v) ? picked : [v]);
        rows = rows.filter((t) => set.has(p.get(t)));
      }
      openTrackMenu(e.clientX, e.clientY, rows.map((t) => t.id));
    });
  }

  // What each pane was last built from. Searching doesn't change the panes,
  // and picking an artist doesn't change the genre pane, so most updates
  // only move the highlight instead of rebuilding thousands of rows.
  const built = panes.map(() => "");

  const render = () => {
    const show = store.view.kind !== "queue" && store.settings.browser !== "off";
    root.style.display = show ? "" : "none";
    if (!show) return;

    // Rows feeding the next pane: the view, narrowed by each selection so far.
    let rows: Track[] | null = null;
    const base = () => (rows ??= store.baseRows().map((r) => r.t));
    let inputs = `${store.tracksVersion}|${store.viewVersion}`;
    for (const [i, p] of panes.entries()) {
      const selected = store.filters[p.key];
      const picked = new Set(selected ?? []);
      if (built[i] === inputs) {
        // Same contents: just move the highlight.
        for (const el of p.list.querySelectorAll<HTMLElement>(".pane-item")) {
          el.classList.toggle("active", el.dataset.all ? selected === null : picked.has(el.dataset.value ?? ""));
        }
      } else {
        built[i] = inputs;
        renderPane(p, base(), selected === null ? null : picked);
      }
      inputs += `|${selected === null ? "*" : selected.join("\u0000")}`;
      if (selected !== null) rows = base().filter((t) => picked.has(p.get(t)));
    }
  };

  const renderPane = (p: Pane, rows: Track[], picked: Set<string> | null) => {
    const counts = new Map<string, number>();
    for (const t of rows) {
      const v = p.get(t);
      counts.set(v, (counts.get(v) ?? 0) + 1);
    }
    const values = [...counts.keys()].sort((a, b) => {
      if (!a) return 1;
      if (!b) return -1;
      return collator.compare(a, b);
    });
    p.values = values;
    const items = [
      h(
        "div",
        { class: `pane-item all${picked === null ? " active" : ""}`, "data-all": "1" },
        `All (${values.length.toLocaleString()} ${p.label.toLowerCase()}${values.length === 1 ? "" : "s"})`,
      ),
      ...values.map((v) =>
        h(
          "div",
          { class: `pane-item${picked?.has(v) ? " active" : ""}`, "data-value": v, title: v || "Unknown" },
          v || h("em", {}, "Unknown"),
        ),
      ),
    ];
    p.list.replaceChildren(...items);
  };

  store.on("visible", render);
  store.on("settings", render);
}

/** Header above the list: view title, counts and search box. */
export function mountToolbar() {
  const title = $("#view-title");
  const summary = $("#view-summary");
  const search = $("#search") as HTMLInputElement;

  let timer = 0;
  search.addEventListener("input", () => {
    clearTimeout(timer);
    timer = window.setTimeout(() => {
      store.search = search.value;
      store.recompute();
    }, 120);
  });
  search.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      search.value = "";
      store.search = "";
      store.recompute();
      search.blur();
    }
  });

  store.on("visible", () => {
    const v = store.view;
    title.textContent =
      v.kind === "library"
        ? "Music"
        : v.kind === "inbox"
          ? "Inbox"
          : v.kind === "queue"
            ? "Next Up"
            : (store.currentPlaylist?.name ?? "Playlist");
    const n = store.visible.length;
    const total = store.visible.reduce((s, t) => s + t.duration, 0);
    summary.textContent = `${n.toLocaleString()} track${n === 1 ? "" : "s"} · ${fmtTotal(total)}`;
  });
}
