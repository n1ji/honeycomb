# Honeycomb

A fast, library-focused desktop music player for macOS and Windows, built with [Tauri](https://tauri.app) (Rust + TypeScript) and its own audio engine, Kaseki.

**Website:** [honeycomb.n1ji.dev](https://honeycomb.n1ji.dev)

> **Status:** early alpha. Expect rough edges, and keep a backup of anything you care about.

## Download

The first alpha is on the [Releases page](https://github.com/n1ji/honeycomb/releases):

- **macOS:** `Honeycomb_0.1.0_universal.dmg` (Apple Silicon and Intel)
- **Windows:** `Honeycomb_0.1.0_x64-setup.exe`, or `Honeycomb_0.1.0_x64_en-US.msi`

Linux builds will come with a later release.

The builds aren't code-signed yet, so your system will warn you the first time:

- **macOS:** open Honeycomb once and dismiss the warning. Then go to **System Settings → Privacy & Security**, scroll down to the message about Honeycomb, click **Open Anyway** and confirm. You only need to do this once. (Right-click → Open no longer skips the warning on macOS 15 and later.) If you'd rather use Terminal, `xattr -cr /Applications/Honeycomb.app` does the same.
- **Windows:** if SmartScreen says it protected your PC, click **More info**, then **Run anyway**.

## Features

**Library and browsing**
- Add music folders and Honeycomb scans them in the background. Unchanged files are skipped on rescans, and files you delete leave the library.
- A Genre → Artist → Album column browser above a virtualized track list that stays fast with 100,000 tracks.
- An **Inbox** that collects music newly added to folders you already scan.
- A sidebar list you can switch between Playlists, Artists, Albums and Genres.

**Playback**
- Gapless playback of MP3, FLAC, AAC and ALAC (M4A), Ogg Vorbis, WAV and AIFF, through Kaseki (see below).
- A play queue called **Next Up**, with shuffle, repeat all and repeat one.
- Media keys and the system's Now Playing controls.

**Lyrics**
- Synced lyrics in the Now Playing panel: the current line is highlighted, and clicking a line jumps there.
- Lyrics come from the file's tags first, then a `.lrc` file next to it, then [LRCLIB](https://lrclib.net) online. Online lookup sends the song's title, artist, album and length, and can be turned off in Settings.

**Tags and artwork**
- Edit one track or many at once: title, artist, album artist, album, genre, composer, year, track and disc numbers, comment. Only the fields you change are written.
- Embed or remove cover art. Click the artwork in Now Playing to see it full size and save it.

**Playlists, ratings and stats**
- Manual playlists (drag tracks onto them), smart playlists built from rules, and M3U/M3U8 import and export.
- Half-star ratings, set straight from the track list. Smart playlist rules use the same scale, so 3.5 means three and a half stars.
- Play counts and last-played times.

**Coming from another player**
- Import ratings, play counts, play dates and playlists from an iTunes-format library XML, which many desktop players can export, and from M3U playlists. Tracks are matched to your library by file path, then by artist, title, album and length.
- Loved tracks come across too. They're stored and usable in smart playlists, but not shown in the app yet.

**Look and layout**
- Dark, Light and Midnight themes, or match the system. Pick an accent color, background and secondary colors, and normal or compact rows.
- Put the sidebar on the left or right, the Now Playing panel on the left, right or hidden, and the player bar at the top or bottom. Show or hide the column browser, and reorder or hide player bar buttons.
- Track list columns can be dragged to reorder and shown or hidden, including Album Artist, Composer, Disc, Format, Bitrate, Sample Rate, Added, Last Played and Comment.

**Updates**
- A bulb in the sidebar lights up when a newer release is out. Click it, then Install, and Honeycomb downloads the new version, checks its signature and restarts on it.

Your library database lives in the app data folder. Honeycomb never moves or deletes your music files, and it only writes to them when you save in the tag editor.

## Kaseki, the audio engine

Kaseki is written in Rust for Honeycomb ([how it works](https://honeycomb.n1ji.dev/kaseki/)):

- **Bit-perfect when it can be.** The output opens at the file's own sample rate whenever the device supports it, so the samples go out unchanged. A test checks this bit for bit.
- **Careful when it can't.** Otherwise it picks the best rate the device offers (a 192 kHz file plays at 96 kHz on a MacBook's headphone jack) and converts with a band-limited sinc resampler.
- **Truly gapless.** The next track decodes into the same buffer, with encoder delay and padding trimmed.
- **Decodes ahead.** Decoding runs on its own thread, about 5 seconds ahead. The real-time audio callback never touches files, so a slow drive can't cause dropouts.
- **Surround folds down** to stereo with proper channel weights.
- **A perceptual volume curve** with 10 ms ramps, so volume changes and pauses don't click.

## Performance

Measured on a generated 100,000-track library on an M1 Pro (Low Power Mode on), release build:

| | Time |
| --- | --- |
| Startup to the first list | 0.95 s |
| Sort by title | 68–76 ms |
| Search, per keystroke | 62–65 ms |

These are one machine's numbers, not a guarantee. Method, the full results and how to reproduce them: [docs/perf-baseline.md](docs/perf-baseline.md).

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

Tests: `cd src-tauri && cargo test` (smart playlists, the importer and the Kaseki engine; some engine tests use ffmpeg).

## Release builds

Pushing a tag such as `v0.1.0-alpha.1` builds a universal macOS `.dmg`, the Windows installers and the Linux packages, and attaches them to a draft pre-release. The **Build** workflow can also be run by hand from the Actions tab, picking the platforms.

## Project layout

```
src/                 TypeScript UI (no framework)
  ui/                sidebar, column browser, track list, player bar, lyrics, dialogs
  store.ts           app state and view filtering/sorting
  api.ts             typed wrappers around the Rust commands
src-tauri/src/
  lib.rs             Tauri commands and the album-art protocol
  player.rs          player thread: queue, shuffle/repeat, device handling
  kaseki.rs          Kaseki audio engine: decoder thread, ring buffer, resampling, output (symphonia + rubato + cpal)
  scanner.rs         background library scanning
  meta.rs            tag and artwork reading/writing (lofty)
  lyrics.rs          lyrics from tags, .lrc files and LRCLIB
  importer.rs        import from iTunes-format library XML and M3U
  media.rs           media keys and the system Now Playing controls
  version.rs         the update check
  smart.rs           smart playlist rules → SQL
  db.rs              SQLite library database
```

## License

Copyright (C) 2026 n1ji

Honeycomb is free software under the GNU General Public License, version 3 or
any later version. See [LICENSE](LICENSE).

The website's images and videos in `site/assets` are not covered by the GPL.
Each keeps its own license (most are public domain), listed with its creator
and source in [site/assets/credits.json](site/assets/credits.json).

Honeycomb is an independent project. It is not affiliated with or derived from any other music player.
