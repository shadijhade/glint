//! Make smaller: one action for a selection of any kind (Make smaller… on the menu, and Ctrl+K), each
//! file made smaller by the best engine for its kind, through actions.rs, so what's made lands beside the
//! originals or in their place and Undo takes it back. Nothing that comes out bigger is kept, and what's
//! made keeps the original's date modified (a photo stays where it was in a folder sorted by date).
//!
//! - Videos: ab-av1 encodes short samples and scores them with VMAF (XPSNR where FFmpeg has no libvmaf)
//!   to find the highest CRF that still looks the same; FFmpeg then encodes the whole video at it with
//!   x264, x265 or SVT-AV1 (software encoders make smaller files than the hardware ones at a quality).
//! - JPEG photos: re-encoded by MozJPEG (rimage), or repacked as JPEG XL without losing a bit (cjxl),
//!   which djxl turns back into the very same JPEG (Back to JPEG). BMPs become PNGs; PNGs go through OxiPNG.
//! - PDFs: Ghostscript rewrites their pictures for screens and mail (150 dpi) or print (300 dpi); pdfcpu's
//!   optimize is the lossless choice.
//! - Word, PowerPoint and Excel files: their pictures recompressed (JPEGs capped at 3000 pixels), the rest
//!   of the file copied as it was.

use crate::actions::{self, Job, Output, Progress, Report};
use crate::fsx::blocking;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use crate::host::ipc::Channel;
use crate::host::Window;

static STOP: AtomicBool = AtomicBool::new(false);

pub fn shrink_cancel() {
    STOP.store(true, Ordering::SeqCst);
}

/// The kinds of file Make smaller works on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Video,
    Jpeg,
    Bmp,
    Png,
    Pdf,
    Office,
}

/// What Make smaller does with `path`, by its extension: nothing (None) for kinds it doesn't take.
pub fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    Some(match ext.as_str() {
        "mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" | "wmv" | "flv" | "mts" | "m2ts" | "ts" | "3gp" | "mpg" | "mpeg" => Kind::Video,
        "jpg" | "jpeg" | "jpe" | "jfif" => Kind::Jpeg,
        "bmp" => Kind::Bmp,
        "png" => Kind::Png,
        "pdf" => Kind::Pdf,
        "docx" | "docm" | "pptx" | "pptm" | "xlsx" | "xlsm" => Kind::Office,
        _ => return None,
    })
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PhotoWay {
    /// MozJPEG at `quality`: still a JPEG, opens everywhere.
    Jpeg,
    /// JPEG XL, losslessly: the very same JPEG comes back with Back to JPEG.
    Jxl,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PdfWay {
    /// Pictures at 150 dpi, for screens and mail.
    Screen,
    /// Pictures at 300 dpi, for print.
    Print,
    /// Nothing lost: pdfcpu's optimize.
    Lossless,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ShrinkChoices {
    pub video: VideoCodec,
    pub photo: PhotoWay,
    pub pdf: PdfWay,
    /// MozJPEG's quality for photos and the pictures in Office files (50 to 95).
    pub quality: u8,
}

/// What this PC's FFmpeg can do for Make smaller: its software encoders, and VMAF.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Engines {
    pub h264: bool,
    pub hevc: bool,
    pub av1: bool,
    pub vmaf: bool,
}

static ENGINES: Mutex<Option<Engines>> = Mutex::new(None);

pub async fn shrink_engines() -> Result<Engines, String> {
    blocking(engines).await
}

fn engines() -> Result<Engines, String> {
    if let Some(known) = ENGINES.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Ok(known);
    }
    let list = |what: &str| -> Result<String, String> {
        let mut command = crate::tools::command("ffmpeg")?;
        command.args(["-hide_banner", what]);
        let done = command.output().map_err(|e| format!("Couldn't start FFmpeg: {e}"))?;
        Ok(String::from_utf8_lossy(&done.stdout).into_owned())
    };
    let (encoders, filters) = (list("-encoders")?, list("-filters")?);
    let found = Engines { h264: listed(&encoders, "libx264"), hevc: listed(&encoders, "libx265"), av1: listed(&encoders, "libsvtav1"), vmaf: listed(&filters, "libvmaf") };
    *ENGINES.lock().unwrap_or_else(|e| e.into_inner()) = Some(found.clone());
    Ok(found)
}

/// Whether FFmpeg's `-encoders` or `-filters` list names `name` (its second word on a line).
fn listed(list: &str, name: &str) -> bool {
    list.lines().any(|l| l.split_whitespace().nth(1) == Some(name))
}

/// Makes `files` smaller as `choices` say, one video at a time (it keeps the processor busy on its own),
/// else three files at once; files of other kinds are left out.
pub async fn shrink_run(window: Window, files: Vec<String>, choices: ShrinkChoices, output: Output, on_progress: Channel<Progress>) -> Result<Report, String> {
    let owner = crate::shell::owner(&window);
    STOP.store(false, Ordering::SeqCst);
    blocking(move || {
        let jobs: Vec<Job> = files.iter().filter(|f| kind_of(Path::new(f)).is_some()).map(Job::one).collect();
        if jobs.is_empty() {
            return Err("Nothing chosen is a kind of file Make smaller works on".into());
        }
        let videos = jobs.iter().any(|j| kind_of(&j.inputs[0]) == Some(Kind::Video));
        actions::run(owner, "shrink", &jobs, &output, if videos { 1 } else { 3 }, &STOP, &|p| drop(on_progress.send(p)), |job, part, work| make(&choices, &job.inputs[0], part, work).map(|f| vec![f]))
    })
    .await
}

/// Back to JPEG: JPEG XL files as JPEGs, the very JPEG they were made from when Make smaller (or cjxl)
/// repacked one, else a JPEG of their pixels.
pub async fn jxl_to_jpeg(window: Window, files: Vec<String>, output: Output, on_progress: Channel<Progress>) -> Result<Report, String> {
    let owner = crate::shell::owner(&window);
    STOP.store(false, Ordering::SeqCst);
    blocking(move || {
        let djxl = djxl()?;
        let jobs: Vec<Job> = files.iter().filter(|f| f.to_ascii_lowercase().ends_with(".jxl")).map(Job::one).collect();
        actions::run(owner, "jpeg", &jobs, &output, 3, &STOP, &|p| drop(on_progress.send(p)), |job, _, work| {
            let file = &job.inputs[0];
            let made = actions::renamed(work, file, "jpg");
            let mut command = Command::new(&djxl);
            crate::proc::no_window(&mut command);
            command.arg(file).arg(&made).arg("--quiet");
            actions::run_tool(command, "djxl", &STOP, |_| {})?;
            keep_date(file, &made);
            Ok(vec![made])
        })
    })
    .await
}

/// djxl, which comes beside cjxl in libjxl's release.
fn djxl() -> Result<PathBuf, String> {
    let (cjxl, _) = crate::proc::locate("cjxl").ok_or("libjxl isn't installed. Settings → Tools installs it.")?;
    let djxl = cjxl.with_file_name("djxl.exe");
    djxl.is_file().then_some(djxl).ok_or_else(|| "djxl isn't beside cjxl: Settings → Tools installs libjxl again".to_string())
}

fn already() -> String {
    "It's already about as small as this makes it".into()
}

/// One file made smaller into `work`: what was made, or why not.
fn make(choices: &ShrinkChoices, file: &Path, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<PathBuf, String> {
    let kind = kind_of(file).ok_or("Make smaller doesn't work on this kind of file")?;
    let quality = choices.quality.clamp(50, 95).to_string();
    let made = match kind {
        Kind::Video => video(file, choices.video, part, work)?,
        Kind::Jpeg if choices.photo == PhotoWay::Jxl => jxl(file, work)?,
        Kind::Jpeg => rimage(file, &["mozjpeg", "-q", &quality], "jpg", work)?,
        Kind::Bmp | Kind::Png => rimage(file, &["oxipng"], "png", work)?,
        Kind::Pdf => pdf(file, choices.pdf, part, work)?,
        Kind::Office => office(file, &quality, part, work)?,
    };
    // Kept only when it saves something worth having.
    let (before, after) = (size(file), size(&made));
    if after == 0 || after as f64 > before as f64 * 0.98 {
        let _ = fs::remove_file(&made);
        return Err(already());
    }
    keep_date(file, &made);
    Ok(made)
}

fn size(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |m| m.len())
}

/// Gives `made` the date modified `original` has.
fn keep_date(original: &Path, made: &Path) {
    if let Ok(modified) = fs::metadata(original).and_then(|m| m.modified()) {
        if let Ok(file) = File::options().write(true).open(made) {
            let _ = file.set_modified(modified);
        }
    }
}

/// A picture through rimage's `codec` (with its options), into `work`, its date modified kept.
fn rimage(file: &Path, codec: &[&str], ext: &str, work: &Path) -> Result<PathBuf, String> {
    let mut command = crate::tools::command("rimage")?;
    command.args(codec).args(["-p", "--no-progress", "-d"]).arg(work).arg("--").arg(file);
    actions::run_tool(command, "rimage", &STOP, |_| {})?;
    // rimage keeps every dot of the name.
    let stem = file.file_stem().ok_or("It has no name")?.to_string_lossy();
    let made = work.join(format!("{stem}.{ext}"));
    made.is_file().then_some(made).ok_or_else(|| "rimage made nothing".to_string())
}

/// A JPEG repacked as JPEG XL, losslessly.
fn jxl(file: &Path, work: &Path) -> Result<PathBuf, String> {
    let made = actions::renamed(work, file, "jxl");
    let mut command = crate::tools::command("libjxl")?;
    command.arg(file).arg(&made).args(["--lossless_jpeg=1", "--quiet"]);
    actions::run_tool(command, "cjxl", &STOP, |_| {})?;
    Ok(made)
}

/// A video's colours as ffprobe reads them: HDR's need keeping on what's made.
#[derive(Default, Debug, PartialEq)]
struct Colour {
    primaries: String,
    transfer: String,
    space: String,
}

impl Colour {
    fn of(file: &Path) -> Colour {
        let Ok(mut command) = crate::media_tools::ffprobe() else { return Colour::default() };
        command.args(["-v", "error", "-select_streams", "V:0", "-show_entries", "stream=color_primaries,color_transfer,color_space", "-of", "json", "--"]).arg(file);
        let Ok(done) = command.output() else { return Colour::default() };
        let value: serde_json::Value = serde_json::from_slice(&done.stdout).unwrap_or_default();
        let stream = &value["streams"][0];
        let text = |key: &str| stream[key].as_str().filter(|v| !v.is_empty() && *v != "unknown").unwrap_or_default().to_string();
        Colour { primaries: text("color_primaries"), transfer: text("color_transfer"), space: text("color_space") }
    }

    /// PQ (HDR10, Dolby Vision's base) or HLG (phones' HDR).
    fn hdr(&self) -> bool {
        matches!(self.transfer.as_str(), "smpte2084" | "arib-std-b67")
    }

    /// FFmpeg's names and values for them, the ones known.
    fn pairs(&self) -> Vec<(&'static str, &str)> {
        [("color_primaries", self.primaries.as_str()), ("color_trc", self.transfer.as_str()), ("colorspace", self.space.as_str())].into_iter().filter(|(_, v)| !v.is_empty()).collect()
    }
}

/// The encoder, its preset and whether this FFmpeg has it.
fn encoder_of(codec: VideoCodec, engines: &Engines) -> (&'static str, &'static str, bool) {
    match codec {
        VideoCodec::H264 => ("libx264", "slow", engines.h264),
        VideoCodec::Hevc => ("libx265", "medium", engines.hevc),
        VideoCodec::Av1 => ("libsvtav1", "7", engines.av1),
    }
}

fn video(file: &Path, codec: VideoCodec, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<PathBuf, String> {
    let info = crate::media_tools::probe(file)?;
    if info.video.is_empty() {
        return Err("It has no video in it".into());
    }
    let engines = engines()?;
    let (encoder, preset, available) = encoder_of(codec, &engines);
    if !available {
        return Err(format!("This FFmpeg has no {encoder}: choose another format"));
    }
    let colour = Colour::of(file);
    if colour.hdr() && codec == VideoCodec::H264 {
        return Err("It's an HDR video, whose colours H.264 can't keep: choose HEVC or AV1".into());
    }
    let pixels = if colour.hdr() || codec == VideoCodec::Av1 { "yuv420p10le" } else { "yuv420p" };
    let crf = search(file, encoder, preset, pixels, &colour, engines.vmaf, codec == VideoCodec::Av1, part, work)?;
    encode(file, &info, encoder, preset, pixels, &colour, crf, part, work)
}

/// ab-av1's search for the highest CRF whose samples still look the same (VMAF 95, or XPSNR 40 where
/// FFmpeg has no libvmaf), at most 90% of the original's size; its samples take the first third of the
/// job's progress.
#[allow(clippy::too_many_arguments)]
fn search(file: &Path, encoder: &str, preset: &str, pixels: &str, colour: &Colour, vmaf: bool, whole_steps: bool, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<f64, String> {
    let mut command = crate::tools::command("ab-av1")?;
    command.args(["crf-search", "-e", encoder, "--preset", preset, "--pix-format", pixels, "--max-encoded-percent", "90", "--cache", "false", "--stdout-format", "json"]);
    command.args(if vmaf { ["--min-vmaf", "95"] } else { ["--min-xpsnr", "40"] });
    if whole_steps {
        // FFmpeg takes SVT-AV1's CRF as a whole number.
        command.args(["--crf-increment", "1"]);
    }
    for (name, value) in colour.pairs() {
        command.arg("--enc").arg(format!("{name}={value}"));
    }
    command.arg("--temp-dir").arg(work).arg("-i").arg(file);
    // ab-av1 runs the FFmpeg on PATH: Glint's first.
    if let Some((ffmpeg, _)) = crate::proc::locate("ffmpeg") {
        if let Some(dir) = ffmpeg.parent() {
            let path = std::env::var_os("PATH").unwrap_or_default();
            let joined = std::env::join_paths(std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&path))).unwrap_or(path);
            command.env("PATH", joined);
        }
    }
    let mut found = None;
    let mut samples = 0u32;
    actions::run_tool(command, "ab-av1", &STOP, |line| {
        let Ok(said) = serde_json::from_str::<serde_json::Value>(line) else { return };
        match said["type"].as_str() {
            Some("sample-encode-done") => {
                samples += 1;
                part((samples as f32 * 0.05).min(0.33));
            }
            Some("crf-search-done") => found = said["crf"].as_f64(),
            _ => {}
        }
    })
    .map_err(|e| if e.to_ascii_lowercase().contains("suitable crf") { already() } else { e })?;
    found.ok_or_else(|| "ab-av1 found no setting".to_string())
}

/// A CRF as FFmpeg takes it: whole, or to two places (x264 and x265 take fractions).
fn crf_text(crf: f64) -> String {
    let text = format!("{crf:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The whole video at `crf`: its first video stream encoded, its sound copied (or AAC where the
/// container can't hold it), subtitles kept where the container can; MP4, or MKV for MKV and WebM.
#[allow(clippy::too_many_arguments)]
fn encode(file: &Path, info: &crate::media_tools::Probe, encoder: &str, preset: &str, pixels: &str, colour: &Colour, crf: f64, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<PathBuf, String> {
    let ext = file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let mkv = matches!(ext.as_str(), "mkv" | "webm");
    let out = actions::renamed(work, file, if mkv { "mkv" } else { "mp4" });
    let mut command = crate::tools::command("ffmpeg")?;
    command.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-progress", "pipe:1", "-nostats", "-i"]).arg(file);
    command.args(["-map", "0:V:0", "-map", "0:a?", "-map_metadata", "0", "-c:v", encoder, "-crf", &crf_text(crf), "-preset", preset, "-pix_fmt", pixels]);
    for (name, value) in colour.pairs() {
        command.arg(format!("-{name}")).arg(value);
    }
    if encoder == "libx265" {
        // Apple's players want HEVC tagged hvc1.
        command.args(["-tag:v", "hvc1"]);
    }
    let mp4_sound = ["aac", "mp3", "alac", "ac3", "eac3", "opus", "flac"].contains(&info.audio.as_str());
    if mkv || mp4_sound || info.audio.is_empty() {
        command.args(["-c:a", "copy"]);
    } else {
        command.args(["-c:a", "aac", "-b:a", "192k"]);
    }
    if mkv {
        command.args(["-map", "0:s?", "-map", "0:t?", "-c:s", "copy", "-c:t", "copy"]);
    } else {
        if matches!(ext.as_str(), "mp4" | "m4v" | "mov" | "3gp") {
            command.args(["-map", "0:s?", "-c:s", "copy"]);
        }
        command.args(["-movflags", "+faststart"]);
    }
    command.arg(&out);
    let seconds = info.duration;
    actions::run_tool(command, "FFmpeg", &STOP, |line| {
        if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
            if seconds > 0.0 {
                part(0.33 + 0.67 * (us / 1e6 / seconds).clamp(0.0, 1.0) as f32);
            }
        }
    })?;
    Ok(out)
}

/// A PDF made smaller: by Ghostscript, its pictures for screens or print, or losslessly by pdfcpu.
fn pdf(file: &Path, way: PdfWay, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<PathBuf, String> {
    let out = actions::renamed(work, file, "pdf");
    if way == PdfWay::Lossless {
        let mut command = crate::tools::command("pdfcpu")?;
        command.args(["-c", "disable", "--offline", "optimize", "--"]).arg(file).arg(&out);
        actions::run_tool(command, "pdfcpu", &STOP, |_| {})?;
        return Ok(out);
    }
    let mut command = crate::tools::command("ghostscript")?;
    command.args(gs_arguments(way, &out)).arg("-f").arg(file);
    let mut pages = 0u32;
    actions::run_tool(command, "Ghostscript", &STOP, |line| {
        if let Some(count) = line.strip_prefix("Processing pages 1 through ") {
            pages = count.trim().trim_end_matches('.').parse().unwrap_or(0);
        } else if let Some(page) = line.strip_prefix("Page ").and_then(|n| n.trim().parse::<u32>().ok()) {
            if pages > 0 {
                part(page as f32 / pages as f32);
            }
        }
    })
    .map_err(|e| if e.to_ascii_lowercase().contains("password") { "It has a password: take it off first, in PDF tools".to_string() } else { e })?;
    Ok(out)
}

/// Ghostscript's arguments (the PDF to read goes after them, behind `-f`).
fn gs_arguments(way: PdfWay, out: &Path) -> Vec<String> {
    let setting = if way == PdfWay::Print { "/printer" } else { "/ebook" };
    vec![
        "-dSAFER".into(),
        "-dBATCH".into(),
        "-dNOPAUSE".into(),
        "-sDEVICE=pdfwrite".into(),
        "-dCompatibilityLevel=1.7".into(),
        format!("-dPDFSETTINGS={setting}"),
        "-dDetectDuplicateImages=true".into(),
        "-dAutoRotatePages=/None".into(),
        // A % starts a format in Ghostscript's file names.
        format!("-sOutputFile={}", out.to_string_lossy().replace('%', "%%")),
    ]
}

/// A Word, PowerPoint or Excel file with its pictures recompressed: JPEGs by MozJPEG (their longest side
/// at most 3000 pixels, enough for a full slide at 220 dpi), PNGs by OxiPNG, each kept only when smaller;
/// everything else copied as it was.
fn office(file: &Path, quality: &str, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<PathBuf, String> {
    let mut zip = zip::ZipArchive::new(File::open(file).map_err(|e| e.to_string())?).map_err(|_| "Glint can't open it (it may have a password)".to_string())?;
    let (pictures_in, pictures_out) = (work.join("in"), work.join("out"));
    fs::create_dir_all(&pictures_in).map_err(|e| e.to_string())?;
    fs::create_dir_all(&pictures_out).map_err(|e| e.to_string())?;
    // Each picture worth the work: its index, its name in the file, and where it was put.
    let mut pictures: Vec<(usize, String, PathBuf)> = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Ok(name) = entry.name().map(|n| n.into_owned()) else { continue };
        let Some(ext) = picture_ext(&name) else { continue };
        if entry.size() < 32 * 1024 {
            continue;
        }
        let path = pictures_in.join(format!("{i}.{ext}"));
        let mut out = File::create(&path).map_err(|e| e.to_string())?;
        std::io::copy(&mut (&mut entry).take(512 << 20), &mut out).map_err(|e| e.to_string())?;
        pictures.push((i, name, path));
    }
    if pictures.is_empty() {
        return Err("It has no pictures big enough to make smaller".into());
    }
    part(0.1);
    let jpegs: Vec<&PathBuf> = pictures.iter().map(|(_, _, p)| p).filter(|p| p.extension().is_some_and(|e| e != "png")).collect();
    let pngs: Vec<&PathBuf> = pictures.iter().map(|(_, _, p)| p).filter(|p| p.extension().is_some_and(|e| e == "png")).collect();
    let jpeg_codec = ["mozjpeg", "-q", quality, "--resize", "3000l", "--reduce-only"];
    for (codec, list) in [(&jpeg_codec[..], jpegs), (&["oxipng"][..], pngs)] {
        for batch in list.chunks(64) {
            let mut command = crate::tools::command("rimage")?;
            command.args(codec).args(["--no-progress", "-d"]).arg(&pictures_out).arg("--").args(batch);
            actions::run_tool(command, "rimage", &STOP, |_| {})?;
        }
    }
    part(0.85);
    let ext = file.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let made = actions::renamed(work, file, &ext);
    let mut writer = zip::ZipWriter::new(File::create(&made).map_err(|e| e.to_string())?);
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for i in 0..zip.len() {
        if let Some((_, name, original)) = pictures.iter().find(|(j, ..)| *j == i) {
            let stem = original.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let smaller = ["jpg", "png"].iter().map(|e| pictures_out.join(format!("{stem}.{e}"))).find(|p| p.is_file()).filter(|p| size(p) < size(original));
            if let Some(smaller) = smaller {
                let bytes = fs::read(&smaller).map_err(|e| e.to_string())?;
                writer.start_file(name.as_str(), stored).map_err(|e| e.to_string())?;
                writer.write_all(&bytes).map_err(|e| e.to_string())?;
                continue;
            }
        }
        let entry = zip.by_index_raw(i).map_err(|e| e.to_string())?;
        writer.raw_copy_file(entry).map_err(|e| e.to_string())?;
    }
    writer.finish().map_err(|e| e.to_string())?;
    Ok(made)
}

/// A picture's extension when `name` is one in an Office file's media folder.
fn picture_ext(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    if !lower.contains("/media/") {
        return None;
    }
    match lower.rsplit('.').next()? {
        "jpg" | "jpeg" => Some("jpg"),
        "png" => Some("png"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_what_it_works_on() {
        assert_eq!(kind_of(Path::new(r"C:\v\Trip.MOV")), Some(Kind::Video));
        assert_eq!(kind_of(Path::new(r"C:\p\IMG_1.jpeg")), Some(Kind::Jpeg));
        assert_eq!(kind_of(Path::new(r"C:\p\scan.bmp")), Some(Kind::Bmp));
        assert_eq!(kind_of(Path::new(r"C:\d\deck.pptx")), Some(Kind::Office));
        assert_eq!(kind_of(Path::new(r"C:\d\old.ppt")), None);
        assert_eq!(kind_of(Path::new(r"C:\d\README")), None);
    }

    #[test]
    fn reads_ffmpegs_lists() {
        let encoders = " V....D libx264              libx264 H.264 / AVC\n V....D libx265              libx265 H.265 / HEVC\n";
        assert!(listed(encoders, "libx264") && listed(encoders, "libx265") && !listed(encoders, "libsvtav1"));
        assert!(listed(" .. libvmaf           VV->V      Calculate the VMAF", "libvmaf"));
    }

    #[test]
    fn writes_crfs_as_ffmpeg_takes_them() {
        assert_eq!(crf_text(23.0), "23");
        assert_eq!(crf_text(22.600000381469727), "22.6");
        assert_eq!(crf_text(31.25), "31.25");
    }

    #[test]
    fn keeps_hdr_colours() {
        let hlg = Colour { primaries: "bt2020".into(), transfer: "arib-std-b67".into(), space: "bt2020nc".into() };
        assert!(hlg.hdr());
        assert_eq!(hlg.pairs(), [("color_primaries", "bt2020"), ("color_trc", "arib-std-b67"), ("colorspace", "bt2020nc")]);
        assert!(!Colour { transfer: "bt709".into(), ..Colour::default() }.hdr());
        assert!(Colour::default().pairs().is_empty());
    }

    #[test]
    fn asks_ghostscript_for_the_resolution_chosen() {
        let args = gs_arguments(PdfWay::Screen, Path::new(r"C:\w\100% done.pdf"));
        assert!(args.contains(&"-dPDFSETTINGS=/ebook".to_string()) && args.contains(&"-dSAFER".to_string()));
        assert!(args.contains(&r"-sOutputFile=C:\w\100%% done.pdf".to_string()));
        assert!(gs_arguments(PdfWay::Print, Path::new("x.pdf")).contains(&"-dPDFSETTINGS=/printer".to_string()));
    }

    /// Every file in SAMPLES made smaller with the tools in TOOLS_DIR (tools\<id>\<program>, as Glint keeps
    /// them; FFmpeg from the PC), each way, and JPEG XL back to the very same JPEG.
    #[test]
    #[ignore]
    fn shrinks_real_files() {
        crate::tools::set_dir(PathBuf::from(std::env::var("TOOLS_DIR").expect("TOOLS_DIR")));
        let samples = PathBuf::from(std::env::var("SAMPLES").expect("SAMPLES"));
        let ways = [
            ShrinkChoices { video: VideoCodec::H264, photo: PhotoWay::Jpeg, pdf: PdfWay::Screen, quality: 80 },
            ShrinkChoices { video: VideoCodec::Hevc, photo: PhotoWay::Jxl, pdf: PdfWay::Print, quality: 80 },
        ];
        let mut failed = Vec::new();
        for (n, choices) in ways.iter().enumerate() {
            for file in fs::read_dir(&samples).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_file() && kind_of(p).is_some()) {
                let work = std::env::temp_dir().join(format!("glint-shrink-{}-{n}", std::process::id())).join(file.file_name().unwrap());
                fs::create_dir_all(&work).unwrap();
                let started = std::time::Instant::now();
                match make(choices, &file, &|_| {}, &work) {
                    Ok(made) => {
                        println!("{n} {}: {} → {} ({}) in {:.1} s", file.display(), size(&file), size(&made), made.extension().unwrap().to_string_lossy(), started.elapsed().as_secs_f32());
                        assert_eq!(fs::metadata(&made).unwrap().modified().unwrap(), fs::metadata(&file).unwrap().modified().unwrap(), "date kept");
                        if choices.photo == PhotoWay::Jxl && kind_of(&file) == Some(Kind::Jpeg) {
                            let back = work.join("back.jpg");
                            let status = Command::new(djxl().unwrap()).arg(&made).arg(&back).status().unwrap();
                            assert!(status.success() && fs::read(&back).unwrap() == fs::read(&file).unwrap(), "the very same JPEG back");
                        }
                    }
                    // Already small enough (a PDF whose pictures are under print's resolution) is an answer.
                    Err(why) if why == already() => println!("{n} {}: {why}", file.display()),
                    Err(why) => {
                        println!("{n} {}: {why}", file.display());
                        failed.push(format!("{} ({n}): {why}", file.display()));
                    }
                }
                let _ = fs::remove_dir_all(work.parent().unwrap());
            }
        }
        assert!(failed.is_empty(), "{failed:#?}");
    }

    #[test]
    fn finds_office_pictures() {
        assert_eq!(picture_ext("ppt/media/image1.JPEG"), Some("jpg"));
        assert_eq!(picture_ext("word/media/image2.png"), Some("png"));
        assert_eq!(picture_ext("xl/media/image3.emf"), None);
        assert_eq!(picture_ext("docProps/thumbnail.jpeg"), None);
    }
}
