//! Watches the library folders and starts a quick rescan a moment after
//! files in them change, so new music turns up without "Rescan now".

use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Manager};

use crate::{db, scanner, AppState};

/// How long the folders have to stay quiet before the rescan starts.
const SETTLE: Duration = Duration::from_secs(2);
/// A copy that keeps going still gets scanned this often.
const MAX_WAIT: Duration = Duration::from_secs(30);

static WATCHER: LazyLock<Mutex<Option<RecommendedWatcher>>> = LazyLock::new(Default::default);
static TX: LazyLock<Mutex<Option<mpsc::Sender<()>>>> = LazyLock::new(Default::default);
/// Folders that are there but couldn't be watched (on Linux, usually the
/// system's limit on watched folders), with the reason.
static FAILED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

pub fn failed() -> Vec<(String, String)> {
    FAILED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Start the rescan thread, then watch the folders.
pub fn start(app: AppHandle) {
    let (tx, rx) = mpsc::channel::<()>();
    *TX.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    let app2 = app.clone();
    std::thread::spawn(move || {
        while rx.recv().is_ok() {
            let first = Instant::now();
            loop {
                match rx.recv_timeout(SETTLE) {
                    Ok(()) if first.elapsed() < MAX_WAIT => continue,
                    Ok(()) | Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            let state = app2.state::<AppState>();
            scanner::spawn_scan(app2.clone(), state.db_path.clone(), state.scanning.clone());
        }
    });
    refresh(&app);
}

/// Watch the folders in the library now (after one is added or removed).
pub fn refresh(app: &AppHandle) {
    let folders = match db::open(&app.state::<AppState>().db_path).and_then(|c| db::folders(&c)) {
        Ok(f) => f,
        Err(e) => return eprintln!("watch: {e:#}"),
    };
    let mut slot = WATCHER.lock().unwrap_or_else(|e| e.into_inner());
    *slot = None;
    let mut failed = FAILED.lock().unwrap_or_else(|e| e.into_inner());
    failed.clear();
    if folders.is_empty() {
        return;
    }
    let watcher = notify::recommended_watcher(|res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        if matches!(ev.kind, EventKind::Access(_)) || ev.paths.iter().all(|p| hidden(p)) {
            return;
        }
        if let Some(tx) = TX.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = tx.send(());
        }
    });
    let mut watcher = match watcher {
        Ok(w) => w,
        Err(e) => return eprintln!("watch: {e}"),
    };
    for f in &folders {
        // A drive that isn't plugged in is skipped; the next scan says so.
        if let Err(e) = watcher.watch(Path::new(f), RecursiveMode::Recursive) {
            eprintln!("watch: {f}: {e}");
            if Path::new(f).is_dir() {
                failed.push((f.clone(), e.to_string()));
            }
        }
    }
    *slot = Some(watcher);
}

/// .DS_Store, ._ files and the like change on their own and never hold music.
fn hidden(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'))
}
