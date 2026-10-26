//! Syncing music to a player or flash drive.
//!
//! A sync copies the chosen tracks to `<destination>/Artist/Album/NN Title.ext`
//! with names that are safe on FAT32 and exFAT, writes each chosen playlist
//! as an .m3u8 with paths relative to the destination, and keeps a manifest
//! (`.honeycomb-sync.json`) of what it put there. Only files in the manifest
//! are ever replaced or removed, so the user's own files on the drive are left
//! alone. Unchanged files (same size and modification time at the source) are
//! skipped.
//!
//! Files are written as `name.part` and renamed when complete, and the
//! manifest is saved as it goes, so a cancelled or unplugged sync leaves no
//! half-written tracks behind and the next sync carries on.
//!
//! Rockbox players keep a log of what they played (`.rockbox/playback.log`,
//! with logging turned on in the player's settings). Before copying, plays of
//! synced files are read from it and added to the library's play counts.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::db;
use crate::models::Track;

pub const MANIFEST: &str = ".honeycomb-sync.json";
/// Longest file or folder name written, in characters. FAT allows 255, but
/// players and car stereos often choke well before that.
const MAX_NAME: usize = 80;
/// Save the manifest at least this often while copying.
const SAVE_EVERY: usize = 20;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    /// Folder on the device that the music goes into.
    pub dest: String,
    /// Sync everything instead of `playlists`.
    pub whole_library: bool,
    pub playlists: Vec<i64>,
    /// Remove synced files that are no longer chosen.
    pub remove_unselected: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    track_id: i64,
    /// Source size and modification time when it was copied.
    size: u64,
    mtime: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    /// Path relative to the destination, with `/` separators → what's there.
    files: HashMap<String, Entry>,
    /// Playlist files written, relative to the destination.
    #[serde(default)]
    playlists: Vec<String>,
    /// Bytes of the Rockbox playback log already read.
    #[serde(default)]
    rockbox_log_read: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub copied: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub playlists: usize,
    pub bytes_copied: u64,
    /// Plays read back from a Rockbox player.
    pub plays_imported: usize,
    pub failed: Vec<String>,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub current: String,
}

// ---------- names ----------

/// One path component made safe for FAT32/exFAT and most players: no
/// reserved characters, no trailing dots or spaces, no reserved device names,
/// and not too long.
pub fn safe_name(s: &str, fallback: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 || c == '\u{7f}' => '_',
            c => c,
        })
        .collect();
    let trim = |s: &str| s.trim().trim_end_matches(['.', ' ']).trim_start().to_string();
    out = trim(&out);
    if out.chars().count() > MAX_NAME {
        out = trim(&out.chars().take(MAX_NAME).collect::<String>());
    }
    if out.is_empty() || out.chars().all(|c| c == '_' || c == '.') {
        out = fallback.to_string();
    }
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        out.insert(0, '_');
    }
    out
}

/// Where a track goes on the device, relative to the destination, with `/`.
pub fn track_path(t: &Track) -> String {
    let src = Path::new(&t.path);
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let artist = if t.album_artist.is_empty() { &t.artist } else { &t.album_artist };
    let artist = safe_name(artist, "Unknown Artist");
    let album = safe_name(&t.album, "Unknown Album");
    let title = if t.title.is_empty() { stem } else { t.title.clone() };
    let mut file = String::new();
    if let Some(d) = t.disc_no.filter(|&d| d > 1) {
        file.push_str(&format!("{d}-"));
    }
    if let Some(n) = t.track_no {
        file.push_str(&format!("{n:02} "));
    }
    file.push_str(&title);
    // Leave room for the extension and a " (2)" when names collide.
    let file = safe_name(&file.chars().take(MAX_NAME - 10).collect::<String>(), "Track");
    format!("{artist}/{album}/{file}.{ext}")
}

/// Give each track its own path; FAT compares names without case.
fn assign_paths(tracks: &[Track]) -> Vec<String> {
    let mut used: HashSet<String> = HashSet::new();
    tracks
        .iter()
        .map(|t| {
            let base = track_path(t);
            let mut p = base.clone();
            let mut n = 2;
            while !used.insert(p.to_lowercase()) {
                let (head, ext) = base.rsplit_once('.').unwrap_or((&base, ""));
                p = format!("{head} ({n}).{ext}");
                n += 1;
            }
            p
        })
        .collect()
}

fn native(dest: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(dest.to_path_buf(), |p, c| p.join(c))
}

/// A path relative to the destination made only of ordinary names: no `..`,
/// no root or drive, no `\`. Anything else in a manifest didn't come from
/// Honeycomb.
fn safe_rel(rel: &str) -> bool {
    !rel.is_empty() && rel.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && !c.contains(['\\', ':']))
}

/// Whether `path`'s folder really is inside `dest`, after following
/// symlinks (a drive could link a folder to somewhere else).
fn inside(dest: &Path, path: &Path) -> bool {
    match (dest.canonicalize(), path.parent().map(fs::canonicalize)) {
        (Ok(d), Some(Ok(p))) => p.starts_with(d),
        _ => false,
    }
}

// ---------- manifest ----------

/// The manifest lives on the device, so anyone with the device can edit it:
/// entries that could point outside the destination are dropped.
fn load_manifest(dest: &Path) -> Manifest {
    let mut m: Manifest =
        fs::read(dest.join(MANIFEST)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    m.files.retain(|rel, _| safe_rel(rel));
    m.playlists.retain(|p| safe_rel(p) && !p.contains('/') && p.ends_with(".m3u8"));
    m
}

fn save_manifest(dest: &Path, m: &Manifest) -> Result<()> {
    let tmp = dest.join(format!("{MANIFEST}.part"));
    fs::write(&tmp, serde_json::to_vec(m)?)?;
    fs::rename(&tmp, dest.join(MANIFEST))?;
    Ok(())
}

// ---------- Rockbox ----------

/// The folder holding `.rockbox`, if `dest` is on a Rockbox player.
pub fn rockbox_root(dest: &Path) -> Option<PathBuf> {
    dest.ancestors().find(|p| p.join(".rockbox").is_dir()).map(Path::to_path_buf)
}

/// Read new plays from the player's log and count them in the library.
/// Lines are `timestamp:elapsed_ms:length_ms:/path/on/player`. A play counts
/// under the same rule as scrobbles: half the song or 4 minutes, songs over
/// 30 seconds (and anything heard to the end).
fn import_rockbox_plays(conn: &Connection, dest: &Path, root: &Path, m: &mut Manifest) -> Result<usize> {
    let log = root.join(".rockbox").join("playback.log");
    let Ok(mut f) = fs::File::open(&log) else { return Ok(0) };
    let len = f.metadata()?.len();
    // A shorter log was cleared on the player: read it from the start.
    let from = if m.rockbox_log_read <= len { m.rockbox_log_read } else { 0 };
    let mut buf = Vec::new();
    std::io::Seek::seek(&mut f, std::io::SeekFrom::Start(from))?;
    f.read_to_end(&mut buf)?;
    // Only whole lines; a line still being written waits for next time.
    let complete = buf.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let text = String::from_utf8_lossy(&buf[..complete]);

    // Player paths like "/Music/Artist/Album/01 Song.mp3" → synced track.
    let prefix: String = dest
        .strip_prefix(root)
        .unwrap_or(Path::new(""))
        .components()
        .map(|c| format!("/{}", c.as_os_str().to_string_lossy()))
        .collect();
    let by_path: HashMap<String, i64> = m
        .files
        .iter()
        .map(|(rel, e)| (format!("{prefix}/{rel}").to_lowercase(), e.track_id))
        .collect();

    let mut plays: HashMap<i64, (u32, i64)> = HashMap::new();
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(4, ':');
        let (Some(ts), Some(elapsed), Some(length), Some(path)) = (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (Ok(ts), Ok(elapsed), Ok(length)) = (ts.parse::<i64>(), elapsed.parse::<f64>(), length.parse::<f64>()) else {
            continue;
        };
        let (elapsed, length) = (elapsed / 1000.0, length / 1000.0);
        let counts = crate::scrobble::threshold(length).map_or(elapsed >= length - 1.0, |t| elapsed >= t);
        if !counts {
            continue;
        }
        if let Some(&id) = by_path.get(&path.to_lowercase()) {
            let p = plays.entry(id).or_default();
            p.0 += 1;
            p.1 = p.1.max(ts);
        }
    }
    let mut imported = 0;
    for (id, (n, last)) in plays {
        conn.execute(
            "UPDATE tracks SET play_count = play_count + ?2,
                 last_played = CASE WHEN ?3 > COALESCE(last_played, 0) THEN ?3 ELSE last_played END
             WHERE id = ?1",
            rusqlite::params![id, n, last],
        )?;
        imported += n as usize;
    }
    m.rockbox_log_read = from + complete as u64;
    Ok(imported)
}

// ---------- the sync ----------

/// The tracks a profile wants, in order, and the playlists to write.
pub fn selection(conn: &Connection, p: &Profile, resolve: &dyn Fn(i64) -> Result<Vec<i64>>) -> Result<(Vec<Track>, Vec<(String, Vec<i64>)>)> {
    let all = db::playlists(conn)?;
    let mut lists = Vec::new();
    let mut ids: Vec<i64> = Vec::new();
    let mut seen = HashSet::new();
    if p.whole_library {
        let mut tracks = db::all_tracks(conn)?;
        tracks.sort_by(|a, b| a.path.cmp(&b.path));
        ids.extend(tracks.iter().map(|t| t.id));
        seen.extend(ids.iter().copied());
    }
    for pl in all.iter().filter(|pl| p.playlists.contains(&pl.id)) {
        let tids = resolve(pl.id)?;
        for &t in &tids {
            if seen.insert(t) {
                ids.push(t);
            }
        }
        lists.push((pl.name.clone(), tids));
    }
    let tracks = db::tracks_by_ids(conn, &ids)?;
    Ok((tracks, lists))
}

fn copy_file(src: &Path, dst: &Path, cancel: &AtomicBool) -> Result<u64> {
    if let Some(dir) = dst.parent() {
        fs::create_dir_all(dir)?;
    }
    let part = dst.with_file_name(format!("{}.part", dst.file_name().unwrap_or_default().to_string_lossy()));
    let mut input = fs::File::open(src).with_context(|| format!("reading {}", src.display()))?;
    let mut out = fs::File::create(&part).with_context(|| format!("writing {}", part.display()))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(out);
            let _ = fs::remove_file(&part);
            bail!("cancelled");
        }
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        total += n as u64;
    }
    out.sync_all()?;
    drop(out);
    fs::rename(&part, dst)?;
    Ok(total)
}

fn write_playlist(dest: &Path, name: &str, tracks: &[(&Track, &str)]) -> Result<String> {
    let rel = format!("{}.m3u8", safe_name(name, "Playlist"));
    let mut out = String::from("#EXTM3U\n");
    for (t, path) in tracks {
        out.push_str(&format!("#EXTINF:{},{} - {}\n{}\n", t.duration.round() as i64, t.artist, t.title, path));
    }
    fs::write(dest.join(&rel), out)?;
    Ok(rel)
}

/// Remove folders under `dest` left empty by removed files.
fn prune_dirs(dest: &Path, rel: &str) {
    let mut p = native(dest, rel);
    while let Some(parent) = p.parent() {
        if parent == dest || !parent.starts_with(dest) || fs::remove_dir(parent).is_err() {
            break;
        }
        p = parent.to_path_buf();
    }
}

pub fn run(
    conn: &Connection,
    p: &Profile,
    resolve: &dyn Fn(i64) -> Result<Vec<i64>>,
    progress: &dyn Fn(Progress),
    cancel: &AtomicBool,
) -> Result<Report> {
    let dest = PathBuf::from(&p.dest);
    if !dest.is_dir() {
        bail!("{} isn't there. Is the device connected?", dest.display());
    }
    let mut report = Report::default();
    let mut m = load_manifest(&dest);

    // Plays on a Rockbox player come back first, while the manifest still
    // knows every file it played.
    if let Some(root) = rockbox_root(&dest) {
        report.plays_imported = import_rockbox_plays(conn, &dest, &root, &mut m)?;
        save_manifest(&dest, &m)?;
    }

    let (tracks, lists) = selection(conn, p, resolve)?;
    let paths = assign_paths(&tracks);
    let wanted: HashSet<&str> = paths.iter().map(String::as_str).collect();

    // What needs copying, and what may be removed.
    let mut todo = Vec::new();
    for (t, rel) in tracks.iter().zip(&paths) {
        let src = fs::metadata(&t.path);
        let (size, mtime) = match &src {
            Ok(md) => (md.len(), crate::meta::file_mtime(Path::new(&t.path))),
            Err(e) => {
                report.failed.push(format!("{}: {e}", t.path));
                continue;
            }
        };
        let target = native(&dest, rel);
        let have = fs::metadata(&target).ok();
        let current = m.files.get(rel).is_some_and(|e| e.track_id == t.id && e.size == size && e.mtime == mtime)
            && have.as_ref().is_some_and(|md| md.len() == size);
        if current {
            report.unchanged += 1;
        } else if have.is_some() && !m.files.contains_key(rel) && have.as_ref().map(|md| md.len()) != Some(size) {
            // Someone else's file is in the way; never overwrite it.
            report.failed.push(format!("{}: a different file is already there", target.display()));
        } else {
            todo.push((t, rel.clone(), size, mtime));
        }
    }
    let stale: Vec<String> = if p.remove_unselected {
        m.files.keys().filter(|k| !wanted.contains(k.as_str())).cloned().collect()
    } else {
        Vec::new()
    };

    // Enough room? Files being replaced and removed free up their space.
    let need: u64 = todo.iter().map(|(_, _, size, _)| *size).sum();
    let freed: u64 = todo
        .iter()
        .map(|(_, rel, _, _)| rel)
        .chain(stale.iter())
        .filter(|rel| m.files.contains_key(*rel))
        .filter_map(|rel| fs::metadata(native(&dest, rel)).ok().map(|md| md.len()))
        .sum();
    let free = fs2::available_space(&dest).unwrap_or(u64::MAX);
    if need > free.saturating_add(freed) {
        bail!(
            "Not enough space: this sync needs {:.1} GB more and {:.1} GB is free. Choose fewer playlists, or turn on removing files no longer chosen.",
            need.saturating_sub(freed) as f64 / 1e9,
            free as f64 / 1e9
        );
    }

    // Remove first, to make room.
    for rel in &stale {
        let path = native(&dest, rel);
        if !inside(&dest, &path) {
            m.files.remove(rel);
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => report.removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                report.failed.push(format!("{}: {e}", path.display()));
                continue;
            }
        }
        m.files.remove(rel);
        prune_dirs(&dest, rel);
    }
    save_manifest(&dest, &m)?;

    // Copy.
    let total = todo.len();
    let mut last_save = Instant::now();
    for (i, (t, rel, size, mtime)) in todo.iter().enumerate() {
        progress(Progress { done: i, total, current: rel.clone() });
        let target = native(&dest, rel);
        if target.parent().is_some_and(Path::exists) && !inside(&dest, &target) {
            report.failed.push(format!("{}: leads outside the destination", target.display()));
            continue;
        }
        match copy_file(Path::new(&t.path), &target, cancel) {
            Ok(bytes) => {
                report.copied += 1;
                report.bytes_copied += bytes;
                m.files.insert(rel.clone(), Entry { track_id: t.id, size: *size, mtime: *mtime });
            }
            Err(_) if cancel.load(Ordering::Relaxed) => {
                report.cancelled = true;
                break;
            }
            Err(e) => report.failed.push(format!("{}: {e:#}", t.path)),
        }
        if (i + 1) % SAVE_EVERY == 0 || last_save.elapsed().as_secs() >= 5 {
            save_manifest(&dest, &m)?;
            last_save = Instant::now();
        }
    }
    progress(Progress { done: total, total, current: String::new() });

    // Playlists, with the files that made it onto the device.
    if !report.cancelled {
        let by_id: HashMap<i64, (&Track, &str)> =
            tracks.iter().zip(&paths).map(|(t, rel)| (t.id, (t, rel.as_str()))).collect();
        let mut written = Vec::new();
        for (name, ids) in &lists {
            let entries: Vec<(&Track, &str)> = ids
                .iter()
                .filter_map(|id| by_id.get(id).copied())
                .filter(|(_, rel)| m.files.contains_key(*rel))
                .collect();
            match write_playlist(&dest, name, &entries) {
                Ok(rel) => written.push(rel),
                Err(e) => report.failed.push(format!("playlist {name}: {e:#}")),
            }
        }
        for old in m.playlists.iter().filter(|p| !written.contains(p)) {
            let path = native(&dest, old);
            if inside(&dest, &path) {
                let _ = fs::remove_file(path);
            }
        }
        report.playlists = written.len();
        m.playlists = written;
    }
    save_manifest(&dest, &m)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_fat_safe() {
        assert_eq!(safe_name("AC/DC: Live?", "x"), "AC_DC_ Live_");
        assert_eq!(safe_name("  Dots...  ", "x"), "Dots");
        assert_eq!(safe_name("", "Unknown"), "Unknown");
        assert_eq!(safe_name("CON", "x"), "_CON");
        assert_eq!(safe_name("com1.txt", "x"), "_com1.txt");
        assert_eq!(safe_name("Console", "x"), "Console");
        assert_eq!(safe_name(&"é".repeat(200), "x").chars().count(), MAX_NAME);
    }

    #[test]
    fn layout_and_collisions() {
        let t = |path: &str, title: &str, no: Option<u32>, disc: Option<u32>| Track {
            path: path.into(),
            title: title.into(),
            artist: "Feat Artist".into(),
            album_artist: "Band".into(),
            album: "Album".into(),
            track_no: no,
            disc_no: disc,
            ..Default::default()
        };
        let tracks = [
            t("/a/x.FLAC", "Song", Some(1), Some(1)),
            t("/a/y.flac", "song", Some(1), Some(1)),
            t("/a/z.mp3", "", None, Some(2)),
        ];
        let paths = assign_paths(&tracks);
        assert_eq!(paths, ["Band/Album/01 Song.flac", "Band/Album/01 song (2).flac", "Band/Album/2-z.mp3"]);
    }

    fn setup(name: &str) -> (PathBuf, Connection, Vec<i64>) {
        let root = std::env::temp_dir().join(format!("hc-sync-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("music")).unwrap();
        fs::create_dir_all(root.join("stick")).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        db::migrate(&conn).unwrap();
        let mut ids = Vec::new();
        for (i, title) in ["One", "Two", "Three"].iter().enumerate() {
            let path = root.join("music").join(format!("{title}.mp3"));
            fs::write(&path, vec![i as u8; 1000 + i]).unwrap();
            let t = Track {
                path: path.to_string_lossy().into(),
                title: title.to_string(),
                artist: "Artist".into(),
                album: "Album".into(),
                track_no: Some(i as u32 + 1),
                duration: 200.0,
                mtime: crate::meta::file_mtime(&path),
                ..Default::default()
            };
            ids.push(db::upsert_track(&conn, &t).unwrap());
        }
        (root, conn, ids)
    }

    fn playlist(conn: &mut Connection, name: &str, ids: &[i64]) -> i64 {
        conn.execute("INSERT INTO playlists (name, kind, created) VALUES (?1, 'manual', 0)", [name]).unwrap();
        let id = conn.last_insert_rowid();
        db::set_playlist_track_ids(conn, id, ids).unwrap();
        id
    }

    fn sync(conn: &Connection, p: &Profile) -> Report {
        let resolve = |id| db::manual_playlist_track_ids(conn, id);
        run(conn, p, &resolve, &|_| {}, &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn copies_only_changes_and_removes_only_its_own_files() {
        let (root, mut conn, ids) = setup("basic");
        let stick = root.join("stick");
        fs::write(stick.join("my notes.txt"), "mine").unwrap();
        let road = playlist(&mut conn, "Road: Trip", &ids[..2]);
        let mut p = Profile { dest: stick.to_string_lossy().into(), whole_library: false, playlists: vec![road], remove_unselected: true };

        let r = sync(&conn, &p);
        assert_eq!((r.copied, r.unchanged, r.removed, r.playlists), (2, 0, 0, 1), "{r:?}");
        assert_eq!(fs::read(stick.join("Artist/Album/01 One.mp3")).unwrap().len(), 1000);
        let m3u = fs::read_to_string(stick.join("Road_ Trip.m3u8")).unwrap();
        assert!(m3u.contains("\nArtist/Album/01 One.mp3\n") && m3u.contains("Artist/Album/02 Two.mp3"), "{m3u}");

        // Nothing changed: nothing copied.
        let r = sync(&conn, &p);
        assert_eq!((r.copied, r.unchanged), (0, 2), "{r:?}");

        // A changed source file is copied again.
        let one = root.join("music/One.mp3");
        fs::write(&one, vec![9u8; 1500]).unwrap();
        conn.execute("UPDATE tracks SET mtime = ?2 WHERE id = ?1", rusqlite::params![ids[0], crate::meta::file_mtime(&one)]).unwrap();
        let r = sync(&conn, &p);
        assert_eq!((r.copied, r.unchanged), (1, 1), "{r:?}");
        assert_eq!(fs::read(stick.join("Artist/Album/01 One.mp3")).unwrap().len(), 1500);

        // A different playlist: the old files and playlist go, the user's file stays.
        p.playlists = vec![playlist(&mut conn, "Third", &ids[2..])];
        let r = sync(&conn, &p);
        assert_eq!((r.copied, r.removed, r.playlists), (1, 2, 1), "{r:?}");
        assert!(!stick.join("Artist/Album/01 One.mp3").exists());
        assert!(!stick.join("Road_ Trip.m3u8").exists());
        assert!(stick.join("Artist/Album/03 Three.mp3").exists());
        assert_eq!(fs::read_to_string(stick.join("my notes.txt")).unwrap(), "mine");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn never_overwrites_a_file_it_did_not_write() {
        let (root, conn, _) = setup("theirs");
        let stick = root.join("stick");
        fs::create_dir_all(stick.join("Artist/Album")).unwrap();
        fs::write(stick.join("Artist/Album/01 One.mp3"), "someone else's").unwrap();
        let p = Profile { dest: stick.to_string_lossy().into(), whole_library: true, playlists: vec![], remove_unselected: true };
        let r = sync(&conn, &p);
        assert_eq!(r.copied, 2);
        assert_eq!(r.failed.len(), 1, "{r:?}");
        assert_eq!(fs::read_to_string(stick.join("Artist/Album/01 One.mp3")).unwrap(), "someone else's");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cancelling_leaves_no_partial_files() {
        let (root, conn, _) = setup("cancel");
        let stick = root.join("stick");
        let p = Profile { dest: stick.to_string_lossy().into(), whole_library: true, playlists: vec![], remove_unselected: false };
        let cancel = AtomicBool::new(true);
        let resolve = |id| db::manual_playlist_track_ids(&conn, id);
        let r = run(&conn, &p, &resolve, &|_| {}, &cancel).unwrap();
        assert!(r.cancelled && r.copied == 0);
        let leftovers: Vec<_> = walkdir::WalkDir::new(&stick)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().to_string_lossy().ends_with(".part"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_tampered_manifest_cannot_reach_outside() {
        let (root, mut conn, ids) = setup("tampered");
        let stick = root.join("stick");
        let outside = root.join("outside.txt");
        let elsewhere = root.join("elsewhere.m3u8");
        fs::write(&outside, "keep me").unwrap();
        fs::write(&elsewhere, "keep me too").unwrap();
        // A manifest someone edited on the drive.
        let manifest = serde_json::json!({
            "files": {
                "../outside.txt": { "trackId": 1, "size": 7, "mtime": 0 },
                "Artist/../../outside.txt": { "trackId": 1, "size": 7, "mtime": 0 },
                "..\\outside.txt": { "trackId": 1, "size": 7, "mtime": 0 },
            },
            "playlists": [elsewhere.to_string_lossy(), "../elsewhere.m3u8"],
        });
        fs::write(stick.join(MANIFEST), manifest.to_string()).unwrap();
        let only = playlist(&mut conn, "Only", &ids[..1]);
        let p = Profile { dest: stick.to_string_lossy().into(), whole_library: false, playlists: vec![only], remove_unselected: true };
        let r = sync(&conn, &p);
        assert_eq!((r.copied, r.removed), (1, 0), "{r:?}");
        assert_eq!(fs::read_to_string(&outside).unwrap(), "keep me");
        assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "keep me too");
        let _ = fs::remove_dir_all(&root);
    }

    /// A folder on the drive that links elsewhere is never written through
    /// or deleted through.
    #[cfg(unix)]
    #[test]
    fn symlinked_folders_on_the_drive_are_not_followed() {
        let (root, conn, _) = setup("symlink");
        let stick = root.join("stick");
        let victim = root.join("victim");
        fs::create_dir_all(victim.join("Album")).unwrap();
        fs::write(victim.join("Album/01 One.mp3"), vec![0u8; 1000]).unwrap();
        std::os::unix::fs::symlink(&victim, stick.join("Artist")).unwrap();
        let manifest = serde_json::json!({
            "files": { "Artist/Album/01 One.mp3": { "trackId": 999, "size": 1000, "mtime": 0 } },
        });
        fs::write(stick.join(MANIFEST), manifest.to_string()).unwrap();
        let p = Profile { dest: stick.to_string_lossy().into(), whole_library: false, playlists: vec![], remove_unselected: true };
        // Nothing chosen: the manifest's file is "stale" and would be removed.
        let r = sync(&conn, &p);
        assert_eq!(r.removed, 0, "{r:?}");
        assert!(victim.join("Album/01 One.mp3").exists());
        // Syncing the library doesn't write through the link either.
        let p = Profile { whole_library: true, ..p };
        let r = sync(&conn, &p);
        assert!(r.failed.iter().any(|f| f.contains("outside the destination")), "{r:?}");
        assert_eq!(fs::read(victim.join("Album/01 One.mp3")).unwrap(), vec![0u8; 1000]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reads_plays_back_from_rockbox() {
        let (root, conn, ids) = setup("rockbox");
        let player = root.join("stick");
        fs::create_dir_all(player.join(".rockbox")).unwrap();
        let music = player.join("Music");
        fs::create_dir_all(&music).unwrap();
        let p = Profile { dest: music.to_string_lossy().into(), whole_library: true, playlists: vec![], remove_unselected: false };
        sync(&conn, &p);

        let log = player.join(".rockbox/playback.log");
        fs::write(
            &log,
            "# Rockbox playback log\n\
             1700000000:200000:200000:/Music/Artist/Album/01 One.mp3\n\
             1700000500:10000:200000:/Music/Artist/Album/02 Two.mp3\n\
             1700001000:150000:200000:/music/artist/album/01 one.mp3\n\
             1700002000:150000:200000:/Music/Other/Not synced.mp3\n",
        )
        .unwrap();
        let r = sync(&conn, &p);
        assert_eq!(r.plays_imported, 2, "skipped Two (10 s of 200) and the unknown file");
        let one = db::track_by_id(&conn, ids[0]).unwrap().unwrap();
        assert_eq!((one.play_count, one.last_played), (2, Some(1700001000)));
        assert_eq!(db::track_by_id(&conn, ids[1]).unwrap().unwrap().play_count, 0);

        // The same lines aren't counted twice; new ones are.
        let r = sync(&conn, &p);
        assert_eq!(r.plays_imported, 0);
        let mut f = fs::OpenOptions::new().append(true).open(&log).unwrap();
        writeln!(f, "1700003000:120000:200000:/Music/Artist/Album/02 Two.mp3").unwrap();
        let r = sync(&conn, &p);
        assert_eq!(r.plays_imported, 1);
        let _ = fs::remove_dir_all(&root);
    }
}
