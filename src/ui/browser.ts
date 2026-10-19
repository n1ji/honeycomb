import { Track } from "../api";
import { artistOf, Filters, store } from "../store";
import { $, fmtTotal, h } from "../util";

/**
 * The three-pane column browser above the track list: Genre → Artist → Album.
 * Each pane only lists values present in the panes to its left.
 */
export function mountBrowser() {
  const root = $("#browser");
  const panes: { key: keyof Filters; label: string; get: (t: Track) => string; list: HTMLElement }[] = [
    { key: "genre", label: "Genre", get: (t) => t.genre, list: h("div", { class: "pane-list" }) },
    { key: "artist", label: "Artist", get: artistOf, list: h("div", { class: "pane-list" }) },
    { key: "album", label: "Album", get: (t) => t.album, list: h("div", { class: "pane-list" }) },
  ];
  for (const p of panes) {
    root.append(h("div", { class: "pane" }, h("div", { class: "pane-head" }, p.label), p.list));
    p.list.addEventListener("click", (e) => {
      const el = (e.target as HTMLElement).closest<HTMLElement>(".pane-item");
      if (!el) return;
      store.setFilter(p.key, el.dataset.all ? null : (el.dataset.value ?? ""));
    });
  }

  const render = () => {
    const show = store.view.kind !== "queue" && store.settings.browser !== "off";
    root.style.display = show ? "" : "none";
    if (!show) return;

    let rows = store.baseRows().map((r) => r.t);
    for (const p of panes) {
      const counts = new Map<string, number>();
      for (const t of rows) {
        const v = p.get(t);
        counts.set(v, (counts.get(v) ?? 0) + 1);
      }
      const values = [...counts.keys()].sort((a, b) => {
        if (!a) return 1;
        if (!b) return -1;
        return a.localeCompare(b, undefined, { sensitivity: "base" });
      });
      const selected = store.filters[p.key];
      const items = [
        h(
          "div",
          { class: `pane-item all${selected === null ? " active" : ""}`, "data-all": "1" },
          `All (${values.length.toLocaleString()} ${p.label.toLowerCase()}${values.length === 1 ? "" : "s"})`,
        ),
        ...values.map((v) =>
          h(
            "div",
            { class: `pane-item${selected === v ? " active" : ""}`, "data-value": v, title: v || "Unknown" },
            v || h("em", {}, "Unknown"),
          ),
        ),
      ];
      p.list.replaceChildren(...items);
      if (selected !== null) rows = rows.filter((t) => p.get(t) === selected);
    }
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
            ? "Now Playing"
            : (store.currentPlaylist?.name ?? "Playlist");
    const n = store.visible.length;
    const total = store.visible.reduce((s, t) => s + t.duration, 0);
    summary.textContent = `${n.toLocaleString()} track${n === 1 ? "" : "s"} · ${fmtTotal(total)}`;
  });
}
