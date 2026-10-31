import { ask, open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, Playlist } from "../api";
import { store } from "../store";
import { starsText, toast } from "../util";
import { promptText } from "./dialogs";
import { MenuItem, openMenu, SEP } from "./menu";
import { openSmartEditor } from "./smarteditor";
import { openTagEditor } from "./tageditor";

export async function run(fn: () => Promise<unknown>) {
  try {
    await fn();
  } catch (e) {
    toast(String(e), true);
  }
}

/**
 * The menu for the selected tracks, or for `group` (every track of an artist,
 * album or genre in the column browser), which works on all of them at once.
 */
export function openTrackMenu(x: number, y: number, group?: number[]) {
  const ids = group ?? store.selectedIds();
  if (!ids.length) return;
  const manual = store.playlists.filter((p) => p.kind === "manual");
  const view = store.view;
  const pl = store.currentPlaylist;

  const items: MenuItem[] = [
    { label: "Play", action: () => run(() => api.load(ids, 0)) },
    ...(ids.length > 1 ? [{ label: "Shuffle", action: () => run(() => api.load(shuffled(ids), 0)) }] : []),
    { label: "Play next", action: () => run(() => api.enqueue(ids, true)) },
    { label: "Add to queue", action: () => run(() => api.enqueue(ids, false)) },
    SEP,
    {
      label: "Add to playlist",
      submenu: [
        { label: "New playlist…", action: () => run(() => newPlaylist(ids)) },
        ...(manual.length ? [SEP] : []),
        ...manual.map((p) => ({ label: p.name, action: () => run(() => addToPlaylist(p, ids)) })),
      ],
    },
    {
      label: "Rating",
      submenu: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10].map((r) => ({
        label: r ? starsText(r) : "No rating",
        action: () => run(() => setRating(ids, r)),
      })),
    },
    { label: ids.length > 1 ? `Edit tags (${ids.length} tracks)…` : "Edit tags…", action: () => openTagEditor(ids.map((id) => store.tracks.get(id)!)) },
    {
      label: "Show in folder",
      disabled: ids.length !== 1,
      action: () => run(() => revealItemInDir(store.tracks.get(ids[0])!.path)),
    },
  ];
  const inbox = ids.filter((id) => store.tracks.get(id)?.inbox);
  if (inbox.length) {
    items.push(
      SEP,
      { label: "Add to Library", action: () => run(() => clearInbox(inbox)) },
      { label: "Remove and ignore…", action: () => run(() => ignoreInboxTracks(inbox)) },
    );
  }
  // Removing works on selected rows, so it's left out of group menus.
  if (!group && view.kind === "queue") {
    items.push(SEP, { label: "Remove from queue", action: () => removeSelected() });
  } else if (!group && pl?.kind === "manual") {
    items.push(SEP, { label: "Remove from playlist", action: () => removeSelected() });
  }
  openMenu(x, y, items);
}

/** A shuffled copy (Fisher-Yates), for playing a whole artist or album in random order. */
function shuffled(ids: number[]): number[] {
  const out = [...ids];
  for (let i = out.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [out[i], out[j]] = [out[j], out[i]];
  }
  return out;
}

/** Add Inbox tracks to the library (all of them when `ids` is null). */
export async function clearInbox(ids: number[] | null = null) {
  await api.clearInbox(ids);
  await markAccepted(ids ?? [...store.tracks.values()].filter((t) => t.inbox).map((t) => t.id));
}

/** Reflect tracks leaving the Inbox (the backend has already been told). */
async function markAccepted(ids: number[]) {
  const done = ids.filter((id) => store.tracks.get(id)?.inbox);
  if (!done.length) return;
  store.updateTracks(done.map((id) => ({ ...store.tracks.get(id)!, inbox: false })));
  if (store.view.kind === "inbox" || store.view.kind === "library") await store.refreshView("keepScroll");
}

/** Take Inbox tracks out of Honeycomb for good: the files stay, and scans skip them. */
export async function ignoreInboxTracks(ids: number[]) {
  const n = ids.length;
  const what = n === 1 ? "this track" : `these ${n} tracks`;
  const ok = await ask(
    `Remove ${what} from Honeycomb? The files stay on disk, and rescans will skip them. You can bring them back in Settings → Library → Ignored files.`,
    { title: "Remove and ignore", kind: "warning" },
  );
  if (!ok) return;
  const removed = await api.ignoreInboxTracks(ids);
  await store.removeTracks(removed);
  toast(`Removed ${removed.length} track${removed.length === 1 ? "" : "s"}; rescans will skip ${removed.length === 1 ? "it" : "them"}`);
}

async function setRating(ids: number[], rating: number) {
  await api.setRating(ids, rating);
  store.updateTracks(ids.map((id) => ({ ...store.tracks.get(id)!, rating })));
}

/** Remove the selected rows from the queue or the open manual playlist. */
export function removeSelected() {
  const rows = [...store.selection].map((i) => store.visibleSource[i]);
  if (!rows.length) return;
  if (store.view.kind === "queue") {
    run(() => api.removeFromQueue(rows));
    return;
  }
  const pl = store.currentPlaylist;
  if (pl?.kind !== "manual") return;
  const drop = new Set(rows);
  const ids = store.viewIds.filter((_, i) => !drop.has(i));
  run(async () => {
    await api.setPlaylistTracks(pl.id, ids);
    store.viewIds = ids;
    store.recompute();
  });
}

export async function addToPlaylist(p: Playlist, ids: number[]) {
  await api.addToPlaylist(p.id, ids);
  toast(`Added ${ids.length} track${ids.length === 1 ? "" : "s"} to ${p.name}`);
  await markAccepted(ids);
  if (store.currentPlaylist?.id === p.id) await store.refreshView();
}

export async function newPlaylist(ids: number[] = []) {
  const name = await promptText("New playlist", "Name", "New playlist");
  if (!name) return;
  const p = await api.createPlaylist(name, null, ids);
  await markAccepted(ids);
  await store.loadPlaylists();
  if (!ids.length) await store.setView({ kind: "playlist", id: p.id });
  else toast(`Created ${name} with ${ids.length} track${ids.length === 1 ? "" : "s"}`);
}

export async function importPlaylist() {
  const path = await open({ multiple: false, filters: [{ name: "Playlists", extensions: ["m3u", "m3u8"] }] });
  if (typeof path !== "string") return;
  const p = await api.importM3u(path);
  await store.loadPlaylists();
  await store.setView({ kind: "playlist", id: p.id });
}

export function openPlaylistMenu(x: number, y: number, p: Playlist) {
  openMenu(x, y, [
    {
      label: "Play",
      action: () => run(async () => api.load(await api.playlistTrackIds(p.id), 0)),
    },
    SEP,
    {
      label: "Rename…",
      action: () =>
        run(async () => {
          const name = await promptText("Rename playlist", "Name", p.name);
          if (!name) return;
          await api.updatePlaylist(p.id, name);
          await store.loadPlaylists();
        }),
    },
    ...(p.kind === "smart" ? [{ label: "Edit rules…", action: () => openSmartEditor(p) }] : []),
    {
      label: "Export as M3U…",
      action: () =>
        run(async () => {
          const path = await save({ defaultPath: `${p.name}.m3u8`, filters: [{ name: "Playlist", extensions: ["m3u8", "m3u"] }] });
          if (!path) return;
          await api.exportM3u(p.id, path);
          toast(`Exported ${p.name}`);
        }),
    },
    SEP,
    {
      label: "Delete",
      action: () =>
        run(async () => {
          if (!(await ask(`Delete the playlist "${p.name}"? Your music files are not affected.`, { title: "Delete playlist", kind: "warning" }))) return;
          await api.deletePlaylist(p.id);
          await store.loadPlaylists();
          if (store.currentPlaylist === undefined && store.view.kind === "playlist") {
            await store.setView({ kind: "library" });
          }
        }),
    },
  ]);
}
