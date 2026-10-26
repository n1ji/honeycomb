import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { api, SyncProfile, SyncReport } from "../api";
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

/** Sync playlists or the whole library to a player or flash drive. */
export function openSync() {
  const m = modal("Sync to a device", { wide: true });
  const saved = savedProfiles();
  let p: SyncProfile = saved[0] ?? { dest: "", wholeLibrary: false, playlists: [], removeUnselected: true };

  const destLabel = h("span", {});
  const destInfo = h("p", { class: "muted" });
  const lists = h("div", { class: "sync-lists" });
  const whole = h("input", { type: "checkbox" });
  const remove = h("input", { type: "checkbox" });
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

  const renderLists = () => {
    whole.checked = p.wholeLibrary;
    remove.checked = p.removeUnselected;
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
      p = savedProfiles().find((q) => q.dest === dir) ?? { ...p, dest: dir };
      renderLists();
      await describeDest();
    });

  const devices = h("select", { class: "input" });
  if (saved.length > 1) {
    for (const q of saved) devices.append(h("option", { value: q.dest }, q.dest));
    devices.addEventListener("change", () =>
      run(async () => {
        p = savedProfiles().find((q) => q.dest === devices.value) ?? p;
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
      h("label", { class: "check" }, remove, "Remove synced files that are no longer chosen"),
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
    if (!p.wholeLibrary && !p.playlists.length) {
      toast("Choose some playlists or the whole library first.", true);
      return;
    }
    await saveProfile(p);
    const bar = h("progress", { max: 1, value: 0 });
    const current = h("p", { class: "muted sync-current" }, "Getting ready…");
    m.body.replaceChildren(h("p", {}, `Syncing to ${p.dest}`), bar, current);
    m.footer.replaceChildren(button("Cancel", () => run(api.syncCancel)));
    const unlisten = await listen<{ done: number; total: number; current: string }>("sync-progress", (e) => {
      bar.max = Math.max(1, e.payload.total);
      bar.value = e.payload.done;
      current.textContent = e.payload.total
        ? `${e.payload.done.toLocaleString()} of ${e.payload.total.toLocaleString()}: ${e.payload.current}`
        : "Nothing to copy.";
    });
    let report: SyncReport;
    try {
      report = await api.syncRun(p);
    } catch (e) {
      showSetup();
      await describeDest();
      throw e;
    } finally {
      unlisten();
    }
    if (report.playsImported) await store.loadTracks(true);
    showReport(report);
  }

  function showReport(r: SyncReport) {
    const lines = [
      r.cancelled ? "Sync cancelled. What was copied so far is complete and stays on the device." : "Sync finished.",
      `${plural(r.copied, "track")} copied (${gb(r.bytesCopied)}), ${r.unchanged.toLocaleString()} already up to date, ${r.removed.toLocaleString()} removed.`,
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

  showSetup();
  renderLists();
  run(describeDest);
}
