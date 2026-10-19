//! Reading and writing audio file tags and artwork.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::Tag;

use crate::models::{TagEdit, Track, now_secs};

/// Extensions we can both read tags from and decode for playback.
pub const AUDIO_EXTS: &[&str] = &["mp3", "flac", "m4a", "mp4", "aac", "ogg", "oga", "wav", "aif", "aiff"];

const FOLDER_ART: &[&str] = &["cover", "folder", "front", "album"];
const ART_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp"];

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn file_mtime(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Read a file's tags and audio properties into a `Track` (id is left at 0).
pub fn read_track(path: &Path) -> Result<Track> {
    read_track_with(path, &|p| folder_art(p).is_some())
}

/// Like `read_track`, with a custom (e.g. cached) check for cover images in the folder.
pub fn read_track_with(path: &Path, has_folder_art: &dyn Fn(&Path) -> bool) -> Result<Track> {
    let tagged = lofty::read_from_path(path).with_context(|| format!("reading {}", path.display()))?;
    let props = tagged.properties();
    // Files can carry several tags (e.g. RIFF INFO + ID3v2 in WAV). Prefer the
    // primary tag, but fill in anything it lacks from the others.
    let tags: Vec<&Tag> = tagged
        .primary_tag()
        .into_iter()
        .chain(tagged.tags().iter().filter(|t| Some(t.tag_type()) != tagged.primary_tag().map(|p| p.tag_type())))
        .collect();
    let first = |f: &dyn Fn(&Tag) -> Option<u32>| tags.iter().find_map(|t| f(t));

    let text = |key: ItemKey| -> String {
        tags.iter()
            .filter_map(|t| t.get_string(&key))
            .map(str::trim)
            .find(|s| !s.is_empty())
            .unwrap_or_default()
            .to_string()
    };

    let mut title = text(ItemKey::TrackTitle);
    if title.is_empty() {
        title = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
    }

    let has_tag_art = tags.iter().any(|t| !t.pictures().is_empty());

    Ok(Track {
        id: 0,
        path: path.to_string_lossy().into_owned(),
        title,
        artist: text(ItemKey::TrackArtist),
        album_artist: text(ItemKey::AlbumArtist),
        album: text(ItemKey::AlbumTitle),
        genre: text(ItemKey::Genre),
        composer: text(ItemKey::Composer),
        comment: text(ItemKey::Comment),
        year: first(&|t| t.year()),
        track_no: first(&|t| t.track()),
        disc_no: first(&|t| t.disk()),
        duration: props.duration().as_secs_f64(),
        bitrate: props.audio_bitrate(),
        sample_rate: props.sample_rate(),
        format: path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_uppercase(),
        rating: 0,
        play_count: 0,
        last_played: None,
        date_added: now_secs(),
        mtime: file_mtime(path),
        has_art: has_tag_art || has_folder_art(path),
        loved: false,
        inbox: false,
    })
}

/// Look for cover.jpg / folder.png etc. next to the file.
pub fn folder_art(path: &Path) -> Option<std::path::PathBuf> {
    let dir = path.parent()?;
    for name in FOLDER_ART {
        for ext in ART_EXTS {
            let p = dir.join(format!("{name}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn mime_for(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        [b'G', b'I', b'F', ..] => "image/gif",
        _ => "image/jpeg",
    }
}

/// Embedded front cover (or any embedded picture), falling back to folder art.
pub fn artwork(path: &Path) -> Option<(Vec<u8>, &'static str)> {
    if let Ok(tagged) = lofty::read_from_path(path) {
        for tag in tagged.tags() {
            let pics = tag.pictures();
            let pic = pics
                .iter()
                .find(|p| p.pic_type() == PictureType::CoverFront)
                .or_else(|| pics.first());
            if let Some(p) = pic {
                let data = p.data().to_vec();
                let mime = mime_for(&data);
                return Some((data, mime));
            }
        }
    }
    let p = folder_art(path)?;
    let data = fs::read(p).ok()?;
    let mime = mime_for(&data);
    Some((data, mime))
}

fn parse_num(s: &str) -> Option<u32> {
    // Accept "3", "03", "3/12" and "2004-05-01".
    s.trim()
        .split(['/', '-'])
        .next()
        .and_then(|n| n.trim().parse().ok())
}

/// Apply an edit to a file's primary tag, creating the tag if the file has none.
pub fn write_tags(path: &Path, edit: &TagEdit) -> Result<()> {
    let mut tagged = lofty::read_from_path(path).with_context(|| format!("reading {}", path.display()))?;
    if tagged.primary_tag().is_none() {
        // Seed the new primary tag from whatever tag the file already has, so
        // existing metadata isn't hidden behind a nearly empty tag.
        let mut tag = Tag::new(tagged.primary_tag_type());
        if let Some(existing) = tagged.first_tag() {
            for item in existing.items() {
                tag.insert(item.clone());
            }
            for pic in existing.pictures() {
                tag.push_picture(pic.clone());
            }
        }
        tagged.insert_tag(tag);
    }
    let tag = tagged.primary_tag_mut().context("file has no writable tag")?;

    let set_text = |tag: &mut Tag, key: ItemKey, v: &Option<String>| {
        if let Some(v) = v {
            let v = v.trim();
            if v.is_empty() {
                tag.remove_key(&key);
            } else {
                tag.insert_text(key, v.to_string());
            }
        }
    };
    set_text(tag, ItemKey::TrackTitle, &edit.title);
    set_text(tag, ItemKey::TrackArtist, &edit.artist);
    set_text(tag, ItemKey::AlbumArtist, &edit.album_artist);
    set_text(tag, ItemKey::AlbumTitle, &edit.album);
    set_text(tag, ItemKey::Genre, &edit.genre);
    set_text(tag, ItemKey::Composer, &edit.composer);
    set_text(tag, ItemKey::Comment, &edit.comment);

    if let Some(y) = &edit.year {
        match parse_num(y) {
            Some(n) => tag.set_year(n),
            None => tag.remove_year(),
        }
    }
    if let Some(n) = &edit.track_no {
        match parse_num(n) {
            Some(n) => tag.set_track(n),
            None => tag.remove_track(),
        }
    }
    if let Some(n) = &edit.disc_no {
        match parse_num(n) {
            Some(n) => tag.set_disk(n),
            None => tag.remove_disk(),
        }
    }

    if edit.remove_art || edit.art_path.is_some() {
        tag.remove_picture_type(PictureType::CoverFront);
    }
    if let Some(art) = &edit.art_path {
        let data = fs::read(art).with_context(|| format!("reading image {art}"))?;
        let mime = match mime_for(&data) {
            "image/png" => MimeType::Png,
            "image/gif" => MimeType::Gif,
            "image/webp" => MimeType::Unknown("image/webp".into()),
            _ => MimeType::Jpeg,
        };
        tag.push_picture(Picture::new_unchecked(PictureType::CoverFront, Some(mime), None, data));
    }

    tagged
        .save_to_path(path, WriteOptions::default())
        .with_context(|| format!("saving {}", path.display()))?;
    Ok(())
}

/// Lyrics stored in the file's tags (USLT, LYRICS, ©lyr…), if any.
pub fn embedded_lyrics(path: &Path) -> Option<String> {
    let tagged = lofty::read_from_path(path).ok()?;
    tagged
        .primary_tag()
        .into_iter()
        .chain(tagged.tags().iter())
        .filter_map(|t| t.get_string(&ItemKey::Lyrics))
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(str::to_string)
}
