//! Scrobbling to Last.fm and ListenBrainz.
//!
//! The player calls `listened` once a song has been heard long enough
//! (Last.fm's rule: songs over 30 s, after half of them or 4 minutes) and
//! `now_playing` when one starts. Scrobbles go into the `scrobbles` table
//! first and a worker thread sends them from there, oldest first, so songs
//! heard offline are sent once the internet is back, in order, and a row is
//! only marked sent after the service accepted it.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter};

use crate::db;
use crate::models::Track;

/// Settings keys. `secret.` keys are never sent to the UI.
const LASTFM_SESSION: &str = "secret.lastfmSession";
const LASTFM_USER: &str = "lastfm.user";
const LB_TOKEN: &str = "secret.listenbrainzToken";
const LB_USER: &str = "listenbrainz.user";

/// Scrobbles per request.
const LASTFM_BATCH: usize = 50;
const LB_BATCH: usize = 100;
/// Retry delays after a failed send: doubled each time, up to the maximum.
const RETRY_FIRST: Duration = Duration::from_secs(30);
const RETRY_MAX: Duration = Duration::from_secs(15 * 60);

/// Where to send, and with which Last.fm API account. Built from the
/// environment, so tests and private builds can point it elsewhere.
#[derive(Clone)]
pub struct Config {
    pub lastfm_api: String,
    pub lastfm_key: Option<(String, String)>,
    pub listenbrainz_api: String,
}

impl Config {
    /// The Last.fm API key and secret are baked in at build time
    /// (HONEYCOMB_LASTFM_KEY / HONEYCOMB_LASTFM_SECRET); the same variables at
    /// run time override them.
    pub fn from_env() -> Config {
        let var = |name: &str, built: Option<&'static str>| {
            std::env::var(name).ok().or(built.map(str::to_string)).filter(|v| !v.is_empty())
        };
        let key = var("HONEYCOMB_LASTFM_KEY", option_env!("HONEYCOMB_LASTFM_KEY"));
        let secret = var("HONEYCOMB_LASTFM_SECRET", option_env!("HONEYCOMB_LASTFM_SECRET"));
        Config {
            lastfm_api: std::env::var("HONEYCOMB_LASTFM_API")
                .unwrap_or_else(|_| "https://ws.audioscrobbler.com/2.0/".into()),
            lastfm_key: key.zip(secret),
            listenbrainz_api: std::env::var("HONEYCOMB_LISTENBRAINZ_API")
                .unwrap_or_else(|_| "https://api.listenbrainz.org".into()),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Service {
    LastFm,
    ListenBrainz,
}

impl Service {
    fn column(self) -> &'static str {
        match self {
            Service::LastFm => "lastfm",
            Service::ListenBrainz => "listenbrainz",
        }
    }

    fn credential(self) -> &'static str {
        match self {
            Service::LastFm => LASTFM_SESSION,
            Service::ListenBrainz => LB_TOKEN,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Service::LastFm => "Last.fm",
            Service::ListenBrainz => "ListenBrainz",
        }
    }
}

/// How long a song of `duration` seconds has to be heard before it counts,
/// or None for songs too short to scrobble.
pub fn threshold(duration: f64) -> Option<f64> {
    (duration > 30.0).then(|| (duration / 2.0).min(240.0))
}

// ---------- settings ----------

fn setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .filter(|v| !v.is_empty())
}

fn remove_setting(conn: &Connection, key: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", [key]).map(|_| ())
}

fn connected(conn: &Connection, service: Service) -> bool {
    setting(conn, service.credential()).is_some()
}

// ---------- the queue ----------

/// Artist and title to scrobble a track as, or None without them.
fn names(t: &Track) -> Option<(String, String)> {
    let artist = if t.artist.is_empty() { &t.album_artist } else { &t.artist };
    (!artist.is_empty() && !t.title.is_empty()).then(|| (artist.clone(), t.title.clone()))
}

/// Queue a scrobble for every connected service. Returns whether one was queued.
pub fn queue(conn: &Connection, t: &Track, played_at: i64) -> rusqlite::Result<bool> {
    let (lastfm, lb) = (connected(conn, Service::LastFm), connected(conn, Service::ListenBrainz));
    let Some((artist, title)) = names(t).filter(|_| lastfm || lb) else {
        return Ok(false);
    };
    conn.execute(
        "INSERT INTO scrobbles (track_id, artist, title, album, album_artist, duration, track_no, played_at, lastfm, listenbrainz)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![t.id, artist, title, t.album, t.album_artist, t.duration, t.track_no, played_at, !lastfm, !lb],
    )?;
    Ok(true)
}

#[derive(Debug, Clone)]
struct Pending {
    id: i64,
    artist: String,
    title: String,
    album: String,
    album_artist: String,
    duration: f64,
    track_no: Option<u32>,
    played_at: i64,
}

fn pending(conn: &Connection, service: Service, limit: usize) -> rusqlite::Result<Vec<Pending>> {
    let col = service.column();
    let mut stmt = conn.prepare(&format!(
        "SELECT id, artist, title, album, album_artist, duration, track_no, played_at
         FROM scrobbles WHERE {col} = 0 ORDER BY played_at, id LIMIT ?1"
    ))?;
    let rows = stmt.query_map([limit as i64], |r| {
        Ok(Pending {
            id: r.get(0)?,
            artist: r.get(1)?,
            title: r.get(2)?,
            album: r.get(3)?,
            album_artist: r.get(4)?,
            duration: r.get(5)?,
            track_no: r.get(6)?,
            played_at: r.get(7)?,
        })
    })?;
    rows.collect()
}

fn mark_sent(conn: &Connection, service: Service, rows: &[Pending]) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(&format!("UPDATE scrobbles SET {} = 1 WHERE id = ?1", service.column()))?;
    for r in rows {
        stmt.execute([r.id])?;
    }
    // Rows every service has taken are done with.
    conn.execute("DELETE FROM scrobbles WHERE lastfm = 1 AND listenbrainz = 1", [])?;
    Ok(())
}

fn pending_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM scrobbles WHERE lastfm = 0 OR listenbrainz = 0", [], |r| r.get(0))
        .unwrap_or(0)
}

// ---------- sending ----------

#[derive(Debug)]
enum SendError {
    /// Offline, rate-limited or the service is down: try again later.
    Retry(String),
    /// The session or token stopped working: sign in again.
    Auth(String),
    /// The service refused these scrobbles for good: skip them.
    Rejected(String),
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("Honeycomb/", env!("CARGO_PKG_VERSION"), " (desktop music player)"))
        .build()
}

/// Last.fm's request signature: every parameter (but `format`) sorted by
/// name, names and values run together, the secret appended, MD5 in hex.
fn sign(params: &[(String, String)], secret: &str) -> String {
    let mut sorted: Vec<&(String, String)> = params.iter().filter(|(k, _)| k != "format").collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut s = String::new();
    for (k, v) in sorted {
        s.push_str(k);
        s.push_str(v);
    }
    s.push_str(secret);
    format!("{:x}", md5::compute(s.as_bytes()))
}

fn lastfm_call(cfg: &Config, method: &str, mut params: Vec<(String, String)>, sk: Option<&str>) -> Result<Value, SendError> {
    let Some((key, secret)) = &cfg.lastfm_key else {
        return Err(SendError::Auth("This build of Honeycomb has no Last.fm API key.".into()));
    };
    params.push(("method".into(), method.into()));
    params.push(("api_key".into(), key.clone()));
    if let Some(sk) = sk {
        params.push(("sk".into(), sk.into()));
    }
    let sig = sign(&params, secret);
    params.push(("api_sig".into(), sig));
    params.push(("format".into(), "json".into()));
    let form: Vec<(&str, &str)> = params.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let body: Value = match agent().post(&cfg.lastfm_api).send_form(&form) {
        Ok(r) => crate::net::read_json(r, crate::net::SCROBBLE_MAX)
            .map_err(|e| SendError::Retry(format!("Last.fm sent an unreadable answer: {e}")))?,
        Err(ureq::Error::Status(code, r)) => match crate::net::read_json::<Value>(r, crate::net::SCROBBLE_MAX) {
            Ok(v) if v.get("error").is_some() => v,
            _ => return Err(SendError::Retry(format!("Last.fm answered with HTTP {code}"))),
        },
        Err(e) => return Err(SendError::Retry(format!("Couldn't reach Last.fm: {e}"))),
    };
    match body.get("error").and_then(Value::as_i64) {
        None => Ok(body),
        Some(code) => {
            let msg = format!(
                "Last.fm: {} (error {code})",
                body.get("message").and_then(Value::as_str).unwrap_or("request failed")
            );
            Err(match code {
                // Auth failed, invalid session key, invalid or suspended API key.
                4 | 9 | 10 | 26 => SendError::Auth(msg),
                // Service offline or temporarily unavailable, rate limited.
                8 | 11 | 16 | 29 => SendError::Retry(msg),
                // Token not authorized yet (sign-in in progress) is handled by the caller.
                _ => SendError::Rejected(msg),
            })
        }
    }
}

fn lastfm_scrobble(cfg: &Config, sk: &str, rows: &[Pending]) -> Result<(), SendError> {
    let mut p = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        p.push((format!("artist[{i}]"), r.artist.clone()));
        p.push((format!("track[{i}]"), r.title.clone()));
        p.push((format!("timestamp[{i}]"), r.played_at.to_string()));
        if !r.album.is_empty() {
            p.push((format!("album[{i}]"), r.album.clone()));
        }
        if !r.album_artist.is_empty() && r.album_artist != r.artist {
            p.push((format!("albumArtist[{i}]"), r.album_artist.clone()));
        }
        if r.duration > 0.0 {
            p.push((format!("duration[{i}]"), (r.duration.round() as i64).to_string()));
        }
        if let Some(n) = r.track_no {
            p.push((format!("trackNumber[{i}]"), n.to_string()));
        }
    }
    // Accepted or ignored (too old, filtered by Last.fm), either way it's done.
    lastfm_call(cfg, "track.scrobble", p, Some(sk)).map(|_| ())
}

fn lb_metadata(r: &Pending) -> Value {
    let mut info = json!({
        "submission_client": "Honeycomb",
        "submission_client_version": env!("CARGO_PKG_VERSION"),
        "media_player": "Honeycomb",
    });
    if r.duration > 0.0 {
        info["duration_ms"] = json!((r.duration * 1000.0).round() as i64);
    }
    if let Some(n) = r.track_no {
        info["tracknumber"] = json!(n);
    }
    let mut meta = json!({ "artist_name": r.artist, "track_name": r.title, "additional_info": info });
    if !r.album.is_empty() {
        meta["release_name"] = json!(r.album);
    }
    meta
}

fn lb_post(cfg: &Config, token: &str, body: Value) -> Result<(), SendError> {
    let url = format!("{}/1/submit-listens", cfg.listenbrainz_api.trim_end_matches('/'));
    match agent().post(&url).set("Authorization", &format!("Token {token}")).send_json(body) {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, r)) => {
            let detail = crate::net::read_json::<Value>(r, crate::net::SCROBBLE_MAX)
                .ok()
                .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_else(|| format!("HTTP {code}"));
            let msg = format!("ListenBrainz: {detail}");
            Err(match code {
                401 => SendError::Auth(msg),
                400 => SendError::Rejected(msg),
                _ => SendError::Retry(msg),
            })
        }
        Err(e) => Err(SendError::Retry(format!("Couldn't reach ListenBrainz: {e}"))),
    }
}

fn lb_submit(cfg: &Config, token: &str, rows: &[Pending]) -> Result<(), SendError> {
    let payload: Vec<Value> =
        rows.iter().map(|r| json!({ "listened_at": r.played_at, "track_metadata": lb_metadata(r) })).collect();
    let kind = if rows.len() == 1 { "single" } else { "import" };
    lb_post(cfg, token, json!({ "listen_type": kind, "payload": payload }))
}

/// Send everything waiting, oldest first. Err means "try again later".
fn flush(conn: &Connection, cfg: &Config) -> Result<(), String> {
    for service in [Service::LastFm, Service::ListenBrainz] {
        loop {
            let Some(cred) = setting(conn, service.credential()) else { break };
            let batch = if service == Service::LastFm { LASTFM_BATCH } else { LB_BATCH };
            let rows = pending(conn, service, batch).map_err(|e| e.to_string())?;
            if rows.is_empty() {
                break;
            }
            let sent = match service {
                Service::LastFm => lastfm_scrobble(cfg, &cred, &rows),
                Service::ListenBrainz => lb_submit(cfg, &cred, &rows),
            };
            match sent {
                Ok(()) => mark_sent(conn, service, &rows).map_err(|e| e.to_string())?,
                Err(SendError::Rejected(msg)) => {
                    eprintln!("scrobble: {} refused {} scrobbles, skipping them: {msg}", service.name(), rows.len());
                    mark_sent(conn, service, &rows).map_err(|e| e.to_string())?;
                }
                Err(SendError::Auth(msg)) => {
                    // Signed out: keep nothing for it, and say so.
                    disconnect(conn, service).map_err(|e| e.to_string())?;
                    return Err(format!("{msg} Sign in again in Settings."));
                }
                Err(SendError::Retry(msg)) => return Err(msg),
            }
        }
    }
    Ok(())
}

fn send_now_playing(conn: &Connection, cfg: &Config, t: &Track) {
    let Some((artist, title)) = names(t) else { return };
    let row = Pending {
        id: 0,
        artist,
        title,
        album: t.album.clone(),
        album_artist: t.album_artist.clone(),
        duration: t.duration,
        track_no: t.track_no,
        played_at: 0,
    };
    if let Some(sk) = setting(conn, LASTFM_SESSION) {
        let mut p = vec![("artist".to_string(), row.artist.clone()), ("track".to_string(), row.title.clone())];
        if !row.album.is_empty() {
            p.push(("album".into(), row.album.clone()));
        }
        if row.duration > 0.0 {
            p.push(("duration".into(), (row.duration.round() as i64).to_string()));
        }
        if let Err(e) = lastfm_call(cfg, "track.updateNowPlaying", p, Some(&sk)) {
            eprintln!("scrobble: now playing: {e:?}");
        }
    }
    if let Some(token) = setting(conn, LB_TOKEN) {
        let body = json!({ "listen_type": "playing_now", "payload": [{ "track_metadata": lb_metadata(&row) }] });
        if let Err(e) = lb_post(cfg, &token, body) {
            eprintln!("scrobble: now playing: {e:?}");
        }
    }
}

// ---------- the worker ----------

enum Msg {
    Flush,
    NowPlaying(i64),
}

struct Worker {
    tx: Sender<Msg>,
    app: AppHandle,
}

static WORKER: OnceLock<Worker> = OnceLock::new();
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

pub fn init(app: AppHandle, db_path: PathBuf) {
    let (tx, rx) = channel();
    let path = db_path;
    let events = app.clone();
    std::thread::Builder::new()
        .name("scrobble".into())
        .spawn(move || {
            let cfg = Config::from_env();
            let mut retry: Option<Duration> = None;
            let mut flush_now = true; // send what's left from last time
            loop {
                if !flush_now {
                    let msg = match retry {
                        Some(d) => rx.recv_timeout(d),
                        None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    };
                    match msg {
                        Ok(Msg::NowPlaying(id)) => {
                            if let Ok(conn) = db::open(&path) {
                                if let Ok(Some(t)) = db::track_by_id(&conn, id) {
                                    send_now_playing(&conn, &cfg, &t);
                                }
                            }
                            continue;
                        }
                        Ok(Msg::Flush) | Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                flush_now = false;
                let Ok(conn) = db::open(&path) else { continue };
                let result = flush(&conn, &cfg);
                retry = match &result {
                    Ok(()) => None,
                    Err(_) => Some(retry.map_or(RETRY_FIRST, |d| (d * 2).min(RETRY_MAX))),
                };
                if let Err(e) = &result {
                    eprintln!("scrobble: {e}");
                }
                *LAST_ERROR.lock().unwrap_or_else(|p| p.into_inner()) = result.err();
                let _ = events.emit("scrobble-status", status(&conn));
            }
        })
        .expect("spawn scrobble thread");
    let _ = WORKER.set(Worker { tx, app });
}

fn send(msg: Msg) {
    if let Some(w) = WORKER.get() {
        let _ = w.tx.send(msg);
    }
}

/// A song started playing.
pub fn now_playing(track_id: i64) {
    send(Msg::NowPlaying(track_id));
}

/// A song was heard long enough to count, starting at `played_at`.
pub fn listened(db_path: &Path, track_id: i64, played_at: i64) {
    let Ok(conn) = db::open(db_path) else { return };
    let Ok(Some(t)) = db::track_by_id(&conn, track_id) else { return };
    if matches!(queue(&conn, &t, played_at), Ok(true)) {
        send(Msg::Flush);
    }
}

// ---------- accounts, for the Settings screen ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// This build has a Last.fm API key.
    pub lastfm_available: bool,
    pub lastfm_user: Option<String>,
    pub listenbrainz_user: Option<String>,
    /// Scrobbles not sent yet.
    pub pending: i64,
    pub error: Option<String>,
}

pub fn status(conn: &Connection) -> Status {
    Status {
        lastfm_available: Config::from_env().lastfm_key.is_some(),
        lastfm_user: connected(conn, Service::LastFm).then(|| setting(conn, LASTFM_USER).unwrap_or_default()),
        listenbrainz_user: connected(conn, Service::ListenBrainz).then(|| setting(conn, LB_USER).unwrap_or_default()),
        pending: pending_count(conn),
        error: LAST_ERROR.lock().unwrap_or_else(|p| p.into_inner()).clone(),
    }
}

fn emit_status(conn: &Connection) {
    if let Some(w) = WORKER.get() {
        let _ = w.app.emit("scrobble-status", status(conn));
    }
}

/// Start signing in: returns a token and the Last.fm page where the user approves it.
pub fn lastfm_begin() -> Result<(String, String), String> {
    let cfg = Config::from_env();
    let v = lastfm_call(&cfg, "auth.getToken", Vec::new(), None).map_err(describe)?;
    let token = v.get("token").and_then(Value::as_str).ok_or("Last.fm sent no token")?.to_string();
    let key = &cfg.lastfm_key.as_ref().ok_or("no API key")?.0;
    Ok((token.clone(), format!("https://www.last.fm/api/auth/?api_key={key}&token={token}")))
}

/// Finish signing in once the user approved `token` in the browser. Ok(None)
/// means they haven't yet.
pub fn lastfm_finish(conn: &Connection, token: &str) -> Result<Option<String>, String> {
    let cfg = Config::from_env();
    let v = match lastfm_call(&cfg, "auth.getSession", vec![("token".into(), token.into())], None) {
        Ok(v) => v,
        // 14: not authorized yet.
        Err(SendError::Rejected(m)) if m.contains("(error 14)") => return Ok(None),
        Err(e) => return Err(describe(e)),
    };
    let session = v.get("session").ok_or("Last.fm sent no session")?;
    let key = session.get("key").and_then(Value::as_str).ok_or("Last.fm sent no session key")?;
    let name = session.get("name").and_then(Value::as_str).unwrap_or_default();
    db::set_setting(conn, LASTFM_SESSION, key).map_err(|e| e.to_string())?;
    db::set_setting(conn, LASTFM_USER, name).map_err(|e| e.to_string())?;
    emit_status(conn);
    Ok(Some(name.to_string()))
}

/// Check a ListenBrainz user token and keep it. Returns the user name.
pub fn listenbrainz_connect(conn: &Connection, token: &str) -> Result<String, String> {
    let cfg = Config::from_env();
    let url = format!("{}/1/validate-token", cfg.listenbrainz_api.trim_end_matches('/'));
    let v: Value = match agent().get(&url).set("Authorization", &format!("Token {token}")).call() {
        Ok(r) => crate::net::read_json(r, crate::net::SCROBBLE_MAX).map_err(|e| format!("ListenBrainz: {e}"))?,
        Err(ureq::Error::Status(_, r)) => crate::net::read_json(r, crate::net::SCROBBLE_MAX).unwrap_or(Value::Null),
        Err(e) => return Err(format!("Couldn't reach ListenBrainz: {e}")),
    };
    if v.get("valid").and_then(Value::as_bool) != Some(true) {
        return Err("ListenBrainz didn't accept that token. Copy it from listenbrainz.org/settings.".into());
    }
    let name = v.get("user_name").and_then(Value::as_str).unwrap_or_default().to_string();
    db::set_setting(conn, LB_TOKEN, token).map_err(|e| e.to_string())?;
    db::set_setting(conn, LB_USER, &name).map_err(|e| e.to_string())?;
    emit_status(conn);
    Ok(name)
}

/// Sign out of a service and drop what was waiting for it.
pub fn disconnect(conn: &Connection, service: Service) -> rusqlite::Result<()> {
    remove_setting(conn, service.credential())?;
    remove_setting(conn, if service == Service::LastFm { LASTFM_USER } else { LB_USER })?;
    conn.execute(&format!("UPDATE scrobbles SET {} = 1 WHERE {0} = 0", service.column()), [])?;
    conn.execute("DELETE FROM scrobbles WHERE lastfm = 1 AND listenbrainz = 1", [])?;
    emit_status(conn);
    Ok(())
}

fn describe(e: SendError) -> String {
    match e {
        SendError::Retry(m) | SendError::Auth(m) | SendError::Rejected(m) => m,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    /// A tiny HTTP server that records each request's path and body and
    /// answers with `answer(path)` as JSON.
    fn mock(answer: fn(&str) -> (u16, String)) -> (String, Arc<Mutex<Vec<(String, String)>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let mut len = 0;
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                reader.read_exact(&mut body).unwrap();
                let body = String::from_utf8_lossy(&body).into_owned();
                log.lock().unwrap().push((path.clone(), body));
                let (code, json) = answer(&path);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                );
            }
        });
        (base, seen)
    }

    fn library() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::migrate(&conn).unwrap();
        conn
    }

    fn track(id: i64, title: &str) -> Track {
        Track {
            id,
            title: title.into(),
            artist: "Artist".into(),
            album: "Album & Co".into(),
            duration: 200.0,
            track_no: Some(id as u32),
            ..Default::default()
        }
    }

    fn decode(s: &str) -> String {
        let s = s.replace('+', " ");
        let mut out = Vec::new();
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn threshold_follows_lastfm_rules() {
        assert_eq!(threshold(30.0), None);
        assert_eq!(threshold(100.0), Some(50.0));
        assert_eq!(threshold(1200.0), Some(240.0));
    }

    #[test]
    fn signature() {
        // md5("api_keyxxmethodauth.getSessiontokenyy" + "secret")
        let p = vec![
            ("token".to_string(), "yy".to_string()),
            ("method".to_string(), "auth.getSession".to_string()),
            ("api_key".to_string(), "xx".to_string()),
            ("format".to_string(), "json".to_string()),
        ];
        assert_eq!(sign(&p, "secret"), format!("{:x}", md5::compute("api_keyxxmethodauth.getSessiontokenyysecret")));
    }

    #[test]
    fn nothing_queued_without_an_account() {
        let conn = library();
        assert!(!queue(&conn, &track(1, "A"), 100).unwrap());
        db::set_setting(&conn, LB_TOKEN, "t").unwrap();
        assert!(queue(&conn, &track(1, "A"), 100).unwrap());
        // Last.fm isn't connected, so the row only waits for ListenBrainz.
        assert!(pending(&conn, Service::LastFm, 10).unwrap().is_empty());
        assert_eq!(pending(&conn, Service::ListenBrainz, 10).unwrap().len(), 1);
        // No artist or title, nothing to scrobble.
        assert!(!queue(&conn, &Track { artist: String::new(), ..track(2, "B") }, 100).unwrap());
    }

    #[test]
    fn sends_in_order_once_and_keeps_them_while_offline() {
        let conn = library();
        db::set_setting(&conn, LASTFM_SESSION, "sk1").unwrap();
        db::set_setting(&conn, LB_TOKEN, "tok").unwrap();
        // Heard out of order of insertion: sent oldest first.
        queue(&conn, &track(2, "Second"), 2000).unwrap();
        queue(&conn, &track(1, "First"), 1000).unwrap();
        queue(&conn, &track(3, "Third"), 3000).unwrap();

        // Offline: nothing is lost.
        let offline = Config {
            lastfm_api: "http://127.0.0.1:9/".into(),
            lastfm_key: Some(("key".into(), "secret".into())),
            listenbrainz_api: "http://127.0.0.1:9".into(),
        };
        assert!(flush(&conn, &offline).is_err());
        assert_eq!(pending_count(&conn), 3);

        let (base, seen) = mock(|path| {
            if path.starts_with("/lastfm") {
                (200, r#"{"scrobbles":{"@attr":{"accepted":3,"ignored":0}}}"#.into())
            } else {
                (200, r#"{"status":"ok"}"#.into())
            }
        });
        let online = Config {
            lastfm_api: format!("{base}/lastfm/"),
            lastfm_key: Some(("key".into(), "secret".into())),
            listenbrainz_api: format!("{base}/lb"),
        };
        flush(&conn, &online).unwrap();
        assert_eq!(pending_count(&conn), 0);
        let count: i64 = conn.query_row("SELECT count(*) FROM scrobbles", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0, "sent rows are cleaned up");

        let reqs = seen.lock().unwrap().clone();
        assert_eq!(reqs.len(), 2, "one batch per service: {reqs:?}");
        let lastfm = decode(&reqs[0].1);
        assert!(lastfm.contains("method=track.scrobble") && lastfm.contains("sk=sk1"), "{lastfm}");
        assert!(lastfm.contains("track[0]=First") && lastfm.contains("timestamp[0]=1000"), "{lastfm}");
        assert!(lastfm.contains("track[2]=Third") && lastfm.contains("album[0]=Album & Co"), "{lastfm}");
        assert!(lastfm.contains("api_sig="), "{lastfm}");
        let lb: Value = serde_json::from_str(&reqs[1].1).unwrap();
        assert_eq!(reqs[1].0, "/lb/1/submit-listens");
        assert_eq!(lb["listen_type"], "import");
        assert_eq!(lb["payload"][0]["listened_at"], 1000);
        assert_eq!(lb["payload"][1]["track_metadata"]["track_name"], "Second");
        assert_eq!(lb["payload"][0]["track_metadata"]["additional_info"]["duration_ms"], 200_000);

        // Nothing goes twice.
        flush(&conn, &online).unwrap();
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn invalid_session_signs_out() {
        let conn = library();
        db::set_setting(&conn, LASTFM_SESSION, "old").unwrap();
        queue(&conn, &track(1, "A"), 1000).unwrap();
        let (base, _) = mock(|_| (403, r#"{"error":9,"message":"Invalid session key"}"#.into()));
        let cfg = Config {
            lastfm_api: format!("{base}/"),
            lastfm_key: Some(("key".into(), "secret".into())),
            listenbrainz_api: base.clone(),
        };
        let err = flush(&conn, &cfg).unwrap_err();
        assert!(err.contains("Invalid session key"), "{err}");
        assert!(!connected(&conn, Service::LastFm));
        assert_eq!(pending_count(&conn), 0);
    }

    #[test]
    fn rate_limits_are_retried_later() {
        let conn = library();
        db::set_setting(&conn, LASTFM_SESSION, "sk").unwrap();
        queue(&conn, &track(1, "A"), 1000).unwrap();
        let (base, _) = mock(|_| (200, r#"{"error":29,"message":"Rate limit exceeded"}"#.into()));
        let cfg = Config {
            lastfm_api: format!("{base}/"),
            lastfm_key: Some(("key".into(), "secret".into())),
            listenbrainz_api: base.clone(),
        };
        assert!(flush(&conn, &cfg).is_err());
        assert!(connected(&conn, Service::LastFm));
        assert_eq!(pending_count(&conn), 1);
    }
}
