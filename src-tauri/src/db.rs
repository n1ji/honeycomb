use std::path::Path;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::models::{Playlist, SmartRules, Track};

pub const TRACK_COLS: &str = "id, path, title, artist, album_artist, album, genre, composer, comment, \
     year, track_no, disc_no, duration, bitrate, sample_rate, format, rating, play_count, \
     last_played, date_added, mtime, has_art, loved, inbox, bit_depth";

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
        CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        -- LRCLIB answers; body NULL and instrumental 0 means \"not found\".
        CREATE TABLE IF NOT EXISTS lyrics (
            track_id INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
            body TEXT,
            instrumental INTEGER NOT NULL DEFAULT 0,
            fetched INTEGER NOT NULL
        );",
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
    if version < 2 {
        // v2: an inbox for tracks that turn up in folders already scanned once.
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE tracks ADD COLUMN inbox INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE folders ADD COLUMN scanned INTEGER NOT NULL DEFAULT 0;
             UPDATE folders SET scanned = 1;
             PRAGMA user_version = 2;
             COMMIT;",
        )?;
    }
    if version < 4 {
        // v3 was a scrobble queue during development; scrobbling is a plugin
        // now and keeps its own queue.
        conn.execute_batch("BEGIN; DROP TABLE IF EXISTS scrobbles; PRAGMA user_version = 4; COMMIT;")?;
    }
    if version < 5 {
        // v5: bit depth. Forgetting the mtime of lossless files makes the next
        // scan read them again and fill it in.
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE tracks ADD COLUMN bit_depth INTEGER;
             UPDATE tracks SET mtime = 0
                WHERE upper(format) IN ('FLAC', 'WAV', 'AIF', 'AIFF', 'M4A', 'ALAC', 'APE', 'WV');
             PRAGMA user_version = 5;
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
        inbox: row.get(23)?,
        bit_depth: row.get(24)?,
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
            year, track_no, disc_no, duration, bitrate, sample_rate, format, date_added, mtime, has_art, bit_depth)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
         ON CONFLICT(path) DO UPDATE SET
            title = excluded.title, artist = excluded.artist, album_artist = excluded.album_artist,
            album = excluded.album, genre = excluded.genre, composer = excluded.composer,
            comment = excluded.comment, year = excluded.year, track_no = excluded.track_no,
            disc_no = excluded.disc_no, duration = excluded.duration, bitrate = excluded.bitrate,
            sample_rate = excluded.sample_rate, format = excluded.format, mtime = excluded.mtime,
            has_art = excluded.has_art, bit_depth = excluded.bit_depth",
        params![
            t.path, t.title, t.artist, t.album_artist, t.album, t.genre, t.composer, t.comment,
            t.year, t.track_no, t.disc_no, t.duration, t.bitrate, t.sample_rate, t.format,
            t.date_added, t.mtime, t.has_art, t.bit_depth
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

/// Folders that finished a scan before. New files in them go to the inbox;
/// a folder's first scan doesn't, so adding a collection doesn't flood it.
pub fn scanned_folders(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM folders WHERE scanned = 1")?;
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
        // A v0 database: the base schema (upgrade steps skipped) with 0-5 star ratings.
        conn.execute_batch("PRAGMA user_version = 99;").unwrap();
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
        let t = &all_tracks(&conn).unwrap()[0];
        assert!(!t.loved && !t.inbox);
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 5);
    }

    #[test]
    fn v5_migration_rereads_lossless_files() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute_batch(
            "ALTER TABLE tracks DROP COLUMN bit_depth;
             PRAGMA user_version = 4;
             INSERT INTO tracks (path, format, mtime, date_added) VALUES ('a', 'FLAC', 5, 0), ('b', 'MP3', 5, 0);",
        )
        .unwrap();
        migrate(&conn).unwrap();
        let mtimes: Vec<i64> = conn
            .prepare("SELECT mtime FROM tracks ORDER BY path")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(mtimes, [0, 5]);
        assert_eq!(all_tracks(&conn).unwrap()[0].bit_depth, None);
    }
}

/// Benchmark library generator (no audio files; the rows are enough to time
/// loading, search, sorting and scrolling). Writes `library.db` into a fresh dir:
/// HONEYCOMB_BENCH_DIR=/tmp/hc-bench HONEYCOMB_BENCH_TRACKS=100000 cargo test --lib gen_bench_library -- --ignored
#[cfg(test)]
mod bench {
    use super::*;
    use crate::models::Track;
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    const WORDS: &[&str] = &[
        "love", "night", "city", "fire", "dream", "light", "heart", "rain", "summer", "ghost", "river", "gold",
        "echo", "neon", "storm", "honey", "glass", "wild", "blue", "silver", "midnight", "ocean", "static", "velvet",
        "paper", "shadow", "electric", "garden", "highway", "orbit", "signal", "winter", "the", "of", "in", "my",
    ];
    const GENRES: &[&str] = &[
        "Rock", "Pop", "Electronic", "Jazz", "Hip-Hop", "Classical", "Metal", "Folk", "Ambient", "Soul", "Phonk",
        "Drum & Bass", "House", "Techno", "Punk", "Shoegaze", "Lo-fi", "Soundtrack", "Country", "Blues",
    ];

    fn words(rng: &mut StdRng, n: usize) -> String {
        let mut s: Vec<String> = (0..n).map(|_| WORDS[rng.gen_range(0..WORDS.len())].to_string()).collect();
        if let Some(first) = s.first_mut() {
            *first = first[..1].to_uppercase() + &first[1..];
        }
        s.join(" ")
    }

    #[test]
    #[ignore]
    fn gen_bench_library() {
        let dir = std::env::var("HONEYCOMB_BENCH_DIR").expect("set HONEYCOMB_BENCH_DIR");
        let n: usize = std::env::var("HONEYCOMB_BENCH_TRACKS").ok().and_then(|v| v.parse().ok()).unwrap_or(100_000);
        std::fs::create_dir_all(&dir).unwrap();
        let path = Path::new(&dir).join("library.db");
        assert!(!path.exists(), "{} already exists; use an empty dir", path.display());
        let mut conn = open(&path).unwrap();
        migrate(&conn).unwrap();
        let mut rng = StdRng::seed_from_u64(42);
        let artists: Vec<String> = (0..n / 20).map(|i| format!("{} {i}", words(&mut rng, 2))).collect();
        let now = crate::models::now_secs();
        let tx = conn.transaction().unwrap();
        let mut i = 0;
        while i < n {
            let artist = &artists[rng.gen_range(0..artists.len())];
            let len = rng.gen_range(1..4);
            let album = words(&mut rng, len);
            let genre = GENRES[rng.gen_range(0..GENRES.len())];
            let year = rng.gen_range(1965..2027);
            for no in 1..=rng.gen_range(6..16u32) {
                if i >= n {
                    break;
                }
                let len = rng.gen_range(1..5);
                let title = words(&mut rng, len);
                let t = Track {
                    path: format!("/bench/{artist}/{album}/{no:02} {title}.flac"),
                    title,
                    artist: artist.clone(),
                    album_artist: artist.clone(),
                    album: album.clone(),
                    genre: genre.into(),
                    year: Some(year),
                    track_no: Some(no),
                    disc_no: Some(1),
                    duration: rng.gen_range(90.0..420.0),
                    bitrate: Some(rng.gen_range(250..1100)),
                    sample_rate: Some(44100),
                    format: "FLAC".into(),
                    date_added: now - rng.gen_range(0..200_000_000),
                    ..Default::default()
                };
                let id = upsert_track(&tx, &t).unwrap();
                if rng.gen_bool(0.3) {
                    tx.execute(
                        "UPDATE tracks SET play_count = ?2, rating = ?3, last_played = ?4 WHERE id = ?1",
                        params![id, rng.gen_range(1..60), rng.gen_range(0..11), now - rng.gen_range(0..30_000_000)],
                    )
                    .unwrap();
                }
                i += 1;
            }
        }
        tx.commit().unwrap();
        let count: i64 = conn.query_row("SELECT count(*) FROM tracks", [], |r| r.get(0)).unwrap();
        println!("wrote {count} tracks to {}", path.display());
    }
}
