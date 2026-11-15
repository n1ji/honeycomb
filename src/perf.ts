import { invoke } from "@tauri-apps/api/core";
import { api } from "./api";
import { store } from "./store";
import { $ } from "./util";
import { library } from "./ui/listenlib";
import { applyMode } from "./ui/mode";

/**
 * Benchmark mode, on when the app runs with HONEYCOMB_PERF=1. Times startup,
 * search, sorting, filtering and scrolling, and prints `perf:` lines to the
 * terminal running the app. Point HONEYCOMB_DATA_DIR at a test library.
 */
const log = (line: string) => invoke("perf_log", { line });
const frame = () => new Promise<number>((r) => requestAnimationFrame(r));
const ms = (n: number) => `${n.toFixed(1)} ms`;

async function settle() {
  await frame();
  await frame();
}

async function timed(label: string, fn: () => unknown) {
  const t = performance.now();
  await fn();
  await settle();
  const total = performance.now() - t;
  await log(`${label}: ${ms(total)} (${store.visible.length.toLocaleString()} rows)`);
  return total;
}

export async function perfEnabled(): Promise<boolean> {
  return invoke<boolean>("perf_enabled").catch(() => false);
}

/** `mainStart`/`loaded` are performance.now() values from startup. */
export async function runBenchmark(mainStart: number, loaded: number) {
  await settle();
  await log(`tracks: ${store.tracks.size.toLocaleString()}`);
  await log(`startup: page → main ${ms(mainStart)}, main → library loaded ${ms(loaded - mainStart)}, first list paint ${ms(performance.now())} after page start`);

  const t = performance.now();
  await api.getTracks();
  await log(`get_tracks round trip (warm): ${ms(performance.now() - t)}`);
  await timed("library view rebuild", () => store.refreshView());

  for (const q of ["a", "love", "night city", "zzqx", ""]) {
    await timed(`search "${q}"`, () => {
      store.search = q;
      store.recompute();
    });
  }
  await timed("sort by title", () => store.setSort("title"));
  await timed("sort by title (desc)", () => store.setSort("title"));
  await timed("sort off", () => store.setSort("title"));
  const artist = store.visible[Math.floor(store.visible.length / 2)]?.artist ?? null;
  await timed("filter one artist", () => store.setFilter("artist", artist === null ? null : [artist]));
  await timed("clear filter", () => store.setFilter("artist", null));

  // Scroll ten rows per frame and record frame times.
  const scroller = $("#tracklist .tl-scroll");
  scroller.scrollTop = 0;
  await settle();
  const deltas: number[] = [];
  let last = await frame();
  for (let i = 0; i < 300; i++) {
    scroller.scrollTop += 260;
    const now = await frame();
    deltas.push(now - last);
    last = now;
  }
  deltas.sort((a, b) => a - b);
  const avg = deltas.reduce((a, b) => a + b, 0) / deltas.length;
  await log(
    `scroll: ${(1000 / avg).toFixed(0)} fps avg, p95 frame ${ms(deltas[Math.floor(deltas.length * 0.95)])}, worst ${ms(deltas[deltas.length - 1])}`,
  );
  await listeningBenchmark();
  await log("done");
}

/** Scroll `el` a few rows per frame and report frame times. */
async function scrollTimes(label: string, el: HTMLElement, step: number) {
  el.scrollTop = 0;
  await settle();
  const deltas: number[] = [];
  let last = await frame();
  for (let i = 0; i < 300; i++) {
    el.scrollTop += step;
    const now = await frame();
    deltas.push(now - last);
    last = now;
  }
  deltas.sort((a, b) => a - b);
  const avg = deltas.reduce((a, b) => a + b, 0) / deltas.length;
  await log(`${label}: ${(1000 / avg).toFixed(0)} fps avg, p95 frame ${ms(deltas[Math.floor(deltas.length * 0.95)])}, worst ${ms(deltas[deltas.length - 1])}`);
}

/** Listening mode: first paint of the cover grid, search, and scrolling albums and songs. The mode isn't saved. */
async function listeningBenchmark() {
  const was = store.settings.mode;
  const step = async (label: string, fn: () => unknown) => {
    const t = performance.now();
    await fn();
    await settle();
    await log(`listening ${label}: ${ms(performance.now() - t)}`);
  };
  const tab = (name: string) => [...document.querySelectorAll<HTMLElement>("#listen .ln-tab")].find((b) => b.textContent!.startsWith(name))!;
  const search = $("#listen-search") as HTMLInputElement;
  const find = (q: string) => {
    search.value = q;
    search.dispatchEvent(new Event("change"));
  };

  await step("switch + first grid", () => {
    store.settings.mode = "listening";
    applyMode();
    store.emit("mode");
    tab("Albums").click();
  });
  const lib = library();
  await log(`listening library: ${lib.albums.length.toLocaleString()} albums, ${lib.artists.length.toLocaleString()} artists`);
  await scrollTimes("listening album grid scroll", $("#listen .vg"), 300);
  for (const q of ["a", "love", "night city", ""]) await step(`search "${q}"`, () => find(q));
  await step("artists tab", () => tab("Artists").click());
  await step("songs tab", () => tab("Songs").click());
  await scrollTimes("listening song list scroll", $("#listen .vg"), 480);
  await step("back to albums", () => tab("Albums").click());

  store.settings.mode = was;
  applyMode();
  store.emit("mode");
}
