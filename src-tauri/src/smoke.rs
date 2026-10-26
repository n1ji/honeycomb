//! Self-test for release builds: `HONEYCOMB_SMOKE=<folder of audio files>`.
//!
//! The app starts as usual (on a throwaway library unless HONEYCOMB_DATA_DIR
//! is set), then checks, in the build itself:
//! - the window's UI loaded and reported in,
//! - scanning the folder finds every audio file,
//! - every file decodes,
//! - every file plays through the real audio output (its position moves),
//! - a track played to its end is counted as played.
//! It prints a report (and writes it to HONEYCOMB_SMOKE_REPORT if set), then
//! exits with 0 when everything passed and 1 otherwise.
//!
//! HONEYCOMB_SMOKE_NO_AUDIO=1 skips the playback checks, for machines without
//! a sound device (CI runners).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Listener, Manager};

use crate::player::{Cmd, QItem};
use crate::{AppState, db, kaseki, scanner};

/// The UI calls `ui_ready` when it has rendered; this holds the track count it saw.
static UI_READY: Mutex<Option<usize>> = Mutex::new(None);

pub fn folder() -> Option<PathBuf> {
    std::env::var_os("HONEYCOMB_SMOKE").filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn ui_ready(tracks: usize) {
    *UI_READY.lock().unwrap_or_else(|p| p.into_inner()) = Some(tracks);
}

struct Report {
    lines: Vec<String>,
    failed: usize,
}

impl Report {
    fn check(&mut self, ok: bool, what: impl Into<String>) -> bool {
        let what = what.into();
        self.lines.push(format!("{} {what}", if ok { "PASS" } else { "FAIL" }));
        if !ok {
            self.failed += 1;
        }
        ok
    }

    fn note(&mut self, what: impl Into<String>) {
        self.lines.push(format!("     {}", what.into()));
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tick {
    track_id: Option<i64>,
    position: f64,
    playing: bool,
}

pub fn spawn(app: AppHandle, dir: PathBuf) {
    std::thread::spawn(move || {
        let mut r = Report { lines: Vec::new(), failed: 0 };
        r.lines.push(format!(
            "Honeycomb {} ({}) self-test on {} {}",
            env!("CARGO_PKG_VERSION"),
            &env!("HONEYCOMB_COMMIT")[..env!("HONEYCOMB_COMMIT").len().min(7)],
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&app, &dir, &mut r)));
        if run.is_err() {
            r.check(false, "the self-test itself crashed");
        }
        r.lines.push(if r.failed == 0 {
            "All checks passed.".into()
        } else {
            format!("{} check{} failed.", r.failed, if r.failed == 1 { "" } else { "s" })
        });
        let text = r.lines.join("\n") + "\n";
        println!("{text}");
        if let Some(path) = std::env::var_os("HONEYCOMB_SMOKE_REPORT") {
            let _ = std::fs::write(path, &text);
        }
        app.exit(if r.failed == 0 { 0 } else { 1 });
    });
}

fn run(app: &AppHandle, dir: &Path, r: &mut Report) {
    let state = app.state::<AppState>();

    // The window's script ran and showed the (empty) library.
    let start = Instant::now();
    let ui = loop {
        if let Some(n) = *UI_READY.lock().unwrap_or_else(|p| p.into_inner()) {
            break Some(n);
        }
        if start.elapsed() > Duration::from_secs(30) {
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    r.check(ui.is_some(), "UI loaded");

    // Scan.
    let files: Vec<PathBuf> = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && crate::meta::is_audio(e.path()))
        .map(|e| e.into_path())
        .collect();
    if !r.check(!files.is_empty(), format!("found audio files in {}", dir.display())) {
        return;
    }
    state
        .conn()
        .execute("INSERT OR IGNORE INTO folders (path) VALUES (?1)", [dir.to_string_lossy()])
        .ok();
    let scanned = scanner::scan(&state.db_path, &|_| {});
    let tracks = db::all_tracks(&state.conn()).unwrap_or_default();
    match scanned {
        Ok(f) => {
            r.check(
                tracks.len() == files.len() && f.errors == 0,
                format!("scan: {} of {} files in the library, {} unreadable", tracks.len(), files.len(), f.errors),
            );
        }
        Err(e) => {
            r.check(false, format!("scan failed: {e:#}"));
            return;
        }
    }
    let untitled: Vec<&str> = tracks.iter().filter(|t| t.title.is_empty()).map(|t| t.path.as_str()).collect();
    r.check(untitled.is_empty(), format!("tags read ({} without a title)", untitled.len()));

    // Decode.
    for t in &tracks {
        let name = file_name(&t.path);
        match kaseki::Source::open(&t.path) {
            Ok(s) => r.check(true, format!("decode {name} ({} Hz)", s.rate)),
            Err(e) => r.check(false, format!("decode {name}: {e}")),
        };
    }

    if std::env::var_os("HONEYCOMB_SMOKE_NO_AUDIO").is_some() {
        r.note("playback checks skipped (HONEYCOMB_SMOKE_NO_AUDIO)");
        return;
    }
    if !r.check(kaseki::default_device_name().is_some(), "an audio output device is available") {
        return;
    }

    // Play each file for a moment.
    let (tx, rx) = channel::<Tick>();
    let tx = Mutex::new(tx);
    let id = app.listen_any("player-tick", move |e| {
        if let Ok(t) = serde_json::from_str::<Tick>(e.payload()) {
            let _ = tx.lock().unwrap().send(t);
        }
    });
    let item = |t: &crate::models::Track| QItem { uid: 0, track_id: t.id, path: t.path.clone(), duration: t.duration };
    for t in &tracks {
        while rx.try_recv().is_ok() {}
        state.player.send(Cmd::Load(vec![item(t)], 0));
        let reached = wait_for(&rx, Duration::from_secs(10), |k| k.playing && k.track_id == Some(t.id) && k.position >= 1.0);
        r.check(reached.is_some(), format!("play {}", file_name(&t.path)));
    }

    // The shortest track, played to its end, counts as a play.
    if let Some(t) = tracks.iter().filter(|t| t.duration > 0.0).min_by(|a, b| a.duration.total_cmp(&b.duration)) {
        let before = t.play_count;
        while rx.try_recv().is_ok() {}
        state.player.send(Cmd::Load(vec![item(t)], 0));
        let limit = Duration::from_secs_f64(t.duration + 10.0);
        wait_for(&rx, limit, |k| k.track_id.is_none() && !k.playing);
        let after = db::track_by_id(&state.conn(), t.id).ok().flatten().map_or(0, |t| t.play_count);
        r.check(after == before + 1, format!("play count after playing {} to the end: {before} → {after}", file_name(&t.path)));
    }
    state.player.send(Cmd::Clear);
    app.unlisten(id);
}

fn wait_for(rx: &Receiver<Tick>, limit: Duration, ok: impl Fn(&Tick) -> bool) -> Option<Tick> {
    let start = Instant::now();
    while let Some(left) = limit.checked_sub(start.elapsed()) {
        match rx.recv_timeout(left) {
            Ok(t) if ok(&t) => return Some(t),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}
