import { api, Playlist, QueueState, Tick, Track } from "./api";

export type View = { kind: "library" } | { kind: "inbox" } | { kind: "queue" } | { kind: "playlist"; id: number };

export type SortKey = keyof Track;

type Keep = "reset" | "keepScroll" | "keepAll";

export interface Filters {
  genre: string | null;
  artist: string | null;
  album: string | null;
}

type Topic = "tracks" | "view" | "visible" | "selection" | "playlists" | "queue" | "tick" | "settings";

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

  queue: QueueState = { trackIds: [], current: null, shuffle: false, repeat: "off", volume: 0.8, error: null };
  tick: Tick = { trackId: null, position: 0, duration: 0, playing: false };

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
    this.emit("tracks");
    await this.refreshView(background ? "keepScroll" : "reset");
  }

  async loadPlaylists() {
    this.playlists = await api.getPlaylists();
    this.emit("playlists");
  }

  updateTracks(list: Track[]) {
    for (const t of list) this.tracks.set(t.id, t);
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
      this.viewIds = [...this.tracks.values()].sort(libraryOrder).map((t) => t.id);
    } else if (v.kind === "inbox") {
      // Newest first.
      this.viewIds = [...this.tracks.values()]
        .filter((t) => t.inbox)
        .sort((a, b) => b.dateAdded - a.dateAdded || libraryOrder(a, b))
        .map((t) => t.id);
    } else if (v.kind === "queue") {
      this.viewIds = this.queue.trackIds;
    } else {
      this.viewIds = await api.playlistTrackIds(v.id);
    }
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
      rows = rows.filter((r) => {
        const hay = `${r.t.title} ${r.t.artist} ${r.t.albumArtist} ${r.t.album} ${r.t.genre}`.toLowerCase();
        return words.every((w) => hay.includes(w));
      });
    }

    if (this.sort) {
      const { key, desc } = this.sort;
      rows.sort((a, b) => cmp(a.t[key], b.t[key]) * (desc ? -1 : 1) || libraryOrder(a.t, b.t));
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
      this.recompute("keepAll");
    }
  }

  get nowPlaying(): Track | undefined {
    return this.tick.trackId !== null ? this.tracks.get(this.tick.trackId) : undefined;
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

function cmp(a: unknown, b: unknown): number {
  if (a === b) return 0;
  if (a === null || a === undefined || a === "") return 1;
  if (b === null || b === undefined || b === "") return -1;
  if (typeof a === "string" && typeof b === "string") return a.localeCompare(b, undefined, { sensitivity: "base" });
  return (a as number) < (b as number) ? -1 : 1;
}

/** Album artist → album → disc → track number → title. */
export function libraryOrder(a: Track, b: Track): number {
  return (
    cmp(artistOf(a), artistOf(b)) ||
    cmp(a.album, b.album) ||
    cmp(a.discNo ?? 0, b.discNo ?? 0) ||
    cmp(a.trackNo ?? 0, b.trackNo ?? 0) ||
    cmp(a.title, b.title)
  );
}

export const store = new Store();
