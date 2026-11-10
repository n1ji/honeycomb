# Known issues

From the code audit on 2026-11-10. Each item says what goes wrong in plain words, where it is, and how to fix it. **Bug** means it does the wrong thing today. **Risky** means it works until a large library, a slow disk, a missing drive or a bad plugin shows up. **Style** means it only makes the code harder to maintain.

## Fixed

| # | Kind | What went wrong | Where |
|---|---|---|---|
| 1 | Bug | A library folder that couldn't be read (drive unplugged, share offline) looked like "every file was deleted", so its tracks lost their ratings, play counts and playlist spots. The app rescans on every start, so just opening it was enough. | `src-tauri/src/scanner.rs` |
| 2 | Bug | Adding or removing a folder while a scan was running did nothing until the next manual rescan. | `src-tauri/src/scanner.rs` |
| 3 | Risky | Rating, loving or accepting many tracks saved one row at a time, each waiting for the disk, with the window frozen. | `src-tauri/src/lib.rs`, `src-tauri/src/db.rs` |
| 4 | Risky | The cover image written for the system's Now Playing display was never deleted: one file per song ever played. | `src-tauri/src/media.rs` |

## Open

| # | Kind | What goes wrong | Where | Fix |
|---|---|---|---|---|
| 5 | Risky | Slow commands run on the thread that draws the window, so the app freezes while they work: loading 100k tracks, editing tags on many files, importing a playlist, starting a huge queue, checking a sync destination. | `lib.rs`: `get_tracks`, `edit_tags`, `import_m3u`, `export_m3u`, `player_load`, `sync_target`, `plugin_secret_get` | Mark them `#[tauri::command(async)]`. |
| 6 | Risky | Every player command resends the whole queue to the interface. With a 100k-track queue, each volume change or pause sends every track id. | `player.rs`: `emit_queue` | Send the list only when it changes. |
| 7 | Risky | The interface reloads the whole library every 4 seconds while a scan runs. | `src/main.ts`, `scan-progress` listener | Send only the rows that changed, or reload less often for big libraries. |
| 8 | Risky | If the library database can't be opened or upgraded, the app exits with no message. There is also no backup before an upgrade. | `lib.rs`: `run` (`.expect`), `db.rs`: `migrate` | Show a dialog; copy `library.db` before migrating. |
| 9 | Risky | During a long scan the scanner holds the database's write lock almost all the time. The player ignores a failed write, so plays can go uncounted. | `player.rs`: `record_play`, `scanner.rs` | Retry the write, or pause between scan batches. |
| 10 | Bug (minor) | A file that fails to read partway through is treated as having reached its end, so it counts as played. | `kaseki.rs`: `decode_next` | Tell "end of file" apart from "read error". |
| 11 | Risky | With a drive unplugged, the player tries every track in the queue one by one and reports an error for each. | `player.rs`: `restart_at` | Stop after a few failures in a row. |
| 12 | Risky (Windows, not tested) | Editing the tags of the song that's playing likely fails: the decoder holds the file open and Windows won't replace an open file. | `meta.rs`: `save_safely` | Release the file first, or retry after the track ends. |
| 13 | Risky | Nothing runs the tests automatically. CI only builds on release tags and checks dependencies. | `.github/workflows/` | Add a workflow running `cargo test`, `tsc` and the plugin tests on every push. |
| 14 | Risky (low) | Two sync writes (the playlist files and the manifest) don't have the protection against planted links that track copies have. Needs a deliberately crafted drive. | `sync.rs`: `write_playlist`, `save_manifest` | Create the files with `create_new`, as `copy_file` does. |
| 15 | Risky (low) | Output devices that only take 24-bit, 8-bit or 64-bit samples are refused. `.mp4` videos in a music folder are scanned as songs. Changes within the same second as the last scan are missed. | `kaseki.rs`: `Output::open`, `meta.rs`: `AUDIO_EXTS`, `file_mtime` | Add the formats; check for an audio-only file; compare size too. |
| 16 | Style | `import_m3u` repeats the importer's playlist reader without its fallback for old Windows-encoded files. `lib.rs` holds all 55 commands in one file. Logs go only to the terminal, so a user can't send one. `db::playlist` loads every playlist to find one. | `lib.rs`, `db.rs` | Reuse `importer::read_m3u`; split commands by area; write a log file. |

## Not found

- Dead code: the compiler reports none, the TypeScript check forbids it, and all 55 commands are called from the interface.
- HTML injection: the interface builds pages from DOM nodes, never from strings.
- Tauri configuration: permissions are minimal, the content policy blocks inline and remote scripts, and updates are signature-checked.
