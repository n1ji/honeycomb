//! Converting lossless files for players while syncing.
//!
//! Audio is decoded, folded down to stereo and resampled with Kaseki's own
//! pieces, so a converted file starts from exactly what Honeycomb would play.
//! Encoders are built in: LAME for MP3 (with its LAME tag, so players that
//! honour it play albums gaplessly), libvorbis for Ogg Vorbis and flacenc for
//! 16-bit FLAC. AAC uses Apple's encoder through `afconvert`, which ships with
//! macOS; there's no AAC encoder that can be built into a GPL app.
//!
//! Tags and embedded artwork are copied over with lofty.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::tag::{Tag, TagExt, TagType};
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::kaseki::{self, Rs, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Mp3,
    Aac,
    Vorbis,
    /// 16-bit FLAC, for players short on space that should stay lossless.
    Flac16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    High,
    Smaller,
}

/// What lossless files are converted to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Convert {
    pub format: Format,
    pub quality: Quality,
}

impl Convert {
    pub fn ext(&self) -> &'static str {
        match self.format {
            Format::Mp3 => "mp3",
            Format::Aac => "m4a",
            Format::Vorbis => "ogg",
            Format::Flac16 => "flac",
        }
    }

    /// Recorded with each converted file, so changing the choice converts again.
    pub fn key(&self) -> String {
        let format = match self.format {
            Format::Mp3 => "mp3",
            Format::Aac => "aac",
            Format::Vorbis => "vorbis",
            Format::Flac16 => return "flac16".into(),
        };
        let quality = if self.quality == Quality::High { "high" } else { "smaller" };
        format!("{format}-{quality}")
    }

    /// Roughly how big a second of audio comes out, for the free-space check.
    pub fn bytes_per_sec(&self) -> u64 {
        let kbps = match (self.format, self.quality) {
            (Format::Mp3, Quality::High) => 260,
            (Format::Mp3, Quality::Smaller) => 200,
            (Format::Aac, Quality::High) => 270,
            (Format::Aac, Quality::Smaller) => 200,
            (Format::Vorbis, Quality::High) => 230,
            (Format::Vorbis, Quality::Smaller) => 170,
            // 16-bit FLAC: about 60% of CD audio's 1411 kbps, rounded up.
            (Format::Flac16, _) => 950,
        };
        kbps * 1000 / 8
    }

    /// Whether a track is converted or copied as it is. Only lossless files
    /// (the ones with a bit depth) are converted; for 16-bit FLAC, files that
    /// are already 16-bit at 48 kHz or less stay as they are.
    pub fn applies(&self, bit_depth: Option<u8>, sample_rate: Option<u32>) -> bool {
        let Some(bits) = bit_depth else { return false };
        match self.format {
            Format::Flac16 => bits > 16 || sample_rate.unwrap_or(0) > 48_000,
            _ => true,
        }
    }
}

/// Whether this system can encode AAC.
pub fn aac_available() -> bool {
    cfg!(target_os = "macos")
}

/// The rate a file is converted at: its own up to 48 kHz; above that, 44.1
/// kHz for multiples of it (88.2, 176.4) and 48 kHz otherwise.
fn target_rate(rate: u32) -> u32 {
    match rate {
        0..=48_000 => rate,
        r if r % 44_100 == 0 => 44_100,
        _ => 48_000,
    }
}

/// Convert `src` into `dst` (which must not exist yet) and copy its tags.
/// Returns the size of the new file. On failure or when `cancel` is set,
/// whatever was written is the caller's to delete.
pub fn convert(src: &Path, dst: &Path, c: Convert, cancel: &AtomicBool) -> Result<u64> {
    let mut pcm = Pcm::open(src, cancel)?;
    match c.format {
        Format::Mp3 => encode_mp3(&mut pcm, dst, c.quality)?,
        Format::Vorbis => encode_vorbis(&mut pcm, dst, c.quality)?,
        Format::Flac16 => encode_flac16(&mut pcm, dst)?,
        Format::Aac => encode_aac(&mut pcm, dst, c.quality)?,
    }
    copy_tags(src, dst, c.format).context("copying tags")?;
    Ok(std::fs::metadata(dst)?.len())
}

// ---------- decoding ----------

/// A file's audio as interleaved f32 chunks, at most stereo, at `rate`.
struct Pcm<'a> {
    src: Source,
    in_ch: usize,
    pub ch: usize,
    pub rate: u32,
    fold: Vec<[f32; 2]>,
    rs: Option<Rs>,
    first: Option<Vec<f32>>,
    in_frames: u64,
    done: bool,
    cancel: &'a AtomicBool,
}

impl<'a> Pcm<'a> {
    fn open(path: &Path, cancel: &'a AtomicBool) -> Result<Pcm<'a>> {
        let mut src = Source::open(&path.to_string_lossy()).map_err(anyhow::Error::msg)?;
        let Some((first, in_ch, in_rate)) = src.decode_next() else { bail!("no audio in the file") };
        let ch = in_ch.min(2);
        let rate = target_rate(in_rate);
        let rs = if rate != in_rate { Some(Rs::new(in_rate, rate, ch).map_err(anyhow::Error::msg)?) } else { None };
        let fold = kaseki::stereo_fold(src.layout(), in_ch);
        let mut pcm = Pcm { src, in_ch, ch, rate, fold, rs, first: None, in_frames: 0, done: false, cancel };
        pcm.first = Some(first);
        Ok(pcm)
    }

    /// The next chunk, or None at the end.
    fn next(&mut self) -> Result<Option<Vec<f32>>> {
        if self.cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        if self.done {
            return Ok(None);
        }
        let input = match self.first.take() {
            Some(s) => Some(s),
            None => self.src.decode_next().map(|(s, ch, _)| {
                self.in_ch = ch;
                s
            }),
        };
        let mut mapped = Vec::new();
        let mut out = Vec::new();
        match input {
            Some(s) => {
                self.in_frames += (s.len() / self.in_ch) as u64;
                kaseki::map_channels(&s, self.in_ch, self.ch, &self.fold, &mut mapped);
                match &mut self.rs {
                    Some(rs) => rs.push(&mapped, &mut out),
                    None => out = mapped,
                }
            }
            None => {
                self.done = true;
                if let Some(rs) = &mut self.rs {
                    let ratio = self.rate as f64 / self.src.rate as f64;
                    rs.finish((self.in_frames as f64 * ratio).round() as u64, &mut out, 0);
                }
                if out.is_empty() {
                    return Ok(None);
                }
            }
        }
        Ok(Some(out))
    }
}

// ---------- encoders ----------

fn encode_mp3(pcm: &mut Pcm, dst: &Path, q: Quality) -> Result<()> {
    use mp3lame_encoder::{Builder, FlushGap, InterleavedPcm, MonoPcm, Quality as Lq, VbrMode};
    let err = |e: mp3lame_encoder::BuildError| anyhow::anyhow!("MP3 encoder: {e:?}");
    let mut lame = Builder::new()
        .context("MP3 encoder")?
        .with_num_channels(pcm.ch as u8)
        .map_err(err)?
        .with_sample_rate(pcm.rate)
        .map_err(err)?
        .with_vbr_mode(VbrMode::Mtrh)
        .map_err(err)?
        // V0 or V2.
        .with_vbr_quality(if q == Quality::High { Lq::Best } else { Lq::NearBest })
        .map_err(err)?
        .with_quality(Lq::NearBest)
        .map_err(err)?
        .with_to_write_vbr_tag(true)
        .map_err(err)?
        .build()
        .map_err(err)?;
    let mut out = BufWriter::new(File::create(dst)?);
    let mut buf = Vec::new();
    while let Some(s) = pcm.next()? {
        buf.clear();
        buf.reserve(mp3lame_encoder::max_required_buffer_size(s.len() / pcm.ch));
        let r = if pcm.ch == 1 { lame.encode_to_vec(MonoPcm(&s[..]), &mut buf) } else { lame.encode_to_vec(InterleavedPcm(&s[..]), &mut buf) };
        r.map_err(|e| anyhow::anyhow!("MP3 encoder: {e:?}"))?;
        out.write_all(&buf)?;
    }
    buf.clear();
    buf.reserve(7200);
    lame.flush_to_vec::<FlushGap>(&mut buf).map_err(|e| anyhow::anyhow!("MP3 encoder: {e:?}"))?;
    out.write_all(&buf)?;
    // LAME starts the stream with an empty frame; the LAME tag (length,
    // encoder delay and padding, for gapless playback) goes in its place.
    let mut tag = Vec::with_capacity(lame.lame_tag_size().max(1));
    if lame.lame_tag_encode_to_vec(&mut tag).is_some() {
        out.seek(SeekFrom::Start(0))?;
        out.write_all(&tag)?;
    }
    out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    Ok(())
}

fn encode_vorbis(pcm: &mut Pcm, dst: &Path, q: Quality) -> Result<()> {
    use std::num::{NonZeroU32, NonZeroU8};
    use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoderBuilder};
    let sink = BufWriter::new(File::create(dst)?);
    let rate = NonZeroU32::new(pcm.rate).context("no sample rate")?;
    let ch = NonZeroU8::new(pcm.ch as u8).context("no channels")?;
    let mut enc = VorbisEncoderBuilder::new_with_serial(rate, ch, sink, rand::thread_rng().r#gen())
        .bitrate_management_strategy(VorbisBitrateManagementStrategy::QualityVbr {
            target_quality: if q == Quality::High { 0.7 } else { 0.5 },
        })
        .build()?;
    let mut planar = vec![Vec::new(); pcm.ch];
    while let Some(s) = pcm.next()? {
        for c in &mut planar {
            c.clear();
        }
        for f in s.chunks_exact(pcm.ch) {
            for (c, &v) in f.iter().enumerate() {
                planar[c].push(v);
            }
        }
        // libvorbis takes blocks of any size, but suggests about 1024 frames.
        for start in (0..planar[0].len()).step_by(1024) {
            let end = (start + 1024).min(planar[0].len());
            let block: Vec<&[f32]> = planar.iter().map(|c| &c[start..end]).collect();
            enc.encode_audio_block(&block)?;
        }
    }
    enc.finish()?.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    Ok(())
}

/// Quantize to 16 bits with TPDF dither, which turns the rounding error into
/// a faint, even hiss instead of distortion that follows the music.
fn to_i16(s: f32, rng: &mut impl Rng) -> i32 {
    let dither = (rng.r#gen::<f32>() - rng.r#gen::<f32>()) / 32768.0;
    ((s + dither) * 32767.0).round().clamp(-32768.0, 32767.0) as i32
}

/// Feeds flacenc as it asks for blocks, so only the encoded stream is held
/// in memory, not the whole decoded file.
struct FlacSource<'p, 'a> {
    pcm: &'p mut Pcm<'a>,
    pending: Vec<i32>,
    rng: rand::rngs::ThreadRng,
    error: Option<anyhow::Error>,
}

impl flacenc::source::Source for FlacSource<'_, '_> {
    fn channels(&self) -> usize {
        self.pcm.ch
    }
    fn bits_per_sample(&self) -> usize {
        16
    }
    fn sample_rate(&self) -> usize {
        self.pcm.rate as usize
    }
    fn read_samples<F: flacenc::source::Fill>(
        &mut self,
        block_size: usize,
        dest: &mut F,
    ) -> Result<usize, flacenc::error::SourceError> {
        let want = block_size * self.pcm.ch;
        while self.pending.len() < want {
            match self.pcm.next() {
                Ok(Some(s)) => {
                    let rng = &mut self.rng;
                    self.pending.extend(s.iter().map(|&v| to_i16(v, rng)));
                }
                Ok(None) => break,
                Err(e) => {
                    self.error = Some(e);
                    return Err(flacenc::error::SourceError::from_unknown());
                }
            }
        }
        let n = want.min(self.pending.len());
        dest.fill_interleaved(&self.pending[..n])?;
        self.pending.drain(..n);
        Ok(n / self.pcm.ch)
    }
}

fn encode_flac16(pcm: &mut Pcm, dst: &Path) -> Result<()> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|e| anyhow::anyhow!("FLAC encoder: {e:?}"))?;
    let mut source = FlacSource { pcm, pending: Vec::new(), rng: rand::thread_rng(), error: None };
    let stream = flacenc::encode_with_fixed_block_size(&config, &mut source, config.block_size);
    if let Some(e) = source.error {
        return Err(e);
    }
    let stream = stream.map_err(|e| anyhow::anyhow!("FLAC encoder: {e:?}"))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|e| anyhow::anyhow!("FLAC encoder: {e:?}"))?;
    // Two fixes to flacenc's STREAMINFO. It counts the short last frame in
    // the minimum block size, which makes decoders take a fixed-size stream
    // for a variable one and misread every frame header; the spec leaves the
    // last frame out. And it's written as the only (so last) metadata block:
    // tag writers append after it without clearing that flag, so a padding
    // block goes last instead, the way the reference encoder lays files out.
    let bytes = sink.as_slice();
    let info_end = 4 + 4 + 34; // "fLaC", block header, STREAMINFO
    anyhow::ensure!(bytes.len() > info_end && &bytes[..4] == b"fLaC", "FLAC encoder wrote no stream");
    let mut head = bytes[..info_end].to_vec();
    head[4] &= 0x7f;
    let max_block = [head[10], head[11]];
    head[8..10].copy_from_slice(&max_block);
    head.extend_from_slice(&[0x81, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0]); // last block: 8 bytes of padding
    let mut out = File::create(dst)?;
    out.write_all(&head)?;
    out.write_all(&bytes[info_end..])?;
    out.sync_all()?;
    Ok(())
}

/// AAC through Apple's encoder: the audio goes to a temporary WAV on this
/// computer (not the device being synced), and `afconvert` turns it into an .m4a.
fn encode_aac(pcm: &mut Pcm, dst: &Path, q: Quality) -> Result<()> {
    if !aac_available() {
        bail!("AAC converting needs macOS for now");
    }
    let wav = std::env::temp_dir().join(format!("honeycomb-convert-{:016x}.wav", rand::random::<u64>()));
    let result = (|| {
        write_float_wav(pcm, &wav)?;
        let out = std::process::Command::new("/usr/bin/afconvert")
            .args(["-f", "m4af", "-d", "aac", "-s", "3", "-q", "127", "-u", "vbrq"])
            // Apple's VBR quality scale: about 256 kbps at 109, 192 at 91.
            .arg(if q == Quality::High { "109" } else { "91" })
            .arg(&wav)
            .arg(dst)
            .output()
            .context("running afconvert")?;
        if !out.status.success() {
            bail!("afconvert: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&wav);
    result
}

/// A 32-bit float WAV, which Core Audio reads without losing anything.
fn write_float_wav(pcm: &mut Pcm, path: &Path) -> Result<()> {
    let mut out = BufWriter::new(File::create(path)?);
    let (ch, rate) = (pcm.ch as u16, pcm.rate);
    let header = |data_len: u32| {
        let mut b = Vec::with_capacity(44);
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
        b.extend_from_slice(&ch.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * ch as u32 * 4).to_le_bytes());
        b.extend_from_slice(&(ch * 4).to_le_bytes());
        b.extend_from_slice(&32u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        b
    };
    out.write_all(&header(0))?;
    let mut len: u64 = 0;
    while let Some(s) = pcm.next()? {
        for v in &s {
            out.write_all(&v.to_le_bytes())?;
        }
        len += s.len() as u64 * 4;
    }
    let len = u32::try_from(len).context("too long for a WAV file")?;
    out.seek(SeekFrom::Start(0))?;
    out.write_all(&header(len))?;
    out.flush()?;
    Ok(())
}

// ---------- tags ----------

/// Copy the source's tags and artwork. A file can carry several tags (WAV:
/// RIFF INFO and ID3v2); they're merged, the primary one winning, the same
/// way the library reads them.
fn copy_tags(src: &Path, dst: &Path, format: Format) -> Result<()> {
    let tagged = lofty::read_from_path(src)?;
    let mut tags: Vec<&Tag> = tagged.tags().iter().collect();
    let primary = tagged.primary_tag_type();
    tags.sort_by_key(|t| t.tag_type() != primary);
    let Some(first) = tags.first() else { return Ok(()) };
    let mut tag = (*first).clone();
    for other in &tags[1..] {
        for item in other.items() {
            if tag.get(item.key()).is_none() {
                tag.insert(item.clone());
            }
        }
        if tag.pictures().is_empty() {
            for p in other.pictures() {
                tag.push_picture(p.clone());
            }
        }
    }
    tag.re_map(match format {
        Format::Mp3 => TagType::Id3v2,
        Format::Aac => TagType::Mp4Ilst,
        Format::Vorbis | Format::Flac16 => TagType::VorbisComments,
    });
    let mut file = std::fs::OpenOptions::new().read(true).write(true).open(dst)?;
    if format == Format::Aac {
        // Apple's encoder records its priming and padding in the file's tags
        // (iTunSMPB), which is how iPods play AAC gaplessly. Keep whatever it
        // wrote and add the copied tags to it.
        use lofty::file::AudioFile;
        use lofty::mp4::{AtomIdent, Ilst};
        let smpb = AtomIdent::Freeform { mean: "com.apple.iTunes".into(), name: "iTunSMPB".into() };
        let kept = lofty::mp4::Mp4File::read_from(&mut file, lofty::config::ParseOptions::new())?
            .ilst()
            .and_then(|ilst| ilst.get(&smpb).cloned());
        let mut ilst = Ilst::from(tag);
        if let Some(atom) = kept {
            ilst.insert(atom);
        }
        file.seek(SeekFrom::Start(0))?;
        ilst.save_to(&mut file, WriteOptions::default())?;
        return Ok(());
    }
    tag.save_to(&mut file, WriteOptions::default())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::tag::Accessor;
    use std::path::PathBuf;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hc-convert-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A 24-bit 96 kHz stereo WAV with a 440 Hz tone, a title and a cover.
    fn source(d: &Path, frames: u32) -> PathBuf {
        let path = d.join("tone.wav");
        let data_len = frames * 6;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&96_000u32.to_le_bytes());
        b.extend_from_slice(&(96_000u32 * 6).to_le_bytes());
        b.extend_from_slice(&6u16.to_le_bytes());
        b.extend_from_slice(&24u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..frames {
            let v = ((i as f64 * 440.0 * std::f64::consts::TAU / 96_000.0).sin() * 0.5 * 8_388_607.0) as i32;
            let bytes = &v.to_le_bytes()[..3];
            b.extend_from_slice(bytes);
            b.extend_from_slice(bytes);
        }
        std::fs::write(&path, b).unwrap();
        let mut tag = Tag::new(TagType::Id3v2);
        tag.insert_text(lofty::tag::ItemKey::TrackTitle, "Tone".into());
        tag.push_picture(lofty::picture::Picture::new_unchecked(
            lofty::picture::PictureType::CoverFront,
            Some(lofty::picture::MimeType::Png),
            None,
            vec![0x89, b'P', b'N', b'G', 0, 0, 0, 0],
        ));
        tag.save_to_path(&path, WriteOptions::default()).unwrap();
        path
    }

    /// Decode a file the way Kaseki would: (frames, channels, rate, bits).
    fn decoded(path: &Path) -> (u64, usize, u32, Option<u32>) {
        let mut src = Source::open(&path.to_string_lossy()).unwrap();
        let (mut frames, mut ch, mut rate) = (0u64, 0, 0);
        while let Some((s, c, r)) = src.decode_next() {
            frames += (s.len() / c) as u64;
            (ch, rate) = (c, r);
        }
        (frames, ch, rate, kaseki::probe(&path.to_string_lossy()).and_then(|p| p.bits))
    }

    /// Convert the test tone and check the result. `extra` is how many frames
    /// longer it may decode: only for AAC, whose gapless information (iTunSMPB)
    /// players read but symphonia doesn't.
    fn check(format: Format, ext: &str, extra: u64) -> PathBuf {
        let d = dir(ext);
        // 2.5 s at 96 kHz, which isn't a multiple of 44.1, so it comes out at 48 kHz.
        let src = source(&d, 240_000);
        let dst = d.join(format!("out.{ext}"));
        let c = Convert { format, quality: Quality::High };
        let size = convert(&src, &dst, c, &AtomicBool::new(false)).unwrap();
        assert!(size > 0);
        let (frames, ch, rate, _) = decoded(&dst);
        assert_eq!((ch, rate), (2, 48_000));
        assert!((120_000..=120_000 + extra).contains(&frames), "{format:?} came out {frames} frames long, not 120000");
        let tagged = lofty::read_from_path(&dst).unwrap();
        let tag = tagged.primary_tag().expect("tags copied");
        assert_eq!(tag.title().as_deref(), Some("Tone"));
        assert_eq!(tag.pictures().len(), 1, "artwork copied");
        dst
    }

    #[test]
    fn mp3_is_gapless_and_tagged() {
        check(Format::Mp3, "mp3", 0);
    }

    #[test]
    fn vorbis_is_gapless_and_tagged() {
        check(Format::Vorbis, "ogg", 0);
    }

    #[test]
    fn flac16_is_16_bit_and_tagged() {
        let dst = check(Format::Flac16, "flac", 0);
        assert_eq!(decoded(&dst).3, Some(16));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn aac_is_gapless_and_tagged() {
        // 2112 frames of encoder priming, plus padding to a whole 1024-frame packet.
        let dst = check(Format::Aac, "m4a", 2112 + 1024);
        let info = std::process::Command::new("afinfo").arg(&dst).output().unwrap();
        let info = String::from_utf8_lossy(&info.stdout);
        assert!(info.contains("120000 valid frames + 2112 priming"), "gapless info kept: {info}");
    }

    #[test]
    fn cancelling_stops() {
        let d = dir("cancel");
        let src = source(&d, 96_000);
        let c = Convert { format: Format::Mp3, quality: Quality::Smaller };
        assert!(convert(&src, &d.join("x.mp3"), c, &AtomicBool::new(true)).is_err());
    }

    #[test]
    fn what_gets_converted() {
        let mp3 = Convert { format: Format::Mp3, quality: Quality::High };
        let flac = Convert { format: Format::Flac16, quality: Quality::High };
        assert!(!mp3.applies(None, Some(44_100)), "lossy files are copied");
        assert!(mp3.applies(Some(16), Some(44_100)));
        assert!(!flac.applies(Some(16), Some(44_100)), "already CD quality");
        assert!(flac.applies(Some(24), Some(44_100)));
        assert!(flac.applies(Some(16), Some(96_000)));
        assert_eq!(target_rate(44_100), 44_100);
        assert_eq!(target_rate(88_200), 44_100);
        assert_eq!(target_rate(96_000), 48_000);
        assert_eq!(target_rate(192_000), 48_000);
    }
}
