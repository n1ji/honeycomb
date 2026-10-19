import { artistOf, Filters, store, View } from "../store";
import { $, h } from "../util";
import { addToPlaylist, clearInbox, importPlaylist, newPlaylist, openPlaylistMenu, run } from "./actions";
import { openMenu } from "./menu";
import { openSettings } from "./settings";
import { openSmartEditor } from "./smarteditor";
import { saveSetting } from "./layout";
import { mountUpdateBulb } from "./update";

const DRAG_TYPE = "application/x-honeycomb-tracks";

const LIST_MODES = { playlists: "Playlists", artists: "Artists", albums: "Albums", genres: "Genres" } as const;
type ListMode = keyof typeof LIST_MODES;
const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });

function same(a: View, b: View) {
  return a.kind === b.kind && (a.kind !== "playlist" || (b.kind === "playlist" && a.id === b.id));
}

export function mountSidebar() {
  const root = $("#sidebar");
  const nav = h("nav", { class: "nav" });
  const status = h("div", { class: "scan-status" });
  root.append(
    h("div", { class: "brand" }, h("span", { class: "logo" }, "⬢"), "Honeycomb"),
    nav,
    status,
    h(
      "div",
      { class: "sidebar-footer" },
      h("button", { class: "icon-btn", title: "Settings", onclick: () => openSettings() }, "⚙ Settings"),
      mountUpdateBulb(),
    ),
  );

  const item = (label: string, view: View, icon: string, extra?: (el: HTMLElement) => void) => {
    const el = h(
      "div",
      { class: `nav-item${same(store.view, view) ? " active" : ""}`, onclick: () => run(() => store.setView(view)) },
      h("span", { class: "nav-icon" }, icon),
      h("span", { class: "nav-label" }, label),
    );
    extra?.(el);
    return el;
  };

  // Library entries, then a dropdown that picks what the rest of the bar lists.
  const libBlock = h("div", {});
  const mode = h("select", { class: "input list-mode", title: "What this list shows" });
  for (const [k, label] of Object.entries(LIST_MODES)) mode.append(h("option", { value: k }, label));
  const actions = h(
    "span",
    { class: "nav-actions" },
    h(
      "button",
      {
        class: "mini",
        title: "New playlist",
        onclick: (e: Event) => {
          const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
          openMenu(r.left, r.bottom + 2, [
            { label: "New playlist…", action: () => run(() => newPlaylist()) },
            { label: "New smart playlist…", action: () => openSmartEditor() },
          ]);
        },
      },
      "+",
    ),
    h("button", { class: "mini", title: "Import M3U playlist", onclick: () => run(importPlaylist) }, "⇩"),
  );
  const filter = h("input", { type: "search", class: "input list-filter", placeholder: "Filter", spellcheck: "false" });
  const listBlock = h("div", { class: "nav-list" });
  nav.append(libBlock, h("div", { class: "list-mode-row" }, mode, actions), filter, listBlock);

  const currentMode = (): ListMode => {
    const m = store.settings.sidebarList as ListMode;
    return m in LIST_MODES ? m : "playlists";
  };
  mode.addEventListener("change", () =>
    run(async () => {
      filter.value = "";
      await saveSetting("sidebarList", mode.value);
      renderList();
    }),
  );
  filter.addEventListener("input", () => renderList());

  const renderLib = () => {
    const queueLen = store.queue.trackIds.length;
    const inboxCount = [...store.tracks.values()].filter((t) => t.inbox).length;
    libBlock.replaceChildren(
      h("div", { class: "nav-head" }, h("span", {}, "Library")),
      item("Music", { kind: "library" }, "♫", (el) => el.append(h("span", { class: "badge" }, store.tracks.size.toLocaleString()))),
      item("Inbox", { kind: "inbox" }, "✉", (el) => {
        if (inboxCount) el.append(h("span", { class: "badge new" }, inboxCount.toLocaleString()));
        el.addEventListener("contextmenu", (e) => {
          e.preventDefault();
          openMenu(e.clientX, e.clientY, [{ label: "Clear Inbox", disabled: !inboxCount, action: () => run(() => clearInbox()) }]);
        });
      }),
      item("Now Playing", { kind: "queue" }, "▶", (el) => {
        if (queueLen) el.append(h("span", { class: "badge" }, String(queueLen)));
      }),
    );
  };

  const renderPlaylists = () => {
    const pls = store.playlists;
    listBlock.replaceChildren(
      ...pls.map((p) =>
        item(p.name, { kind: "playlist", id: p.id }, p.kind === "smart" ? "⚡" : "≡", (el) => {
          el.addEventListener("contextmenu", (e) => {
            e.preventDefault();
            openPlaylistMenu(e.clientX, e.clientY, p);
          });
          if (p.kind !== "manual") return;
          // Drop tracks from the list onto a playlist to add them.
          el.addEventListener("dragover", (e) => {
            if (e.dataTransfer?.types.includes(DRAG_TYPE)) {
              e.preventDefault();
              el.classList.add("drop");
            }
          });
          el.addEventListener("dragleave", () => el.classList.remove("drop"));
          el.addEventListener("drop", (e) => {
            el.classList.remove("drop");
            const data = e.dataTransfer?.getData(DRAG_TYPE);
            if (data) run(() => addToPlaylist(p, JSON.parse(data)));
          });
        }),
      ),
      ...(pls.length ? [] : [h("div", { class: "nav-empty muted" }, "No playlists yet")]),
    );
  };

  /** Artists, albums or genres, each a shortcut to the library filtered to it. */
  const renderGroups = (m: Exclude<ListMode, "playlists">) => {
    const groups = new Map<string, { label: string; sub: string; filters: Partial<Filters>; count: number }>();
    for (const t of store.tracks.values()) {
      const artist = artistOf(t);
      const [key, label, sub, filters]: [string, string, string, Partial<Filters>] =
        m === "artists"
          ? [artist, artist || "Unknown artist", "", { artist }]
          : m === "albums"
            ? [`${t.album}\u0000${artist}`, t.album || "Unknown album", artist, { artist, album: t.album }]
            : [t.genre, t.genre || "No genre", "", { genre: t.genre }];
      const g = groups.get(key);
      if (g) g.count++;
      else groups.set(key, { label, sub, filters, count: 1 });
    }
    const q = filter.value.trim().toLowerCase();
    const rows = [...groups.values()]
      .filter((g) => !q || `${g.label} ${g.sub}`.toLowerCase().includes(q))
      .sort((a, b) => collator.compare(a.label, b.label) || collator.compare(a.sub, b.sub));
    listBlock.replaceChildren(
      ...rows.map((g) => {
        const el = h(
          "div",
          { class: "nav-item group-item", title: g.sub ? `${g.label} — ${g.sub}` : g.label, onclick: () => run(() => store.showFiltered(g.filters)) },
          h("span", { class: "nav-label" }, g.label, g.sub ? h("span", { class: "nav-sub" }, g.sub) : null),
          h("span", { class: "badge" }, g.count.toLocaleString()),
        );
        groupFilters.set(el, g.filters);
        return el;
      }),
      ...(rows.length ? [] : [h("div", { class: "nav-empty muted" }, q ? "No matches" : "Nothing here yet")]),
    );
    markActive();
  };
  const groupFilters = new WeakMap<HTMLElement, Partial<Filters>>();

  /** Highlight the artist/album/genre the library is filtered to. */
  const markActive = () => {
    const f = store.filters;
    const on = store.view.kind === "library";
    for (const el of listBlock.querySelectorAll<HTMLElement>(".group-item")) {
      const g = groupFilters.get(el);
      const match =
        on && !!g && (Object.keys(g) as (keyof Filters)[]).every((k) => f[k] === g[k]) &&
        (Object.keys(f) as (keyof Filters)[]).every((k) => f[k] === null || g[k] === f[k]);
      el.classList.toggle("active", match);
    }
  };

  const renderList = () => {
    const m = currentMode();
    mode.value = m;
    actions.style.display = m === "playlists" ? "" : "none";
    filter.style.display = m === "playlists" ? "none" : "";
    filter.placeholder = `Filter ${LIST_MODES[m].toLowerCase()}`;
    if (m === "playlists") renderPlaylists();
    else renderGroups(m);
  };

  store.on("playlists", () => currentMode() === "playlists" && renderList());
  store.on("view", () => {
    renderLib();
    if (currentMode() === "playlists") renderList();
    else markActive();
  });
  store.on("tracks", () => {
    renderLib();
    if (currentMode() !== "playlists") renderList();
  });
  store.on("queue", renderLib);
  store.on("visible", markActive);
  renderLib();
  renderList();

  return {
    setStatus(text: string) {
      status.textContent = text;
      status.style.display = text ? "" : "none";
    },
  };
}
