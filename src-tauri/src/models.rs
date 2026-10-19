use serde::{Deserialize, Serialize};

/// A track as stored in the library database and sent to the UI.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: i64,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub genre: String,
    pub composer: String,
    pub comment: String,
    pub year: Option<u32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub duration: f64,
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub format: String,
    /// Half-stars, 0-10.
    pub rating: u8,
    pub play_count: u32,
    pub last_played: Option<i64>,
    pub date_added: i64,
    pub mtime: i64,
    pub has_art: bool,
    pub loved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    /// "manual" or "smart"
    pub kind: String,
    pub rules: Option<SmartRules>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartRules {
    /// "all" or "any"
    pub r#match: String,
    pub rules: Vec<SmartRule>,
    pub limit: Option<u32>,
    /// A track field name, or "random"
    pub sort_by: Option<String>,
    #[serde(default)]
    pub sort_desc: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartRule {
    pub field: String,
    pub op: String,
    pub value: String,
}

/// Fields to change in the tag editor. `None` leaves a field untouched,
/// an empty string clears it.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TagEdit {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub composer: Option<String>,
    pub comment: Option<String>,
    pub year: Option<String>,
    pub track_no: Option<String>,
    pub disc_no: Option<String>,
    /// Image file to embed as the front cover.
    pub art_path: Option<String>,
    #[serde(default)]
    pub remove_art: bool,
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
