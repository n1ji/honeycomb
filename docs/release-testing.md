# Testing a release

Every release is installed and run on real machines before it's published.
Building it isn't enough.

## 1. Cut the release

```bash
scripts/release.sh 0.1.1
```

This bumps the version everywhere, turns CHANGELOG.md's **Unreleased** section
into the version's notes, commits and tags. Nothing is pushed yet.

## 2. Self-test each build

On every machine you have (Mac on Apple Silicon, Mac on Intel if possible,
a Linux PC, a Windows PC):

```bash
scripts/smoke-test.sh path/to/Honeycomb.dmg   # or .app, .AppImage, .exe
```

The app starts on a throwaway library and checks that the window's UI
loads, that a scan finds and reads every test file (MP3, FLAC at 96 kHz, M4A,
Ogg Vorbis, WAV), that each one decodes and plays through the real sound
output, and that a track played to its end counts as played. It exits non-zero
if anything failed. With no argument the script tests the newest local build.

## 3. By hand, 10 minutes

The self-test can't hear anything, so these need a person:

- [ ] Install the way a user would (open the .dmg / install the .deb or .exe) and start it from the dock or menu.
- [ ] The library from the previous version is still there, with play counts, ratings and playlists.
- [ ] Play an album that's meant to be gapless (a live album). No clicks or gaps between tracks.
- [ ] Seek, skip, pause for over 30 s and resume.
- [ ] Unplug headphones while playing: playback pauses, and doesn't jump to the speakers.
- [ ] Media keys and the system's Now Playing controls work, and show the cover.
- [ ] Whatever this release changed (read its CHANGELOG section) works.
- [ ] If scrobbling changed: a song shows as "now playing" on Last.fm, then as a scrobble.
- [ ] If sync changed: sync to a USB stick (never to a real music collection), then play it in another device.

## 4. Publish

```bash
git push origin main v0.1.1
```

CI builds every platform into a draft pre-release with the CHANGELOG section
as its notes. Download one artifact per platform from the draft, run the
self-test on it once more, then publish the draft.
