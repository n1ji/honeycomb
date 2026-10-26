//! Lyrics for the now-playing panel. Sources, in order: lyrics embedded in
//! the file's tags, a `.lrc` file next to it, then LRCLIB (lrclib.net, a free
//! and open lyrics database) when online lookup is on. LRCLIB answers are
//! cached in the library database, including "not found".

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::models::{Track, now_secs};

const LRCLIB: &str = "https://lrclib.net/api";
/// Ask LRCLIB again about a track it didn't have after this long.
const RETRY_MISSING_SECS: i64 = 14 * 86400;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    /// Seconds from the start; None for unsynced lyrics.
    pub time: Option<f64>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    /// "embedded", "file", "lrclib" or "none".
    pub source: &'static str,
    pub synced: bool,
    pub instrumental: bool,
    pub lines: Vec<Line>,
}

impl Lyrics {
    fn none() -> Self {
        Lyrics { source: "none", synced: false, instrumental: false, lines: Vec::new() }
    }

    fn from_text(source: &'static str, text: &str) -> Option<Self> {
        let lines = parse(text);
        if lines.is_empty() {
            return None;
        }
        let synced = lines.iter().any(|l| l.time.is_some());
        Some(Lyrics { source, synced, instrumental: false, lines })
    }
}

/// Parse LRC (`[01:23.45]text`, several stamps per line allowed, `[offset:+250]`)
/// or plain text. Plain text keeps its blank lines as stanza breaks.
pub fn parse(text: &str) -> Vec<Line> {
    let text = text.trim_start_matches('\u{feff}');
    let mut offset = 0.0;
    let mut timed = Vec::new();
    let mut plain = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some(body) = rest.strip_prefix('[') {
            let Some(end) = body.find(']') else { break };
            let tag = &body[..end];
            if let Some(t) = parse_stamp(tag) {
                stamps.push(t);
            } else if let Some(v) = tag.strip_prefix("offset:") {
                // Positive offsets make lyrics appear sooner.
                offset = v.trim().parse::<f64>().unwrap_or(0.0) / 1000.0;
            } else if tag.contains(':') && stamps.is_empty() {
                // Metadata like [ar:Artist]; not a lyric line.
                rest = "";
                break;
            } else {
                break;
            }
            rest = body[end + 1..].trim_start();
        }
        if stamps.is_empty() {
            if !raw.trim_start().starts_with('[') {
                plain.push(raw.trim().to_string());
            }
        } else {
            for t in stamps {
                timed.push((t, rest.trim().to_string()));
            }
        }
    }
    if !timed.is_empty() {
        timed.sort_by(|a, b| a.0.total_cmp(&b.0));
        return timed
            .into_iter()
            .map(|(t, text)| Line { time: Some((t - offset).max(0.0)), text })
            .collect();
    }
    // Plain text: trim blank lines at the ends, collapse runs of blanks.
    let mut out: Vec<Line> = Vec::new();
    for l in plain {
        if l.is_empty() && out.last().is_none_or(|p: &Line| p.text.is_empty()) {
            continue;
        }
        out.push(Line { time: None, text: l });
    }
    while out.last().is_some_and(|l| l.text.is_empty()) {
        out.pop();
    }
    out
}

/// `mm:ss`, `mm:ss.xx` or `mm:ss:xx` into seconds.
fn parse_stamp(tag: &str) -> Option<f64> {
    let (min, rest) = tag.split_once(':')?;
    let min: f64 = min.trim().parse().ok()?;
    let rest = rest.replacen(':', ".", 1);
    let sec: f64 = rest.trim().parse().ok()?;
    (min >= 0.0 && (0.0..60.0).contains(&sec)).then_some(min * 60.0 + sec)
}

fn sidecar(path: &Path) -> Option<String> {
    let lrc = path.with_extension("lrc");
    let raw = std::fs::read(lrc).ok()?;
    Some(String::from_utf8_lossy(&raw).into_owned())
}

/// Lyrics from the file and the folder only; no network.
pub fn local(track: &Track) -> Option<Lyrics> {
    let path = Path::new(&track.path);
    if let Some(l) = crate::meta::embedded_lyrics(path).and_then(|t| Lyrics::from_text("embedded", &t)) {
        return Some(l);
    }
    sidecar(path).and_then(|t| Lyrics::from_text("file", &t))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrclibRecord {
    #[serde(default)]
    instrumental: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
    duration: Option<f64>,
}

fn from_record(r: &LrclibRecord) -> Lyrics {
    if r.instrumental {
        return Lyrics { source: "lrclib", synced: false, instrumental: true, lines: Vec::new() };
    }
    r.synced_lyrics
        .as_deref()
        .and_then(|t| Lyrics::from_text("lrclib", t))
        .or_else(|| r.plain_lyrics.as_deref().and_then(|t| Lyrics::from_text("lrclib", t)))
        .unwrap_or_else(Lyrics::none)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .user_agent(concat!("Honeycomb/", env!("CARGO_PKG_VERSION"), " (desktop music player)"))
        .build()
}

/// Look a track up on LRCLIB: an exact match first, then a search filtered by length.
fn fetch_lrclib(track: &Track) -> Result<Option<LrclibRecord>> {
    if track.title.is_empty() || track.artist.is_empty() {
        return Ok(None);
    }
    let agent = agent();
    let exact = agent
        .get(&format!("{LRCLIB}/get"))
        .query("track_name", &track.title)
        .query("artist_name", &track.artist)
        .query("album_name", &track.album)
        .query("duration", &format!("{}", track.duration.round() as i64))
        .call();
    match exact {
        Ok(r) => return Ok(Some(crate::net::read_json(r, crate::net::LRCLIB_MAX)?)),
        Err(ureq::Error::Status(404, _)) => {}
        Err(e) => return Err(e.into()),
    }
    let found: Vec<LrclibRecord> = crate::net::read_json(
        agent
            .get(&format!("{LRCLIB}/search"))
            .query("track_name", &track.title)
            .query("artist_name", &track.artist)
            .call()?,
        crate::net::LRCLIB_MAX,
    )?;
    Ok(found
        .into_iter()
        .filter(|r| r.duration.is_none_or(|d| (d - track.duration).abs() <= 5.0))
        .find(|r| r.instrumental || r.synced_lyrics.is_some() || r.plain_lyrics.is_some()))
}

/// Cached LRCLIB answer: Some(None) means "asked recently, not found".
fn cached(conn: &Connection, track_id: i64) -> Result<Option<Option<Lyrics>>> {
    let row: Option<(Option<String>, bool, i64)> = conn
        .query_row(
            "SELECT body, instrumental, fetched FROM lyrics WHERE track_id = ?1",
            [track_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(match row {
        None => None,
        Some((_, true, _)) => Some(Some(Lyrics { source: "lrclib", synced: false, instrumental: true, lines: Vec::new() })),
        Some((Some(body), false, _)) => Some(Lyrics::from_text("lrclib", &body)),
        Some((None, false, fetched)) if now_secs() - fetched < RETRY_MISSING_SECS => Some(None),
        Some((None, false, _)) => None,
    })
}

fn store(conn: &Connection, track_id: i64, record: Option<&LrclibRecord>) -> Result<()> {
    let body = record.and_then(|r| r.synced_lyrics.clone().or_else(|| r.plain_lyrics.clone()));
    let instrumental = record.is_some_and(|r| r.instrumental);
    conn.execute(
        "INSERT INTO lyrics (track_id, body, instrumental, fetched) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(track_id) DO UPDATE SET body = excluded.body,
            instrumental = excluded.instrumental, fetched = excluded.fetched",
        params![track_id, body, instrumental, now_secs()],
    )?;
    Ok(())
}

/// Find lyrics for a track. `online` allows asking LRCLIB.
pub fn get(conn: &Connection, track: &Track, online: bool) -> Lyrics {
    if let Some(l) = local(track) {
        return l;
    }
    if !online {
        return Lyrics::none();
    }
    match cached(conn, track.id) {
        Ok(Some(hit)) => return hit.unwrap_or_else(Lyrics::none),
        Ok(None) => {}
        Err(e) => eprintln!("lyrics cache: {e:#}"),
    }
    match fetch_lrclib(track) {
        Ok(record) => {
            if let Err(e) = store(conn, track.id, record.as_ref()) {
                eprintln!("lyrics cache: {e:#}");
            }
            record.as_ref().map(from_record).unwrap_or_else(Lyrics::none)
        }
        // Offline or LRCLIB down: don't cache, try again next time.
        Err(e) => {
            eprintln!("lyrics lookup failed: {e:#}");
            Lyrics::none()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn parses_synced_lrc() {
        let l = parse("[ar:Someone]\n[offset:500]\n[00:12.50]First\n[00:05.00][01:00.00]Chorus\n[00:20:25] Third \n");
        let got: Vec<(f64, &str)> = l.iter().map(|x| (x.time.unwrap(), x.text.as_str())).collect();
        assert_eq!(got, [(4.5, "Chorus"), (12.0, "First"), (19.75, "Third"), (59.5, "Chorus")]);
    }

    #[test]
    fn parses_plain_text() {
        let l = parse("\n\nLine one\nLine two\n\n\n\nVerse two\n\n");
        let got: Vec<&str> = l.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(got, ["Line one", "Line two", "", "Verse two"]);
        assert!(l.iter().all(|x| x.time.is_none()));
    }

    #[test]
    fn sidecar_lrc_is_used() {
        let dir = std::env::temp_dir().join(format!("hc-lyrics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let audio = dir.join("song.flac");
        std::fs::write(&audio, b"not really audio").unwrap();
        std::fs::write(dir.join("song.lrc"), "[00:01.00]Hello").unwrap();
        let t = Track { path: audio.to_string_lossy().into_owned(), ..Default::default() };
        let l = local(&t).unwrap();
        assert_eq!((l.source, l.synced, l.lines.len()), ("file", true, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn caches_lrclib_answers() {
        let conn = Connection::open_in_memory().unwrap();
        db::migrate(&conn).unwrap();
        let t = Track { path: "/x.mp3".into(), title: "A".into(), artist: "B".into(), date_added: 0, ..Default::default() };
        let id = db::upsert_track(&conn, &t).unwrap();
        assert_eq!(cached(&conn, id).unwrap(), None);
        // Not found is remembered for a while.
        store(&conn, id, None).unwrap();
        assert_eq!(cached(&conn, id).unwrap(), Some(None));
        // A found record is served from the cache.
        let rec: LrclibRecord = serde_json::from_str(
            r#"{"instrumental":false,"plainLyrics":"la la","syncedLyrics":"[00:02.00]la la","duration":180}"#,
        )
        .unwrap();
        assert!(from_record(&rec).synced);
        store(&conn, id, Some(&rec)).unwrap();
        let hit = cached(&conn, id).unwrap().unwrap().unwrap();
        assert_eq!((hit.source, hit.synced, hit.lines[0].time), ("lrclib", true, Some(2.0)));
    }

    /// Needs network: cargo test --lib lrclib_live -- --ignored --nocapture
    #[test]
    #[ignore]
    fn lrclib_live() {
        let t = Track {
            title: "Smells Like Teen Spirit".into(),
            artist: "Nirvana".into(),
            album: "Nevermind".into(),
            duration: 301.0,
            ..Default::default()
        };
        let r = fetch_lrclib(&t).unwrap().expect("LRCLIB should know this one");
        let l = from_record(&r);
        println!("{} lines, synced: {}", l.lines.len(), l.synced);
        assert!(!l.lines.is_empty());
    }
}
