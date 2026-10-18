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
struct Progress {
    done: usize,
    total: usize,
    current: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Finished {
    added_or_updated: usize,
    removed: usize,
    errors: usize,
}

/// Scan every library folder on a background thread. Unchanged files (same
/// mtime) are skipped; files that disappeared are removed from the library.
pub fn spawn_scan(app: AppHandle, db_path: PathBuf, scanning: Arc<AtomicBool>) {
    if scanning.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scan(&app, &db_path)));
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

fn scan(app: &AppHandle, db_path: &Path) -> Result<Finished> {
    let started = std::time::Instant::now();
    let conn = db::open(db_path)?;
    let folders = db::folders(&conn)?;

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
                    db::upsert_track(&conn, &t)?;
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
                let _ = app.emit("scan-progress", Progress { done, total, current: path });
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
    let stale: Vec<&String> = known.keys().filter(|p| !seen.contains(*p)).collect();
    conn.execute_batch("BEGIN")?;
    for p in &stale {
        conn.execute("DELETE FROM tracks WHERE path = ?1", [p])?;
    }
    conn.execute_batch("COMMIT")?;

    Ok(Finished { added_or_updated: changed, removed: stale.len(), errors })
}
