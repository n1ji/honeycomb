mod convert;
mod db;
mod importer;
mod kaseki;
mod lyrics;
mod matching;
mod media;
mod meta;
mod models;
mod net;
mod player;
mod plugins;
mod scanner;
mod secrets;
mod smart;
mod smoke;
mod sync;
mod version;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use tauri::http::{Response, StatusCode, header};
use tauri::{AppHandle, Emitter, Manager, State};

use models::{Playlist, SmartRules, TagEdit, Track, TrackTable, now_secs};
use player::{Cmd, PlayerHandle, QItem, Repeat};

pub struct AppState {
    db: Mutex<Connection>,
    db_path: PathBuf,
    player: PlayerHandle,
    scanning: Arc<AtomicBool>,
    /// Set to stop a running sync.
    sync_cancel: Arc<AtomicBool>,
    plugins: plugins::Dirs,
}

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

impl AppState {
    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn queue_items(&self, ids: &[i64]) -> CmdResult<Vec<QItem>> {
        let conn = self.conn();
        Ok(db::tracks_by_ids(&conn, ids)
            .map_err(err)?
            .into_iter()
            .map(|t| QItem { uid: 0, track_id: t.id, path: t.path, duration: t.duration })
            .collect())
    }
}

// ---------- library ----------

#[tauri::command]
fn get_tracks(state: State<AppState>) -> CmdResult<TrackTable> {
    let t = std::time::Instant::now();
    let tracks = db::all_tracks(&state.conn()).map_err(err)?;
    if perf_enabled() {
        eprintln!("perf: get_tracks query {} tracks in {:.1} ms", tracks.len(), t.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(TrackTable(tracks))
}

#[tauri::command]
fn get_folders(state: State<AppState>) -> CmdResult<Vec<String>> {
    db::folders(&state.conn()).map_err(err)
}

#[tauri::command]
fn add_folder(app: AppHandle, state: State<AppState>, path: String) -> CmdResult<()> {
    state
        .conn()
        .execute("INSERT OR IGNORE INTO folders (path) VALUES (?1)", [&path])
        .map_err(err)?;
    scanner::spawn_scan(app, state.db_path.clone(), state.scanning.clone());
    Ok(())
}

#[tauri::command]
fn remove_folder(app: AppHandle, state: State<AppState>, path: String) -> CmdResult<()> {
    state.conn().execute("DELETE FROM folders WHERE path = ?1", [&path]).map_err(err)?;
    // A rescan drops tracks that are no longer under any folder.
    scanner::spawn_scan(app, state.db_path.clone(), state.scanning.clone());
    Ok(())
}

#[tauri::command]
fn rescan(app: AppHandle, state: State<AppState>) {
    scanner::spawn_scan(app, state.db_path.clone(), state.scanning.clone());
}

#[tauri::command]
fn set_rating(state: State<AppState>, ids: Vec<i64>, rating: u8) -> CmdResult<()> {
    let conn = state.conn();
    for id in ids {
        conn.execute("UPDATE tracks SET rating = ?2 WHERE id = ?1", rusqlite::params![id, rating.min(10)])
            .map_err(err)?;
    }
    Ok(())
}

/// Take tracks out of the inbox (all of them when `ids` is None).
#[tauri::command]
fn clear_inbox(state: State<AppState>, ids: Option<Vec<i64>>) -> CmdResult<()> {
    let conn = state.conn();
    match ids {
        None => conn.execute("UPDATE tracks SET inbox = 0 WHERE inbox = 1", []).map(|_| ()),
        Some(ids) => ids
            .iter()
            .try_for_each(|id| conn.execute("UPDATE tracks SET inbox = 0 WHERE id = ?1", [id]).map(|_| ())),
    }
    .map_err(err)
}

#[tauri::command]
fn edit_tags(state: State<AppState>, ids: Vec<i64>, edit: TagEdit) -> CmdResult<Vec<Track>> {
    let tracks = db::tracks_by_ids(&state.conn(), &ids).map_err(err)?;
    let mut updated = Vec::new();
    let mut failures = Vec::new();
    for t in tracks {
        let path = Path::new(&t.path);
        match meta::write_tags(path, &edit).and_then(|_| meta::read_track(path)) {
            Ok(fresh) => {
                let conn = state.conn();
                db::upsert_track(&conn, &fresh).map_err(err)?;
                if let Some(t) = db::track_by_id(&conn, t.id).map_err(err)? {
                    updated.push(t);
                }
            }
            Err(e) => failures.push(format!("{}: {e:#}", t.path)),
        }
    }
    if !failures.is_empty() && updated.is_empty() {
        return Err(failures.join("\n"));
    }
    Ok(updated)
}

// ---------- settings ----------

/// All settings but secrets (session keys, tokens), which stay in the backend.
#[tauri::command]
fn get_settings(state: State<AppState>) -> CmdResult<HashMap<String, String>> {
    Ok(db::settings(&state.conn()).map_err(err)?.into_iter().filter(|(k, _)| !k.starts_with("secret.")).collect())
}

#[tauri::command]
fn set_setting(state: State<AppState>, key: String, value: String) -> CmdResult<()> {
    if key.starts_with("secret.") {
        return Err("not a setting the UI can change".into());
    }
    db::set_setting(&state.conn(), &key, &value).map_err(err)
}

// ---------- playlists ----------

#[tauri::command]
fn get_playlists(state: State<AppState>) -> CmdResult<Vec<Playlist>> {
    db::playlists(&state.conn()).map_err(err)
}

#[tauri::command]
fn create_playlist(
    state: State<AppState>,
    name: String,
    rules: Option<SmartRules>,
    track_ids: Option<Vec<i64>>,
) -> CmdResult<Playlist> {
    let mut conn = state.conn();
    let kind = if rules.is_some() { "smart" } else { "manual" };
    let rules_json = rules.as_ref().map(serde_json::to_string).transpose().map_err(err)?;
    conn.execute(
        "INSERT INTO playlists (name, kind, rules, created) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![name, kind, rules_json, now_secs()],
    )
    .map_err(err)?;
    let id = conn.last_insert_rowid();
    if let Some(ids) = track_ids {
        db::set_playlist_track_ids(&mut conn, id, &ids).map_err(err)?;
    }
    Ok(Playlist { id, name, kind: kind.into(), rules })
}

#[tauri::command]
fn update_playlist(
    state: State<AppState>,
    id: i64,
    name: String,
    rules: Option<SmartRules>,
) -> CmdResult<()> {
    let rules_json = rules.as_ref().map(serde_json::to_string).transpose().map_err(err)?;
    state
        .conn()
        .execute(
            "UPDATE playlists SET name = ?2, rules = COALESCE(?3, rules) WHERE id = ?1",
            rusqlite::params![id, name, rules_json],
        )
        .map_err(err)?;
    Ok(())
}

#[tauri::command]
fn delete_playlist(state: State<AppState>, id: i64) -> CmdResult<()> {
    state.conn().execute("DELETE FROM playlists WHERE id = ?1", [id]).map_err(err)?;
    Ok(())
}

fn resolve_playlist(conn: &Connection, id: i64) -> CmdResult<Vec<i64>> {
    let pl = db::playlist(conn, id).map_err(err)?.ok_or("playlist not found")?;
    match (&pl.kind[..], &pl.rules) {
        ("smart", Some(rules)) => smart::evaluate(conn, rules).map_err(err),
        _ => db::manual_playlist_track_ids(conn, id).map_err(err),
    }
}

#[tauri::command]
fn playlist_track_ids(state: State<AppState>, id: i64) -> CmdResult<Vec<i64>> {
    resolve_playlist(&state.conn(), id)
}

#[tauri::command]
fn preview_smart(state: State<AppState>, rules: SmartRules) -> CmdResult<usize> {
    smart::evaluate(&state.conn(), &rules).map(|v| v.len()).map_err(err)
}

#[tauri::command]
fn set_playlist_tracks(state: State<AppState>, id: i64, track_ids: Vec<i64>) -> CmdResult<()> {
    db::set_playlist_track_ids(&mut state.conn(), id, &track_ids).map_err(err)
}

#[tauri::command]
fn add_to_playlist(state: State<AppState>, id: i64, track_ids: Vec<i64>) -> CmdResult<()> {
    let mut conn = state.conn();
    let mut ids = db::manual_playlist_track_ids(&conn, id).map_err(err)?;
    ids.extend(track_ids);
    db::set_playlist_track_ids(&mut conn, id, &ids).map_err(err)
}

#[tauri::command]
fn export_m3u(state: State<AppState>, id: i64, path: String) -> CmdResult<()> {
    let conn = state.conn();
    let ids = resolve_playlist(&conn, id)?;
    let tracks = db::tracks_by_ids(&conn, &ids).map_err(err)?;
    let mut out = String::from("#EXTM3U\n");
    for t in tracks {
        out.push_str(&format!(
            "#EXTINF:{},{} - {}\n{}\n",
            t.duration.round() as i64,
            t.artist,
            t.title,
            t.path
        ));
    }
    std::fs::write(&path, out).map_err(err)
}

/// Import an .m3u/.m3u8 as a new manual playlist. Entries that aren't in the
/// library yet are read and added.
#[tauri::command]
fn import_m3u(state: State<AppState>, path: String) -> CmdResult<Playlist> {
    let bytes = std::fs::read(&path).map_err(err)?;
    let text = String::from_utf8_lossy(&bytes);
    let base = Path::new(&path).parent().map(Path::to_path_buf).unwrap_or_default();
    let mut ids = Vec::new();
    {
        let conn = state.conn();
        // Paths written on another machine fall back to matching by their tail.
        let matcher = importer::Matcher::new(&db::all_tracks(&conn).map_err(err)?);
        for line in text.lines() {
            let line = line.trim().trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with('#') || importer::is_placeholder(line) {
                continue;
            }
            let p = PathBuf::from(line.replace(['\\', '/'], std::path::MAIN_SEPARATOR_STR));
            let p = if p.is_absolute() { p } else { base.join(p) };
            let key = p.to_string_lossy().into_owned();
            let id = match db::track_id_by_path(&conn, &key).map_err(err)? {
                Some(id) => Some(id),
                None if p.is_file() && meta::is_audio(&p) => match meta::read_track(&p) {
                    Ok(t) => Some(db::upsert_track(&conn, &t).map_err(err)?),
                    Err(_) => None,
                },
                None => matcher.find(line, None),
            };
            ids.extend(id);
        }
    }
    let name = Path::new(&path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Imported")
        .to_string();
    create_playlist(state, name, None, Some(ids))
}

// ---------- import from another player ----------

fn load_import_source(xml_path: &Option<String>, m3u_paths: &[String]) -> CmdResult<importer::Source> {
    let mut src = match xml_path {
        Some(p) => importer::read_library_xml(Path::new(p)).map_err(|e| format!("{e:#}"))?,
        None => importer::Source::default(),
    };
    for f in importer::expand_m3u_paths(m3u_paths) {
        src.playlists.push(importer::read_m3u(&f).map_err(|e| format!("{e:#}"))?);
    }
    Ok(src)
}

#[tauri::command]
async fn import_preview(
    state: State<'_, AppState>,
    xml_path: Option<String>,
    m3u_paths: Vec<String>,
) -> CmdResult<importer::Preview> {
    let src = load_import_source(&xml_path, &m3u_paths)?;
    importer::preview(&state.conn(), &src, xml_path.is_some()).map_err(err)
}

#[tauri::command]
async fn import_apply(
    state: State<'_, AppState>,
    xml_path: Option<String>,
    m3u_paths: Vec<String>,
    import_stats: bool,
    playlists: Vec<usize>,
) -> CmdResult<importer::Outcome> {
    let src = load_import_source(&xml_path, &m3u_paths)?;
    importer::apply(&mut state.conn(), &src, import_stats && xml_path.is_some(), &playlists).map_err(err)
}

#[tauri::command]
fn save_text(path: String, text: String) -> CmdResult<()> {
    std::fs::write(path, text).map_err(err)
}

// ---------- artwork ----------

/// File extension for a track's cover ("jpg", "png"…), or None without one.
#[tauri::command]
async fn artwork_ext(state: State<'_, AppState>, track_id: i64) -> CmdResult<Option<String>> {
    let path = db::track_by_id(&state.conn(), track_id).map_err(err)?.ok_or("track not found")?.path;
    Ok(meta::artwork(Path::new(&path)).map(|(_, mime)| mime.rsplit('/').next().unwrap_or("jpg").replace("jpeg", "jpg")))
}

/// Save a track's cover, as stored (no re-encoding), to `dest`.
#[tauri::command]
async fn save_artwork(state: State<'_, AppState>, track_id: i64, dest: String) -> CmdResult<()> {
    let path = db::track_by_id(&state.conn(), track_id).map_err(err)?.ok_or("track not found")?.path;
    let (data, _) = meta::artwork(Path::new(&path)).ok_or("this track has no artwork")?;
    std::fs::write(dest, data).map_err(err)
}

// ---------- lyrics ----------

#[tauri::command]
async fn get_lyrics(app: AppHandle, track_id: i64) -> CmdResult<lyrics::Lyrics> {
    // File reads and network lookups run off the async workers.
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = db::open(&state.db_path).map_err(err)?;
        let track = db::track_by_id(&conn, track_id).map_err(err)?.ok_or("track not found")?;
        let online = db::settings(&conn)
            .map_err(err)?
            .into_iter()
            .find(|(k, _)| k == "lyricsOnline")
            .is_none_or(|(_, v)| v != "off");
        Ok(lyrics::get(&conn, &track, online))
    })
    .await
    .map_err(err)?
}

// ---------- sync to a device ----------

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncTarget {
    exists: bool,
    rockbox: bool,
    free_bytes: Option<u64>,
    /// Tracks a previous sync put there.
    synced_files: usize,
}

#[tauri::command]
fn sync_target(dest: String) -> SyncTarget {
    let dest = Path::new(&dest);
    let synced_files = std::fs::read(dest.join(sync::MANIFEST))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("files").and_then(|f| f.as_object()).map(|f| f.len()))
        .unwrap_or(0);
    SyncTarget {
        exists: dest.is_dir(),
        rockbox: sync::rockbox_root(dest).is_some(),
        free_bytes: fs2::available_space(dest).ok(),
        synced_files,
    }
}

#[tauri::command]
async fn sync_run(app: AppHandle, profile: sync::Profile) -> CmdResult<sync::Report> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.sync_cancel.store(false, std::sync::atomic::Ordering::SeqCst);
        let conn = db::open(&state.db_path).map_err(err)?;
        let resolve = |id| resolve_playlist(&conn, id).map_err(anyhow::Error::msg);
        let progress = |p: sync::Progress| {
            let _ = tauri::Emitter::emit(&app, "sync-progress", p);
        };
        sync::run(&conn, &profile, &resolve, &progress, &state.sync_cancel).map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(err)?
}

/// Which conversion formats this system can make (AAC needs the system's encoder).
#[tauri::command]
fn sync_formats() -> serde_json::Value {
    serde_json::json!({ "aac": convert::aac_available() })
}

#[tauri::command]
fn sync_cancel(state: State<AppState>) {
    state.sync_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
}

// ---------- plugins ----------

#[tauri::command]
fn plugin_list(state: State<AppState>) -> Vec<plugins::Listed> {
    state.plugins.list()
}

#[tauri::command]
fn plugin_folder(state: State<AppState>) -> String {
    state.plugins.user.to_string_lossy().into_owned()
}

fn plugin_manifest(app: &AppHandle, plugin: &str) -> CmdResult<plugins::Manifest> {
    app.state::<AppState>().plugins.manifest(plugin).map_err(|e| format!("{plugin}: {e:#}"))
}

#[tauri::command]
async fn plugin_http(app: AppHandle, plugin: String, request: plugins::HttpRequest) -> CmdResult<plugins::HttpResponse> {
    let m = plugin_manifest(&app, &plugin)?;
    tauri::async_runtime::spawn_blocking(move || plugins::http(&m, &request).map_err(|e| format!("{e:#}")))
        .await
        .map_err(err)?
}

/// Waits for the browser to come back to `http://127.0.0.1:<port><path>`; returns the query string.
#[tauri::command]
async fn plugin_oauth_redirect(app: AppHandle, plugin: String, port: u16, path: String, timeout_secs: Option<u64>) -> CmdResult<String> {
    let m = plugin_manifest(&app, &plugin)?;
    let timeout = std::time::Duration::from_secs(timeout_secs.unwrap_or(300).min(900));
    tauri::async_runtime::spawn_blocking(move || plugins::wait_for_redirect(&m, port, &path, timeout).map_err(|e| format!("{e:#}")))
        .await
        .map_err(err)?
}

#[tauri::command]
fn plugin_secret_get(app: AppHandle, plugin: String, key: String) -> CmdResult<Option<String>> {
    plugin_manifest(&app, &plugin)?;
    Ok(plugins::secret_get(&app.state::<AppState>().conn(), &plugin, &key))
}

/// A null value deletes the secret.
#[tauri::command]
fn plugin_secret_set(app: AppHandle, plugin: String, key: String, value: Option<String>) -> CmdResult<()> {
    plugin_manifest(&app, &plugin)?;
    plugins::secret_set(&app.state::<AppState>().conn(), &plugin, &key, value.as_deref())
}

/// Library track ids for tracks described by another service (null where none matched).
#[tauri::command]
async fn match_tracks(app: AppHandle, candidates: Vec<matching::Candidate>) -> CmdResult<Vec<Option<i64>>> {
    tauri::async_runtime::spawn_blocking(move || {
        let index = matching::LibraryIndex::new(&db::all_tracks(&app.state::<AppState>().conn()).map_err(err)?);
        Ok(candidates.iter().map(|c| index.find(c)).collect())
    })
    .await
    .map_err(err)?
}

// ---------- updates ----------

#[tauri::command]
async fn update_status(refresh: bool) -> version::Status {
    version::status(refresh)
}

/// Install the newest release over this one, then restart. Sends
/// `update-progress` {done, total} while downloading.
#[tauri::command]
async fn update_install(app: AppHandle) -> Result<(), String> {
    let events = app.clone();
    version::install(&app, move |done, total| {
        let _ = events.emit("update-progress", serde_json::json!({ "done": done, "total": total }));
    })
    .await
}

// ---------- self-test ----------

/// The UI has loaded and shows `tracks` tracks (see `smoke`).
#[tauri::command]
fn ui_ready(tracks: usize) {
    smoke::ui_ready(tracks);
}

// ---------- benchmark mode ----------

#[tauri::command]
fn perf_enabled() -> bool {
    std::env::var_os("HONEYCOMB_PERF").is_some()
}

#[tauri::command]
fn perf_log(line: String) {
    eprintln!("perf: {line}");
}

// ---------- player ----------

#[tauri::command]
fn player_load(state: State<AppState>, track_ids: Vec<i64>, index: usize) -> CmdResult<()> {
    let items = state.queue_items(&track_ids)?;
    state.player.send(Cmd::Load(items, index));
    Ok(())
}

#[tauri::command]
fn player_enqueue(state: State<AppState>, track_ids: Vec<i64>, next: bool) -> CmdResult<()> {
    let items = state.queue_items(&track_ids)?;
    state.player.send(Cmd::Enqueue(items, next));
    Ok(())
}

#[tauri::command]
fn player_cmd(state: State<AppState>, cmd: String, value: Option<f64>) -> CmdResult<()> {
    let v = value.unwrap_or(0.0);
    let c = match cmd.as_str() {
        "toggle" => Cmd::Toggle,
        "play" => Cmd::Play,
        "pause" => Cmd::Pause,
        "next" => Cmd::Next,
        "prev" => Cmd::Prev,
        "seek" => Cmd::Seek(v),
        "seekBy" => Cmd::SeekBy(v),
        "volume" => {
            let _ = db::set_setting(&state.conn(), "volume", &v.to_string());
            Cmd::Volume(v as f32)
        }
        "shuffle" => Cmd::Shuffle(v != 0.0),
        "repeatOff" => Cmd::Repeat(Repeat::Off),
        "repeatAll" => Cmd::Repeat(Repeat::All),
        "repeatOne" => Cmd::Repeat(Repeat::One),
        "jump" => Cmd::Jump(v as usize),
        "clear" => Cmd::Clear,
        "emit" => Cmd::Emit,
        other => return Err(format!("unknown player command {other}")),
    };
    state.player.send(c);
    Ok(())
}

#[tauri::command]
fn player_remove(state: State<AppState>, indices: Vec<usize>) {
    state.player.send(Cmd::Remove(indices));
}

// ---------- artwork protocol ----------

/// Serves album art at `art://localhost/<track id>` (`http://art.localhost/<id>` on Windows).
fn serve_art(app: &AppHandle, uri_path: &str) -> Response<Vec<u8>> {
    let not_found = || Response::builder().status(StatusCode::NOT_FOUND).body(Vec::new()).unwrap();
    let Ok(id) = uri_path.trim_matches('/').parse::<i64>() else {
        return not_found();
    };
    let state = app.state::<AppState>();
    let path = match db::track_by_id(&state.conn(), id) {
        Ok(Some(t)) => t.path,
        _ => return not_found(),
    };
    match meta::artwork(Path::new(&path)) {
        Some((data, mime)) => Response::builder()
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "max-age=86400")
            .body(data)
            .unwrap(),
        None => not_found(),
    }
}

/// Identifier used before 0.1.x builds were renamed; its data is copied over once.
const LEGACY_IDENTIFIER: &str = "com.honeycomb.player";

/// Where the library lives. `HONEYCOMB_DATA_DIR` points it elsewhere, so test
/// and benchmark libraries never touch the real one.
fn data_dir(app: &AppHandle) -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("HONEYCOMB_DATA_DIR") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir)?;
        return Ok(dir);
    }
    if smoke::folder().is_some() {
        // A self-test never touches the real library.
        let dir = std::env::temp_dir().join(format!("honeycomb-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        return Ok(dir);
    }
    let dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&dir)?;
    let legacy = dir.parent().map(|p| p.join(LEGACY_IDENTIFIER));
    if let Some(legacy) = legacy.filter(|l| l.join("library.db").is_file() && !dir.join("library.db").exists()) {
        // Copy rather than move, so the old copy stays as a backup.
        for name in ["library.db", "library.db-wal", "library.db-shm"] {
            if legacy.join(name).is_file() {
                std::fs::copy(legacy.join(name), dir.join(name))?;
            }
        }
        eprintln!("moved library from {} to {}", legacy.display(), dir.display());
    }
    Ok(dir)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .register_asynchronous_uri_scheme_protocol("art", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_string();
            std::thread::spawn(move || responder.respond(serve_art(&app, &path)));
        })
        .register_asynchronous_uri_scheme_protocol("plugin", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_string();
            std::thread::spawn(move || {
                let state = app.state::<AppState>();
                let enabled = plugins::enabled_ids(&state.conn());
                responder.respond(plugins::serve(&state.plugins, &enabled, &path))
            });
        })
        .setup(|app| {
            let dir = data_dir(app.handle())?;
            let db_path = dir.join("library.db");
            let plugins_dir = dir.join("plugins");
            std::fs::create_dir_all(&plugins_dir)?;
            // Plugins that ship with the app. Dev builds read them straight from
            // the checkout, so edits show up on the next start.
            let bundled = if cfg!(debug_assertions) {
                Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins"))
            } else {
                app.path().resource_dir().ok().map(|d| d.join("plugins"))
            };
            let conn = db::open(&db_path)?;
            db::migrate(&conn)?;
            let test_library = std::env::var_os("HONEYCOMB_DATA_DIR").is_some() || smoke::folder().is_some();
            secrets::init(&app.config().identifier, test_library.then_some(dir.as_path()));
            secrets::migrate(&conn);
            let volume = db::settings(&conn)?
                .into_iter()
                .find(|(k, _)| k == "volume")
                .and_then(|(_, v)| v.parse().ok())
                .unwrap_or(0.8);
            let player = player::spawn(app.handle().clone(), db_path.clone(), volume);
            media::init(app.handle(), player.clone());
            app.manage(AppState {
                db: Mutex::new(conn),
                db_path,
                player,
                scanning: Arc::new(AtomicBool::new(false)),
                sync_cancel: Arc::new(AtomicBool::new(false)),
                plugins: plugins::Dirs { user: plugins_dir, bundled },
            });
            if let Some(folder) = smoke::folder() {
                smoke::spawn(app.handle().clone(), folder);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_tracks,
            get_folders,
            add_folder,
            remove_folder,
            rescan,
            set_rating,
            clear_inbox,
            edit_tags,
            get_settings,
            set_setting,
            get_playlists,
            create_playlist,
            update_playlist,
            delete_playlist,
            playlist_track_ids,
            preview_smart,
            set_playlist_tracks,
            add_to_playlist,
            export_m3u,
            import_m3u,
            import_preview,
            import_apply,
            save_text,
            update_status,
            update_install,
            plugin_list,
            plugin_folder,
            plugin_http,
            plugin_oauth_redirect,
            plugin_secret_get,
            plugin_secret_set,
            match_tracks,
            sync_target,
            sync_run,
            sync_cancel,
            sync_formats,
            get_lyrics,
            artwork_ext,
            save_artwork,
            ui_ready,
            perf_enabled,
            perf_log,
            player_load,
            player_enqueue,
            player_cmd,
            player_remove,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
