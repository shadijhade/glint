//! Photo, video and music details, for the preview, batch rename's {taken}, {camera}, {artist}…, the
//! Date taken sort and property filters (width:, taken:, artist:…): a picture's size, camera, lens,
//! exposure, when and where it was taken (its EXIF, through nom-exif: JPEG, HEIC, AVIF, PNG, TIFF and the
//! RAW formats; its header for the size otherwise), a video's size, date, length and place (its track's
//! metadata), a song's tags, length and cover (lofty: MP3, FLAC, M4A, Ogg, Opus, WAV, AIFF…).

use crate::fsx::blocking;
use base64::Engine;
use chrono::{Local, NaiveDateTime, TimeZone};
use lofty::prelude::*;
use nom_exif::{EntryValue, ExifDateTime, ExifTag, MediaKind, MediaParser, MediaSource, TrackInfoTag};
use serde::Serialize;
use shadui_index::query::{Prop, Taken};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

const PICTURES: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "heic", "heif", "avif", "png", "tif", "tiff", "webp", "dng", "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "pef", "iiq", "srw",
    // Without EXIF: only their size, from their header.
    "gif", "bmp", "ico", "jxl", "psd", "qoi",
];
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
    /// A picture's or video's size in pixels, as it shows (a photo turned by its EXIF is turned here).
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// A video's or song's length, in seconds.
    pub duration: Option<f64>,
    /// The photo's EXIF turns it a quarter (orientations 5 to 8): its width and height swap.
    #[serde(skip)]
    turned: bool,
}

/// Camera RAW files: their header is a TIFF whose first image is often a small preview, so their size
/// is the EXIF's.
const RAW: &[&str] = &["dng", "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "pef", "iiq", "srw"];

/// When a photo or a video was taken (or recorded), as ms since 1970: None when it doesn't say.
pub(crate) fn taken_of(path: &str) -> Option<i64> {
    details(path, false)?.taken.map(|ms| ms as i64)
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
    let kind = kind_of(path)?;
    let mut found = match kind {
        "song" => song(path, cover),
        _ => picture_or_video(path),
    };
    // A picture's size from its header (EXIF sizes go stale when a photo is resized), but a RAW file's
    // from its EXIF; turned as the photo shows.
    let raw = Path::new(path).extension().and_then(|e| e.to_str()).is_some_and(|e| RAW.contains(&e.to_ascii_lowercase().as_str()));
    if kind == "picture" && (!raw || found.as_ref().is_none_or(|f| f.width.is_none())) {
        if let Ok(size) = imagesize::size(path) {
            let found = found.get_or_insert_with(MediaDetails::default);
            let (w, h) = (u32::try_from(size.width).ok(), u32::try_from(size.height).ok());
            (found.width, found.height) = if found.turned { (h, w) } else { (w, h) };
        }
    }
    let found = found?;
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
            // Its size as its EXIF says it (the header's is preferred, in `details`), turned as it shows.
            out.turned = get(ExifTag::Orientation).and_then(|v| v.try_as_integer()).is_some_and(|o| (5..=8).contains(&o));
            let pixels = |tags: [ExifTag; 2]| tags.into_iter().find_map(|t| get(t).and_then(|v| v.try_as_integer()).and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0));
            if let (Some(w), Some(h)) = (pixels([ExifTag::ExifImageWidth, ExifTag::ImageWidth]), pixels([ExifTag::ExifImageHeight, ExifTag::ImageHeight])) {
                (out.width, out.height) = if out.turned { (Some(h), Some(w)) } else { (Some(w), Some(h)) };
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
                out.duration = Some(ms as f64 / 1000.0);
                out.rows.push(["Length".into(), duration(ms as f64 / 1000.0)]);
            }
            let pixels = |tag| track.get(tag).and_then(|v| v.try_as_integer()).and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0);
            if let (Some(w), Some(h)) = (pixels(TrackInfoTag::Width), pixels(TrackInfoTag::Height)) {
                (out.width, out.height) = (Some(w), Some(h));
                out.rows.push(["Dimensions".into(), format!("{w} × {h}")]);
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
        out.duration = Some(props.duration().as_secs_f64());
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

// Property filters: width:, height:, duration:, taken:, camera:, artist: and album: (shadui_index::query::Prop).

/// The extensions of the files that can satisfy all of `props`: photos and videos have a size, a date
/// taken and a camera; songs an artist and an album; videos and songs a length. Empty when none can.
pub(crate) fn exts_for(props: &[Prop]) -> Vec<&'static str> {
    const PICTURE: u8 = 1;
    const VIDEO: u8 = 2;
    const SONG: u8 = 4;
    let kinds = props.iter().fold(PICTURE | VIDEO | SONG, |kinds, prop| {
        kinds
            & match prop {
                Prop::Width(_) | Prop::Height(_) | Prop::Taken(_) | Prop::Camera(_) => PICTURE | VIDEO,
                Prop::Artist(_) | Prop::Album(_) => SONG,
                Prop::Duration(_) => VIDEO | SONG,
            }
    });
    [(PICTURE, PICTURES), (VIDEO, VIDEOS), (SONG, SONGS)].into_iter().filter(|(kind, _)| kinds & kind != 0).flat_map(|(_, exts)| exts.iter().copied()).collect()
}

/// What property filters look at, of one version of a file.
#[derive(Default)]
struct Facts {
    width: Option<u32>,
    height: Option<u32>,
    duration: Option<f64>,
    taken: Option<f64>,
    /// Lowercase.
    camera: Option<String>,
    artist: Option<String>,
    album: Option<String>,
}

/// Files read for property filters this session, by path, size and date: a search narrowed again reads
/// none twice. Emptied past this many.
type Read = HashMap<(String, u64, i64), Option<Arc<Facts>>>;
static FACTS: Mutex<Option<Read>> = Mutex::new(None);
const MAX_FACTS: usize = 200_000;

fn facts(path: &str, size: u64, modified: i64) -> Option<Arc<Facts>> {
    let key = (path.to_lowercase(), size, modified);
    if let Some(known) = FACTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|cache| cache.get(&key)) {
        return known.clone();
    }
    let lower = |s: Option<String>| s.map(|s| s.to_lowercase());
    let found = details(path, false).map(|d| Arc::new(Facts { width: d.width, height: d.height, duration: d.duration, taken: d.taken, camera: lower(d.camera), artist: lower(d.artist), album: lower(d.album) }));
    let mut cache = FACTS.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    if cache.len() >= MAX_FACTS {
        cache.clear();
    }
    cache.insert(key, found.clone());
    found
}

/// Midnight of a day, here, in ms.
fn midnight((year, month, day): shadui_index::query::Day) -> Option<f64> {
    Local.with_ymd_and_hms(year, month, day, 0, 0, 0).earliest().map(|t| t.timestamp_millis() as f64)
}

fn holds(prop: &Prop, facts: &Facts, now: f64) -> bool {
    let text = |value: &Option<String>, wanted: &String| value.as_ref().is_some_and(|v| v.contains(wanted.as_str()));
    match prop {
        Prop::Width(range) => facts.width.is_some_and(|w| range.holds(w.into())),
        Prop::Height(range) => facts.height.is_some_and(|h| range.holds(h.into())),
        Prop::Duration(range) => facts.duration.is_some_and(|s| range.holds(s as u64)),
        Prop::Camera(wanted) => text(&facts.camera, wanted),
        Prop::Artist(wanted) => text(&facts.artist, wanted),
        Prop::Album(wanted) => text(&facts.album, wanted),
        Prop::Taken(taken) => facts.taken.is_some_and(|ms| match *taken {
            Taken::Within(secs) => ms >= now - secs as f64 * 1000.0,
            Taken::Before(secs) => ms < now - secs as f64 * 1000.0,
            Taken::Days { from, to } => from.is_none_or(|day| midnight(day).is_some_and(|m| ms >= m)) && to.is_none_or(|day| midnight(day).is_some_and(|m| ms < m)),
        }),
    }
}

/// Whether a file (this version of it) says all `props` hold: read once, then from memory.
pub(crate) fn matches(path: &str, size: u64, modified: i64, props: &[Prop]) -> bool {
    let now = Local::now().timestamp_millis() as f64;
    facts(path, size, modified).is_some_and(|facts| props.iter().all(|p| holds(p, &facts, now)))
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
    fn property_filters_hold_as_they_read() {
        let props = |query: &str| shadui_index::query::parse(query).unwrap().1.props;
        let photo = Facts { width: Some(4032), height: Some(3024), taken: midnight((2024, 6, 15)).map(|m| m + 3_600_000.0), camera: Some("canon eos r5".into()), ..Default::default() };
        let song = Facts { duration: Some(187.4), artist: Some("the beatles".into()), album: Some("abbey road".into()), ..Default::default() };
        let now = midnight((2026, 10, 23)).unwrap();
        let all = |facts: &Facts, query: &str| props(query).iter().all(|p| holds(p, facts, now));
        assert!(all(&photo, "width:>4000 height:>=3024 camera:eos"));
        assert!(!all(&photo, "width:>4032"));
        assert!(all(&photo, "taken:2024 taken:2024-06 taken:2024-06-15 taken:>2023 taken:<2025-01 taken:>1y"));
        assert!(!all(&photo, "taken:2024-06-16") && !all(&photo, "taken:>2024") && !all(&photo, "taken:30d"));
        assert!(all(&song, "duration:>3m duration:<=3:07 artist:beatles album:\"abbey road\""));
        assert!(!all(&song, "duration:>3:07") && !all(&song, "width:>1") && !all(&photo, "artist:"));
        assert!(all(&song, "album:"), "a word left out: any album");

        let exts = |query: &str| exts_for(&props(query));
        assert!(exts("width:>1920").contains(&"png") && exts("width:>1920").contains(&"mp4") && !exts("width:>1920").contains(&"mp3"));
        assert!(exts("duration:>5m").contains(&"mp3") && exts("duration:>5m").contains(&"mkv") && !exts("duration:>5m").contains(&"jpg"));
        assert!(exts("width:>1 artist:x").is_empty(), "nothing has both");
    }

    #[test]
    fn reads_sizes_from_headers() {
        let dir = std::env::temp_dir().join(format!("glint-media-size-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A 3 × 2 PNG, no EXIF.
        let png = dir.join("small.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        bytes.extend([0, 0, 0, 3, 0, 0, 0, 2, 8, 2, 0, 0, 0]);
        bytes.extend([0; 4]);
        std::fs::write(&png, &bytes).unwrap();
        let found = details(png.to_str().unwrap(), false).unwrap();
        assert_eq!((found.width, found.height), (Some(3), Some(2)));
        assert!(matches(png.to_str().unwrap(), 33, 1, &shadui_index::query::parse("width:3 height:<3").unwrap().1.props));
        let _ = std::fs::remove_dir_all(&dir);
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
