import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "./api";
import type { PluginManifest } from "./plugin-api";
import { PluginDom, SidebarEntry } from "./plugin-dom";
import { store } from "./store";
import { toast } from "./util";
import { promptText } from "./ui/dialogs";

export interface ListedPlugin {
  folder: string;
  manifest: PluginManifest | null;
  error: string | null;
  /** Ships with Honeycomb. */
  bundled: boolean;
}

/**
 * A running plugin. Its code is in `frame`, a sandboxed frame with no origin,
 * which can't reach this page or the app's commands; all it has is `port`.
 * Whatever comes in on that port is this plugin asking, so the id used for its
 * settings, secrets and network rules is always `manifest.id`, never something
 * the plugin sent.
 */
interface Running {
  manifest: PluginManifest;
  frame: HTMLIFrameElement;
  port: MessagePort;
  dom: PluginDom;
}

const ENABLED = "plugins.enabled";
const RUNTIME = "__honeycomb__/runtime.js";
const START_TIMEOUT = 20_000;

const active = new Map<string, Running>();
/** Plugins that failed to load, by id. */
const failed = new Map<string, string>();
export const sidebarItems: SidebarEntry[] = [];
const sidebarListeners = new Set<() => void>();
let following = false;

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
  return [...active.values()].flatMap((p) =>
    Array.from({ length: p.dom.sections }, (_, i) => ({ manifest: p.manifest, render: (el: HTMLElement) => p.dom.mount(i, el) })),
  );
}

/** Turn a plugin on (loading it now) or off (unloading it). */
export async function setEnabled(m: PluginManifest, on: boolean) {
  const ids = enabledIds().filter((id) => id !== m.id);
  if (on) ids.push(m.id);
  store.settings[ENABLED] = JSON.stringify(ids);
  await api.setSetting(ENABLED, store.settings[ENABLED]);
  if (on && !active.has(m.id)) await activate(m);
  if (!on) {
    failed.delete(m.id);
    await stop(m.id);
  }
}

const ids = (v: unknown): number[] => (Array.isArray(v) ? v.map(Number).filter(Number.isFinite) : []);

/** What a plugin can ask for, by name. `p` is the plugin whose port the request came in on. */
const handlers: Record<string, (p: Running, ...args: any[]) => unknown> = {
  "library.tracks": () => [...store.tracks.values()],
  "library.track": (_p, id) => store.tracks.get(Number(id)),
  "library.match": (_p, candidates) => invoke("match_tracks", { candidates }),
  "playlists.list": () => [...store.playlists],
  "playlists.create": async (_p, name, trackIds) => {
    const pl = await api.createPlaylist(String(name), null, ids(trackIds));
    await store.loadPlaylists();
    return pl;
  },
  "playlists.setTracks": async (_p, id, trackIds) => {
    await api.setPlaylistTracks(Number(id), ids(trackIds));
    if (store.view.kind === "playlist" && store.view.id === Number(id)) await store.refreshView("keepScroll");
  },
  "playlists.trackIds": (_p, id) => api.playlistTrackIds(Number(id)),
  "playlists.show": (_p, id) => store.setView({ kind: "playlist", id: Number(id) }),
  "player.play": (_p, trackIds, index) => api.load(ids(trackIds), Number(index) || 0),
  "player.enqueue": (_p, trackIds, next) => api.enqueue(ids(trackIds), !!next),
  "http.request": (p, request) => invoke("plugin_http", { plugin: p.manifest.id, request }),
  "settings.set": async (p, key, value) => {
    const name = `plugin.${p.manifest.id}.${key}`;
    store.settings[name] = String(value);
    await api.setSetting(name, String(value));
  },
  "secrets.get": (p, key) => invoke("plugin_secret_get", { plugin: p.manifest.id, key: String(key) }),
  "secrets.set": (p, key, value) => invoke("plugin_secret_set", { plugin: p.manifest.id, key: String(key), value: String(value) }),
  "secrets.delete": (p, key) => invoke("plugin_secret_set", { plugin: p.manifest.id, key: String(key), value: null }),
  openUrl: (_p, url) => {
    if (!/^https?:\/\//i.test(String(url))) throw new Error("plugins can only open http:// and https:// links");
    return openUrl(String(url));
  },
  waitForRedirect: (p, port, path, timeoutSecs) =>
    invoke("plugin_oauth_redirect", { plugin: p.manifest.id, port: Number(port), path: String(path), timeoutSecs: timeoutSecs ?? null }),
  "ui.prompt": (_p, title, label, initial) => promptText(String(title), String(label), String(initial ?? "")),
  "ui.copyText": (_p, text) => navigator.clipboard.writeText(String(text)),
};

async function answer(p: Running, data: any) {
  if (Array.isArray(data?.ops)) return p.dom.apply(data.ops);
  if (typeof data?.call !== "string") return;
  let reply: Record<string, unknown>;
  try {
    if (!Object.prototype.hasOwnProperty.call(handlers, data.call)) throw new Error(`unknown request ${data.call}`);
    const args = Array.isArray(data.args) ? data.args : [];
    reply = { ok: true, value: await handlers[data.call](p, ...args) };
  } catch (e) {
    reply = { ok: false, error: String(e instanceof Error ? e.message : e) };
  }
  p.port.postMessage({ reply: data.id, ...reply });
}

/** Pass library and player events on to every running plugin. */
function follow() {
  if (following) return;
  following = true;
  const each = (event: string, data: Record<string, unknown> = {}) => active.forEach((p) => p.port.postMessage({ event, ...data }));
  store.on("tracks", () => each("library"));
  void listen<{ trackId: number }>("player-now-playing", (e) => {
    const track = store.tracks.get(e.payload.trackId);
    if (track) each("nowPlaying", { track });
  });
  void listen<{ trackId: number; playedAt: number }>("player-listened", (e) => {
    const track = store.tracks.get(e.payload.trackId);
    if (track) each("listened", { track, playedAt: e.payload.playedAt });
  });
}

/** The plugin's own settings, without the `plugin.<id>.` in front. */
function ownSettings(id: string): Record<string, string> {
  const prefix = `plugin.${id}.`;
  return Object.fromEntries(
    Object.entries(store.settings)
      .filter(([k]) => k.startsWith(prefix))
      .map(([k, v]) => [k.slice(prefix.length), v]),
  );
}

async function activate(m: PluginManifest) {
  failed.delete(m.id);
  let p: Running | undefined;
  try {
    // `plugin://localhost/<token>/`, or `http://plugin.localhost/…` on Windows.
    const base = `${convertFileSrc("", "plugin")}${await invoke<string>("plugin_start", { plugin: m.id })}/`;
    const frame = document.createElement("iframe");
    // No allow-same-origin: the frame gets an origin of its own that matches nothing.
    frame.setAttribute("sandbox", "allow-scripts");
    frame.style.display = "none";
    // Loaded as srcdoc so its address isn't one the app takes commands from.
    frame.srcdoc =
      `<!doctype html><meta charset="utf-8">` +
      `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src ${base}">` +
      `<script type="module" src="${base}${RUNTIME}"></script>`;
    const loaded = new Promise<void>((resolve, reject) => {
      frame.addEventListener("load", () => resolve(), { once: true });
      frame.addEventListener("error", () => reject(new Error("its frame didn't load")), { once: true });
    });
    document.body.append(frame);
    await loaded;

    const { port1, port2 } = new MessageChannel();
    const running: Running = {
      manifest: m,
      frame,
      port: port1,
      dom: new PluginDom(m.id, (event, data) => port1.postMessage({ event, ...data }), sidebarItems, sidebarChanged),
    };
    p = running;
    const ready = new Promise<void>((resolve, reject) => {
      const timer = window.setTimeout(() => reject(new Error("it didn't finish starting")), START_TIMEOUT);
      port1.onmessage = ({ data }) => {
        if (typeof data?.ready === "boolean") {
          window.clearTimeout(timer);
          if (data.ready) resolve();
          else reject(new Error(String(data.error)));
        } else {
          void answer(running, data);
        }
      };
    });
    const main = base + m.main.split("/").map(encodeURIComponent).join("/");
    frame.contentWindow!.postMessage({ honeycomb: "init", manifest: m, settings: ownSettings(m.id), main }, "*", [port2]);
    await ready;
    active.set(m.id, running);
    follow();
  } catch (e) {
    if (p) unload(p);
    void invoke("plugin_stop", { plugin: m.id });
    const why = String(e instanceof Error ? e.message : e);
    failed.set(m.id, why);
    throw new Error(`Plugin ${m.name} failed to load: ${why}`);
  }
}

function unload(p: Running) {
  p.dom.dispose();
  p.port.close();
  p.frame.remove();
}

async function stop(id: string) {
  const p = active.get(id);
  if (!p) return;
  // Give its deactivate() a moment, then take the frame away either way.
  await new Promise<void>((resolve) => {
    const timer = window.setTimeout(resolve, 2000);
    p.port.addEventListener("message", ({ data }) => {
      if (data?.deactivated) {
        window.clearTimeout(timer);
        resolve();
      }
    });
    p.port.postMessage({ event: "deactivate" });
  });
  active.delete(id);
  unload(p);
  await invoke("plugin_stop", { plugin: id });
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
