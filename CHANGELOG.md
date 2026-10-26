# Changelog

What changed in each Honeycomb release, newest first. New work goes under
**Unreleased**; `scripts/release.sh` turns that section into the next version
and uses it as the GitHub release notes.

## Unreleased

### New

- **Last.fm scrobbling.** Sign in from Settings → Scrobbling. Songs longer than 30 seconds are scrobbled once you've heard half of them or 4 minutes, and Last.fm shows what's playing now.
- **ListenBrainz scrobbling**, with the user token from your ListenBrainz settings page.
- **Scrobbles made offline are sent later**, in the order you heard them, and only once.
- **Sync to a player or flash drive** (⇅ Sync in the sidebar). Pick a destination and the playlists, or the whole library, to put on it. Honeycomb copies only what changed, writes each playlist as an .m3u8 the device can read, keeps file names safe for FAT32 and exFAT, checks the free space first, and only ever replaces or removes files it put there itself. Cancelling or unplugging mid-sync leaves no half-copied tracks.
- **Rockbox players**: plays logged on the player (turn on playback logging in its settings) are added to your play counts on the next sync.
- **Self-test mode** for release builds: `HONEYCOMB_SMOKE=<folder>` scans the folder, plays every file through the real audio output, and exits with a report. `scripts/smoke-test.sh` runs it on a built app with generated test files.

### Changed

- Releases now have numbered versions (0.1.1, 0.1.2, …) and this changelog.

## 0.1.0 (alpha-1), 2026-10-22

The first alpha for macOS and Windows.

- Library with a Genre / Artist / Album column browser, fast search and sorting, tested with 100,000 tracks.
- Kaseki, Honeycomb's own playback engine: gapless playback, output at the file's own sample rate when the device allows it, surround folded down to stereo.
- Tag editor for one or many tracks, with album art.
- Manual and smart playlists, M3U import and export.
- Import play counts, ratings and playlists from an iTunes-format library XML and M3U files.
- Half-star ratings, Inbox for newly found files, synced lyrics (from the file, a .lrc next to it, or LRCLIB).
- Media keys and the system's Now Playing controls.
- Movable panels, a customizable player bar and track list columns, themes and colors.
- An update bulb in the sidebar that says when a newer release is out.
