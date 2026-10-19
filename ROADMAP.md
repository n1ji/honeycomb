# Honeycomb roadmap: November 2026 to April 2027

**Goal:** a native, fast, library-focused music player for macOS and Linux (Windows too) that people coming from other desktop players can switch to without losing anything they rely on.

**What makes it better:**
1. A real native app on Mac and Linux, not Wine.
2. Fast with huge libraries: measurably faster startup, search and scrolling at 100k tracks.
3. Painless migration: play counts, ratings and playlists come with you (done).

**The plan is built around two must-have features:** Last.fm scrobbling, and syncing music to players and flash drives.

This is a plan, not a promise. Each month has a "done when" line. Whatever slips moves into the next month, and the roadmap gets reviewed at the end of every month.

---

## Already done (October 2026)

- Library, column browser, tag editor, smart playlists, gapless playback, play counts.
- Importer for iTunes-format library XML and M3U files, tested on a real 4.5k-track export.
- Half-star ratings and a loved flag.
- Resizable panels and columns, themes, accent color, custom background and secondary colors.

## Month 1 (November): ship something people can install

- **Decide:** public or private repo (this decides where downloads live and how CI is paid for), whether to keep the name, and when to buy the Apple Developer account.
- Change the app identifier to a domain you own (e.g. `dev.n1ji.honeycomb`) and move the existing library database across automatically.
- Linux in CI: AppImage, .deb and .rpm, tested on a real distro. The webview on Linux (WebKitGTK) is the main scrolling risk.
- Built-in auto-updater (Tauri updater with a signing key).
- A download page on n1ji.dev.
- Performance baseline: generate a 100k-track test library and record startup time, memory, search, scrolling and idle CPU.
- Fix: removing the playing track stops playback (#4), and audio dies when headphones are unplugged (#5).

**Done when:** a stranger can download an alpha for Mac or Linux, install it, and receive the next update automatically.

## Month 2 (December): Last.fm

- Sign in through Last.fm's browser login, and keep the session in the system keychain.
- "Now playing" updates, and scrobbles following Last.fm's rules: tracks longer than 30 seconds, scrobbled after half the track or 4 minutes.
- An offline queue: scrobbles made without internet are sent later, in order, with no duplicates.
- Sync loves both ways with the loved flag.
- Optional: import play counts from Last.fm for tracks that have none locally.
- Media keys and the system Now Playing controls (macOS Now Playing, MPRIS on Linux), so headphone buttons and keyboard keys work. This also matters for scrobbling from the background.

**Done when:** a week of daily listening shows up correctly on Last.fm, including songs played offline.

**Note:** Last.fm needs an API key and secret built into the app. Every desktop scrobbler does this, and it's fine.

## Month 3 (January): sync to flash drives and USB players

- Sync profiles: pick a destination (any mounted drive or folder) and what goes on it (playlists, smart playlists, or the whole library).
- Copy only what changed, optionally remove files that are no longer selected, and check free space before starting.
- Folder layout options (Artist/Album/Track), and playlists written as M3U files with relative paths, so car stereos and players can read them.
- File names made safe for FAT32/exFAT, with very long paths shortened.
- A progress bar, and safe to cancel or unplug mid-sync.

**Done when:** syncing 5 playlists to a USB stick plays correctly in a car stereo, and re-syncing after a change copies only the difference.

## Month 4 (February): sync, part 2

- Convert while syncing (e.g. FLAC → AAC or MP3) for small or older players, with converted files cached so re-syncs are fast. Needs an encoder. ffmpeg is the likely option, but check its license before bundling it.
- Android phones over MTP. This is hard on Mac, so check what's realistic early. If it isn't, fall back to "sync to a folder" plus a phone sync app.
- Rockbox and similar players: read play counts back from the device. Stretch goal.

**Done when:** a lossless library syncs to a small player as lossy files, and a phone works or the documented fallback does.

## Month 5 (March): the rest of what blocks switching

- Watch library folders for new and changed files instead of manual rescans.
- An album art grid view.
- ReplayGain (track and album volume leveling).
- A basic equalizer.
- Lyrics: show embedded lyrics. Online lookup comes later.
- Fix: changing the queue in the last 2 seconds of a track (#3), and unreadable files being re-read on every rescan (#6). Find out why 64 files fail to read (#2).

**Done when:** Honeycomb works as someone's only player for two weeks without them going back.

## Month 6 (April): polish toward 1.0

- Performance pass against the Month 1 baseline: load the library lazily so startup stays fast at 100k tracks, a faster search index, and cached artwork thumbnails.
- Customization: choose, order and resize track list columns, and pick the column browser's fields.
- A signed and notarized Mac build (needs the Apple account), and Linux packages on Flathub and/or the AUR.
- Windows builds kept working, even though it isn't the main target.
- A user guide covering migration, sync and Last.fm setup.

**Done when:** a 1.0 beta is announced to Mac and Linux music library users.

---

## Later (not in these 6 months)

Online tag and artwork lookup, automatic file renaming and organizing, theme files and custom CSS, saved layout profiles, podcasts, internet radio, CD ripping, plugins, mobile apps.

## Ground rules

- **Original work:** Honeycomb may share familiar ideas with other players, but never their code, assets, skins or branding.
- **Never test tag editing or sync deletes on a real music collection.** Use generated test libraries.
