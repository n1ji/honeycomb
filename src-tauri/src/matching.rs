//! Loose matching of tracks described elsewhere (another service, another
//! device) to tracks in the library: by artist and title with version notes
//! like "(Remastered)" ignored, and lengths that roughly agree.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use unicode_normalization::UnicodeNormalization;

use crate::models::Track;

/// A track described by another service, to find in the library.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub title: String,
    #[serde(default)]
    pub artists: Vec<String>,
    #[serde(default)]
    pub album: String,
    /// Seconds.
    #[serde(default)]
    pub duration: Option<f64>,
}

/// Bracketed or " - " suffixes that name the same recording: "(feat. X)", "- 2011 Remaster".
const SAME_RECORDING: &[&str] = &["feat", "ft.", "with ", "remaster", "deluxe", "edition", "bonus", "mono", "stereo", "explicit", "single version", "album version"];

fn strip_versions(s: &str) -> String {
    // Drop "(…)" and "[…]" parts that name the same recording, keep the rest.
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find(['(', '[']) {
        let close_ch = if rest[open..].starts_with('(') { ')' } else { ']' };
        let Some(close) = rest[open..].find(close_ch).map(|i| open + i) else { break };
        out.push_str(&rest[..open]);
        if SAME_RECORDING.iter().any(|k| rest[open + 1..close].contains(k)) {
            out.push(' ');
        } else {
            out.push_str(&rest[open..=close]);
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    while let Some(i) = out.rfind(" - ") {
        if !SAME_RECORDING.iter().any(|k| out[i..].contains(k)) {
            break;
        }
        out.truncate(i);
    }
    out
}

/// Lowercased, version notes removed, punctuation folded to single spaces.
fn loose(s: &str) -> String {
    let s: String = s.nfc().collect::<String>().to_lowercase();
    let s = strip_versions(&s).replace('&', " and ");
    let words: Vec<&str> = s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let words = if words.len() > 1 && words[0] == "the" { &words[1..] } else { &words[..] };
    words.join(" ")
}

/// Each artist named in a credit like "A feat. B & C", plus the whole credit.
fn artist_keys(credit: &str) -> Vec<String> {
    let lower = credit.nfc().collect::<String>().to_lowercase();
    let mut parts = vec![lower.clone()];
    let mut rest = lower;
    for sep in [" featuring ", " feat. ", " feat ", " ft. ", " with ", " vs. ", " x "] {
        rest = rest.replace(sep, ",");
    }
    parts.extend(rest.split([',', ';', '&', '/']).map(str::to_string));
    let mut keys: Vec<String> = parts.iter().map(|p| loose(p)).filter(|k| !k.is_empty()).collect();
    keys.sort();
    keys.dedup();
    keys
}

pub struct LibraryIndex {
    /// (artist key, title key) -> indexes into `tracks`.
    by_artist_title: HashMap<(String, String), Vec<usize>>,
    tracks: Vec<(i64, f64, String)>,
}

impl LibraryIndex {
    pub fn new(tracks: &[Track]) -> Self {
        let mut idx = LibraryIndex { by_artist_title: HashMap::new(), tracks: Vec::with_capacity(tracks.len()) };
        for t in tracks {
            let title = loose(&t.title);
            if title.is_empty() {
                continue;
            }
            let i = idx.tracks.len();
            idx.tracks.push((t.id, t.duration, loose(&t.album)));
            let mut keys: HashSet<String> = artist_keys(&t.artist).into_iter().collect();
            keys.extend(artist_keys(&t.album_artist));
            for a in keys {
                idx.by_artist_title.entry((a, title.clone())).or_default().push(i);
            }
        }
        idx
    }

    /// The best library track for `c`: same artist and title, length within
    /// 3 s (10 s when the album matches too), preferring the same album.
    pub fn find(&self, c: &Candidate) -> Option<i64> {
        let title = loose(&c.title);
        let album = loose(&c.album);
        let mut hits: Vec<usize> = c
            .artists
            .iter()
            .flat_map(|a| artist_keys(a))
            .filter_map(|a| self.by_artist_title.get(&(a, title.clone())))
            .flatten()
            .copied()
            .collect();
        hits.sort_unstable();
        hits.dedup();
        hits.into_iter()
            .filter_map(|i| {
                let (id, dur, lib_album) = &self.tracks[i];
                let same_album =
                    !album.is_empty() && !lib_album.is_empty() && (lib_album.contains(&album) || album.contains(lib_album.as_str()));
                let diff = c.duration.map_or(0.0, |d| (d - dur).abs());
                (diff <= 3.0 || (same_album && diff <= 10.0)).then_some((!same_album, diff, *id))
            })
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, _, id)| id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: i64, artist: &str, title: &str, album: &str, duration: f64) -> Track {
        Track {
            id,
            artist: artist.into(),
            title: title.into(),
            album: album.into(),
            duration,
            ..Track::default()
        }
    }

    fn cand(artists: &[&str], title: &str, album: &str, duration: f64) -> Candidate {
        Candidate {
            title: title.into(),
            artists: artists.iter().map(|s| s.to_string()).collect(),
            album: album.into(),
            duration: Some(duration),
        }
    }

    #[test]
    fn loose_drops_version_notes() {
        assert_eq!(loose("Here Comes the Sun - Remastered 2009"), "here comes the sun");
        assert_eq!(loose("Song (feat. Someone) [2015 Remaster]"), "song");
        assert_eq!(loose("Song (Live)"), "song live");
        assert_eq!(loose("The Beatles"), "beatles");
        assert_eq!(loose("Simon & Garfunkel"), "simon and garfunkel");
        assert_eq!(loose("Song - Live at Wembley"), "song live at wembley");
    }

    #[test]
    fn artist_credits_split() {
        let keys = artist_keys("Daft Punk feat. Pharrell Williams & Nile Rodgers");
        assert!(keys.contains(&"daft punk".to_string()));
        assert!(keys.contains(&"pharrell williams".to_string()));
        assert!(keys.contains(&"nile rodgers".to_string()));
    }

    #[test]
    fn finds_by_artist_title_and_length() {
        let lib = LibraryIndex::new(&[
            track(1, "The Beatles", "Here Comes the Sun", "Abbey Road", 185.0),
            track(2, "The Beatles", "Here Comes the Sun", "1967-1970", 186.0),
            track(3, "Daft Punk feat. Pharrell Williams", "Get Lucky", "Random Access Memories", 369.0),
            track(4, "Someone", "Long Song", "Album", 300.0),
        ]);
        // Album breaks the tie between two copies.
        assert_eq!(lib.find(&cand(&["The Beatles"], "Here Comes The Sun - Remastered 2009", "Abbey Road (Remastered)", 185.5)), Some(1));
        assert_eq!(lib.find(&cand(&["Beatles"], "Here Comes the Sun", "1967 - 1970", 186.0)), Some(2));
        // Spotify lists each artist separately.
        assert_eq!(lib.find(&cand(&["Daft Punk", "Pharrell Williams", "Nile Rodgers"], "Get Lucky (feat. Pharrell Williams & Nile Rodgers)", "Random Access Memories", 369.6)), Some(3));
        // Too far off in length to be the same recording.
        assert_eq!(lib.find(&cand(&["Someone"], "Long Song", "Other", 320.0)), None);
        assert_eq!(lib.find(&cand(&["Someone"], "Long Song", "Album", 306.0)), Some(4));
        assert_eq!(lib.find(&cand(&["Nobody"], "Here Comes the Sun", "", 185.0)), None);
    }
}
