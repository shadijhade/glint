//! Song details: title, artist, album, album artist, track, year, genre and cover art, read and written by
//! lofty (MP3, FLAC, M4A, Ogg Vorbis, Opus, WAV, AIFF, APE, WavPack, Musepack), for one song or many. Each
//! song's new details go into a copy in actions.rs's work folder, which then takes the song's place (the
//! song itself goes to the Recycle Bin), so Undo puts every song back as it was.

use crate::actions::{self, Job, Output, Progress, Report};
use crate::fsx::blocking;
use lofty::config::WriteOptions;
use lofty::picture::{Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::ipc::Channel;
use tauri::Window;

static STOP: AtomicBool = AtomicBool::new(false);

/// What lofty reads and writes tags in.
pub const SONGS: &[&str] = &["mp3", "flac", "m4a", "m4b", "aac", "ogg", "oga", "opus", "wav", "aif", "aiff", "ape", "wv", "mpc", "spx"];

pub fn is_song(path: &Path) -> bool {
    path.extension().is_some_and(|e| SONGS.contains(&e.to_string_lossy().to_ascii_lowercase().as_str()))
}

/// A song's details as they are.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SongTags {
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track: Option<u32>,
    pub track_total: Option<u32>,
    pub year: Option<u16>,
    pub genre: String,
    pub cover: bool,
    /// Why its details couldn't be read, when they couldn't.
    pub error: Option<String>,
}

#[tauri::command]
pub async fn song_tags(paths: Vec<String>) -> Result<Vec<SongTags>, String> {
    blocking(move || Ok(paths.iter().map(|p| read(Path::new(p))).collect())).await
}

fn read(path: &Path) -> SongTags {
    let mut out = SongTags { path: path.to_string_lossy().into_owned(), ..SongTags::default() };
    let file = match lofty::read_from_path(path) {
        Ok(file) => file,
        Err(e) => {
            out.error = Some(e.to_string());
            return out;
        }
    };
    if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
        let text = |s: Option<std::borrow::Cow<'_, str>>| s.map(|s| s.trim().to_string()).unwrap_or_default();
        out.title = text(tag.title());
        out.artist = text(tag.artist());
        out.album = text(tag.album());
        out.album_artist = tag.get_string(ItemKey::AlbumArtist).unwrap_or_default().trim().to_string();
        out.track = tag.track();
        out.track_total = tag.track_total();
        out.year = tag.date().map(|d| d.year);
        out.genre = text(tag.genre());
        out.cover = !tag.pictures().is_empty();
    }
    out
}

/// One song's new details: a field that's None stays as it is; an empty one is taken out.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SongEdit {
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track: Option<String>,
    pub track_total: Option<String>,
    pub year: Option<String>,
    pub genre: Option<String>,
    /// A picture file to be the front cover; "" takes the covers out.
    pub cover: Option<String>,
}

#[tauri::command]
pub fn song_cancel() {
    STOP.store(true, Ordering::SeqCst);
}

/// Writes each song's new details, four songs at a time.
#[tauri::command]
pub async fn song_write(window: Window, edits: Vec<SongEdit>, on_progress: Channel<Progress>) -> Result<Report, String> {
    let owner = crate::shell::owner(&window);
    STOP.store(false, Ordering::SeqCst);
    blocking(move || {
        let edits: Vec<SongEdit> = edits.into_iter().filter(|e| is_song(Path::new(&e.path))).collect();
        let jobs: Vec<Job> = edits.iter().map(|e| Job::one(&e.path)).collect();
        actions::run(owner, "tags", &jobs, &Output::Replace, 4, &STOP, &|p| drop(on_progress.send(p)), |job, _, work| {
            let edit = edits.iter().find(|e| Path::new(&e.path) == job.inputs[0]).ok_or("No details for it")?;
            write(&job.inputs[0], edit, work).map(|f| vec![f])
        })
    })
    .await
}

/// A copy of `song` in `work` with `edit`'s details.
fn write(song: &Path, edit: &SongEdit, work: &Path) -> Result<PathBuf, String> {
    let name = song.file_name().ok_or("It has no name")?;
    let copy = work.join(name);
    fs::copy(song, &copy).map_err(|e| e.to_string())?;
    let mut file = lofty::read_from_path(&copy).map_err(|e| e.to_string())?;
    if file.primary_tag_mut().is_none() {
        let kind = file.primary_tag_type();
        file.insert_tag(Tag::new(kind));
    }
    let tag = file.primary_tag_mut().ok_or("Its kind of file takes no details")?;
    apply(tag, edit)?;
    file.save_to_path(&copy, WriteOptions::default()).map_err(|e| e.to_string())?;
    Ok(copy)
}

fn apply(tag: &mut Tag, edit: &SongEdit) -> Result<(), String> {
    let set = |tag: &mut Tag, value: &Option<String>, put: fn(&mut Tag, String), remove: fn(&mut Tag)| {
        if let Some(value) = value {
            let value = value.trim();
            if value.is_empty() {
                remove(tag);
            } else {
                put(tag, value.to_string());
            }
        }
    };
    set(tag, &edit.title, |t, v| t.set_title(v), |t| t.remove_title());
    set(tag, &edit.artist, |t, v| t.set_artist(v), |t| t.remove_artist());
    set(tag, &edit.album, |t, v| t.set_album(v), |t| t.remove_album());
    set(tag, &edit.genre, |t, v| t.set_genre(v), |t| t.remove_genre());
    set(
        tag,
        &edit.album_artist,
        |t, v| {
            t.insert_text(ItemKey::AlbumArtist, v);
        },
        |t| t.remove_key(ItemKey::AlbumArtist),
    );
    if let Some(track) = &edit.track {
        match number(track)? {
            Some(n) => tag.set_track(n),
            None => tag.remove_track(),
        }
    }
    if let Some(total) = &edit.track_total {
        match number(total)? {
            Some(n) => tag.set_track_total(n),
            None => tag.remove_track_total(),
        }
    }
    if let Some(year) = &edit.year {
        match number(year)? {
            Some(y) if (1000..=9999).contains(&y) => tag.set_date(lofty::tag::items::Timestamp { year: y as u16, ..Default::default() }),
            Some(_) => return Err(format!("“{year}” isn't a year")),
            None => tag.remove_date(),
        }
    }
    if let Some(cover) = &edit.cover {
        tag.remove_picture_type(PictureType::CoverFront);
        if !cover.is_empty() {
            let mut file = fs::File::open(cover).map_err(|e| format!("The cover: {e}"))?;
            let mut picture = Picture::from_reader(&mut file).map_err(|_| "The cover isn't a JPEG, PNG, GIF, BMP or TIFF picture".to_string())?;
            picture.set_pic_type(PictureType::CoverFront);
            tag.push_picture(picture);
        } else {
            while !tag.pictures().is_empty() {
                tag.remove_picture(0);
            }
        }
    }
    Ok(())
}

/// A whole number, or None for an empty field.
fn number(text: &str) -> Result<Option<u32>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse().map(Some).map_err(|_| format!("“{text}” isn't a number"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_songs() {
        assert!(is_song(Path::new(r"C:\m\Track 01.MP3")) && is_song(Path::new("a.flac")) && !is_song(Path::new("a.mp4")));
    }

    #[test]
    fn reads_numbers() {
        assert_eq!(number(" 7 "), Ok(Some(7)));
        assert_eq!(number(""), Ok(None));
        assert!(number("seven").is_err());
    }

    /// A WAV made here gets every detail, then has them read back, then cleared.
    #[test]
    fn writes_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("glint-songs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("work")).unwrap();
        let song = dir.join("song.wav");
        fs::write(&song, wav()).unwrap();
        let edit = SongEdit {
            path: song.to_string_lossy().into_owned(),
            title: Some("Morning".into()),
            artist: Some("فيروز".into()),
            album: Some("Live".into()),
            album_artist: Some("Various".into()),
            track: Some("3".into()),
            track_total: Some("12".into()),
            year: Some("1971".into()),
            genre: Some("Folk".into()),
            cover: None,
        };
        let made = write(&song, &edit, &dir.join("work")).unwrap();
        let tags = read(&made);
        assert_eq!((tags.title.as_str(), tags.artist.as_str(), tags.album.as_str(), tags.genre.as_str()), ("Morning", "فيروز", "Live", "Folk"));
        assert_eq!((tags.track, tags.year), (Some(3), Some(1971)));
        let cleared = SongEdit { path: made.to_string_lossy().into_owned(), title: Some(String::new()), year: Some(String::new()), ..SongEdit::default() };
        fs::create_dir_all(dir.join("work2")).unwrap();
        let again = read(&write(&made, &cleared, &dir.join("work2")).unwrap());
        assert_eq!((again.title.as_str(), again.year, again.artist.as_str()), ("", None, "فيروز"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A tenth of a second of silence as a 16-bit mono WAV.
    fn wav() -> Vec<u8> {
        let samples = 4410u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + samples * 2).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&44100u32.to_le_bytes());
        out.extend_from_slice(&88200u32.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(samples * 2).to_le_bytes());
        out.resize(out.len() + samples as usize * 2, 0);
        out
    }
}
