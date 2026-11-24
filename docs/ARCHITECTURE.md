# Multi-device architecture

Status: **proposal**, not built. This covers how Honeycomb on the desktop, Honeycomb on iOS and OpenSubsonic servers work together. It describes the code on `dev` as of 24 November 2026 and lists the known places where the plan and the code disagree.

## Problem

Honeycomb is a desktop player with the library on one computer. People who also listen on a phone, or keep their music on an OpenSubsonic server (Navidrome and others), have to use a different app there. Their playlists, ratings and play counts then live in two places and drift apart. Honeycomb already works as a client of a server (see What exists today). This document covers the rest: the desktop serving phones, a phone app, and keeping one set of playlists, ratings and play counts across all of them.

## Goals and non-goals

**Goals for v1**
- One library state (playlists, stars, ratings, play counts) per setup, with every device reading and writing the same values.
- Work with standard OpenSubsonic servers (Navidrome 0.64.2 is the reference), with clearly stated fallbacks where a server lacks the Honeycomb extension.
- The desktop can serve paired phones on the home network.
- Phones play offline, and their plays and ratings reach the state owner when they reconnect.
- Never delete music without asking (see Connections in Settings → Library).

**Non-goals for v1**
- Third-party clients against Honeycomb's own server. It only accepts API keys issued by `pair`, behind a pinned self-signed certificate, so ordinary Subsonic apps can't connect, and that is accepted.
- The desktop acting as a faster LAN copy of a server for phones. Phones in a server setup talk to the server.
- A headless `honeycomb-server` (Docker). The server crate keeps it possible later (no Tauri dependency), but no phase builds it.
- Two-way merging, tag editing for remote tracks, Android, a Navidrome plugin.

## Terms

- **Computer** or **desktop:** the machine running the Honeycomb desktop app.
- **Server:** an OpenSubsonic server such as Navidrome. The desktop's own built-in server is always called **the desktop's server**.
- **Song:** one track in the library. Code and endpoints say `track` and `song`; they mean the same thing.
- **LAN:** the home network (same Wi-Fi or wired).
- **Kaseki:** Honeycomb's audio engine (decoding, resampling, gapless playback).

## Principles

1. **Every link between devices speaks OpenSubsonic.** There's no custom sync protocol. Anything the standard can't do goes into one Honeycomb extension, advertised through `getOpenSubsonicExtensions`. Any OpenSubsonic server can be a source. Where a server lacks something, Honeycomb falls back: legacy salted-token auth (C12), walking albums when an empty search returns nothing (C13), whole stars instead of half stars (C3), and standard `scrobble` instead of play totals (C5).
2. **Roles, not devices.** Each app keeps a list of `Source`s, and sync, dedupe and fetch order only look at what a source reports:

   | Capability | Meaning |
   | --- | --- |
   | `fullLibrary` | holds the whole library, not just a cache |
   | `alwaysOn` | reachable whenever clients run (a server); a laptop is not |
   | `ownsState` | its playlists, stars, ratings and play counts are the true ones |
   | `local` | files on this device; no network involved |

3. **One state owner.** Each setup has exactly one source that owns playlists, stars, ratings and play counts. Other devices send changes to it and read from it. v1 never merges two-way. The one exception is play counts from Honeycomb devices (C5): each device only raises its own running totals, so adding them up can never conflict.

## Setups

| Setup | Library on | State owner | Desktop role | Phone role |
| --- | --- | --- | --- | --- |
| Phone only | phone (local files) | phone | none | local player |
| Desktop only | computer | desktop | library (today's app) | none |
| Desktop + phone | computer | desktop | library + OpenSubsonic server | client of the desktop |
| Server + desktop and/or phone | server | server | client of the server | client of the server |

Onboarding on both apps asks **"Where's your music?"** (on this device / on my computer / on a server) instead of assuming a setup.

### Modes

The answer becomes the app's **mode**, which can be changed later in Settings. A mode is a preset on top of the source list: it decides which sources are allowed and which UI is shown. Sync and dedupe code never look at the mode.

#### Desktop: 3 modes

| Mode | What it does | UI |
| --- | --- | --- |
| **1. Standalone** | your library, as today | no server or phone rows, no remote or pinned states |
| **2. With phones** | your library, plus the OpenSubsonic server for paired phones. State owner | phone rows (Sync, Disconnect) in Settings → Library, Settings → Phone |
| **3. With server** | client of a server (metadata mirror, streaming, pinning). Doesn't serve phones; they use the server directly | server row, track states, tag editing and Inbox off for `remote` tracks (C11) |

#### Phone: 3 modes

| Mode | Sources | State owner |
| --- | --- | --- |
| **1. Standalone** | its own folders (and optionally the system Music library) | phone |
| **2. With computer** | the desktop | desktop |
| **3. With server** | a server only, no desktop | server |

#### Switching modes

Switching goes through the upgrade paths below. Going down a mode (2 → 1, 3 → 1) runs **Disconnect** on the sources that go away, with its keep-or-delete choice. Moving the desktop from mode 2 to mode 3 (adding a server while phones are paired) is "Anything → + server" below; each paired phone then adds the server itself, and its pairing with the computer is disconnected with the usual keep-or-delete choice.

## What exists today

| Piece | Where | Notes |
| --- | --- | --- |
| Library DB | `db.rs`, `models.rs` | SQLite, flat `tracks` table (no artist or album tables), `i64` ids, half-star ratings 0–10, `loved` flag, smart playlists evaluated in SQL (`smart.rs`) |
| Scanner | `scanner.rs` | `scan()` has no Tauri dependency. Only `spawn_scan` uses `AppHandle` (for progress events) |
| Tags | `meta.rs` | lofty. MusicBrainz ids are **not read** today |
| Kaseki | `kaseki.rs` | has no Tauri dependency. `convert.rs` reuses its decoder and resampler |
| Converter | `convert.rs` | MP3, AAC (macOS only), Vorbis, FLAC 16. Used by device sync |
| Device sync | `sync.rs` | copies or converts to players and folders; also downloads server songs straight into a sync folder (through `mirror`) |
| OpenSubsonic client | `subsonic.rs`, `net.rs` | blocking `ureq` + rustls. `search3` paging with the `getAlbumList2` + `getAlbum` fallback (C13), playlists, `star`, `setRating`, `scrobble`, `stream`, `download`, cover art. API-key auth when offered, else salted token (C12); `.p12` client certificates |
| Server mirror | `mirror.rs` | tables `sources`, `remote_tracks` (`remote` / `downloading` / `pinned` / `local-only`), `remote_pins` (album pins take in new songs), `remote_playlists`, `remote_outbox` (changes waiting for the server). Mirrored rows live in `tracks` with path `subsonic://<source>/<id>`. Pull, pin, background downloads, offline copies (optionally transcoded), now-playing, Disconnect with keep or delete |
| Streaming | `stream.rs` | stream cache; Kaseki opens server tracks through `mirror::resolve` |
| Secrets | `secrets.rs` | system keychain (server passwords and certificates, plugin secrets) |
| App glue | `lib.rs`, `player.rs`, `media.rs`, `plugins.rs`, `version.rs`, `servers.rs` (server commands, 15-minute pull, outbox worker), `watch.rs` (folder watching), `smoke.rs` (self-test) | Tauri-dependent |
| Phone sync server | `phone.rs`, `docs/phone-sync.md`, **only on branch `wip/phone-sync`** (one commit on top of the 30 October `dev`) | custom JSON-over-HTTPS protocol (details below). Not on `dev` |

Everything is one crate (`src-tauri`), with no Cargo workspace. The settings list under Settings → Library already shows server rows with Disconnect (`src/ui/servers.ts`, `src/ui/settings.ts`).

### The current phone protocol, and what to keep from it

`phone.rs` is a hyper 1 + rustls 0.23 server on its own tokio runtime:

- a self-signed certificate (`phone-cert.der`) that the phone pins by SHA-256;
- Bonjour `_honeycomb._tcp` with `libraryId` (a random id for the library, so a phone can tell two computers apart) in TXT;
- pairing with a 6-digit code (single use, 5 minutes, 5 tries), shown as a QR code (`honeycomb://pair?h=…&p=…&c=…&f=…`) or typed after checking a short fingerprint;
- per-device tokens stored as SHA-256 hashes in `devices` (v6 on that branch; on `dev`, v6 is ReplayGain, so `devices` needs the next free schema version), and revocable;
- `/v1/library`, a whole-library snapshot; `/v1/tracks/{id}/file`, with Range support; `/v1/tracks/{id}/art`; `/v1/events` for plays (de-duplicated by phone-made UUID in `phone_plays`) and ratings.

#### Recommendation: replace the API surface, keep the plumbing

Of the three options:

- **Keep it alongside OpenSubsonic: no.** Two protocols means two things to secure, test and document. Nothing depends on the custom one yet: it's unreleased, and there's no phone app.
- **Replace all of it: no.** Most of it isn't protocol-specific, and the branch has tests for it.
- **Reuse (recommended):**

  | Keep as is | Becomes | Dropped |
  | --- | --- | --- |
  | TLS identity and pinning, Bonjour advert, server lifecycle (`Server`, `Advert`, `Running`, on/off setting), Range parsing, the pinned-client test harness, Settings → Phone UI | the code/QR pairing becomes a Honeycomb extension endpoint that returns an **API key** for `apiKeyAuthentication`. The `devices` table holds the hashed keys (same table, same revocation). `phone_plays` is replaced by per-device play totals (see conflict C5) | `/v1/library`, `/v1/events`, `/v1/tracks/*`; these are replaced by standard endpoints and `playTotals` |

#### Getting it onto `dev`

Port, don't merge. The branch is one commit on a `dev` from 30 October, and `dev` has moved a long way since. Phase 2 starts by copying the pieces in the "Keep as is" column into the new server crate (with the hyper/tokio/rustls/rcgen/mdns-sd dependencies they need) and giving `devices` its new schema version. The branch stays as it is for reference, and nothing on it is deleted without the owner's sign-off.

## Alternatives considered

### Protocol: OpenSubsonic only, vs a custom sync protocol

- *Chosen:* OpenSubsonic plus one extension. Honeycomb works with servers people already run (Navidrome is the owner's), the server crate can be tested with existing clients, and the desktop's client code is shared with the phone.
- *Rejected: a custom protocol with a changes feed* (what `phone.rs` does today). It would allow sending only what changed instead of re-listing the library, and exact play events. But it only works between Honeycomb apps, and it is a second thing to secure and document. Delta sync can be added later as an extension if a full listing turns out to be too slow (see phase 4).

### State: one owner, vs merging changes from every device

- *Chosen:* one state owner per setup. Other devices queue changes (`remote_outbox`) and send them to the owner, which keeps the last write it receives. Play counts are the one thing every device adds to, and running totals per device (C5) make that safe without merging.
- *Rejected: two-way merge* (per-field last-writer-wins with timestamps, or an operation log). It would let two offline devices edit the same playlist and both keep their edits. But it needs clocks or version vectors on every device, a merge rule per kind of data, and OpenSubsonic servers can't take part in it. No real conflicts have been seen yet.

## Desktop

### Server crate

New crate `crates/honeycomb-server`: an OpenSubsonic server library. It takes a library handle (DB path plus data dir) and has no Tauri dependency. The desktop starts it in-process. Because it has no Tauri dependency, a headless build stays possible later (see non-goals).

- **HTTP stack:** axum on the hyper 1 + rustls 0.23 + tokio stack `phone.rs` already uses. Today's handlers are a hand-written `match` on the path. With the 27 endpoints below, axum's routing and extractors replace that `match`.
- **Network:** LAN only. It binds every interface, as `phone.rs` does today, but answers only private and link-local addresses, judged by the connection's peer address (never a forwarded header). `pair` and failed logins are rate-limited per address.
- **Shared database:** the desktop and its server use the same `library.db` in WAL mode, so reads never wait. Scans commit in small transactions, so phones' writes (`star`, `setRating`, `playTotals`) aren't held up for long.
- **Endpoints for v1:** only what Honeycomb's own clients need. Third-party clients are a non-goal.

| Group | Endpoints |
| --- | --- |
| System | `ping`, `getLicense`, `getOpenSubsonicExtensions` |
| Browsing | `getMusicFolders`, `getArtists`, `getArtist`, `getAlbum`, `getSong`, `getAlbumList2`, `search3` (empty query = everything, for offline sync) |
| Playlists | `getPlaylists`, `getPlaylist`, `createPlaylist`, `updatePlaylist`, `deletePlaylist` |
| Media | `stream`, `download`, `getCoverArt`, `getLyricsBySongId` (`songLyrics`) |
| State | `star`, `unstar`, `setRating`, `scrobble`, `getStarred2` |
| Honeycomb extension | `pair` (no auth), `playTotals`, and the `rating10` field. `upload` comes in phase 5 |

### Core crate (the restructuring the server needs)

The server needs the DB, models, smart playlists, tags and the scanner, but not the app. Two crates come out of `src-tauri`. Every module in `src-tauri/src` goes somewhere:

| Module | Goes to | What has to change |
| --- | --- | --- |
| `kaseki`, `convert` | `crates/kaseki` | the test helper `write_wav` (used by `player` and `sync` tests) goes behind a `test-util` feature that `src-tauri` and core turn on as a dev-dependency. Kaseki's streaming test uses `stream`, so it moves to core's tests |
| `models`, `db`, `smart`, `meta`, `matching`, `lyrics`, `net`, `secrets`, `importer` | `crates/honeycomb-core` | nothing: none of them use Tauri or the app modules |
| `subsonic`, `stream`, `mirror` | `crates/honeycomb-core` | nothing. Together they are **the client and sync planner** that the iOS section and phase 4 refer to |
| `scanner` | `crates/honeycomb-core` | `spawn_scan` (the only part that uses `AppHandle`) moves to the app. `scan()` keeps using `sync::MANIFEST` and `sync::synced_files`, which works because `sync` moves too |
| `sync` | `crates/honeycomb-core` | it calls `player::listen_threshold`, a one-line pure function that moves into core with it. Core depends on the `kaseki` crate for conversion |
| `resolve_playlist` (in `lib.rs`) | `crates/honeycomb-core` | returns `anyhow::Result<Vec<i64>>` instead of the Tauri command type `CmdResult`. The app's commands wrap it |
| `lib`, `main`, `player`, `media`, `plugins`, `version`, `servers`, `watch`, `smoke` | stay in `src-tauri` | depend on both crates. These are the modules that use Tauri |

Apart from the four changes in the table, this is moving files and making items `pub`. Phase 1 compiling and passing its tests is the check.

### Desktop as a client of a server

**Built** (`subsonic.rs`, `mirror.rs`, `servers.rs`; see What exists today). This section records the design; phase 4 now only covers what is missing.

When a server source is added, the desktop keeps a **metadata mirror** in its normal `library.db`:

- it pages through `search3` with an empty query, then fetches playlists and starred items;
- it stores **no audio** by default.

The column browser, search and sort read local SQLite, so they should scale like a local library. 100k tracks is the target; it hasn't been measured with a server yet. Each track gets a state:

| State | Meaning |
| --- | --- |
| `remote` | metadata only; plays by streaming |
| `downloading` | in progress |
| `pinned` | downloaded on request and kept |
| `local-only` | an offline copy of a song the server has since dropped, while the source is still connected |

Without a server source, every track is local, and the remote and pinned UI is hidden.

### Connections in Settings → Library

Servers and paired phones are listed in the same place as music folders: the list under **Settings → Library** (`src/ui/settings.ts`, `renderFolders`). A folder row keeps its **Remove** button. Server and phone rows get **Sync** and **Disconnect** instead:

| Row | Shows | Sync | Disconnect |
| --- | --- | --- | --- |
| Folder | path | (none; "Rescan now" covers it) | **Remove**, as today |
| Server | name, address, track count, last synced, state owner or not | refreshes the metadata mirror and sends any changes still waiting | asks **Keep the music on this computer** or **Delete it**, then removes the source. Keep moves the offline copies into `Music/Honeycomb/<server>` and turns them into ordinary file tracks. Delete removes them and the mirrored tracks |
| Phone | device name, last seen, last synced, and how to sync now (C15) | none (C15) | asks **Leave the music on the phone** or **Delete it from the phone**, then revokes the device's API key (today's "Remove" in Settings → Phone). The row then shows "Removing… · Undo" for a week, and the phone carries out the choice the next time it connects (C17) |

**Disconnecting never deletes music without asking.** Every Disconnect asks whether to keep the music on that device or delete it, and **Keep** is the default button. The same choice appears when disconnecting from the other side: the phone app's own "Disconnect from this computer" or "Remove server" asks too. Keeping always leaves a working library: the kept files become local tracks with their tags, art and the stats they had when last synced. **Delete** only ever removes files that came from that connection: music in the phone's own picked folders, or in the computer's library folders, is never touched.

Settings → Phone keeps only the on/off switch and **Pair a phone…**. Its device list moves into this one, next to the folders. A row gets a small status line when its server can't be reached or its phone hasn't been seen for 30 days.

### Always ready for phones

In desktop mode 2 (With phones) the desktop needs no buttons to connect: the server starts with the app, Bonjour announces it, and phones connect on their own whenever their app opens or background refresh runs. Only pairing takes a click, once. Two options cover the time Honeycomb isn't open:

- **Keep running in the menu bar when the window is closed** (system tray on Windows and Linux). The server and Bonjour keep going; playback stops as it does today when the window closes. This uses Tauri's built-in tray icon.
- **Start at login**, hidden in the menu bar (`tauri-plugin-autostart`).

Both are off by default. After the first phone is paired, Settings → Phone suggests turning them on: "So your phone can sync even when Honeycomb's window is closed." iOS can't wake an app when the phone joins a Wi-Fi network, so the phone side stays "when the app opens, and when iOS allows background refresh".

## iOS

### Repository and stack

- **Where:** the iOS app is developed in a separate, private repository. It uses the core crates (`honeycomb-core`, `kaseki`, the client and sync planner) from this repo. No iOS app code goes into this repo.
- **Stack:** native SwiftUI, plus the shared Rust core (`honeycomb-core` for the library DB, `kaseki` for playback).

### Standalone mode

The user picks folders with the document picker. The app stores security-scoped bookmarks and plays files in place. It rescans on open and with a button, handles iCloud placeholders (`NSMetadataUbiquitousItemDownloadingStatus`) and stale bookmarks (asks the user to pick the folder again). Optional: non-DRM songs from the system Music library.

### Remote sources

A server or the desktop:
  - **Offline:** `download` through a background `URLSession`. Synced files are excluded from iCloud backup (`isExcludedFromBackup`).
  - **Info.plist:** `NSLocalNetworkUsageDescription`, `NSBonjourServices = [_honeycomb._tcp]`.
  - **TLS:** the desktop's certificate is self-signed and pinned in the URLSession delegate. ATS still applies its protocol rules (TLS 1.2+, forward secrecy, SHA-256 signatures). An rcgen ECDSA P-256 certificate over rustls should meet them; this is checked on a real iPhone in phase 5.
- **The phone never runs an OpenSubsonic server.** Over a cable it accepts one USB connection from the computer and uses it only to send its own requests (C9).

### Where the networking lives

Approved. The shared Rust core owns the networking. Swift only handles the two things iOS reserves for its own APIs.

| Request | Decides | Sends |
| --- | --- | --- |
| Browsing, library listing, playlists, ratings, play totals, pairing | Rust | **Rust** |
| Streaming and downloads while the app is open (Wi-Fi or cable) | Rust | **Rust** |
| Downloads that must continue after the user leaves the app | Rust | **Swift**: background `URLSession`, from a job list the core hands over (URL, API key, expected size, certificate fingerprint). Swift reports each finished file back to the core |
| Finding computers on the network | n/a | **Swift**: `NWBrowser` (no multicast entitlement needed, unlike a Rust mDNS library), which passes the addresses to the core |

- **The core** contains the OpenSubsonic client, certificate pinning (the rustls verifier from `phone.rs`'s test harness), the sync planner (compares a source's listing with the local DB and decides what to download, delete or push), dedupe, the local DB, Kaseki and the cable transport. Over a cable, the core speaks OpenSubsonic directly over the usbmuxd stream, so nothing on the phone has to forward traffic.
- **Swift** also watches network conditions (`NWPathMonitor`: Low Data Mode, cellular allowed, VPN) and passes them to the core, since Rust's networking doesn't see those system settings.
- **Pinning exists twice:** in the core for its requests, and in the `URLSession` delegate for background downloads. Both compare one SHA-256 string against the same stored value.
- **The desktop** already uses this client and planner (`subsonic`, `mirror`; they move into `honeycomb-core` in phase 1). Its discovery uses `mdns-sd`, which comes over with the phone-server code in phase 2.

## Multi-source rules

1. **Dedupe:** by MusicBrainz recording id when both sides have one. Otherwise, the existing loose match (`matching::LibraryIndex`: artist, title, duration). See C4 on audio hashes.
2. **Fetch priority:** a local file first, then the source the song came from. (A faster LAN copy from the desktop is a non-goal for v1.)
3. **State:** changes go only to the state owner. Others read from it.
4. **Changes made offline** wait in an outbox (`remote_outbox`, already built) until the owner is reachable:
   - Only the newest change per item is kept; a newer star, rating or playlist edit replaces the waiting one.
   - Stars and ratings: the owner keeps whatever arrives last.
   - Playlists are always sent as the whole list, never as index-based removals. A playlist changed on the owner meanwhile is overwritten by the queued version; that is accepted for v1.
   - Changes to songs the source no longer has are dropped.

## Auth

- **Our server:** `apiKeyAuthentication` only. A key is issued per device by pairing, stored hashed, and revocable in Settings (the OpenSubsonic `apiKeyAuthentication` extension asks servers to let users see and revoke keys).
- **Talking to other servers:** `apiKeyAuthentication` when the server advertises it. Otherwise legacy salted-token auth, which means storing the user's password in the system keychain (`secrets.rs`).

## Honeycomb extension

Advertised as `honeycomb` in `getOpenSubsonicExtensions`. v1 covers:

- **Pair** (no auth): code → API key, plus `libraryId`.
- **Upload** (auth, phase 5): a file plus its play stats, for migrations. The server places it under a folder it chooses and scans it. File names are sanitised (no path separators or `..`); the full contract (size limits, resuming) is written when phase 5 starts.
- **Play totals** (`playTotals`, auth): the device sends, for each song it played, its **running total since pairing** plus its latest play time: `{trackId, count, lastPlayed}`. The server stores one row per device and song, and replaces it only when `count` goes up. A song's play count = the owner's own plays + the sum of every device's total; `last_played` = the latest of all. Sending the same totals twice changes nothing, so retries are always safe. A device whose total for a song goes *down* (its database was reset or restored) is treated as re-paired: it gets a new device row, and the old row's plays stay counted. Totals are keyed on song ids, which survive moves and renames (C2).
- **Half-star ratings:** an optional `rating10` field (0–10) on `setRating` and on song results, so Honeycomb clients don't lose half-stars. It's a field, not an endpoint.
- **Version:** advertised as version 1. A client ignores extension features it doesn't know.

## Upgrade paths

- **Phone only → + desktop:**
  1. The phone pairs.
  2. It pushes its files and playlists through `upload`.
  3. Dedupe skips files the desktop already has.
  4. The desktop becomes the state owner, and the phone's plays and ratings go with the upload.
- **Anything → + server:**
  1. The user copies the music into the server's folder; standard Subsonic has no upload.
  2. Honeycomb pushes playlists, stars and ratings (`createPlaylist`, `updatePlaylist`, `star`, `setRating`).
  3. The state owner switches to the server.
  4. Dedupe links the server's copies to the local files.
  5. The user decides whether to keep or free the local files.

## Cable

Cable is a transport, not a protocol: OpenSubsonic over a usbmuxd TCP tunnel. The computer needs usbmuxd: built into macOS, Apple Devices on Windows, `usbmuxd` (libimobiledevice) on Linux. Android cable support is future work.

## Decision log

Every conflict between the plan and the code, with what was decided. All are approved by the owner. Longer behaviour is under Decision details below.

| # | Conflict | Decision | Phase |
| --- | --- | --- | --- |
| C1 | `tracks` is flat, but `getArtists` and `getAlbum` need stable artist and album ids. | Derive them from a hash of the normalized album artist + album (album id) and the artist name (artist id). Clients keep long-lived data per song id. [Details](#c1-artist-and-album-ids) | 2 |
| C2 | Moving or renaming a file used to give it a new id and lose its stats. | **Built:** the scanner keeps a moved file's row (see [details](#c2-moved-files-keep-their-id)), so song ids survive moves. | done |
| C3 | Ratings are half-stars; Subsonic `userRating` is whole stars. | The owner stores 0–10. `userRating` = `ceil(rating10 / 2)`; a standard `setRating` of n is stored as 2n. Whole-star picker for server songs (**built**). [Details](#c3-half-star-ratings) | 2 (server) |
| C4 | "Else audio hash" can't work across sources: Subsonic exposes no content hash. | Dedupe by MusicBrainz recording id, then the loose match (`matching::LibraryIndex`). Hashing only if it turns out to be needed. | 4 |
| C5 | `scrobble` isn't idempotent, so a retried batch double-counts. | Honeycomb servers: per-device running totals through `playTotals`. Other servers: standard `scrobble`, accepting rare duplicates. [Details](#c5-play-counts) | 2, 5 |
| C6 | Putting the API key in the QR code would let anyone who photographs the screen keep a permanent key. | The QR code carries the single-use code (5 minutes); it's exchanged for a key over pinned TLS. | 3 |
| C7 | Smart playlists are read-only in Subsonic terms. | Resolved on `getPlaylist`; `updatePlaylist` refuses them with a clear error. | 2 |
| C8 | With a server as owner, the server and Honeycomb's scrobbling plugin may both scrobble to Last.fm. | Desktop mode 3 warns when both are on, and the user picks one. | 4 |
| C9 | usbmuxd only lets the computer connect *to* the phone. | The phone accepts that one connection and uses it only for its own requests; it serves nothing. TLS stays end to end. | 6 |
| C10 | Background `URLSession` runs out of process, so it can't use the cable. | Cable sync is foreground only; Wi-Fi keeps background downloads. [Details](#c10-cable-sync-in-the-foreground) | 6 |
| C11 | Tag editing, the Inbox (new files waiting to be added), rescans and file-based lyrics assume local files. | `remote` tracks disable those actions in v1 (Subsonic can't write tags). | done |
| C12 | Does Navidrome support `apiKeyAuthentication`? | **Verified 2026-11-09:** Navidrome 0.64.2 doesn't advertise it. Honeycomb uses the legacy salted token, with the password in the keychain, and switches to API keys on servers that offer them. **Built.** | done |
| C13 | Empty `search3` queries aren't guaranteed by the spec. | Fall back to walking `getAlbumList2` + `getAlbum`. **Built.** | done |
| C14 | Headless server and the GUI-only pieces. | Out of v1 (see non-goals). If it's built later, pairing prints the code and QR code to the console. | — |
| C15 | The desktop can't start a phone sync; the phone is only a client. | No Sync button on phone rows; the row says how to sync now. [Details](#c15-phone-rows) | 3 |
| C16 | Disconnecting a server: plays and ratings made while it owned the state live on the server. | A last pull before Disconnect, when the server can be reached. [Details](#c16-last-pull-before-disconnecting-a-server) | 4 |
| C17 | The desktop can't tell a removed phone what to do, and a revoked key only gives an auth error. | Disconnect suspends the key for a week and stores keep-or-delete for the phone. [Details](#c17-a-weeks-grace-for-removed-phones) | 3 |

## Decision details

### C1: artist and album ids

Compute ids on demand at first, and cache them in a table only if that's too slow at 100k tracks. Renaming an album or artist changes its id. Song ids don't change on tag edits, because they update the row in place (`upsert_track`, `ON CONFLICT(path)`), so nothing is downloaded again. Two releases with the same artist and album name share an id unless they differ by year, which is added to the hash when present. Songs with no album tag are grouped per album artist.

**Rule for clients:** keep long-lived data (offline pins, later album stars) per **song id**. A pinned album also remembers its album id. When that id disappears, the client looks up the album its songs moved to and moves the pin there, so songs added to the album later are still kept.

### C2: moved files keep their id

At the end of a scan, before removing vanished paths, the scanner pairs each vanished track with a newly found one that has the same title, artist, album and audio format (bitrate and sample rate) and a duration within half a second. Only one-to-one pairs count. On a match, the old row takes the new path and tags, so the id, play count, rating, loved flag and playlist entries are kept (`scanner.rs`, `find_moves`). Files that change format or tags while moving still come back as new songs.

### C3: half-star ratings

**Reading:** `rating10` returns the stored value exactly (extension). `userRating` = `ceil(rating10 / 2)`, so a half star shows as 1 and never as unrated, and 3.5 shows as 4. **Writing:** `rating10` is stored as is; a standard `setRating` of n is stored as n × 2. Clients send a rating only when the user changes it, never a value they just read, so rounding never writes itself back. **Owner without the extension** (a server that only stores whole stars): the desktop's star picker offers whole stars for that server's songs (built). Moving a library there rounds half-stars up, and the migration dialog says so first.

### C5: play counts

**Honeycomb servers:** devices count their own plays and send running totals per song through `playTotals` (see Honeycomb extension). They send when the app opens, and in the background once or twice a day when the OS allows it (iOS `BGAppRefreshTask`). Background refresh only pushes totals and waiting changes; a full library listing happens when the app is open. This replaces both `playId` and the `phone_plays` table. The library only stores `play_count` and `last_played`, so totals lose nothing the app uses. Totals count plays **since pairing**: a re-paired device starts from 0 under a new device row, and the old row's total stays counted. Removing a device keeps its counted plays. The existing `listens` table (listening history) stays as it is; it's a local log, not something that syncs.

**Last.fm:** a device that scrobbles does it itself, straight to the service, because totals can't be split back into timestamped plays.

**Other servers:** standard `scrobble`. Plays are deleted locally only after a 200, and rare duplicates are accepted.

A Navidrome plugin that accepts play totals isn't possible on 0.64.2 (plugins can't receive requests). It could be revisited if Navidrome adds plugin HTTP endpoints ([navidrome/navidrome#5045](https://github.com/navidrome/navidrome/issues/5045)).

### C10: cable sync in the foreground

The Rust core speaks OpenSubsonic over the tunnel directly (see "Where the networking lives"). When a cable sync starts, the phone explains that the app has to stay open until it's done, and offers to keep the screen on until then. When the computer is also reachable over Wi-Fi, it offers to continue over Wi-Fi instead, handing the rest to background downloads. If the user leaves anyway, the app asks iOS for a little extra time to finish the current file, then pauses. Finished files are kept, so the next sync resumes.

### C15: phone rows

The row shows when the phone last synced, and a short line saying how to sync now: open Honeycomb on the phone, on the same Wi-Fi or plugged in. The phone also syncs by itself once or twice a day when iOS lets it; the computer can't start a sync on its own. When a phone hasn't synced for 30 days, the hint is highlighted.

### C16: last pull before disconnecting a server

When the server can be reached, Disconnect first offers to bring things over:
- **Ratings, play counts, loved songs and playlists** (on by default; metadata only): copied to the desktop for every song it keeps (offline copies, and songs that dedupe matched to local files). Playlists become manual playlists, and play counts take the larger of the two numbers.
- **The songs that aren't on this computer yet** (off by default; shows the total size). Disconnect waits for the download, can be cancelled, and resumes on the next try.

After the pull, the desktop becomes the state owner, then the usual keep-or-delete choice runs. When the server can't be reached, the dialog says the stats can't be brought over right now, and offers to try again later (the default) or disconnect anyway.

### C17: a week's grace for removed phones

Disconnect **suspends** the device's key for 7 days and stores the user's choice (`keep` or `delete`). The row shows that removal is pending, with Undo.
- **Undo within the week** restores the device as it was: no re-pairing, and its play totals carry on.
- **The phone connects within the week:** instead of library access, a request with the suspended key gets the stored choice. Keep: the music stays as local tracks. Delete: the phone asks its user to confirm first, since it's the device that loses the files. Then the device row is deleted.
- **No contact for 7 days:** the key, the choice and the row are deleted. A phone that later gets a plain auth error asks its user, Keep by default, and never deletes on its own.
- **Never** treat "can't reach the computer" as removal; that happens every time the phone leaves home.

## Phased plan

Each phase ends in something testable. Reference server for tests: Navidrome 0.64.2.

1. **Workspace split.** Create a Cargo workspace with `crates/honeycomb-core` and `crates/kaseki` (see the module table under Core crate), and make `src-tauri` depend on them. No behaviour changes.
   *Done when:* the full `cargo test` suite passes with the same number of tests as before the split, and a manual check (scan a folder, play a track, edit a tag, sync from a server) behaves as before.
2. **Server crate.** Starts by porting the reusable phone-server pieces from `wip/phone-sync` (see [Getting it onto `dev`](#getting-it-onto-dev)). Then `crates/honeycomb-server` with the v1 endpoints over a `honeycomb-core` library, API-key auth, `pair`, `playTotals` and the `rating10` field. Covers C1, C3 (server side), C5 (`playTotals`) and C7.
   *Done when:* tests drive every endpoint with a pinned client (the harness from `phone.rs`), including: a bad or missing API key; `updatePlaylist` on a smart playlist is refused; `playTotals` sent twice, or with a lower count, changes nothing; `rating10` 7 reads back as `userRating` 4; requests from a public address are refused.
3. **Desktop server and pairing.** The desktop starts the server: same certificate, Bonjour advert, Settings → Phone and `devices` table. Pairing returns an API key (C6). Paired phones appear in the Settings → Library list with Disconnect, including the week's grace (C17) and the sync hint (C15). It adds menu-bar mode and start at login. The old `/v1` endpoints are removed, with the owner's sign-off. `docs/phone-sync.md` (on `wip/phone-sync`) is rewritten as the extension spec.
   *Done when:* QR and typed-code pairing both work from a test client; an expired, reused or fifth wrong code is refused; a disconnected device's key gets the keep-or-delete answer during the week and an auth error after it; Undo restores the device; the app keeps serving with its window closed in menu-bar mode.
4. **Desktop as a client of a server: the rest.** Already built: sources, the metadata mirror, track states, streaming through Kaseki, pinning, the outbox for playlists, stars and ratings, and server rows with Sync and Disconnect. Left: the MBID column (read by `meta.rs`; a new schema version that rescans files once) and dedupe by MBID (C4), the state-owner switch (pushing playlists, stars and ratings when a server becomes the owner), the double-scrobble warning (C8), and the last pull on Disconnect (C16).
   *Done when:* those pieces work against a real Navidrome library of 10k+ tracks, and a full listing of that library is timed. If it's too slow, a delta-sync extension is designed before phase 5.
5. **iOS** (in its own private repository). Each step ends on a real iPhone:
   1. Kaseki plays a local file through the iOS audio stack (a time-boxed spike of about 2–3 days; if it fails, playback falls back to the system player and the rest of the plan stands).
   2. Standalone mode: picked folders play offline and survive an app restart.
   3. The desktop and server sources: pairing, browsing, background downloads finishing with the app closed, and plays reaching the owner.
   4. `upload`-based migration (phone only → + desktop), with the server side of `upload` built here.
6. **Cable (iPhone).** OpenSubsonic over usbmuxd (C9, C10) on top of phase 5.
   *Done when:* a full sync completes over USB with Wi-Fi off, and leaving the app mid-sync pauses and later resumes without losing finished files.

Not planned: everything under non-goals.
