import { api, artUrl, Playlist, Track } from "../api";
import { artistOf, collator, store } from "../store";
import { $, fmtTime, fmtTotal, h, heart } from "../util";
import { addToPlaylist, newPlaylist, offlineItems, run, setLoved, shuffled } from "./actions";
import { promptText } from "./dialogs";
import { saveSetting } from "./layout";
import { Album, albumKey, albumTitle, Artist, artistName, library, splitArtists, Library } from "./listenlib";
import { mountNowView } from "./listennow";
import { MenuItem, openMenu, SEP } from "./menu";
import { mode, modeSwitch, nowView, setMode } from "./mode";
import { openSettings } from "./settings";
import { openLibrariesMenu } from "./sidebar";
import { openSync, syncLabel } from "./sync";
import { offlineMark } from "./tracklist";
import { vgrid, VGrid } from "./vgrid";

const TABS = { albums: "Albums", artists: "Artists", playlists: "Playlists", songs: "Songs" } as const;
type Tab = keyof typeof TABS;

type Page = { kind: "tab"; tab: Tab } | { kind: "album"; key: string } | { kind: "artist"; name: string } | { kind: "playlist"; id: number };

const ALBUM_SORTS = { artist: "By artist", title: "By title", added: "Recently added", year: "Newest first" } as const;
type AlbumSort = keyof typeof ALBUM_SORTS;

interface PageView {
  el: HTMLElement;
  grid?: VGrid;
}

const plural = (n: number, word: string) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;

/** A stable hue per name, for covers that have no artwork. */
function hue(name: string): number {
  let x = 7;
  for (let i = 0; i < name.length; i++) x = (x * 31 + name.charCodeAt(i)) >>> 0;
  return x % 360;
}

/** A square (or, for artists, hexagonal) cover; without artwork it shows the name's first letter on its own tint. */
function cover(artId: number | null, name: string, lazy = true): HTMLElement {
  const box = h("div", { class: "ln-cover", "data-letter": (name.trim()[0] ?? "♫").toUpperCase() });
  box.style.setProperty("--tint", `hsl(${hue(name)} 42% 52%)`);
  if (artId !== null) {
    const img = h("img", { alt: "", decoding: "async", draggable: "false" });
    img.addEventListener("load", () => img.classList.add("ready"));
    img.addEventListener("error", () => img.remove());
    if (lazy) img.dataset.src = artUrl(artId);
    else img.src = artUrl(artId);
    box.append(img);
  }
  return box;
}

/**
 * Listening mode: albums, artists, playlists and songs with search, a page
 * per album, artist and playlist, and the Now Playing view. It reads the same
 * store as Advanced mode and keeps no library state of its own.
 */
export function mountListening() {
  const root = $("#listen");
  const pageBox = h("div", { class: "ln-page" });
  const status = h("span", { class: "ln-status" });
  const search = h("input", { id: "listen-search", type: "search", class: "ln-search", placeholder: "Search", spellcheck: "false", autocomplete: "off" });
  const tabs = h("nav", { class: "ln-tabs" });
  const libsBtn = h("button", { class: "icon-btn", title: "Show or hide libraries", onclick: (e: Event) => openLibrariesMenu(e.currentTarget as HTMLElement) }, "⧉");
  root.append(
    h(
      "header",
      { class: "ln-bar" },
      h("div", { class: "brand" }, h("span", { class: "logo" }, "⬢"), h("span", { class: "label" }, "Honeycomb")),
      tabs,
      status,
      search,
      libsBtn,
      h("button", { class: "icon-btn", title: "Sync to a player or flash drive", onclick: () => openSync() }, h("span", { class: "ic" }, "⇅"), syncLabel()),
      h("button", { class: "icon-btn", title: "Settings", onclick: () => openSettings() }, h("span", { class: "ic" }, "⚙")),
      modeSwitch(),
    ),
    pageBox,
  );

  const startTab = store.settings["listen.tab"] as Tab;
  let page: Page = { kind: "tab", tab: startTab in TABS ? startTab : "albums" };
  const history: { page: Page; top: number }[] = [];
  let current: PageView = { el: h("div", {}) };
  let query = "";
  let dirty = true;
  let drawn = "";
  /** What a page's contents depend on; anything else (play counts, offline dots) is a repaint. */
  const stamp = () => `${store.tracksVersion}|${store.settings.hiddenLibraries ?? ""}|${store.folders.join("\n")}`;

  const now = mountNowView(root, {
    album: (t) => show({ kind: "album", key: albumKey(t) }),
    artist: (t) => show({ kind: "artist", name: artistOf(t) }),
    thumb: (t) => library().byKey.get(albumKey(t))?.artId ?? (t.hasArt ? t.id : null),
  });
  nowView.toggle = now.toggle;

  // ---------- navigation ----------

  function show(next: Page, push = true) {
    now.close();
    if (push) history.push({ page, top: current.grid?.el.scrollTop ?? 0 });
    page = next;
    if (next.kind === "tab") {
      history.length = 0;
      void saveSetting("listen.tab", next.tab);
    }
    draw(0);
  }

  function back() {
    const prev = history.pop();
    if (!prev) return show({ kind: "tab", tab: "albums" }, false);
    page = prev.page;
    draw(prev.top);
  }

  /** Build the page again from the store. `top` is where to scroll; undefined stays put. */
  function draw(top?: number) {
    if (mode() !== "listening") {
      dirty = true;
      return;
    }
    dirty = false;
    drawn = stamp();
    const at = top ?? current.grid?.el.scrollTop ?? 0;
    pendingTop = at;
    current = build();
    pageBox.replaceChildren(current.el);
    current.grid?.restore(at);
    renderTabs();
    libsBtn.style.display = store.libraries().length > 1 || store.hiddenLibraries().size ? "" : "none";
    libsBtn.classList.toggle("on", store.hiddenLibraries().size > 0);
  }

  function renderTabs() {
    const found = query ? results() : null;
    tabs.replaceChildren(
      ...(Object.keys(TABS) as Tab[]).map((t) =>
        h(
          "button",
          { type: "button", class: `ln-tab${page.kind === "tab" && page.tab === t ? " active" : ""}`, onclick: () => show({ kind: "tab", tab: t }, false) },
          TABS[t],
          found ? h("span", { class: "ln-tab-count" }, found[t].length.toLocaleString()) : null,
        ),
      ),
    );
  }

  // ---------- search ----------

  let found: { key: string; albums: Album[]; artists: Artist[]; playlists: Playlist[]; songs: number[] } | null = null;

  /** What the search matches in each tab, worked out once per query. */
  function results() {
    const lib = library();
    const key = `${query}\u0000${store.tracksVersion}\u0000${store.settings.hiddenLibraries ?? ""}\u0000${store.playlists.length}`;
    if (found?.key === key) return found;
    const words = query.split(/\s+/);
    const has = (text: string) => words.every((w) => text.includes(w));
    const text = store.sortCache().searchText;
    found = {
      key,
      albums: lib.albums.filter((a) => has((a.text ??= `${a.title} ${a.artist}`.toLowerCase()))),
      artists: lib.artists.filter((a) => has(a.name.toLowerCase())),
      playlists: store.playlists.filter((p) => has(p.name.toLowerCase())),
      songs: lib.songs.filter((id) => has(text.get(id) ?? "")),
    };
    return found;
  }

  let searchTimer = 0;
  const applySearch = () => {
    query = search.value.trim().toLowerCase();
    if (page.kind !== "tab") {
      history.length = 0;
      page = { kind: "tab", tab: "albums" };
    }
    draw(0);
  };
  search.addEventListener("input", () => {
    clearTimeout(searchTimer);
    searchTimer = window.setTimeout(applySearch, 120);
  });
  search.addEventListener("change", () => {
    clearTimeout(searchTimer);
    if (search.value.trim().toLowerCase() !== query) applySearch();
  });
  search.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    e.stopPropagation();
    clearTimeout(searchTimer);
    search.value = "";
    search.blur();
    if (query) applySearch();
  });

  // ---------- playing and menus ----------

  const play = (ids: number[], index = 0) => run(() => api.load(ids, index));

  const sortedAlbums = new WeakMap<Library, Partial<Record<AlbumSort, Album[]>>>();
  function albumsBy(sort: AlbumSort): Album[] {
    const lib = library();
    let cache = sortedAlbums.get(lib);
    if (!cache) sortedAlbums.set(lib, (cache = { artist: lib.albums }));
    return (cache[sort] ??= [...lib.albums].sort(
      sort === "title" ? (a, b) => collator.compare(a.title, b.title) : sort === "added" ? (a, b) => b.added - a.added : (a, b) => (b.year ?? 0) - (a.year ?? 0),
    ));
  }

  const idsOfArtist = (a: Artist) => a.albums.flatMap((al) => al.ids);

  /** One link per credited artist; a name with no page of its own goes to `fallback`. */
  const artistLinks = (names: string, cls: string, fallback?: string) =>
    splitArtists(names).flatMap((n, i) => {
      const go = library().byName.has(n) || !fallback ? n : fallback;
      const link = h("span", { class: cls, onclick: (e: Event) => (e.stopPropagation(), show({ kind: "artist", name: go })) }, artistName(n));
      return i ? ["; ", link] : [link];
    });

  /** Songs of each playlist, fetched once until playlists or tracks change. */
  const playlistIds = new Map<number, Promise<number[]>>();
  const idsOfPlaylist = (id: number) => {
    let p = playlistIds.get(id);
    if (!p) playlistIds.set(id, (p = api.playlistTrackIds(id).then((ids) => ids.filter((t) => store.tracks.has(t)))));
    return p;
  };

  /** Listening actions only; anything about managing the library is one click away in Advanced mode. */
  function openSongMenu(x: number, y: number, ids: number[], on: { album?: Album; artist?: string; wholeAlbum?: boolean; manage: () => Promise<unknown> }) {
    if (!ids.length) return;
    const manual = store.playlists.filter((p) => p.kind === "manual");
    const loved = ids.every((id) => store.tracks.get(id)?.loved);
    const remote = ids.filter((id) => store.remote.has(id));
    const items: MenuItem[] = [
      { label: "Play", action: () => play(ids) },
      ...(ids.length > 1 ? [{ label: "Shuffle", action: () => play(shuffled(ids)) }] : []),
      { label: "Play next", action: () => run(() => api.enqueue(ids, true)) },
      { label: "Add to queue", action: () => run(() => api.enqueue(ids, false)) },
      SEP,
      {
        label: "Add to playlist",
        submenu: [
          { label: "New playlist…", action: () => run(() => newPlaylist(ids)) },
          ...(manual.length ? [SEP] : []),
          ...manual.map((p) => ({
            label: p.name,
            action: () =>
              run(async () => {
                await addToPlaylist(p, ids);
                playlistIds.delete(p.id);
              }),
          })),
        ],
      },
      { label: loved ? "Unlove" : "Love", action: () => run(() => setLoved(ids, !loved)) },
    ];
    const album = on.album;
    if (album || on.artist !== undefined) items.push(SEP);
    if (album) items.push({ label: "Go to album", action: () => show({ kind: "album", key: album.key }) });
    if (on.artist !== undefined) items.push({ label: "Go to artist", action: () => show({ kind: "artist", name: on.artist! }) });
    if (remote.length) items.push(SEP, ...offlineItems(remote, !!on.wholeAlbum));
    items.push(SEP, { label: "Open in Advanced mode", action: () => run(on.manage) });
    openMenu(x, y, items);
  }

  /** Advanced mode, with the track list narrowed to an artist or album. */
  const manageGroup = (artist: string, album?: string) => async () => {
    await setMode("advanced");
    await store.showFiltered(album === undefined ? { artist } : { artist, album });
  };

  const albumMenu = (e: MouseEvent, a: Album, here = false) => {
    e.preventDefault();
    openSongMenu(e.clientX, e.clientY, a.ids, { album: here ? undefined : a, artist: a.artist, wholeAlbum: true, manage: manageGroup(a.artist, a.title) });
  };

  // ---------- tiles ----------

  function tile(o: { art: HTMLElement; title: string; sub: string; mark?: HTMLElement | null; playing?: boolean; open: () => void; play: () => void; menu: (e: MouseEvent) => void }) {
    const el = h(
      "div",
      { class: `ln-tile${o.playing ? " playing" : ""}`, title: o.title, onclick: o.open },
      o.art,
      h("div", { class: "ln-tile-title" }, o.mark, o.title),
      h("div", { class: "ln-tile-sub" }, o.sub),
    );
    o.art.append(
      h(
        "button",
        {
          class: "ln-tile-play",
          type: "button",
          title: "Play",
          onclick: (e: Event) => {
            e.stopPropagation();
            o.play();
          },
        },
        "▶",
      ),
    );
    el.addEventListener("contextmenu", o.menu as EventListener);
    return el;
  }

  /** ● when every server song of the album is kept on this computer, ↓ while it downloads. */
  function albumMark(a: Album): HTMLElement | null {
    if (!store.remote.size) return null;
    let kept = 0;
    let remote = 0;
    let downloading = false;
    for (const id of a.ids) {
      const s = store.remote.get(id);
      if (!s) continue;
      remote++;
      if (s === "downloading") downloading = true;
      else if (s !== "remote") kept++;
    }
    if (downloading) return h("span", { class: "offline-mark", title: "Downloading to this computer" }, "↓");
    return remote && kept === remote ? h("span", { class: "offline-mark", title: "On this computer" }, "●") : null;
  }

  const playingAlbum = () => {
    const t = store.nowPlaying;
    return t ? albumKey(t) : null;
  };

  const albumTile = (a: Album, sub = artistName(a.artist)) =>
    tile({
      art: cover(a.artId, albumTitle(a)),
      title: albumTitle(a),
      sub,
      mark: albumMark(a),
      playing: a.key === playingAlbum(),
      open: () => show({ kind: "album", key: a.key }),
      play: () => play(a.ids),
      menu: (e) => albumMenu(e, a),
    });

  const tileSize = () => ({ min: store.settings.density === "compact" ? 132 : 156, extra: 46, gap: 20 });

  /** The line above a tab's grid: what's listed, and its controls. */
  const summary = (text: string, ...controls: (HTMLElement | null)[]) => h("div", { class: "ln-summary" }, h("span", { class: "muted" }, text), h("span", { class: "ln-summary-tools" }, ...controls));

  const emptyText = (what: string) => () =>
    query ? `No ${what} match “${search.value.trim()}”.` : store.tracks.size === 0 ? "Your library is empty. Open Settings (⚙) and add a music folder." : `No ${what} yet.`;

  // ---------- tabs ----------

  function albumsTab(): PageView {
    const sort = (store.settings["listen.albumSort"] as AlbumSort) in ALBUM_SORTS ? (store.settings["listen.albumSort"] as AlbumSort) : "artist";
    const list = query ? results().albums : albumsBy(sort);
    const pick = h("select", { class: "input ln-sort", title: "Order" });
    for (const [k, label] of Object.entries(ALBUM_SORTS)) pick.append(h("option", { value: k, selected: k === sort }, label));
    pick.addEventListener("change", () =>
      run(async () => {
        await saveSetting("listen.albumSort", pick.value);
        draw(0);
      }),
    );
    const grid = vgrid({
      tile: tileSize(),
      header: summary(plural(list.length, "album"), query ? null : pick),
      count: () => list.length,
      render: (i) => albumTile(list[i]),
      empty: emptyText("albums"),
    });
    return { el: grid.el, grid };
  }

  function artistsTab(): PageView {
    const list = query ? results().artists : library().artists;
    const grid = vgrid({
      tile: tileSize(),
      header: summary(plural(list.length, "artist")),
      count: () => list.length,
      render: (i) => {
        const a = list[i];
        const el = tile({
          art: cover(a.artId, artistName(a.name)),
          title: artistName(a.name),
          sub: `${plural(a.albums.length, "album")} · ${plural(a.songs, "song")}`,
          open: () => show({ kind: "artist", name: a.name }),
          play: () => play(idsOfArtist(a)),
          menu: (e) => {
            e.preventDefault();
            openSongMenu(e.clientX, e.clientY, idsOfArtist(a), { manage: manageGroup(a.name) });
          },
        });
        el.classList.add("hex");
        return el;
      },
      empty: emptyText("artists"),
    });
    return { el: grid.el, grid };
  }

  const managePlaylist = (p: Playlist) => async () => {
    await setMode("advanced");
    await store.setView({ kind: "playlist", id: p.id });
  };

  function playlistsTab(): PageView {
    const list = query ? results().playlists : store.playlists;
    const create = h(
      "button",
      {
        class: "btn",
        type: "button",
        onclick: () =>
          run(async () => {
            const name = await promptText("New playlist", "Name", "New playlist");
            if (!name) return;
            const p = await api.createPlaylist(name, null, []);
            await store.loadPlaylists();
            show({ kind: "playlist", id: p.id });
          }),
      },
      "New playlist…",
    );
    const grid = vgrid({
      tile: tileSize(),
      header: summary(plural(list.length, "playlist"), query ? null : create),
      count: () => list.length,
      render: (i) => {
        const p = list[i];
        const art = cover(null, p.name);
        const el = tile({
          art,
          title: p.name,
          sub: p.kind === "smart" ? "Smart playlist" : "Playlist",
          open: () => show({ kind: "playlist", id: p.id }),
          play: () => run(async () => api.load(await idsOfPlaylist(p.id), 0)),
          menu: (e) => {
            e.preventDefault();
            void idsOfPlaylist(p.id).then((ids) => openSongMenu(e.clientX, e.clientY, ids, { manage: managePlaylist(p) }));
          },
        });
        void idsOfPlaylist(p.id).then((ids) => {
          if (!el.isConnected) return;
          el.querySelector(".ln-tile-sub")!.textContent = `${p.kind === "smart" ? "Smart playlist" : "Playlist"} · ${plural(ids.length, "song")}`;
          const withArt = ids.find((id) => store.tracks.get(id)?.hasArt);
          if (withArt === undefined) return;
          const img = cover(withArt, p.name, false).querySelector("img")!;
          art.prepend(img);
        });
        return el;
      },
      empty: () => (query ? `No playlists match “${search.value.trim()}”.` : "No playlists yet. Right-click an album or a song and choose Add to playlist."),
    });
    return { el: grid.el, grid };
  }

  // ---------- song rows ----------

  type Row = { disc: number } | { t: Track; at: number };

  /**
   * A list of songs under `header`. `numbered` rows show track numbers (an
   * album's own page); otherwise each row has its cover, artist and album.
   */
  function songList(ids: number[], header: HTMLElement, o: { numbered?: boolean; album?: Album; empty: () => string }): VGrid {
    const rows: Row[] = [];
    const discs = o.numbered ? new Set(ids.map((id) => store.tracks.get(id)?.discNo ?? 1)) : null;
    let disc = -1;
    ids.forEach((id, at) => {
      const t = store.tracks.get(id);
      if (!t) return;
      if (discs && discs.size > 1 && (t.discNo ?? 1) !== disc) {
        disc = t.discNo ?? 1;
        rows.push({ disc });
      }
      rows.push({ t, at });
    });
    const lib = library();
    return vgrid({
      rowH: o.numbered ? 38 : 48,
      header,
      count: () => rows.length,
      empty: o.empty,
      render: (i) => {
        const r = rows[i];
        if ("disc" in r) return h("div", { class: "ln-disc" }, `Disc ${r.disc}`);
        const { t, at } = r;
        const playing = t.id === store.tick.trackId;
        const album = lib.byKey.get(albumKey(t));
        const lead = o.numbered
          ? h("span", { class: "ln-row-n", title: "Play", onclick: () => play(ids, at) }, h("span", { class: "n" }, playing ? "♪" : String(t.trackNo ?? at + 1)), h("span", { class: "go" }, "▶"))
          : (() => {
              const thumb = album?.artId ?? (t.hasArt ? t.id : null);
              return thumb === null ? h("span", { class: "ln-row-art" }) : h("img", { class: "ln-row-art", alt: "", decoding: "async", "data-src": artUrl(thumb) });
            })();
        const artist = t.artist || artistName(artistOf(t));
        const love = heart(t.loved);
        love.addEventListener("click", (e) => {
          e.stopPropagation();
          run(() => setLoved([t.id], !t.loved));
        });
        love.addEventListener("dblclick", (e) => e.stopPropagation());
        const row = h(
          "div",
          { class: `ln-row${playing ? " playing" : ""}${o.numbered ? " numbered" : ""}`, ondblclick: () => play(ids, at) },
          lead,
          h(
            "div",
            { class: "ln-row-main" },
            h("div", { class: "ln-row-title", title: t.title }, offlineMark(t.id), t.title),
            o.numbered && t.artist === o.album?.artist ? null : h("div", { class: "ln-row-sub" }, ...artistLinks(artist, "ln-link", artistOf(t))),
          ),
          o.numbered ? null : h("span", { class: "ln-row-album" }, album ? h("span", { class: "ln-link", onclick: () => show({ kind: "album", key: album.key }) }, albumTitle(album)) : null),
          love,
          h("span", { class: "ln-row-time" }, fmtTime(t.duration)),
        );
        row.addEventListener("contextmenu", (e) => {
          e.preventDefault();
          openSongMenu(e.clientX, e.clientY, [t.id], {
            album: o.numbered ? undefined : album,
            artist: artistOf(t),
            manage: manageGroup(artistOf(t), t.album),
          });
        });
        return row;
      },
    });
  }

  function songsTab(): PageView {
    const ids = query ? results().songs : library().songs;
    const shuffle = h("button", { class: "btn", type: "button", onclick: () => play(shuffled(ids)) }, "Shuffle all");
    const grid = songList(ids, summary(plural(ids.length, "song"), ids.length > 1 ? shuffle : null), { empty: emptyText("songs") });
    return { el: grid.el, grid };
  }

  // ---------- album, artist and playlist pages ----------

  const backLabel = () => {
    const prev = history[history.length - 1]?.page;
    return !prev ? "Albums" : prev.kind === "tab" ? TABS[prev.tab] : "Back";
  };

  /** The top of a detail page: back link, cover, name, a line of facts and the play buttons. */
  function hero(o: { art: HTMLElement; title: string; lines: (HTMLElement | string)[]; ids: () => number[]; more?: (e: MouseEvent) => void; hex?: boolean }) {
    const more = o.more;
    // The cover, blurred far out behind the header, so each page takes its album's colours.
    const src = o.art.querySelector("img")?.getAttribute("src");
    const bg = src ? h("div", { class: "ln-hero-bg", "aria-hidden": "true" }) : null;
    if (bg) bg.style.backgroundImage = `url("${src}")`;
    return h(
      "div",
      { class: "ln-hero-wrap" },
      bg,
      h("button", { class: "ln-back", type: "button", title: "Back (Esc)", onclick: () => back() }, `‹ ${backLabel()}`),
      h(
        "div",
        { class: `ln-hero${o.hex ? " hex" : ""}` },
        o.art,
        h(
          "div",
          { class: "ln-hero-text" },
          h("h1", {}, o.title),
          ...o.lines.map((l) => h("div", { class: "ln-hero-line" }, l)),
          h(
            "div",
            { class: "ln-hero-actions" },
            h("button", { class: "btn primary", type: "button", disabled: !o.ids().length, onclick: () => play(o.ids()) }, "▶  Play"),
            h("button", { class: "btn", type: "button", disabled: o.ids().length < 2, onclick: () => play(shuffled(o.ids())) }, "Shuffle"),
            more ? h("button", { class: "btn", type: "button", title: "More", onclick: (e: Event) => more(e as MouseEvent) }, "•••") : null,
          ),
        ),
      ),
    );
  }

  const gone = (what: string): PageView => ({
    el: h("div", { class: "ln-missing" }, h("p", { class: "muted" }, `This ${what} isn't in your library anymore.`), h("button", { class: "btn", type: "button", onclick: () => back() }, "Back")),
  });

  function albumPage(key: string): PageView {
    const a = library().byKey.get(key);
    if (!a) return gone("album");
    const art = cover(a.artId, albumTitle(a), false);
    const head = hero({
      art,
      title: albumTitle(a),
      lines: [
        h("span", {}, ...artistLinks(a.artist, "ln-link accent")),
        h("span", { class: "muted" }, albumMark(a), [a.year, plural(a.ids.length, "song"), fmtTotal(a.duration)].filter(Boolean).join(" · ")),
      ],
      ids: () => a.ids,
      more: (e) => albumMenu(e, a, true),
    });
    const grid = songList(a.ids, head, { numbered: true, album: a, empty: () => "" });
    return { el: grid.el, grid };
  }

  function artistPage(name: string): PageView {
    const lib = library();
    const a = lib.byName.get(name) ?? lib.byName.get(splitArtists(name)[0]);
    if (!a) return gone("artist");
    const albums = [...a.albums].sort((x, y) => (x.year ?? 9999) - (y.year ?? 9999) || collator.compare(x.title, y.title));
    const head = hero({
      art: cover(a.artId, artistName(a.name), false),
      title: artistName(a.name),
      lines: [h("span", { class: "muted" }, `${plural(a.albums.length, "album")} · ${plural(a.songs, "song")}`)],
      ids: () => albums.flatMap((al) => al.ids),
      more: (e) => {
        e.preventDefault();
        openSongMenu(e.clientX, e.clientY, idsOfArtist(a), { manage: manageGroup(a.name) });
      },
      hex: true,
    });
    const grid = vgrid({
      tile: tileSize(),
      header: head,
      count: () => albums.length,
      render: (i) => albumTile(albums[i], albums[i].year ? String(albums[i].year) : ""),
    });
    return { el: grid.el, grid };
  }

  function playlistPage(id: number): PageView {
    const p = store.playlists.find((x) => x.id === id);
    if (!p) return gone("playlist");
    const box = h("div", { class: "ln-fill" });
    const view: PageView = { el: box };
    void run(async () => {
      const ids = await idsOfPlaylist(id);
      if (current !== view) return;
      const total = ids.reduce((s, t) => s + (store.tracks.get(t)?.duration ?? 0), 0);
      const withArt = ids.find((t) => store.tracks.get(t)?.hasArt);
      const head = hero({
        art: cover(withArt ?? null, p.name, false),
        title: p.name,
        lines: [h("span", { class: "muted" }, [p.kind === "smart" ? "Smart playlist" : "Playlist", plural(ids.length, "song"), fmtTotal(total)].join(" · "))],
        ids: () => ids,
        more: (e) => {
          e.preventDefault();
          openSongMenu(e.clientX, e.clientY, ids, { manage: managePlaylist(p) });
        },
      });
      const grid = songList(ids, head, { empty: () => (p.kind === "smart" ? "No songs match this playlist's rules right now." : "This playlist is empty. Right-click a song or an album and choose Add to playlist.") });
      view.grid = grid;
      box.replaceChildren(grid.el);
      grid.restore(pendingTop);
    });
    return view;
  }
  /** Where a playlist page scrolls once its songs have arrived. */
  let pendingTop = 0;

  function build(): PageView {
    switch (page.kind) {
      case "album":
        return albumPage(page.key);
      case "artist":
        return artistPage(page.name);
      case "playlist":
        return playlistPage(page.id);
      default:
        return page.tab === "albums" ? albumsTab() : page.tab === "artists" ? artistsTab() : page.tab === "playlists" ? playlistsTab() : songsTab();
    }
  }

  // ---------- staying in step with the store ----------

  let redraw = 0;
  const changed = () => {
    if (mode() !== "listening") {
      dirty = true;
      return;
    }
    if (!redraw) {
      redraw = requestAnimationFrame(() => {
        redraw = 0;
        draw();
      });
    }
  };
  store.on("tracks", () => {
    playlistIds.clear();
    if (mode() === "listening" && !dirty && !redraw && stamp() === drawn && current.grid) return current.grid.repaint();
    changed();
  });
  store.on("playlists", () => {
    playlistIds.clear();
    if (page.kind === "playlist" || (page.kind === "tab" && page.tab === "playlists")) changed();
    else if (query) renderTabs();
  });
  store.on("settings", changed);
  store.on("mode", () => {
    playlistIds.clear();
    if (mode() === "listening" && dirty) draw();
  });
  let lastId: number | null = null;
  store.on("tick", () => {
    if (store.tick.trackId === lastId) return;
    lastId = store.tick.trackId;
    if (mode() === "listening") current.grid?.refresh();
  });

  // Capturing, so an open menu or dialog is still there to be seen and gets the key instead.
  window.addEventListener(
    "keydown",
    (e) => {
      if (e.key !== "Escape" || mode() !== "listening" || e.target === search || document.querySelector(".backdrop, .menu")) return;
      if (now.isOpen()) now.close();
      else if (page.kind !== "tab") back();
    },
    true,
  );

  draw(0);

  return {
    setStatus(text: string) {
      status.textContent = text;
    },
    focusSearch() {
      now.close();
      search.focus();
      search.select();
    },
  };
}
