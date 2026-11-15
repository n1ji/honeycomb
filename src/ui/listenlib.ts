import { Track } from "../api";
import { artistOf, store } from "../store";

export interface Album {
  key: string;
  title: string;
  artist: string;
  year: number | null;
  /** In album order: disc, track number, title. */
  ids: number[];
  /** A track whose artwork stands for the album. */
  artId: number | null;
  added: number;
  duration: number;
  /** Lowercased title and artist, filled in the first time a search needs it. */
  text?: string;
}

export interface Artist {
  name: string;
  albums: Album[];
  songs: number;
  artId: number | null;
}

export interface Library {
  /** By artist, then album. */
  albums: Album[];
  artists: Artist[];
  songs: number[];
  byKey: Map<string, Album>;
  byName: Map<string, Artist>;
}

export const albumKey = (t: Track) => `${t.album}\u0000${artistOf(t)}`;
export const albumTitle = (a: Album) => a.title || "Unknown album";
export const artistName = (name: string) => name || "Unknown artist";

let cached: Library | null = null;
let stamp = "";

/**
 * Albums and artists of the shown library (no Inbox, no hidden libraries),
 * grouped in one pass over the library order and kept until tracks change.
 */
export function library(): Library {
  const now = `${store.tracksVersion}|${store.settings.hiddenLibraries ?? ""}`;
  if (cached && now === stamp) return cached;
  const lib: Library = { albums: [], artists: [], songs: [], byKey: new Map(), byName: new Map() };
  for (const id of store.sortCache().libraryIds) {
    const t = store.tracks.get(id)!;
    if (t.inbox || !store.shown(t)) continue;
    lib.songs.push(id);
    const key = albumKey(t);
    let album = lib.byKey.get(key);
    if (!album) {
      album = { key, title: t.album, artist: artistOf(t), year: null, ids: [], artId: null, added: 0, duration: 0 };
      lib.byKey.set(key, album);
      lib.albums.push(album);
      let artist = lib.byName.get(album.artist);
      if (!artist) {
        artist = { name: album.artist, albums: [], songs: 0, artId: null };
        lib.byName.set(album.artist, artist);
        lib.artists.push(artist);
      }
      artist.albums.push(album);
    }
    album.ids.push(id);
    album.duration += t.duration;
    if (t.dateAdded > album.added) album.added = t.dateAdded;
    if (album.year === null && t.year) album.year = t.year;
    if (album.artId === null && t.hasArt) album.artId = id;
  }
  for (const a of lib.artists) {
    for (const al of a.albums) {
      a.songs += al.ids.length;
      a.artId ??= al.artId;
    }
  }
  cached = lib;
  stamp = now;
  return lib;
}
