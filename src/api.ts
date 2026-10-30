import { convertFileSrc, invoke } from "@tauri-apps/api/core";

export interface Track {
  id: number;
  path: string;
  title: string;
  artist: string;
  albumArtist: string;
  album: string;
  genre: string;
  composer: string;
  comment: string;
  year: number | null;
  trackNo: number | null;
  discNo: number | null;
  duration: number;
  bitrate: number | null;
  sampleRate: number | null;
  format: string;
  /** Half-stars, 0-10. */
  rating: number;
  playCount: number;
  lastPlayed: number | null;
  dateAdded: number;
  mtime: number;
  hasArt: boolean;
  loved: boolean;
  /** Newly found and not reviewed yet. */
  inbox: boolean;
  /** Bits per sample, for lossless files. */
  bitDepth: number | null;
}

/** A file removed from the Inbox that scans skip. */
export interface IgnoredFile {
  path: string;
  /** Unix seconds. */
  ignoredAt: number;
}

export interface SmartRule {
  field: string;
  op: string;
  value: string;
}

export interface SmartRules {
  match: "all" | "any";
  rules: SmartRule[];
  limit: number | null;
  sortBy: string | null;
  sortDesc: boolean;
}

export interface Playlist {
  id: number;
  name: string;
  kind: "manual" | "smart";
  rules: SmartRules | null;
}

export type Repeat = "off" | "all" | "one";

export interface QueueState {
  trackIds: number[];
  current: number | null;
  shuffle: boolean;
  repeat: Repeat;
  volume: number;
  error: string | null;
}

/** What's playing and how it reaches the speakers (`player-signal`). */
export interface Signal {
  trackId: number;
  source: { codec: string; lossless: boolean; bits: number | null; rate: number | null; channels: number | null } | null;
  /** Null while the output is closed. */
  output: { device: string | null; rate: number; channels: number; format: string } | null;
}

export interface Tick {
  trackId: number | null;
  position: number;
  duration: number;
  playing: boolean;
}

/** Fields left undefined are not changed; "" clears a field. */
export interface TagEdit {
  title?: string;
  artist?: string;
  albumArtist?: string;
  album?: string;
  genre?: string;
  composer?: string;
  comment?: string;
  year?: string;
  trackNo?: string;
  discNo?: string;
  artPath?: string;
  removeArt?: boolean;
}

export interface ImportPreview {
  libraryTracks: number;
  stats: {
    tracks: number;
    matched: number;
    unmatched: number;
    withStats: number;
    rated: number;
    played: number;
    loved: number;
  } | null;
  playlists: {
    index: number;
    name: string;
    source: string;
    total: number;
    matched: number;
    placeholders: number;
    missing: number;
    exists: boolean;
  }[];
  report: string;
}

export interface ImportOutcome {
  statsUpdated: number;
  playlistsCreated: number;
  playlistsReplaced: number;
  placeholdersSkipped: number;
  missingSkipped: number;
}

export interface UpdateStatus {
  state: "upToDate" | "behind" | "ahead" | "diverged" | "unknown";
  behind: number;
  ahead: number;
  local: string;
  remote: string;
  repo: string;
  message: string;
  /** "checkout": compared with GitHub's main through git; "release": with the newest release. */
  source: "checkout" | "release";
  /** The newest release's page (release checks only). */
  url: string;
  /** The newest release's signed update manifest, when it can be installed from inside the app. */
  updateUrl: string;
}

export interface SyncConvert {
  format: "mp3" | "aac" | "vorbis" | "flac16";
  quality: "high" | "smaller";
}

export interface SyncProfile {
  /** Folder on the device that the music goes into. */
  dest: string;
  wholeLibrary: boolean;
  playlists: number[];
  /** Remove synced files that are no longer chosen. */
  removeUnselected: boolean;
  /** Convert lossless files to this; missing or null copies everything as it is. */
  convert?: SyncConvert | null;
}

export interface SyncTarget {
  exists: boolean;
  rockbox: boolean;
  freeBytes: number | null;
  syncedFiles: number;
}

export interface SyncReport {
  copied: number;
  unchanged: number;
  removed: number;
  playlists: number;
  bytesCopied: number;
  playsImported: number;
  failed: string[];
  cancelled: boolean;
}

export interface Lyrics {
  source: "embedded" | "file" | "lrclib" | "none";
  synced: boolean;
  instrumental: boolean;
  lines: { time: number | null; text: string }[];
}

export const api = {
  /** The whole library. It arrives as field names plus one array per track (much smaller than objects). */
  getTracks: async () => {
    const table = await invoke<{ fields: string[]; rows: unknown[][] }>("get_tracks");
    const { fields, rows } = table;
    return rows.map((row) => {
      const t: Record<string, unknown> = {};
      for (let i = 0; i < fields.length; i++) t[fields[i]] = row[i];
      return t as unknown as Track;
    });
  },
  getFolders: () => invoke<string[]>("get_folders"),
  addFolder: (path: string) => invoke<void>("add_folder", { path }),
  removeFolder: (path: string) => invoke<void>("remove_folder", { path }),
  rescan: () => invoke<void>("rescan"),
  setRating: (ids: number[], rating: number) => invoke<void>("set_rating", { ids, rating }),
  /** Add Inbox tracks to the library; all of them when `ids` is null. */
  clearInbox: (ids: number[] | null = null) => invoke<void>("clear_inbox", { ids }),
  /** Remove Inbox tracks (files stay on disk) and skip them on later scans. Returns the removed ids. */
  ignoreInboxTracks: (ids: number[]) => invoke<number[]>("ignore_inbox_tracks", { ids }),
  ignoredFiles: () => invoke<IgnoredFile[]>("ignored_files"),
  /** Stop ignoring files; a rescan brings them back into the Inbox. */
  unignoreFiles: (paths: string[]) => invoke<void>("unignore_files", { paths }),
  editTags: (ids: number[], edit: TagEdit) => invoke<Track[]>("edit_tags", { ids, edit }),

  getSettings: () => invoke<Record<string, string>>("get_settings"),
  setSetting: (key: string, value: string) => invoke<void>("set_setting", { key, value }),

  getPlaylists: () => invoke<Playlist[]>("get_playlists"),
  createPlaylist: (name: string, rules: SmartRules | null = null, trackIds: number[] | null = null) =>
    invoke<Playlist>("create_playlist", { name, rules, trackIds }),
  updatePlaylist: (id: number, name: string, rules: SmartRules | null = null) =>
    invoke<void>("update_playlist", { id, name, rules }),
  deletePlaylist: (id: number) => invoke<void>("delete_playlist", { id }),
  playlistTrackIds: (id: number) => invoke<number[]>("playlist_track_ids", { id }),
  previewSmart: (rules: SmartRules) => invoke<number>("preview_smart", { rules }),
  setPlaylistTracks: (id: number, trackIds: number[]) => invoke<void>("set_playlist_tracks", { id, trackIds }),
  addToPlaylist: (id: number, trackIds: number[]) => invoke<void>("add_to_playlist", { id, trackIds }),
  exportM3u: (id: number, path: string) => invoke<void>("export_m3u", { id, path }),
  importM3u: (path: string) => invoke<Playlist>("import_m3u", { path }),
  importPreview: (xmlPath: string | null, m3uPaths: string[]) =>
    invoke<ImportPreview>("import_preview", { xmlPath, m3uPaths }),
  importApply: (xmlPath: string | null, m3uPaths: string[], importStats: boolean, playlists: number[]) =>
    invoke<ImportOutcome>("import_apply", { xmlPath, m3uPaths, importStats, playlists }),
  saveText: (path: string, text: string) => invoke<void>("save_text", { path, text }),
  artworkExt: (trackId: number) => invoke<string | null>("artwork_ext", { trackId }),
  saveArtwork: (trackId: number, dest: string) => invoke<void>("save_artwork", { trackId, dest }),
  lyrics: (trackId: number) => invoke<Lyrics>("get_lyrics", { trackId }),
  syncTarget: (dest: string) => invoke<SyncTarget>("sync_target", { dest }),
  syncRun: (profile: SyncProfile) => invoke<SyncReport>("sync_run", { profile }),
  syncCancel: () => invoke<void>("sync_cancel"),
  /** Which conversion formats this system can make. */
  syncFormats: () => invoke<{ aac: boolean }>("sync_formats"),
  uiReady: (tracks: number) => invoke<void>("ui_ready", { tracks }),
  updateStatus: (refresh = false) => invoke<UpdateStatus>("update_status", { refresh }),
  /** Downloads, installs and restarts; resolves only if something went wrong before the restart. */
  updateInstall: () => invoke<void>("update_install"),

  load: (trackIds: number[], index: number) => invoke<void>("player_load", { trackIds, index }),
  enqueue: (trackIds: number[], next: boolean) => invoke<void>("player_enqueue", { trackIds, next }),
  cmd: (cmd: string, value?: number) => invoke<void>("player_cmd", { cmd, value }),
  removeFromQueue: (indices: number[]) => invoke<void>("player_remove", { indices }),
};

const artVersions = new Map<number, number>();

export function artUrl(id: number): string {
  const v = artVersions.get(id) ?? 0;
  return convertFileSrc(String(id), "art") + (v ? `?v=${v}` : "");
}

/** Force reloading a track's artwork after it changed. */
export function bumpArt(id: number) {
  artVersions.set(id, (artVersions.get(id) ?? 0) + 1);
}
