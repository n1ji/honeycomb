//! Audio playback on a dedicated thread. The thread owns the output stream
//! (which is not `Send`) and the play queue; the UI talks to it through
//! `PlayerHandle` and hears back through `player-tick` / `player-queue` events.
//!
//! Gapless playback: shortly before the current track ends, the next track is
//! appended to the same rodio `Sink`, so there is no gap between them.

use std::collections::VecDeque;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use rand::seq::SliceRandom;
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::db;
use crate::models::now_secs;

/// How long before the end of a track the next one is queued in the sink.
const PRELOAD_SECS: f64 = 2.0;
const TICK: Duration = Duration::from_millis(200);
/// How often to check whether the system's default output device changed.
const DEVICE_CHECK: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct QItem {
    pub uid: u64,
    pub track_id: i64,
    pub path: String,
    pub duration: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

pub enum Cmd {
    /// Replace the queue and start at `index`.
    Load(Vec<QItem>, usize),
    Toggle,
    Play,
    Pause,
    Next,
    Prev,
    Seek(f64),
    /// Seek relative to the current position, in seconds.
    SeekBy(f64),
    Volume(f32),
    Shuffle(bool),
    Repeat(Repeat),
    /// Add items at the end, or right after the current track when `next` is true.
    Enqueue(Vec<QItem>, bool),
    Jump(usize),
    Remove(Vec<usize>),
    Clear,
    Emit,
}

#[derive(Clone)]
pub struct PlayerHandle {
    tx: Sender<Cmd>,
}

impl PlayerHandle {
    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Tick {
    track_id: Option<i64>,
    position: f64,
    duration: f64,
    playing: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct QueueEvent {
    track_ids: Vec<i64>,
    current: Option<usize>,
    shuffle: bool,
    repeat: Repeat,
    volume: f32,
    error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Played {
    id: i64,
    play_count_delta: u32,
    last_played: i64,
}

pub fn spawn(app: AppHandle, db_path: PathBuf, volume: f32) -> PlayerHandle {
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || Engine::new(app, db_path, volume).run(rx))
        .expect("spawn audio thread");
    PlayerHandle { tx }
}

struct Engine {
    app: AppHandle,
    db_path: PathBuf,
    /// Kept alive for as long as the engine runs; dropping it stops audio.
    stream: Option<OutputStream>,
    sink: Option<Sink>,
    /// uids of sources currently appended to the sink; front is playing.
    in_sink: VecDeque<u64>,
    queue: Vec<QItem>,
    /// Queue order before shuffling, restored when shuffle is turned off.
    unshuffled: Option<Vec<QItem>>,
    current: Option<usize>,
    repeat: Repeat,
    volume: f32,
    next_uid: u64,
    error: Option<String>,
    /// Name of the device the stream plays on.
    device: Option<String>,
    /// Set from the audio callback thread when the stream fails (device gone).
    stream_failed: Arc<AtomicBool>,
    last_device_check: Instant,
    /// Where to pick up again once an output device is available.
    resume: Option<(usize, f64)>,
    /// (playing item uid, playing) last shown in the OS media controls.
    media_shown: Option<(u64, bool)>,
    /// Position jumped; the OS controls need the new position.
    media_dirty: bool,
    /// Queue index last shown, kept after playback stops so the OS still sees a paused track.
    media_last_index: Option<usize>,
}

fn default_device_name() -> Option<String> {
    rodio::cpal::default_host().default_output_device().and_then(|d| d.name().ok())
}

fn device_exists(name: &str) -> bool {
    rodio::cpal::default_host()
        .output_devices()
        .map(|mut ds| ds.any(|d| d.name().ok().as_deref() == Some(name)))
        .unwrap_or(false)
}

/// Open the system's default output. Stream errors only raise `failed`;
/// the audio thread notices on its next tick and reopens.
fn open_output(failed: &Arc<AtomicBool>) -> Result<(OutputStream, Option<String>), String> {
    failed.store(false, Ordering::SeqCst);
    let flag = failed.clone();
    let builder = OutputStreamBuilder::from_default_device()
        .map_err(|e| format!("No audio output device: {e}"))?
        .with_error_callback(move |e| {
            eprintln!("audio stream error: {e}");
            flag.store(true, Ordering::SeqCst);
        });
    let mut stream = builder.open_stream_or_fallback().map_err(|e| format!("No audio output device: {e}"))?;
    stream.log_on_drop(false);
    Ok((stream, default_device_name()))
}

impl Engine {
    fn new(app: AppHandle, db_path: PathBuf, volume: f32) -> Self {
        let stream_failed = Arc::new(AtomicBool::new(false));
        let (stream, device, error) = match open_output(&stream_failed) {
            Ok((s, d)) => (Some(s), d, None),
            Err(e) => (None, None, Some(e)),
        };
        Engine {
            app,
            db_path,
            stream,
            sink: None,
            in_sink: VecDeque::new(),
            queue: Vec::new(),
            unshuffled: None,
            current: None,
            repeat: Repeat::Off,
            volume,
            next_uid: 1,
            error,
            device,
            stream_failed,
            last_device_check: Instant::now(),
            resume: None,
            media_shown: None,
            media_dirty: false,
            media_last_index: None,
        }
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        loop {
            match rx.recv_timeout(TICK) {
                Ok(cmd) => {
                    self.guarded(|e| e.handle_cmd(cmd));
                    // Drain any burst of commands before ticking.
                    while let Ok(cmd) = rx.try_recv() {
                        self.guarded(|e| e.handle_cmd(cmd));
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.guarded(Engine::tick);
        }
    }

    /// Run `f`, and if it panics, stop playback instead of killing the thread.
    fn guarded(&mut self, f: impl FnOnce(&mut Self)) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
        if result.is_err() {
            self.stop();
            self.error = Some("Playback stopped after an internal error".into());
            self.emit_queue();
        }
    }

    fn handle_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Load(items, index) => {
                self.unshuffled = None;
                self.queue = self.stamp(items);
                if index < self.queue.len() {
                    self.start(index);
                } else {
                    self.stop();
                }
            }
            Cmd::Toggle => match &self.sink {
                Some(s) if s.is_paused() => s.play(),
                Some(s) => s.pause(),
                None => self.start(self.current.unwrap_or(0)),
            },
            Cmd::Play => match &self.sink {
                Some(s) => s.play(),
                None => self.start(self.current.unwrap_or(0)),
            },
            Cmd::Pause => {
                if let Some(s) = &self.sink {
                    s.pause();
                }
            }
            Cmd::Next => {
                if let Some(cur) = self.current {
                    if self.position() > self.queue[cur].duration * 0.5 {
                        self.record_play(self.queue[cur].track_id);
                    }
                    let next = if cur + 1 < self.queue.len() {
                        Some(cur + 1)
                    } else if self.repeat != Repeat::Off && !self.queue.is_empty() {
                        Some(0)
                    } else {
                        None
                    };
                    match next {
                        Some(n) => self.start(n),
                        None => self.stop(),
                    }
                }
            }
            Cmd::Prev => {
                if let Some(cur) = self.current {
                    if self.position() > 3.0 || cur == 0 {
                        self.seek(0.0);
                    } else {
                        self.start(cur - 1);
                    }
                }
            }
            Cmd::Seek(secs) => {
                self.seek(secs);
                self.media_dirty = true;
            }
            Cmd::SeekBy(delta) => {
                let pos = self.position();
                self.seek(pos + delta);
                self.media_dirty = true;
            }
            Cmd::Volume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                if let Some(s) = &self.sink {
                    s.set_volume(self.volume);
                }
            }
            Cmd::Shuffle(on) => self.set_shuffle(on),
            Cmd::Repeat(r) => self.repeat = r,
            Cmd::Enqueue(items, next) => {
                let items = self.stamp(items);
                if let Some(orig) = &mut self.unshuffled {
                    orig.extend(items.iter().cloned());
                }
                match (next, self.current) {
                    (true, Some(cur)) => {
                        let at = cur + 1;
                        self.queue.splice(at..at, items);
                    }
                    _ => self.queue.extend(items),
                }
            }
            Cmd::Jump(i) => {
                if i < self.queue.len() {
                    self.start(i);
                }
            }
            Cmd::Remove(mut idxs) => {
                idxs.sort_unstable();
                idxs.dedup();
                let current_uid = self.current.map(|c| self.queue[c].uid);
                let removed_before = self
                    .current
                    .map(|c| idxs.iter().filter(|&&i| i < c && i < self.queue.len()).count())
                    .unwrap_or(0);
                let mut removed_uids = Vec::new();
                for i in idxs.into_iter().rev() {
                    if i < self.queue.len() {
                        removed_uids.push(self.queue.remove(i).uid);
                    }
                }
                if let Some(orig) = &mut self.unshuffled {
                    orig.retain(|q| !removed_uids.contains(&q.uid));
                }
                match current_uid {
                    Some(uid) if removed_uids.contains(&uid) => {
                        // Carry on with the first track after it that's still queued.
                        let old = self.current.unwrap_or(0);
                        let next = old - removed_before;
                        let paused = self.is_paused();
                        if next < self.queue.len() {
                            self.restart_at(next, 0.0, paused);
                        } else if self.repeat != Repeat::Off && !self.queue.is_empty() {
                            self.restart_at(0, 0.0, paused);
                        } else {
                            self.stop();
                        }
                    }
                    Some(uid) => self.current = self.queue.iter().position(|q| q.uid == uid),
                    None => {}
                }
            }
            Cmd::Clear => {
                self.queue.clear();
                self.unshuffled = None;
                self.stop();
            }
            Cmd::Emit => {}
        }
        self.fix_preload();
        self.emit_queue();
    }

    fn is_paused(&self) -> bool {
        self.sink.as_ref().is_some_and(|s| s.is_paused())
    }

    /// Start queue[index] at `pos` seconds, optionally paused.
    fn restart_at(&mut self, index: usize, pos: f64, paused: bool) {
        self.start(index);
        if pos > 0.0 {
            self.seek(pos);
        }
        if paused {
            if let Some(s) = &self.sink {
                s.pause();
            }
        }
    }

    /// The next track is appended to the sink a moment early for gapless
    /// playback. If the queue, shuffle or repeat changed since, that track may
    /// no longer be next; rebuild the sink at the current position so it isn't played.
    fn fix_preload(&mut self) {
        let Some(&preloaded) = self.in_sink.get(1) else { return };
        let want = self.next_index().map(|n| self.queue[n].uid);
        if want != Some(preloaded) {
            if let Some(cur) = self.current {
                let (pos, paused) = (self.position(), self.is_paused());
                self.restart_at(cur, pos, paused);
            }
        }
    }

    /// Reopen the output when the stream failed or the system's default device
    /// changed (headphones plugged in or out). If the old device disappeared,
    /// playback pauses rather than jumping to the speakers.
    fn check_device(&mut self) {
        let failed = self.stream_failed.load(Ordering::SeqCst);
        if !failed && self.last_device_check.elapsed() < DEVICE_CHECK {
            return;
        }
        self.last_device_check = Instant::now();
        let now = default_device_name();
        let changed = now.is_some() && now != self.device;
        if !failed && !changed && self.stream.is_some() {
            return;
        }
        let gone = failed || self.device.as_deref().is_some_and(|d| !device_exists(d));
        let state = match (self.current, self.resume) {
            (Some(cur), _) if self.sink.is_some() => Some((cur, self.position(), self.is_paused() || gone)),
            (_, Some((cur, pos))) => Some((cur, pos, true)),
            _ => None,
        };
        self.sink = None;
        self.in_sink.clear();
        self.stream = None;
        match open_output(&self.stream_failed) {
            Ok((stream, device)) => {
                eprintln!("audio output: {}", device.as_deref().unwrap_or("default device"));
                self.stream = Some(stream);
                self.device = device;
                self.error = None;
                self.resume = None;
                if let Some((cur, pos, paused)) = state {
                    if cur < self.queue.len() {
                        self.restart_at(cur, pos, paused);
                    }
                }
            }
            Err(e) => {
                self.error = Some(e);
                self.device = None;
                self.resume = state.map(|(cur, pos, _)| (cur, pos));
            }
        }
        self.emit_queue();
    }

    fn stamp(&mut self, items: Vec<QItem>) -> Vec<QItem> {
        items
            .into_iter()
            .map(|mut q| {
                q.uid = self.next_uid;
                self.next_uid += 1;
                q
            })
            .collect()
    }

    fn open(path: &str) -> Result<Decoder<BufReader<File>>, String> {
        let file = File::open(path).map_err(|e| format!("Can't open {path}: {e}"))?;
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        // Decoders for unusual files have been known to panic; never let that
        // take the audio thread down with it.
        std::panic::catch_unwind(move || {
            Decoder::builder()
                .with_data(BufReader::new(file))
                .with_byte_len(len)
                .with_seekable(true)
                .with_gapless(true)
                .with_hint(&ext)
                .build()
        })
        .map_err(|_| format!("Can't decode {path}: decoder crashed"))?
        .map_err(|e| format!("Can't decode {path}: {e}"))
    }

    /// Hard-switch to queue[index] with a fresh sink.
    fn start(&mut self, index: usize) {
        let Some(stream) = &self.stream else {
            return;
        };
        if index >= self.queue.len() {
            return;
        }
        // Dropping the old sink stops it immediately.
        self.sink = None;
        self.in_sink.clear();

        // Skip unplayable files, giving up after one pass over the queue.
        let mut i = index;
        for _ in 0..self.queue.len() {
            let item = &self.queue[i];
            match Self::open(&item.path) {
                Ok(src) => {
                    let sink = Sink::connect_new(stream.mixer());
                    sink.set_volume(self.volume);
                    sink.append(src);
                    self.sink = Some(sink);
                    self.in_sink.push_back(item.uid);
                    self.current = Some(i);
                    self.error = None;
                    return;
                }
                Err(e) => {
                    let _ = self.app.emit("player-error", &e);
                    self.error = Some(e);
                }
            }
            i += 1;
            if i >= self.queue.len() {
                if self.repeat == Repeat::Off {
                    break;
                }
                i = 0;
            }
        }
        self.current = None;
    }

    fn stop(&mut self) {
        self.sink = None;
        self.in_sink.clear();
        self.current = None;
    }

    fn position(&self) -> f64 {
        self.sink.as_ref().map(|s| s.get_pos().as_secs_f64()).unwrap_or(0.0)
    }

    fn seek(&mut self, secs: f64) {
        if let Some(s) = &self.sink {
            let _ = s.try_seek(Duration::from_secs_f64(secs.max(0.0)));
        }
    }

    fn set_shuffle(&mut self, on: bool) {
        let mut rng = rand::thread_rng();
        let cur_uid = self.current.map(|c| self.queue[c].uid);
        if on {
            if self.unshuffled.is_none() {
                self.unshuffled = Some(self.queue.clone());
            }
            // Keep the playing track first, shuffle the rest after it.
            if let Some(c) = self.current {
                let playing = self.queue.remove(c);
                self.queue.shuffle(&mut rng);
                self.queue.insert(0, playing);
                self.current = Some(0);
            } else {
                self.queue.shuffle(&mut rng);
            }
        } else if let Some(orig) = self.unshuffled.take() {
            self.queue = orig;
            self.current = cur_uid.and_then(|u| self.queue.iter().position(|q| q.uid == u));
        }
    }

    fn next_index(&self) -> Option<usize> {
        let cur = self.current?;
        match self.repeat {
            Repeat::One => Some(cur),
            Repeat::All if !self.queue.is_empty() => Some((cur + 1) % self.queue.len()),
            _ if cur + 1 < self.queue.len() => Some(cur + 1),
            _ => None,
        }
    }

    fn tick(&mut self) {
        self.check_device();
        let Some(sink) = &self.sink else {
            self.sync_media();
            self.emit_tick();
            return;
        };

        // Sources that finished since the last tick.
        let mut finished = Vec::new();
        while self.in_sink.len() > sink.len() {
            if let Some(uid) = self.in_sink.pop_front() {
                finished.push(uid);
            }
        }
        if !finished.is_empty() {
            for uid in &finished {
                if let Some(q) = self.queue.iter().find(|q| q.uid == *uid) {
                    self.record_play(q.track_id);
                }
            }
            match self.in_sink.front() {
                Some(uid) => self.current = self.queue.iter().position(|q| q.uid == *uid),
                None => {
                    // Reached the end of the queue.
                    self.sink = None;
                    self.current = None;
                }
            }
            self.emit_queue();
        }
        self.sync_media();

        // Preload the next track for gapless playback.
        if let (Some(sink), Some(cur)) = (&self.sink, self.current) {
            if self.in_sink.len() == 1 && !sink.is_paused() {
                let remaining = self.queue[cur].duration - sink.get_pos().as_secs_f64();
                if remaining < PRELOAD_SECS {
                    if let Some(next) = self.next_index() {
                        let item = &self.queue[next];
                        if let Ok(src) = Self::open(&item.path) {
                            sink.append(src);
                            self.in_sink.push_back(item.uid);
                        }
                    }
                }
            }
        }
        self.emit_tick();
    }

    /// Tell the OS media controls about a new track, play/pause, or a seek.
    /// While anything is queued, an idle player reports "paused", never
    /// "stopped": macOS treats a stopped app as done and hands the media keys
    /// to another app (e.g. a browser tab).
    fn sync_media(&mut self) {
        let active = self.sink.is_some();
        let item = self
            .current
            .or(self.media_last_index)
            .and_then(|c| self.queue.get(c))
            .or_else(|| self.queue.first());
        if active {
            self.media_last_index = self.current;
        }
        let playing = active && !self.is_paused();
        let key = item.map(|q| (q.uid, playing));
        if key == self.media_shown && !self.media_dirty {
            return;
        }
        self.media_shown = key;
        self.media_dirty = false;
        let np = item.map(|q| {
            let track = db::open(&self.db_path).ok().and_then(|c| db::track_by_id(&c, q.track_id).ok().flatten());
            let (title, artist, album) = match &track {
                Some(t) => (
                    if t.title.is_empty() { file_stem(&q.path) } else { t.title.clone() },
                    if t.artist.is_empty() { t.album_artist.clone() } else { t.artist.clone() },
                    t.album.clone(),
                ),
                None => (file_stem(&q.path), String::new(), String::new()),
            };
            crate::media::NowPlaying {
                title,
                artist,
                album,
                duration: q.duration,
                cover_url: crate::media::cover_url(&self.app, q.track_id, &q.path),
                playing,
                position: if active { self.position() } else { 0.0 },
            }
        });
        crate::media::update(&self.app, np);
    }

    fn record_play(&self, track_id: i64) {
        let when = now_secs();
        if let Ok(conn) = db::open(&self.db_path) {
            let _ = db::record_play(&conn, track_id, when);
        }
        let _ = self.app.emit(
            "track-played",
            Played { id: track_id, play_count_delta: 1, last_played: when },
        );
    }

    fn emit_tick(&self) {
        let (track_id, duration) = match self.current.and_then(|c| self.queue.get(c)) {
            Some(q) => (Some(q.track_id), q.duration),
            None => (None, 0.0),
        };
        let _ = self.app.emit(
            "player-tick",
            Tick {
                track_id,
                position: self.position(),
                duration,
                playing: self.sink.as_ref().map(|s| !s.is_paused()).unwrap_or(false),
            },
        );
    }

    fn emit_queue(&self) {
        let _ = self.app.emit(
            "player-queue",
            QueueEvent {
                track_ids: self.queue.iter().map(|q| q.track_id).collect(),
                current: self.current,
                shuffle: self.unshuffled.is_some(),
                repeat: self.repeat,
                volume: self.volume,
                error: self.error.clone(),
            },
        );
    }
}

fn file_stem(path: &str) -> String {
    std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}
