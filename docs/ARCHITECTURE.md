# Multi-device architecture

Status: **proposal**, not built. This covers how Honeycomb on the desktop, Honeycomb on iOS and OpenSubsonic servers work together. It starts from the code as it is (October 2026) and lists every place where the plan and the code disagree.

## Principles

1. **Every link between devices speaks OpenSubsonic.** There's no custom sync protocol. Anything the standard can't do goes into one Honeycomb extension, advertised through `getOpenSubsonicExtensions`. As a result, any OpenSubsonic server (Navidrome and others) works as a source with no extra code.
2. **Roles, not devices.** Each app keeps a list of `Source`s, and sync, dedupe and fetch order only look at what a source reports:

   | Capability | Meaning |
   | --- | --- |
   | `fullLibrary` | holds the whole library, not just a cache |
   | `alwaysOn` | can be expected to answer (a server), as opposed to sometimes (a laptop) |
   | `ownsState` | its playlists, stars, ratings and play counts are the true ones |
   | `local` | files on this device; no network involved |

3. **One state owner.** Each setup has exactly one source that owns playlists, stars, ratings and play counts. Other devices send changes to it and read from it. v1 never merges two-way. The one exception is play counts from Honeycomb devices (C5): each device only raises its own running totals, so adding them up can never conflict.

## Setups

| Setup | Library on | State owner | Desktop role | Phone role |
| --- | --- | --- | --- | --- |
| Phone only | phone (local files) | phone | none | local player |
| Desktop only | computer | desktop | library (today's app) | none |
| Desktop + phone | computer | desktop | library + OpenSubsonic server | client of the desktop |
| Server + desktop and/or phone | server | server | client of the server | client of the server (plus the desktop on the LAN, as a faster copy) |

Onboarding on both apps asks **"Where's your music?"** (on this device / on my computer / on a server) instead of assuming a setup.

### Modes

The answer becomes the app's **mode**, which can be changed later in Settings. A mode is a preset on top of the source list: it decides which sources are allowed and which UI is shown. Sync and dedupe code never look at the mode.

**Desktop: 3 modes**

| Mode | What it does | UI |
| --- | --- | --- |
| **1. Standalone** | your library, as today | no server or phone rows, no remote or pinned states |
| **2. With phones** | your library, plus the OpenSubsonic server for paired phones. State owner | phone rows (Sync, Disconnect) in Settings → Library, Settings → Phone |
| **3. With server** | client of a server (metadata mirror, streaming, pinning). Serves phones on the same Wi-Fi from what it has locally, as a faster copy, never as the state owner | server row, track states, tag editing and Inbox off for `remote` tracks (C11) |

**Phone: 4 modes**

| Mode | Sources | State owner |
| --- | --- | --- |
| **1. Standalone** | its own folders (and optionally the system Music library) | phone |
| **2. With PC** | the desktop | desktop |
| **3. With server** | a server only, no desktop | server |
| **4. With server + PC** | the server, plus the desktop on the same Wi-Fi as a faster copy (fetch priority: local → desktop → server) | server |

**Switching modes** goes through the upgrade paths below. Going down a mode (2 → 1, 3 → 1, 4 → 3, …) runs **Disconnect** on the sources that go away, with its keep-or-delete choice.

## What exists today

| Piece | Where | Notes |
| --- | --- | --- |
| Library DB | `db.rs`, `models.rs` | SQLite, flat `tracks` table (no artist or album tables), `i64` ids, half-star ratings 0–10, `loved` flag, smart playlists evaluated in SQL (`smart.rs`) |
| Scanner | `scanner.rs` | `scan()` has no Tauri dependency. Only `spawn_scan` uses `AppHandle` (for progress events) |
| Tags | `meta.rs` | lofty. MusicBrainz ids are **not read** today |
| Kaseki | `kaseki.rs` | has no Tauri dependency. `convert.rs` reuses its decoder and resampler |
| Converter | `convert.rs` | MP3, AAC (macOS only), Vorbis, FLAC 16. Used by device sync only |
| Phone sync server | `phone.rs`, `docs/phone-sync.md`, branch `wip/phone-sync` | custom JSON-over-HTTPS protocol (details below) |
| App glue | `lib.rs`, `player.rs`, `media.rs`, `plugins.rs`, `version.rs` | Tauri-dependent |

Everything is one crate (`src-tauri`), with no Cargo workspace.

### The current phone protocol, and what to keep from it

`phone.rs` is a hyper 1 + rustls 0.23 server on its own tokio runtime:

- a self-signed certificate (`phone-cert.der`) that the phone pins by SHA-256;
- Bonjour `_honeycomb._tcp` with `libraryId` in TXT;
- pairing with a 6-digit code (single use, 5 minutes, 5 tries), shown as a QR code (`honeycomb://pair?h=…&p=…&c=…&f=…`) or typed after checking a short fingerprint;
- per-device tokens stored as SHA-256 hashes in `devices` (DB v6), and revocable;
- `/v1/library`, a whole-library snapshot; `/v1/tracks/{id}/file`, with Range support; `/v1/tracks/{id}/art`; `/v1/events` for plays (de-duplicated by phone-made UUID in `phone_plays`) and ratings.

**Recommendation: replace the API surface, keep the plumbing.** Of the three options:

- **Keep it alongside OpenSubsonic: no.** Two protocols means two things to secure, test and document. Nothing depends on the custom one yet: it's unreleased, and there's no phone app.
- **Replace all of it: no.** Most of it isn't protocol-specific and is already tested.
- **Reuse (recommended):**

  | Keep as is | Becomes | Dropped |
  | --- | --- | --- |
  | TLS identity and pinning, Bonjour advert, server lifecycle (`Server`, `Advert`, `Running`, on/off setting), Range parsing, the pinned-client test harness, Settings → Phone UI | the code/QR pairing becomes a Honeycomb extension endpoint that returns an **API key** for `apiKeyAuthentication`. The `devices` table holds the hashed keys (same table, same revocation). `phone_plays` is replaced by per-device play totals (see conflict C5) | `/v1/library`, `/v1/events`, `/v1/tracks/*`; these are replaced by standard endpoints and `playTotals` |

Nothing is deleted until you approve it. The WIP branch keeps the current version.

## Desktop

### Server crate

New crate `crates/honeycomb-server`: an OpenSubsonic server library. It takes a library handle (DB path plus data dir) and has no Tauri dependency. The desktop starts it in-process. Later, a small `main.rs` in the same crate becomes a headless `honeycomb-server` (Docker image) that scans folders itself.

- **HTTP stack:** axum on the hyper 1 + rustls 0.23 + tokio stack `phone.rs` already uses. Today's handlers are a hand-written `match` on the path. With around 25 endpoints, axum's routing and extractors pay for themselves.
- **Network:** LAN only by default. It binds every interface, as `phone.rs` does today, but answers only private and link-local addresses unless the user turns on "allow from anywhere" (headless defaults to on).
- **Endpoints for v1:** only what Honeycomb's own clients need. Third-party clients get whatever falls out of that.

| Group | Endpoints |
| --- | --- |
| System | `ping`, `getLicense`, `getOpenSubsonicExtensions` |
| Browsing | `getMusicFolders`, `getArtists`, `getArtist`, `getAlbum`, `getSong`, `getAlbumList2`, `search3` (empty query = everything, for offline sync) |
| Playlists | `getPlaylists`, `getPlaylist`, `createPlaylist`, `updatePlaylist`, `deletePlaylist` |
| Media | `stream`, `download`, `getCoverArt`, `getLyricsBySongId` (`songLyrics`) |
| State | `star`, `unstar`, `setRating`, `scrobble`, `getStarred2` |
| Honeycomb extension | `pair` (no auth), `upload`, `playTotals` |

### Core crate (the restructuring the server needs)

The server needs the DB, models, smart playlists, tags and the scanner, but not the app. Two crates come out of `src-tauri`:

- **`crates/honeycomb-core`:** `models`, `db`, `smart`, `meta`, `scanner::scan`, `matching`, `lyrics` and `net`, plus `resolve_playlist` (moved out of `lib.rs`). `spawn_scan` stays in the app, because it emits Tauri events.
- **`crates/kaseki`:** `kaseki.rs`, plus `convert.rs` (which depends on it). This is the same split the iOS experiment needs anyway.

`src-tauri` keeps `lib.rs`, `player.rs`, `media.rs`, `plugins.rs`, `version.rs`, `sync.rs` and `importer.rs`, and depends on both crates. The move is mechanical: none of the moved modules import Tauri.

### Desktop as a client of a server

When a server source is added, the desktop keeps a **metadata mirror** in its normal `library.db`:

- it pages through `search3` with an empty query, then fetches playlists and starred items;
- it stores **no audio** by default.

The column browser, search and sort keep working at 100k tracks, because they still read local SQLite. Each track gets a state:

| State | Meaning |
| --- | --- |
| `remote` | metadata only; plays by streaming |
| `downloading` | in progress |
| `pinned` | downloaded on request and kept |
| `local-only` | a file on this computer that the server doesn't have |

Without a server source, every track is local, and the remote and pinned UI is hidden.

### Connections in Settings → Library

Servers and paired phones are listed in the same place as music folders: the list under **Settings → Library** (`settings.ts`, `renderFolders`). A folder row keeps its **Remove** button. Server and phone rows get **Sync** and **Disconnect** instead:

| Row | Shows | Sync | Disconnect |
| --- | --- | --- | --- |
| Folder | path | (none; "Rescan now" covers it) | **Remove**, as today |
| Server | name, address, track count, last synced, state owner or not | refreshes the metadata mirror and sends any changes still waiting | asks **Keep the music on this computer** or **Delete it**, then removes the source. Keep turns pinned files into `local-only` tracks. Delete removes them and the mirrored tracks |
| Phone | device name, last seen, last synced, and how to sync now (C15) | none (C15) | asks **Leave the music on the phone** or **Delete it from the phone**, then revokes the device's API key (today's "Remove" in Settings → Phone). The row then shows "Removing… · Undo" for a week, and the phone carries out the choice the next time it connects (C17) |

**Disconnecting never deletes music without asking.** Every Disconnect asks whether to keep the music on that device or delete it, and **Keep** is the default button. The same choice appears when disconnecting from the other side: the phone app's own "Disconnect from this computer" or "Remove server" asks too. Keeping always leaves a working library: the kept files become local tracks with their tags, art and the stats they had when last synced. **Delete** only ever removes files that came from that connection: music in the phone's own picked folders, or in the computer's library folders, is never touched.

Settings → Phone keeps only the on/off switch and **Pair a phone…**. Its device list moves into this one, next to the folders. A row gets a small status line when something is wrong (server unreachable, phone not seen for 30 days).

### Always ready for phones

In mode 2 the desktop needs no buttons to connect: the server starts with the app, Bonjour announces it, and phones connect on their own whenever their app opens or background refresh runs. Only pairing takes a click, once. Two options cover the time Honeycomb isn't open:

- **Keep running in the menu bar when the window is closed** (system tray on Windows and Linux). The server and Bonjour keep going; playback stops as it does today when the window closes. Tauri's built-in tray icon.
- **Start at login**, hidden in the menu bar (`tauri-plugin-autostart`).

Both are off by default. After the first phone is paired, Settings → Phone suggests turning them on: "So your phone can sync even when Honeycomb's window is closed." iOS can't wake an app when the phone joins a Wi-Fi network, so the phone side stays "when the app opens, and when iOS allows background refresh".

## iOS

- **Where:** the iOS app is developed in a separate, private repository. It uses the core crates (`honeycomb-core`, `kaseki`, the client and sync planner) from this repo. No iOS app code goes into this repo.
- **Stack:** native SwiftUI, plus the shared Rust core (`honeycomb-core` for the library DB, `kaseki` for playback).
- **Standalone mode:** the user picks folders with the document picker. The app stores security-scoped bookmarks and plays files in place. It rescans on open and with a button, handles iCloud placeholders (`NSMetadataUbiquitousItemDownloadingStatus`) and stale bookmarks (asks the user to pick the folder again). Optional: non-DRM songs from the system Music library.
- **Remote sources:** a server, the desktop, or both.
  - **Offline:** `download` through a background `URLSession`. Synced files are excluded from iCloud backup (`isExcludedFromBackup`).
  - **Info.plist:** `NSLocalNetworkUsageDescription`, `NSBonjourServices = [_honeycomb._tcp]`.
  - **TLS:** the desktop's certificate is self-signed and pinned in the URLSession delegate. ATS still applies its protocol rules (TLS 1.2+, forward secrecy, SHA-256 signatures), and rcgen with rustls meets them.
- **The phone never runs an OpenSubsonic server.** Over a cable it only relays its own requests (C9).

**Where the networking lives (A ✅, approved).** The shared Rust core owns the networking. Swift only handles the two things iOS reserves for its own APIs.

| Request | Decides | Sends |
| --- | --- | --- |
| Browsing, library listing, playlists, ratings, play totals, pairing | Rust | **Rust** |
| Streaming and downloads while the app is open (Wi-Fi or cable) | Rust | **Rust** |
| Downloads that must continue after the user leaves the app | Rust | **Swift**: background `URLSession`, from a job list the core hands over (URL, API key, expected size, certificate fingerprint). Swift reports each finished file back to the core |
| Finding computers on the network | n/a | **Swift**: `NWBrowser` (no multicast entitlement needed, unlike a Rust mDNS library), which passes the addresses to the core |

- **The core** contains the OpenSubsonic client, certificate pinning (the rustls verifier from `phone.rs`'s test harness), the sync planner (compares a source's listing with the local DB and decides what to download, delete or push), dedupe, the local DB, Kaseki and the cable transport. The cable needs no local relay, because the core can speak OpenSubsonic over the tunnel's stream directly.
- **Swift** also watches network conditions (`NWPathMonitor`: Low Data Mode, cellular allowed, VPN) and passes them to the core, since Rust's networking doesn't see those system settings.
- **Pinning exists twice:** in the core for its requests, and in the `URLSession` delegate for background downloads. Both compare one SHA-256 string against the same stored value.
- **The desktop** uses the same client and planner for phase 4 (as a client of a server). Its discovery stays `mdns-sd`.

## Multi-source rules

1. **Dedupe:** by MusicBrainz recording id when both sides have one. Otherwise, the existing loose match (`matching::LibraryIndex`: artist, title, duration). See C4 on audio hashes.
2. **Fetch priority:** local file → desktop on the LAN (found over Bonjour) → server.
3. **State:** changes go only to the state owner. Others read from it.

## Auth

- **Our server:** `apiKeyAuthentication` only. A key is issued per device by pairing, stored hashed, and revocable in Settings (the spec requires showing and revoking keys).
- **Talking to other servers:** `apiKeyAuthentication` when the server advertises it. Otherwise legacy salted-token auth, which means storing the user's password in the system keychain (`secrets.rs`).

## Honeycomb extension

Advertised as `honeycomb` in `getOpenSubsonicExtensions`. v1 covers:

- **Pair** (no auth): code → API key, plus `libraryId`.
- **Upload** (auth): a file plus its play stats, for migrations. The server places it under a folder it chooses and scans it.
- **Play totals** (`playTotals`, auth): the device sends, for each song it played, its **running total since pairing** plus its latest play time: `{trackId, count, lastPlayed}`. The server stores one row per device and song, and replaces it only when `count` goes up. A song's play count = the owner's own plays + the sum of every device's total; `last_played` = the latest of all. Sending the same totals twice changes nothing, so retries are always safe.
- **Half-star ratings:** an optional `rating10` (0–10) on `setRating` and on song results, so Honeycomb clients don't lose half-stars.

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

Cable is a transport, not a protocol: OpenSubsonic over a usbmuxd TCP tunnel. Android cable support is future work.

## Conflicts and open questions

| # | Conflict | Proposal |
| --- | --- | --- |
| C1 ✅ | **No artist or album entities.** `tracks` is flat, but `getArtists` and `getAlbum` need stable ids for them. | **Approved.** Derive ids from a hash of the normalized album artist + album (album id) and the artist name (artist id). Compute them on demand at first, and cache them in a table only if it's too slow at 100k tracks. Renaming an album or artist changes its id. Song ids never change, because tag edits update the row in place (`upsert_track`, `ON CONFLICT(path)`), so nothing is downloaded again. **Rule:** clients keep long-lived data (offline pins, later album stars) per **song id**. A pinned album also remembers its album id. When that id disappears, the client looks up the album its songs moved to and moves the pin there, so songs added to the album later are still kept. |
| C2 ✅ | **Track ids aren't stable.** They're SQLite autoincrement and tied to the path, so moving a file gives it a new id, and clients' caches go stale. | **Approved for v1:** accepted, since clients re-sync from the snapshot. **Later fix: move detection in the scanner.** Today a moved or renamed file is deleted from `tracks` (`scanner.rs`, "files that disappeared") and found again as a new song. Besides the new id, that also loses its play count, rating, loved flag and playlist entries, which is a bug on the desktop with no phone involved. The fix: at the end of a scan, before deleting vanished paths, match each vanished track with a newly found one (same MusicBrainz id, or same size + duration + title/artist/album). On a match, update the old row's `path` instead of deleting it and inserting a new one, so the id and all stats are kept. Planned as its own small change, independent of the phases. |
| C3 ✅ | **Ratings are half-stars.** Subsonic `userRating` is 1–5 whole stars. | **Approved.** The state owner stores one value, 0–10. **Reading:** `rating10` returns it exactly (extension). `userRating` = `ceil(rating10 / 2)`, so a half star shows as 1 and never as unrated, and 3.5 shows as 4. **Writing:** `rating10` is stored as is. A standard `setRating` of n is stored as n × 2. **Rule:** clients send a rating only when the user changes it, never a value they just read, so rounding never writes itself back. **Owner without the extension** (a server only storing whole stars): the desktop's star picker switches to whole stars while that server is the state owner. Moving a library there rounds half-stars up, and the migration dialog says so first. |
| C4 ✅ | **"Else audio hash" can't work across sources.** Subsonic doesn't expose a content hash, so comparing would mean downloading every file. Hashing locally at scan time also slows scans down a lot. | **Approved.** Use MBID → loose match (`matching::LibraryIndex`, which moves into `honeycomb-core`), which already handles real libraries. Add hashing only if it turns out to be needed. MBIDs need a new column plus reading them in `meta.rs` (lofty `ItemKey::MusicBrainzRecordingId`). |
| C5 ✅ | **`scrobble` isn't idempotent.** A retried batch double-counts. The current protocol solves this with `phone_plays`. | **Approved (revised).** **Honeycomb servers:** devices count their own plays and send running totals per song through `playTotals` (extension; see above). Syncing happens when the app opens, and in the background once or twice a day when the OS allows it (iOS `BGAppRefreshTask`). This replaces both `playId` and the `phone_plays` table. The library only stores `play_count` and `last_played`, so totals lose nothing the app uses. Totals count plays **since pairing**: a re-paired device starts from 0 under a new device row, and the old row's total stays counted. Removing a device keeps its counted plays. **Last.fm:** a device that scrobbles does it itself, straight to the service, because totals can't be split back into timestamped plays. **Other servers:** standard `scrobble`; delete plays locally only after a 200, and accept rare duplicates. **Navidrome plugin (later):** not possible on v0.64.2, because plugins can't receive requests. Possible once navidrome/navidrome#5045 (plugin HTTP endpoints, open as of 2026-10-30) lands: `POST /rest/ext/honeycomb/playTotals` keeps the last total it applied per user, device and song, and calls the standard `scrobble` for the difference (zero or less means nothing to do). Same totals model as Honeycomb servers. Rough effort: 2 days plugin + half a day client. Known limits: send one request per device at a time; a server crash between scrobbling and saving can affect one batch; Last.fm sees the difference as several plays at almost the same moment (C8 already keeps Last.fm in one place). |
| C6 ✅ | **The brief puts the API key in the QR code.** Anyone who photographs the screen gets a permanent key. It also contradicts "keep the existing pairing design". | **Approved.** Keep the code in the QR code (single use, 5 minutes) and exchange it for a key over pinned TLS. |
| C7 ✅ | **Smart playlists are read-only in Subsonic terms.** | **Approved.** Resolve them on `getPlaylist`, and refuse `updatePlaylist` with a clear error. |
| C8 ✅ | **Double scrobbles to Last.fm.** With a server as state owner, the server may scrobble to Last.fm (if it's set up) and so may Honeycomb's scrobbling plugin. | **Approved.** Remote mode warns when both are on. The user picks one. |
| C9 ✅ | **Cable vs. "phone never acts as a server".** usbmuxd only lets the computer connect *to* the phone, so the phone must accept a TCP connection over USB. | **Approved.** The phone accepts that one connection only as a relay: it carries the phone's own client requests to the desktop, and serves nothing. TLS stays end to end. |
| C10 ✅ | **Background downloads can't use the cable.** Background `URLSession` runs out of process, so it can't reach the app's tunnel. | **Approved.** Cable sync is foreground only: the Rust core speaks OpenSubsonic over the tunnel directly (see A). Wi-Fi keeps the background downloads. **Warning pop-up:** when a cable sync starts, the phone explains that the app has to stay open until it's done, and offers **Keep the screen on until done** (`isIdleTimerDisabled = true`, turned back off when the sync finishes, fails or is cancelled). When the computer is also reachable over Wi-Fi, it offers **Continue over Wi-Fi** too, which hands the rest to background downloads. If the user leaves anyway, the app asks iOS for a little extra time (`beginBackgroundTask`) to finish the current file, then pauses. Files already finished are kept, so the next sync resumes. A "Don't show again" option keeps the screen-on choice. |
| C11 ✅ | **Desktop as a server client is a second app mode.** Tag editing, the Inbox, rescans and file-based lyrics assume local files. | **Approved.** `remote` tracks disable those actions in v1. Tag edits for remote tracks are out of scope (Subsonic can't write tags). Which UI each app shows is decided by its mode (see Modes). |
| C12 ✅ | **Not verified: does Navidrome support `apiKeyAuthentication`?** The spec exists, and Ampache advertises it. | **Approved.** Check with `getOpenSubsonicExtensions` on a real instance before phase 4. The legacy fallback covers it either way. |
| C13 ✅ | **Empty `search3` queries** are an OpenSubsonic clarification that most servers follow, but not all. | **Approved.** Fall back to walking `getAlbumList2` + `getAlbum` when an empty query returns nothing. |
| C14 ✅ | **Headless server and the GUI-only pieces.** Plugins, the updater and media keys don't apply headless. Settings → Phone becomes a web or CLI pairing flow there. | **Approved.** Headless pairing prints the code and QR code to the console. Nothing more in v1. |
| C15 ✅ | **The desktop can't make a phone sync.** The phone is only a client, so the desktop can't reach it. A phone only syncs when its app opens a connection. | **Approved: no Sync button on phone rows.** The row shows "Last synced 2 h ago", plus a short line under it saying how to sync right now: "To sync now, open Honeycomb on the phone (same Wi-Fi, or plugged in)." A small ⓘ next to it explains the rest: the phone also syncs by itself once or twice a day when iOS lets it, and the computer can't start a sync on its own. When the phone hasn't synced for 30 days (the status line in Settings → Library), the hint is highlighted. |
| C16 ✅ | **What Disconnect does to a server's tracks.** Plays and ratings made while it was the state owner live on the server. | **Approved, with a last pull.** When the server can be reached, Disconnect first asks: **Bring your stuff over before disconnecting?** • **Ratings, play counts, loved songs and playlists** (on by default, small): copied to the desktop for every track it keeps (pinned files and tracks that dedupe matched to local files). Playlists become manual playlists, and play counts take the larger of the two numbers. • **Also download the songs that aren't on this computer yet** (off by default, shows the size, e.g. "2,340 songs, 41 GB"). Disconnect waits for it, can be cancelled, and resumes on the next try. After the pull, the desktop becomes the state owner, then the usual keep-or-delete choice runs. When the server can't be reached, the dialog says the stats on the server can't be brought over right now, and offers **Try again later** (the default) or **Disconnect anyway**. |
| C17 ✅ | **Telling a phone what to do after it's removed.** The desktop can't reach the phone, and once its key is revoked the phone's next request just gets an auth error. | **Approved: a week's grace.** Disconnect **suspends** the device's key for 7 days and stores the user's choice (`keep` or `delete`). The row shows "Removing… (6 days left) · Undo". **Undo** within the week restores the device as it was: no re-pairing, and its play totals carry on. **Phone connects within the week:** instead of library access it gets the choice. Keep → the music stays as local tracks. Delete → the phone shows "Removed from <computer>. Its music will be deleted from this phone" with **Keep it instead** / **OK**, a second confirmation on the device that loses the files. Then the device row is deleted. **No contact for 7 days:** the key, the choice and the row are deleted. A phone that later gets a plain auth error asks its user, Keep by default, and never deletes on its own. **Never:** treating "can't reach the computer" as removal; that happens every time the phone leaves home. |

## Phased plan

Each phase ends in something testable.

1. **Workspace split.** Create a Cargo workspace with `crates/honeycomb-core` and `crates/kaseki`, and make `src-tauri` depend on them. No behaviour changes.
   *Done when:* all 72 tests pass, and the app runs, scans and plays exactly as before.
2. **Server crate.** `crates/honeycomb-server` with the v1 endpoints over a `honeycomb-core` library, plus API-key auth and the Honeycomb extension (`pair`, `playTotals`, `rating10`). This includes C1 (ids) and C7 (smart playlists).
   *Done when:* tests drive every endpoint with a pinned client (the harness moves over from `phone.rs`), and an off-the-shelf OpenSubsonic client can browse and play from it on a laptop.
3. **Desktop server and pairing.** The desktop starts the server in place of `phone.rs`'s API: same certificate, Bonjour advert, Settings → Phone and `devices` table. Pairing returns an API key. Paired phones appear in the Settings → Library list with Disconnect. Menu-bar mode and start at login. The old `/v1` endpoints are removed (only with your approval). `docs/phone-sync.md` is rewritten as the extension spec.
   *Done when:* QR pairing and typed-code pairing both work from a test client, and revoking a device turns its key's next request into an auth error.
4. **Desktop as a client of a server.** Sources in Settings, the metadata mirror, track states (`remote`, `downloading`, `pinned`, `local-only`), streaming through Kaseki, MBID column + dedupe, the state-owner switch with playlist and star push, and server rows (Sync, Disconnect) in Settings → Library. Starts with verifying C12.
   *Done when:* the desktop browses and plays a real Navidrome library of 10k+ tracks, pins an album for offline use, and plays and stars show up on the server.
5. **iOS** (in its own private repo). Kaseki on iOS (the 2–3 day experiment), then standalone local playback, then the desktop and server sources with background downloads, then upload-based migration.
   *Done when:* each step works on a real iPhone.
6. **Cable (iPhone).** The usbmuxd relay (C9, C10) on top of phase 5.

Not planned: Android, two-way merging, tag editing for remote tracks, a web UI for the headless server.
