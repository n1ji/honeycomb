//! Playing a song while it downloads. The download goes into a cache file,
//! and readers wait for the bytes they need, so the decoder can start as
//! soon as the first chunk is in. Finished files stay as a cache.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};

/// How long a reader waits for a download that stopped moving.
const STALL: Duration = Duration::from_secs(20);

#[derive(Default)]
struct Progress {
    started: bool,
    have: u64,
    total: Option<u64>,
    done: bool,
    error: Option<String>,
}

pub struct Download {
    part: PathBuf,
    state: Mutex<Progress>,
    cv: Condvar,
}

static ACTIVE: LazyLock<Mutex<HashMap<PathBuf, Weak<Download>>>> = LazyLock::new(Default::default);

impl Download {
    fn lock(&self) -> std::sync::MutexGuard<'_, Progress> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn update(&self, f: impl FnOnce(&mut Progress)) {
        f(&mut self.lock());
        self.cv.notify_all();
    }

    /// Wait until `f` holds, or the download fails or stops moving.
    fn wait(&self, f: impl Fn(&Progress) -> bool) -> io::Result<std::sync::MutexGuard<'_, Progress>> {
        let mut st = self.lock();
        let mut last = (st.have, Instant::now());
        loop {
            if f(&st) {
                return Ok(st);
            }
            if let Some(e) = &st.error {
                return Err(io::Error::other(e.clone()));
            }
            if st.done {
                return Ok(st);
            }
            if st.have != last.0 {
                last = (st.have, Instant::now());
            } else if last.1.elapsed() > STALL {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "the server stopped sending"));
            }
            st = self.cv.wait_timeout(st, Duration::from_millis(250)).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

/// What a song's audio is read from.
pub enum Opened {
    File(File),
    Stream(Reader),
}

/// Open `cache` for reading: the finished file if it's there, else a reader
/// over its download, started with `request` when nothing else is fetching
/// it. Returns once the server has answered.
pub fn open(cache: &Path, request: impl FnOnce() -> ureq::Request) -> io::Result<Opened> {
    if let Ok(f) = File::open(cache) {
        let _ = f.set_modified(SystemTime::now());
        return Ok(Opened::File(f));
    }
    let dl = start(cache, request)?;
    drop(dl.wait(|p| p.started)?);
    let file = File::open(&dl.part)?;
    Ok(Opened::Stream(Reader { dl, file, pos: 0 }))
}

/// Start fetching `cache` in the background unless it's there or on its way.
pub fn prefetch(cache: &Path, request: impl FnOnce() -> ureq::Request) {
    if !cache.is_file() {
        let _ = start(cache, request);
    }
}

fn start(cache: &Path, request: impl FnOnce() -> ureq::Request) -> io::Result<Arc<Download>> {
    let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(dl) = active.get(cache).and_then(Weak::upgrade) {
        return Ok(dl);
    }
    if let Some(dir) = cache.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut part = cache.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let out = File::create(&part)?;
    let dl = Arc::new(Download { part, state: Mutex::default(), cv: Condvar::new() });
    active.retain(|_, w| w.strong_count() > 0);
    active.insert(cache.to_path_buf(), Arc::downgrade(&dl));
    let req = request();
    let (dl2, dest) = (dl.clone(), cache.to_path_buf());
    std::thread::spawn(move || {
        if let Err(e) = fetch(&dl2, req, out, &dest) {
            let _ = fs::remove_file(&dl2.part);
            dl2.update(|p| p.error = Some(e));
        }
        ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).remove(&dest);
    });
    Ok(dl)
}

fn fetch(dl: &Download, req: ureq::Request, mut out: File, dest: &Path) -> Result<(), String> {
    let resp = req.call().map_err(|e| match e {
        ureq::Error::Status(code, _) => format!("The server answered {code}"),
        e => format!("Can't reach the server: {e}"),
    })?;
    if resp.content_type().contains("json") || resp.content_type().contains("xml") {
        // Subsonic reports errors as a 200 with a JSON body.
        let mut body = String::new();
        let _ = resp.into_reader().take(64 * 1024).read_to_string(&mut body);
        return Err(format!("The server refused the stream: {}", body.chars().take(200).collect::<String>()));
    }
    let total = resp.header("Content-Length").and_then(|v| v.parse().ok());
    dl.update(|p| {
        p.started = true;
        p.total = total;
    });
    let mut body = resp.into_reader();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = body.read(&mut buf).map_err(|e| format!("The download broke off: {e}"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        dl.update(|p| p.have += n as u64);
    }
    out.flush().map_err(|e| e.to_string())?;
    let have = dl.lock().have;
    if total.is_some_and(|t| t != have) {
        return Err(format!("The download stopped at {have} of {} bytes", total.unwrap_or(0)));
    }
    fs::rename(&dl.part, dest).map_err(|e| e.to_string())?;
    dl.update(|p| {
        p.done = true;
        p.total = Some(have);
    });
    Ok(())
}

/// Reads a download in progress, waiting for bytes that aren't in yet.
pub struct Reader {
    dl: Arc<Download>,
    file: File,
    pos: u64,
}

impl Reader {
    pub fn len(&self) -> Option<u64> {
        self.dl.lock().total
    }
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let pos = self.pos;
        let have = self.dl.wait(|p| p.have > pos)?.have;
        if have <= pos {
            return Ok(0);
        }
        let n = buf.len().min((have - pos) as usize);
        self.file.seek(SeekFrom::Start(pos))?;
        let n = self.file.read(&mut buf[..n])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Reader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let new = match to {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(d) => self.pos as i64 + d,
            SeekFrom::End(d) => {
                let total = match self.len() {
                    Some(t) => t,
                    None => self.dl.wait(|p| p.done)?.have,
                };
                total as i64 + d
            }
        };
        if new < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before the start"));
        }
        self.pos = new as u64;
        Ok(self.pos)
    }
}

impl symphonia::core::io::MediaSource for Reader {
    fn is_seekable(&self) -> bool {
        self.len().is_some()
    }

    fn byte_len(&self) -> Option<u64> {
        self.len()
    }
}

/// Delete the least recently played cache files until `dir` holds at most `max` bytes.
pub fn trim(dir: &Path, max: u64) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x != "part"))
        .filter_map(|e| {
            let md = e.metadata().ok()?;
            Some((md.modified().ok()?, md.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort();
    for (_, len, path) in files {
        if total <= max {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Serves `body` in `chunks` pieces, `gap` apart.
    fn slow_server(body: Vec<u8>, chunks: usize, gap: Duration) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for s in listener.incoming() {
                let Ok(mut s) = s else { continue };
                let mut req = [0u8; 2048];
                let _ = s.read(&mut req);
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: audio/flac\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                for piece in body.chunks(body.len().div_ceil(chunks)) {
                    let _ = s.write_all(piece);
                    let _ = s.flush();
                    std::thread::sleep(gap);
                }
            }
        });
        url
    }

    #[test]
    fn readers_wait_for_bytes_and_the_file_is_kept() {
        let dir = tempdir("stream-wait");
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let url = slow_server(body.clone(), 8, Duration::from_millis(40));
        let cache = dir.join("song.flac");
        let Opened::Stream(mut r) = open(&cache, || ureq::get(&url)).unwrap() else { panic!("expected a stream") };
        assert_eq!(r.len(), Some(body.len() as u64));
        let mut got = Vec::new();
        r.read_to_end(&mut got).unwrap();
        assert_eq!(got, body);
        r.seek(SeekFrom::Start(1000)).unwrap();
        let mut b = [0u8; 10];
        r.read_exact(&mut b).unwrap();
        assert_eq!(&b[..], &body[1000..1010]);
        assert_eq!(r.seek(SeekFrom::End(-5)).unwrap(), body.len() as u64 - 5);
        drop(r);
        for _ in 0..50 {
            if cache.is_file() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fs::read(&cache).unwrap(), body);
        assert!(matches!(open(&cache, || panic!("no second download")).unwrap(), Opened::File(_)));
    }

    #[test]
    fn two_readers_share_one_download() {
        let dir = tempdir("stream-share");
        let body = vec![7u8; 50_000];
        let url = slow_server(body.clone(), 4, Duration::from_millis(30));
        let cache = dir.join("a.mp3");
        let Opened::Stream(mut a) = open(&cache, || ureq::get(&url)).unwrap() else { panic!() };
        let Opened::Stream(mut b) = open(&cache, || panic!("started twice")).unwrap() else { panic!() };
        let (mut x, mut y) = (Vec::new(), Vec::new());
        a.read_to_end(&mut x).unwrap();
        b.read_to_end(&mut y).unwrap();
        assert_eq!(x, body);
        assert_eq!(y, body);
    }

    #[test]
    fn errors_reach_the_opener() {
        let dir = tempdir("stream-err");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for s in listener.incoming() {
                let Ok(mut s) = s else { continue };
                let mut req = [0u8; 2048];
                let _ = s.read(&mut req);
                let body = r#"{"subsonic-response":{"status":"failed","error":{"code":70,"message":"not found"}}}"#;
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            }
        });
        let Err(e) = open(&dir.join("x.ogg"), || ureq::get(&url)) else { panic!("expected an error") };
        assert!(e.to_string().contains("not found"), "{e}");
        assert!(!dir.join("x.ogg").exists() && !dir.join("x.ogg.part").exists());
    }

    #[test]
    fn trim_drops_the_oldest_files() {
        let dir = tempdir("stream-trim");
        for (i, name) in ["old.mp3", "mid.mp3", "new.mp3"].iter().enumerate() {
            let p = dir.join(name);
            fs::write(&p, vec![0u8; 100]).unwrap();
            let f = File::options().write(true).open(&p).unwrap();
            f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1000 + i as u64)).unwrap();
        }
        trim(&dir, 250);
        assert!(!dir.join("old.mp3").exists());
        assert!(dir.join("mid.mp3").exists() && dir.join("new.mp3").exists());
    }

    fn tempdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("honeycomb-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }
}
