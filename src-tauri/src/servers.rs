//! Server sources in the app: the commands behind Settings → Library, syncs
//! with progress events, and a worker that sends changes and downloads
//! offline copies in the background.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::mirror::{self, RemoveReport, SourceRow};
use crate::{AppState, CmdResult, db, err};

static SYNCING: LazyLock<Mutex<HashSet<i64>>> = LazyLock::new(Default::default);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncEvent {
    id: i64,
    songs: usize,
    done: bool,
    error: Option<String>,
    report: Option<mirror::SyncReport>,
}

#[tauri::command]
pub fn servers(state: State<AppState>) -> CmdResult<Vec<SourceRow>> {
    mirror::sources(&state.conn()).map_err(err)
}

/// Check and save a server, then mirror it in the background.
#[tauri::command]
pub async fn server_add(app: AppHandle, url: String, username: String, password: String) -> CmdResult<i64> {
    let app2 = app.clone();
    let id = tauri::async_runtime::spawn_blocking(move || {
        let conn = db::open(&app2.state::<AppState>().db_path).map_err(err)?;
        mirror::add_source(&conn, &url, username.trim(), &password).map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(err)??;
    spawn_sync(app, id);
    Ok(id)
}

#[tauri::command]
pub fn server_sync(app: AppHandle, id: i64) {
    spawn_sync(app, id);
}

/// Forget a server. With `keep`, its offline copies move to
/// Music/Honeycomb/<server> and stay in the library as files.
#[tauri::command]
pub async fn server_remove(app: AppHandle, id: i64, keep: bool) -> CmdResult<RemoveReport> {
    let app2 = app.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        let state = app2.state::<AppState>();
        let mut conn = db::open(&state.db_path).map_err(err)?;
        let name = mirror::sources(&conn).map_err(err)?.into_iter().find(|s| s.id == id).map(|s| s.name).ok_or("no such server")?;
        let dir = match keep {
            true => {
                let music = app2.path().audio_dir().or_else(|_| app2.path().home_dir().map(|h| h.join("Music"))).map_err(err)?;
                Some(music.join("Honeycomb").join(name.replace(['/', '\\', ':'], "_")))
            }
            false => None,
        };
        mirror::remove_source(&mut conn, id, dir.as_deref()).map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(err)??;
    let _ = app.emit("library-changed", ());
    Ok(report)
}

#[derive(Serialize)]
pub struct States {
    ids: Vec<i64>,
    states: Vec<String>,
}

#[tauri::command]
pub fn remote_states(state: State<AppState>) -> CmdResult<States> {
    let (ids, states) = mirror::states(&state.conn()).map_err(err)?;
    Ok(States { ids, states })
}

#[tauri::command]
pub fn remote_pin(app: AppHandle, state: State<AppState>, ids: Vec<i64>, albums: bool) -> CmdResult<usize> {
    let n = mirror::pin(&state.conn(), &ids, albums).map_err(err)?;
    let _ = app.emit("remote-states", ());
    Ok(n)
}

#[tauri::command]
pub fn remote_unpin(app: AppHandle, state: State<AppState>, ids: Vec<i64>) -> CmdResult<()> {
    mirror::unpin(&state.conn(), &ids).map_err(err)?;
    let _ = app.emit("library-changed", ());
    Ok(())
}

fn spawn_sync(app: AppHandle, id: i64) {
    if !SYNCING.lock().unwrap_or_else(|e| e.into_inner()).insert(id) {
        return;
    }
    std::thread::spawn(move || {
        let ev = |songs, done, error: Option<String>, report| {
            let _ = app.emit("server-sync", SyncEvent { id, songs, done, error, report });
        };
        ev(0, false, None, None);
        let result = (|| -> anyhow::Result<mirror::SyncReport> {
            let mut conn = db::open(&app.state::<AppState>().db_path)?;
            mirror::flush(&conn)?;
            let client = mirror::client(&conn, id)?;
            let pulled = mirror::pull(&client, &|n| ev(n, false, None, None))?;
            mirror::apply(&mut conn, id, &pulled)
        })();
        SYNCING.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        match result {
            Ok(r) => {
                ev(r.tracks, true, None, Some(r));
                let _ = app.emit("library-changed", ());
                mirror::poke();
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if let Ok(conn) = db::open(&app.state::<AppState>().db_path) {
                    let _ = mirror::set_error(&conn, id, Some(&msg));
                }
                ev(0, true, Some(msg), None);
            }
        }
    });
}

/// Sends waiting changes and downloads offline copies, when poked and once a minute.
pub fn spawn_worker(app: AppHandle) {
    std::thread::spawn(move || {
        let Ok(conn) = db::open(&app.state::<AppState>().db_path) else { return };
        loop {
            match mirror::flush(&conn) {
                Ok(n) if n > 0 => {
                    let _ = app.emit("servers-changed", ());
                }
                Err(e) => eprintln!("servers: sending changes: {e:#}"),
                _ => {}
            }
            let mut downloaded = 0;
            loop {
                match mirror::download_next(&conn) {
                    Ok(Some(_)) => {
                        downloaded += 1;
                        if downloaded % 5 == 1 {
                            let _ = app.emit("remote-states", ());
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        eprintln!("servers: offline copy: {e:#}");
                        break;
                    }
                }
            }
            if downloaded > 0 {
                let _ = app.emit("remote-states", ());
            }
            mirror::wait_for_work(Duration::from_secs(60));
        }
    });
}
