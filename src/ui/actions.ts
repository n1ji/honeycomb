import { ask, open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, Playlist } from "../api";
import { store } from "../store";
import { starsText, toast } from "../util";
import { promptText } from "./dialogs";
import { MenuItem, openMenu, SEP } from "./menu";
import { openSmartEditor } from "./smarteditor";
import { openTagEditor } from "./tageditor";
import { openSync, SyncPick } from "./sync";

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
 * `pick` names the artists or albums the group stands for, so they can be
 * added to a device's sync list.
 */
export function openTrackMenu(x: number, y: number, group?: number[], pick?: SyncPick) {
  const ids = group ?? store.selectedIds();
  if (!ids.length) return;
  const manual = store.playlists.filter((p) => p.kind === "manual");
  const view = store.view;
  const pl = store.currentPlaylist;
  // Songs on a server have no file to edit or show, and whole-star ratings (C3).
  const remote = ids.filter((id) => store.remote.has(id));
  const allRemote = remote.length === ids.length;

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
    ...(pick && (pick.artists.length || pick.albums.length) ? [{ label: `Sync ${pickName(pick)} to a device…`, action: () => openSync(pick) }] : []),
    {
      label: "Rating",
      submenu: (remote.length ? [0, 2, 4, 6, 8, 10] : [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]).map((r) => ({
        label: r ? starsText(r) : "No rating",
        action: () => run(() => setRating(ids, r)),
      })),
    },
    (() => {
      const loved = ids.every((id) => store.tracks.get(id)?.loved);
      return { label: loved ? "Unlove" : "Love", action: () => run(() => setLoved(ids, !loved)) };
    })(),
    {
      label: ids.length > 1 ? `Edit tags (${ids.length} tracks)…` : "Edit tags…",
      disabled: allRemote,
      action: () => openTagEditor(ids.map((id) => store.tracks.get(id)!).filter((t) => !store.remote.has(t.id))),
    },
    {
      label: "Show in folder",
      disabled: ids.length !== 1 || allRemote,
      action: () => run(() => revealItemInDir(store.tracks.get(ids[0])!.path)),
    },
  ];
  if (remote.length) items.push(SEP, ...offlineItems(remote, !!(group && pick?.albums.length)));
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

/** Keeping server songs on this computer, or letting them go. */
function offlineItems(ids: number[], albums: boolean): MenuItem[] {
  const online = ids.filter((id) => store.remote.get(id) === "remote");
  const kept = ids.filter((id) => store.remote.get(id) !== "remote");
  const out: MenuItem[] = [];
  if (online.length) {
    out.push({
      label: albums ? "Keep album offline" : "Make available offline",
      action: () =>
        run(async () => {
          await api.remotePin(ids, albums);
          await store.loadRemote();
          toast(`Downloading ${online.length === 1 ? "1 song" : `${online.length} songs`} to this computer…`);
        }),
    });
  }
  if (kept.length) {
    out.push({
      label: "Remove offline copy",
      action: () =>
        run(async () => {
          const gone = kept.filter((id) => store.remote.get(id) === "local-only").length;
          if (gone && !(await ask(`${gone === 1 ? "1 song is" : `${gone} songs are`} no longer on the server, so removing the copy removes the song from your library. Remove anyway?`, { title: "Remove offline copy", kind: "warning" })))
            return;
          await api.remoteUnpin(kept);
        }),
    });
  }
  return out;
}

/** "this artist", "these 3 albums" and so on. */
function pickName(pick: SyncPick): string {
  const n = pick.artists.length + pick.albums.length;
  const what = pick.albums.length === 0 ? "artist" : pick.artists.length === 0 ? "album" : "item";
  return n === 1 ? `this ${what}` : `these ${n} ${what}s`;
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

export async function setLoved(ids: number[], loved: boolean) {
  await api.setLoved(ids, loved);
  store.updateTracks(ids.map((id) => ({ ...store.tracks.get(id)!, loved })));
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
