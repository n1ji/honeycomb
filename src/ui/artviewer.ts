import { save } from "@tauri-apps/plugin-dialog";
import { api, artUrl, Track } from "../api";
import { h, toast } from "../util";
import { run } from "./actions";
import { button } from "./dialogs";

/** Characters that aren't allowed in file names on some systems. */
const unsafeName = /[\\/:*?"<>|]+/g;

/** Full-size cover over the window. Click outside it or press Escape to close. */
export function openArtViewer(t: Track) {
  const img = h("img", { src: artUrl(t.id), alt: `Cover of ${t.album || t.title}` });
  const saveBtn = button("Save artwork…", () =>
    run(async () => {
      const ext = (await api.artworkExt(t.id)) ?? "jpg";
      const base = [t.albumArtist || t.artist, t.album || t.title].filter(Boolean).join(" - ").replace(unsafeName, " ").trim();
      const dest = await save({ defaultPath: `${base || "cover"}.${ext}`, filters: [{ name: "Image", extensions: [ext] }] });
      if (!dest) return;
      await api.saveArtwork(t.id, dest);
      toast("Artwork saved");
    }),
  );
  const box = h(
    "div",
    { class: "backdrop art-viewer", role: "dialog", "aria-label": "Artwork" },
    img,
    h("div", { class: "caption" }, h("div", {}, t.album || t.title), h("div", { class: "muted" }, t.albumArtist || t.artist)),
    h("div", { class: "actions" }, saveBtn, button("Close", () => close(), true)),
  );
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      close();
    }
  };
  const close = () => {
    box.remove();
    window.removeEventListener("keydown", onKey, true);
  };
  box.addEventListener("mousedown", (e) => {
    if (e.target === box) close();
  });
  window.addEventListener("keydown", onKey, true);
  document.body.append(box);
}
