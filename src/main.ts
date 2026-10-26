import { listen } from "@tauri-apps/api/event";
import { api, QueueState, Tick } from "./api";
import { perfEnabled, runBenchmark } from "./perf";
import { store } from "./store";
import { $, modKey, toast } from "./util";
import { run } from "./ui/actions";
import { mountBrowser, mountToolbar } from "./ui/browser";
import { mountNowPlaying, mountPlayerBar } from "./ui/playerbar";
import { applyLayout, mountSplitters } from "./ui/layout";
import { applyTheme, openSettings } from "./ui/settings";
import { mountSidebar } from "./ui/sidebar";
import { mountTrackList } from "./ui/tracklist";
import "./styles.css";

interface ScanProgress {
  done: number;
  total: number;
  current: string;
}
interface ScanFinished {
  addedOrUpdated: number;
  removed: number;
  errors: number;
  inboxed: number;
}
interface Played {
  id: number;
  playCountDelta: number;
  lastPlayed: number;
}

async function main() {
  const mainStart = performance.now();
  store.settings = await api.getSettings();
  applyTheme();
  applyLayout();

  const sidebar = mountSidebar();
  mountToolbar();
  mountBrowser();
  mountTrackList();
  mountPlayerBar();
  mountNowPlaying();
  mountSplitters();
  sidebar.setStatus("");

  await listen<Tick>("player-tick", (e) => {
    store.tick = e.payload;
    store.emit("tick");
  });
  await listen<QueueState>("player-queue", (e) => {
    const prevError = store.queue.error;
    store.setQueue(e.payload);
    if (e.payload.error && e.payload.error !== prevError) toast(e.payload.error, true);
  });
  await listen<string>("player-error", (e) => toast(e.payload, true));
  await listen<Played>("track-played", (e) => {
    const t = store.tracks.get(e.payload.id);
    if (!t) return;
    store.tracks.set(t.id, { ...t, playCount: t.playCount + e.payload.playCountDelta, lastPlayed: e.payload.lastPlayed });
    store.emit("tracks");
  });

  let lastReload = 0;
  await listen<ScanProgress>("scan-progress", (e) => {
    const { done, total } = e.payload;
    sidebar.setStatus(`Scanning… ${done.toLocaleString()} / ${total.toLocaleString()}`);
    // Show new music as it arrives on big first scans, without reloading constantly.
    if (Date.now() - lastReload > 4000) {
      lastReload = Date.now();
      run(() => store.loadTracks(true));
    }
  });
  await listen<ScanFinished>("scan-finished", (e) => {
    sidebar.setStatus("");
    const { addedOrUpdated, removed, errors, inboxed } = e.payload;
    if (addedOrUpdated || removed) {
      toast(
        `Library updated: ${addedOrUpdated} added or changed, ${removed} removed${errors ? `, ${errors} unreadable` : ""}` +
          (inboxed ? `. ${inboxed} new in Inbox.` : ""),
      );
    }
    run(() => store.loadTracks(true));
  });
  await listen<string>("scan-error", (e) => {
    sidebar.setStatus("");
    toast(`Scan failed: ${e.payload}`, true);
  });

  await store.loadPlaylists();
  await store.loadTracks();
  const loaded = performance.now();
  await api.cmd("emit");
  void api.uiReady(store.tracks.size);
  if (await perfEnabled()) void runBenchmark(mainStart, loaded);

  // Pick up files changed while the app was closed.
  const folders = await api.getFolders();
  if (folders.length) api.rescan();
  else if (store.tracks.size === 0) openSettings();

  window.addEventListener("keydown", (e) => {
    const target = e.target as HTMLElement;
    const typing = target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.tagName === "SELECT";
    if (modKey(e) && e.key.toLowerCase() === "f") {
      e.preventDefault();
      ($("#search") as HTMLInputElement).focus();
      return;
    }
    if (modKey(e) && e.key === ",") {
      e.preventDefault();
      openSettings();
      return;
    }
    if (typing || document.querySelector(".backdrop")) return;
    if (e.key === " ") {
      e.preventDefault();
      run(() => api.cmd("toggle"));
    } else if (modKey(e) && e.key === "ArrowRight") {
      run(() => api.cmd("next"));
    } else if (modKey(e) && e.key === "ArrowLeft") {
      run(() => api.cmd("prev"));
    }
  });

  // Disable the webview's default context menu outside our own menus.
  window.addEventListener("contextmenu", (e) => {
    const t = e.target as HTMLElement;
    if (!(t.tagName === "INPUT" || t.tagName === "TEXTAREA")) e.preventDefault();
  });
}

main().catch((e) => toast(String(e), true));
