//! Plugins: folders under `<data dir>/plugins/<id>/` holding a `plugin.json`
//! manifest and an ES module the UI imports. Plugins that ship with
//! Honeycomb sit in the app's resources (`plugins/` in the repo); one of the
//! same id in the user's folder takes its place. They're served over the
//! `plugin://` protocol, and get network access only to the hosts their
//! manifest lists. Plugins run in the app's own webview, so these limits are
//! guard rails against mistakes, not a sandbox: only enable plugins you trust.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::http::{Response, StatusCode, header};


pub const MANIFEST: &str = "plugin.json";
/// Bumped when the UI's plugin API changes in a way that breaks plugins.
pub const API_VERSION: u32 = 1;
/// Largest HTTP response body handed to a plugin.
const MAX_BODY: u64 = 32 * 1024 * 1024;
/// Largest browser request the OAuth redirect listener reads.
const MAX_REDIRECT_REQUEST: u64 = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// The module the UI imports, relative to the plugin's folder.
    #[serde(default = "default_main")]
    pub main: String,
    #[serde(default = "default_api")]
    pub api_version: u32,
    /// Hosts `http` may reach, e.g. "api.spotify.com". HTTPS only.
    #[serde(default)]
    pub hosts: Vec<String>,
    /// Ports on 127.0.0.1 the plugin may listen on for an OAuth redirect.
    #[serde(default)]
    pub loopback_ports: Vec<u16>,
}

fn default_main() -> String {
    "main.js".into()
}

fn default_api() -> u32 {
    API_VERSION
}

/// A folder in the plugins directory, with its manifest or why it couldn't be read.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub folder: String,
    pub manifest: Option<Manifest>,
    pub error: Option<String>,
    /// Ships with Honeycomb.
    pub bundled: bool,
}

/// Where plugins are looked for: the user's folder first, then the ones that
/// ship with the app.
#[derive(Debug, Clone)]
pub struct Dirs {
    pub user: PathBuf,
    pub bundled: Option<PathBuf>,
}

impl Dirs {
    /// The folder holding plugin `id`'s folder.
    pub fn dir_of(&self, id: &str) -> &Path {
        match &self.bundled {
            Some(b) if !self.user.join(id).is_dir() && b.join(id).is_dir() => b,
            _ => &self.user,
        }
    }

    pub fn manifest(&self, id: &str) -> Result<Manifest> {
        read_manifest(self.dir_of(id), id)
    }

    pub fn list(&self) -> Vec<Listed> {
        let mut out = list(&self.user);
        if let Some(b) = &self.bundled {
            for mut p in list(b) {
                if !out.iter().any(|u| u.folder == p.folder) {
                    p.bundled = true;
                    out.push(p);
                }
            }
        }
        out.sort_by(|a, b| a.folder.cmp(&b.folder));
        out
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

pub fn read_manifest(dir: &Path, id: &str) -> Result<Manifest> {
    if !valid_id(id) {
        bail!("plugin ids are lowercase letters, digits, - and _");
    }
    let raw = std::fs::read(dir.join(id).join(MANIFEST)).with_context(|| format!("no {MANIFEST}"))?;
    let m: Manifest = serde_json::from_slice(&raw).with_context(|| format!("{MANIFEST} is not valid"))?;
    if m.id != id {
        bail!("{MANIFEST} says its id is \"{}\", but the folder is named \"{id}\"", m.id);
    }
    if m.api_version > API_VERSION {
        bail!("needs a newer Honeycomb (plugin API {}, this build has {API_VERSION})", m.api_version);
    }
    Ok(m)
}

pub fn list(dir: &Path) -> Vec<Listed> {
    let mut out: Vec<Listed> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .map(|folder| match read_manifest(dir, &folder) {
            Ok(m) => Listed { folder, manifest: Some(m), error: None, bundled: false },
            Err(e) => Listed { folder, manifest: None, error: Some(format!("{e:#}")), bundled: false },
        })
        .collect();
    out.sort_by(|a, b| a.folder.cmp(&b.folder));
    out
}

// ---------- plugin:// protocol ----------

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The file `uri_path` ("/<id>/<file>") names, if it's inside that plugin's folder.
fn resolve(dir: &Path, uri_path: &str) -> Option<PathBuf> {
    let path = percent_decode(uri_path);
    let (id, rest) = path.trim_start_matches('/').split_once('/')?;
    if !valid_id(id) || rest.is_empty() {
        return None;
    }
    let root = dir.join(id).canonicalize().ok()?;
    let file = root.join(rest).canonicalize().ok()?;
    (file.starts_with(&root) && file.is_file()).then_some(file)
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("js" | "mjs") => "text/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("html") => "text/html",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

/// Ids in the `plugins.enabled` setting (a JSON list).
pub fn enabled_ids(conn: &Connection) -> Vec<String> {
    conn.query_row("SELECT value FROM settings WHERE key = 'plugins.enabled'", [], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

/// Serves plugin files at `plugin://localhost/<id>/<file>` (`http://plugin.localhost/…` on Windows),
/// for plugins that are turned on.
pub fn serve(dirs: &Dirs, enabled: &[String], uri_path: &str) -> Response<Vec<u8>> {
    let id = percent_decode(uri_path).trim_start_matches('/').split('/').next().unwrap_or("").to_string();
    let file = enabled.contains(&id).then(|| resolve(dirs.dir_of(&id), uri_path)).flatten();
    let Some(file) = file else {
        return Response::builder().status(StatusCode::NOT_FOUND).body(Vec::new()).unwrap();
    };
    match std::fs::read(&file) {
        Ok(data) => Response::builder()
            .header(header::CONTENT_TYPE, content_type(&file))
            // Module scripts from a custom scheme count as cross-origin.
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(data)
            .unwrap(),
        Err(_) => Response::builder().status(StatusCode::NOT_FOUND).body(Vec::new()).unwrap(),
    }
}

// ---------- HTTP for plugins ----------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpRequest {
    pub url: String,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}

/// The host of an https:// URL, without a port.
fn https_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') {
        return None;
    }
    let host = authority.rsplit_once(':').map_or(authority, |(h, _)| h);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

pub fn check_host(m: &Manifest, url: &str) -> Result<()> {
    let host = https_host(url).ok_or_else(|| anyhow!("plugins can only make https:// requests"))?;
    if m.hosts.iter().any(|h| h.eq_ignore_ascii_case(&host)) {
        Ok(())
    } else {
        bail!("{} isn't in the hosts {MANIFEST} lists for {}", host, m.id)
    }
}

pub fn http(m: &Manifest, req: &HttpRequest) -> Result<HttpResponse> {
    check_host(m, &req.url)?;
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .redirects(0)
        .user_agent(concat!("Honeycomb/", env!("CARGO_PKG_VERSION"), " (desktop music player)"))
        .build();
    let method = req.method.as_deref().unwrap_or("GET").to_ascii_uppercase();
    let mut r = agent.request(&method, &req.url);
    for (k, v) in &req.headers {
        r = r.set(k, v);
    }
    let resp = match match &req.body {
        Some(b) => r.send_string(b),
        None => r.call(),
    } {
        Ok(resp) | Err(ureq::Error::Status(_, resp)) => resp,
        Err(e) => return Err(anyhow!("{}: {e}", req.url)),
    };
    let headers = resp
        .headers_names()
        .into_iter()
        .filter_map(|k| resp.header(&k).map(|v| (k.to_ascii_lowercase(), v.to_string())))
        .collect();
    let status = resp.status();
    let mut body = String::new();
    resp.into_reader().take(MAX_BODY).read_to_string(&mut body).context("reading the response")?;
    Ok(HttpResponse { status, headers, body })
}

// ---------- OAuth redirect on a loopback port ----------

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Honeycomb</title>\
<body style=\"font:16px system-ui;margin:4em;text-align:center\">\
<p>Signed in. You can close this tab and go back to Honeycomb.</p>";

/// The running wait on each port, so a new sign-in can take the port over.
static WAITING: Mutex<Vec<(u16, Arc<AtomicBool>)>> = Mutex::new(Vec::new());

/// Wait for one browser request to `http://127.0.0.1:<port><path>` and return
/// its query string (without the "?"). Other paths get a 404 and are ignored.
pub fn wait_for_redirect(m: &Manifest, port: u16, path: &str, timeout: Duration) -> Result<String> {
    if !m.loopback_ports.contains(&port) {
        bail!("port {port} isn't in the loopbackPorts {MANIFEST} lists for {}", m.id);
    }
    // Stop an earlier sign-in still waiting on this port, then take it over.
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut waiting = WAITING.lock().unwrap_or_else(|p| p.into_inner());
        for (_, old) in waiting.iter().filter(|(p, _)| *p == port) {
            old.store(true, Ordering::SeqCst);
        }
        waiting.retain(|(p, _)| *p != port);
        waiting.push((port, cancel.clone()));
    }
    let bind_until = Instant::now() + Duration::from_secs(2);
    let listener = loop {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => break l,
            Err(_) if Instant::now() < bind_until => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e).with_context(|| format!("port {port} is in use")),
        }
    };
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + timeout;
    loop {
        let stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if cancel.load(Ordering::SeqCst) {
                    bail!("replaced by a newer sign-in");
                }
                if Instant::now() >= deadline {
                    bail!("timed out waiting for the browser");
                }
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut line = String::new();
        let mut reader = BufReader::new((&stream).take(MAX_REDIRECT_REQUEST));
        if reader.read_line(&mut line).is_err() {
            continue;
        }
        // Drain the headers so the browser sees a clean response.
        let mut h = String::new();
        while reader.read_line(&mut h).is_ok_and(|n| n > 2) {
            h.clear();
        }
        let target = line.split_whitespace().nth(1).unwrap_or("");
        let (p, query) = target.split_once('?').unwrap_or((target, ""));
        let mut stream = &stream;
        if line.starts_with("GET ") && p == path {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{DONE_PAGE}",
                DONE_PAGE.len()
            );
            return Ok(query.to_string());
        }
        let _ = write!(stream, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    }
}

// ---------- secrets ----------

/// Plugin secrets are named `secret.plugin.<id>.<key>` in the system keychain (see `secrets`).
fn secret_key(id: &str, key: &str) -> String {
    format!("secret.plugin.{id}.{key}")
}

pub fn secret_get(conn: &Connection, id: &str, key: &str) -> Option<String> {
    crate::secrets::get(conn, &secret_key(id, key))
}

pub fn secret_set(conn: &Connection, id: &str, key: &str, value: Option<&str>) -> Result<(), String> {
    match value {
        Some(v) => crate::secrets::set(conn, &secret_key(id, key), v),
        None => crate::secrets::delete(conn, &secret_key(id, key)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_hosts() {
        assert_eq!(https_host("https://api.spotify.com/v1/me").as_deref(), Some("api.spotify.com"));
        assert_eq!(https_host("https://API.spotify.com:443?x").as_deref(), Some("api.spotify.com"));
        assert_eq!(https_host("http://api.spotify.com/"), None);
        assert_eq!(https_host("https://evil.com@api.spotify.com/"), None);
    }

    #[test]
    fn new_sign_in_takes_over_the_port() {
        let m: Manifest = serde_json::from_str(r#"{"id":"t","name":"T","loopbackPorts":[47399]}"#).unwrap();
        let m2 = m.clone();
        let first = std::thread::spawn(move || wait_for_redirect(&m2, 47399, "/cb", Duration::from_secs(10)));
        std::thread::sleep(Duration::from_millis(300));
        let m3 = m.clone();
        let second = std::thread::spawn(move || wait_for_redirect(&m3, 47399, "/cb", Duration::from_secs(10)));
        assert!(first.join().unwrap().is_err());
        std::thread::sleep(Duration::from_millis(300));
        let mut s = std::net::TcpStream::connect(("127.0.0.1", 47399)).unwrap();
        write!(s, "GET /cb?code=x HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
        assert_eq!(second.join().unwrap().unwrap(), "code=x");
    }

    #[test]
    fn oversized_requests_dont_stall_the_redirect() {
        let m: Manifest = serde_json::from_str(r#"{"id":"t","name":"T","loopbackPorts":[47398]}"#).unwrap();
        let waiter = std::thread::spawn(move || wait_for_redirect(&m, 47398, "/cb", Duration::from_secs(10)));
        std::thread::sleep(Duration::from_millis(300));
        // An endless header line is cut off at the cap and answered with a 404.
        let mut junk = std::net::TcpStream::connect(("127.0.0.1", 47398)).unwrap();
        let _ = write!(junk, "GET /x HTTP/1.1\r\nX: {}", "a".repeat(64 * 1024));
        let mut answer = String::new();
        let _ = junk.read_to_string(&mut answer);
        assert!(answer.starts_with("HTTP/1.1 404"));
        let mut s = std::net::TcpStream::connect(("127.0.0.1", 47398)).unwrap();
        write!(s, "GET /cb?code=y HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(waiter.join().unwrap().unwrap(), "code=y");
    }

    #[test]
    fn serves_only_inside_the_plugin_folder() {
        let dir = std::env::temp_dir().join(format!("honeycomb-plugins-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("demo/lib")).unwrap();
        std::fs::write(dir.join("demo/main.js"), "export {}").unwrap();
        std::fs::write(dir.join("demo/lib/x.js"), "").unwrap();
        std::fs::write(dir.join("secret.txt"), "").unwrap();
        assert!(resolve(&dir, "/demo/main.js").is_some());
        assert!(resolve(&dir, "/demo%2Flib%2Fx.js").is_some());
        assert!(resolve(&dir, "/demo/../secret.txt").is_none());
        assert!(resolve(&dir, "/demo/%2E%2E/secret.txt").is_none());
        assert!(resolve(&dir, "/Demo/main.js").is_none());
        // Plugins that are turned off aren't served at all.
        let dirs = Dirs { user: dir.clone(), bundled: None };
        assert_eq!(serve(&dirs, &["demo".into()], "/demo/main.js").status(), StatusCode::OK);
        assert_eq!(serve(&dirs, &[], "/demo/main.js").status(), StatusCode::NOT_FOUND);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn user_plugins_take_the_place_of_bundled_ones() {
        let root = std::env::temp_dir().join(format!("honeycomb-plugins-dirs-{}", std::process::id()));
        let write = |dir: &str, id: &str, name: &str| {
            std::fs::create_dir_all(root.join(dir).join(id)).unwrap();
            let m = format!(r#"{{"id":"{id}","name":"{name}"}}"#);
            std::fs::write(root.join(dir).join(id).join(MANIFEST), m).unwrap();
            std::fs::write(root.join(dir).join(id).join("main.js"), name).unwrap();
        };
        write("bundled", "scrobbling", "Built in");
        write("bundled", "other", "Other");
        write("user", "scrobbling", "Mine");
        let dirs = Dirs { user: root.join("user"), bundled: Some(root.join("bundled")) };
        let listed = dirs.list();
        let names: Vec<(&str, bool)> =
            listed.iter().map(|p| (p.manifest.as_ref().unwrap().name.as_str(), p.bundled)).collect();
        assert_eq!(names, [("Other", true), ("Mine", false)]);
        assert_eq!(dirs.manifest("other").unwrap().name, "Other");
        let ids = ["scrobbling".to_string(), "other".to_string()];
        assert_eq!(serve(&dirs, &ids, "/scrobbling/main.js").body(), b"Mine");
        assert_eq!(serve(&dirs, &ids, "/other/main.js").body(), b"Other");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
