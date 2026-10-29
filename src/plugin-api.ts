/**
 * The API plugins get. A plugin is a folder in the plugins directory holding
 * `plugin.json` and an ES module that exports `activate(ctx: PluginContext)`
 * (and optionally `deactivate()`). See docs/plugins.md.
 *
 * Plugins can copy this file for type checking; it only uses types.
 */
import type { Playlist, Track } from "./api";
import type { Modal } from "./ui/dialogs";

export type { Playlist, Track };

export interface PluginManifest {
  id: string;
  name: string;
  version: string;
  description: string;
  main: string;
  apiVersion: number;
  /** Hosts `http.request` may reach (HTTPS only). */
  hosts: string[];
  /** Ports on 127.0.0.1 `waitForRedirect` may listen on. */
  loopbackPorts: number[];
}

/** A track from another service, to look up in the library. */
export interface MatchCandidate {
  title: string;
  /** Every credited artist, e.g. ["Daft Punk", "Pharrell Williams"]. */
  artists: string[];
  album?: string;
  /** Seconds. Leave out when unknown. */
  duration?: number;
}

export interface PluginResponse {
  status: number;
  ok: boolean;
  /** Header names are lowercase. */
  headers: Record<string, string>;
  text: string;
  json<T = any>(): T;
}

export interface RequestInit {
  method?: string;
  headers?: Record<string, string>;
  /** A string, or an object sent as a form (application/x-www-form-urlencoded). */
  body?: string | Record<string, string>;
}

export interface SidebarItem {
  setBadge(text: string): void;
  remove(): void;
}

export interface PluginContext {
  manifest: PluginManifest;
  library: {
    /** Every track in the library, as loaded right now. */
    tracks(): Track[];
    track(id: number): Track | undefined;
    /** Library ids for outside tracks, null where nothing matched. Same artist and title, similar length. */
    match(candidates: MatchCandidate[]): Promise<(number | null)[]>;
    /** Called after the library reloads or changes. */
    onChange(fn: () => void): void;
  };
  playlists: {
    list(): Playlist[];
    /** A new manual playlist. */
    create(name: string, trackIds: number[]): Promise<Playlist>;
    /** Replace a manual playlist's tracks. */
    setTracks(id: number, trackIds: number[]): Promise<void>;
    trackIds(id: number): Promise<number[]>;
    /** Show a playlist in the track list. */
    show(id: number): Promise<void>;
  };
  player: {
    play(trackIds: number[], index?: number): Promise<void>;
    enqueue(trackIds: number[], next?: boolean): Promise<void>;
    /** A song started playing (again after a pause doesn't count). */
    onNowPlaying(fn: (track: Track) => void): void;
    /**
     * A song was heard long enough to count as listened to: half of it or
     * 4 minutes, for songs over 30 seconds. `playedAt` is when it started, in
     * Unix seconds. Fires once per play.
     */
    onListened(fn: (track: Track, playedAt: number) => void): void;
  };
  http: {
    /** Goes through Honeycomb, so CORS doesn't apply. Only hosts in the manifest. Never throws on 4xx/5xx. */
    request(url: string, init?: RequestInit): Promise<PluginResponse>;
  };
  /** Plain settings, stored as strings, private to this plugin. */
  settings: {
    get(key: string): string | undefined;
    set(key: string, value: string): Promise<void>;
  };
  /** Tokens and keys, kept in the system keychain (not in library.db or the settings the UI loads). */
  secrets: {
    get(key: string): Promise<string | null>;
    set(key: string, value: string): Promise<void>;
    delete(key: string): Promise<void>;
  };
  /** Open a URL in the default browser. */
  openUrl(url: string): Promise<void>;
  /**
   * Wait for the browser to load `http://127.0.0.1:<port><path>` (an OAuth
   * redirect) and return its query parameters. The port must be in the manifest.
   */
  waitForRedirect(port: number, path: string, timeoutSecs?: number): Promise<URLSearchParams>;
  ui: {
    /** An entry in the sidebar's Plugins group. */
    addSidebarItem(item: { label: string; icon?: string; onClick: () => void }): SidebarItem;
    /** A section under Settings → Plugins, rendered each time Settings opens. */
    addSettingsSection(render: (el: HTMLElement) => void): void;
    modal(title: string, opts?: { wide?: boolean; onCancel?: () => void }): Modal;
    button(label: string, onclick: () => void, primary?: boolean): HTMLButtonElement;
    h: typeof import("./util").h;
    toast(message: string, isError?: boolean): void;
    prompt(title: string, label: string, initial?: string): Promise<string | null>;
  };
}

export interface PluginModule {
  activate(ctx: PluginContext): void | Promise<void>;
  deactivate?(): void | Promise<void>;
}
