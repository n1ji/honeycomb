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
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

use rand::seq::SliceRandom;
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::db;
use crate::models::now_secs;

/// How long before the end of a track the next one is queued in the sink.
const PRELOAD_SECS: f64 = 2.0;
const TICK: Duration = Duration::from_millis(200);

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
}

impl Engine {
    fn new(app: AppHandle, db_path: PathBuf, volume: f32) -> Self {
        let (stream, error) = match OutputStreamBuilder::open_default_stream() {
            Ok(mut s) => {
                s.log_on_drop(false);
                (Some(s), None)
            }
            Err(e) => (None, Some(format!("No audio output device: {e}"))),
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
            Cmd::Seek(secs) => self.seek(secs),
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
                    Some(uid) if removed_uids.contains(&uid) => self.stop(),
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
        let Some(sink) = &self.sink else {
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
