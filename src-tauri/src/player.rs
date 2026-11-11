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

use crate::kaseki::{self, DecMsg, Marker, Markers, Output, PlanItem, Source};
use crate::db;
use crate::models::{ReplayGain, now_secs};

const TICK: Duration = Duration::from_millis(200);
/// How often to check whether the system's default output device changed.
const DEVICE_CHECK: Duration = Duration::from_secs(2);
/// Close the output after this long paused or stopped, so the device can sleep.
const IDLE_CLOSE: Duration = Duration::from_secs(30);
/// Unplayable songs skipped in a row before playback stops.
const MAX_SKIPS: usize = 5;
/// How many tracks ahead the decoder is told about.
const PLAN_LEN: usize = 4;

#[derive(Debug, Clone)]
pub struct QItem {
    pub uid: u64,
    pub track_id: i64,
    pub path: String,
    pub duration: f64,
    pub rg: ReplayGain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RgMode {
    #[default]
    Off,
    Track,
    Album,
}

impl RgMode {
    pub fn parse(s: &str) -> RgMode {
        match s {
            "track" => RgMode::Track,
            "album" => RgMode::Album,
            _ => RgMode::Off,
        }
    }
}

/// The gain to apply in dB, or None when ReplayGain is off or the file has no
/// gain tags. Album mode falls back to the track gain and vice versa. A known
/// peak caps the gain so the loudest sample stays at or under full scale.
pub fn replay_gain(rg: &ReplayGain, mode: RgMode) -> Option<f32> {
    let track = rg.track_gain.map(|g| (g, rg.track_peak));
    let album = rg.album_gain.map(|g| (g, rg.album_peak));
    let (gain, peak) = match mode {
        RgMode::Off => return None,
        RgMode::Track => track.or(album)?,
        RgMode::Album => album.or(track)?,
    };
    Some(match peak {
        Some(p) if p > 0.0 => gain.min(-20.0 * p.log10()),
        _ => gain,
    })
}

fn linear(db: Option<f32>) -> f32 {
    db.map_or(1.0, |g| 10f32.powf(g / 20.0))
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
    ReplayGain(RgMode),
    Shuffle(bool),
    Repeat(Repeat),
    /// Add items at the end, or right after the current track when `next` is true.
    Enqueue(Vec<QItem>, bool),
    Jump(usize),
    Remove(Vec<usize>),
    /// Move the items at these indices so they sit together before index `to`
    /// (counted in the queue as it was; the length moves them to the end).
    Move(Vec<usize>, usize),
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

/// What's playing and how it reaches the speakers, for the quality badge.
#[derive(Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Signal {
    track_id: i64,
    source: Option<kaseki::SourceInfo>,
    /// ReplayGain applied to this song, in dB.
    replay_gain: Option<f32>,
    /// ReplayGain is on but the song has no gain tags.
    replay_gain_missing: bool,
    /// The output this song plays through. While the output is closed (paused
    /// for a while) it stays what it was for the same song; `None` until the
    /// song has played, or with no device.
    output: Option<OutputInfo>,
}

#[derive(Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct OutputInfo {
    device: Option<String>,
    rate: u32,
    channels: usize,
    format: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct QueueEvent {
    /// Left out when the list is the same as last time.
    #[serde(skip_serializing_if = "Option::is_none")]
    track_ids: Option<Vec<i64>>,
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
    duration: f64,
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

pub fn spawn<R: Runtime>(app: AppHandle<R>, db_path: PathBuf, volume: f32, rg_mode: RgMode) -> PlayerHandle {
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || Engine::new(app, db_path, volume, rg_mode).run(rx))
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
    /// The track ids last sent with `player-queue`.
    sent_ids: Vec<i64>,
    queue: Vec<QItem>,
    /// Queue order before shuffling, restored when shuffle is turned off.
    unshuffled: Option<Vec<QItem>>,
    current: Option<usize>,
    repeat: Repeat,
    volume: f32,
    rg_mode: RgMode,
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
    /// Why the playing song is stopping and where (see `note_ending`).
    ending: Option<(&'static str, f64)>,
    /// (queue item uid, what `kaseki::probe` found in its file).
    probed: Option<(u64, Option<kaseki::SourceInfo>)>,
    last_signal: Option<Signal>,
}

impl<R: Runtime> Engine<R> {
    fn new(app: AppHandle<R>, db_path: PathBuf, volume: f32, rg_mode: RgMode) -> Self {
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
            sent_ids: Vec::new(),
            queue: Vec::new(),
            unshuffled: None,
            current: None,
            repeat: Repeat::Off,
            volume,
            rg_mode,
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
            ending: None,
            probed: None,
            last_signal: None,
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
                self.note_ending("replaced");
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
                self.note_ending("next");
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
                        self.note_ending("previous");
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
            Cmd::ReplayGain(mode) => {
                if mode != self.rg_mode {
                    self.rg_mode = mode;
                    // Already decoded audio has the old gain baked in.
                    if let (true, None, Some(cur)) = (self.active, self.suspended, self.current) {
                        let (pos, paused) = (self.position(), self.is_paused());
                        self.restart_at(cur, pos, paused);
                    }
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
                    self.note_ending("jump");
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
                        self.note_ending("removed");
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
            Cmd::Move(idxs, to) => {
                // The playing track keeps playing wherever it ends up. When
                // shuffled, only the shuffled order changes; turning shuffle
                // off still brings back the original order.
                let current_uid = self.current.map(|c| self.queue[c].uid);
                move_items(&mut self.queue, &idxs, to);
                self.current = current_uid.and_then(|u| self.queue.iter().position(|q| q.uid == u));
            }
            Cmd::Clear => {
                self.note_ending("stopped");
                self.queue.clear();
                self.unshuffled = None;
                self.stop();
            }
            // The window asking for everything, e.g. after a reload.
            Cmd::Emit => self.sent_ids.clear(),
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
    /// files are skipped, giving up after a few in a row (a drive that isn't
    /// plugged in would otherwise fail on every song in the queue).
    fn restart_at(&mut self, index: usize, pos: f64, paused: bool) {
        if index >= self.queue.len() {
            return;
        }
        let mut i = index;
        let mut skipped = 0;
        while skipped < self.queue.len().min(MAX_SKIPS) {
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
                    let db = replay_gain(&self.queue[i].rg, self.rg_mode);
                    if self.rg_mode != RgMode::Off {
                        eprintln!("replaygain: {:?} mode, {}", self.rg_mode, db.map_or("no tags".into(), |g| format!("{g:+.1} dB")));
                    }
                    let gain = linear(db);
                    let _ = self.dec.send(DecMsg::Start { epoch: self.epoch, uid, source: Box::new(source), pos, gain });
                    self.current = Some(i);
                    self.error = None;
                    self.send_plan();
                    return;
                }
                Err(e) => {
                    skipped += 1;
                    if skipped == 1 {
                        let _ = self.app.emit("player-error", &e);
                    } else {
                        eprintln!("player: {e}");
                    }
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
        if skipped == MAX_SKIPS && self.queue.len() > MAX_SKIPS {
            let _ = self.app.emit("player-error", format!("Stopped after {MAX_SKIPS} songs in a row couldn't be opened"));
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
            let cut = self.segs[1].cut;
            if let Some(uid) = self.segs.pop_front().and_then(|m| m.uid) {
                finished.push((uid, cut));
            }
        }
        if finished.is_empty() {
            return;
        }
        // A track that broke off on a read error wasn't heard to the end.
        for (uid, _) in finished.iter().filter(|(_, cut)| !cut) {
            if let Some(q) = self.queue.iter().find(|q| q.uid == *uid) {
                self.record_play(q.track_id);
            }
        }
        let finished: Vec<u64> = finished.into_iter().map(|(uid, _)| uid).collect();
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
        let items: Vec<PlanItem> = idxs
            .iter()
            .map(|&i| {
                let q = &self.queue[i];
                PlanItem { uid: q.uid, path: q.path.clone(), gain: linear(replay_gain(&q.rg, self.rg_mode)) }
            })
            .collect();
        crate::mirror::prefetch(items.iter().map(|i| i.path.as_str()));
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
        self.follow_signal(force);
        self.emit_tick(force);
    }

    /// Send `player-signal` when the track or the output changes.
    fn follow_signal(&mut self, force: bool) {
        let sig = self.current.and_then(|c| self.queue.get(c)).map(|q| {
            if self.probed.as_ref().map(|p| p.0) != Some(q.uid) {
                self.probed = Some((q.uid, kaseki::probe(&q.path)));
            }
            let replay_gain = replay_gain(&q.rg, self.rg_mode);
            Signal {
                track_id: q.track_id,
                source: self.probed.as_ref().and_then(|p| p.1.clone()),
                replay_gain,
                replay_gain_missing: self.rg_mode != RgMode::Off && replay_gain.is_none(),
                output: self
                    .out
                    .as_ref()
                    .map(|o| OutputInfo {
                        device: o.device.clone(),
                        rate: o.rate,
                        channels: o.channels,
                        format: o.format.clone(),
                    })
                    // Closed to save power: the song still plays through the
                    // same output when it resumes, so keep showing it.
                    .or_else(|| {
                        self.last_signal.as_ref().filter(|s| s.track_id == q.track_id).and_then(|s| s.output.clone())
                    }),
            }
        });
        if !force && sig == self.last_signal {
            return;
        }
        let _ = self.app.emit("player-signal", &sig);
        self.last_signal = sig;
    }

    /// Announce when a song starts playing, and once it has been heard long
    /// enough to count as listened to.
    fn follow_listen(&mut self) {
        let Some(q) = self.current.and_then(|c| self.queue.get(c)).cloned() else {
            if let Some(old) = self.listen.take() {
                self.end_listen(old);
            }
            self.ending = None;
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
            if let Some(old) = self.listen.take() {
                self.end_listen(old);
            }
            self.listen = Some(Listen {
                uid: q.uid,
                track_id: q.track_id,
                duration: q.duration,
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
        // A command that didn't end this song (e.g. Previous restarting it) leaves nothing behind.
        self.ending = None;
    }

    /// Remember why the playing song is about to stop, and where, for the
    /// listening history. The first reason in a burst of commands wins.
    fn note_ending(&mut self, reason: &'static str) {
        if self.listen.is_some() && self.ending.is_none() {
            self.ending = Some((reason, self.position()));
        }
    }

    /// Write a finished listen to the history. Songs that never actually
    /// played (loaded paused, then skipped) aren't recorded.
    fn end_listen(&mut self, l: Listen) {
        if !l.announced {
            return;
        }
        let (reason, position) = self.ending.take().unwrap_or(("finished", l.duration));
        let end = db::ListenEnd {
            track_id: l.track_id,
            started_at: l.started_at,
            ended_at: now_secs(),
            position,
            duration: l.duration,
            heard: l.heard,
            reason,
        };
        if let Ok(conn) = db::open(&self.db_path) {
            let _ = db::record_listen(&conn, &end);
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
            let _ = crate::mirror::note_play(&conn, track_id, when);
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

    fn emit_queue(&mut self) {
        let same = self.sent_ids.len() == self.queue.len() && self.sent_ids.iter().zip(&self.queue).all(|(a, q)| *a == q.track_id);
        let track_ids = (!same).then(|| {
            self.sent_ids = self.queue.iter().map(|q| q.track_id).collect();
            self.sent_ids.clone()
        });
        let _ = self.app.emit(
            "player-queue",
            QueueEvent {
                track_ids,
                current: self.current,
                shuffle: self.unshuffled.is_some(),
                repeat: self.repeat,
                volume: self.volume,
                error: self.error.clone(),
            },
        );
    }
}

/// Move the items at `idxs` (in their current order) to sit before index `to`
/// of the original list. Out-of-range indices are ignored.
fn move_items<T>(v: &mut Vec<T>, idxs: &[usize], to: usize) {
    let mut idxs: Vec<usize> = idxs.iter().copied().filter(|&i| i < v.len()).collect();
    idxs.sort_unstable();
    idxs.dedup();
    let to = to.min(v.len());
    let at = to - idxs.iter().filter(|&&i| i < to).count();
    let mut taken: Vec<T> = idxs.iter().rev().map(|&i| v.remove(i)).collect();
    taken.reverse();
    v.splice(at..at, taken);
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
    fn replay_gain_picks_and_caps() {
        let rg = ReplayGain { track_gain: Some(-6.0), track_peak: Some(0.5), album_gain: Some(-8.0), album_peak: None };
        assert_eq!(replay_gain(&rg, RgMode::Off), None);
        assert_eq!(replay_gain(&rg, RgMode::Track), Some(-6.0));
        assert_eq!(replay_gain(&rg, RgMode::Album), Some(-8.0));
        // Each mode falls back to the other gain.
        let track_only = ReplayGain { track_gain: Some(-3.0), ..Default::default() };
        assert_eq!(replay_gain(&track_only, RgMode::Album), Some(-3.0));
        assert_eq!(replay_gain(&ReplayGain::default(), RgMode::Track), None);
        // A quiet song turned up stops where its peak reaches full scale.
        let quiet = ReplayGain { track_gain: Some(9.0), track_peak: Some(0.5), ..Default::default() };
        let g = replay_gain(&quiet, RgMode::Track).unwrap();
        assert!((g - 6.0206).abs() < 1e-3, "{g}");
        assert!((linear(Some(g)) * 0.5 - 1.0).abs() < 1e-4);
        assert_eq!(linear(None), 1.0);
    }

    #[test]
    fn listen_threshold_follows_lastfm_rules() {
        assert_eq!(super::listen_threshold(30.0), None);
        assert_eq!(super::listen_threshold(100.0), Some(50.0));
        assert_eq!(super::listen_threshold(1200.0), Some(240.0));
    }

    #[test]
    fn move_items_keeps_the_moved_ones_together_in_order() {
        let moved = |idxs: &[usize], to: usize| {
            let mut v = vec!['a', 'b', 'c', 'd', 'e'];
            super::move_items(&mut v, idxs, to);
            v.into_iter().collect::<String>()
        };
        assert_eq!(moved(&[0], 3), "bcade", "down: lands before what was at index 3");
        assert_eq!(moved(&[3], 1), "adbce", "up");
        assert_eq!(moved(&[4, 1], 0), "beacd", "several, picked out of order, keep list order");
        assert_eq!(moved(&[0, 2], 5), "bdeac", "to the end");
        assert_eq!(moved(&[1, 3], 2), "abdce", "dropped between the moved ones");
        assert_eq!(moved(&[2], 2), "abcde", "onto itself");
        assert_eq!(moved(&[9], 0), "abcde", "out of range is ignored");
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
            for name in ["player-tick", "player-queue", "track-played", "player-error"] {
                let log = log.clone();
                app.listen_any(name, move |e| {
                    let v = serde_json::from_str(e.payload()).unwrap_or_default();
                    log.lock().unwrap().push((name.to_string(), v));
                });
            }
            let db_path = dir.join("library.db");
            db::migrate(&db::open(&db_path).unwrap()).unwrap();
            let player = spawn(app.handle().clone(), db_path, 0.0, RgMode::Off);
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
        QItem { uid: 0, track_id: id, path: path.to_string_lossy().into_owned(), duration, rg: ReplayGain::default() }
    }

    /// The queue's ids go out only when the list changes, or when the window
    /// asks for everything.
    #[test]
    fn queue_ids_are_sent_only_when_they_change() {
        let dir = std::env::temp_dir().join(format!("hc-queueids-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rig = Rig::new(&dir);
        let has_ids = |what: &str| rig.wait("player-queue", 2.0, what, |_| true).get("trackIds").is_some();
        rig.send(Cmd::Enqueue(vec![item(1, &dir.join("a.flac"), 60.0), item(2, &dir.join("b.flac"), 60.0)], false));
        rig.wait("player-queue", 2.0, "the new queue", |q| q["trackIds"] == serde_json::json!([1, 2]));
        let sent = rig.count("player-queue");
        rig.send(Cmd::Volume(0.5));
        rig.wait("player-queue", 2.0, "the volume change", |q| q["volume"] == 0.5);
        assert_eq!(rig.count("player-queue"), sent + 1);
        assert!(!has_ids("volume"), "a volume change doesn't resend the list");
        rig.send(Cmd::Emit);
        rig.wait("player-queue", 2.0, "the full state", |q| q.get("trackIds").is_some());
    }

    /// With the drive gone, playback stops after a few missing songs instead
    /// of trying (and reporting) every one in the queue.
    #[test]
    fn missing_files_stop_playback_after_a_few() {
        let dir = std::env::temp_dir().join(format!("hc-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rig = Rig::new(&dir);
        let gone = dir.join("unplugged");
        let items = (1..=12).map(|i| item(i, &gone.join(format!("{i}.flac")), 60.0)).collect();
        rig.send(Cmd::Load(items, 0));
        rig.wait("player-error", 3.0, "giving up", |e| e.as_str().is_some_and(|m| m.starts_with("Stopped after")));
        assert_eq!(rig.count("player-error"), 2, "the first failure, then one about stopping");
        assert!(rig.last("player-queue").is_some_and(|q| q["current"].is_null()));
    }

    #[test]
    #[ignore]
    fn listening_history_records_how_songs_end() {
        let dir = std::env::temp_dir().join(format!("hc-listens-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = |name: &str, secs: f64| {
            let p = dir.join(name);
            write_wav(&p, 44100, (44100.0 * secs) as u32, |i| (i % 2000) as i16);
            p
        };
        let (a, b, c) = (wav("a.wav", 3.0), wav("b.wav", 2.0), wav("c.wav", 3.0));
        let rig = Rig::new(&dir);
        let conn = db::open(&dir.join("library.db")).unwrap();
        db::set_setting(&conn, "listenLog", "on").unwrap();
        for p in [&a, &b, &c] {
            conn.execute("INSERT INTO tracks (path, date_added) VALUES (?1, 0)", [p.to_string_lossy()]).unwrap();
        }

        // Skipped early, played to the end, then stopped partway.
        rig.send(Cmd::Load(vec![item(1, &a, 3.0), item(2, &b, 2.0), item(3, &c, 3.0)], 0));
        rig.playing(1, "track 1 to play");
        std::thread::sleep(Duration::from_millis(500));
        rig.send(Cmd::Next);
        rig.playing(2, "next");
        rig.playing(3, "track 3 to follow track 2");
        std::thread::sleep(Duration::from_millis(800));
        rig.send(Cmd::Clear);
        rig.wait("player-queue", 2.0, "the stop", |q| q["current"].is_null());
        std::thread::sleep(Duration::from_millis(300));

        let rows: Vec<(i64, String, f64)> = conn
            .prepare("SELECT track_id, reason, position FROM listens ORDER BY ended_at, rowid")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let reasons: Vec<(i64, &str)> = rows.iter().map(|(t, r, _)| (*t, r.as_str())).collect();
        assert_eq!(reasons, [(1, "next"), (2, "finished"), (3, "stopped")], "{rows:?}");
        assert!((0.3..1.5).contains(&rows[0].2), "skipped at {}", rows[0].2);
        assert!((0.5..2.0).contains(&rows[2].2), "stopped at {}", rows[2].2);
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

    /// Plays the test Navidrome's gapless album through the real output:
    /// `HONEYCOMB_NAVIDROME=http://127.0.0.1:4533 cargo test --lib live_server_playback -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_server_playback() {
        let Ok(url) = std::env::var("HONEYCOMB_NAVIDROME") else { return };
        let dir = std::env::temp_dir().join(format!("hc-player-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rig = Rig::new(&dir);
        let db_path = dir.join("library.db");
        let mut conn = db::open(&db_path).unwrap();
        crate::mirror::migrate(&conn).unwrap();
        crate::mirror::init(&db_path, &dir);
        crate::kaseki::set_opener(crate::mirror::resolve);
        let id = crate::mirror::add_source(&conn, &url, "honeycomb", "honeycomb-test").unwrap();
        let client = crate::mirror::client(&conn, id).unwrap();
        let pulled = crate::mirror::pull(&client, &|_| {}).unwrap();
        crate::mirror::apply(&mut conn, id, &pulled).unwrap();
        let mut suite: Vec<_> = db::all_tracks(&conn).unwrap().into_iter().filter(|t| t.album == "Continuous Suite").collect();
        suite.sort_by_key(|t| t.track_no);
        let items: Vec<QItem> = suite.iter().map(|t| QItem { uid: 0, track_id: t.id, path: t.path.clone(), duration: t.duration, rg: t.rg }).collect();
        let (m2, m3) = (suite[1].id, suite[2].id);

        rig.send(Cmd::Load(items, 1));
        rig.wait("player-tick", 10.0, "movement 2 to stream", |t| t["trackId"] == m2 && t["playing"] == true);
        rig.send(Cmd::Seek(suite[1].duration - 2.5));
        let t = rig.wait("player-tick", 8.0, "movement 3 to follow", |t| t["trackId"] == m3);
        assert!(t["position"].as_f64().unwrap() < 1.0, "a gap or a restart: {t}");
        assert_eq!(rig.last("track-played").unwrap()["id"], m2);
        rig.send(Cmd::Pause);
        let waiting: i64 = conn.query_row("SELECT COUNT(*) FROM remote_outbox WHERE kind = 'play'", [], |r| r.get(0)).unwrap();
        assert_eq!(waiting, 1, "the play waits for the server");
        assert_eq!(crate::mirror::flush(&conn).unwrap(), 1);
        crate::mirror::remove_source(&mut conn, id, None).unwrap();
    }
}
