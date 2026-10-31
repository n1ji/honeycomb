# Honeycomb roadmap: November 2026 to April 2027

**Goal:** a native, fast, library-focused music player for macOS and Linux (Windows too) that people coming from other desktop players can switch to without losing anything they rely on.

**What makes it better:**
1. A real native app on Mac and Linux, not Wine.
2. Fast with huge libraries: measurably faster startup, search and scrolling at 100k tracks.
3. Painless migration: play counts, ratings and playlists come with you (done).

**The plan is built around two must-have features:** Last.fm scrobbling, and syncing music to players and flash drives.

This is a plan, not a promise. Each month has a "done when" line. Whatever slips moves into the next month, and the roadmap gets reviewed at the end of every month.

---

## Where things stand (reviewed 2026-10-31)

October went faster than planned: most of Months 1–4 is already built. Every item in this file carries one of these marks:

| Mark | Meaning |
| --- | --- |
| 🚀 | **Live:** in the newest release (0.1.1), so people who download Honeycomb have it |
| 🔧 | **On dev:** done and pushed to `dev`, waiting for the next release (listed under "Unreleased" in `CHANGELOG.md`) |
| 🟡 | Partly done, or done but not tested yet (the note says which, and whether it's live) |
| (none) | Not started |

To check yourself: `git log --oneline v0.1.1..origin/dev` lists what's on dev but not released, and `git log --oneline origin/dev..dev` lists local commits that aren't pushed yet.

**Next up (picked 2026-10-31, aimed at 0.2)**

These come first, ahead of the month order below:

1. **Remember unreadable files (#6, #2).** Store each file the scanner can't read with its mtime and the error, skip it on rescans until it changes, and list them in Settings → Library with a "Read again" button. The list also shows which formats fail and why.
2. **Show the loved flag.** A heart in the track list and player bar, and Love / Unlove in the context menu. Needed before loves sync with Last.fm.
3. **Linux on release tags.** After one test on a real distro, add Linux to the tag default in `.github/workflows/build.yml` (today it only builds on manual runs).
4. **ReplayGain.** Read track and album gain and peak tags, apply them in Kaseki's gain stage (switching exactly at the gapless marker), with a setting (Off / Track / Album), clipping prevention from the peak, and a line in the signal path view.
5. **Watch library folders.** Use the `notify` crate to start the quick rescan a moment after files change, so new music lands in the Inbox without "Rescan now". If a scan is already running, run another one when it finishes.
6. **Album art grid view.** A grid of albums for whatever the column browser and search currently show, with cached cover thumbnails so it stays fast at 100k tracks. Click to filter to the album, double-click to play it.

## October 2026: done ahead of plan

Everything finished so far, including items that were planned for later months and moved here when they were done.

**Live in 0.1.1**

- 🚀 Library, column browser, tag editor, smart playlists, gapless playback, play counts.
- 🚀 Kaseki, Honeycomb's own audio engine.
- 🚀 Importer for iTunes-format library XML and M3U files, tested on a real 4.5k-track export.
- 🚀 Half-star ratings and a loved flag (stored, not shown yet).
- 🚀 Resizable panels and columns, themes, accent color, custom background and secondary colors.
- 🚀 Plugins (moved up from "Later"), with scrobbling as the first bundled one.
- 🚀 Scrobbling to Last.fm and ListenBrainz (was Month 2): browser login with the session in the system keychain, "now playing" updates, Last.fm's rules (tracks over 30 seconds, after half the track or 4 minutes), and an offline queue sent later, in order, with no duplicates.
- 🚀 Sync to players and flash drives (was Month 3): pick a destination and playlists, smart playlists or the whole library; copy only what changed, remove only files Honeycomb put there, check free space first; M3U8 playlists with relative paths; FAT32/exFAT-safe names with long paths shortened; a progress bar, and safe to cancel or unplug.
- 🚀 Rockbox play counts read back from `playback.log` (was a Month 4 stretch goal).
- 🚀 Lyrics: embedded, `.lrc` files and online lookup, synced lines highlighted (was Month 5).
- 🚀 Media keys and the system Now Playing display.

**On dev, coming in the next release**

- 🔧 In-app updates: signed releases install from the update bulb.
- 🔧 Quality badge and signal path view; bit depth column and smart rule.
- 🔧 Converting while syncing (was Month 4): lossless → MP3, AAC, Ogg Vorbis or 16-bit FLAC, re-syncs only convert what changed. Built-in encoders instead of ffmpeg, so there's no license question. AAC is macOS-only for now (`afconvert`).
- 🔧 Moving or renaming files keeps their play counts, ratings and playlist spots.
- 🔧 Inbox as a waiting room for new files, and ignored files.
- 🔧 Multi-pick and right-click in the column browser.
- 🔧 Listening history (what played, when, and how it ended), off until the user turns it on. The groundwork for sound-based playlists.
- 🔧 Setup screen for new users (and once for existing ones), smoother motion, clickable artist and album names, Shuffle in the track menu.
- 🔧 Multi-device plan over OpenSubsonic (`docs/ARCHITECTURE.md`): approved, a document only, nothing built.

## Month 1 (November): ship something people can install

- 🟡 **Decide:** public repo (since 2026-10-22) and keeping the name are decided; still open: when to buy the Apple Developer account.
- 🚀 Change the app identifier to `dev.n1ji.honeycomb` and move the existing library database across automatically.
- 🟡 Linux in CI (live in the workflow): AppImage, .deb and .rpm build on manual runs. Still to do: test on a real distro, then add Linux to tag builds. The webview on Linux (WebKitGTK) is the main scrolling risk.
- 🔧 Built-in auto-updater (Tauri updater with a signing key; first release with it is the next one).
- 🚀 A download page (honeycomb.n1ji.dev).
- 🚀 Performance baseline: 100k-track library, see `docs/perf-baseline.md` (startup 0.95 s, search ~65 ms, sort ~70 ms).
- 🟡 Fix: removing the playing track stops playback (#4), and audio dies when headphones are unplugged (#5). Fixed in code and live in 0.1.1, still needs a listening test.

**Done when:** a stranger can download an alpha for Mac or Linux, install it, and receive the next update automatically.

## Month 2 (December): Last.fm

Sign-in, scrobbling and the offline queue were done in October.

- Sync loves both ways with the loved flag. (The loved flag isn't shown in the UI yet either.)
- Optional: import play counts from Last.fm for tracks that have none locally.
- 🟡 Media keys and the system Now Playing controls (`media.rs`). Live in 0.1.1 and works on macOS; not tested on Windows or Linux, and media keys sometimes going to a browser tab isn't confirmed fixed.

**Done when:** a week of daily listening shows up correctly on Last.fm, including songs played offline.

**Note:** Last.fm needs an API key and secret in the app. Scrobbling is a plugin that ships with Honeycomb, and CI writes the key into it from the repo's encrypted secrets at build time, so it's never in the repo. Builds without it ask users for their own key.

## Month 3 (January): sync to flash drives and USB players

Sync itself was done in October. What's left:

- Folder layout options (today it's always Artist/Album/NN Title).
- The car-stereo test from "Done when".

**Done when:** syncing 5 playlists to a USB stick plays correctly in a car stereo, and re-syncing after a change copies only the difference.

## Month 4 (February): sync, part 2

Converting and reading Rockbox play counts were done in October.

- Android phones over MTP. This is hard on Mac, so check what's realistic early. If it isn't, fall back to "sync to a folder" plus a phone sync app.
- Rockbox: scrobble the plays read from `playback.log`. Needs a UTC offset first (Rockbox logs local time).

**Done when:** a lossless library syncs to a small player as lossy files, and a phone works or the documented fallback does.

## Month 5 (March): the rest of what blocks switching

Lyrics were done in October.

- Watch library folders for new and changed files instead of manual rescans.
- An album art grid view.
- ReplayGain (track and album volume leveling).
- A basic equalizer.
- 🟡 Fix: changing the queue in the last 2 seconds of a track (#3) is fixed in code (live in 0.1.1) and needs a listening test. Still open: unreadable files being re-read on every rescan (#6), and why 64 files fail to read (#2).

**Done when:** Honeycomb works as someone's only player for two weeks without them going back.

## Month 6 (April): polish toward 1.0

- Performance pass against the Month 1 baseline: load the library lazily so startup stays fast at 100k tracks, a faster search index, and cached artwork thumbnails.
- Customization: choose, order and resize track list columns, and pick the column browser's fields.
- A signed and notarized Mac build (needs the Apple account), and Linux packages on Flathub and/or the AUR.
- Windows builds kept working, even though it isn't the main target.
- A user guide covering migration, sync and Last.fm setup.
- Launch on Product Hunt with 1.0 (not before: the first launch gets the most attention). Prepare the gallery, the demo video, a maker's first comment, and early users who'll be around on launch day. Update the AlternativeTo listing and post to r/MacApps at the same time.

**Done when:** a 1.0 beta is announced to Mac and Linux music library users.

---

## Later (not in these 6 months)

Online tag and artwork lookup, automatic file renaming and organizing, theme files and custom CSS, saved layout profiles, podcasts, internet radio, CD ripping.

Multi-device over OpenSubsonic, planned in detail in `docs/ARCHITECTURE.md` (phases 1–6): the workspace split into shared crates, an OpenSubsonic server in the desktop app for paired phones, the desktop as a client of Navidrome and other Subsonic servers (streaming through Kaseki, pinning albums offline, ratings and play counts going back to the server), then the iOS app (separate, private repository) and cable sync. Phase 1 (the crate split) is small and could start early.

Sound-based playlists: score tracks by measured sound (grit, punch, density, BPM) with Honeycomb's own analysis code, learn from the listening history, and only then mix in Last.fm similarity.

## Ground rules

- **Original work:** Honeycomb may share familiar ideas with other players, but never their code, assets, skins or branding.
- **Never test tag editing or sync deletes on a real music collection.** Use generated test libraries.
