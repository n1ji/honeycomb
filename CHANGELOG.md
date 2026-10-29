# Changelog

What changed in each Honeycomb release, newest first. New work goes under
**Unreleased**; `scripts/release.sh` turns that section into the next version
and uses it as the GitHub release notes.

## Unreleased

## 0.1.1, 2026-10-29

So this one is a big one. The two things I promised right after the alpha are here, scrobbling and syncing to players, plus a plugin system that scrobbling now runs on, and a whole lot of security work that happened before the repo went public.

### Scrobbling (Last.fm and ListenBrainz)

Scrobbling is a plugin that comes with Honeycomb, so it stays off until you want it. Go to Settings → Plugins, turn on Scrobbling and sign in right there. For Last.fm you sign in on Last.fm's own website, so Honeycomb never sees your password, and for ListenBrainz you paste the user token from your ListenBrainz settings page. A song counts once you've heard half of it or 4 minutes of it (only songs over 30 seconds, that's Last.fm's rule), and both services show what you're playing right now. If you're offline the scrobbles just wait and go out later, in the order you heard them and only once. Signing out of a service drops whatever was still waiting for it.

Why a plugin and not built in? Well, it keeps the core app smaller, and it's also the best real-life test the plugin system could get. Release builds come with Honeycomb's Last.fm API key, which gets added while the app is being built and is NOT in the repo. If you build Honeycomb yourself without one, the plugin lets you paste your own key instead.

### Sync to a player or flash drive

There's a new ⇅ Sync button at the bottom of the sidebar. Pick where to sync and which playlists (or the whole library), and Honeycomb copies over only what changed, writes each playlist as an .m3u8 the device can read, keeps file names safe for FAT32 and exFAT, and checks there's enough free space before it starts. It keeps a small manifest file on the device and only ever replaces or removes files it put there itself, so the rest of your stuff stays where it is. Cancelling or pulling the cable halfway through leaves no half-copied tracks behind.

And if you have a Rockbox player, turn on playback logging in its settings and the plays from the player get added to your play counts on the next sync. They don't get scrobbled yet though, because Rockbox saves the time in local time and I need to sort that out first.

### Plugins

Settings → Plugins is new. A plugin is a folder with a `plugin.json` and a JavaScript file that you drop into the plugins folder (there's a "Show plugins folder" button for that). Plugins can add things to the sidebar and to Settings, make playlists, play or queue tracks, follow what's playing, and match songs from other services to the ones in your library. They're off until you turn them on, they can only reach the websites their `plugin.json` lists, the logins they save go into your system keychain, and a browser sign-in can only come back on the ports the plugin lists.

One honest warning though: plugins run inside Honeycomb with the same access as the app itself, so those limits stop mistakes but they're not a sandbox (yet). Only turn on plugins you trust. How to write one is in `docs/plugins.md`.

### Security

The repo went public, so before that I went through the whole app looking for anything sketchy and fixed what turned up:

- Logins (Last.fm, ListenBrainz and anything a plugin saves) live in your system's keychain: Keychain on macOS, Credential Manager on Windows, Secret Service on Linux. They're never in Honeycomb's database.
- The app window has a Content Security Policy, so it only runs scripts that Honeycomb itself serves.
- Tag edits get written to a copy first, and the copy only replaces the original once the write is complete, so a crash in the middle of saving can't wreck a file.
- Sync checks every path in the device's manifest and never touches anything outside the folder you synced to, even through symlinks. Before this fix a messed-up manifest on the device could point it somewhere else, which was the biggest thing the review found.
- Every answer from the internet (GitHub, LRCLIB, Last.fm, ListenBrainz and plugins) has a size limit, so a broken server can't make Honeycomb eat all your memory.
- The update check only ever offers download links that point to GitHub.
- Updated rustls-webpki and webpki-roots for RUSTSEC-2026-0382 and RUSTSEC-2026-0381.
- Every release comes with a `SHA256SUMS.txt` so you can check your download (how to do it is in `SECURITY.md`), the GitHub Actions that build it are pinned to exact versions, and the dependencies get checked with cargo audit.
- `SECURITY.md` says how to report a problem privately and lists everything Honeycomb connects to, and the privacy policy on honeycomb.n1ji.dev now covers scrobbling and sync too.

Honestly, I didn't even know about most of these until I ran a proper security check on the whole app, and I was kinda bamboozled by how many things it found. Some were tiny, like a setting that could have been stricter, and some were actually pretty bad, like the sync one above. But that's exactly why you check BEFORE a lot of people are using it and not after, so I'm glad they all got found now, and I'll keep running that check before every release from here on, and adding to this check more and more until there's literally nothing left for it to find (spoiler: that day is NEVER coming).

### Smaller stuff

The layout doesn't fall apart anymore when you make the window small. Nothing gets cut off at the right edge, the track list columns squeeze down instead of falling off the side, and Settings and Sync at the bottom of the sidebar stay on one line and in line with each other. The Rating column was also juuust too narrow for five stars with some fonts, so the last star turned into "…". Fixed that too.

Releases have real version numbers now (0.1.1, 0.1.2 and so on) and this changelog. Every build can also run a self-test that scans, decodes and actually plays test files through your speakers (`scripts/smoke-test.sh`), so a broken build shouldn't make it out the door.

That's it for this one. If something breaks, open an issue on GitHub and I'll take a look!

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
