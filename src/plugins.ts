import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "./api";
import type { MatchCandidate, PluginContext, PluginManifest, PluginModule, PluginResponse, RequestInit, Track } from "./plugin-api";
import { store } from "./store";
import { h, toast } from "./util";
import { button, modal, promptText } from "./ui/dialogs";

export interface ListedPlugin {
  folder: string;
  manifest: PluginManifest | null;
  error: string | null;
  /** Ships with Honeycomb. */
  bundled: boolean;
}

interface SidebarEntry {
  plugin: string;
  label: string;
  icon: string;
  badge: string;
  onClick: () => void;
}

const ENABLED = "plugins.enabled";

/** Plugins that activated this session, by id. */
const active = new Map<string, { manifest: PluginManifest; module: PluginModule; sections: ((el: HTMLElement) => void)[] }>();
/** Plugins that failed to load, by id. */
const failed = new Map<string, string>();
/** Event listeners each plugin added, removed when it's turned off. */
const listeners = new Map<string, Promise<UnlistenFn>[]>();
export const sidebarItems: SidebarEntry[] = [];
const sidebarListeners = new Set<() => void>();

export const pluginApi = {
  list: () => invoke<ListedPlugin[]>("plugin_list"),
  folder: () => invoke<string>("plugin_folder"),
};

export function onSidebarItems(fn: () => void) {
  sidebarListeners.add(fn);
}

function sidebarChanged() {
  sidebarListeners.forEach((fn) => fn());
}

export function enabledIds(): string[] {
  try {
    const v = JSON.parse(store.settings[ENABLED] || "[]");
    return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
  } catch {
    return [];
  }
}

export function pluginState(id: string): { active: boolean; error: string | null } {
  return { active: active.has(id), error: failed.get(id) ?? null };
}

export function settingsSections(): { manifest: PluginManifest; render: (el: HTMLElement) => void }[] {
  return [...active.values()].flatMap((p) => p.sections.map((render) => ({ manifest: p.manifest, render })));
}

/** Turn a plugin on (loading it now) or off (it unloads on the next start). */
export async function setEnabled(m: PluginManifest, on: boolean) {
  const ids = enabledIds().filter((id) => id !== m.id);
  if (on) ids.push(m.id);
  store.settings[ENABLED] = JSON.stringify(ids);
  await api.setSetting(ENABLED, store.settings[ENABLED]);
  if (on && !active.has(m.id)) await activate(m);
  if (!on) {
    const p = active.get(m.id);
    try {
      await p?.module.deactivate?.();
    } catch (e) {
      console.error(e);
    }
    active.delete(m.id);
    failed.delete(m.id);
    for (const un of listeners.get(m.id) ?? []) void un.then((f) => f());
    listeners.delete(m.id);
    for (let i = sidebarItems.length - 1; i >= 0; i--) if (sidebarItems[i].plugin === m.id) sidebarItems.splice(i, 1);
    sidebarChanged();
  }
}

/** `plugin://localhost/<id>/<file>`, or `http://plugin.localhost/…` on Windows. */
function fileUrl(id: string, file: string): string {
  return convertFileSrc("", "plugin") + `${id}/${file.split("/").map(encodeURIComponent).join("/")}`;
}

function context(m: PluginManifest, sections: ((el: HTMLElement) => void)[]): PluginContext {
  const plugin = m.id;
  const prefix = `plugin.${plugin}.`;
  /** Call `fn` with the library track a player event names, if it's still in the library. */
  const onPlayer = <P extends { trackId: number }>(event: string, fn: (track: Track, payload: P) => void) => {
    const un = listen<P>(event, (e) => {
      const t = store.tracks.get(e.payload.trackId);
      if (!t) return;
      try {
        fn(t, e.payload);
      } catch (err) {
        console.error(`plugin ${plugin}:`, err);
      }
    });
    listeners.set(plugin, [...(listeners.get(plugin) ?? []), un]);
  };
  return {
    manifest: m,
    library: {
      tracks: () => [...store.tracks.values()],
      track: (id) => store.tracks.get(id),
      match: (candidates: MatchCandidate[]) => invoke<(number | null)[]>("match_tracks", { candidates }),
      onChange: (fn) => store.on("tracks", fn),
    },
    playlists: {
      list: () => [...store.playlists],
      create: async (name, trackIds) => {
        const pl = await api.createPlaylist(name, null, trackIds);
        await store.loadPlaylists();
        return pl;
      },
      setTracks: async (id, trackIds) => {
        await api.setPlaylistTracks(id, trackIds);
        if (store.view.kind === "playlist" && store.view.id === id) await store.refreshView("keepScroll");
      },
      trackIds: (id) => api.playlistTrackIds(id),
      show: (id) => store.setView({ kind: "playlist", id }),
    },
    player: {
      play: (ids, index = 0) => api.load(ids, index),
      enqueue: (ids, next = false) => api.enqueue(ids, next),
      onNowPlaying: (fn) => onPlayer("player-now-playing", (t) => fn(t)),
      onListened: (fn) => onPlayer<{ trackId: number; playedAt: number }>("player-listened", (t, p) => fn(t, p.playedAt)),
    },
    http: {
      request: async (url: string, init: RequestInit = {}): Promise<PluginResponse> => {
        const headers = { ...(init.headers ?? {}) };
        let body = init.body;
        if (body !== undefined && typeof body !== "string") {
          body = new URLSearchParams(body).toString();
          headers["Content-Type"] ??= "application/x-www-form-urlencoded";
        }
        const r = await invoke<{ status: number; headers: Record<string, string>; body: string }>("plugin_http", {
          plugin,
          request: { url, method: init.method, headers, body },
        });
        return {
          status: r.status,
          ok: r.status >= 200 && r.status < 300,
          headers: r.headers,
          text: r.body,
          json: () => JSON.parse(r.body),
        };
      },
    },
    settings: {
      get: (key) => store.settings[prefix + key],
      set: async (key, value) => {
        store.settings[prefix + key] = value;
        await api.setSetting(prefix + key, value);
      },
    },
    secrets: {
      get: (key) => invoke<string | null>("plugin_secret_get", { plugin, key }),
      set: (key, value) => invoke<void>("plugin_secret_set", { plugin, key, value }),
      delete: (key) => invoke<void>("plugin_secret_set", { plugin, key, value: null }),
    },
    openUrl: (url) => openUrl(url),
    waitForRedirect: async (port, path, timeoutSecs) =>
      new URLSearchParams(await invoke<string>("plugin_oauth_redirect", { plugin, port, path, timeoutSecs })),
    ui: {
      addSidebarItem: ({ label, icon = "◆", onClick }) => {
        const entry: SidebarEntry = { plugin, label, icon, badge: "", onClick };
        sidebarItems.push(entry);
        sidebarChanged();
        return {
          setBadge: (text) => {
            entry.badge = text;
            sidebarChanged();
          },
          remove: () => {
            const i = sidebarItems.indexOf(entry);
            if (i >= 0) sidebarItems.splice(i, 1);
            sidebarChanged();
          },
        };
      },
      addSettingsSection: (render) => sections.push(render),
      modal,
      button,
      h,
      toast,
      prompt: promptText,
    },
  };
}

async function activate(m: PluginManifest) {
  failed.delete(m.id);
  try {
    const module = (await import(/* @vite-ignore */ fileUrl(m.id, m.main))) as PluginModule;
    if (typeof module.activate !== "function") throw new Error(`${m.main} doesn't export an activate() function`);
    const sections: ((el: HTMLElement) => void)[] = [];
    await module.activate(context(m, sections));
    active.set(m.id, { manifest: m, module, sections });
  } catch (e) {
    failed.set(m.id, String(e));
    throw new Error(`Plugin ${m.name} failed to load: ${e}`);
  }
}

/** Load every enabled plugin. One failing doesn't stop the others. */
export async function loadPlugins() {
  const enabled = new Set(enabledIds());
  if (!enabled.size) return;
  for (const p of await pluginApi.list()) {
    if (!p.manifest || !enabled.has(p.manifest.id)) continue;
    try {
      await activate(p.manifest);
    } catch (e) {
      toast(String(e), true);
    }
  }
}
