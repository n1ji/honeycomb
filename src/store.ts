import { api, Playlist, QueueState, Signal, Tick, Track } from "./api";

export type View = { kind: "library" } | { kind: "inbox" } | { kind: "queue" } | { kind: "playlist"; id: number };

export type SortKey = keyof Track;

type Keep = "reset" | "keepScroll" | "keepAll";

export interface Filters {
  genre: string | null;
  artist: string | null;
  album: string | null;
}

type Topic = "tracks" | "view" | "visible" | "selection" | "playlists" | "queue" | "tick" | "signal" | "settings";

class Store {
  tracks = new Map<number, Track>();
  playlists: Playlist[] = [];
  settings: Record<string, string> = {};

  view: View = { kind: "library" };
  /** Track ids that make up the current view, before browser filters and search. */
  viewIds: number[] = [];
  filters: Filters = { genre: null, artist: null, album: null };
  search = "";
  sort: { key: SortKey; desc: boolean } | null = null;

  /** Rows shown in the track list, after filters, search and sort. */
  visible: Track[] = [];
  /** Position of each visible row in viewIds (needed to edit playlists/queue). */
  visibleSource: number[] = [];
  /** Indices into `visible`. */
  selection = new Set<number>();
  anchor = -1;
  /** True when the last recompute replaced the list (scroll back to top). */
  listReset = true;

  /** Bumped whenever track data changes; caches below are keyed on it. */
  tracksVersion = 0;
  /** Bumped whenever `viewIds` changes. */
  viewVersion = 0;
  private cache: SortCache | null = null;

  queue: QueueState = { trackIds: [], current: null, shuffle: false, repeat: "off", volume: 0.8, error: null };
  tick: Tick = { trackId: null, position: 0, duration: 0, playing: false };
  signal: Signal | null = null;

  private subs = new Map<Topic, Set<() => void>>();

  on(topic: Topic, fn: () => void) {
    if (!this.subs.has(topic)) this.subs.set(topic, new Set());
    this.subs.get(topic)!.add(fn);
  }

  emit(topic: Topic) {
    this.subs.get(topic)?.forEach((fn) => fn());
  }

  // ---------- loading ----------

  /** Reload the library. `background` keeps the scroll position (used while scanning). */
  async loadTracks(background = false) {
    const list = await api.getTracks();
    this.tracks = new Map(list.map((t) => [t.id, t]));
    this.tracksVersion++;
    this.emit("tracks");
    await this.refreshView(background ? "keepScroll" : "reset");
  }

  async loadPlaylists() {
    this.playlists = await api.getPlaylists();
    this.emit("playlists");
  }

  updateTracks(list: Track[]) {
    for (const t of list) this.tracks.set(t.id, t);
    this.tracksVersion++;
    this.emit("tracks");
    this.recompute("keepAll");
  }

  // ---------- view ----------

  async setView(view: View) {
    this.view = view;
    this.filters = { genre: null, artist: null, album: null };
    this.sort = null;
    this.selection.clear();
    this.emit("view");
    await this.refreshView();
  }

  /** Reload the ids behind the current view (e.g. after a playlist changed). */
  async refreshView(mode: Keep = "reset") {
    const v = this.view;
    if (v.kind === "library") {
      this.viewIds = this.sortCache().libraryIds;
    } else if (v.kind === "inbox") {
      // Newest first.
      const pos = this.sortCache().pos;
      this.viewIds = [...this.tracks.values()]
        .filter((t) => t.inbox)
        .sort((a, b) => b.dateAdded - a.dateAdded || pos.get(a.id)! - pos.get(b.id)!)
        .map((t) => t.id);
    } else if (v.kind === "queue") {
      this.viewIds = this.queue.trackIds;
    } else {
      this.viewIds = await api.playlistTrackIds(v.id);
    }
    this.viewVersion++;
    this.recompute(mode);
  }

  get currentPlaylist(): Playlist | undefined {
    const v = this.view;
    return v.kind === "playlist" ? this.playlists.find((p) => p.id === v.id) : undefined;
  }

  /** Rows of the view before the browser filters are applied. */
  baseRows(): { t: Track; src: number }[] {
    const out: { t: Track; src: number }[] = [];
    this.viewIds.forEach((id, src) => {
      const t = this.tracks.get(id);
      if (t) out.push({ t, src });
    });
    return out;
  }

  /**
   * Rebuild the visible rows. "reset" clears the selection and scrolls to the top,
   * "keepScroll" clears only the selection, "keepAll" keeps both.
   */
  recompute(mode: Keep = "reset") {
    let rows = this.baseRows();
    const f = this.filters;
    if (f.genre !== null) rows = rows.filter((r) => r.t.genre === f.genre);
    if (f.artist !== null) rows = rows.filter((r) => artistOf(r.t) === f.artist);
    if (f.album !== null) rows = rows.filter((r) => r.t.album === f.album);

    const q = this.search.trim().toLowerCase();
    if (q) {
      const words = q.split(/\s+/);
      const hay = this.sortCache().searchText;
      rows = rows.filter((r) => {
        const text = hay.get(r.t.id) ?? "";
        return words.every((w) => text.includes(w));
      });
    }

    if (this.sort) {
      const { key, desc } = this.sort;
      const cache = this.sortCache();
      const value = cache.valueOf(key);
      const d = desc ? -1 : 1;
      // Sort on plain numbers computed once per row, ties in library order.
      const keyed = rows.map((r) => ({ r, v: value(r.t), p: cache.pos.get(r.t.id) ?? 0 }));
      keyed.sort((a, b) => (a.v === b.v ? a.p - b.p : a.v < b.v ? -d : d));
      rows = keyed.map((k) => k.r);
    }

    this.visible = rows.map((r) => r.t);
    this.visibleSource = rows.map((r) => r.src);
    this.listReset = mode === "reset";
    if (mode !== "keepAll") {
      this.selection.clear();
      this.anchor = -1;
    }
    this.emit("visible");
    this.emit("selection");
  }

  /** Show the library narrowed to an artist, album or genre (from the sidebar). */
  async showFiltered(filters: Partial<Filters>) {
    if (this.view.kind !== "library") await this.setView({ kind: "library" });
    this.filters = { genre: null, artist: null, album: null, ...filters };
    this.recompute();
  }

  setFilter(key: keyof Filters, value: string | null) {
    this.filters[key] = value;
    // Narrowing an outer pane resets the panes to its right.
    if (key === "genre") this.filters.artist = this.filters.album = null;
    if (key === "artist") this.filters.album = null;
    this.recompute();
  }

  setSort(key: SortKey) {
    if (this.sort?.key === key) {
      this.sort = this.sort.desc ? null : { key, desc: true };
    } else {
      this.sort = { key, desc: false };
    }
    this.recompute();
  }

  selectedTracks(): Track[] {
    return [...this.selection].sort((a, b) => a - b).map((i) => this.visible[i]).filter(Boolean);
  }

  selectedIds(): number[] {
    return this.selectedTracks().map((t) => t.id);
  }

  // ---------- player ----------

  setQueue(q: QueueState) {
    this.queue = q;
    this.emit("queue");
    if (this.view.kind === "queue") {
      this.viewIds = q.trackIds;
      this.viewVersion++;
      this.recompute("keepAll");
    }
  }

  get nowPlaying(): Track | undefined {
    return this.tick.trackId !== null ? this.tracks.get(this.tick.trackId) : undefined;
  }

  /** Sort ranks and search text for the current tracks, rebuilt when they change. */
  sortCache(): SortCache {
    if (this.cache?.version !== this.tracksVersion) this.cache = new SortCache(this.tracks, this.tracksVersion);
    return this.cache;
  }

  /** Play the visible list starting at row `index`. */
  async playVisible(index: number) {
    if (this.view.kind === "queue") {
      await api.cmd("jump", this.visibleSource[index]);
    } else {
      await api.load(
        this.visible.map((t) => t.id),
        index,
      );
    }
  }
}

export function artistOf(t: Track): string {
  return t.albumArtist || t.artist;
}

/**
 * One collator for every text comparison: case- and accent-insensitive.
 * (`localeCompare` with options builds a new one on every call, which made
 * sorting 100k tracks take seconds.)
 */
export const collator = new Intl.Collator(undefined, { sensitivity: "base" });

/** Rank of each distinct text value in collation order; equal-looking values share a rank. */
function ranks(values: Iterable<string>): Map<string, number> {
  const distinct = [...new Set(values)].filter((v) => v !== "").sort(collator.compare);
  const out = new Map<string, number>();
  let rank = 0;
  distinct.forEach((v, i) => {
    if (i > 0 && collator.compare(distinct[i - 1], v) !== 0) rank++;
    out.set(v, rank);
  });
  return out;
}

/**
 * Precomputed sort keys, so sorting compares numbers instead of text. Empty
 * and missing values sort last (Infinity).
 */
export class SortCache {
  /** Library order: album artist → album → disc → track number → title. */
  libraryIds: number[];
  /** Each track's position in library order (the tie-breaker for every sort). */
  pos = new Map<number, number>();
  /** Lowercased text that search matches against. */
  searchText = new Map<number, string>();
  private textRanks = new Map<string, Map<string, number>>();

  constructor(
    private tracks: Map<number, Track>,
    readonly version: number,
  ) {
    const all = [...tracks.values()];
    const artist = this.rankOf((t) => artistOf(t), "artistOf");
    const album = this.rankOf((t) => t.album, "album");
    const title = this.rankOf((t) => t.title, "title");
    const keyed = all.map((t) => [artist(t), album(t), t.discNo ?? 0, t.trackNo ?? 0, title(t), t.id] as const);
    keyed.sort((a, b) => a[0] - b[0] || a[1] - b[1] || a[2] - b[2] || a[3] - b[3] || a[4] - b[4]);
    this.libraryIds = keyed.map((k) => k[5]);
    this.libraryIds.forEach((id, i) => this.pos.set(id, i));
    for (const t of all) {
      this.searchText.set(t.id, `${t.title} ${t.artist} ${t.albumArtist} ${t.album} ${t.genre}`.toLowerCase());
    }
  }

  /** A function giving a track's rank for a text field, computed once per field. */
  private rankOf(get: (t: Track) => string, name: string): (t: Track) => number {
    let r = this.textRanks.get(name);
    if (!r) {
      r = ranks([...this.tracks.values()].map(get));
      this.textRanks.set(name, r);
    }
    const map = r;
    return (t) => map.get(get(t)) ?? Infinity;
  }

  /** A numeric sort value for any column. */
  valueOf(key: SortKey): (t: Track) => number {
    const sample = [...this.tracks.values()].find((t) => t[key] !== null && t[key] !== undefined)?.[key];
    if (typeof sample === "string") return this.rankOf((t) => t[key] as string, key);
    return (t) => {
      const v = t[key];
      return v === null || v === undefined ? Infinity : Number(v);
    };
  }
}

export const store = new Store();
