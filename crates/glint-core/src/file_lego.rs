//! Engine actions for contextual file-on-file drops ("File Lego"): lossless media muxing
//! (subtitles or audio into video via FFmpeg) and single-file checksum verification against a sums
//! sidecar file.

use crate::actions;
use crate::checksums;
use crate::fsx::blocking;
use glint_model::aware::{Algo, SumsWant};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// Builds the FFmpeg argument list for lossless stream muxing of `track` into `video`.
pub(crate) fn mux_args(video: &Path, track: &Path, out: &Path, is_subtitles: bool) -> Vec<OsString> {
    let ext = out
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let mut args: Vec<OsString> = vec![
        "-y".into(),
        "-i".into(),
        video.into(),
        "-i".into(),
        track.into(),
        "-map".into(),
        "0".into(),
        "-map".into(),
        "1".into(),
        "-c".into(),
        "copy".into(),
    ];
    if is_subtitles && matches!(ext.as_str(), "mp4" | "m4v") {
        args.extend(["-c:s".into(), "mov_text".into()]);
    }
    args.push(out.into());
    args
}

/// Where the mux of `video` is written: beside it, as `<stem>_muxed.<ext>`, under a free name.
fn mux_output_path(video: &Path) -> Result<PathBuf, String> {
    let parent = video
        .parent()
        .ok_or_else(|| "Could not determine output folder".to_string())?;
    let stem = video
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "video".to_string());
    let ext = video
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mp4".to_string());
    let candidate = parent.join(format!("{stem}_muxed.{ext}"));
    // `free_name` numbers from (2) whatever the name is, so it is asked only when the plain one is taken.
    Ok(if candidate.exists() {
        crate::queue::free_name(&candidate)
    } else {
        candidate
    })
}

/// The file name a mux of `video` will make (`clip_muxed.mp4`, or `clip_muxed (2).mp4` when that is taken), for
/// the drop's pill. Looks at the disk: call it off the UI thread.
pub fn mux_output_name(video: &str) -> Option<String> {
    let out = mux_output_path(Path::new(video)).ok()?;
    out.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Losslessly muxes `track` (subtitles or audio) into `video`, writing a sibling `<stem>_muxed.<ext>`
/// file without overwriting existing files. Returns the created file path.
pub async fn mux_track(video: String, track: String, is_subtitles: bool) -> Result<String, String> {
    blocking(move || mux_track_sync(Path::new(&video), Path::new(&track), is_subtitles)).await
}

fn mux_track_sync(video: &Path, track: &Path, is_subtitles: bool) -> Result<String, String> {
    if !video.is_file() {
        return Err(format!("Video file not found: {}", video.display()));
    }
    if !track.is_file() {
        return Err(format!("Track file not found: {}", track.display()));
    }
    let out = mux_output_path(video)?;

    let mut command = crate::tools::command("ffmpeg")?;
    command.args(mux_args(video, track, &out, is_subtitles));
    let stop = AtomicBool::new(false);
    if let Err(err) = actions::run_tool(command, "FFmpeg", &stop, |_| {}) {
        let _ = std::fs::remove_file(&out);
        return Err(err);
    }
    if !out.is_file() {
        return Err("FFmpeg did not create the muxed video".into());
    }
    Ok(out.to_string_lossy().into_owned())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SingleCheckResult {
    pub matched: bool,
    pub message: String,
}

/// Verifies `file` against its entry in `sums_file` (`.sha256`, `.sha512`, `.sha1`, `.md5`, `.b3sum`).
pub async fn verify_single_against_sums(file: String, sums_file: String) -> Result<SingleCheckResult, String> {
    blocking(move || verify_single_sync(Path::new(&file), &sums_file)).await
}

fn want_of(algo: Algo) -> SumsWant {
    SumsWant {
        md5: algo == Algo::Md5,
        sha1: algo == Algo::Sha1,
        sha256: algo == Algo::Sha256,
        sha512: algo == Algo::Sha512,
        blake3: algo == Algo::Blake3,
    }
}

fn algo_label(algo: Algo) -> &'static str {
    match algo {
        Algo::Md5 => "MD5",
        Algo::Sha1 => "SHA-1",
        Algo::Sha256 => "SHA-256",
        Algo::Sha512 => "SHA-512",
        Algo::Blake3 => "BLAKE3",
    }
}

pub(crate) fn verify_single_sync(file: &Path, sums_file: &str) -> Result<SingleCheckResult, String> {
    if !file.is_file() {
        return Err("Target item is not a file".into());
    }
    let algo = checksums::algo_of(sums_file).ok_or_else(|| "Unsupported checksum file format".to_string())?;
    let bytes = std::fs::read(sums_file).map_err(|e| format!("Couldn't read checksum file: {e}"))?;
    let text = String::from_utf8_lossy(&bytes);
    let (lines, _) = checksums::parse(&text, algo);
    if lines.is_empty() {
        return Err("No valid checksum entries found in this file".into());
    }
    let sums_dir: PathBuf = Path::new(sums_file).parent().map(Path::to_path_buf).unwrap_or_default();
    let file_name = file
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let file_lower = file.to_string_lossy().to_lowercase();

    let found = lines.iter().find(|line| {
        let listed = sums_dir.join(line.name.replace('/', "\\"));
        let listed_lower = listed.to_string_lossy().to_lowercase();
        listed_lower == file_lower || line.name.to_lowercase() == file_name
    });

    let Some(entry) = found else {
        return Err(format!(
            "“{}” is not listed in this {} file",
            file.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
            algo_label(algo)
        ));
    };

    let stop = AtomicBool::new(false);
    let sums = checksums::digest(file, want_of(algo), None, &stop)
        .ok_or_else(|| "Couldn't read the file to compute its checksum".to_string())?;
    let actual = sums
        .get(algo)
        .ok_or_else(|| format!("Couldn't compute {}", algo_label(algo)))?;
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let label = algo_label(algo);
    if actual.eq_ignore_ascii_case(&entry.hex) {
        Ok(SingleCheckResult {
            matched: true,
            message: format!("{name} matches {label}"),
        })
    } else {
        Ok(SingleCheckResult {
            matched: false,
            message: format!("{name} does NOT match {label}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mux_names_its_output_beside_the_video_and_avoids_a_taken_name() {
        let dir = std::env::temp_dir().join(format!("glint-mux-name-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("clip.mp4");
        assert_eq!(
            mux_output_name(&video.to_string_lossy()).as_deref(),
            Some("clip_muxed.mp4")
        );
        std::fs::write(dir.join("clip_muxed.mp4"), b"x").unwrap();
        let next = mux_output_name(&video.to_string_lossy()).unwrap();
        assert_ne!(next, "clip_muxed.mp4");
        assert!(next.starts_with("clip_muxed") && next.ends_with(".mp4"), "{next}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn builds_mux_arguments_for_mp4_subtitles_and_mkv_audio() {
        let args_mp4 = mux_args(
            Path::new(r"C:\v\clip.mp4"),
            Path::new(r"C:\v\subs.srt"),
            Path::new(r"C:\v\clip_muxed.mp4"),
            true,
        );
        let strs: Vec<String> = args_mp4.iter().map(|s| s.to_string_lossy().into_owned()).collect();
        assert!(strs.contains(&"mov_text".to_string()));
        assert_eq!(strs.last().map(String::as_str), Some(r"C:\v\clip_muxed.mp4"));

        let args_mkv = mux_args(
            Path::new(r"C:\v\clip.mkv"),
            Path::new(r"C:\v\audio.flac"),
            Path::new(r"C:\v\clip_muxed.mkv"),
            false,
        );
        let mkv_strs: Vec<String> = args_mkv.iter().map(|s| s.to_string_lossy().into_owned()).collect();
        assert!(!mkv_strs.contains(&"mov_text".to_string()));
    }

    #[test]
    fn verifies_single_file_against_sha256_sidecar() {
        let dir = std::env::temp_dir().join(format!("glint-file-lego-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let target = dir.join("hello.bin");
        std::fs::write(&target, b"glint-lego-payload").unwrap();
        let digest = checksums::digest(&target, want_of(Algo::Sha256), None, &AtomicBool::new(false)).unwrap();
        let hex = digest.get(Algo::Sha256).unwrap();

        let sums = dir.join("SHA256SUMS.sha256");
        std::fs::write(&sums, format!("{hex}  hello.bin\n")).unwrap();

        let ok = verify_single_sync(&target, &sums.to_string_lossy()).unwrap();
        assert!(ok.matched, "{}", ok.message);

        // Mutate payload -> mismatch
        std::fs::write(&target, b"tampered").unwrap();
        let bad = verify_single_sync(&target, &sums.to_string_lossy()).unwrap();
        assert!(!bad.matched, "{}", bad.message);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
