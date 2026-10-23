//! Photo, video and music details, for the preview, batch rename's {taken}, {camera}, {artist}…, and the
//! Date taken sort: a picture's camera, lens, exposure, when and where it was taken (its EXIF, through
//! nom-exif: JPEG, HEIC, AVIF, PNG, TIFF and the RAW formats), a video's date, length and place (its
//! track's metadata), a song's tags and cover (lofty: MP3, FLAC, M4A, Ogg, Opus, WAV, AIFF…).

use crate::fsx::blocking;
use base64::Engine;
use chrono::{Local, NaiveDateTime, TimeZone};
use lofty::prelude::*;
use nom_exif::{EntryValue, ExifDateTime, ExifTag, MediaKind, MediaParser, MediaSource, TrackInfoTag};
use serde::Serialize;
use std::path::Path;

const PICTURES: &[&str] = &["jpg", "jpeg", "jpe", "jfif", "heic", "heif", "avif", "png", "tif", "tiff", "webp", "dng", "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "pef", "iiq", "srw"];
const VIDEOS: &[&str] = &["mp4", "m4v", "mov", "3gp", "3g2", "mkv", "webm"];
const SONGS: &[&str] = &["mp3", "flac", "m4a", "m4b", "ogg", "oga", "opus", "wav", "aif", "aiff", "aifc", "ape", "wv", "mpc", "spx", "aac"];
/// A cover bigger than this isn't sent to the preview.
const MAX_COVER: usize = 1 << 20;

/// What's worth knowing about a photo, video or song.
#[derive(Serialize, Default, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaDetails {
    /// Label and value, in the order the preview shows them.
    pub rows: Vec<[String; 2]>,
    /// When it was taken (or recorded), as milliseconds since 1970: the local time it says.
    pub taken: Option<f64>,
    pub camera: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
    pub track: Option<u32>,
    /// Latitude and longitude, where it was taken.
    pub place: Option<[f64; 2]>,
    /// A song's cover, as a data: URL.
    pub cover: Option<String>,
}

fn kind_of(path: &str) -> Option<&'static str> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    if PICTURES.contains(&ext.as_str()) {
        Some("picture")
    } else if VIDEOS.contains(&ext.as_str()) {
        Some("video")
    } else if SONGS.contains(&ext.as_str()) {
        Some("song")
    } else {
        None
    }
}

/// A photo's, video's or song's details; None for other files, or when it says nothing.
#[tauri::command]
pub async fn media_details(path: String) -> Result<Option<MediaDetails>, String> {
    blocking(move || Ok(details(&path, true))).await
}

/// Many files' details at once, without covers (batch rename, the Date taken sort), four at a time.
#[tauri::command]
pub async fn media_details_many(paths: Vec<String>) -> Result<Vec<Option<MediaDetails>>, String> {
    blocking(move || {
        let chunk = paths.len().div_ceil(4).max(1);
        Ok(std::thread::scope(|scope| {
            let parts: Vec<_> = paths.chunks(chunk).map(|part| scope.spawn(move || part.iter().map(|p| details(p, false)).collect::<Vec<_>>())).collect();
            parts.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        }))
    })
    .await
}

fn details(path: &str, cover: bool) -> Option<MediaDetails> {
    let found = match kind_of(path)? {
        "song" => song(path, cover),
        _ => picture_or_video(path),
    }?;
    (found != MediaDetails::default()).then_some(found)
}

/// A naive date's milliseconds, read as this PC's local time (a camera writes its own clock's time).
fn local_ms(naive: &NaiveDateTime) -> Option<f64> {
    Local.from_local_datetime(naive).earliest().map(|t| t.timestamp_millis() as f64)
}

fn when(value: &ExifDateTime) -> (String, Option<f64>) {
    match value {
        ExifDateTime::Aware(t) => (format!("{} {}", t.format("%Y-%m-%d %H:%M:%S"), t.offset()), Some(t.timestamp_millis() as f64)),
        ExifDateTime::Naive(t) => (t.format("%Y-%m-%d %H:%M:%S").to_string(), local_ms(t)),
    }
}

fn date_of(value: &EntryValue) -> Option<(String, Option<f64>)> {
    match value {
        EntryValue::DateTime(t) => Some(when(&ExifDateTime::Aware(*t))),
        EntryValue::NaiveDateTime(t) => Some(when(&ExifDateTime::Naive(*t))),
        other => other.as_datetime().map(|d| when(&d)),
    }
}

fn text(value: Option<&EntryValue>) -> Option<String> {
    let text = value?.as_str()?.trim().trim_end_matches('\0').trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// "Canon EOS R5", not "Canon Canon EOS R5": a model that names its maker goes on its own.
fn camera_name(make: Option<String>, model: Option<String>) -> Option<String> {
    match (make, model) {
        (Some(make), Some(model)) if model.to_lowercase().starts_with(&make.split_whitespace().next().unwrap_or("").to_lowercase()) => Some(model),
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (make, model) => model.or(make),
    }
}

/// An exposure as a camera says it: "1/250 s", "2.5 s".
fn exposure(seconds: f64) -> String {
    if seconds >= 0.3 {
        format!("{} s", (seconds * 10.0).round() / 10.0)
    } else {
        format!("1/{} s", (1.0 / seconds).round())
    }
}

fn picture_or_video(path: &str) -> Option<MediaDetails> {
    let source = MediaSource::open(path).ok()?;
    let mut parser = MediaParser::new();
    let mut out = MediaDetails::default();
    match source.kind() {
        MediaKind::Image => {
            let exif: nom_exif::Exif = parser.parse_exif(source).ok()?.into();
            let get = |tag| exif.get(tag);
            if let Some((shown, ms)) = [ExifTag::DateTimeOriginal, ExifTag::CreateDate].into_iter().find_map(|t| get(t).and_then(date_of)) {
                out.rows.push(["Taken".into(), shown]);
                out.taken = ms;
            }
            out.camera = camera_name(text(get(ExifTag::Make)), text(get(ExifTag::Model)));
            if let Some(camera) = &out.camera {
                out.rows.push(["Camera".into(), camera.clone()]);
            }
            if let Some(lens) = text(get(ExifTag::LensModel)) {
                out.rows.push(["Lens".into(), lens]);
            }
            let rational = |tag| get(tag).and_then(|v| v.as_urational()).and_then(|r| r.to_f64()).filter(|f| *f > 0.0);
            let mut settings = Vec::new();
            if let Some(t) = rational(ExifTag::ExposureTime) {
                settings.push(exposure(t));
            }
            if let Some(f) = rational(ExifTag::FNumber) {
                settings.push(format!("f/{}", (f * 10.0).round() / 10.0));
            }
            if let Some(iso) = get(ExifTag::ISOSpeedRatings).and_then(|v| v.try_as_integer()).filter(|i| *i > 0) {
                settings.push(format!("ISO {iso}"));
            }
            if let Some(mm) = rational(ExifTag::FocalLength) {
                let full = get(ExifTag::FocalLengthIn35mmFilm).and_then(|v| v.try_as_integer()).filter(|i| *i > 0 && (*i as f64 - mm).abs() > 1.0);
                settings.push(match full {
                    Some(full) => format!("{} mm ({full} mm full-frame)", mm.round()),
                    None => format!("{} mm", mm.round()),
                });
            }
            if !settings.is_empty() {
                out.rows.push(["Exposure".into(), settings.join(" · ")]);
            }
            if let Some(g) = exif.gps_info() {
                if let (Some(lat), Some(lon)) = (g.latitude_decimal(), g.longitude_decimal()) {
                    if lat != 0.0 || lon != 0.0 {
                        out.place = Some([lat, lon]);
                    }
                }
            }
        }
        MediaKind::Track => {
            let track = parser.parse_track(source).ok()?;
            if let Some((shown, ms)) = track.get(TrackInfoTag::CreateDate).and_then(date_of) {
                // An unset date reads as 1904 (QuickTime's zero).
                if !shown.starts_with("1904") && !shown.starts_with("1970") {
                    out.rows.push(["Recorded".into(), shown]);
                    out.taken = ms;
                }
            }
            if let Some(ms) = track.get(TrackInfoTag::DurationMs).and_then(|v| v.try_as_integer()).filter(|ms| *ms > 0) {
                out.rows.push(["Length".into(), duration(ms as f64 / 1000.0)]);
            }
            out.camera = camera_name(text(track.get(TrackInfoTag::Make)), text(track.get(TrackInfoTag::Model)));
            if let Some(camera) = &out.camera {
                out.rows.push(["Camera".into(), camera.clone()]);
            }
            if let Some(g) = track.gps_info() {
                if let (Some(lat), Some(lon)) = (g.latitude_decimal(), g.longitude_decimal()) {
                    out.place = Some([lat, lon]);
                }
            }
        }
    }
    if let Some([lat, lon]) = out.place {
        out.rows.push(["Place".into(), format!("{lat:.5}, {lon:.5}")]);
    }
    Some(out)
}

/// 3:07, or 1:02:03 past an hour.
fn duration(seconds: f64) -> String {
    let s = seconds.round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn song(path: &str, cover: bool) -> Option<MediaDetails> {
    let file = lofty::read_from_path(path).ok()?;
    let mut out = MediaDetails::default();
    let tag = file.primary_tag().or_else(|| file.first_tag());
    let clean = |s: Option<std::borrow::Cow<'_, str>>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(tag) = tag {
        out.title = clean(tag.title());
        out.artist = clean(tag.artist());
        out.album = clean(tag.album());
        out.track = tag.track();
        for (label, value) in [("Title", &out.title), ("Artist", &out.artist)] {
            if let Some(value) = value {
                out.rows.push([label.into(), value.clone()]);
            }
        }
        if let Some(album) = &out.album {
            let year = tag.date().map(|d| format!(" ({})", d.year)).unwrap_or_default();
            out.rows.push(["Album".into(), format!("{album}{year}")]);
        }
        if let Some(n) = out.track {
            out.rows.push(["Track".into(), match tag.track_total() {
                Some(total) => format!("{n} of {total}"),
                None => n.to_string(),
            }]);
        }
        if let Some(genre) = clean(tag.genre()) {
            out.rows.push(["Genre".into(), genre]);
        }
        if cover {
            if let Some(picture) = tag.pictures().iter().find(|p| p.data().len() <= MAX_COVER) {
                let mime = picture.mime_type().map(|m| m.as_str().to_string()).unwrap_or_else(|| "image/jpeg".into());
                if mime.starts_with("image/") {
                    out.cover = Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(picture.data())));
                }
            }
        }
    }
    let props = file.properties();
    if props.duration().as_secs_f64() > 0.0 {
        out.rows.push(["Length".into(), duration(props.duration().as_secs_f64())]);
    }
    let quality: Vec<String> = [
        props.audio_bitrate().filter(|b| *b > 0).map(|b| format!("{b} kbps")),
        props.sample_rate().filter(|r| *r > 0).map(|r| format!("{} kHz", r as f64 / 1000.0)),
        props.bit_depth().filter(|b| *b > 0).map(|b| format!("{b}-bit")),
        props.channels().map(|c| match c {
            1 => "mono".to_string(),
            2 => "stereo".to_string(),
            n => format!("{n} channels"),
        }),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !quality.is_empty() {
        out.rows.push(["Quality".into(), quality.join(" · ")]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, FixedOffset};

    /// A date with its offset, as the preview says it (a fixed offset converts without the PC's zone).
    fn aware(t: DateTime<FixedOffset>) -> (String, Option<f64>) {
        when(&ExifDateTime::Aware(t))
    }

    #[test]
    fn says_cameras_exposures_and_lengths_as_photographers_do() {
        assert_eq!(camera_name(Some("Canon".into()), Some("Canon EOS R5".into())).as_deref(), Some("Canon EOS R5"));
        assert_eq!(camera_name(Some("NIKON CORPORATION".into()), Some("NIKON Z 6".into())).as_deref(), Some("NIKON Z 6"));
        assert_eq!(camera_name(Some("Google".into()), Some("Pixel 8".into())).as_deref(), Some("Google Pixel 8"));
        assert_eq!(camera_name(None, Some("iPhone 15".into())).as_deref(), Some("iPhone 15"));
        assert_eq!(exposure(0.004), "1/250 s");
        assert_eq!(exposure(2.5), "2.5 s");
        assert_eq!(duration(187.4), "3:07");
        assert_eq!(duration(3723.0), "1:02:03");
        let t = FixedOffset::east_opt(7200).unwrap().with_ymd_and_hms(2026, 1, 2, 8, 30, 15).unwrap();
        assert_eq!(aware(t), ("2026-01-02 08:30:15 +02:00".to_string(), Some(t.timestamp_millis() as f64)));
        assert_eq!(kind_of("C:\\a\\IMG_1.HEIC"), Some("picture"));
        assert_eq!(kind_of("C:\\a\\song.flac"), Some("song"));
        assert_eq!(kind_of("C:\\a\\notes.txt"), None);
    }

    #[test]
    fn reads_nothing_from_what_isnt_media() {
        let dir = std::env::temp_dir().join(format!("glint-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes) in [("broken.jpg", &b"\xFF\xD8\xFF\xE1\x00\x10Exif\x00\x00garbage"[..]), ("empty.mp3", &b""[..]), ("text.mp4", &b"not a video"[..])] {
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(details(path.to_str().unwrap(), true), None, "{name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
