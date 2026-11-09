//! Servers as library sources. A server's songs are mirrored into `tracks`
//! as rows whose path is `subsonic://<source>/<song id>`, so browsing,
//! search and smart playlists work as for files. The server owns their
//! ratings, loved flags, play counts and playlists: changes made here wait in
//! `remote_outbox` until the server has taken them.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;

use crate::models::{Track, now_secs};
use crate::stream::{self, Opened};
use crate::subsonic::{Auth, Client, Playlist, Song, parse_time};
use crate::{db, secrets};

pub const SCHEME: &str = "subsonic://";
const PAGE: usize = 500;
const STREAM_CACHE_MAX: u64 = 1024 * 1024 * 1024;

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sources (
            id INTEGER PRIMARY KEY,
            kind TEXT NOT NULL DEFAULT 'subsonic',
            name TEXT NOT NULL,
            url TEXT NOT NULL,
            username TEXT NOT NULL,
            auth TEXT NOT NULL DEFAULT 'password',
            server TEXT NOT NULL DEFAULT '',
            version TEXT NOT NULL DEFAULT '',
            extensions TEXT NOT NULL DEFAULT '',
            last_sync INTEGER,
            error TEXT
        );
        CREATE TABLE IF NOT EXISTS remote_tracks (
            track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
            source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
            remote_id TEXT NOT NULL,
            album_id TEXT,
            cover_id TEXT,
            suffix TEXT NOT NULL DEFAULT '',
            size INTEGER,
            -- remote, downloading, pinned, local-only
            state TEXT NOT NULL DEFAULT 'remote',
            file TEXT,
            UNIQUE (source_id, remote_id)
        );
        CREATE INDEX IF NOT EXISTS idx_remote_album ON remote_tracks(source_id, album_id);
        CREATE TABLE IF NOT EXISTS remote_playlists (
            playlist_id INTEGER PRIMARY KEY REFERENCES playlists(id) ON DELETE CASCADE,
            source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
            remote_id TEXT NOT NULL
        );
        -- Albums kept offline, so songs added to them later are kept too.
        CREATE TABLE IF NOT EXISTS remote_pins (
            source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
            album_id TEXT NOT NULL,
            PRIMARY KEY (source_id, album_id)
        );
        CREATE TABLE IF NOT EXISTS remote_outbox (
            id INTEGER PRIMARY KEY,
            source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
            kind TEXT NOT NULL,
            target TEXT NOT NULL,
            value INTEGER NOT NULL DEFAULT 0,
            at INTEGER NOT NULL
        );",
    )?;
    Ok(())
}

struct Ctx {
    db_path: PathBuf,
    data_dir: PathBuf,
    clients: Mutex<HashMap<i64, Client>>,
}

static CTX: OnceLock<Ctx> = OnceLock::new();
static WORK: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

pub fn init(db_path: &Path, data_dir: &Path) {
    let _ = CTX.set(Ctx { db_path: db_path.into(), data_dir: data_dir.into(), clients: Mutex::default() });
}

fn ctx() -> Result<&'static Ctx> {
    CTX.get().ok_or_else(|| anyhow!("server sources aren't set up"))
}

pub fn poke() {
    *WORK.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
    WORK.1.notify_all();
}

pub fn wait_for_work(timeout: Duration) {
    let mut w = WORK.0.lock().unwrap_or_else(|e| e.into_inner());
    if !*w {
        w = WORK.1.wait_timeout(w, timeout).unwrap_or_else(|e| e.into_inner()).0;
    }
    *w = false;
}

fn cert_key(id: i64) -> String {
    format!("secret.server.{id}.certificate")
}

fn secret_key(id: i64) -> String {
    format!("secret.server.{id}.password")
}

pub fn client(conn: &Connection, id: i64) -> Result<Client> {
    if let Some(c) = CTX.get().and_then(|x| x.clients.lock().ok()?.get(&id).cloned()) {
        return Ok(c);
    }
    let (url, user, auth): (String, String, String) = conn
        .query_row("SELECT url, username, auth FROM sources WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?
        .ok_or_else(|| anyhow!("no such server"))?;
    let secret = secrets::get(conn, &secret_key(id)).ok_or_else(|| anyhow!("The password for {url} isn't in the keychain any more"))?;
    let mut c = Client::new(&url, &user, if auth == "apikey" { Auth::ApiKey(secret) } else { Auth::Password(secret) });
    if let Some(x) = CTX.get() {
        if let Ok(p12) = fs::read(x.data_dir.join("certs").join(format!("{id}.p12"))) {
            let pw = secrets::get(conn, &cert_key(id)).unwrap_or_default();
            c = c.with_identity(&p12, &pw)?;
        }
    }
    if let Some(x) = CTX.get() {
        x.clients.lock().unwrap_or_else(|e| e.into_inner()).insert(id, c.clone());
    }
    Ok(c)
}

fn forget_client(id: i64) {
    if let Some(x) = CTX.get() {
        x.clients.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRow {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub username: String,
    pub server: String,
    pub version: String,
    pub last_sync: Option<i64>,
    pub error: Option<String>,
    pub tracks: i64,
    pub pinned: i64,
    pub waiting: i64,
}

pub fn sources(conn: &Connection) -> Result<Vec<SourceRow>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.name, s.url, s.username, s.server, s.version, s.last_sync, s.error,
            (SELECT COUNT(*) FROM remote_tracks r WHERE r.source_id = s.id),
            (SELECT COUNT(*) FROM remote_tracks r WHERE r.source_id = s.id AND r.state IN ('pinned', 'local-only')),
            (SELECT COUNT(*) FROM remote_outbox o WHERE o.source_id = s.id)
         FROM sources s ORDER BY s.id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(SourceRow {
            id: r.get(0)?,
            name: r.get(1)?,
            url: r.get(2)?,
            username: r.get(3)?,
            server: r.get(4)?,
            version: r.get(5)?,
            last_sync: r.get(6)?,
            error: r.get(7)?,
            tracks: r.get(8)?,
            pinned: r.get(9)?,
            waiting: r.get(10)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn add_source(conn: &Connection, url: &str, username: &str, password: &str) -> Result<i64> {
    add_source_with(conn, url, username, password, None)
}

/// `cert` is a client certificate: the .p12 file's bytes and its password.
pub fn add_source_with(conn: &Connection, url: &str, username: &str, password: &str, cert: Option<(&[u8], &str)>) -> Result<i64> {
    let make = |auth: Auth| -> Result<Client> {
        let c = Client::new(url, username, auth);
        match cert {
            Some((p12, pw)) => c.with_identity(p12, pw),
            None => Ok(c),
        }
    };
    let probe = make(Auth::Password(password.into()))?;
    let first = probe.ping();
    let (client, info) = match first {
        Ok(info) if !username.is_empty() => (probe, info),
        _ => {
            let keyed = make(Auth::ApiKey(password.into()))?;
            match keyed.ping() {
                Ok(info) if info.extensions.iter().any(|e| e == "apiKeyAuthentication") => (keyed, info),
                _ => (probe, first?),
            }
        }
    };
    let host = client.base.split("://").nth(1).unwrap_or(&client.base).to_string();
    let exists: Option<i64> = conn
        .query_row("SELECT id FROM sources WHERE url = ?1 AND username = ?2", params![client.base, username], |r| r.get(0))
        .optional()?;
    if exists.is_some() {
        bail!("{host} is already in your library");
    }
    let auth = if matches!(client.auth, Auth::ApiKey(_)) { "apikey" } else { "password" };
    let name = match info.kind.as_str() {
        "" => host.clone(),
        k => format!("{}{} on {host}", k[..1].to_uppercase(), &k[1..]),
    };
    conn.execute(
        "INSERT INTO sources (name, url, username, auth, server, version, extensions) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![name, client.base, username, auth, info.kind, info.version, info.extensions.join(",")],
    )?;
    let id = conn.last_insert_rowid();
    let saved = (|| -> Result<()> {
        secrets::set(conn, &secret_key(id), password).map_err(|e| anyhow!(e))?;
        if let Some((p12, pw)) = cert {
            let dir = ctx()?.data_dir.join("certs");
            fs::create_dir_all(&dir)?;
            fs::write(dir.join(format!("{id}.p12")), p12)?;
            secrets::set(conn, &cert_key(id), pw).map_err(|e| anyhow!(e))?;
        }
        Ok(())
    })();
    if let Err(e) = saved {
        conn.execute("DELETE FROM sources WHERE id = ?1", [id])?;
        let _ = secrets::delete(conn, &secret_key(id));
        bail!(e);
    }
    Ok(id)
}

pub fn set_error(conn: &Connection, id: i64, error: Option<&str>) -> Result<()> {
    conn.execute("UPDATE sources SET error = ?2 WHERE id = ?1", params![id, error])?;
    Ok(())
}

pub fn remove_source(conn: &mut Connection, id: i64, keep: Option<&Path>) -> Result<RemoveReport> {
    let kept_files: Vec<(i64, String, Track)> = {
        let mut stmt = conn.prepare(
            "SELECT track_id, file FROM remote_tracks
             WHERE source_id = ?1 AND file IS NOT NULL AND state IN ('pinned', 'local-only')",
        )?;
        let rows = stmt.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let rows: Vec<(i64, String)> = rows.collect::<rusqlite::Result<_>>()?;
        let mut out = Vec::new();
        for (tid, file) in rows {
            if let Some(t) = db::track_by_id(conn, tid)? {
                out.push((tid, file, t));
            }
        }
        out
    };
    let mut report = RemoveReport::default();
    let tx = conn.transaction()?;
    if let Some(dir) = keep {
        for (track_id, file, t) in &kept_files {
            let dest = unique_path(&dir.join(safe(&t.album_artist_or_artist())).join(safe(&t.album)).join(file_name(t, file)));
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            if fs::rename(file, &dest).is_err() {
                fs::copy(file, &dest)?;
                let _ = fs::remove_file(file);
            }
            let dest_s = dest.to_string_lossy().to_string();
            tx.execute(
                "UPDATE tracks SET path = ?2, mtime = ?3 WHERE id = ?1",
                params![track_id, dest_s, crate::meta::file_mtime(&dest)],
            )?;
            tx.execute("DELETE FROM remote_tracks WHERE track_id = ?1", [track_id])?;
            report.kept += 1;
        }
        let folder = dir.to_string_lossy().to_string();
        if report.kept > 0 {
            tx.execute("INSERT OR IGNORE INTO folders (path, scanned) VALUES (?1, 1)", [&folder])?;
            report.folder = Some(folder);
        }
    } else {
        for (_, file, _) in &kept_files {
            if fs::remove_file(file).is_ok() {
                report.deleted += 1;
            }
        }
    }
    report.removed =
        tx.execute("DELETE FROM tracks WHERE id IN (SELECT track_id FROM remote_tracks WHERE source_id = ?1)", [id])?;
    tx.execute("DELETE FROM sources WHERE id = ?1", [id])?;
    tx.commit()?;
    let _ = secrets::delete(conn, &secret_key(id));
    let _ = secrets::delete(conn, &cert_key(id));
    forget_client(id);
    if let Some(x) = CTX.get() {
        let _ = fs::remove_file(x.data_dir.join("certs").join(format!("{id}.p12")));
    }
    if let Ok(c) = ctx() {
        let _ = fs::remove_dir_all(c.data_dir.join("offline").join(id.to_string()));
    }
    Ok(report)
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveReport {
    pub kept: usize,
    pub deleted: usize,
    pub removed: usize,
    pub folder: Option<String>,
}

impl Track {
    fn album_artist_or_artist(&self) -> String {
        if self.album_artist.is_empty() { self.artist.clone() } else { self.album_artist.clone() }
    }
}

fn safe(s: &str) -> String {
    let s: String = s.chars().map(|c| if "/\\:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() { "Unknown".into() } else { s.chars().take(120).collect() }
}

fn file_name(t: &Track, file: &str) -> String {
    let ext = Path::new(file).extension().and_then(|e| e.to_str()).unwrap_or("bin");
    let title = if t.title.is_empty() { "Untitled".to_string() } else { safe(&t.title) };
    match t.track_no {
        Some(n) => format!("{n:02} {title}.{ext}"),
        None => format!("{title}.{ext}"),
    }
}

fn unique_path(p: &Path) -> PathBuf {
    if !p.exists() {
        return p.to_path_buf();
    }
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
    (2..).map(|n| p.with_file_name(format!("{stem} ({n}).{ext}"))).find(|q| !q.exists()).unwrap_or_else(|| p.into())
}

pub struct Pulled {
    pub songs: Vec<Song>,
    pub playlists: Vec<Playlist>,
}

pub fn pull(client: &Client, progress: &dyn Fn(usize)) -> Result<Pulled> {
    let mut songs = Vec::new();
    loop {
        let page = client.songs_page(songs.len(), PAGE)?;
        let n = page.len();
        songs.extend(page);
        progress(songs.len());
        if n < PAGE {
            break;
        }
    }
    if songs.is_empty() {
        let mut offset = 0;
        loop {
            let ids = client.album_ids_page(offset, PAGE)?;
            for id in &ids {
                songs.extend(client.album(id)?.song);
                progress(songs.len());
            }
            if ids.len() < PAGE {
                break;
            }
            offset += ids.len();
        }
    }
    songs.retain(|s| !s.is_video.unwrap_or(false) && !s.id.is_empty());
    let mut playlists = Vec::new();
    for p in client.playlists()? {
        playlists.push(client.playlist(&p.id)?);
    }
    Ok(Pulled { songs, playlists })
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub added: usize,
    pub removed: usize,
    pub tracks: usize,
    pub playlists: usize,
}

pub fn path_for(source: i64, remote_id: &str) -> String {
    format!("{SCHEME}{source}/{remote_id}")
}

pub fn parse_path(path: &str) -> Option<(i64, &str)> {
    let (sid, rid) = path.strip_prefix(SCHEME)?.split_once('/')?;
    Some((sid.parse().ok()?, rid))
}

pub fn is_remote(path: &str) -> bool {
    path.starts_with(SCHEME)
}

fn to_track(source: i64, s: &Song) -> Track {
    let pick = |a: &Option<String>, b: &str| a.clone().filter(|x| !x.is_empty()).unwrap_or_else(|| b.to_string());
    Track {
        id: 0,
        path: path_for(source, &s.id),
        title: s.title.clone(),
        artist: pick(&s.display_artist, &s.artist),
        album_artist: s.display_album_artist.clone().unwrap_or_default(),
        album: s.album.clone(),
        genre: s.genre.clone().unwrap_or_default(),
        composer: s.display_composer.clone().unwrap_or_default(),
        comment: s.comment.clone().unwrap_or_default(),
        year: s.year.filter(|y| *y > 0),
        track_no: s.track.filter(|n| *n > 0),
        disc_no: s.disc_number.filter(|n| *n > 0),
        duration: s.duration.unwrap_or(0.0),
        bitrate: s.bit_rate.filter(|b| *b > 0),
        sample_rate: s.sampling_rate.filter(|r| *r > 0),
        format: s.suffix.clone().unwrap_or_default().to_ascii_uppercase(),
        rating: 0,
        play_count: 0,
        last_played: None,
        date_added: s.created.as_deref().and_then(parse_time).unwrap_or_else(now_secs),
        mtime: 0,
        has_art: s.cover_art.is_some(),
        loved: false,
        inbox: false,
        bit_depth: s.bit_depth.filter(|b| *b > 0),
        rg: Default::default(),
    }
}

pub fn apply(conn: &mut Connection, source: i64, pulled: &Pulled) -> Result<SyncReport> {
    let tx = conn.transaction()?;
    let mut report = SyncReport::default();
    let known: HashMap<String, (i64, String)> = {
        let mut stmt = tx.prepare("SELECT remote_id, track_id, state FROM remote_tracks WHERE source_id = ?1")?;
        let rows = stmt.query_map([source], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    // Changes still on their way to the server win over what it just said.
    let waiting: HashMap<(String, String), i64> = {
        let mut stmt = tx.prepare("SELECT kind, target, COUNT(*) FROM remote_outbox WHERE source_id = ?1 GROUP BY kind, target")?;
        let rows = stmt.query_map([source], |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let pending = |kind: &str, id: &str| waiting.get(&(kind.to_string(), id.to_string())).copied().unwrap_or(0);
    let mut seen = HashSet::new();
    let mut ids: HashMap<String, i64> = HashMap::new();
    {
        let mut state = tx.prepare(
            "UPDATE tracks SET
                rating = CASE WHEN ?2 THEN rating ELSE ?3 END,
                loved = CASE WHEN ?4 THEN loved ELSE ?5 END,
                play_count = ?6,
                last_played = CASE WHEN ?7 IS NULL THEN last_played ELSE MAX(COALESCE(last_played, 0), ?7) END
             WHERE id = ?1",
        )?;
        let mut link = tx.prepare(
            "INSERT INTO remote_tracks (track_id, source_id, remote_id, album_id, cover_id, suffix, size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(track_id) DO UPDATE SET album_id = excluded.album_id, cover_id = excluded.cover_id,
                suffix = excluded.suffix, size = excluded.size,
                state = CASE WHEN state = 'local-only' THEN 'pinned' ELSE state END",
        )?;
        for s in &pulled.songs {
            if !seen.insert(s.id.clone()) {
                continue;
            }
            let id = db::upsert_track(&tx, &to_track(source, s))?;
            if !known.contains_key(&s.id) {
                report.added += 1;
            }
            let rating = s.user_rating.unwrap_or(0).min(5) * 2;
            let plays = s.play_count.unwrap_or(0) as i64 + pending("play", &s.id);
            let played = s.played.as_deref().and_then(parse_time);
            state.execute(params![
                id,
                pending("rating", &s.id) > 0,
                rating,
                pending("love", &s.id) > 0,
                s.starred.is_some(),
                plays,
                played
            ])?;
            link.execute(params![id, source, s.id, s.album_id, s.cover_art, s.suffix.clone().unwrap_or_default(), s.size])?;
            ids.insert(s.id.clone(), id);
        }
    }
    for (rid, (track_id, state)) in &known {
        if seen.contains(rid) {
            continue;
        }
        if state == "pinned" || state == "local-only" {
            tx.execute("UPDATE remote_tracks SET state = 'local-only' WHERE track_id = ?1", [track_id])?;
        } else {
            tx.execute("DELETE FROM tracks WHERE id = ?1", [track_id])?;
            report.removed += 1;
        }
    }
    report.playlists = apply_playlists(&tx, source, &pulled.playlists, &ids, &waiting)?;
    tx.execute(
        "UPDATE remote_tracks SET state = 'downloading'
         WHERE source_id = ?1 AND state = 'remote'
           AND album_id IN (SELECT album_id FROM remote_pins WHERE source_id = ?1)",
        [source],
    )?;
    tx.execute("UPDATE sources SET last_sync = ?2, error = NULL WHERE id = ?1", params![source, now_secs()])?;
    tx.commit()?;
    report.tracks = seen.len();
    Ok(report)
}

fn apply_playlists(
    tx: &Transaction,
    source: i64,
    lists: &[Playlist],
    ids: &HashMap<String, i64>,
    waiting: &HashMap<(String, String), i64>,
) -> Result<usize> {
    let linked: HashMap<String, i64> = {
        let mut stmt = tx.prepare("SELECT remote_id, playlist_id FROM remote_playlists WHERE source_id = ?1")?;
        let rows = stmt.query_map([source], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut seen = HashSet::new();
    for p in lists {
        seen.insert(p.id.clone());
        let local = match linked.get(&p.id) {
            Some(&id) => {
                if waiting.contains_key(&("playlist".to_string(), id.to_string())) {
                    continue;
                }
                tx.execute("UPDATE playlists SET name = ?2 WHERE id = ?1", params![id, p.name])?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO playlists (name, kind, created) VALUES (?1, 'manual', ?2)",
                    params![p.name, p.changed.as_deref().and_then(parse_time).unwrap_or_else(now_secs)],
                )?;
                let id = tx.last_insert_rowid();
                tx.execute("INSERT INTO remote_playlists (playlist_id, source_id, remote_id) VALUES (?1, ?2, ?3)", params![
                    id, source, p.id
                ])?;
                id
            }
        };
        tx.execute("DELETE FROM playlist_tracks WHERE playlist_id = ?1", [local])?;
        let mut ins = tx.prepare_cached("INSERT INTO playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)")?;
        for (pos, tid) in p.entry.iter().filter_map(|s| ids.get(&s.id)).enumerate() {
            ins.execute(params![local, tid, pos as i64])?;
        }
    }
    for (rid, local) in &linked {
        if !seen.contains(rid) && !waiting.contains_key(&("playlist".to_string(), local.to_string())) {
            tx.execute("DELETE FROM playlists WHERE id = ?1", [local])?;
        }
    }
    Ok(seen.len())
}

pub fn states(conn: &Connection) -> Result<(Vec<i64>, Vec<String>)> {
    let mut stmt = conn.prepare("SELECT track_id, state FROM remote_tracks")?;
    let mut ids = Vec::new();
    let mut st = Vec::new();
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (i, s) = row?;
        ids.push(i);
        st.push(s);
    }
    Ok((ids, st))
}

fn remote_of(conn: &Connection, track_id: i64) -> Result<Option<(i64, String)>> {
    Ok(conn
        .query_row("SELECT source_id, remote_id FROM remote_tracks WHERE track_id = ?1", [track_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?)
}

fn queue(conn: &Connection, source: i64, kind: &str, target: &str, value: i64, replace: bool) -> Result<()> {
    if replace {
        conn.execute("DELETE FROM remote_outbox WHERE source_id = ?1 AND kind = ?2 AND target = ?3", params![source, kind, target])?;
    }
    conn.execute(
        "INSERT INTO remote_outbox (source_id, kind, target, value, at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![source, kind, target, value, now_secs()],
    )?;
    Ok(())
}

pub fn note_play(conn: &Connection, track_id: i64, when: i64) -> Result<()> {
    if let Some((source, rid)) = remote_of(conn, track_id)? {
        queue(conn, source, "play", &rid, when, false)?;
        poke();
    }
    Ok(())
}

pub fn queue_love(conn: &Connection, ids: &[i64], loved: bool) -> Result<()> {
    let mut any = false;
    for id in ids {
        if let Some((source, rid)) = remote_of(conn, *id)? {
            queue(conn, source, "love", &rid, loved as i64, true)?;
            any = true;
        }
    }
    if any {
        poke();
    }
    Ok(())
}

/// Subsonic ratings are whole stars, so half stars round up (C3), here too.
pub fn queue_rating(conn: &Connection, ids: &[i64], rating: u8) -> Result<()> {
    let stars = rating.min(10).div_ceil(2);
    let mut any = false;
    for id in ids {
        if let Some((source, rid)) = remote_of(conn, *id)? {
            queue(conn, source, "rating", &rid, stars as i64, true)?;
            conn.execute("UPDATE tracks SET rating = ?2 WHERE id = ?1", params![id, stars * 2])?;
            any = true;
        }
    }
    if any {
        poke();
    }
    Ok(())
}

fn playlist_source(conn: &Connection, playlist: i64) -> Result<Option<i64>> {
    if let Some(s) = conn.query_row("SELECT source_id FROM remote_playlists WHERE playlist_id = ?1", [playlist], |r| r.get(0)).optional()? {
        return Ok(Some(s));
    }
    let kind: Option<String> = conn.query_row("SELECT kind FROM playlists WHERE id = ?1", [playlist], |r| r.get(0)).optional()?;
    if kind.as_deref() != Some("manual") {
        return Ok(None);
    }
    let (n, local, sources, source): (i64, i64, i64, Option<i64>) = conn.query_row(
        "SELECT COUNT(*), SUM(r.track_id IS NULL), COUNT(DISTINCT r.source_id), MIN(r.source_id)
         FROM playlist_tracks p LEFT JOIN remote_tracks r ON r.track_id = p.track_id WHERE p.playlist_id = ?1",
        [playlist],
        |r| Ok((r.get(0)?, r.get::<_, Option<i64>>(1)?.unwrap_or(0), r.get(2)?, r.get(3)?)),
    )?;
    Ok(if n > 0 && local == 0 && sources == 1 { source } else { None })
}

pub fn queue_playlist(conn: &Connection, playlist: i64) -> Result<()> {
    if let Some(source) = playlist_source(conn, playlist)? {
        queue(conn, source, "playlist", &playlist.to_string(), 0, true)?;
        poke();
    }
    Ok(())
}

pub fn queue_playlist_delete(conn: &Connection, playlist: i64) -> Result<()> {
    let linked: Option<(i64, String)> = conn
        .query_row("SELECT source_id, remote_id FROM remote_playlists WHERE playlist_id = ?1", [playlist], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    conn.execute("DELETE FROM remote_outbox WHERE kind = 'playlist' AND target = ?1", [playlist.to_string()])?;
    if let Some((source, rid)) = linked {
        queue(conn, source, "playlist-delete", &rid, 0, true)?;
        poke();
    }
    Ok(())
}

struct Waiting {
    id: i64,
    source: i64,
    kind: String,
    target: String,
    value: i64,
}

/// Send what's waiting, oldest first. A change the server refuses is dropped;
/// one that couldn't be sent stops that server's queue until next time.
pub fn flush(conn: &Connection) -> Result<usize> {
    let rows: Vec<Waiting> = {
        let mut stmt = conn.prepare("SELECT id, source_id, kind, target, value FROM remote_outbox ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok(Waiting { id: r.get(0)?, source: r.get(1)?, kind: r.get(2)?, target: r.get(3)?, value: r.get(4)? }))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut sent = 0;
    let mut stopped: HashSet<i64> = HashSet::new();
    for w in rows {
        if stopped.contains(&w.source) {
            continue;
        }
        let result = client(conn, w.source).and_then(|c| send(conn, &c, &w));
        match result {
            Ok(()) => {
                conn.execute("DELETE FROM remote_outbox WHERE id = ?1", [w.id])?;
                set_error(conn, w.source, None)?;
                sent += 1;
            }
            Err(e) if e.to_string().starts_with("The server refused") => {
                eprintln!("mirror: dropping {} {}: {e}", w.kind, w.target);
                conn.execute("DELETE FROM remote_outbox WHERE id = ?1", [w.id])?;
            }
            Err(e) => {
                set_error(conn, w.source, Some(&e.to_string()))?;
                stopped.insert(w.source);
            }
        }
    }
    Ok(sent)
}

fn send(conn: &Connection, c: &crate::subsonic::Client, w: &Waiting) -> Result<()> {
    match w.kind.as_str() {
        "play" => c.scrobble(&w.target, w.value),
        "love" => c.star(&w.target, w.value != 0),
        "rating" => c.set_rating(&w.target, w.value as u8),
        "playlist-delete" => c.delete_playlist(&w.target),
        "playlist" => {
            let local: i64 = w.target.parse()?;
            let Some(name) = conn.query_row("SELECT name FROM playlists WHERE id = ?1", [local], |r| r.get::<_, String>(0)).optional()? else {
                return Ok(());
            };
            let songs: Vec<String> = {
                let mut stmt = conn.prepare(
                    "SELECT r.remote_id FROM playlist_tracks p JOIN remote_tracks r ON r.track_id = p.track_id
                     WHERE p.playlist_id = ?1 AND r.source_id = ?2 ORDER BY p.position",
                )?;
                let rows = stmt.query_map(params![local, w.source], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let linked: Option<String> =
                conn.query_row("SELECT remote_id FROM remote_playlists WHERE playlist_id = ?1", [local], |r| r.get(0)).optional()?;
            let rid = c.save_playlist(linked.as_deref(), &name, &songs)?;
            if linked.is_none() {
                conn.execute("INSERT OR REPLACE INTO remote_playlists (playlist_id, source_id, remote_id) VALUES (?1, ?2, ?3)", params![
                    local, w.source, rid
                ])?;
            }
            Ok(())
        }
        k => bail!("The server refused an unknown change ({k})"),
    }
}

pub fn pin(conn: &Connection, ids: &[i64], albums: bool) -> Result<usize> {
    if albums {
        for id in ids {
            conn.execute(
                "INSERT OR IGNORE INTO remote_pins (source_id, album_id)
                 SELECT source_id, album_id FROM remote_tracks WHERE track_id = ?1 AND album_id IS NOT NULL",
                [id],
            )?;
        }
        conn.execute(
            "UPDATE remote_tracks SET state = 'downloading' WHERE state = 'remote'
               AND EXISTS (SELECT 1 FROM remote_pins p WHERE p.source_id = remote_tracks.source_id AND p.album_id = remote_tracks.album_id)",
            [],
        )?;
    }
    let mut stmt = conn.prepare_cached("UPDATE remote_tracks SET state = 'downloading' WHERE track_id = ?1 AND state = 'remote'")?;
    for id in ids {
        stmt.execute([id])?;
    }
    poke();
    Ok(conn.query_row("SELECT COUNT(*) FROM remote_tracks WHERE state = 'downloading'", [], |r| r.get::<_, i64>(0))? as usize)
}

pub fn unpin(conn: &Connection, ids: &[i64]) -> Result<()> {
    for id in ids {
        let row: Option<(Option<String>, i64, Option<String>)> = conn
            .query_row("SELECT file, source_id, album_id FROM remote_tracks WHERE track_id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?;
        let Some((file, source, album)) = row else { continue };
        conn.execute("DELETE FROM remote_pins WHERE source_id = ?1 AND album_id = ?2", params![source, album])?;
        let state: String = conn.query_row("SELECT state FROM remote_tracks WHERE track_id = ?1", [id], |r| r.get(0))?;
        if state == "local-only" {
            conn.execute("DELETE FROM tracks WHERE id = ?1", [id])?;
        } else {
            conn.execute("UPDATE remote_tracks SET state = 'remote', file = NULL WHERE track_id = ?1", [id])?;
        }
        if let Some(f) = file {
            let _ = fs::remove_file(f);
        }
    }
    Ok(())
}

pub fn download_next(conn: &Connection) -> Result<Option<i64>> {
    let next: Option<(i64, i64, String, String)> = conn
        .query_row(
            "SELECT track_id, source_id, remote_id, suffix FROM remote_tracks WHERE state = 'downloading' ORDER BY track_id LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((track_id, source, rid, suffix)) = next else { return Ok(None) };
    let c = ctx()?;
    let dest = c.data_dir.join("offline").join(source.to_string()).join(format!("{}.{}", safe(&rid), ext_or(&suffix)));
    fs::create_dir_all(dest.parent().unwrap_or(&c.data_dir))?;
    let cached = stream_cache_path(source, &rid, &suffix, None);
    if fs::rename(&cached, &dest).is_err() || !dest.is_file() {
        let client = client(conn, source)?;
        let resp = client.download(&rid).call().map_err(|e| anyhow!("Downloading failed: {}", crate::subsonic::describe(&e)))?;
        if resp.content_type().contains("json") {
            conn.execute("UPDATE remote_tracks SET state = 'remote' WHERE track_id = ?1", [track_id])?;
            bail!("The server refused to send {rid}");
        }
        let part = dest.with_extension("part");
        let mut out = fs::File::create(&part)?;
        std::io::copy(&mut resp.into_reader(), &mut out).context("The download broke off")?;
        fs::rename(&part, &dest)?;
    }
    conn.execute(
        "UPDATE remote_tracks SET state = 'pinned', file = ?2 WHERE track_id = ?1 AND state = 'downloading'",
        params![track_id, dest.to_string_lossy()],
    )?;
    Ok(Some(track_id))
}

fn ext_or(suffix: &str) -> &str {
    if suffix.is_empty() { "bin" } else { suffix }
}

fn transcode(conn: &Connection) -> Option<(String, u32)> {
    let v: String = conn.query_row("SELECT value FROM settings WHERE key = 'serverStream'", [], |r| r.get(0)).optional().ok()??;
    let (fmt, kbps) = v.split_once('-')?;
    Some((fmt.to_string(), kbps.parse().ok()?))
}

fn stream_cache_path(source: i64, rid: &str, suffix: &str, transcode: Option<&(String, u32)>) -> PathBuf {
    let dir = CTX.get().map(|c| c.data_dir.join("stream-cache")).unwrap_or_default();
    match transcode {
        Some((fmt, kbps)) => dir.join(format!("{source}-{}-{fmt}{kbps}.{fmt}", safe(rid))),
        None => dir.join(format!("{source}-{}.{}", safe(rid), ext_or(suffix))),
    }
}

struct Playable {
    source: i64,
    rid: String,
    suffix: String,
    file: Option<String>,
}

fn playable(conn: &Connection, path: &str) -> Result<Playable> {
    let (source, rid) = parse_path(path).ok_or_else(|| anyhow!("not a server song"))?;
    let (suffix, file) = conn
        .query_row("SELECT suffix, file FROM remote_tracks WHERE source_id = ?1 AND remote_id = ?2", params![source, rid], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })
        .optional()?
        .ok_or_else(|| anyhow!("This song isn't in the library any more"))?;
    Ok(Playable { source, rid: rid.to_string(), suffix, file })
}

/// Kaseki's opener for server songs: the offline copy when there is one, else
/// a stream that plays while it downloads.
pub fn resolve(path: &str) -> Option<Result<(Box<dyn symphonia::core::io::MediaSource>, String), String>> {
    if !is_remote(path) {
        return None;
    }
    Some(open_audio(path).map_err(|e| format!("{e:#}")))
}

fn open_audio(path: &str) -> Result<(Box<dyn symphonia::core::io::MediaSource>, String)> {
    let c = ctx()?;
    let conn = db::open(&c.db_path)?;
    let p = playable(&conn, path)?;
    if let Some(f) = p.file.as_deref().and_then(|f| fs::File::open(f).ok()) {
        return Ok((Box::new(f), p.suffix.to_ascii_lowercase()));
    }
    let tc = transcode(&conn);
    let cache = stream_cache_path(p.source, &p.rid, &p.suffix, tc.as_ref());
    let client = client(&conn, p.source)?;
    drop(conn);
    if !cache.is_file() {
        stream::trim(&c.data_dir.join("stream-cache"), STREAM_CACHE_MAX);
    }
    let ext = match &tc {
        Some((fmt, _)) => fmt.clone(),
        None => p.suffix.to_ascii_lowercase(),
    };
    let opened = stream::open(&cache, || client.stream(&p.rid, tc.as_ref().map(|(f, k)| (f.as_str(), *k))))?;
    Ok(match opened {
        Opened::File(f) => (Box::new(f), ext),
        Opened::Stream(r) => (Box::new(r), ext),
    })
}

pub fn prefetch<'a>(paths: impl IntoIterator<Item = &'a str>) {
    let paths: Vec<String> = paths.into_iter().filter(|p| is_remote(p)).take(2).map(String::from).collect();
    if paths.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let Ok(c) = ctx() else { return };
        let Ok(conn) = db::open(&c.db_path) else { return };
        for path in paths {
            let Ok(p) = playable(&conn, &path) else { continue };
            if p.file.as_deref().is_some_and(|f| Path::new(f).is_file()) {
                continue;
            }
            let tc = transcode(&conn);
            let Ok(client) = client(&conn, p.source) else { continue };
            let cache = stream_cache_path(p.source, &p.rid, &p.suffix, tc.as_ref());
            stream::prefetch(&cache, || client.stream(&p.rid, tc.as_ref().map(|(f, k)| (f.as_str(), *k))));
        }
    });
}

pub fn cover(conn: &Connection, track_id: i64) -> Option<(Vec<u8>, String)> {
    let (source, cover_id, file): (i64, Option<String>, Option<String>) = conn
        .query_row("SELECT source_id, cover_id, file FROM remote_tracks WHERE track_id = ?1", [track_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .ok()?;
    let from_file = || file.as_deref().and_then(|f| crate::meta::artwork(Path::new(f))).map(|(d, m)| (d, m.to_string()));
    let Some(cover_id) = cover_id else { return from_file() };
    let cached = ctx().ok()?.data_dir.join("covers").join(format!("{source}-{}", safe(&cover_id)));
    if let Ok(data) = fs::read(&cached) {
        let mime = sniff(&data);
        return Some((data, mime));
    }
    let fetched = client(conn, source).ok().and_then(|c| {
        let resp = c.cover(&cover_id, 600).timeout(Duration::from_secs(20)).call().ok()?;
        if !resp.content_type().starts_with("image/") {
            return None;
        }
        let mut data = Vec::new();
        resp.into_reader().take(16 * 1024 * 1024).read_to_end(&mut data).ok()?;
        Some(data)
    });
    match fetched {
        Some(data) => {
            let _ = fs::create_dir_all(cached.parent()?);
            let _ = fs::write(&cached, &data);
            let mime = sniff(&data);
            Some((data, mime))
        }
        None => from_file(),
    }
}

pub fn cover_file(track_id: i64) -> Option<PathBuf> {
    let conn = db::open(&ctx().ok()?.db_path).ok()?;
    let (data, _) = cover(&conn, track_id)?;
    let path = ctx().ok()?.data_dir.join("covers").join(format!("track-{track_id}"));
    fs::write(&path, data).ok()?;
    Some(path)
}

fn sniff(data: &[u8]) -> String {
    match data {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        [b'G', b'I', b'F', ..] => "image/gif",
        _ => "image/jpeg",
    }
    .into()
}

pub fn local_file(conn: &Connection, track: &Track) -> Option<PathBuf> {
    if !is_remote(&track.path) {
        return Some(PathBuf::from(&track.path));
    }
    let file: Option<String> = conn.query_row("SELECT file FROM remote_tracks WHERE track_id = ?1", [track.id], |r| r.get(0)).ok()?;
    file.map(PathBuf::from).filter(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsonic::tests::{fake, ok};
    use std::sync::Arc;

    fn library() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        db::migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        conn.execute("INSERT INTO sources (id, name, url, username) VALUES (1, 'Test', 'http://x', 'u')", []).unwrap();
        conn
    }

    fn song(id: &str, title: &str, album: &str) -> Song {
        Song {
            id: id.into(),
            title: title.into(),
            album: album.into(),
            artist: "Artist".into(),
            album_id: Some(format!("al-{album}")),
            suffix: Some("flac".into()),
            duration: Some(100.0),
            cover_art: Some(format!("mf-{id}")),
            ..Default::default()
        }
    }

    fn pulled(songs: Vec<Song>) -> Pulled {
        Pulled { songs, playlists: vec![] }
    }

    fn track(conn: &Connection, rid: &str) -> Option<Track> {
        let id = db::track_id_by_path(conn, &path_for(1, rid)).unwrap()?;
        db::track_by_id(conn, id).unwrap()
    }

    #[test]
    fn songs_become_tracks_with_the_servers_state() {
        let mut conn = library();
        let mut a = song("a", "One", "Al");
        a.user_rating = Some(4);
        a.starred = Some("2026-01-01T00:00:00Z".into());
        a.play_count = Some(7);
        a.track = Some(3);
        a.display_album_artist = Some("Various Artists".into());
        a.bit_depth = Some(0);
        a.created = Some("2026-11-09T09:18:38+02:00".into());
        let r = apply(&mut conn, 1, &pulled(vec![a, song("b", "Two", "Al")])).unwrap();
        assert_eq!(r, SyncReport { added: 2, removed: 0, tracks: 2, playlists: 0 });
        let t = track(&conn, "a").unwrap();
        assert_eq!((t.title.as_str(), t.rating, t.loved, t.play_count, t.track_no), ("One", 8, true, 7, Some(3)));
        assert_eq!((t.album_artist.as_str(), t.format.as_str(), t.bit_depth, t.date_added), ("Various Artists", "FLAC", None, 1794208718));
        assert!(t.has_art && !t.inbox);
        assert_eq!(states(&conn).unwrap().1, ["remote", "remote"]);
    }

    #[test]
    fn a_new_sync_keeps_ids_and_drops_what_the_server_lost() {
        let mut conn = library();
        apply(&mut conn, 1, &pulled(vec![song("a", "One", "Al"), song("b", "Two", "Al"), song("c", "Three", "Al")])).unwrap();
        let a = track(&conn, "a").unwrap().id;
        let c = track(&conn, "c").unwrap().id;
        conn.execute("UPDATE remote_tracks SET state = 'pinned', file = '/x/c.flac' WHERE track_id = ?1", [c]).unwrap();
        let r = apply(&mut conn, 1, &pulled(vec![song("a", "One (Remastered)", "Al")])).unwrap();
        assert_eq!((r.added, r.removed), (0, 1));
        let t = track(&conn, "a").unwrap();
        assert_eq!((t.id, t.title.as_str()), (a, "One (Remastered)"));
        assert!(track(&conn, "b").is_none());
        let st: String = conn.query_row("SELECT state FROM remote_tracks WHERE track_id = ?1", [c], |r| r.get(0)).unwrap();
        assert_eq!(st, "local-only", "an offline copy outlives the server's");
        apply(&mut conn, 1, &pulled(vec![song("a", "One", "Al"), song("c", "Three", "Al")])).unwrap();
        let st: String = conn.query_row("SELECT state FROM remote_tracks WHERE track_id = ?1", [c], |r| r.get(0)).unwrap();
        assert_eq!(st, "pinned");
    }

    #[test]
    fn changes_still_waiting_win_over_the_pull() {
        let mut conn = library();
        apply(&mut conn, 1, &pulled(vec![song("a", "One", "Al")])).unwrap();
        let id = track(&conn, "a").unwrap().id;
        queue_rating(&conn, &[id], 7).unwrap();
        conn.execute("UPDATE tracks SET loved = 1 WHERE id = ?1", [id]).unwrap();
        queue_love(&conn, &[id], true).unwrap();
        note_play(&conn, id, 100).unwrap();
        note_play(&conn, id, 200).unwrap();
        let mut a = song("a", "One", "Al");
        a.play_count = Some(5);
        apply(&mut conn, 1, &pulled(vec![a])).unwrap();
        let t = track(&conn, "a").unwrap();
        assert_eq!((t.rating, t.loved, t.play_count), (8, true, 7), "3.5 stars rounds up to 4");
    }

    #[test]
    fn the_scanner_leaves_server_songs_alone() {
        let dir = std::env::temp_dir().join(format!("honeycomb-mirror-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("library.db");
        let mut conn = db::open(&db_path).unwrap();
        db::migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        conn.execute("INSERT INTO sources (id, name, url, username) VALUES (1, 'Test', 'http://x', 'u')", []).unwrap();
        apply(&mut conn, 1, &pulled(vec![song("a", "One", "Al")])).unwrap();
        crate::scanner::scan(&db_path, &|_| {}).unwrap();
        assert!(track(&conn, "a").is_some());
    }

    #[test]
    fn the_outbox_sends_coalesces_and_retries() {
        let mut conn = library();
        let fail = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let f2 = fail.clone();
        let (url, log) = fake(Arc::new(move |ep, _| {
            if f2.load(std::sync::atomic::Ordering::SeqCst) {
                return (503, String::new());
            }
            match ep {
                "createPlaylist" => (200, ok(r#""playlist":{"id":"pl-9","name":"Mix"}"#)),
                _ => (200, ok("")),
            }
        }));
        conn.execute("UPDATE sources SET url = ?1 WHERE id = 1", [&url]).unwrap();
        secrets::set(&conn, &secret_key(1), "pw").unwrap();
        forget_client(1);
        apply(&mut conn, 1, &pulled(vec![song("a", "One", "Al"), song("b", "Two", "Al")])).unwrap();
        let (a, b) = (track(&conn, "a").unwrap().id, track(&conn, "b").unwrap().id);
        note_play(&conn, a, 1000).unwrap();
        queue_love(&conn, &[a], true).unwrap();
        queue_love(&conn, &[a], false).unwrap();
        queue_rating(&conn, &[b], 3).unwrap();
        conn.execute("INSERT INTO playlists (id, name, kind, created) VALUES (5, 'Mix', 'manual', 0)", []).unwrap();
        db::set_playlist_track_ids(&mut conn, 5, &[b, a]).unwrap();
        queue_playlist(&conn, 5).unwrap();
        assert_eq!(flush(&conn).unwrap(), 0);
        assert_eq!(log.lock().unwrap().len(), 1, "a failure stops that server's queue");
        let err: Option<String> = conn.query_row("SELECT error FROM sources WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert!(err.unwrap().contains("503"));
        fail.store(false, std::sync::atomic::Ordering::SeqCst);
        log.lock().unwrap().clear();
        assert_eq!(flush(&conn).unwrap(), 4);
        let log = log.lock().unwrap();
        let eps: Vec<&str> = log.iter().map(|(e, _)| e.as_str()).collect();
        assert_eq!(eps, ["scrobble", "unstar", "setRating", "createPlaylist"]);
        assert_eq!(log[0].1["time"][0], "1000000");
        assert_eq!(log[2].1["rating"][0], "2");
        assert_eq!(log[3].1["songId"], ["b", "a"]);
        let rid: String = conn.query_row("SELECT remote_id FROM remote_playlists WHERE playlist_id = 5", [], |r| r.get(0)).unwrap();
        assert_eq!(rid, "pl-9");
        let left: i64 = conn.query_row("SELECT COUNT(*) FROM remote_outbox", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn server_playlists_are_mirrored() {
        let mut conn = library();
        let p = Playlist { id: "p1".into(), name: "Road".into(), entry: vec![song("b", "Two", "Al"), song("a", "One", "Al")], ..Default::default() };
        let mut pl = pulled(vec![song("a", "One", "Al"), song("b", "Two", "Al")]);
        pl.playlists = vec![p];
        apply(&mut conn, 1, &pl).unwrap();
        let local: i64 = conn.query_row("SELECT playlist_id FROM remote_playlists WHERE remote_id = 'p1'", [], |r| r.get(0)).unwrap();
        let ids = db::manual_playlist_track_ids(&conn, local).unwrap();
        assert_eq!(ids, [track(&conn, "b").unwrap().id, track(&conn, "a").unwrap().id]);
        pl.playlists.clear();
        apply(&mut conn, 1, &pl).unwrap();
        assert!(db::playlist(&conn, local).unwrap().is_none());
    }

    #[test]
    fn pinned_albums_take_in_new_songs() {
        let mut conn = library();
        apply(&mut conn, 1, &pulled(vec![song("a", "One", "Al"), song("x", "Other", "B")])).unwrap();
        let a = track(&conn, "a").unwrap().id;
        pin(&conn, &[a], true).unwrap();
        conn.execute("UPDATE remote_tracks SET state = 'pinned' WHERE track_id = ?1", [a]).unwrap();
        apply(&mut conn, 1, &pulled(vec![song("a", "One", "Al"), song("n", "New", "Al"), song("x", "Other", "B")])).unwrap();
        let st = |rid: &str| -> String {
            conn.query_row("SELECT state FROM remote_tracks WHERE remote_id = ?1", [rid], |r| r.get(0)).unwrap()
        };
        assert_eq!((st("a").as_str(), st("n").as_str(), st("x").as_str()), ("pinned", "downloading", "remote"));
    }

    #[test]
    fn removing_a_server_can_keep_its_offline_copies() {
        let base = std::env::temp_dir().join(format!("honeycomb-mirror-keep-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let mut conn = library();
        let mut a = song("a", "One/Two", "Al");
        a.track = Some(1);
        apply(&mut conn, 1, &pulled(vec![a, song("b", "Two", "Al")])).unwrap();
        let id = track(&conn, "a").unwrap().id;
        let file = base.join("a.flac");
        fs::write(&file, b"audio").unwrap();
        conn.execute("UPDATE remote_tracks SET state = 'pinned', file = ?2 WHERE track_id = ?1", params![id, file.to_string_lossy()]).unwrap();
        conn.execute("UPDATE tracks SET play_count = 9 WHERE id = ?1", [id]).unwrap();
        let keep = base.join("Kept");
        let r = remove_source(&mut conn, 1, Some(&keep)).unwrap();
        assert_eq!((r.kept, r.removed), (1, 1));
        let t = db::track_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(Path::new(&t.path), keep.join("Artist/Al/01 One_Two.flac"));
        assert_eq!(t.play_count, 9);
        assert_eq!(fs::read(&t.path).unwrap(), b"audio");
        assert!(db::folders(&conn).unwrap().contains(&keep.to_string_lossy().to_string()));
        assert_eq!(db::all_tracks(&conn).unwrap().len(), 1);
    }

    #[test]
    fn paths_round_trip() {
        assert_eq!(parse_path(&path_for(12, "abc-1")), Some((12, "abc-1")));
        assert_eq!(parse_path("/music/a.mp3"), None);
    }

    /// Against a real server, by default the throwaway test Navidrome:
    /// `HONEYCOMB_NAVIDROME=http://127.0.0.1:4533 cargo test --lib live_navidrome -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_navidrome() {
        let Ok(url) = std::env::var("HONEYCOMB_NAVIDROME") else { return };
        let user = std::env::var("HONEYCOMB_NAVIDROME_USER").unwrap_or("honeycomb".into());
        let pass = std::env::var("HONEYCOMB_NAVIDROME_PASSWORD").unwrap_or("honeycomb-test".into());
        let dir = std::env::temp_dir().join(format!("honeycomb-live-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("library.db");
        init(&db_path, &dir);
        let mut conn = db::open(&db_path).unwrap();
        db::migrate(&conn).unwrap();
        migrate(&conn).unwrap();

        let id = add_source(&conn, &url, &user, &pass).unwrap();
        let client = client(&conn, id).unwrap();
        let t = std::time::Instant::now();
        let pulled = pull(&client, &|_| {}).unwrap();
        let r = apply(&mut conn, id, &pulled).unwrap();
        println!("synced {} songs, {} playlists in {:?}", r.tracks, r.playlists, t.elapsed());
        assert!(r.tracks >= 15);
        let tracks = db::all_tracks(&conn).unwrap();
        let by_title = |title: &str| tracks.iter().find(|t| t.title == title).unwrap().clone();
        let m1 = by_title("Movement 1");
        assert_eq!((m1.album.as_str(), m1.album_artist.as_str(), m1.format.as_str(), m1.track_no), ("Continuous Suite", "The Long Hall", "FLAC", Some(1)));
        assert_eq!(by_title("Últimas Luces").artist, "Café Ñandú");

        for t in &tracks {
            let (mut media, ext) = resolve(&t.path).unwrap().unwrap();
            let mut bytes = Vec::new();
            media.read_to_end(&mut bytes).unwrap();
            assert!(bytes.len() > 10_000, "{}: {} bytes", t.title, bytes.len());
            assert_eq!(ext, t.format.to_ascii_lowercase());
        }
        let (art, mime) = cover(&conn, m1.id).unwrap();
        assert!(art.len() > 100 && mime.starts_with("image/"), "{mime}");

        let northern: Vec<i64> = tracks.iter().filter(|t| t.album == "Northern Lights").map(|t| t.id).collect();
        pin(&conn, &northern[..1], true).unwrap();
        while download_next(&conn).unwrap().is_some() {}
        for t in &northern {
            let f = local_file(&conn, &db::track_by_id(&conn, *t).unwrap().unwrap()).expect("offline copy");
            assert!(f.starts_with(dir.join("offline")));
        }

        fn song_of(conn: &Connection, client: &Client, tid: i64) -> Song {
            let rid: String = conn.query_row("SELECT remote_id FROM remote_tracks WHERE track_id = ?1", [tid], |r| r.get(0)).unwrap();
            let album: String = conn.query_row("SELECT album_id FROM remote_tracks WHERE track_id = ?1", [tid], |r| r.get(0)).unwrap();
            client.album(&album).unwrap().song.into_iter().find(|s| s.id == rid).unwrap()
        }
        let before = song_of(&conn, &client, m1.id).play_count.unwrap_or(0);
        note_play(&conn, m1.id, now_secs()).unwrap();
        conn.execute("UPDATE tracks SET loved = 1 WHERE id = ?1", [m1.id]).unwrap();
        queue_love(&conn, &[m1.id], true).unwrap();
        queue_rating(&conn, &[m1.id], 7).unwrap();
        conn.execute("INSERT INTO playlists (name, kind, created) VALUES ('Honeycomb live test', 'manual', 0)", []).unwrap();
        let pl = conn.last_insert_rowid();
        db::set_playlist_track_ids(&mut conn, pl, &[northern[1], m1.id]).unwrap();
        queue_playlist(&conn, pl).unwrap();
        assert_eq!(flush(&conn).unwrap(), 4);
        let s = song_of(&conn, &client, m1.id);
        assert_eq!(s.play_count.unwrap_or(0), before + 1, "the play reached the server");
        assert!(s.starred.is_some(), "the star reached the server");
        assert_eq!(s.user_rating, Some(4));
        let remote = client.playlists().unwrap().into_iter().find(|p| p.name == "Honeycomb live test").expect("playlist on the server");
        assert_eq!(client.playlist(&remote.id).unwrap().entry.len(), 2);

        queue_playlist_delete(&conn, pl).unwrap();
        conn.execute("UPDATE tracks SET loved = 0 WHERE id = ?1", [m1.id]).unwrap();
        queue_love(&conn, &[m1.id], false).unwrap();
        queue_rating(&conn, &[m1.id], 0).unwrap();
        flush(&conn).unwrap();
        assert!(client.playlists().unwrap().iter().all(|p| p.name != "Honeycomb live test"));
        let r = remove_source(&mut conn, id, None).unwrap();
        assert_eq!(r.deleted, northern.len());
        assert!(db::all_tracks(&conn).unwrap().is_empty());
    }

    #[test]
    #[ignore]
    fn live_sync_timing() {
        let Ok(url) = std::env::var("HONEYCOMB_NAVIDROME") else { return };
        let dir = std::env::temp_dir().join(format!("honeycomb-timing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut conn = db::open(&dir.join("library.db")).unwrap();
        db::migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        let id = add_source(&conn, &url, "honeycomb", "honeycomb-test").unwrap();
        let c = client(&conn, id).unwrap();
        let t = std::time::Instant::now();
        let pulled = pull(&c, &|_| {}).unwrap();
        let fetched = t.elapsed();
        let r = apply(&mut conn, id, &pulled).unwrap();
        let first = t.elapsed() - fetched;
        let t = std::time::Instant::now();
        apply(&mut conn, id, &pulled).unwrap();
        let again = t.elapsed();
        let t = std::time::Instant::now();
        let (ids, _) = states(&conn).unwrap();
        let st = t.elapsed();
        println!("{} songs: fetched in {fetched:?}, first apply {first:?}, re-sync apply {again:?}, states for the UI {st:?}", r.tracks);
        assert_eq!(ids.len(), r.tracks);
        remove_source(&mut conn, id, None).unwrap();
    }
}
