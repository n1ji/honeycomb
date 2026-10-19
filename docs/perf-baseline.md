# Performance baseline

First baseline, recorded on 2026-10-19 with commit `0d83811` plus the audio and update work. Release build (`npm run tauri build -- --no-bundle`).

- **Machine:** MacBook Pro, Apple M1 Pro, 16 GB, macOS 27.0, **Low Power Mode on** (WebKit caps animation at 30 fps).
- **Library:** 100,000 generated tracks, about 5,000 artists (no audio files; `gen_bench_library` in `db.rs`).
- **How to reproduce:**
  ```bash
  cd src-tauri
  HONEYCOMB_BENCH_DIR=/tmp/hc-bench HONEYCOMB_BENCH_TRACKS=100000 cargo test --release --lib gen_bench_library -- --ignored
  cd .. && npm run tauri build -- --no-bundle
  HONEYCOMB_PERF=1 HONEYCOMB_DATA_DIR=/tmp/hc-bench src-tauri/target/release/honeycomb
  ```
  The app prints `perf:` lines to the terminal. Memory and idle CPU come from `ps`: the app plus its WebKit helper processes, idle (nothing playing) for 30 s.

| What | Result | Notes |
|---|---|---|
| Startup to first list paint | **2.1 s** | DB query 193 ms. Most of the time is moving 100k tracks to the UI and building the view |
| `get_tracks` round trip (warm) | 346 ms | DB query 105 ms. The rest is JSON serialization over IPC |
| Library view rebuild | 1.4 s | Sorting 100k rows with `localeCompare` |
| Search (any query) | 210–270 ms | About the same even with 0 results, so the cost is per-keystroke overhead, not matching |
| Sort by title | **3.0–3.4 s** | `localeCompare` with options, called about 1.7M times |
| Filter one artist | 94 ms | |
| Scroll (10 rows per frame) | 30 fps, p95 36 ms, worst 49 ms | Capped by Low Power Mode. Re-measure with it off |
| Memory | **639 MB** | App 90 MB, WebKit WebContent 508 MB (JS heap holding all tracks), GPU 33 MB, network 8 MB |
| Idle CPU | **6.8%** of one core | Player ticks every 200 ms, even when nothing plays |

## Likely fixes, biggest first

1. **Sorting:** use one shared `Intl.Collator` instead of `localeCompare` per comparison, and cache sort keys. This should cut sort and rebuild times by roughly 10×.
2. **Idle CPU:** only send player ticks while playing (or when something changed), and check the audio device less often while idle. *Done with the Kaseki audio engine; see below.*
3. **Search:** precompute each track's lowercase search text once, and avoid rebuilding the column browser's panes when only the search changed.
4. **Startup and memory:** send tracks in a compact form (arrays instead of objects with repeated keys), or load the visible rows on demand instead of the whole library.

## Idle CPU after the Kaseki audio engine

Measured 2026-10-19 on the same machine (Low Power Mode on), same 100k library, release builds. The old build (`2aca6fd`) and the new one ran one after the other with the same method: 20 s to settle, then CPU time of the app plus the WebKit helper processes it started, over 30 s.

| | App | WebContent | GPU | Total |
|---|---|---|---|---|
| Before (`2aca6fd`) | 2.43% | 2.50% | 0.93% | **5.86%** |
| After | 1.03% | 0.87% | 0.43% | **2.33%** |

With Kaseki, the player no longer opens the output until something plays, closes it after 30 s paused or stopped, and stops ticking while idle. A `sample` of the idle app shows the `audio` and `decoder` threads blocked for the whole run, so what's left is WebKit and the main event loop.
