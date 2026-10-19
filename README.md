# Honeycomb

A fast, library-focused desktop music player for macOS and Windows, built with [Tauri](https://tauri.app) (Rust + TypeScript).

> **Status:** early alpha.

## Features

- **Library**: add music folders and Honeycomb scans them in the background. Unchanged files are skipped on rescans, and files you delete are removed from the library.
- **Column browser**: Genre → Artist → Album panes above a fast, virtualized track list that handles libraries with tens of thousands of tracks.
- **Playback**: gapless playback through Kaseki, its native Rust audio engine (MP3, FLAC, AAC/ALAC in M4A, Ogg Vorbis, WAV, AIFF), with a queue, shuffle, repeat all and repeat one.
- **Tag editor**: edit one track or many at once, including album artist, composer, year, track and disc numbers. You can embed or remove cover art.
- **Playlists**: manual playlists (drag tracks onto them), smart playlists with rules, and M3U/M3U8 import and export.
- **Ratings and stats**: 0–5 star ratings, play counts and last-played times.
- **Themes**: Dark, Light, Midnight or match the system, with a custom accent color and compact row density.

Your library database lives in the app data folder. Honeycomb never moves or deletes your music files, and it only writes to them when you save in the tag editor.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| Space | Play / pause |
| Ctrl/⌘ + → / ← | Next / previous track |
| Ctrl/⌘ + F | Search |
| Ctrl/⌘ + , | Settings |
| Enter | Play selected track |
| Delete | Remove from playlist / queue |
| Ctrl/⌘ + A | Select all |

## Building from source

Prerequisites: [Node.js](https://nodejs.org) 20+, [Rust](https://rustup.rs), and the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS (Xcode Command Line Tools on macOS, the MSVC build tools on Windows).

```bash
npm install
npm run tauri dev      # run in development
npm run tauri build    # build an installer (.dmg / .msi / .exe) into src-tauri/target/release/bundle
```

## Alpha builds

Pushing a tag such as `v0.1.0-alpha.1` (or running the **Build** workflow manually) builds a universal macOS `.dmg` and a Windows installer, then attaches them to a draft pre-release.

Builds are not code-signed yet. On macOS, right-click the app and choose **Open** the first time.

## Project layout

```
src/                 TypeScript UI (no framework)
  ui/                sidebar, column browser, track list, player bar, dialogs
  store.ts           app state and view filtering/sorting
  api.ts             typed wrappers around the Rust commands
src-tauri/src/
  lib.rs             Tauri commands and the album-art protocol
  player.rs          player thread: queue, shuffle/repeat, device handling
  kaseki.rs          Kaseki audio engine: decoder thread, ring buffer, resampling, output (symphonia + rubato + cpal)
  scanner.rs         background library scanning
  meta.rs            tag and artwork reading/writing (lofty)
  smart.rs           smart playlist rules → SQL
  db.rs              SQLite library database
```

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

Honeycomb is an independent project. It is not affiliated with or derived from any other music player.
