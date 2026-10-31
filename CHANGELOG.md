# Changelog

What changed in each Honeycomb release, newest first. New work goes under
**Unreleased**; `scripts/release.sh` turns that section into the next version
and uses it as the GitHub release notes.

## Unreleased

- Sync can convert now! In ⇅ Sync, pick what lossless files (FLAC, ALAC, WAV, AIFF) should become on the device: MP3, AAC, Ogg Vorbis, or 16-bit FLAC for players that should stay lossless but are short on space. MP3s, AACs and Oggs you already have just get copied, since converting them again would only make them worse. Tags and cover art come along, albums stay gapless, and the encoders are built in, so there's nothing extra to install. AAC uses Apple's own encoder, so for now it's Mac-only. The first sync takes a while, but later ones only convert what's new or changed, and switching to another format swaps the files out instead of leaving two of each.
- Updates install themselves now! When the bulb lights up, click it and hit Install, and Honeycomb downloads the new version, checks that it really came from me (every update is signed) and restarts on it. No more going to GitHub and installing over the top by hand. This only works from this version on, so 0.1.1 still has to be updated the old way one last time.
- You can finally see what quality you're listening at. There's a little badge next to the song title in the player bar (like `FLAC 24/96` or `MP3 320`, gold for lossless), and clicking it shows the whole signal path: the file, whether it gets resampled or downmixed, the volume, and what the output device runs at. If something changes the audio on the way out, the badge gets a small orange dot.
- Honeycomb now knows the bit depth of lossless files. There's a Bit Depth column and a smart playlist rule for it, so "everything 24-bit" is one rule away. Lossless files get read again once on the first scan after updating to fill it in.
- The Inbox is a real waiting room now. New music that shows up in your folders sits in the Inbox until you say so, and stays out of the library, the column browser, smart playlists and sync until then. You can still play it from the Inbox to have a listen first. Right-click → **Add to Library** to keep it (dropping it on a playlist does the same), or **Remove and ignore** to get rid of it: the files stay where they are, but Honeycomb stops picking them up on rescans. Changed your mind? Settings → Library → **Ignored files** lists everything you ignored, and you can bring a whole bunch back at once (Select all, or Shift-click a range).
- The Genre / Artist / Album browser up top got smarter. Shift-click picks a range, Ctrl-click (Cmd-click on Mac) adds or removes one, so you can look at five albums at once. And right-clicking there works like right-clicking songs, for everything in that row (or everything you picked): play it, put it on a playlist, rate it or edit the tags of a whole album in one go, and in the Inbox add or remove the lot.
- Honeycomb now keeps a listening history: which songs you play, when, and how each one ended (played to the end, skipped, went back, stopped), down to the second you skipped at. It's the groundwork for playlists that learn what you actually like. It never leaves your computer, and Settings → Listening history can turn it off or clear it.
- Moving or renaming your music files inside a library folder doesn't lose their play counts, ratings or playlist spots anymore. Honeycomb notices it's the same song and just follows it to the new place.

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
