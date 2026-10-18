import { store, View } from "../store";
import { $, h } from "../util";
import { addToPlaylist, importPlaylist, newPlaylist, openPlaylistMenu, run } from "./actions";
import { openSettings } from "./settings";
import { openSmartEditor } from "./smarteditor";

const DRAG_TYPE = "application/x-honeycomb-tracks";

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

  const render = () => {
    const pls = store.playlists;
    const queueLen = store.queue.trackIds.length;
    nav.replaceChildren(
      h("div", { class: "nav-head" }, "Library"),
      item("Music", { kind: "library" }, "♫", (el) => el.append(h("span", { class: "badge" }, store.tracks.size.toLocaleString()))),
      item("Now Playing", { kind: "queue" }, "▶", (el) => {
        if (queueLen) el.append(h("span", { class: "badge" }, String(queueLen)));
      }),
      h(
        "div",
        { class: "nav-head" },
        "Playlists",
        h(
          "span",
          { class: "nav-actions" },
          h("button", { class: "mini", title: "New playlist", onclick: () => run(() => newPlaylist()) }, "+"),
          h("button", { class: "mini", title: "New smart playlist", onclick: () => openSmartEditor() }, "⚡"),
          h("button", { class: "mini", title: "Import M3U playlist", onclick: () => run(importPlaylist) }, "⇩"),
        ),
      ),
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

  store.on("playlists", render);
  store.on("view", render);
  store.on("tracks", render);
  store.on("queue", render);
  render();

  return {
    setStatus(text: string) {
      status.textContent = text;
      status.style.display = text ? "" : "none";
    },
  };
}
