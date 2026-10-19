//! System media controls: media keys, headphone buttons and the OS
//! "Now Playing" display (macOS Now Playing, MPRIS on Linux, SMTC on Windows).
//!
//! The controls live on the main thread. The audio thread reports changes
//! with `update`, which hops over to the main thread to apply them.

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};
use tauri::{AppHandle, Manager};

use crate::player::{Cmd, PlayerHandle};

/// How far the seek buttons (where an OS has them) jump.
const SEEK_STEP: f64 = 10.0;

thread_local! {
    static CONTROLS: RefCell<Option<MediaControls>> = const { RefCell::new(None) };
}

/// What the OS should show. `None` from `update` means nothing is playing.
#[derive(Debug, Clone, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    /// A file:// URL to the cover image, if the track has one.
    pub cover_url: Option<String>,
    pub playing: bool,
    pub position: f64,
}

/// Register with the OS. Call once from the main thread (Tauri's `setup`).
pub fn init(app: &AppHandle, player: PlayerHandle) {
    #[cfg(windows)]
    let hwnd = app
        .get_webview_window("main")
        .and_then(|w| w.hwnd().ok())
        .map(|h| h.0 as *mut std::ffi::c_void);
    #[cfg(not(windows))]
    let hwnd = {
        let _ = app;
        None
    };
    let config = PlatformConfig { display_name: "Honeycomb", dbus_name: "honeycomb", hwnd };
    let mut controls = match MediaControls::new(config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("media controls unavailable: {e:?}");
            return;
        }
    };
    let attached = controls.attach(move |event| {
        eprintln!("media key: {event:?}");
        let cmd = match event {
            MediaControlEvent::Play => Cmd::Play,
            MediaControlEvent::Pause | MediaControlEvent::Stop => Cmd::Pause,
            MediaControlEvent::Toggle => Cmd::Toggle,
            MediaControlEvent::Next => Cmd::Next,
            MediaControlEvent::Previous => Cmd::Prev,
            MediaControlEvent::SetPosition(MediaPosition(p)) => Cmd::Seek(p.as_secs_f64()),
            MediaControlEvent::Seek(dir) => Cmd::SeekBy(step(dir, SEEK_STEP)),
            MediaControlEvent::SeekBy(dir, d) => Cmd::SeekBy(step(dir, d.as_secs_f64())),
            _ => return,
        };
        player.send(cmd);
    });
    if let Err(e) = attached {
        eprintln!("media controls unavailable: {e:?}");
        return;
    }
    CONTROLS.with(|c| *c.borrow_mut() = Some(controls));
}

fn step(dir: SeekDirection, secs: f64) -> f64 {
    match dir {
        SeekDirection::Forward => secs,
        SeekDirection::Backward => -secs,
    }
}

/// Show `np` in the OS controls (or clear them). Safe to call from any thread.
pub fn update(app: &AppHandle, np: Option<NowPlaying>) {
    match &np {
        Some(n) => eprintln!("media state: {} \"{}\"", if n.playing { "playing" } else { "paused" }, n.title),
        None => eprintln!("media state: stopped"),
    }
    let _ = app.run_on_main_thread(move || {
        CONTROLS.with(|c| {
            let mut c = c.borrow_mut();
            let Some(controls) = c.as_mut() else { return };
            let result = match &np {
                Some(np) => controls
                    .set_metadata(MediaMetadata {
                        title: Some(&np.title),
                        artist: Some(&np.artist),
                        album: Some(&np.album),
                        cover_url: np.cover_url.as_deref(),
                        duration: Some(Duration::from_secs_f64(np.duration.max(0.0))),
                    })
                    .and_then(|_| {
                        let progress = Some(MediaPosition(Duration::from_secs_f64(np.position.max(0.0))));
                        controls.set_playback(if np.playing {
                            MediaPlayback::Playing { progress }
                        } else {
                            MediaPlayback::Paused { progress }
                        })
                    }),
                None => controls.set_playback(MediaPlayback::Stopped),
            };
            if let Err(e) = result {
                eprintln!("media controls update failed: {e:?}");
            }
        });
    });
}

/// Write a track's cover to the cache folder and return a file:// URL for it.
pub fn cover_url(app: &AppHandle, track_id: i64, audio_path: &str) -> Option<String> {
    let dir: PathBuf = app.path().app_cache_dir().ok()?.join("now-playing");
    std::fs::create_dir_all(&dir).ok()?;
    // The file's mtime is in the name, so editing a cover gets a fresh image.
    let mtime = crate::meta::file_mtime(std::path::Path::new(audio_path));
    let file = dir.join(format!("{track_id}-{mtime}.img"));
    if !file.is_file() {
        let (data, _mime) = crate::meta::artwork(std::path::Path::new(audio_path))?;
        std::fs::write(&file, data).ok()?;
    }
    Some(file_url(&file))
}

fn file_url(path: &std::path::Path) -> String {
    let p = path.to_string_lossy();
    let mut url = String::from("file://");
    if !p.starts_with('/') {
        url.push('/'); // Windows: file:///C:/...
    }
    for b in p.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => url.push(b as char),
            b'\\' => url.push('/'),
            _ => url.push_str(&format!("%{b:02X}")),
        }
    }
    url
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_urls_are_escaped() {
        assert_eq!(
            super::file_url(std::path::Path::new("/Users/a b/Caches/now-playing/7.img")),
            "file:///Users/a%20b/Caches/now-playing/7.img"
        );
    }
}
