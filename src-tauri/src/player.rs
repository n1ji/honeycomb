//! The player: owns the play queue and the output stream (which is not
//! `Send`) on a dedicated thread. The UI talks to it through `PlayerHandle`
//! and hears back through `player-tick` / `player-queue` events.
//!
//! Playback itself is the Kaseki engine (`kaseki`): decoding happens on a
//! separate thread (`kaseki::Dec`), which fills a ring buffer the output
//! callback copies from. Gapless playback: the player sends
//! the decoder a plan of the tracks after the current one, and the decoder
//! writes them into the same buffer straight after it, leaving markers where
//! each one starts.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};

use crate::kaseki::{self, DecMsg, Marker, Markers, Output, Source};
use crate::db;
use crate::models::now_secs;

const TICK: Duration = Duration::from_millis(200);
/// How often to check whether the system's default output device changed.
const DEVICE_CHECK: Duration = Duration::from_secs(2);
/// Close the output after this long paused or stopped, so the device can sleep.
const IDLE_CLOSE: Duration = Duration::from_secs(30);
/// How many tracks ahead the decoder is told about.
const PLAN_LEN: usize = 4;

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

#[derive(Clone, Serialize, PartialEq)]
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

/// How long a song of `duration` seconds has to be heard before it counts as
/// listened to (Last.fm's rule: half of it or 4 minutes), or None for songs of
/// 30 seconds or less.
pub fn listen_threshold(duration: f64) -> Option<f64> {
    (duration > 30.0).then(|| (duration / 2.0).min(240.0))
}

/// The song being listened to, followed for the `player-now-playing` and
/// `player-listened` events (plugins scrobble from them).
struct Listen {
    uid: u64,
    track_id: i64,
    started_at: i64,
    /// Seconds actually heard; seeking ahead doesn't count.
    heard: f64,
    last_pos: f64,
    announced: bool,
    scrobbled: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Played {
    id: i64,
    play_count_delta: u32,
    last_played: i64,
}

pub fn spawn<R: Runtime>(app: AppHandle<R>, db_path: PathBuf, volume: f32) -> PlayerHandle {
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || Engine::new(app, db_path, volume).run(rx))
        .expect("spawn audio thread");
    PlayerHandle { tx }
}

struct Engine<R: Runtime> {
    app: AppHandle<R>,
    db_path: PathBuf,
    /// The open output, if any. It's opened on play and closed after a while idle.
    out: Option<Output>,
    dec: Sender<DecMsg>,
    markers: Markers,
    /// Bumped on every start, seek and stop; the decoder's markers carry it.
    epoch: u64,
    /// Markers of this epoch that playback hasn't passed yet; front is playing.
    segs: VecDeque<Marker>,
    /// Position to report until the decoder confirms a start.
    pending_pos: f64,
    /// A track is loaded, playing or paused.
    active: bool,
    paused: bool,
    /// The output was closed while paused; playing resumes from here.
    suspended: Option<f64>,
    idle_since: Option<Instant>,
    /// Last plan sent to the decoder: (epoch, base segment, uids).
    sent_plan: Option<(u64, u32, Vec<u64>)>,
    queue: Vec<QItem>,
    /// Queue order before shuffling, restored when shuffle is turned off.
    unshuffled: Option<Vec<QItem>>,
    current: Option<usize>,
    repeat: Repeat,
    volume: f32,
    next_uid: u64,
    error: Option<String>,
    /// Name of the device the output plays on.
    device: Option<String>,
    last_device_check: Instant,
    /// Where to pick up again once an output device is available.
    resume: Option<(usize, f64)>,
    /// (playing item uid, playing) last shown in the OS media controls.
    media_shown: Option<(u64, bool)>,
    /// Position jumped; the OS controls need the new position.
    media_dirty: bool,
    /// Queue index last shown, kept after playback stops so the OS still sees a paused track.
    media_last_index: Option<usize>,
    last_tick: Option<Tick>,
    listen: Option<Listen>,
}

impl<R: Runtime> Engine<R> {
    fn new(app: AppHandle<R>, db_path: PathBuf, volume: f32) -> Self {
        let markers = Markers::default();
        Engine {
            app,
            db_path,
            out: None,
            dec: kaseki::spawn_decoder(markers.clone()),
            markers,
            epoch: 0,
            segs: VecDeque::new(),
            pending_pos: 0.0,
            active: false,
            paused: false,
            suspended: None,
            idle_since: None,
            sent_plan: None,
            queue: Vec::new(),
            unshuffled: None,
            current: None,
            repeat: Repeat::Off,
            volume,
            next_uid: 1,
            error: None,
            device: None,
            last_device_check: Instant::now(),
            resume: None,
            media_shown: None,
            media_dirty: false,
            media_last_index: None,
            last_tick: None,
            listen: None,
        }
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        loop {
            // Wake often only while playing; idle, wait for a command.
            let timeout = if self.out.is_some() && self.active && !self.paused {
                Some(TICK)
            } else if self.out.is_some() || self.resume.is_some() {
                Some(DEVICE_CHECK)
            } else {
                None
            };
            let got = match timeout {
                Some(t) => rx.recv_timeout(t),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            let mut force = false;
            match got {
                Ok(cmd) => {
                    self.guarded(|e| e.handle_cmd(cmd));
                    // Drain any burst of commands before ticking.
                    while let Ok(cmd) = rx.try_recv() {
                        self.guarded(|e| e.handle_cmd(cmd));
                    }
                    force = true;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.guarded(|e| e.tick(force));
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
        self.poll();
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
            Cmd::Toggle => {
                if !self.active {
                    self.start(self.current.unwrap_or(0));
                } else if self.paused {
                    self.resume_play();
                } else {
                    self.set_paused(true);
                }
            }
            Cmd::Play => {
                if self.active {
                    self.resume_play();
                } else {
                    self.start(self.current.unwrap_or(0));
                }
            }
            Cmd::Pause => {
                if self.active {
                    self.set_paused(true);
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
                if let Some(o) = &self.out {
                    o.shared.set_volume(self.volume);
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
        self.active && self.paused
    }

    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if let Some(o) = &self.out {
            o.shared.set_paused(paused);
        }
    }

    /// Unpause, reopening the output if it was closed while idle.
    fn resume_play(&mut self) {
        match (self.suspended, self.current) {
            (Some(pos), Some(cur)) => self.restart_at(cur, pos, false),
            _ => self.set_paused(false),
        }
    }

    /// Start queue[index], skipping unplayable files.
    fn start(&mut self, index: usize) {
        self.restart_at(index, 0.0, false);
    }

    /// Start queue[index] at `pos` seconds, optionally paused. Unplayable
    /// files are skipped, giving up after one pass over the queue.
    fn restart_at(&mut self, index: usize, pos: f64, paused: bool) {
        if index >= self.queue.len() {
            return;
        }
        let mut i = index;
        for _ in 0..self.queue.len() {
            let pos = if i == index { pos.max(0.0) } else { 0.0 };
            match Source::open(&self.queue[i].path) {
                Ok(source) => {
                    if let Err(e) = self.ensure_output(source.rate) {
                        // No device: remember where we were and retry later.
                        self.halt();
                        self.error = Some(e);
                        self.current = Some(i);
                        self.resume = Some((i, pos));
                        return;
                    }
                    self.epoch += 1;
                    self.segs.clear();
                    self.sent_plan = None;
                    self.pending_pos = pos;
                    self.active = true;
                    self.suspended = None;
                    self.resume = None;
                    self.set_paused(paused);
                    let uid = self.queue[i].uid;
                    let _ = self.dec.send(DecMsg::Start { epoch: self.epoch, uid, source: Box::new(source), pos });
                    self.current = Some(i);
                    self.error = None;
                    self.send_plan();
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
        self.halt();
        self.current = None;
    }

    /// Make sure the output is open and suits a file at `rate`, reopening it
    /// at that rate if the device supports it.
    fn ensure_output(&mut self, rate: u32) -> Result<(), String> {
        if self.out.as_ref().is_some_and(|o| o.suits(rate)) {
            return Ok(());
        }
        self.close_output();
        let (out, prod) = Output::open(rate, self.volume, self.paused)?;
        eprintln!(
            "kaseki: audio output: {} at {} Hz{}",
            out.device.as_deref().unwrap_or("default device"),
            out.rate,
            if out.rate == rate { String::new() } else { format!(" (resampling from {rate} Hz)") }
        );
        let _ = self.dec.send(DecMsg::Output {
            prod,
            shared: out.shared.clone(),
            rate: out.rate,
            channels: out.channels,
        });
        self.device = out.device.clone();
        self.out = Some(out);
        self.last_device_check = Instant::now();
        self.idle_since = None;
        Ok(())
    }

    fn close_output(&mut self) {
        if self.out.take().is_some() {
            let _ = self.dec.send(DecMsg::Close);
        }
        self.epoch += 1;
        self.segs.clear();
        self.sent_plan = None;
    }

    /// Silence the decoder without touching the queue position.
    fn halt(&mut self) {
        self.epoch += 1;
        self.segs.clear();
        self.sent_plan = None;
        self.active = false;
        self.paused = false;
        self.suspended = None;
        if self.out.is_some() {
            let _ = self.dec.send(DecMsg::Stop);
        }
    }

    fn stop(&mut self) {
        self.halt();
        self.resume = None;
        self.current = None;
    }

    fn position(&self) -> f64 {
        if !self.active {
            return 0.0;
        }
        if let Some(pos) = self.suspended {
            return pos;
        }
        match (&self.out, self.segs.front()) {
            (Some(o), Some(m)) => {
                let played = o.shared.consumed().saturating_sub(m.at);
                m.start + played as f64 / (o.rate as f64 * o.channels as f64)
            }
            _ => self.pending_pos,
        }
    }

    fn seek(&mut self, secs: f64) {
        let (true, Some(cur)) = (self.active, self.current) else { return };
        let secs = secs.max(0.0);
        if self.suspended.is_some() {
            self.suspended = Some(secs);
        } else {
            self.restart_at(cur, secs, self.paused);
        }
    }

    /// Take the decoder's new markers and follow playback across them:
    /// record finished tracks, move `current` along, and handle the end.
    fn poll(&mut self) {
        let Some(out) = &self.out else { return };
        if !self.active || self.suspended.is_some() {
            return;
        }
        kaseki::take_markers(&self.markers, self.epoch, &mut self.segs);
        let consumed = out.shared.consumed();
        let mut finished = Vec::new();
        while self.segs.len() >= 2 && consumed >= self.segs[1].at {
            if let Some(uid) = self.segs.pop_front().and_then(|m| m.uid) {
                finished.push(uid);
            }
        }
        if finished.is_empty() {
            return;
        }
        for uid in &finished {
            if let Some(q) = self.queue.iter().find(|q| q.uid == *uid) {
                self.record_play(q.track_id);
            }
        }
        match self.segs.front().map(|m| m.uid) {
            Some(Some(uid)) => self.current = self.queue.iter().position(|q| q.uid == uid),
            _ => {
                // Played everything that was decoded. Usually that's the end of
                // the queue; otherwise the next track needs a fresh start (it
                // has another sample rate, or was queued very late).
                let next = match self.current {
                    Some(c) if self.queue.get(c).is_some_and(|q| Some(&q.uid) == finished.last()) => {
                        self.next_index()
                    }
                    _ => None,
                };
                match next {
                    Some(n) => self.start(n),
                    None => {
                        self.halt();
                        self.current = None;
                    }
                }
            }
        }
        self.emit_queue();
    }

    /// Successors of the current track, as many as the decoder gets told about.
    fn plan(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut at = self.current;
        while out.len() < PLAN_LEN {
            match at.and_then(|c| self.next_after(c)) {
                Some(n) => {
                    out.push(n);
                    at = Some(n);
                }
                None => break,
            }
        }
        out
    }

    /// Tell the decoder which tracks follow the playing one, if that changed.
    fn send_plan(&mut self) {
        if !self.active || self.suspended.is_some() || self.out.is_none() {
            return;
        }
        let base = self.segs.front().map_or(0, |m| m.seq);
        let idxs = self.plan();
        let uids: Vec<u64> = idxs.iter().map(|&i| self.queue[i].uid).collect();
        let key = (self.epoch, base, uids);
        if self.sent_plan.as_ref() == Some(&key) {
            return;
        }
        let items = idxs.iter().map(|&i| (self.queue[i].uid, self.queue[i].path.clone())).collect();
        let _ = self.dec.send(DecMsg::Plan { epoch: self.epoch, base, items });
        self.sent_plan = Some(key);
    }

    /// The decoder runs a few seconds ahead, so the next tracks may already be
    /// in the buffer. If the queue, shuffle or repeat changed since, they may
    /// no longer be next; restart at the current position so they aren't played.
    fn fix_preload(&mut self) {
        if !self.active || self.suspended.is_some() {
            return;
        }
        let Some(cur) = self.current else { return };
        kaseki::take_markers(&self.markers, self.epoch, &mut self.segs);
        let want: Vec<u64> = self.plan().iter().map(|&i| self.queue[i].uid).collect();
        let stale = self
            .segs
            .iter()
            .skip(1)
            .enumerate()
            .any(|(k, m)| m.uid.is_some() && m.uid != want.get(k).copied());
        if stale {
            let (pos, paused) = (self.position(), self.is_paused());
            self.restart_at(cur, pos, paused);
        } else {
            self.send_plan();
        }
    }

    /// Reopen the output when the stream failed or the system's default device
    /// changed (headphones plugged in or out). If the old device disappeared,
    /// playback pauses rather than jumping to the speakers. A closed output
    /// (idle) has nothing to follow; it opens on the default device on play.
    fn check_device(&mut self) {
        let failed = self.out.as_ref().is_some_and(|o| o.shared.failed());
        if !failed && self.last_device_check.elapsed() < DEVICE_CHECK {
            return;
        }
        if self.out.is_none() {
            // Waiting for any device to play on.
            if let Some((cur, pos)) = self.resume {
                self.last_device_check = Instant::now();
                if cur < self.queue.len() && kaseki::default_device_name().is_some() {
                    self.restart_at(cur, pos, true);
                    self.emit_queue();
                }
            }
            return;
        }
        self.last_device_check = Instant::now();
        let now = kaseki::default_device_name();
        let changed = now.is_some() && now != self.device;
        if !failed && !changed {
            return;
        }
        let gone = failed || self.device.as_deref().is_some_and(|d| !kaseki::device_exists(d));
        let state = match self.current {
            Some(cur) if self.active => Some((cur, self.position(), self.is_paused() || gone)),
            _ => None,
        };
        self.close_output();
        match state {
            Some((cur, pos, paused)) if cur < self.queue.len() => self.restart_at(cur, pos, paused),
            _ => {
                self.active = false;
                eprintln!("kaseki: audio output: default device is now {}", now.as_deref().unwrap_or("none"));
            }
        }
        self.emit_queue();
    }

    /// Close the output after a while paused or stopped.
    fn check_idle(&mut self) {
        if self.out.is_none() || (self.active && !self.paused) {
            self.idle_since = None;
            return;
        }
        let since = *self.idle_since.get_or_insert_with(Instant::now);
        if since.elapsed() >= IDLE_CLOSE {
            let pos = self.position();
            self.close_output();
            if self.active {
                self.suspended = Some(pos);
            }
            self.idle_since = None;
            eprintln!("kaseki: audio output: closed after {} s idle", IDLE_CLOSE.as_secs());
        }
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
        self.next_after(self.current?)
    }

    fn next_after(&self, cur: usize) -> Option<usize> {
        match self.repeat {
            Repeat::One => Some(cur),
            Repeat::All if !self.queue.is_empty() => Some((cur + 1) % self.queue.len()),
            _ if cur + 1 < self.queue.len() => Some(cur + 1),
            _ => None,
        }
    }

    fn tick(&mut self, force: bool) {
        self.poll();
        self.check_device();
        self.check_idle();
        self.fix_preload();
        self.sync_media();
        self.follow_listen();
        self.emit_tick(force);
    }

    /// Announce when a song starts playing, and once it has been heard long
    /// enough to count as listened to.
    fn follow_listen(&mut self) {
        let Some(q) = self.current.and_then(|c| self.queue.get(c)).cloned() else {
            self.listen = None;
            return;
        };
        let pos = self.position();
        let fresh = match &self.listen {
            // Repeat One plays the same item again: the position jumps back
            // after the song was counted.
            Some(l) => l.uid != q.uid || (l.scrobbled && pos + 2.0 < l.last_pos),
            None => true,
        };
        if fresh {
            self.listen = Some(Listen {
                uid: q.uid,
                track_id: q.track_id,
                started_at: now_secs() - pos as i64,
                heard: 0.0,
                last_pos: pos,
                announced: false,
                scrobbled: false,
            });
        }
        let playing = self.active && !self.paused && self.suspended.is_none();
        let Some(l) = self.listen.as_mut() else { return };
        if playing && !l.announced {
            l.announced = true;
            let _ = self.app.emit("player-now-playing", serde_json::json!({ "trackId": l.track_id }));
        }
        let delta = pos - l.last_pos;
        if playing && delta > 0.0 && delta < 2.0 {
            l.heard += delta;
        }
        l.last_pos = pos;
        if !l.scrobbled && listen_threshold(q.duration).is_some_and(|t| l.heard >= t) {
            l.scrobbled = true;
            let _ = self
                .app
                .emit("player-listened", serde_json::json!({ "trackId": l.track_id, "playedAt": l.started_at }));
        }
    }

    /// Tell the OS media controls about a new track, play/pause, or a seek.
    /// While anything is queued, an idle player reports "paused", never
    /// "stopped": macOS treats a stopped app as done and hands the media keys
    /// to another app (e.g. a browser tab).
    fn sync_media(&mut self) {
        let active = self.active;
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

    /// Ticks go out while playing, and otherwise only when something changed.
    fn emit_tick(&mut self, force: bool) {
        let (track_id, duration) = match self.current.and_then(|c| self.queue.get(c)) {
            Some(q) => (Some(q.track_id), q.duration),
            None => (None, 0.0),
        };
        let tick = Tick { track_id, position: self.position(), duration, playing: self.active && !self.paused };
        if !force && !tick.playing && self.last_tick.as_ref() == Some(&tick) {
            return;
        }
        let _ = self.app.emit("player-tick", tick.clone());
        self.last_tick = Some(tick);
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

/// Drives the real player (the Kaseki engine's decoder thread, ring buffer and the default output
/// device, at volume 0) through the commands the UI sends. Needs an audio
/// device, so it's opt-in:
/// `cargo test --lib player_end_to_end -- --ignored --nocapture --test-threads=1`
/// Set `HONEYCOMB_AUDIO_DIR` to a folder of real files to also play, seek and
/// skip through each of them.
#[cfg(test)]
mod tests {
    #[test]
    fn listen_threshold_follows_lastfm_rules() {
        assert_eq!(super::listen_threshold(30.0), None);
        assert_eq!(super::listen_threshold(100.0), Some(50.0));
        assert_eq!(super::listen_threshold(1200.0), Some(240.0));
    }

    use super::*;
    use crate::kaseki::tests::write_wav;
    use std::sync::{Arc, Mutex};
    use tauri::Listener;

    type Log = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

    struct Rig {
        player: PlayerHandle,
        log: Log,
        _app: tauri::App<tauri::test::MockRuntime>,
    }

    impl Rig {
        fn new(dir: &std::path::Path) -> Rig {
            let app = tauri::test::mock_app();
            let log: Log = Default::default();
            for name in ["player-tick", "player-queue", "track-played"] {
                let log = log.clone();
                app.listen_any(name, move |e| {
                    let v = serde_json::from_str(e.payload()).unwrap_or_default();
                    log.lock().unwrap().push((name.to_string(), v));
                });
            }
            let db_path = dir.join("library.db");
            db::migrate(&db::open(&db_path).unwrap()).unwrap();
            let player = spawn(app.handle().clone(), db_path, 0.0);
            Rig { player, log, _app: app }
        }

        fn send(&self, c: Cmd) {
            self.player.send(c);
        }

        fn last(&self, name: &str) -> Option<serde_json::Value> {
            self.log.lock().unwrap().iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.clone())
        }

        fn count(&self, name: &str) -> usize {
            self.log.lock().unwrap().iter().filter(|(n, _)| n == name).count()
        }

        /// Wait until the latest `name` event satisfies `f`.
        fn wait(&self, name: &str, secs: f64, what: &str, f: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
            let until = Instant::now() + Duration::from_secs_f64(secs);
            while Instant::now() < until {
                if let Some(v) = self.last(name).filter(|v| f(v)) {
                    return v;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("timed out waiting for {what}; last {name}: {:?}", self.last(name));
        }

        fn playing(&self, track: i64, what: &str) -> serde_json::Value {
            self.wait("player-tick", 3.0, what, |t| t["trackId"] == track && t["playing"] == true)
        }
    }

    fn item(id: i64, path: &std::path::Path, duration: f64) -> QItem {
        QItem { uid: 0, track_id: id, path: path.to_string_lossy().into_owned(), duration }
    }

    #[test]
    #[ignore]
    fn player_end_to_end() {
        let dir = std::env::temp_dir().join(format!("hc-player-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = |name: &str, secs: f64| {
            let p = dir.join(name);
            write_wav(&p, 44100, (44100.0 * secs) as u32, |i| (i % 2000) as i16);
            p
        };
        let (a, b, c, d) = (wav("a.wav", 2.0), wav("b.wav", 2.0), wav("c.wav", 2.0), wav("d.wav", 2.0));
        let rig = Rig::new(&dir);

        // Play through two tracks: the second follows on by itself and the
        // first is counted as played.
        rig.send(Cmd::Load(vec![item(1, &a, 2.0), item(2, &b, 2.0), item(3, &c, 2.0)], 0));
        rig.playing(1, "track 1 to play");
        let t = rig.wait("player-tick", 3.0, "track 2 to follow track 1", |t| t["trackId"] == 2);
        assert!(t["position"].as_f64().unwrap() < 1.0, "{t}");
        assert_eq!(rig.last("track-played").unwrap()["id"], 1);
        assert_eq!(rig.last("player-queue").unwrap()["current"], 1);

        // Seek, pause, play.
        rig.send(Cmd::Seek(1.5));
        rig.wait("player-tick", 1.0, "the seek", |t| (t["position"].as_f64().unwrap() - 1.5).abs() < 0.3);
        rig.send(Cmd::Pause);
        let paused = rig.wait("player-tick", 1.0, "pause", |t| t["playing"] == false);
        std::thread::sleep(Duration::from_millis(500));
        let still = rig.last("player-tick").unwrap();
        assert_eq!(still["position"], paused["position"], "position moved while paused");
        rig.send(Cmd::Play);
        rig.playing(2, "play after pause");

        // Next, previous.
        rig.send(Cmd::Next);
        rig.playing(3, "next");
        rig.send(Cmd::Prev);
        rig.playing(2, "previous");

        // A late queue change: track 1 is in the buffer as next, then track 4
        // is queued to play next. 4 must play, not the stale 1.
        rig.send(Cmd::Load(vec![item(1, &a, 2.0), item(2, &b, 2.0)], 0));
        rig.playing(1, "reload");
        rig.send(Cmd::Seek(1.2));
        std::thread::sleep(Duration::from_millis(300)); // decoder runs into track 2
        rig.send(Cmd::Enqueue(vec![item(4, &d, 2.0)], true));
        rig.wait("player-tick", 3.0, "the queued track after the late change", |t| t["trackId"] != 1);
        assert_eq!(rig.last("player-tick").unwrap()["trackId"], 4);

        // Removing the playing track moves on to the next, keeping pause.
        rig.send(Cmd::Pause);
        rig.send(Cmd::Remove(vec![1]));
        let t = rig.wait("player-tick", 2.0, "moving on after removal", |t| t["trackId"] == 2);
        assert_eq!(t["playing"], false);
        rig.send(Cmd::Play);
        rig.playing(2, "play after removal");

        // Shuffle keeps the playing track; repeat one replays it.
        rig.send(Cmd::Repeat(Repeat::One));
        let played = rig.count("track-played");
        rig.wait("track-played", 3.0, "repeat one", |_| rig.count("track-played") > played);
        rig.playing(2, "repeat one to keep playing track 2");
        rig.send(Cmd::Repeat(Repeat::Off));

        // The end of the queue stops, and then the player goes quiet.
        rig.wait("player-queue", 4.0, "the end of the queue", |q| q["current"].is_null());
        rig.wait("player-tick", 1.0, "a stopped tick", |t| t["playing"] == false);
        std::thread::sleep(Duration::from_millis(300));
        let ticks = rig.count("player-tick");
        std::thread::sleep(Duration::from_secs(2));
        assert_eq!(rig.count("player-tick"), ticks, "ticks kept coming while stopped");

        // Paused long enough for the output to close, play picks up where it was.
        let long = wav("long.wav", 60.0);
        rig.send(Cmd::Load(vec![item(5, &long, 60.0)], 0));
        rig.playing(5, "the long track");
        rig.send(Cmd::Seek(20.0));
        rig.send(Cmd::Pause);
        let paused = rig.wait("player-tick", 1.0, "pause", |t| t["playing"] == false);
        std::thread::sleep(IDLE_CLOSE + Duration::from_secs(3));
        rig.send(Cmd::Emit);
        let t = rig.wait("player-tick", 1.0, "a tick while closed", |t| t["playing"] == false);
        assert_eq!(t["position"], paused["position"]);
        rig.send(Cmd::Toggle);
        let t = rig.playing(5, "resume after the output closed");
        let resumed = t["position"].as_f64().unwrap();
        assert!((resumed - paused["position"].as_f64().unwrap()).abs() < 0.3, "{t}");

        // Every real file plays, seeks and skips.
        if let Ok(real) = std::env::var("HONEYCOMB_AUDIO_DIR") {
            let mut files: Vec<_> = std::fs::read_dir(real).unwrap().map(|e| e.unwrap().path()).collect();
            files.sort();
            let items: Vec<_> = files.iter().enumerate().map(|(i, f)| item(100 + i as i64, f, 20.0)).collect();
            rig.send(Cmd::Load(items, 0));
            for (i, f) in files.iter().enumerate() {
                let id = 100 + i as i64;
                rig.playing(id, &format!("{} to play", f.display()));
                rig.send(Cmd::Seek(15.0));
                let t = rig.wait("player-tick", 2.0, "the seek", |t| {
                    t["trackId"] == id && (t["position"].as_f64().unwrap() - 15.0).abs() < 0.5
                });
                println!("{}: playing, seeked to {:.2}", f.file_name().unwrap().to_string_lossy(), t["position"]);
                rig.send(Cmd::Next);
            }
        }
    }
}
