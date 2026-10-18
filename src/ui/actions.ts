import { ask, open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, Playlist } from "../api";
import { store } from "../store";
import { toast } from "../util";
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

export function openTrackMenu(x: number, y: number) {
  const ids = store.selectedIds();
  if (!ids.length) return;
  const manual = store.playlists.filter((p) => p.kind === "manual");
  const view = store.view;
  const pl = store.currentPlaylist;

  const items: MenuItem[] = [
    { label: "Play", action: () => run(() => api.load(ids, 0)) },
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
      submenu: [0, 1, 2, 3, 4, 5].map((r) => ({
        label: r ? "★".repeat(r) : "No rating",
        action: () => run(() => setRating(ids, r)),
      })),
    },
    { label: ids.length > 1 ? `Edit tags (${ids.length} tracks)…` : "Edit tags…", action: () => openTagEditor(store.selectedTracks()) },
    {
      label: "Show in folder",
      disabled: ids.length !== 1,
      action: () => run(() => revealItemInDir(store.tracks.get(ids[0])!.path)),
    },
  ];
  if (view.kind === "queue") {
    items.push(SEP, { label: "Remove from queue", action: () => removeSelected() });
  } else if (pl?.kind === "manual") {
    items.push(SEP, { label: "Remove from playlist", action: () => removeSelected() });
  }
  openMenu(x, y, items);
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
  if (store.currentPlaylist?.id === p.id) await store.refreshView();
}

export async function newPlaylist(ids: number[] = []) {
  const name = await promptText("New playlist", "Name", "New playlist");
  if (!name) return;
  const p = await api.createPlaylist(name, null, ids);
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
