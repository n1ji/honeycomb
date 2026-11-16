import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { AlbumPick, api, SyncConvert, SyncProfile, SyncReport } from "../api";
import { store } from "../store";
import { h, toast } from "../util";
import { run } from "./actions";
import { button, modal } from "./dialogs";

const SETTING = "sync.profiles";
const plural = (n: number, word: string) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;
const gb = (bytes: number) => (bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(bytes / 1e6))} MB`);

function savedProfiles(): SyncProfile[] {
  try {
    return JSON.parse(store.settings[SETTING] || "[]");
  } catch {
    return [];
  }
}

async function saveProfile(p: SyncProfile) {
  const list = savedProfiles().filter((q) => q.dest !== p.dest);
  list.unshift(p);
  store.settings[SETTING] = JSON.stringify(list);
  await api.setSetting(SETTING, store.settings[SETTING]);
}

/** The sync in progress, shared by every Sync window and button. */
let running: { dest: string; done: number; total: number; current: string } | null = null;
/** The open window that shows it, if any. */
let watcher: { progress: () => void; done: (r: SyncReport | null) => void } | null = null;
const labels = new Set<HTMLElement>();

function paintLabels() {
  const text = !running ? "Sync" : running.total ? `Syncing ${running.done.toLocaleString()} / ${running.total.toLocaleString()}` : "Syncing…";
  for (const l of labels) l.textContent = text;
}

/** The label of a Sync button, which shows the progress while a sync runs. */
export function syncLabel(): HTMLElement {
  const l = h("span", { class: "label" });
  labels.add(l);
  paintLabels();
  return l;
}

/** Artists and albums picked with right-click → Sync to a device. */
export interface SyncPick {
  artists: string[];
  albums: AlbumPick[];
}

/** `q` with the picked artists and albums added to what it already syncs. */
function withPick(q: SyncProfile, pick?: SyncPick): SyncProfile {
  const artists = [...(q.artists ?? [])];
  const albums = [...(q.albums ?? [])];
  for (const a of pick?.artists ?? []) if (!artists.includes(a)) artists.push(a);
  for (const a of pick?.albums ?? []) if (!albums.some((b) => b.artist === a.artist && b.album === a.album)) albums.push(a);
  return { ...q, artists, albums };
}

/**
 * Sync playlists, artists, albums or the whole library to a player or flash
 * drive. `pick` adds artists or albums to the device's list (whichever device
 * is chosen), where later syncs keep them.
 */
export function openSync(pick?: SyncPick) {
  let forget = () => {};
  const m = modal("Sync to a device", { wide: true, onCancel: () => forget() });
  const saved = savedProfiles();
  let p: SyncProfile = withPick(saved[0] ?? { dest: "", wholeLibrary: false, playlists: [], removeUnselected: true }, pick);

  const destLabel = h("span", {});
  const destInfo = h("p", { class: "muted" });
  const lists = h("div", { class: "sync-lists" });
  const groups = h("div", { class: "sync-lists" });
  const whole = h("input", { type: "checkbox" });
  const remove = h("input", { type: "checkbox" });
  const convertOn = h("input", { type: "checkbox" });
  const format = h(
    "select",
    { class: "input" },
    h("option", { value: "mp3" }, "MP3"),
    h("option", { value: "aac" }, "AAC (.m4a)"),
    h("option", { value: "vorbis" }, "Ogg Vorbis"),
    h("option", { value: "flac16" }, "FLAC, 16-bit"),
  );
  const quality = h(
    "select",
    { class: "input" },
    h("option", { value: "high" }, "High"),
    h("option", { value: "smaller" }, "Smaller files"),
  );
  const qualityField = h("label", { class: "field inline" }, h("span", {}, "Quality"), quality);
  const convertNote = h("p", { class: "muted" });
  const convertOptions = h(
    "div",
    {},
    h("label", { class: "field inline" }, h("span", {}, "Convert to"), format),
    qualityField,
    convertNote,
  );
  /** The last format chosen, so switching converting off and on again keeps it. */
  let lastConvert: SyncConvert = { format: "mp3", quality: "high" };
  const syncBtn = button("Sync", () => run(startSync), true);

  const describeDest = async () => {
    destLabel.textContent = p.dest || "No destination chosen";
    destLabel.title = p.dest;
    syncBtn.disabled = !p.dest;
    if (!p.dest) {
      destInfo.textContent = "Choose the drive, or a folder on it, where the music should go.";
      return;
    }
    const t = await api.syncTarget(p.dest);
    destInfo.textContent = !t.exists
      ? "Not connected right now."
      : [
          t.rockbox ? "Rockbox player: plays logged on it are added to your play counts when you sync." : "",
          t.freeBytes !== null ? `${gb(t.freeBytes)} free.` : "",
          t.syncedFiles ? `${plural(t.syncedFiles, "track")} synced here before.` : "",
        ]
          .filter(Boolean)
          .join(" ");
  };

  // `.field` sets its own display, which beats the hidden attribute.
  const show = (el: HTMLElement, on: boolean) => (el.style.display = on ? "" : "none");
  const renderConvert = () => {
    if (p.convert) lastConvert = p.convert;
    convertOn.checked = !!p.convert;
    format.value = lastConvert.format;
    quality.value = lastConvert.quality;
    show(convertOptions, !!p.convert);
    show(qualityField, lastConvert.format !== "flac16");
    convertNote.textContent =
      lastConvert.format === "flac16"
        ? "24-bit and high-resolution files become 16-bit FLAC, still lossless at CD quality. Everything else is copied as it is."
        : "FLAC, ALAC, WAV and AIFF files are converted; MP3, AAC and Ogg files are copied as they are. Converting takes a while the first time; later syncs only convert what's new or changed.";
  };
  const setConvert = () => {
    const chosen = { format: format.value as SyncConvert["format"], quality: quality.value as SyncConvert["quality"] };
    p.convert = convertOn.checked ? chosen : null;
    lastConvert = chosen;
    renderConvert();
  };
  convertOn.addEventListener("change", setConvert);
  format.addEventListener("change", setConvert);
  quality.addEventListener("change", setConvert);
  run(async () => {
    const formats = await api.syncFormats();
    const aac = format.querySelector<HTMLOptionElement>('option[value="aac"]')!;
    aac.disabled = !formats.aac;
    if (!formats.aac) aac.textContent = "AAC (.m4a), only on macOS for now";
  });

  const renderLists = () => {
    whole.checked = p.wholeLibrary;
    remove.checked = p.removeUnselected;
    renderConvert();
    lists.replaceChildren(
      ...(store.playlists.length
        ? store.playlists.map((pl) => {
            const box = h("input", { type: "checkbox", checked: p.playlists.includes(pl.id), disabled: p.wholeLibrary });
            box.addEventListener("change", () => {
              p.playlists = box.checked ? [...p.playlists, pl.id] : p.playlists.filter((id) => id !== pl.id);
            });
            return h("label", { class: "check" }, box, pl.name, pl.kind === "smart" ? h("span", { class: "muted" }, " (smart)") : "");
          })
        : [h("p", { class: "muted" }, "No playlists yet.")]),
    );
    // Artists and albums added from a right-click; unticking one takes it off the list.
    const box = (label: string, sub: string, drop: () => void) => {
      const el = h("input", { type: "checkbox", checked: true, disabled: p.wholeLibrary });
      el.addEventListener("change", () => {
        drop();
        renderLists();
      });
      return h("label", { class: "check" }, el, label, sub ? h("span", { class: "muted" }, ` ${sub}`) : "");
    };
    const artists = p.artists ?? [];
    const albums = p.albums ?? [];
    groups.replaceChildren(
      ...(artists.length || albums.length ? [h("p", { class: "muted" }, "Artists and albums")] : []),
      ...artists.map((a) => box(a || "Unknown artist", "(everything by them)", () => (p.artists = artists.filter((x) => x !== a)))),
      ...albums.map((a) =>
        box(a.album || "Unknown album", a.artist ? `by ${a.artist}` : "", () => (p.albums = albums.filter((x) => x !== a))),
      ),
    );
  };
  whole.addEventListener("change", () => {
    p.wholeLibrary = whole.checked;
    renderLists();
  });
  remove.addEventListener("change", () => (p.removeUnselected = remove.checked));

  const chooseDest = () =>
    run(async () => {
      const dir = await open({ directory: true, multiple: false, title: "Where should the music go?" });
      if (typeof dir !== "string") return;
      p = withPick(savedProfiles().find((q) => q.dest === dir) ?? { ...p, dest: dir }, pick);
      renderLists();
      await describeDest();
    });

  const devices = h("select", { class: "input" });
  if (saved.length > 1) {
    for (const q of saved) devices.append(h("option", { value: q.dest }, q.dest));
    devices.addEventListener("change", () =>
      run(async () => {
        p = withPick(savedProfiles().find((q) => q.dest === devices.value) ?? p, pick);
        renderLists();
        await describeDest();
      }),
    );
  }

  function showSetup() {
    m.body.replaceChildren(
      h("h3", {}, "Destination"),
      saved.length > 1 ? h("label", { class: "field inline" }, h("span", {}, "Saved devices"), devices) : "",
      h("div", { class: "row" }, button("Choose…", chooseDest), destLabel),
      destInfo,
      h("h3", {}, "What to put on it"),
      h("label", { class: "check" }, whole, "The whole library"),
      lists,
      groups,
      h("label", { class: "check" }, remove, "Remove synced files that are no longer chosen"),
      h("h3", {}, "Converting"),
      h("label", { class: "check" }, convertOn, "Convert lossless files to save space or suit the player"),
      convertOptions,
      h(
        "p",
        { class: "muted" },
        "Music goes into Artist/Album folders with names that work on FAT32 and exFAT drives, and each playlist becomes an .m3u8 file next to them. Only files Honeycomb put there are ever replaced or removed, and only what changed is copied.",
      ),
    );
    m.footer.replaceChildren(button("Close", m.close), syncBtn);
  }

  async function startSync() {
    if (!p.dest) return;
    if (!p.wholeLibrary && !p.playlists.length && !p.artists?.length && !p.albums?.length) {
      toast("Choose some playlists or the whole library first.", true);
      return;
    }
    await saveProfile(p);
    running = { dest: p.dest, done: 0, total: 0, current: "Getting ready…" };
    paintLabels();
    const unlisten = await listen<{ done: number; total: number; current: string }>("sync-progress", (e) => {
      if (!running) return;
      Object.assign(running, e.payload);
      paintLabels();
      watcher?.progress();
    });
    watchProgress();
    let report: SyncReport;
    try {
      report = await api.syncRun(p);
    } catch (e) {
      watcher?.done(null);
      throw e;
    } finally {
      unlisten();
      running = null;
      paintLabels();
    }
    if (report.playsImported) await store.loadTracks(true);
    if (watcher) watcher.done(report);
    else
      toast(
        report.cancelled
          ? "Sync cancelled."
          : `Sync finished: ${plural(report.copied, "track")} copied${report.failed.length ? `, ${plural(report.failed.length, "problem")}` : ""}.`,
        report.failed.length > 0,
      );
  }

  /** The progress of the running sync, in this window, until it ends or the window is hidden. */
  function watchProgress() {
    if (!running) return;
    const bar = h("progress", { max: 1, value: 0 });
    const current = h("p", { class: "muted sync-current" });
    m.body.replaceChildren(h("p", {}, `Syncing to ${running.dest}`), bar, current, h("p", { class: "muted" }, "You can hide this window; the sync carries on, and the Sync button shows how far it is."));
    const me = {
      progress: () => {
        if (!running) return;
        bar.max = Math.max(1, running.total);
        bar.value = running.done;
        current.textContent = running.total ? `${running.done.toLocaleString()} of ${running.total.toLocaleString()}: ${running.current}` : running.current || "Nothing to copy.";
      },
      done: (r: SyncReport | null) => {
        forget();
        if (r) return showReport(r);
        showSetup();
        run(describeDest);
      },
    };
    forget = () => {
      if (watcher === me) watcher = null;
    };
    m.footer.replaceChildren(
      button("Hide", () => {
        forget();
        m.close();
      }),
      button("Cancel sync", () => run(api.syncCancel)),
    );
    watcher = me;
    me.progress();
  }

  function showReport(r: SyncReport) {
    const lines = [
      r.cancelled ? "Sync cancelled. What was copied so far is complete and stays on the device." : "Sync finished.",
      `${plural(r.copied, "track")} ${p.convert ? "copied or converted" : "copied"} (${gb(r.bytesCopied)}), ${r.unchanged.toLocaleString()} already up to date, ${r.removed.toLocaleString()} removed.`,
      r.playlists ? `${plural(r.playlists, "playlist")} written.` : "",
      r.playsImported ? `${plural(r.playsImported, "play")} from the player added to your play counts.` : "",
    ].filter(Boolean);
    m.body.replaceChildren(
      ...lines.map((l) => h("p", {}, l)),
      ...(r.failed.length
        ? [
            h("p", { class: "error" }, `${plural(r.failed.length, "problem")}:`),
            h("pre", { class: "sync-failed" }, r.failed.slice(0, 50).join("\n") + (r.failed.length > 50 ? "\n…" : "")),
          ]
        : []),
    );
    m.footer.replaceChildren(
      button("Back", () => {
        showSetup();
        run(describeDest);
      }),
      button("Done", m.close, true),
    );
  }

  if (running) watchProgress();
  else showSetup();
  renderLists();
  run(describeDest);
}
