//! One-time import from another player: play stats and ratings from an
//! iTunes-format library XML, and playlists from that XML or from M3U files.
//!
//! The source library usually lived on another machine, so its paths
//! (`C:\Music\...`, `\\server\share\...`) never match ours. Tracks are matched
//! by the tail of their path, then by tags.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::db;
use crate::models::Track;

/// Tags used to match a track whose path didn't match.
#[derive(Debug, Clone, Default)]
pub struct Tags {
    pub artist: String,
    pub title: String,
    pub album: String,
    /// Seconds.
    pub duration: Option<f64>,
}

/// A track entry from the source library XML.
#[derive(Debug, Clone, Default)]
pub struct SourceTrack {
    pub path: String,
    pub tags: Tags,
    /// Half-stars, 0-10. `None` when unrated.
    pub rating: Option<u8>,
    pub play_count: u32,
    pub last_played: Option<i64>,
    pub date_added: Option<i64>,
    pub loved: bool,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: String,
    pub tags: Option<Tags>,
}

#[derive(Debug, Clone)]
pub struct SourcePlaylist {
    pub name: String,
    /// "xml" or the M3U file name.
    pub source: String,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Default)]
pub struct Source {
    pub tracks: Vec<SourceTrack>,
    pub playlists: Vec<SourcePlaylist>,
}

// ---------- reading ----------

/// Read an iTunes-format library XML. Some players write non-numbers inside
/// `<integer>` (vinyl track numbers like "A1", fractional BPM), which strict
/// plist parsers reject, so those values are turned into strings first.
pub fn read_library_xml(path: &Path) -> Result<Source> {
    let raw = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    parse_library_xml(&raw)
}

pub fn parse_library_xml(raw: &[u8]) -> Result<Source> {
    use plist::Value;
    let fixed = fix_integers(&String::from_utf8_lossy(raw));
    let root = Value::from_reader_xml(fixed.as_bytes()).context("not a valid library XML")?;
    let root = root.as_dictionary().context("library XML has no top-level dictionary")?;

    let str_of = |d: &plist::Dictionary, k: &str| d.get(k).and_then(Value::as_string).unwrap_or("").to_string();
    let int_of = |d: &plist::Dictionary, k: &str| d.get(k).and_then(Value::as_signed_integer);
    let date_of = |d: &plist::Dictionary, k: &str| {
        d.get(k).and_then(Value::as_date).and_then(|t| {
            std::time::SystemTime::from(t).duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
        })
    };

    let mut out = Source::default();
    // Track ID -> index into out.tracks, for resolving playlist items.
    let mut by_id = HashMap::new();
    if let Some(tracks) = root.get("Tracks").and_then(Value::as_dictionary) {
        for (key, t) in tracks {
            let Some(t) = t.as_dictionary() else { continue };
            let location = str_of(t, "Location");
            if location.is_empty() {
                continue; // streams and other non-file entries
            }
            // A computed rating is inherited from the album, not set on the track.
            let computed = t.get("Rating Computed").and_then(Value::as_boolean).unwrap_or(false);
            let rating = int_of(t, "Rating").filter(|_| !computed).map(|r| ((r.clamp(0, 100) + 5) / 10) as u8);
            let love = str_of(t, "Love");
            let loved = t.get("Loved").and_then(Value::as_boolean).unwrap_or(false)
                || love.eq_ignore_ascii_case("l")
                || love.eq_ignore_ascii_case("loved");
            by_id.insert(key.clone(), out.tracks.len());
            out.tracks.push(SourceTrack {
                path: file_url_to_path(&location),
                tags: Tags {
                    artist: str_of(t, "Artist"),
                    title: str_of(t, "Name"),
                    album: str_of(t, "Album"),
                    duration: int_of(t, "Total Time").map(|ms| ms as f64 / 1000.0),
                },
                rating,
                play_count: int_of(t, "Play Count").unwrap_or(0).max(0) as u32,
                last_played: date_of(t, "Play Date UTC"),
                date_added: date_of(t, "Date Added"),
                loved,
            });
        }
    }

    let playlists: Vec<&plist::Dictionary> = root
        .get("Playlists")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_dictionary).collect())
        .unwrap_or_default();
    // Folder persistent ID -> (name, parent), for "Folder / Playlist" names.
    let folders: HashMap<String, (String, String)> = playlists
        .iter()
        .filter(|p| p.get("Folder").and_then(Value::as_boolean).unwrap_or(false))
        .map(|p| (str_of(p, "Playlist Persistent ID"), (str_of(p, "Name"), str_of(p, "Parent Persistent ID"))))
        .collect();
    for p in playlists {
        let flag = |k: &str| p.get(k).and_then(Value::as_boolean).unwrap_or(false);
        let builtin = flag("Master") || p.contains_key("Distinguished Kind");
        let hidden = p.get("Visible").and_then(Value::as_boolean) == Some(false);
        if builtin || hidden || flag("Folder") {
            continue;
        }
        let entries: Vec<Entry> = p
            .get("Playlist Items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let id = item.as_dictionary()?.get("Track ID")?.as_signed_integer()?;
                let t = &out.tracks[*by_id.get(&id.to_string())?];
                Some(Entry { path: t.path.clone(), tags: Some(t.tags.clone()) })
            })
            .collect();
        // Players often export playlists they keep elsewhere as empty shells.
        if entries.is_empty() {
            continue;
        }
        let mut name = str_of(p, "Name");
        let mut parent = str_of(p, "Parent Persistent ID");
        for _ in 0..16 {
            let Some((folder, up)) = folders.get(&parent) else { break };
            name = format!("{folder} / {name}");
            parent = up.clone();
        }
        out.playlists.push(SourcePlaylist { name, source: "xml".into(), entries });
    }
    Ok(out)
}

/// Replace `<integer>x</integer>` with `<string>x</string>` when x isn't an integer.
fn fix_integers(xml: &str) -> String {
    const OPEN: &str = "<integer>";
    const CLOSE: &str = "</integer>";
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(i) = rest.find(OPEN) {
        out.push_str(&rest[..i]);
        let after = &rest[i + OPEN.len()..];
        let Some(j) = after.find(CLOSE) else {
            out.push_str(&rest[i..]);
            return out;
        };
        let value = &after[..j];
        let trimmed = value.trim();
        let digits = trimmed.strip_prefix('-').unwrap_or(trimmed);
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            out.push_str(OPEN);
            out.push_str(value);
            out.push_str(CLOSE);
        } else {
            out.push_str("<string>");
            out.push_str(value);
            out.push_str("</string>");
        }
        rest = &after[j + CLOSE.len()..];
    }
    out.push_str(rest);
    out
}

/// `file://localhost/C:/Music/a%20b.flac` -> `C:/Music/a b.flac`. Only the
/// path's tail is used for matching, so the exact host form doesn't matter.
fn file_url_to_path(url: &str) -> String {
    let rest = url.strip_prefix("file://").unwrap_or(url);
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let mut bytes = Vec::with_capacity(rest.len());
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                bytes.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        bytes.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Read an .m3u/.m3u8 file. Accepts UTF-8 (with or without BOM) and falls
/// back to Windows-1252 for old files.
pub fn read_m3u(path: &Path) -> Result<SourcePlaylist> {
    let raw = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let text = match std::str::from_utf8(&raw) {
        Ok(s) => s.to_string(),
        Err(_) => raw.iter().map(|&b| cp1252(b)).collect(),
    };
    let entries = text
        .lines()
        .map(|l| l.trim().trim_start_matches('\u{feff}'))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| Entry { path: l.to_string(), tags: None })
        .collect();
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Imported").to_string();
    let source = path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
    Ok(SourcePlaylist { name, source, entries })
}

fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}',
        '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9f => HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

/// Expand folders into the playlist files directly inside them.
pub fn expand_m3u_paths(paths: &[String]) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        let p = Path::new(p);
        if p.is_dir() {
            let mut files: Vec<_> = std::fs::read_dir(p)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|f| {
                    f.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("m3u") || e.eq_ignore_ascii_case("m3u8"))
                })
                .collect();
            files.sort();
            out.extend(files);
        } else {
            out.push(p.to_path_buf());
        }
    }
    out
}

// ---------- matching ----------

/// Placeholder entries some players write for tracks they lost track of,
/// e.g. `Unknown Album Artist/Unknown Album/01 - Unknown Title.mp3`.
pub fn is_placeholder(path: &str) -> bool {
    let stem = Path::new(&path.replace('\\', "/"))
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    stem.ends_with("unknown title")
}

fn norm(s: &str) -> String {
    s.nfc().collect::<String>().to_lowercase()
}

fn path_parts(path: &str) -> Vec<String> {
    path.split(['/', '\\']).filter(|p| !p.is_empty()).map(norm).collect()
}

fn tag_key(t: &Tags) -> Option<(String, String, String)> {
    let key = (norm(t.artist.trim()), norm(t.title.trim()), norm(t.album.trim()));
    (!key.1.is_empty()).then_some(key)
}

pub struct Matcher {
    exact: HashMap<String, i64>,
    /// "n|last n path parts" -> track ids, for n = 1..=3.
    suffix: HashMap<String, Vec<i64>>,
    tags: HashMap<(String, String, String), Vec<(i64, f64)>>,
}

impl Matcher {
    pub fn new(tracks: &[Track]) -> Self {
        let mut m = Matcher { exact: HashMap::new(), suffix: HashMap::new(), tags: HashMap::new() };
        for t in tracks {
            m.exact.insert(t.path.clone(), t.id);
            let parts = path_parts(&t.path);
            for n in 1..=3.min(parts.len()) {
                let key = format!("{n}|{}", parts[parts.len() - n..].join("/"));
                m.suffix.entry(key).or_default().push(t.id);
            }
            let tags = Tags { artist: t.artist.clone(), title: t.title.clone(), album: t.album.clone(), duration: None };
            if let Some(k) = tag_key(&tags) {
                m.tags.entry(k).or_default().push((t.id, t.duration));
            }
        }
        m
    }

    fn by_suffix(&self, parts: &[String], n: usize) -> Option<i64> {
        if parts.len() < n {
            return None;
        }
        match self.suffix.get(&format!("{n}|{}", parts[parts.len() - n..].join("/")))?.as_slice() {
            [id] => Some(*id),
            _ => None, // missing or ambiguous
        }
    }

    /// Find the library track for a source entry. Ambiguous matches count as no match.
    pub fn find(&self, path: &str, tags: Option<&Tags>) -> Option<i64> {
        if let Some(id) = self.exact.get(path) {
            return Some(*id);
        }
        let parts = path_parts(path);
        if let Some(id) = self.by_suffix(&parts, 3).or_else(|| self.by_suffix(&parts, 2)) {
            return Some(id);
        }
        if let Some(t) = tags {
            let hits: Vec<i64> = tag_key(t)
                .and_then(|k| self.tags.get(&k))
                .into_iter()
                .flatten()
                .filter(|(_, d)| t.duration.is_none_or(|want| (want - d).abs() <= 2.0))
                .map(|(id, _)| *id)
                .collect();
            return match hits.as_slice() {
                [id] => Some(*id),
                _ => None,
            };
        }
        // No tags to check against, so a unique file name is the last resort.
        self.by_suffix(&parts, 1)
    }
}

// ---------- preview and apply ----------

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StatsPreview {
    pub tracks: usize,
    pub matched: usize,
    pub unmatched: usize,
    /// Matched tracks that carry a rating, play count or love.
    pub with_stats: usize,
    pub rated: usize,
    pub played: usize,
    pub loved: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistPreview {
    /// Position in the source list; sent back to choose what to import.
    pub index: usize,
    pub name: String,
    pub source: String,
    pub total: usize,
    pub matched: usize,
    pub placeholders: usize,
    pub missing: usize,
    /// A playlist with this name already exists and would be replaced.
    pub exists: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub library_tracks: usize,
    pub stats: Option<StatsPreview>,
    pub playlists: Vec<PlaylistPreview>,
    /// Plain-text list of everything that didn't match, for saving.
    pub report: String,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub stats_updated: usize,
    pub playlists_created: usize,
    pub playlists_replaced: usize,
    pub placeholders_skipped: usize,
    pub missing_skipped: usize,
}

struct ResolvedPlaylist {
    ids: Vec<i64>,
    placeholders: Vec<String>,
    missing: Vec<String>,
}

fn resolve(m: &Matcher, p: &SourcePlaylist) -> ResolvedPlaylist {
    let mut r = ResolvedPlaylist { ids: Vec::new(), placeholders: Vec::new(), missing: Vec::new() };
    for e in &p.entries {
        if is_placeholder(&e.path) {
            r.placeholders.push(e.path.clone());
        } else if let Some(id) = m.find(&e.path, e.tags.as_ref()) {
            r.ids.push(id);
        } else {
            r.missing.push(e.path.clone());
        }
    }
    r
}

fn manual_playlist_ids_by_name(conn: &Connection) -> Result<HashMap<String, i64>> {
    Ok(db::playlists(conn)?
        .into_iter()
        .filter(|p| p.kind == "manual")
        .map(|p| (p.name, p.id))
        .collect())
}

pub fn preview(conn: &Connection, src: &Source, has_xml: bool) -> Result<Preview> {
    let library = db::all_tracks(conn)?;
    let m = Matcher::new(&library);
    let mut report = String::new();

    let stats = has_xml.then(|| {
        let mut s = StatsPreview { tracks: src.tracks.len(), ..Default::default() };
        let mut unmatched = Vec::new();
        for t in &src.tracks {
            if m.find(&t.path, Some(&t.tags)).is_some() {
                s.matched += 1;
                let rated = t.rating.is_some_and(|r| r > 0);
                s.rated += rated as usize;
                s.played += (t.play_count > 0) as usize;
                s.loved += t.loved as usize;
                s.with_stats += (rated || t.play_count > 0 || t.loved) as usize;
            } else {
                unmatched.push(&t.path);
            }
        }
        s.unmatched = unmatched.len();
        if !unmatched.is_empty() {
            report.push_str(&format!("Library tracks not found ({}):\n", unmatched.len()));
            for p in unmatched {
                report.push_str(&format!("  {p}\n"));
            }
            report.push('\n');
        }
        s
    });

    let existing = manual_playlist_ids_by_name(conn)?;
    let mut playlists = Vec::new();
    for (index, p) in src.playlists.iter().enumerate() {
        let r = resolve(&m, p);
        if !r.placeholders.is_empty() || !r.missing.is_empty() {
            report.push_str(&format!("Playlist \"{}\" ({}):\n", p.name, p.source));
            if !r.placeholders.is_empty() {
                report.push_str(&format!("  {} placeholder entries skipped\n", r.placeholders.len()));
            }
            for path in &r.missing {
                report.push_str(&format!("  not found: {path}\n"));
            }
            report.push('\n');
        }
        playlists.push(PlaylistPreview {
            index,
            name: p.name.clone(),
            source: p.source.clone(),
            total: p.entries.len(),
            matched: r.ids.len(),
            placeholders: r.placeholders.len(),
            missing: r.missing.len(),
            exists: existing.contains_key(&p.name),
        });
    }
    Ok(Preview { library_tracks: library.len(), stats, playlists, report })
}

/// Write imported stats and the chosen playlists. Stats merge with what's
/// already here: the higher play count, the newer last-played date, the
/// older date added, and the imported rating only for unrated tracks.
/// Running it twice gives the same result.
pub fn apply(conn: &mut Connection, src: &Source, import_stats: bool, playlist_indices: &[usize]) -> Result<Outcome> {
    let library = db::all_tracks(conn)?;
    let m = Matcher::new(&library);
    let mut out = Outcome::default();
    let tx = conn.transaction()?;
    if import_stats {
        let mut stmt = tx.prepare(
            "UPDATE tracks SET
                play_count = MAX(play_count, ?2),
                rating = CASE WHEN rating = 0 THEN ?3 ELSE rating END,
                last_played = CASE WHEN ?4 IS NULL THEN last_played
                                   ELSE MAX(IFNULL(last_played, 0), ?4) END,
                date_added = MIN(date_added, IFNULL(?5, date_added)),
                loved = MAX(loved, ?6)
             WHERE id = ?1",
        )?;
        for t in &src.tracks {
            if let Some(id) = m.find(&t.path, Some(&t.tags)) {
                stmt.execute(params![
                    id,
                    t.play_count,
                    t.rating.unwrap_or(0).min(10),
                    t.last_played,
                    t.date_added,
                    t.loved
                ])?;
                out.stats_updated += 1;
            }
        }
    }
    let mut existing = manual_playlist_ids_by_name(&tx)?;
    for &i in playlist_indices {
        let Some(p) = src.playlists.get(i) else { continue };
        let r = resolve(&m, p);
        out.placeholders_skipped += r.placeholders.len();
        out.missing_skipped += r.missing.len();
        let id = match existing.get(&p.name) {
            Some(&id) => {
                out.playlists_replaced += 1;
                tx.execute("DELETE FROM playlist_tracks WHERE playlist_id = ?1", [id])?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO playlists (name, kind, rules, created) VALUES (?1, 'manual', NULL, ?2)",
                    params![p.name, crate::models::now_secs()],
                )?;
                out.playlists_created += 1;
                let id = tx.last_insert_rowid();
                existing.insert(p.name.clone(), id);
                id
            }
        };
        let mut stmt =
            tx.prepare_cached("INSERT INTO playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)")?;
        for (pos, tid) in r.ids.iter().enumerate() {
            stmt.execute(params![id, tid, pos as i64])?;
        }
    }
    tx.commit()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::now_secs;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple Computer//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Major Version</key><integer>1</integer>
	<key>Tracks</key>
	<dict>
		<key>10</key>
		<dict>
			<key>Track ID</key><integer>10</integer>
			<key>Name</key><string>Side A Opener</string>
			<key>Artist</key><string>Comb Theory</string>
			<key>Album</key><string>Wax</string>
			<key>Track Number</key><integer>A1</integer>
			<key>BPM</key><integer>145.263000488281</integer>
			<key>Total Time</key><integer>201000</integer>
			<key>Rating</key><integer>70</integer>
			<key>Play Count</key><integer>12</integer>
			<key>Play Date UTC</key><date>2026-05-01T10:00:00Z</date>
			<key>Date Added</key><date>2020-01-02T03:04:05Z</date>
			<key>Love</key><string>L</string>
			<key>Location</key><string>file://localhost//nas/music/Comb%20Theory/Wax/A1%20Side%20A%20Opener.flac</string>
		</dict>
		<key>11</key>
		<dict>
			<key>Track ID</key><integer>11</integer>
			<key>Name</key><string>Renamed Song</string>
			<key>Artist</key><string>Alpha Bees</string>
			<key>Album</key><string>Hive</string>
			<key>Total Time</key><integer>180500</integer>
			<key>Rating</key><integer>60</integer>
			<key>Rating Computed</key><true/>
			<key>Location</key><string>file://localhost/C:/Users/me/Music/old%20name.mp3</string>
		</dict>
		<key>12</key>
		<dict>
			<key>Track ID</key><integer>12</integer>
			<key>Name</key><string>Not In Library</string>
			<key>Location</key><string>file://localhost/C:/Music/gone.mp3</string>
		</dict>
	</dict>
	<key>Playlists</key>
	<array>
		<dict><key>Name</key><string>Library</string><key>Master</key><true/>
			<key>Playlist Items</key><array><dict><key>Track ID</key><integer>10</integer></dict></array></dict>
		<dict><key>Name</key><string>Kept elsewhere</string><key>Playlist Items</key><array/></dict>
		<dict><key>Name</key><string>Moods</string><key>Folder</key><true/>
			<key>Playlist Persistent ID</key><string>F1</string></dict>
		<dict><key>Name</key><string>Late night</string><key>Parent Persistent ID</key><string>F1</string>
			<key>Playlist Items</key><array>
				<dict><key>Track ID</key><integer>11</integer></dict>
				<dict><key>Track ID</key><integer>12</integer></dict>
				<dict><key>Track ID</key><integer>10</integer></dict>
			</array></dict>
	</array>
</dict>
</plist>"#;

    fn library() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::migrate(&conn).unwrap();
        let add = |path: &str, artist: &str, title: &str, album: &str, duration: f64| {
            let t = Track {
                path: path.into(),
                artist: artist.into(),
                title: title.into(),
                album: album.into(),
                duration,
                date_added: now_secs(),
                ..Default::default()
            };
            db::upsert_track(&conn, &t).unwrap()
        };
        add("/Users/me/Music/Comb Theory/Wax/A1 Side A Opener.flac", "Comb Theory", "Side A Opener", "Wax", 201.0);
        add("/Users/me/Music/Alpha Bees/Hive/02 Renamed Song.mp3", "Alpha Bees", "Renamed Song", "Hive", 181.0);
        add("/Users/me/Music/Other/Intro.mp3", "X", "Intro", "Other", 60.0);
        add("/Users/me/Music/Else/Intro.mp3", "Y", "Intro", "Else", 60.0);
        conn
    }

    #[test]
    fn parses_quirky_xml() {
        let src = parse_library_xml(XML.as_bytes()).unwrap();
        assert_eq!(src.tracks.len(), 3);
        let t = src.tracks.iter().find(|t| t.tags.title == "Side A Opener").unwrap();
        assert_eq!(t.path, "//nas/music/Comb Theory/Wax/A1 Side A Opener.flac");
        assert_eq!(t.rating, Some(7));
        assert_eq!(t.play_count, 12);
        assert!(t.loved);
        assert_eq!(t.tags.duration, Some(201.0));
        assert_eq!(t.date_added, Some(1577934245));
        // A computed (album) rating isn't the track's own.
        let r = src.tracks.iter().find(|t| t.tags.title == "Renamed Song").unwrap();
        assert_eq!(r.rating, None);
        // Built-in, empty and folder entries are skipped; folders prefix names.
        let names: Vec<_> = src.playlists.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Moods / Late night"]);
        assert_eq!(src.playlists[0].entries.len(), 3);
    }

    #[test]
    fn fixes_only_bad_integers() {
        assert_eq!(fix_integers("<integer>-12</integer><integer>A1</integer>"), "<integer>-12</integer><string>A1</string>");
    }

    #[test]
    fn matches_by_path_tail_then_tags() {
        let conn = library();
        let m = Matcher::new(&db::all_tracks(&conn).unwrap());
        assert!(m.find("C:\\Music\\Comb Theory\\Wax\\A1 Side A Opener.flac", None).is_some());
        // Decomposed (NFD) names match their composed form.
        let conn2 = Connection::open_in_memory().unwrap();
        db::migrate(&conn2).unwrap();
        let t = Track { path: "/m/Caf\u{e9}/x/Song.mp3".into(), title: "Song".into(), ..Default::default() };
        db::upsert_track(&conn2, &t).unwrap();
        let m2 = Matcher::new(&db::all_tracks(&conn2).unwrap());
        assert!(m2.find("D:\\Cafe\u{301}\\x\\Song.mp3", None).is_some());
        // Renamed file: tags and a close duration still match.
        let tags = Tags { artist: "alpha bees".into(), title: "Renamed Song".into(), album: "Hive".into(), duration: Some(180.5) };
        assert!(m.find("C:/old name.mp3", Some(&tags)).is_some());
        let far = Tags { duration: Some(240.0), ..tags };
        assert!(m.find("C:/old name.mp3", Some(&far)).is_none());
        // Two library files share the name: ambiguous, so no match.
        assert!(m.find("C:/Intro.mp3", None).is_none());
    }

    #[test]
    fn placeholders() {
        assert!(is_placeholder("/Music/Unknown Album Artist/Unknown Album/01 - Unknown Title.mp3"));
        assert!(!is_placeholder("/Music/Band/Album/Unknown Title Remix.mp3"));
    }

    #[test]
    fn preview_and_apply_merge_stats_and_playlists() {
        let mut conn = library();
        let mut src = parse_library_xml(XML.as_bytes()).unwrap();
        src.playlists.push(SourcePlaylist {
            name: "From m3u".into(),
            source: "From m3u.m3u".into(),
            entries: vec![
                Entry { path: "/Music/Unknown Album Artist/Unknown Album/01 - Unknown Title.mp3".into(), tags: None },
                Entry { path: "/music/Comb Theory/Wax/A1 Side A Opener.flac".into(), tags: None },
            ],
        });
        let p = preview(&conn, &src, true).unwrap();
        let s = p.stats.unwrap();
        assert_eq!((s.matched, s.unmatched, s.played, s.loved), (2, 1, 1, 1));
        assert_eq!((p.playlists[0].matched, p.playlists[0].missing), (2, 1));
        assert_eq!(p.playlists[1].placeholders, 1);
        assert!(p.report.contains("gone.mp3"));

        // Existing local stats: rated 2 stars and played 20 times here already.
        let id = db::track_id_by_path(&conn, "/Users/me/Music/Comb Theory/Wax/A1 Side A Opener.flac").unwrap().unwrap();
        conn.execute("UPDATE tracks SET play_count = 3, rating = 4, date_added = 1900000000 WHERE id = ?1", [id]).unwrap();
        let out = apply(&mut conn, &src, true, &[0, 1]).unwrap();
        assert_eq!((out.stats_updated, out.playlists_created, out.placeholders_skipped, out.missing_skipped), (2, 2, 1, 1));
        let t = db::track_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(t.play_count, 12); // higher count wins
        assert_eq!(t.rating, 4); // local rating kept
        assert_eq!(t.date_added, 1577934245); // older date wins
        assert_eq!(t.last_played, Some(1777629600));
        assert!(t.loved);

        // Re-running replaces playlists instead of duplicating them.
        let again = apply(&mut conn, &src, true, &[0, 1]).unwrap();
        assert_eq!((again.playlists_created, again.playlists_replaced), (0, 2));
        let pls = db::playlists(&conn).unwrap();
        assert_eq!(pls.len(), 2);
        let late = pls.iter().find(|p| p.name == "Moods / Late night").unwrap();
        assert_eq!(db::manual_playlist_track_ids(&conn, late.id).unwrap().len(), 2);
    }

    /// Check against a real export without committing it:
    /// HONEYCOMB_IMPORT_XML=… [HONEYCOMB_IMPORT_M3U=dir] cargo test real_export -- --ignored --nocapture
    /// Builds a library from the XML's own entries under a different root, so
    /// every track should match by path.
    #[test]
    #[ignore]
    fn real_export() {
        let xml = std::env::var("HONEYCOMB_IMPORT_XML").expect("set HONEYCOMB_IMPORT_XML");
        let mut src = read_library_xml(Path::new(&xml)).unwrap();
        if let Ok(dir) = std::env::var("HONEYCOMB_IMPORT_M3U") {
            for f in expand_m3u_paths(&[dir]) {
                src.playlists.push(read_m3u(&f).unwrap());
            }
        }
        let conn = Connection::open_in_memory().unwrap();
        db::migrate(&conn).unwrap();
        for t in &src.tracks {
            let parts: Vec<&str> = t.path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
            let track = Track {
                path: format!("/Users/test/Music/{}", parts[parts.len().saturating_sub(3)..].join("/")),
                artist: t.tags.artist.clone(),
                title: t.tags.title.clone(),
                album: t.tags.album.clone(),
                duration: t.tags.duration.unwrap_or(0.0),
                date_added: now_secs(),
                ..Default::default()
            };
            db::upsert_track(&conn, &track).unwrap();
        }
        let p = preview(&conn, &src, true).unwrap();
        println!("library {} | stats {:?}", p.library_tracks, p.stats);
        for pl in &p.playlists {
            println!(
                "{:>5}/{:<5} placeholders {:>4} missing {:>3}  {}",
                pl.matched, pl.total, pl.placeholders, pl.missing, pl.source
            );
        }
        let s = p.stats.unwrap();
        assert_eq!(s.unmatched, 0, "every XML track should match its own copy");
    }
}
