use serde::ser::{SerializeSeq, SerializeStruct, SerializeTuple};
use serde::{Deserialize, Serialize, Serializer};

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
    /// Newly found and not reviewed yet.
    pub inbox: bool,
}

/// The whole library for the UI in a compact form: the field names once, then
/// one array of values per track. With 100k tracks, repeating every key in
/// every object roughly doubled the JSON and the time to send and parse it.
pub struct TrackTable(pub Vec<Track>);

/// `Track`'s fields as the UI names them, in the order `TrackTable` sends them.
pub const TRACK_FIELDS: [&str; 24] = [
    "id",
    "path",
    "title",
    "artist",
    "albumArtist",
    "album",
    "genre",
    "composer",
    "comment",
    "year",
    "trackNo",
    "discNo",
    "duration",
    "bitrate",
    "sampleRate",
    "format",
    "rating",
    "playCount",
    "lastPlayed",
    "dateAdded",
    "mtime",
    "hasArt",
    "loved",
    "inbox",
];

impl Serialize for TrackTable {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        struct Rows<'a>(&'a [Track]);
        struct Row<'a>(&'a Track);
        impl Serialize for Rows<'_> {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let mut seq = s.serialize_seq(Some(self.0.len()))?;
                for t in self.0 {
                    seq.serialize_element(&Row(t))?;
                }
                seq.end()
            }
        }
        impl Serialize for Row<'_> {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let t = self.0;
                let mut r = s.serialize_tuple(TRACK_FIELDS.len())?;
                r.serialize_element(&t.id)?;
                r.serialize_element(&t.path)?;
                r.serialize_element(&t.title)?;
                r.serialize_element(&t.artist)?;
                r.serialize_element(&t.album_artist)?;
                r.serialize_element(&t.album)?;
                r.serialize_element(&t.genre)?;
                r.serialize_element(&t.composer)?;
                r.serialize_element(&t.comment)?;
                r.serialize_element(&t.year)?;
                r.serialize_element(&t.track_no)?;
                r.serialize_element(&t.disc_no)?;
                r.serialize_element(&t.duration)?;
                r.serialize_element(&t.bitrate)?;
                r.serialize_element(&t.sample_rate)?;
                r.serialize_element(&t.format)?;
                r.serialize_element(&t.rating)?;
                r.serialize_element(&t.play_count)?;
                r.serialize_element(&t.last_played)?;
                r.serialize_element(&t.date_added)?;
                r.serialize_element(&t.mtime)?;
                r.serialize_element(&t.has_art)?;
                r.serialize_element(&t.loved)?;
                r.serialize_element(&t.inbox)?;
                r.end()
            }
        }
        let mut m = s.serialize_struct("TrackTable", 2)?;
        m.serialize_field("fields", &TRACK_FIELDS)?;
        m.serialize_field("rows", &Rows(&self.0))?;
        m.end()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The compact table must carry exactly what a `Track` object does, under
    /// the same names, so the UI can rebuild identical objects.
    #[test]
    fn track_table_matches_track_objects() {
        let t = Track {
            id: 7,
            path: "/m/a.flac".into(),
            title: "T".into(),
            artist: "A".into(),
            album_artist: "AA".into(),
            album: "Al".into(),
            genre: "G".into(),
            composer: "C".into(),
            comment: "Cm".into(),
            year: Some(1998),
            track_no: Some(3),
            disc_no: None,
            duration: 123.5,
            bitrate: Some(900),
            sample_rate: Some(44100),
            format: "FLAC".into(),
            rating: 7,
            play_count: 4,
            last_played: Some(11),
            date_added: 12,
            mtime: 13,
            has_art: true,
            loved: true,
            inbox: false,
        };
        let obj = serde_json::to_value(&t).unwrap();
        let table = serde_json::to_value(TrackTable(vec![t])).unwrap();
        let fields: Vec<&str> = table["fields"].as_array().unwrap().iter().map(|f| f.as_str().unwrap()).collect();
        let row = table["rows"][0].as_array().unwrap();
        assert_eq!(fields.len(), obj.as_object().unwrap().len(), "a Track field is missing from TRACK_FIELDS");
        assert_eq!(row.len(), fields.len());
        for (i, f) in fields.iter().enumerate() {
            assert_eq!(&row[i], &obj[*f], "field {f}");
        }
    }
}
