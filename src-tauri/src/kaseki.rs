//! Kaseki, Honeycomb's audio engine: the output stream, the decoder thread
//! and the ring buffer between them. `player` drives it.
//!
//! The decoder thread opens files with symphonia, converts channels, resamples
//! with rubato when the device runs at another rate, and keeps a lock-free
//! ring buffer (rtrb) several seconds ahead. The cpal callback only copies
//! from that buffer and applies the volume, so no file I/O or decoding ever
//! happens on the real-time thread.
//!
//! Gapless playback: when a track runs out, the decoder carries straight on
//! with the next one from the plan the player sent, writing it into the same
//! buffer. It drops a `Marker` at the sample where each track starts, and the
//! player compares those with the callback's sample counter to know what's
//! playing and where.

use std::collections::VecDeque;
use std::fs::File;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use rtrb::{Consumer, Producer, RingBuffer};
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{CODEC_TYPE_NULL, Decoder, DecoderOptions};
use symphonia::core::errors::{Error as SymError, SeekErrorKind};
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};

/// Seconds of audio the ring buffer holds.
const BUFFER_SECS: usize = 5;
/// The decoder tops the buffer up once this many seconds are free...
const REFILL_SECS: usize = 1;
/// ...checking this often while it's full.
const FULL_WAIT: Duration = Duration::from_millis(250);
/// Volume and pause changes ramp over this long, so they don't click.
const RAMP_SECS: f32 = 0.010;
/// Input frames per resampler call.
const RESAMPLE_CHUNK: usize = 1024;

/// Map the volume slider (0-1) to a gain. Loudness is roughly logarithmic, so
/// a cubic curve feels even across the slider where a linear one bunches all
/// the change at the bottom.
pub fn perceptual_gain(volume: f32) -> f32 {
    volume.clamp(0.0, 1.0).powi(3)
}

/// State shared between the player, the decoder and the output callback.
pub struct Shared {
    /// Samples the callback has taken from the buffer (played or discarded).
    consumed: AtomicU64,
    /// The callback drops everything before this sample (set on a flush).
    discard_until: AtomicU64,
    paused: AtomicBool,
    /// Gain as f32 bits.
    gain: AtomicU32,
    /// Raised by the stream's error callback (device gone).
    failed: AtomicBool,
}

impl Shared {
    fn new(gain: f32, paused: bool) -> Self {
        Shared {
            consumed: AtomicU64::new(0),
            discard_until: AtomicU64::new(0),
            paused: AtomicBool::new(paused),
            gain: AtomicU32::new(gain.to_bits()),
            failed: AtomicBool::new(false),
        }
    }

    pub fn consumed(&self) -> u64 {
        self.consumed.load(Ordering::Acquire)
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn set_volume(&self, volume: f32) {
        self.gain.store(perceptual_gain(volume).to_bits(), Ordering::Relaxed);
    }

    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }
}

/// Where a track (or the end of everything decoded) starts in the buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    /// Which `Start` this belongs to; older ones are stale.
    pub epoch: u64,
    /// 0 for the started track, then 1, 2… for each one played gaplessly after it.
    pub seq: u32,
    /// Queue item, or `None` for the end: nothing more was decoded after this.
    pub uid: Option<u64>,
    /// Sample index (interleaved) in the output's buffer.
    pub at: u64,
    /// Position in the track at `at`, in seconds.
    pub start: f64,
}

pub type Markers = Arc<Mutex<Vec<Marker>>>;

pub enum DecMsg {
    /// The output (re)opened, with a fresh buffer.
    Output { prod: Producer<f32>, shared: Arc<Shared>, rate: u32, channels: usize },
    /// Drop what's buffered and play `source` from `pos`.
    Start { epoch: u64, uid: u64, source: Box<Source>, pos: f64 },
    /// Tracks to play after segment `base`, in order: `items[k]` is segment `base + 1 + k`.
    Plan { epoch: u64, base: u32, items: Vec<(u64, String)> },
    /// Drop what's buffered and stay quiet.
    Stop,
    /// The output closed.
    Close,
}

// ---------- output ----------

pub struct Output {
    stream: cpal::Stream,
    pub shared: Arc<Shared>,
    pub rate: u32,
    pub channels: usize,
    pub device: Option<String>,
    default_rate: u32,
    /// Sample-rate ranges the device offers at this channel count and format.
    rates: Vec<(u32, u32)>,
}

pub fn default_device_name() -> Option<String> {
    cpal::default_host().default_output_device().and_then(|d| d.name().ok())
}

pub fn device_exists(name: &str) -> bool {
    cpal::default_host()
        .output_devices()
        .map(|mut ds| ds.any(|d| d.name().ok().as_deref() == Some(name)))
        .unwrap_or(false)
}

impl Output {
    /// Open the system's default output, at `want_rate` if the device can run
    /// at it (so nothing has to be resampled), otherwise at its default rate.
    pub fn open(want_rate: u32, volume: f32, paused: bool) -> Result<(Output, Producer<f32>), String> {
        let device = cpal::default_host().default_output_device().ok_or("No audio output device")?;
        let name = device.name().ok();
        let def = device.default_output_config().map_err(|e| format!("No audio output device: {e}"))?;
        let channels = def.channels();
        let format = def.sample_format();
        let rates: Vec<(u32, u32)> = device
            .supported_output_configs()
            .map(|cs| {
                cs.filter(|c| c.channels() == channels && c.sample_format() == format)
                    .map(|c| (c.min_sample_rate().0, c.max_sample_rate().0))
                    .collect()
            })
            .unwrap_or_default();
        let default_rate = def.sample_rate().0;
        let supported = |r: u32| rates.iter().any(|&(lo, hi)| lo <= r && r <= hi);
        let rate = if supported(want_rate) { want_rate } else { default_rate };

        let build = |rate: u32| {
            let shared = Arc::new(Shared::new(perceptual_gain(volume), paused));
            let (prod, cons) = RingBuffer::new(rate as usize * channels as usize * BUFFER_SECS);
            let config = cpal::StreamConfig {
                channels,
                sample_rate: cpal::SampleRate(rate),
                buffer_size: cpal::BufferSize::Default,
            };
            let flag = shared.clone();
            let on_error = move |e| {
                eprintln!("kaseki: audio stream error: {e}");
                flag.failed.store(true, Ordering::SeqCst);
            };
            let s = shared.clone();
            let (ch, r) = (channels as usize, rate);
            let stream = match format {
                SampleFormat::F32 => device.build_output_stream(&config, callback::<f32>(cons, s, ch, r), on_error, None),
                SampleFormat::I16 => device.build_output_stream(&config, callback::<i16>(cons, s, ch, r), on_error, None),
                SampleFormat::I32 => device.build_output_stream(&config, callback::<i32>(cons, s, ch, r), on_error, None),
                SampleFormat::U16 => device.build_output_stream(&config, callback::<u16>(cons, s, ch, r), on_error, None),
                other => return Err(format!("Unsupported output sample format {other}")),
            }
            .map_err(|e| format!("Can't open audio output: {e}"))?;
            stream.play().map_err(|e| format!("Can't start audio output: {e}"))?;
            Ok::<_, String>((stream, shared, prod))
        };
        let (stream, shared, prod, rate) = match build(rate) {
            Ok((st, sh, p)) => (st, sh, p, rate),
            Err(e) if rate != default_rate => {
                eprintln!("kaseki: audio output: {e} at {rate} Hz, using {default_rate} Hz");
                let (st, sh, p) = build(default_rate)?;
                (st, sh, p, default_rate)
            }
            Err(e) => return Err(e),
        };
        let out = Output { stream, shared, rate, channels: channels as usize, device: name, default_rate, rates };
        Ok((out, prod))
    }

    /// Whether this output already suits a file at `rate`: it runs at that
    /// rate, or the device can't and this is its default.
    pub fn suits(&self, rate: u32) -> bool {
        let supported = self.rates.iter().any(|&(lo, hi)| lo <= rate && rate <= hi);
        self.rate == rate || (!supported && self.rate == self.default_rate)
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        let _ = self.stream.pause();
    }
}

/// The real-time callback: copy from the buffer, apply the gain, and fill any
/// shortfall with silence. No locks, allocation or I/O.
fn callback<T: SizedSample + FromSample<f32>>(
    mut cons: Consumer<f32>,
    shared: Arc<Shared>,
    channels: usize,
    rate: u32,
) -> impl FnMut(&mut [T], &cpal::OutputCallbackInfo) + Send + 'static {
    let mut consumed = 0u64;
    let mut gain = 0f32;
    let step = 1.0 / (rate as f32 * RAMP_SECS);
    move |out: &mut [T], _| {
        let discard = shared.discard_until.load(Ordering::Acquire);
        if consumed < discard {
            let n = ((discard - consumed) as usize).min(cons.slots());
            if let Ok(chunk) = cons.read_chunk(n) {
                chunk.commit_all();
                consumed += n as u64;
            }
            // Fade in whatever comes after a jump.
            gain = 0.0;
        }
        let paused = shared.paused.load(Ordering::Relaxed);
        let target = if paused { 0.0 } else { f32::from_bits(shared.gain.load(Ordering::Relaxed)) };
        let mut filled = 0;
        if !(paused && gain == 0.0) {
            let n = out.len().min(cons.slots()) / channels * channels;
            if let Ok(chunk) = cons.read_chunk(n) {
                let (a, b) = chunk.as_slices();
                for (i, (o, &s)) in out.iter_mut().zip(a.iter().chain(b)).enumerate() {
                    if i % channels == 0 {
                        gain = if gain < target { (gain + step).min(target) } else { (gain - step).max(target) };
                    }
                    *o = T::from_sample(s * gain);
                }
                chunk.commit_all();
                consumed += n as u64;
                filled = n;
            }
        }
        for o in &mut out[filled..] {
            *o = T::EQUILIBRIUM;
        }
        shared.consumed.store(consumed, Ordering::Release);
    }
}

// ---------- decoding ----------

/// An open, decodable audio file.
pub struct Source {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    pub rate: u32,
    time_base: Option<TimeBase>,
    /// After a seek: drop decoded audio before this timestamp.
    skip_to: Option<u64>,
    /// A buffer decoded early (to learn the sample rate), returned first.
    first: Option<(Vec<f32>, usize, u32)>,
    sbuf: Option<(SampleBuffer<f32>, SignalSpec)>,
}

/// Decoded audio: interleaved samples, channel count, sample rate.
type Buf = (Vec<f32>, usize, u32);

impl Source {
    pub fn open(path: &str) -> Result<Source, String> {
        let file = File::open(path).map_err(|e| format!("Can't open {path}: {e}"))?;
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        // Decoders for unusual files have been known to panic; never let that
        // take a thread down with it.
        catch_unwind(AssertUnwindSafe(|| Self::open_file(file, &ext)))
            .map_err(|_| format!("Can't decode {path}: decoder crashed"))?
            .map_err(|e| format!("Can't decode {path}: {e}"))
    }

    fn open_file(file: File, ext: &str) -> Result<Source, String> {
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if !ext.is_empty() {
            hint.with_extension(ext);
        }
        let opts = FormatOptions { enable_gapless: true, ..Default::default() };
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &opts, &MetadataOptions::default())
            .map_err(|e| e.to_string())?;
        let format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or("no audio track")?;
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| e.to_string())?;
        let mut src = Source {
            track_id: track.id,
            rate: track.codec_params.sample_rate.unwrap_or(0),
            time_base: track.codec_params.time_base,
            format,
            decoder,
            skip_to: None,
            first: None,
            sbuf: None,
        };
        if src.rate == 0 {
            let first = src.decode_next().ok_or("no audio")?;
            src.rate = first.2;
            src.first = Some(first);
        }
        Ok(src)
    }

    /// Seek to `secs`; returns where playback will start, or `None` when
    /// that's past the end.
    fn seek(&mut self, secs: f64) -> Option<f64> {
        if secs <= 0.0 {
            return Some(0.0);
        }
        self.first = None;
        let to = SeekTo::Time { time: Time::from(secs), track_id: Some(self.track_id) };
        match self.format.seek(SeekMode::Accurate, to) {
            Ok(seeked) => {
                self.decoder.reset();
                self.skip_to = Some(seeked.required_ts);
                Some(secs)
            }
            Err(SymError::SeekError(SeekErrorKind::OutOfRange)) => None,
            Err(e) => {
                // Can't seek this file: play it from the start.
                eprintln!("kaseki: seek failed: {e}");
                Some(0.0)
            }
        }
    }

    /// The next decoded buffer, or `None` at the end of the file.
    fn decode_next(&mut self) -> Option<Buf> {
        if let Some(first) = self.first.take() {
            return Some(first);
        }
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(_) => return None, // end of stream, or unreadable from here on
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                Err(SymError::DecodeError(_)) => continue, // a corrupt packet; skip it
                Err(_) => return None,
            };
            let spec = *decoded.spec();
            let frames = decoded.frames();
            if frames == 0 {
                continue;
            }
            let ch = spec.channels.count().max(1);
            let sbuf = match &mut self.sbuf {
                Some((b, s)) if b.capacity() >= frames * ch && *s == spec => b,
                slot => &mut slot.insert((SampleBuffer::new(decoded.capacity() as u64, spec), spec)).0,
            };
            sbuf.copy_interleaved_ref(decoded);
            let mut s = sbuf.samples();

            // Encoder delay and padding: most decoders trim what the container
            // reports themselves; trim here for those that don't.
            let (trim_start, trim_end) = (packet.trim_start() as usize, packet.trim_end() as usize);
            if (trim_start > 0 || trim_end > 0) && frames as u64 == packet.block_dur() {
                let end = frames.saturating_sub(trim_end).max(trim_start.min(frames));
                s = &s[trim_start.min(frames) * ch..end * ch];
            }
            if let Some(req) = self.skip_to {
                let skip = ts_frames(self.time_base, self.rate, req.saturating_sub(packet.ts())) as usize;
                let have = s.len() / ch;
                if skip >= have {
                    continue;
                }
                s = &s[skip * ch..];
                self.skip_to = None;
            }
            if s.is_empty() {
                continue;
            }
            return Some((s.to_vec(), ch, spec.rate));
        }
    }
}

/// Timestamp units to frames.
fn ts_frames(time_base: Option<TimeBase>, rate: u32, ts: u64) -> u64 {
    match time_base {
        Some(tb) if tb.numer > 0 && tb.denom > 0 => {
            (ts as f64 * tb.numer as f64 * rate as f64 / tb.denom as f64).round() as u64
        }
        _ => ts,
    }
}

/// Copy interleaved audio from `in_ch` to `out_ch` channels.
fn map_channels(input: &[f32], in_ch: usize, out_ch: usize, out: &mut Vec<f32>) {
    if in_ch == out_ch {
        out.extend_from_slice(input);
    } else if in_ch == 1 {
        for &s in input {
            out.extend(std::iter::repeat_n(s, out_ch));
        }
    } else if out_ch == 1 {
        out.extend(input.chunks_exact(in_ch).map(|f| f.iter().sum::<f32>() / in_ch as f32));
    } else {
        // Keep the front left/right (and whatever else fits); drop the rest.
        for f in input.chunks_exact(in_ch) {
            out.extend((0..out_ch).map(|c| f.get(c).copied().unwrap_or(0.0)));
        }
    }
}

/// Band-limited sinc resampling with rubato, trimming its start-up delay so
/// the output lines up with the input.
struct Rs {
    r: SincFixedIn<f32>,
    ch: usize,
    ratio: f64,
    /// Planar input waiting for a full chunk.
    inbuf: Vec<Vec<f32>>,
    /// Output frames still to drop (the filter's delay).
    to_trim: usize,
    out_total: u64,
}

impl Rs {
    fn new(in_rate: u32, out_rate: u32, ch: usize) -> Result<Rs, String> {
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: rubato::calculate_cutoff(256, WindowFunction::BlackmanHarris2),
            oversampling_factor: 256,
            interpolation: SincInterpolationType::Linear,
            window: WindowFunction::BlackmanHarris2,
        };
        let ratio = out_rate as f64 / in_rate as f64;
        let r = SincFixedIn::<f32>::new(ratio, 1.0, params, RESAMPLE_CHUNK, ch).map_err(|e| e.to_string())?;
        let to_trim = r.output_delay();
        Ok(Rs { r, ch, ratio, inbuf: vec![Vec::new(); ch], to_trim, out_total: 0 })
    }

    fn push(&mut self, interleaved: &[f32], out: &mut Vec<f32>) {
        for f in interleaved.chunks_exact(self.ch) {
            for (c, &s) in f.iter().enumerate() {
                self.inbuf[c].push(s);
            }
        }
        loop {
            let need = self.r.input_frames_next();
            if self.inbuf[0].len() < need {
                break;
            }
            let res = {
                let chunk: Vec<&[f32]> = self.inbuf.iter().map(|c| &c[..need]).collect();
                self.r.process(&chunk, None)
            };
            for c in &mut self.inbuf {
                c.drain(..need);
            }
            match res {
                Ok(planar) => self.emit(&planar, out),
                Err(e) => {
                    eprintln!("kaseki: resampler: {e}");
                    break;
                }
            }
        }
    }

    fn emit(&mut self, planar: &[Vec<f32>], out: &mut Vec<f32>) {
        let n = planar.first().map_or(0, |c| c.len());
        let skip = self.to_trim.min(n);
        self.to_trim -= skip;
        out.reserve((n - skip) * self.ch);
        for f in skip..n {
            for c in planar {
                out.push(c[f]);
            }
        }
        self.out_total += (n - skip) as u64;
    }

    /// Flush the filter so the output holds exactly `expected` frames in all.
    /// `out[..keep]` is already in the buffer and can't be taken back.
    fn finish(&mut self, expected: u64, out: &mut Vec<f32>, keep: usize) {
        if !self.inbuf[0].is_empty() {
            if let Ok(planar) = self.r.process_partial(Some(&self.inbuf), None) {
                self.emit(&planar, out);
            }
            for c in &mut self.inbuf {
                c.clear();
            }
        }
        let mut guard = 0;
        while self.out_total < expected && guard < 64 {
            match self.r.process_partial(None::<&[Vec<f32>]>, None) {
                Ok(planar) => self.emit(&planar, out),
                Err(_) => break,
            }
            guard += 1;
        }
        if self.out_total > expected {
            let extra = ((self.out_total - expected) as usize * self.ch).min(out.len().saturating_sub(keep));
            out.truncate(out.len() - extra);
            self.out_total -= (extra / self.ch) as u64;
        }
    }
}

/// Turns one run of same-format tracks into output samples.
struct Chain {
    in_rate: u32,
    in_ch: usize,
    out_ch: usize,
    ratio: f64,
    rs: Option<Rs>,
    /// Buffer sample index where this chain's output starts.
    base: u64,
    /// Input frames fed so far.
    in_total: u64,
    scratch: Vec<f32>,
}

impl Chain {
    fn new(in_rate: u32, in_ch: usize, out_rate: u32, out_ch: usize, base: u64) -> Chain {
        let rs = if in_rate == out_rate {
            None
        } else {
            match Rs::new(in_rate, out_rate, out_ch) {
                Ok(rs) => Some(rs),
                Err(e) => {
                    eprintln!("kaseki: resampler: {e}");
                    None
                }
            }
        };
        let ratio = rs.as_ref().map_or(1.0, |r| r.ratio);
        Chain { in_rate, in_ch, out_ch, ratio, rs, base, in_total: 0, scratch: Vec::new() }
    }

    fn push(&mut self, data: &[f32], out: &mut Vec<f32>) {
        self.in_total += (data.len() / self.in_ch) as u64;
        match &mut self.rs {
            None => map_channels(data, self.in_ch, self.out_ch, out),
            Some(rs) => {
                self.scratch.clear();
                map_channels(data, self.in_ch, self.out_ch, &mut self.scratch);
                rs.push(&self.scratch, out);
            }
        }
    }

    /// The buffer sample index that the next input frame will land on.
    fn at(&self) -> u64 {
        self.base + (self.in_total as f64 * self.ratio).round() as u64 * self.out_ch as u64
    }

    fn finish(&mut self, out: &mut Vec<f32>, keep: usize) {
        let expected = (self.in_total as f64 * self.ratio).round() as u64;
        if let Some(rs) = &mut self.rs {
            rs.finish(expected, out, keep);
        }
    }
}

struct Out {
    prod: Producer<f32>,
    shared: Arc<Shared>,
    rate: u32,
    channels: usize,
    /// Samples written to the buffer so far.
    written: u64,
}

struct Playing {
    source: Box<Source>,
    seq: u32,
}

/// The decoder thread's state.
pub struct Dec {
    out: Option<Out>,
    markers: Markers,
    epoch: u64,
    cur: Option<Playing>,
    chain: Option<Chain>,
    plan: (u32, Vec<(u64, String)>),
    /// Output samples not yet in the buffer.
    pending: Vec<f32>,
    pending_off: usize,
    /// Decoding until the buffer is full; then waiting for room again.
    filling: bool,
    /// Sequence number of the latest marker.
    last_seq: u32,
}

pub fn spawn_decoder(markers: Markers) -> Sender<DecMsg> {
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("decoder".into())
        .spawn(move || Dec::new(markers).run(rx))
        .expect("spawn decoder thread");
    tx
}

impl Dec {
    pub fn new(markers: Markers) -> Dec {
        Dec {
            out: None,
            markers,
            epoch: 0,
            cur: None,
            chain: None,
            plan: (0, Vec::new()),
            pending: Vec::new(),
            pending_off: 0,
            filling: false,
            last_seq: 0,
        }
    }

    fn run(mut self, rx: Receiver<DecMsg>) {
        loop {
            let msg = if self.has_work() {
                match rx.try_recv() {
                    Ok(m) => Some(m),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => return,
                }
            } else if self.waiting() {
                match rx.recv_timeout(FULL_WAIT) {
                    Ok(m) => Some(m),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            } else {
                match rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => return,
                }
            };
            match msg {
                Some(m) => {
                    if catch_unwind(AssertUnwindSafe(|| self.handle(m))).is_err() {
                        self.halt();
                    }
                }
                None => {
                    if catch_unwind(AssertUnwindSafe(|| self.step())).is_err() {
                        eprintln!("kaseki: decoder crashed; ending the track");
                        self.cur = None;
                        self.end();
                    }
                }
            }
        }
    }

    fn has_audio(&self) -> bool {
        self.out.is_some() && (self.cur.is_some() || self.pending_off < self.pending.len())
    }

    fn has_work(&mut self) -> bool {
        if !self.has_audio() {
            return false;
        }
        if !self.filling {
            let out = self.out.as_ref().unwrap();
            self.filling = out.prod.slots() >= out.rate as usize * out.channels * REFILL_SECS;
        }
        self.filling
    }

    fn waiting(&self) -> bool {
        self.has_audio() && !self.filling
    }

    pub fn handle(&mut self, msg: DecMsg) {
        match msg {
            DecMsg::Output { prod, shared, rate, channels } => {
                self.out = Some(Out { prod, shared, rate, channels, written: 0 });
                self.reset();
            }
            DecMsg::Start { epoch, uid, mut source, pos } => {
                self.flush();
                self.epoch = epoch;
                self.plan = (0, Vec::new());
                let Some(at) = self.out.as_ref().map(|o| o.written) else { return };
                let start = catch_unwind(AssertUnwindSafe(|| source.seek(pos))).unwrap_or(Some(0.0));
                self.mark(Marker { epoch, seq: 0, uid: Some(uid), at, start: start.unwrap_or(pos) });
                match start {
                    Some(_) => {
                        self.cur = Some(Playing { source, seq: 0 });
                        self.filling = true;
                    }
                    // Past the end: the track is over.
                    None => self.end(),
                }
            }
            DecMsg::Plan { epoch, base, items } => {
                if epoch == self.epoch {
                    self.plan = (base, items);
                }
            }
            DecMsg::Stop => self.reset(),
            DecMsg::Close => {
                self.reset();
                self.out = None;
            }
        }
    }

    fn reset(&mut self) {
        self.flush();
        self.cur = None;
        self.chain = None;
        self.plan = (0, Vec::new());
    }

    /// Something went badly wrong: drop everything and wait for the next start.
    fn halt(&mut self) {
        self.reset();
    }

    /// Throw away what's buffered: the callback skips everything written so far.
    fn flush(&mut self) {
        self.pending.clear();
        self.pending_off = 0;
        self.chain = None;
        if let Some(out) = &self.out {
            out.shared.discard_until.store(out.written, Ordering::Release);
        }
        self.markers.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    fn mark(&mut self, m: Marker) {
        self.last_seq = m.seq;
        self.markers.lock().unwrap_or_else(|e| e.into_inner()).push(m);
    }

    /// Where the next output sample will land in the buffer.
    fn next_at(&self) -> u64 {
        let written = self.out.as_ref().map_or(0, |o| o.written);
        written + (self.pending.len() - self.pending_off) as u64
    }

    /// Write pending samples into the buffer, as many as fit.
    fn flush_pending(&mut self) {
        let Some(out) = &mut self.out else { return };
        let avail = self.pending.len() - self.pending_off;
        let n = avail.min(out.prod.slots()) / out.channels * out.channels;
        if n > 0 {
            if let Ok(mut chunk) = out.prod.write_chunk(n) {
                let (a, b) = chunk.as_mut_slices();
                let src = &self.pending[self.pending_off..self.pending_off + n];
                a.copy_from_slice(&src[..a.len()]);
                b.copy_from_slice(&src[a.len()..]);
                chunk.commit_all();
                self.pending_off += n;
                out.written += n as u64;
            }
        }
        if self.pending_off == self.pending.len() {
            self.pending.clear();
            self.pending_off = 0;
        }
    }

    /// Do one unit of work: write pending audio, or decode one more buffer.
    pub fn step(&mut self) {
        self.flush_pending();
        if self.pending_off < self.pending.len() {
            self.filling = false; // buffer full
            return;
        }
        let Some(cur) = &mut self.cur else { return };
        let Some((data, ch, rate)) = cur.source.decode_next() else {
            self.track_ended();
            return;
        };
        let (out_rate, out_ch) = match &self.out {
            Some(o) => (o.rate, o.channels),
            None => return,
        };
        if self.chain.as_ref().is_some_and(|c| c.in_rate != rate || c.in_ch != ch) {
            // The format changed mid-file: finish the old chain, start a new one.
            self.finish_chain();
        }
        let base = self.next_at();
        let chain = self.chain.get_or_insert_with(|| Chain::new(rate, ch, out_rate, out_ch, base));
        chain.push(&data, &mut self.pending);
    }

    fn finish_chain(&mut self) {
        if let Some(mut chain) = self.chain.take() {
            chain.finish(&mut self.pending, self.pending_off);
        }
    }

    /// The current track ran out: carry on gaplessly with the planned next
    /// track if its format matches, otherwise mark the end.
    fn track_ended(&mut self) {
        let Some(done) = self.cur.take() else { return };
        let (base, items) = &self.plan;
        let next = done
            .seq
            .checked_sub(*base)
            .and_then(|k| items.get(k as usize))
            .cloned();
        if let (Some((uid, path)), Some(chain)) = (next, &self.chain) {
            match catch_unwind(AssertUnwindSafe(|| Source::open(&path))) {
                // Same rate: gapless. Another rate ends here so the player can
                // reopen the output at the new rate instead of resampling.
                Ok(Ok(source)) if source.rate == chain.in_rate => {
                    let seq = done.seq + 1;
                    let at = chain.at();
                    self.cur = Some(Playing { source: Box::new(source), seq });
                    self.mark(Marker { epoch: self.epoch, seq, uid: Some(uid), at, start: 0.0 });
                    return;
                }
                Ok(Err(e)) => eprintln!("kaseki: {e}"),
                _ => {}
            }
        }
        self.end();
    }

    /// Nothing more to decode: flush the resampler and mark the end.
    fn end(&mut self) {
        let at = match &self.chain {
            Some(c) => c.at(),
            None => self.next_at(),
        };
        self.finish_chain();
        let seq = self.last_seq + 1;
        self.mark(Marker { epoch: self.epoch, seq, uid: None, at, start: 0.0 });
    }
}

/// Markers of `epoch` not yet taken, oldest first.
pub fn take_markers(markers: &Markers, epoch: u64, into: &mut VecDeque<Marker>) {
    let mut m = markers.lock().unwrap_or_else(|e| e.into_inner());
    into.extend(m.drain(..).filter(|m| m.epoch == epoch));
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// A 16-bit stereo WAV. Left holds `f(frame)`, right its negation.
    pub(crate) fn write_wav(path: &Path, rate: u32, frames: u32, f: impl Fn(u32) -> i16) {
        let data_len = frames * 4;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&2u16.to_le_bytes()); // stereo
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 4).to_le_bytes());
        b.extend_from_slice(&4u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..frames {
            let v = f(i);
            b.extend_from_slice(&v.to_le_bytes());
            b.extend_from_slice(&(-v.max(-i16::MAX)).to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hc-kaseki-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// Frame index encoded as a sample value, so order and gaps are visible.
    fn ramp(i: u32) -> i16 {
        (i % 30000) as i16
    }

    fn to_i16(s: f32) -> i16 {
        (s * 32768.0).round() as i16
    }

    struct Rig {
        dec: Dec,
        cons: Consumer<f32>,
        shared: Arc<Shared>,
        markers: Markers,
        got: Vec<f32>,
        seen: VecDeque<Marker>,
    }

    impl Rig {
        fn new(rate: u32, buffer_frames: usize) -> Rig {
            let markers: Markers = Default::default();
            let mut dec = Dec::new(markers.clone());
            let (prod, cons) = RingBuffer::new(buffer_frames * 2);
            let shared = Arc::new(Shared::new(1.0, false));
            dec.handle(DecMsg::Output { prod, shared: shared.clone(), rate, channels: 2 });
            Rig { dec, cons, shared, markers, got: Vec::new(), seen: VecDeque::new() }
        }

        fn start(&mut self, epoch: u64, uid: u64, path: &Path, pos: f64) {
            let source = Box::new(Source::open(path.to_str().unwrap()).unwrap());
            self.dec.handle(DecMsg::Start { epoch, uid, source, pos });
        }

        /// Read like the callback does, honouring flushes.
        fn read(&mut self, max: usize) -> usize {
            let discard = self.shared.discard_until.load(Ordering::Acquire);
            let mut consumed = self.shared.consumed();
            if consumed < discard {
                let n = ((discard - consumed) as usize).min(self.cons.slots());
                self.cons.read_chunk(n).unwrap().commit_all();
                consumed += n as u64;
            }
            let n = max.min(self.cons.slots());
            let chunk = self.cons.read_chunk(n).unwrap();
            let (a, b) = chunk.as_slices();
            self.got.extend_from_slice(a);
            self.got.extend_from_slice(b);
            chunk.commit_all();
            self.shared.consumed.store(consumed + n as u64, Ordering::Release);
            n
        }

        /// Run the decoder until it reports the end, draining as we go.
        fn run_to_end(&mut self, epoch: u64) {
            for _ in 0..1_000_000 {
                if self.dec.has_work() {
                    self.dec.step();
                }
                self.read(4096);
                take_markers(&self.markers, epoch, &mut self.seen);
                if self.seen.back().is_some_and(|m| m.uid.is_none()) && !self.dec.has_audio() {
                    while self.read(usize::MAX) > 0 {}
                    return;
                }
            }
            panic!("decoder never finished");
        }
    }

    #[test]
    fn plays_a_file_bit_exact() {
        let path = tmp("exact.wav");
        write_wav(&path, 44100, 44100, ramp);
        let mut rig = Rig::new(44100, 8192);
        rig.start(1, 7, &path, 0.0);
        rig.run_to_end(1);
        assert_eq!(rig.got.len(), 44100 * 2);
        for (i, f) in rig.got.chunks_exact(2).enumerate() {
            assert_eq!(to_i16(f[0]), ramp(i as u32), "frame {i}");
        }
        assert_eq!(rig.seen[0], Marker { epoch: 1, seq: 0, uid: Some(7), at: 0, start: 0.0 });
        assert_eq!(rig.seen[1].at, 44100 * 2);
    }

    #[test]
    fn stops_when_the_buffer_is_full_and_refills() {
        let path = tmp("refill.wav");
        write_wav(&path, 44100, 44100 * 3, ramp);
        // A one-second buffer; the decoder must stop at that and wait.
        let mut rig = Rig::new(44100, 44100);
        rig.start(1, 1, &path, 0.0);
        for _ in 0..10_000 {
            if !rig.dec.has_work() {
                break;
            }
            rig.dec.step();
        }
        assert!(!rig.dec.has_work(), "decoder kept going with a full buffer");
        assert!(rig.dec.waiting());
        assert_eq!(rig.cons.slots(), 44100 * 2, "buffer should be full");
        // Taking a little isn't enough to wake it; taking a second's worth is.
        rig.read(1000);
        assert!(!rig.dec.has_work());
        rig.read(44100 * 2);
        assert!(rig.dec.has_work());
        rig.run_to_end(1);
        assert_eq!(rig.got.len(), 44100 * 3 * 2);
        for (i, f) in rig.got.chunks_exact(2).enumerate() {
            assert_eq!(to_i16(f[0]), ramp(i as u32), "frame {i}");
        }
    }

    #[test]
    fn next_track_follows_without_a_gap() {
        let (a, b) = (tmp("gap-a.wav"), tmp("gap-b.wav"));
        write_wav(&a, 44100, 10000, |i| ramp(i));
        write_wav(&b, 44100, 7000, |i| ramp(i + 10000));
        let mut rig = Rig::new(44100, 4096);
        rig.start(3, 1, &a, 0.0);
        rig.dec.handle(DecMsg::Plan { epoch: 3, base: 0, items: vec![(2, b.to_string_lossy().into())] });
        rig.run_to_end(3);
        assert_eq!(rig.got.len(), 17000 * 2);
        for (i, f) in rig.got.chunks_exact(2).enumerate() {
            assert_eq!(to_i16(f[0]), ramp(i as u32), "frame {i}");
        }
        let seen: Vec<_> = rig.seen.iter().map(|m| (m.seq, m.uid, m.at)).collect();
        assert_eq!(seen, vec![(0, Some(1), 0), (1, Some(2), 20000), (2, None, 34000)]);
    }

    #[test]
    fn a_stale_plan_is_ignored() {
        let (a, b) = (tmp("stale-a.wav"), tmp("stale-b.wav"));
        write_wav(&a, 44100, 1000, ramp);
        write_wav(&b, 44100, 1000, ramp);
        let mut rig = Rig::new(44100, 4096);
        rig.start(5, 1, &a, 0.0);
        rig.dec.handle(DecMsg::Plan { epoch: 4, base: 0, items: vec![(2, b.to_string_lossy().into())] });
        rig.run_to_end(5);
        assert_eq!(rig.got.len(), 2000);
        assert_eq!(rig.seen.len(), 2);
    }

    #[test]
    fn seeks_to_the_exact_frame() {
        let path = tmp("seek.wav");
        write_wav(&path, 44100, 44100 * 2, ramp);
        let mut rig = Rig::new(44100, 8192);
        rig.start(1, 1, &path, 0.0);
        for _ in 0..20 {
            rig.dec.step();
            rig.read(512);
        }
        assert!(!rig.got.is_empty());
        // Seek while audio is buffered: the old audio must be dropped.
        rig.got.clear();
        rig.start(2, 1, &path, 0.5);
        rig.run_to_end(2);
        assert_eq!(rig.seen[0].start, 0.5);
        assert_eq!(rig.got.len(), (44100 * 2 - 22050) * 2);
        assert_eq!(to_i16(rig.got[0]), ramp(22050));
        assert_eq!(to_i16(rig.got[rig.got.len() - 2]), ramp(44100 * 2 - 1));
    }

    /// Least-squares fit of a sine at `freq`: (amplitude, RMS of what's left).
    /// Independent of any constant delay the filter adds.
    fn fit_sine(x: &[f64], freq: f64, rate: f64) -> (f64, f64) {
        let w = freq * std::f64::consts::TAU / rate;
        let (mut ss, mut sc, mut cc, mut xs, mut xc) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (i, &v) in x.iter().enumerate() {
            let (s, c) = (i as f64 * w).sin_cos();
            ss += s * s;
            sc += s * c;
            cc += c * c;
            xs += v * s;
            xc += v * c;
        }
        let det = ss * cc - sc * sc;
        let (a, b) = ((xs * cc - xc * sc) / det, (xc * ss - xs * sc) / det);
        let res: f64 = x
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let (s, c) = (i as f64 * w).sin_cos();
                (v - a * s - b * c).powi(2)
            })
            .sum();
        ((a * a + b * b).sqrt(), (res / x.len() as f64).sqrt())
    }

    fn resample_tone(name: &str, freq: f64, from: u32, to: u32) -> Rig {
        let path = tmp(name);
        let tone = move |i: u32| ((i as f64 * freq * std::f64::consts::TAU / from as f64).sin() * 16000.0) as i16;
        write_wav(&path, from, from, tone);
        let mut rig = Rig::new(to, 8192);
        rig.start(1, 1, &path, 0.0);
        rig.run_to_end(1);
        rig
    }

    #[test]
    fn seeking_past_the_end_ends_the_track() {
        let path = tmp("past.wav");
        write_wav(&path, 44100, 44100, ramp);
        let mut rig = Rig::new(44100, 8192);
        rig.start(1, 1, &path, 5.0);
        rig.run_to_end(1);
        assert!(rig.got.is_empty());
        let seen: Vec<_> = rig.seen.iter().map(|m| (m.uid, m.at)).collect();
        assert_eq!(seen, vec![(Some(1), 0), (None, 0)]);
    }

    #[test]
    fn resamples_with_a_clean_band_limited_filter() {
        // A 15 kHz tone from 44.1 to 48 kHz. Linear interpolation (what rodio
        // did) leaves about 10% of the signal as error here.
        let rig = resample_tone("rs-up.wav", 15000.0, 44100, 48000);
        // Exactly one second at the new rate, and the end marker agrees.
        assert_eq!(rig.got.len(), 48000 * 2);
        assert_eq!(rig.seen.back().unwrap().at, 48000 * 2);
        let left: Vec<f64> = rig.got.chunks_exact(2).map(|f| f[0] as f64).collect();
        assert!(left.iter().all(|s| s.is_finite()));
        let (amp, residual) = fit_sine(&left[2000..46000], 15000.0, 48000.0);
        let want = 16000.0 / 32768.0;
        assert!((amp - want).abs() < want * 0.01, "amplitude {amp}");
        assert!(residual < want * 0.002, "residual {residual}");
    }

    #[test]
    fn downsampling_filters_what_no_longer_fits() {
        // 23 kHz can't exist at 44.1 kHz. Without a filter it would fold back
        // as a loud 21.1 kHz tone; it must come out as near silence.
        let rig = resample_tone("rs-down.wav", 23000.0, 48000, 44100);
        assert_eq!(rig.got.len(), 44100 * 2);
        let left: Vec<f64> = rig.got.chunks_exact(2).map(|f| f[0] as f64).skip(2000).take(40000).collect();
        let rms = (left.iter().map(|s| s * s).sum::<f64>() / left.len() as f64).sqrt();
        assert!(rms < 16000.0 / 32768.0 * 0.001, "rms {rms}");
    }

    #[test]
    fn resampled_tracks_stay_gapless() {
        let (a, b) = (tmp("rsgap-a.wav"), tmp("rsgap-b.wav"));
        write_wav(&a, 44100, 30000, |_| 8000);
        write_wav(&b, 44100, 20000, |_| 8000);
        let mut rig = Rig::new(48000, 8192);
        rig.start(1, 1, &a, 0.0);
        rig.dec.handle(DecMsg::Plan { epoch: 1, base: 0, items: vec![(2, b.to_string_lossy().into())] });
        rig.run_to_end(1);
        let expected = (50000.0f64 * 48000.0 / 44100.0).round() as usize;
        assert_eq!(rig.got.len(), expected * 2);
        let b_at = rig.seen[1].at as usize;
        assert_eq!(b_at, (30000.0f64 * 48000.0 / 44100.0).round() as usize * 2);
        // A steady level across the boundary: no dip where the tracks meet.
        let lvl = 8000.0 / 32768.0;
        for f in rig.got[b_at - 2000..b_at + 2000].chunks_exact(2) {
            assert!((f[0] - lvl).abs() < 1e-3, "{}", f[0]);
        }
    }

    #[test]
    fn a_different_rate_ends_instead_of_joining() {
        let (a, b) = (tmp("rate-a.wav"), tmp("rate-b.wav"));
        write_wav(&a, 44100, 1000, ramp);
        write_wav(&b, 48000, 1000, ramp);
        let mut rig = Rig::new(44100, 4096);
        rig.start(1, 1, &a, 0.0);
        rig.dec.handle(DecMsg::Plan { epoch: 1, base: 0, items: vec![(2, b.to_string_lossy().into())] });
        rig.run_to_end(1);
        let seen: Vec<_> = rig.seen.iter().map(|m| m.uid).collect();
        assert_eq!(seen, vec![Some(1), None]);
    }

    /// Decode, seek and join every file in a folder of real encoded audio:
    /// `HONEYCOMB_AUDIO_DIR=… cargo test real_formats -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_formats() {
        let dir = std::env::var("HONEYCOMB_AUDIO_DIR").expect("set HONEYCOMB_AUDIO_DIR");
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
        files.sort();
        for f in &files {
            let src = Source::open(f.to_str().unwrap()).unwrap();
            let rate = src.rate;
            let mut rig = Rig::new(48000, 48000);
            rig.start(1, 1, f, 0.0);
            rig.run_to_end(1);
            let whole = rig.got.len() as f64 / 2.0 / 48000.0;
            let mut rig = Rig::new(48000, 48000);
            rig.start(1, 1, f, 10.0);
            rig.run_to_end(1);
            let after_seek = rig.got.len() as f64 / 2.0 / 48000.0;
            println!("{}: {rate} Hz, {whole:.3} s, {after_seek:.3} s after seeking to 10 s", f.display());
            assert!((whole - after_seek - 10.0).abs() < 0.1);
        }
        // All of them back to back, gaplessly where the rates match.
        let mut rig = Rig::new(44100, 44100);
        rig.start(1, 0, &files[0], 0.0);
        let items = files[1..].iter().enumerate().map(|(i, f)| (i as u64 + 1, f.to_string_lossy().into())).collect();
        rig.dec.handle(DecMsg::Plan { epoch: 1, base: 0, items });
        rig.run_to_end(1);
        println!("chain: {:?}", rig.seen.iter().map(|m| (m.uid, m.at)).collect::<Vec<_>>());
    }

    #[test]
    fn volume_curve_is_cubic() {
        assert_eq!(perceptual_gain(1.0), 1.0);
        assert_eq!(perceptual_gain(0.0), 0.0);
        assert!((perceptual_gain(0.5) - 0.125).abs() < 1e-6);
        assert_eq!(perceptual_gain(2.0), 1.0);
    }

    #[test]
    fn maps_channels() {
        let mut out = Vec::new();
        map_channels(&[0.5, 0.25], 1, 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5, 0.25, 0.25]);
        out.clear();
        map_channels(&[0.5, 0.25], 2, 1, &mut out);
        assert_eq!(out, vec![0.375]);
        out.clear();
        map_channels(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 6, 2, &mut out);
        assert_eq!(out, vec![1.0, 2.0]);
    }
}
