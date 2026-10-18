import { open } from "@tauri-apps/plugin-dialog";
import { api, artUrl, bumpArt, TagEdit, Track } from "../api";
import { store } from "../store";
import { h, toast } from "../util";
import { button, modal } from "./dialogs";

type Field = Exclude<keyof TagEdit, "artPath" | "removeArt">;

const FIELDS: { key: Field; label: string; get: (t: Track) => string; short?: boolean }[] = [
  { key: "title", label: "Title", get: (t) => t.title },
  { key: "artist", label: "Artist", get: (t) => t.artist },
  { key: "albumArtist", label: "Album artist", get: (t) => t.albumArtist },
  { key: "album", label: "Album", get: (t) => t.album },
  { key: "genre", label: "Genre", get: (t) => t.genre },
  { key: "composer", label: "Composer", get: (t) => t.composer },
  { key: "year", label: "Year", get: (t) => (t.year ? String(t.year) : ""), short: true },
  { key: "trackNo", label: "Track #", get: (t) => (t.trackNo ? String(t.trackNo) : ""), short: true },
  { key: "discNo", label: "Disc #", get: (t) => (t.discNo ? String(t.discNo) : ""), short: true },
  { key: "comment", label: "Comment", get: (t) => t.comment },
];

/**
 * Edit tags for one or more tracks. With several tracks, fields whose values
 * differ show "<multiple values>" and are only written if you type in them.
 */
export function openTagEditor(tracks: Track[]) {
  if (!tracks.length) return;
  const multi = tracks.length > 1;
  const m = modal(multi ? `Edit tags — ${tracks.length} tracks` : "Edit tags", { wide: true });

  const inputs = new Map<Field, HTMLInputElement | HTMLTextAreaElement>();
  const dirty = new Set<Field>();
  const grid = h("div", { class: "tag-grid" });

  for (const f of FIELDS) {
    // Title and track number are rarely shared across tracks.
    if (multi && (f.key === "title" || f.key === "trackNo")) continue;
    const values = new Set(tracks.map(f.get));
    const same = values.size === 1;
    const input =
      f.key === "comment"
        ? h("textarea", { class: "input", rows: 2 })
        : h("input", { type: "text", class: `input${f.short ? " short" : ""}` });
    input.value = same ? [...values][0] : "";
    if (!same) input.placeholder = "<multiple values>";
    input.addEventListener("input", () => dirty.add(f.key));
    inputs.set(f.key, input);
    grid.append(h("label", { class: `field${f.short ? " short" : ""}` }, h("span", {}, f.label), input));
  }

  // Artwork
  let artPath: string | undefined;
  let removeArt = false;
  const first = tracks[0];
  const img = h("img", { class: "tag-art", alt: "" });
  const noArt = h("div", { class: "tag-art placeholder" }, "No artwork");
  const showArt = (src: string | null) => {
    img.style.display = src ? "" : "none";
    noArt.style.display = src ? "none" : "";
    if (src) img.src = src;
  };
  img.onerror = () => showArt(null);
  showArt(first.hasArt && !multi ? artUrl(first.id) : null);
  if (multi) noArt.textContent = "Artwork unchanged";

  const artBox = h(
    "div",
    { class: "tag-art-box" },
    img,
    noArt,
    h(
      "div",
      { class: "row" },
      button("Choose image…", async () => {
        const p = await open({ multiple: false, filters: [{ name: "Images", extensions: ["jpg", "jpeg", "png", "webp"] }] });
        if (typeof p !== "string") return;
        artPath = p;
        removeArt = false;
        showArt(null);
        noArt.textContent = `New: ${p.split(/[\\/]/).pop()}`;
      }),
      button("Remove", () => {
        artPath = undefined;
        removeArt = true;
        showArt(null);
        noArt.textContent = "Artwork will be removed";
      }),
    ),
  );

  const info = multi
    ? h("p", { class: "muted" }, `${tracks.length} files selected. Only fields you change are written.`)
    : h(
        "p",
        { class: "muted path" },
        `${first.format} · ${first.bitrate ? `${first.bitrate} kbps · ` : ""}${first.sampleRate ? `${(first.sampleRate / 1000).toFixed(1)} kHz · ` : ""}${first.path}`,
      );

  m.body.append(h("div", { class: "tag-layout" }, grid, artBox), info);

  const save = async () => {
    const edit: TagEdit = {};
    for (const [key, input] of inputs) {
      // In single-track mode every field is authoritative; in multi mode only edited ones.
      if (!multi || dirty.has(key)) edit[key] = input.value;
    }
    if (artPath) edit.artPath = artPath;
    if (removeArt) edit.removeArt = true;
    saveBtn.disabled = true;
    saveBtn.textContent = "Saving…";
    try {
      const updated = await api.editTags(
        tracks.map((t) => t.id),
        edit,
      );
      if (artPath || removeArt) updated.forEach((t) => bumpArt(t.id));
      store.updateTracks(updated);
      if (updated.length < tracks.length) {
        toast(`Saved ${updated.length} of ${tracks.length} files; some could not be written.`, true);
      } else {
        toast(`Saved ${updated.length} file${updated.length === 1 ? "" : "s"}`);
      }
      m.close();
    } catch (e) {
      toast(String(e), true);
      saveBtn.disabled = false;
      saveBtn.textContent = "Save";
    }
  };
  const saveBtn = button("Save", save, true);
  m.footer.append(button("Cancel", m.close), saveBtn);
  m.el.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && (e.target as HTMLElement).tagName === "INPUT") save();
  });
  (inputs.values().next().value as HTMLElement | undefined)?.focus();
}
