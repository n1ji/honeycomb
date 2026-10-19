use std::path::Path;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::models::{Playlist, SmartRules, Track};

pub const TRACK_COLS: &str = "id, path, title, artist, album_artist, album, genre, composer, comment, \
     year, track_no, disc_no, duration, bitrate, sample_rate, format, rating, play_count, \
     last_played, date_added, mtime, has_art, loved";

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(conn)
}

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tracks (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL UNIQUE,
            title TEXT NOT NULL DEFAULT '',
            artist TEXT NOT NULL DEFAULT '',
            album_artist TEXT NOT NULL DEFAULT '',
            album TEXT NOT NULL DEFAULT '',
            genre TEXT NOT NULL DEFAULT '',
            composer TEXT NOT NULL DEFAULT '',
            comment TEXT NOT NULL DEFAULT '',
            year INTEGER,
            track_no INTEGER,
            disc_no INTEGER,
            duration REAL NOT NULL DEFAULT 0,
            bitrate INTEGER,
            sample_rate INTEGER,
            format TEXT NOT NULL DEFAULT '',
            rating INTEGER NOT NULL DEFAULT 0,
            play_count INTEGER NOT NULL DEFAULT 0,
            last_played INTEGER,
            date_added INTEGER NOT NULL,
            mtime INTEGER NOT NULL DEFAULT 0,
            has_art INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS folders (path TEXT PRIMARY KEY);
        CREATE TABLE IF NOT EXISTS playlists (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            kind TEXT NOT NULL DEFAULT 'manual',
            rules TEXT,
            created INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS playlist_tracks (
            playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
            track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
            position INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_pt_playlist ON playlist_tracks(playlist_id, position);
        CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        // v1: ratings move from 0-5 stars to 0-10 half-stars; tracks can be loved.
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE tracks ADD COLUMN loved INTEGER NOT NULL DEFAULT 0;
             UPDATE tracks SET rating = rating * 2;
             PRAGMA user_version = 1;
             COMMIT;",
        )?;
    }
    Ok(())
}

pub fn row_to_track(row: &Row) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get(0)?,
        path: row.get(1)?,
        title: row.get(2)?,
        artist: row.get(3)?,
        album_artist: row.get(4)?,
        album: row.get(5)?,
        genre: row.get(6)?,
        composer: row.get(7)?,
        comment: row.get(8)?,
        year: row.get(9)?,
        track_no: row.get(10)?,
        disc_no: row.get(11)?,
        duration: row.get(12)?,
        bitrate: row.get(13)?,
        sample_rate: row.get(14)?,
        format: row.get(15)?,
        rating: row.get(16)?,
        play_count: row.get(17)?,
        last_played: row.get(18)?,
        date_added: row.get(19)?,
        mtime: row.get(20)?,
        has_art: row.get(21)?,
        loved: row.get(22)?,
    })
}

pub fn all_tracks(conn: &Connection) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare(&format!("SELECT {TRACK_COLS} FROM tracks"))?;
    let rows = stmt.query_map([], row_to_track)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn track_by_id(conn: &Connection, id: i64) -> Result<Option<Track>> {
    Ok(conn
        .query_row(
            &format!("SELECT {TRACK_COLS} FROM tracks WHERE id = ?1"),
            [id],
            row_to_track,
        )
        .optional()?)
}

pub fn tracks_by_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {TRACK_COLS} FROM tracks WHERE id = ?1"))?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(t) = stmt.query_row([id], row_to_track).optional()? {
            out.push(t);
        }
    }
    Ok(out)
}

pub fn track_id_by_path(conn: &Connection, path: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row("SELECT id FROM tracks WHERE path = ?1", [path], |r| r.get(0))
        .optional()?)
}

/// Insert a track or update its file-derived metadata, keeping rating and play stats.
pub fn upsert_track(conn: &Connection, t: &Track) -> Result<i64> {
    conn.execute(
        "INSERT INTO tracks (path, title, artist, album_artist, album, genre, composer, comment,
            year, track_no, disc_no, duration, bitrate, sample_rate, format, date_added, mtime, has_art)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
         ON CONFLICT(path) DO UPDATE SET
            title = excluded.title, artist = excluded.artist, album_artist = excluded.album_artist,
            album = excluded.album, genre = excluded.genre, composer = excluded.composer,
            comment = excluded.comment, year = excluded.year, track_no = excluded.track_no,
            disc_no = excluded.disc_no, duration = excluded.duration, bitrate = excluded.bitrate,
            sample_rate = excluded.sample_rate, format = excluded.format, mtime = excluded.mtime,
            has_art = excluded.has_art",
        params![
            t.path, t.title, t.artist, t.album_artist, t.album, t.genre, t.composer, t.comment,
            t.year, t.track_no, t.disc_no, t.duration, t.bitrate, t.sample_rate, t.format,
            t.date_added, t.mtime, t.has_art
        ],
    )?;
    Ok(track_id_by_path(conn, &t.path)?.unwrap_or_default())
}

pub fn record_play(conn: &Connection, id: i64, when: i64) -> Result<()> {
    conn.execute(
        "UPDATE tracks SET play_count = play_count + 1, last_played = ?2 WHERE id = ?1",
        params![id, when],
    )?;
    Ok(())
}

// ---------- folders ----------

pub fn folders(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM folders ORDER BY path")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------- settings ----------

pub fn settings(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

// ---------- playlists ----------

pub fn playlists(conn: &Connection) -> Result<Vec<Playlist>> {
    let mut stmt = conn.prepare("SELECT id, name, kind, rules FROM playlists ORDER BY name COLLATE NOCASE")?;
    let rows = stmt.query_map([], |r| {
        let rules: Option<String> = r.get(3)?;
        Ok(Playlist {
            id: r.get(0)?,
            name: r.get(1)?,
            kind: r.get(2)?,
            rules: rules.and_then(|s| serde_json::from_str::<SmartRules>(&s).ok()),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn playlist(conn: &Connection, id: i64) -> Result<Option<Playlist>> {
    Ok(playlists(conn)?.into_iter().find(|p| p.id == id))
}

pub fn manual_playlist_track_ids(conn: &Connection, id: i64) -> Result<Vec<i64>> {
    let mut stmt =
        conn.prepare("SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position")?;
    let rows = stmt.query_map([id], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Rewrite a manual playlist's contents in the given order.
pub fn set_playlist_track_ids(conn: &mut Connection, id: i64, ids: &[i64]) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM playlist_tracks WHERE playlist_id = ?1", [id])?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
        )?;
        for (pos, tid) in ids.iter().enumerate() {
            stmt.execute(params![id, tid, pos as i64])?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_migration_doubles_ratings_once() {
        let conn = Connection::open_in_memory().unwrap();
        // A v0 database: the base schema (v1 step skipped) with 0-5 star ratings.
        conn.execute_batch("PRAGMA user_version = 1;").unwrap();
        migrate(&conn).unwrap();
        conn.execute_batch(
            "PRAGMA user_version = 0;
             INSERT INTO tracks (path, rating, date_added) VALUES ('a', 4, 0), ('b', 0, 0);",
        )
        .unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap(); // a second start must not double again
        let ratings: Vec<u8> = conn
            .prepare("SELECT rating FROM tracks ORDER BY path")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(ratings, [8, 0]);
        assert!(!all_tracks(&conn).unwrap()[0].loved);
    }
}
