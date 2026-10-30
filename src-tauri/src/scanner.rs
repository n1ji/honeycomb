//! Background library scanning.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use anyhow::Result;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use walkdir::WalkDir;

use crate::{db, meta};

/// Rows written per transaction.
const BATCH: usize = 200;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Progress {
    done: usize,
    total: usize,
    current: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Finished {
    pub added_or_updated: usize,
    pub removed: usize,
    pub errors: usize,
    pub inboxed: usize,
}

/// Scan every library folder on a background thread. Unchanged files (same
/// mtime) are skipped; files that disappeared are removed from the library.
pub fn spawn_scan(app: AppHandle, db_path: PathBuf, scanning: Arc<AtomicBool>) {
    if scanning.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let progress = |p: Progress| {
            let _ = app.emit("scan-progress", p);
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scan(&db_path, &progress)));
        scanning.store(false, Ordering::SeqCst);
        match result {
            Ok(Ok(f)) => {
                let _ = app.emit("scan-finished", f);
            }
            Ok(Err(e)) => {
                let _ = app.emit("scan-error", e.to_string());
            }
            Err(_) => {
                let _ = app.emit("scan-error", "the scanner crashed".to_string());
            }
        }
    });
}

pub(crate) fn scan(db_path: &Path, progress: &dyn Fn(Progress)) -> Result<Finished> {
    let started = std::time::Instant::now();
    let conn = db::open(db_path)?;
    let folders = db::folders(&conn)?;
    let established: Vec<PathBuf> = db::scanned_folders(&conn)?.into_iter().map(PathBuf::from).collect();

    let files: Vec<PathBuf> = folders
        .iter()
        .flat_map(|f| WalkDir::new(f).follow_links(true).into_iter().filter_map(|e| e.ok()))
        .filter(|e| e.file_type().is_file() && meta::is_audio(e.path()))
        .map(|e| e.into_path())
        .collect();

    let known: HashMap<String, i64> = {
        let mut stmt = conn.prepare("SELECT path, mtime FROM tracks")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };

    let walked = started.elapsed();
    let total = files.len();
    let seen: HashSet<String> = files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let todo: Vec<&PathBuf> = files
        .iter()
        .filter(|p| known.get(p.to_string_lossy().as_ref()) != Some(&meta::file_mtime(p)))
        .collect();

    // Reading tags is the slow part, so it runs on several threads while this
    // thread writes results to the database in batches.
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
    let next = AtomicUsize::new(0);
    let dir_art: Mutex<HashMap<PathBuf, bool>> = Mutex::new(HashMap::new());
    let mut changed = 0;
    let mut errors = 0;
    let mut inboxed = 0;
    // Rows first inserted by this scan, and whether they went to the inbox.
    let mut fresh: Vec<(i64, bool)> = Vec::new();

    std::thread::scope(|s| -> Result<()> {
        let (tx, rx) = mpsc::sync_channel(256);
        for _ in 0..workers {
            let tx = tx.clone();
            let (todo, next, dir_art) = (&todo, &next, &dir_art);
            s.spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = todo.get(i) else { break };
                    let has_folder_art = |p: &Path| {
                        let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
                        if let Some(&v) = dir_art.lock().unwrap().get(&dir) {
                            return v;
                        }
                        let v = meta::folder_art(p).is_some();
                        dir_art.lock().unwrap().insert(dir, v);
                        v
                    };
                    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        meta::read_track_with(path, &has_folder_art)
                    }));
                    let result = match read {
                        Ok(r) => r.ok(),
                        Err(_) => None,
                    };
                    if tx.send((path.to_string_lossy().into_owned(), result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);

        let mut write_time = std::time::Duration::ZERO;
        let mut done = total - todo.len();
        let mut in_batch = 0;
        conn.execute_batch("BEGIN")?;
        for (path, track) in rx {
            let w = std::time::Instant::now();
            match track {
                Some(t) => {
                    let id = db::upsert_track(&conn, &t)?;
                    let is_new = !known.contains_key(&t.path);
                    let to_inbox = is_new && established.iter().any(|f| Path::new(&t.path).starts_with(f));
                    if to_inbox {
                        conn.execute("UPDATE tracks SET inbox = 1 WHERE id = ?1", [id])?;
                        inboxed += 1;
                    }
                    if is_new {
                        fresh.push((id, to_inbox));
                    }
                    changed += 1;
                }
                None => errors += 1,
            }
            done += 1;
            in_batch += 1;
            if in_batch >= BATCH {
                conn.execute_batch("COMMIT; BEGIN")?;
                in_batch = 0;
            }
            write_time += w.elapsed();
            if done % 25 == 0 || done == total {
                progress(Progress { done, total, current: path });
            }
        }
        conn.execute_batch("COMMIT")?;
        eprintln!(
            "scan: walk {:?}, {} files to read, {} workers, db writes {:?}, total {:?}",
            walked, todo.len(), workers, write_time, started.elapsed()
        );
        Ok(())
    })?;

    // Drop tracks whose files are gone or no longer under a library folder.
    let mut stale: Vec<&String> = known.keys().filter(|p| !seen.contains(*p)).collect();
    conn.execute_batch("BEGIN")?;
    let moved = find_moves(&conn, &stale, &fresh)?;
    for (old, _, new, new_inboxed) in &moved {
        // The old row keeps its id, stats, inbox state and playlist spots and
        // takes the new file's path and tags; the duplicate row goes.
        let Some(t) = db::track_by_id(&conn, *new)? else { continue };
        conn.execute("DELETE FROM tracks WHERE id = ?1", [new])?;
        conn.execute(
            "UPDATE tracks SET path = ?2, title = ?3, artist = ?4, album_artist = ?5, album = ?6,
                genre = ?7, composer = ?8, comment = ?9, year = ?10, track_no = ?11, disc_no = ?12,
                duration = ?13, bitrate = ?14, sample_rate = ?15, format = ?16, mtime = ?17, has_art = ?18,
                bit_depth = ?19
             WHERE id = ?1",
            rusqlite::params![
                old, t.path, t.title, t.artist, t.album_artist, t.album, t.genre, t.composer, t.comment,
                t.year, t.track_no, t.disc_no, t.duration, t.bitrate, t.sample_rate, t.format, t.mtime,
                t.has_art, t.bit_depth
            ],
        )?;
        if *new_inboxed {
            inboxed -= 1;
        }
    }
    let moved_from: HashSet<&String> = moved.iter().map(|m| &m.1).collect();
    stale.retain(|p| !moved_from.contains(p));
    for p in &stale {
        conn.execute("DELETE FROM tracks WHERE path = ?1", [p])?;
    }
    conn.execute("UPDATE folders SET scanned = 1", [])?;
    conn.execute_batch("COMMIT")?;

    Ok(Finished { added_or_updated: changed, removed: stale.len(), errors, inboxed })
}

/// Pair tracks whose files vanished with tracks first found in this scan that
/// look like the same file moved: same title, artist, album and audio format
/// (there's no file size in the library, so format, bitrate and sample rate
/// stand in for it) and a duration within half a second. Only one-to-one pairs
/// count. Returns (old id, old path, new id, new row went to the inbox).
fn find_moves(
    conn: &rusqlite::Connection,
    stale: &[&String],
    fresh: &[(i64, bool)],
) -> Result<Vec<(i64, String, i64, bool)>> {
    if stale.is_empty() || fresh.is_empty() {
        return Ok(Vec::new());
    }
    type Key = (String, String, String, String, Option<u32>, Option<u32>);
    let key = |t: &crate::models::Track| -> Key {
        (t.title.clone(), t.artist.clone(), t.album.clone(), t.format.clone(), t.bitrate, t.sample_rate)
    };
    let mut new_by_key: HashMap<Key, Vec<(crate::models::Track, bool)>> = HashMap::new();
    for t in db::tracks_by_ids(conn, &fresh.iter().map(|f| f.0).collect::<Vec<_>>())? {
        let inboxed = fresh.iter().any(|f| f.0 == t.id && f.1);
        new_by_key.entry(key(&t)).or_default().push((t, inboxed));
    }
    let mut old = Vec::new();
    for p in stale {
        if let Some(id) = db::track_id_by_path(conn, p)? {
            old.extend(db::track_by_id(conn, id)?);
        }
    }
    let close = |a: &crate::models::Track, b: &crate::models::Track| (a.duration - b.duration).abs() <= 0.5;
    let mut pairs = Vec::new();
    for o in &old {
        let Some(group) = new_by_key.get(&key(o)) else { continue };
        let mut hits = group.iter().filter(|(n, _)| close(o, n));
        let (Some((n, inboxed)), None) = (hits.next(), hits.next()) else { continue };
        let rivals = old.iter().filter(|o2| key(o2) == key(o) && close(o2, n)).count();
        if rivals == 1 {
            pairs.push((o.id, o.path.clone(), n.id, *inboxed));
        }
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny valid 16-bit mono WAV (0.1 s of silence).
    fn write_wav(path: &Path) {
        let samples = 4410u32;
        let data_len = samples * 2;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&1u16.to_le_bytes()); // mono
        b.extend_from_slice(&44100u32.to_le_bytes());
        b.extend_from_slice(&(44100u32 * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        b.resize(b.len() + data_len as usize, 0);
        std::fs::write(path, b).unwrap();
    }

    fn inbox_names(conn: &rusqlite::Connection) -> Vec<String> {
        db::all_tracks(conn)
            .unwrap()
            .into_iter()
            .filter(|t| t.inbox)
            .map(|t| Path::new(&t.path).file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn new_files_in_scanned_folders_go_to_inbox() {
        let root = std::env::temp_dir().join(format!("hc-inbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (music, more) = (root.join("music"), root.join("more"));
        std::fs::create_dir_all(&music).unwrap();
        std::fs::create_dir_all(&more).unwrap();
        write_wav(&music.join("old.wav"));
        let db_path = root.join("library.db");
        let conn = db::open(&db_path).unwrap();
        db::migrate(&conn).unwrap();
        let add = |f: &Path| {
            conn.execute("INSERT INTO folders (path) VALUES (?1)", [f.to_string_lossy()]).unwrap();
        };
        let quiet = |_: Progress| {};

        // A folder's first scan fills the library but not the inbox.
        add(&music);
        let f = scan(&db_path, &quiet).unwrap();
        assert_eq!((f.added_or_updated, f.inboxed), (1, 0));

        // A file that turns up later lands in the inbox; a newly added folder doesn't.
        write_wav(&music.join("new.wav"));
        write_wav(&more.join("other.wav"));
        add(&more);
        let f = scan(&db_path, &quiet).unwrap();
        assert_eq!((f.added_or_updated, f.inboxed), (2, 1));
        assert_eq!(inbox_names(&conn), ["new.wav"]);

        // Rescanning leaves it there; nothing else is added.
        let f = scan(&db_path, &quiet).unwrap();
        assert_eq!((f.added_or_updated, f.inboxed), (0, 0));
        assert_eq!(inbox_names(&conn), ["new.wav"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn moved_files_keep_their_row() {
        let root = std::env::temp_dir().join(format!("hc-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let music = root.join("music");
        std::fs::create_dir_all(&music).unwrap();
        let before = music.join("song.wav");
        write_wav(&before);
        let tags = crate::models::TagEdit {
            title: Some("Song".into()),
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            ..Default::default()
        };
        meta::write_tags(&before, &tags).unwrap();
        let db_path = root.join("library.db");
        let conn = db::open(&db_path).unwrap();
        db::migrate(&conn).unwrap();
        conn.execute("INSERT INTO folders (path) VALUES (?1)", [music.to_string_lossy()]).unwrap();
        let quiet = |_: Progress| {};
        scan(&db_path, &quiet).unwrap();

        let id = db::track_id_by_path(&conn, &before.to_string_lossy()).unwrap().unwrap();
        conn.execute("UPDATE tracks SET rating = 7, loved = 1 WHERE id = ?1", [id]).unwrap();
        db::record_play(&conn, id, 1_700_000_000).unwrap();
        conn.execute("INSERT INTO playlists (name, created) VALUES ('Mix', 0)", []).unwrap();
        let pl = conn.last_insert_rowid();
        conn.execute("INSERT INTO playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, 0)", [pl, id])
            .unwrap();
        let old = db::track_by_id(&conn, id).unwrap().unwrap();

        // Renamed and moved into a subfolder: same row, new path, nothing in the inbox.
        std::fs::create_dir_all(music.join("sub")).unwrap();
        let after = music.join("sub").join("renamed.wav");
        std::fs::rename(&before, &after).unwrap();
        let f = scan(&db_path, &quiet).unwrap();
        assert_eq!((f.removed, f.inboxed), (0, 0));
        let tracks = db::all_tracks(&conn).unwrap();
        assert_eq!(tracks.len(), 1);
        let t = &tracks[0];
        assert_eq!(t.id, id);
        assert_eq!(t.path, after.to_string_lossy());
        assert_eq!(
            (t.rating, t.loved, t.play_count, t.last_played, t.date_added, t.inbox),
            (7, true, 1, Some(1_700_000_000), old.date_added, false)
        );
        assert_eq!(db::manual_playlist_track_ids(&conn, pl).unwrap(), [id]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
