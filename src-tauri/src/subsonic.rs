//! A client for Subsonic and OpenSubsonic servers (Navidrome and others).

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

const API_VERSION: &str = "1.16.1";
const CLIENT: &str = "Honeycomb";
/// A page of 500 songs with every OpenSubsonic field is around 1 MB.
const MAX_ANSWER: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum Auth {
    Password(String),
    ApiKey(String),
}

#[derive(Debug, Clone)]
pub struct Client {
    pub base: String,
    pub user: String,
    pub auth: Auth,
    agent: ureq::Agent,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Song {
    pub id: String,
    pub title: String,
    pub album: String,
    pub artist: String,
    pub display_artist: Option<String>,
    pub display_album_artist: Option<String>,
    pub display_composer: Option<String>,
    pub album_id: Option<String>,
    pub track: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub cover_art: Option<String>,
    pub size: Option<u64>,
    pub suffix: Option<String>,
    pub duration: Option<f64>,
    pub bit_rate: Option<u32>,
    pub sampling_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub comment: Option<String>,
    pub created: Option<String>,
    pub starred: Option<String>,
    pub user_rating: Option<u8>,
    pub play_count: Option<u32>,
    pub played: Option<String>,
    pub is_video: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Album {
    pub id: String,
    pub name: String,
    pub song: Vec<Song>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub owner: Option<String>,
    pub song_count: u32,
    pub changed: Option<String>,
    pub entry: Vec<Song>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerInfo {
    pub kind: String,
    pub version: String,
    pub open_subsonic: bool,
    pub extensions: Vec<String>,
}

impl Client {
    pub fn new(base: &str, user: &str, auth: Auth) -> Client {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(60))
            .build();
        Client { base: normalize_url(base), user: user.to_string(), auth, agent }
    }

    pub fn request(&self, endpoint: &str) -> ureq::Request {
        let r = self
            .agent
            .get(&format!("{}/rest/{endpoint}", self.base))
            .query("v", API_VERSION)
            .query("c", CLIENT)
            .query("f", "json");
        match &self.auth {
            Auth::ApiKey(key) => r.query("apiKey", key),
            Auth::Password(pw) => {
                let salt = salt();
                r.query("u", &self.user).query("t", &token(pw, &salt)).query("s", &salt)
            }
        }
    }

    fn call(&self, endpoint: &str, params: &[(&str, &str)]) -> Result<Value> {
        let mut r = self.request(endpoint);
        for (k, v) in params {
            r = r.query(k, v);
        }
        let resp = match r.call() {
            Ok(resp) => resp,
            Err(e) => bail!(describe(&e)),
        };
        let body: Value = crate::net::read_json(resp, MAX_ANSWER).with_context(|| format!("{endpoint}: not a Subsonic answer"))?;
        let inner = body.get("subsonic-response").cloned().ok_or_else(|| anyhow!("{endpoint}: not a Subsonic answer"))?;
        if inner.get("status").and_then(Value::as_str) != Some("ok") {
            let e = inner.get("error");
            let code = e.and_then(|e| e.get("code")).and_then(Value::as_i64).unwrap_or(0);
            let msg = e.and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or("unknown error");
            bail!(match code {
                40 | 41 | 44 => "The server didn't accept the username or password".to_string(),
                _ => format!("The server refused {endpoint}: {msg}"),
            });
        }
        Ok(inner)
    }

    fn get<T: DeserializeOwned + Default>(&self, endpoint: &str, params: &[(&str, &str)], key: &str) -> Result<T> {
        let inner = self.call(endpoint, params)?;
        match inner.get(key) {
            Some(v) => serde_json::from_value(v.clone()).with_context(|| format!("{endpoint}: unexpected {key}")),
            None => Ok(T::default()),
        }
    }

    pub fn ping(&self) -> Result<ServerInfo> {
        let r = self.call("ping", &[])?;
        let s = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let open = r.get("openSubsonic").and_then(Value::as_bool).unwrap_or(false);
        let mut info = ServerInfo { kind: s("type"), version: s("serverVersion"), open_subsonic: open, extensions: Vec::new() };
        if open {
            #[derive(Deserialize, Default)]
            struct Ext {
                name: String,
            }
            let exts: Vec<Ext> = self.get("getOpenSubsonicExtensions", &[], "openSubsonicExtensions").unwrap_or_default();
            info.extensions = exts.into_iter().map(|e| e.name).collect();
        }
        Ok(info)
    }

    pub fn songs_page(&self, offset: usize, count: usize) -> Result<Vec<Song>> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct R {
            song: Vec<Song>,
        }
        let (o, c) = (offset.to_string(), count.to_string());
        let params = [("query", ""), ("artistCount", "0"), ("albumCount", "0"), ("songCount", &c), ("songOffset", &o)];
        let r: R = self.get("search3", &params, "searchResult3")?;
        Ok(r.song)
    }

    pub fn album_ids_page(&self, offset: usize, count: usize) -> Result<Vec<String>> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct R {
            album: Vec<Album>,
        }
        let (o, c) = (offset.to_string(), count.to_string());
        let params = [("type", "alphabeticalByName"), ("size", c.as_str()), ("offset", o.as_str())];
        let r: R = self.get("getAlbumList2", &params, "albumList2")?;
        Ok(r.album.into_iter().map(|a| a.id).collect())
    }

    pub fn album(&self, id: &str) -> Result<Album> {
        self.get("getAlbum", &[("id", id)], "album")
    }

    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct R {
            playlist: Vec<Playlist>,
        }
        Ok(self.get::<R>("getPlaylists", &[], "playlists")?.playlist)
    }

    pub fn playlist(&self, id: &str) -> Result<Playlist> {
        self.get("getPlaylist", &[("id", id)], "playlist")
    }

    pub fn save_playlist(&self, id: Option<&str>, name: &str, songs: &[String]) -> Result<String> {
        let mut params: Vec<(&str, &str)> = match id {
            Some(id) => vec![("playlistId", id)],
            None => vec![("name", name)],
        };
        params.extend(songs.iter().map(|s| ("songId", s.as_str())));
        let p: Playlist = self.get("createPlaylist", &params, "playlist")?;
        if let Some(id) = id {
            // createPlaylist with an id keeps the old name.
            self.call("updatePlaylist", &[("playlistId", id), ("name", name)])?;
            return Ok(id.to_string());
        }
        if !p.id.is_empty() {
            return Ok(p.id);
        }
        // Subsonic 1.14 and older answer with nothing: find it by name.
        self.playlists()?
            .into_iter()
            .filter(|p| p.name == name)
            .max_by(|a, b| a.changed.cmp(&b.changed))
            .map(|p| p.id)
            .ok_or_else(|| anyhow!("The server didn't say which playlist it made"))
    }

    pub fn delete_playlist(&self, id: &str) -> Result<()> {
        self.call("deletePlaylist", &[("id", id)]).map(drop)
    }

    pub fn star(&self, id: &str, on: bool) -> Result<()> {
        self.call(if on { "star" } else { "unstar" }, &[("id", id)]).map(drop)
    }

    pub fn set_rating(&self, id: &str, stars: u8) -> Result<()> {
        self.call("setRating", &[("id", id), ("rating", &stars.min(5).to_string())]).map(drop)
    }

    pub fn scrobble(&self, id: &str, played_at: i64) -> Result<()> {
        let ms = (played_at * 1000).to_string();
        self.call("scrobble", &[("id", id), ("time", &ms), ("submission", "true")]).map(drop)
    }

    pub fn stream(&self, id: &str, transcode: Option<(&str, u32)>) -> ureq::Request {
        let r = self.request("stream").query("id", id);
        match transcode {
            Some((fmt, kbps)) => r.query("format", fmt).query("maxBitRate", &kbps.to_string()),
            None => r.query("format", "raw"),
        }
    }

    pub fn download(&self, id: &str) -> ureq::Request {
        self.request("download").query("id", id)
    }

    pub fn cover(&self, id: &str, size: u32) -> ureq::Request {
        self.request("getCoverArt").query("id", id).query("size", &size.to_string())
    }
}

/// A request error for people to read. ureq's own messages include the URL,
/// and with it the login token, so they're never shown or logged.
pub fn describe(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Status(401 | 403, _) => "The server didn't accept the username or password".into(),
        ureq::Error::Status(code, _) => format!("The server answered with error {code}"),
        ureq::Error::Transport(t) => {
            let why = t.message().filter(|m| !m.contains("?")).map(|m| format!(" ({m})")).unwrap_or_default();
            format!("Can't reach the server: {}{why}", t.kind())
        }
    }
}

pub fn normalize_url(s: &str) -> String {
    let s = s.trim().trim_end_matches('/');
    let s = s.strip_suffix("/rest").unwrap_or(s);
    if s.starts_with("http://") || s.starts_with("https://") {
        return s.to_string();
    }
    let host = s.split([':', '/']).next().unwrap_or_default();
    let local = host == "localhost"
        || host.ends_with(".local")
        || host.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.is_private() || ip.is_loopback() || ip.is_link_local());
    format!("{}://{s}", if local { "http" } else { "https" })
}

fn salt() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..12).map(|_| format!("{:x}", rng.gen_range(0..16u8))).collect()
}

fn token(password: &str, salt: &str) -> String {
    use md5::{Digest, Md5};
    Md5::digest(format!("{password}{salt}").as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn parse_time(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[10] != b'T' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, sec) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    let rest = &s[19..];
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest.chars().next() {
        Some(sign @ ('+' | '-')) => {
            let hh: i64 = rest.get(1..3)?.parse().ok()?;
            let mm: i64 = rest.get(rest.len() - 2..)?.parse().ok()?;
            (hh * 3600 + mm * 60) * if sign == '-' { -1 } else { 1 }
        }
        _ => 0,
    };
    // Days from civil (Howard Hinnant's algorithm).
    let (y2, m2) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + mi * 60 + sec - offset)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    pub type Handler = Arc<dyn Fn(&str, &HashMap<String, Vec<String>>) -> (u16, String) + Send + Sync>;

    pub fn fake(handler: Handler) -> (String, Arc<Mutex<Vec<(String, HashMap<String, Vec<String>>)>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(Mutex::new(Vec::new()));
        let log2 = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut line = String::new();
                let mut r = BufReader::new(s.try_clone().unwrap());
                if r.read_line(&mut line).is_err() {
                    continue;
                }
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).is_err() || h.trim().is_empty() {
                        break;
                    }
                }
                let target = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (path, query) = target.split_once('?').unwrap_or((&target, ""));
                let endpoint = path.trim_start_matches("/rest/").to_string();
                let mut q: HashMap<String, Vec<String>> = HashMap::new();
                for pair in query.split('&').filter(|p| !p.is_empty()) {
                    let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                    q.entry(k.to_string()).or_default().push(decode(v));
                }
                log2.lock().unwrap().push((endpoint.clone(), q.clone()));
                let (code, body) = handler(&endpoint, &q);
                let _ = write!(
                    s,
                    "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (url, log)
    }

    fn decode(s: &str) -> String {
        let mut out = Vec::new();
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'%' if i + 2 < b.len() => {
                    out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'?'));
                    i += 3;
                }
                b'+' => {
                    out.push(b' ');
                    i += 1;
                }
                c => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    pub fn ok(extra: &str) -> String {
        let sep = if extra.is_empty() { "" } else { "," };
        format!(r#"{{"subsonic-response":{{"status":"ok","version":"1.16.1","type":"fake","openSubsonic":true{sep}{extra}}}}}"#)
    }

    #[test]
    fn requests_carry_a_salted_token() {
        let (url, log) = fake(Arc::new(|_, _| (200, ok(""))));
        let c = Client::new(&url, "ann", Auth::Password("sesame".into()));
        c.ping().unwrap();
        let log = log.lock().unwrap();
        let q = &log[0].1;
        assert_eq!(log[0].0, "ping");
        assert_eq!(q["u"][0], "ann");
        assert!(!q.contains_key("p"), "the password itself is never sent");
        let (t, s) = (&q["t"][0], &q["s"][0]);
        assert_eq!(*t, token("sesame", s));
        assert_eq!(token("sesame", "c19b2d"), "26719a1196d2a940705a59634eb18eab");
    }

    #[test]
    fn api_keys_replace_the_user_and_token() {
        let (url, log) = fake(Arc::new(|_, _| (200, ok(""))));
        Client::new(&url, "ann", Auth::ApiKey("k1".into())).ping().unwrap();
        let q = &log.lock().unwrap()[0].1;
        assert_eq!(q["apiKey"][0], "k1");
        assert!(!q.contains_key("u") && !q.contains_key("t"));
    }

    #[test]
    fn errors_become_readable_messages() {
        let (url, _) = fake(Arc::new(|ep, _| match ep {
            "ping" => (200, r#"{"subsonic-response":{"status":"failed","error":{"code":40,"message":"Wrong username or password"}}}"#.into()),
            _ => (500, String::new()),
        }));
        let c = Client::new(&url, "ann", Auth::Password("x".into()));
        assert!(c.ping().unwrap_err().to_string().contains("username or password"));
        assert!(c.playlists().unwrap_err().to_string().contains("500"));
    }

    #[test]
    fn extensions_are_read_when_the_server_has_them() {
        let (url, _) = fake(Arc::new(|ep, _| match ep {
            "getOpenSubsonicExtensions" => (200, ok(r#""openSubsonicExtensions":[{"name":"songLyrics","versions":[1]},{"name":"apiKeyAuthentication","versions":[1]}]"#)),
            _ => (200, ok(r#""serverVersion":"0.64.2""#)),
        }));
        let info = Client::new(&url, "a", Auth::Password("x".into())).ping().unwrap();
        assert_eq!(info.kind, "fake");
        assert_eq!(info.version, "0.64.2");
        assert_eq!(info.extensions, ["songLyrics", "apiKeyAuthentication"]);
    }

    #[test]
    fn errors_never_show_the_login() {
        let c = Client::new("http://127.0.0.1:9", "ann", Auth::Password("sesame".into()));
        let e = c.ping().unwrap_err().to_string();
        assert!(e.starts_with("Can't reach the server"), "{e}");
        assert!(!e.contains("ann") && !e.contains("t=") && !e.contains("rest/"), "{e}");
    }

    #[test]
    fn urls_get_a_scheme() {
        assert_eq!(normalize_url("music.example.com/"), "https://music.example.com");
        assert_eq!(normalize_url("192.168.1.5:4533"), "http://192.168.1.5:4533");
        assert_eq!(normalize_url("localhost:4533/rest"), "http://localhost:4533");
        assert_eq!(normalize_url("http://nas.example.com:4533"), "http://nas.example.com:4533");
    }

    #[test]
    fn iso_times_parse() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2026-11-09T09:18:38.791068+02:00"), Some(1794208718));
        assert_eq!(parse_time("2000-03-01T00:00:00-01:30"), Some(951874200));
        assert_eq!(parse_time("nope"), None);
    }
}
