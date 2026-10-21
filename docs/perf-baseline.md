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

1. **Sorting:** use one shared `Intl.Collator` instead of `localeCompare` per comparison, and cache sort keys. This should cut sort and rebuild times by roughly 10×. *Done; see below.*
2. **Idle CPU:** only send player ticks while playing (or when something changed), and check the audio device less often while idle. *Done with the Kaseki audio engine; see below.*
3. **Search:** precompute each track's lowercase search text once, and avoid rebuilding the column browser's panes when only the search changed. *Done; see below.*
4. **Startup and memory:** send tracks in a compact form (arrays instead of objects with repeated keys), or load the visible rows on demand instead of the whole library. *Compact form done; loading on demand not needed yet.*

## Idle CPU after the Kaseki audio engine

Measured 2026-10-19 on the same machine (Low Power Mode on), same 100k library, release builds. The old build (`2aca6fd`) and the new one ran one after the other with the same method: 20 s to settle, then CPU time of the app plus the WebKit helper processes it started, over 30 s.

| | App | WebContent | GPU | Total |
|---|---|---|---|---|
| Before (`2aca6fd`) | 2.43% | 2.50% | 0.93% | **5.86%** |
| After | 1.03% | 0.87% | 0.43% | **2.33%** |

With Kaseki, the player no longer opens the output until something plays, closes it after 30 s paused or stopped, and stops ticking while idle. A `sample` of the idle app shows the `audio` and `decoder` threads blocked for the whole run, so what's left is WebKit and the main event loop.

## After the sorting, search and loading fixes

Measured 2026-10-21, same machine and 100k library, Low Power Mode still on. Both release builds ran back to back on fresh copies of the same library with `HONEYCOMB_PERF=1`: "before" is `ed2a658` (Kaseki), "after" adds:

- **Sorting:** one shared `Intl.Collator`, and each text value's rank in collation order computed once per library load. Sorts then compare plain numbers, with ties broken by a precomputed library-order position. The library view reuses the precomputed order.
- **Search:** each track's lowercase search text is built once per library load.
- **Column browser:** a pane is rebuilt only when its contents change. Searching or sorting leaves the panes alone, and picking an artist only moves the highlight in the genre pane.
- **Loading:** `get_tracks` sends field names once plus one array per track (`TrackTable`) instead of 100k objects with repeated keys.

| What | Before | After |
|---|---|---|
| Startup to first list paint | 2.4 s | **0.95 s** |
| `get_tracks` round trip (warm) | 335 ms | 289 ms |
| Library view rebuild | 1.5 s | **142 ms** |
| Search (any query) | 230–252 ms | **62–65 ms** |
| Sort by title | 3.1–3.6 s | **68–76 ms** |
| Sort off | 225 ms | 43 ms |
| Filter one artist | 133 ms | 65 ms |
| Clear filter | 222 ms | 79 ms |
| Scroll (10 rows per frame) | 30 fps, p95 36 ms, worst 47 ms | 30 fps, p95 34 ms, worst 35 ms |
| Memory (RSS after the run) | 222 MB | 196 MB |
| Idle CPU (30 s) | 1.1% | 1.7% |

Memory here is resident memory after 20 s idle (`ps` RSS) of the app plus the WebKit processes it started, which macOS compresses, so it's lower than the first baseline's 639 MB. Compare the two columns with each other, not with that number. Idle CPU is about 1–2% either way; a 30 s sample at this level is mostly noise. Scrolling is still capped at 30 fps by Low Power Mode.

