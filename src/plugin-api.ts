/**
 * The API plugins get. A plugin is a folder in the plugins directory holding
 * `plugin.json` and an ES module that exports `activate(ctx: PluginContext)`
 * (and optionally `deactivate()`). See docs/plugins.md.
 *
 * Plugins run in a sandbox of their own, so everything here that reaches
 * Honeycomb is a message to it: most calls return promises, and the interface
 * is built from `PluginElement`s that Honeycomb draws.
 *
 * Plugins can copy this file for type checking; it only uses types.
 */
import type { Playlist, Track } from "./api";

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

export type PluginChild = PluginElement | string | number | null | undefined | false;

export interface PluginEvent {
  type: string;
  target: PluginElement;
  /** For key events. */
  key?: string;
}

/**
 * An element of the plugin's interface. It isn't a DOM node: Honeycomb draws
 * the real one, with its own styles, and sends events back. These members are
 * all there is. Tags are limited to plain layout, text, table and form
 * elements, attributes to the likes of class, style, type, value, placeholder,
 * checked, disabled, readonly, rows and title, and `style` to layout and text
 * properties. Events: click, dblclick, input, change, keydown, keyup, focus, blur.
 */
export interface PluginElement {
  readonly tagName: string;
  /** False once the section or dialog this is in has left the screen. */
  readonly isConnected: boolean;
  /** Notes of the plugin's own; not drawn. */
  dataset: Record<string, string>;
  /** What's typed in a field, kept up to date. */
  value: string;
  checked: boolean;
  disabled: boolean;
  hidden: boolean;
  title: string;
  className: string;
  textContent: string;
  append(...children: PluginChild[]): void;
  replaceChildren(...children: PluginChild[]): void;
  remove(): void;
  getAttribute(name: string): string | null;
  setAttribute(name: string, value: string | number): void;
  removeAttribute(name: string): void;
  addEventListener(type: string, fn: (e: PluginEvent) => void): void;
  removeEventListener(type: string, fn: (e: PluginEvent) => void): void;
  focus(): void;
  select(): void;
}

export interface PluginModal {
  body: PluginElement;
  footer: PluginElement;
  close(): void;
}

export interface PluginContext {
  manifest: PluginManifest;
  library: {
    /** Every track in the library, as loaded right now. */
    tracks(): Promise<Track[]>;
    track(id: number): Promise<Track | undefined>;
    /** Library ids for outside tracks, null where nothing matched. Same artist and title, similar length. */
    match(candidates: MatchCandidate[]): Promise<(number | null)[]>;
    /** Called after the library reloads or changes. */
    onChange(fn: () => void): void;
  };
  playlists: {
    list(): Promise<Playlist[]>;
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
  /** Open an http:// or https:// URL in the default browser. */
  openUrl(url: string): Promise<void>;
  /**
   * Wait for the browser to load `http://127.0.0.1:<port><path>` (an OAuth
   * redirect) and return its query parameters. The port must be in the manifest.
   */
  waitForRedirect(port: number, path: string, timeoutSecs?: number): Promise<URLSearchParams>;
  ui: {
    /** An entry in the sidebar's Plugins group. */
    addSidebarItem(item: { label: string; icon?: string; onClick: () => void }): SidebarItem;
    /** A section under Settings → Plugins. `render` is called with an empty element each time Settings opens. */
    addSettingsSection(render: (el: PluginElement) => void): void;
    /** A dialog. `onCancel` runs when it's closed with Escape or a click outside it. */
    modal(title: string, opts?: { wide?: boolean; onCancel?: () => void }): PluginModal;
    button(label: string, onclick: () => void, primary?: boolean): PluginElement;
    /** Builds an element: h("div", { class: "row", onclick: fn }, child, "text"). */
    h(tag: string, attrs?: Record<string, string | number | boolean | ((e: PluginEvent) => void) | undefined>, ...children: PluginChild[]): PluginElement;
    toast(message: string, isError?: boolean): void;
    prompt(title: string, label: string, initial?: string): Promise<string | null>;
    /** Put text on the clipboard. */
    copyText(text: string): Promise<void>;
  };
}

export interface PluginModule {
  activate(ctx: PluginContext): void | Promise<void>;
  deactivate?(): void | Promise<void>;
}
