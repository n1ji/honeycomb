import { invoke } from "@tauri-apps/api/core";
import { api } from "./api";
import { store } from "./store";
import { $ } from "./util";

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
  await log("done");
}
