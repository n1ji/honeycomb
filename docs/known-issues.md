# Known issues

From the code audit on 2026-11-10. Each item says what goes wrong in plain words, where it is, and how to fix it. **Bug** means it does the wrong thing today. **Risky** means it works until a large library, a slow disk, a missing drive or a bad plugin shows up. **Style** means it only makes the code harder to maintain.

## Fixed

| # | Kind | What went wrong | Where |
|---|---|---|---|
| 1 | Bug | A library folder that couldn't be read (drive unplugged, share offline) looked like "every file was deleted", so its tracks lost their ratings, play counts and playlist spots. The app rescans on every start, so just opening it was enough. | `src-tauri/src/scanner.rs` |
| 2 | Bug | Adding or removing a folder while a scan was running did nothing until the next manual rescan. | `src-tauri/src/scanner.rs` |
| 3 | Risky | Rating, loving or accepting many tracks saved one row at a time, each waiting for the disk, with the window frozen. | `src-tauri/src/lib.rs`, `src-tauri/src/db.rs` |
| 4 | Risky | The cover image written for the system's Now Playing display was never deleted: one file per song ever played. | `src-tauri/src/media.rs` |
| 5 | Risky | Slow commands ran on the thread that draws the window, so the app froze while they worked: loading 100k tracks, editing tags on many files, importing or exporting a playlist, checking a sync destination, plugin logins. Starting the queue stays on it, to keep its order with the other player commands. | `lib.rs` |
| 6 | Risky | Every player command resent the whole queue to the interface, so with a 100k-track queue each volume change sent every track id. | `player.rs`: `emit_queue` |
| 7 | Risky | The interface reloaded the whole library every 4 seconds while a scan ran, however long a reload took. | `src/main.ts` |
| 8 | Risky | If the library database couldn't be opened or upgraded, the app exited with no message, and there was no backup before an upgrade. | `lib.rs`: `open_library`, `db.rs`: `backup_before_upgrade` |
| 9 | Risky | A scan batch stayed open while slow files were read, holding the write lock past what the player waits to save a play, so plays went uncounted. | `scanner.rs`, `player.rs`: `record_play` |
| 10 | Bug | A file or stream that failed partway through counted as played. | `kaseki.rs`: `decode_next`, `player.rs`: `poll` |
| 11 | Risky | With a drive unplugged, the player tried every track in the queue and reported an error for each. It now stops after five. | `player.rs`: `restart_at` |
| 12 | Risky | Nothing ran the tests automatically. | `.github/workflows/test.yml` |
| 13 | Risky (low) | The sync manifest and playlist files could be written through a link planted on the drive. | `sync.rs`: `write_replacing` |

## Open

| # | Kind | What goes wrong | Where | Fix |
|---|---|---|---|---|
| 14 | Risky (Windows, not tested) | Editing the tags of the song that's playing likely fails: the decoder holds the file open and Windows won't replace an open file. | `meta.rs`: `save_safely` | Release the file first, or retry after the track ends. |
| 15 | Risky (low) | Output devices that only take 24-bit, 8-bit or 64-bit samples are refused. `.mp4` videos in a music folder are scanned as songs. Changes within the same second as the last scan are missed. | `kaseki.rs`: `Output::open`, `meta.rs`: `AUDIO_EXTS`, `file_mtime` | Add the formats; check for an audio-only file; compare size too. |
| 16 | Style | `import_m3u` repeats the importer's playlist reader without its fallback for old Windows-encoded files. `lib.rs` holds all its commands in one file. Logs go only to the terminal, so a user can't send one. `db::playlist` loads every playlist to find one. | `lib.rs`, `db.rs` | Reuse `importer::read_m3u`; split commands by area; write a log file. |

## Not found

- Dead code: the compiler reports none, the TypeScript check forbids it, and all 55 commands are called from the interface.
- HTML injection: the interface builds pages from DOM nodes, never from strings.
- Tauri configuration: permissions are minimal, the content policy blocks inline and remote scripts, and updates are signature-checked.
