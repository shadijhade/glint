//! Media studio: FFmpeg (a tool Glint installs) trims, converts, shrinks to a size, takes the sound out
//! of and makes GIFs of videos and recordings, and whisper.cpp (speech search's) writes their
//! subtitles; and a video WebView2 can't play is remuxed (or, when its codec is the problem, converted)
//! into a copy it can, for the preview and Quick Look. Each action runs through actions.rs, so what's
//! made lands beside the originals (or in their place) and Undo takes it back.
//!
//! Encoders: the PC's hardware ones (NVIDIA's NVENC, Intel's Quick Sync, AMD's AMF) when FFmpeg has them
//! and a test frame goes through, else x264, x265 and SVT-AV1. They're found once a session.

use crate::actions::{self, Job, Output, Progress, Report};
use crate::fsx::blocking;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::ipc::Channel;
use tauri::Window;

/// Video codecs WebView2 decodes itself (HEVC only with Windows' extension, so it's left out), and sounds.
const PLAYS_VIDEO: &[&str] = &["h264", "vp8", "vp9", "av1"];
const PLAYS_AUDIO: &[&str] = &["aac", "mp3", "opus", "vorbis", "flac", "pcm_s16le", "pcm_f32le"];

static STOP: AtomicBool = AtomicBool::new(false);

#[tauri::command]
pub fn media_cancel() {
    STOP.store(true, Ordering::SeqCst);
}

fn ffmpeg() -> Result<Command, String> {
    crate::tools::command("ffmpeg")
}

/// ffprobe, from beside the FFmpeg that runs (every FFmpeg build has it), else any on the PC.
pub(crate) fn ffprobe() -> Result<Command, String> {
    let beside = crate::proc::locate("ffmpeg").map(|(exe, _)| exe.with_file_name("ffprobe.exe")).filter(|p| p.is_file());
    let exe = beside.or_else(|| crate::proc::locate("ffprobe").map(|(p, _)| p)).ok_or("FFmpeg isn't installed. Settings → Tools installs it.")?;
    let mut command = Command::new(exe);
    crate::proc::no_window(&mut command);
    Ok(command)
}

/// What a video or recording holds.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    /// Seconds.
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// The first video stream's codec (h264, hevc…), "" for a sound.
    pub video: String,
    /// The first sound stream's codec, "" for none.
    pub audio: String,
    /// The container, as ffprobe names it (mov,mp4,m4a…).
    pub container: String,
    /// Bits a second, all streams.
    pub bitrate: u64,
    /// Whether WebView2 plays it as it is.
    pub playable: bool,
}

#[tauri::command]
pub async fn media_probe(path: String) -> Result<Probe, String> {
    blocking(move || probe(Path::new(&path))).await
}

pub fn probe(path: &Path) -> Result<Probe, String> {
    let mut command = ffprobe()?;
    command.args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams", "--"]).arg(path);
    let done = command.output().map_err(|e| format!("Couldn't start ffprobe: {e}"))?;
    if !done.status.success() {
        return Err(String::from_utf8_lossy(&done.stderr).lines().last().unwrap_or("ffprobe couldn't read it").trim().to_string());
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    Ok(read_probe(&String::from_utf8_lossy(&done.stdout), &ext))
}

/// ffprobe's JSON, read; `ext` is the file's extension (WebView2 goes by the container too).
fn read_probe(json: &str, ext: &str) -> Probe {
    let value: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let streams = value["streams"].as_array().cloned().unwrap_or_default();
    let of = |kind: &str| streams.iter().find(|s| s["codec_type"] == kind && s["disposition"]["attached_pic"] != 1);
    let mut probe = Probe {
        duration: value["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0),
        container: value["format"]["format_name"].as_str().unwrap_or_default().to_string(),
        bitrate: value["format"]["bit_rate"].as_str().and_then(|b| b.parse().ok()).unwrap_or(0),
        ..Probe::default()
    };
    if let Some(v) = of("video") {
        probe.video = v["codec_name"].as_str().unwrap_or_default().to_string();
        probe.width = v["width"].as_u64().unwrap_or(0) as u32;
        probe.height = v["height"].as_u64().unwrap_or(0) as u32;
        probe.fps = v["avg_frame_rate"].as_str().and_then(rate).or_else(|| v["r_frame_rate"].as_str().and_then(rate)).unwrap_or(0.0);
    }
    if let Some(a) = of("audio") {
        probe.audio = a["codec_name"].as_str().unwrap_or_default().to_string();
    }
    let container_ok = matches!(ext, "mp4" | "m4v" | "mov" | "webm" | "m4a" | "mp3" | "ogg" | "opus" | "wav" | "flac" | "aac" | "oga");
    let video_ok = probe.video.is_empty() || PLAYS_VIDEO.contains(&probe.video.as_str());
    let audio_ok = probe.audio.is_empty() || PLAYS_AUDIO.contains(&probe.audio.as_str());
    probe.playable = container_ok && video_ok && audio_ok && !(probe.video.is_empty() && probe.audio.is_empty());
    probe
}

/// "30000/1001" as frames a second.
fn rate(text: &str) -> Option<f64> {
    let (n, d) = text.split_once('/')?;
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    (d > 0.0 && n > 0.0).then(|| n / d)
}

/// The encoders each format gets on this PC, the first that works (hardware first).
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Encoders {
    pub h264: Option<String>,
    pub hevc: Option<String>,
    pub av1: Option<String>,
    pub webp: bool,
    /// The hardware encoders that took a test frame.
    pub hardware: Vec<String>,
}

static ENCODERS: Mutex<Option<Encoders>> = Mutex::new(None);

#[tauri::command]
pub async fn media_encoders() -> Result<Encoders, String> {
    blocking(encoders).await
}

pub fn encoders() -> Result<Encoders, String> {
    if let Some(known) = ENCODERS.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Ok(known);
    }
    let mut list = ffmpeg()?;
    list.args(["-hide_banner", "-encoders"]);
    let done = list.output().map_err(|e| format!("Couldn't start FFmpeg: {e}"))?;
    let names: Vec<String> = String::from_utf8_lossy(&done.stdout).lines().filter_map(|l| l.split_whitespace().nth(1).map(str::to_string)).collect();
    let has = |n: &str| names.iter().any(|x| x == n);
    let works = |n: &str| has(n) && test_encoder(n);
    let mut found = Encoders { webp: has("libwebp"), ..Encoders::default() };
    for (format, candidates) in [
        ("h264", ["h264_nvenc", "h264_qsv", "h264_amf"]),
        ("hevc", ["hevc_nvenc", "hevc_qsv", "hevc_amf"]),
        ("av1", ["av1_nvenc", "av1_qsv", "av1_amf"]),
    ] {
        let hardware = candidates.into_iter().find(|c| works(c)).map(str::to_string);
        if let Some(h) = &hardware {
            found.hardware.push(h.clone());
        }
        let software = match format {
            "h264" => has("libx264").then(|| "libx264".to_string()),
            "hevc" => has("libx265").then(|| "libx265".to_string()),
            _ => has("libsvtav1").then(|| "libsvtav1".to_string()),
        };
        let pick = hardware.or(software);
        match format {
            "h264" => found.h264 = pick,
            "hevc" => found.hevc = pick,
            _ => found.av1 = pick,
        }
    }
    *ENCODERS.lock().unwrap_or_else(|e| e.into_inner()) = Some(found.clone());
    Ok(found)
}

/// Whether a hardware encoder takes a few frames here (FFmpeg lists them whether the PC has the hardware or not).
fn test_encoder(name: &str) -> bool {
    let Ok(mut test) = ffmpeg() else { return false };
    let pixels = if name.ends_with("_qsv") { "nv12" } else { "yuv420p" };
    test.args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=320x240:r=30:d=0.3", "-pix_fmt", pixels, "-c:v", name, "-frames:v", "5", "-f", "null", "-"]);
    let stop = AtomicBool::new(false);
    actions::run_tool(test, name, &stop, |_| {}).is_ok()
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum VideoFormat {
    H264,
    Hevc,
    Av1,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Quality {
    /// Good enough to share, small.
    Good,
    /// Close to the original.
    High,
    /// Under this many megabytes (MiB), all of it.
    Size { mb: f64 },
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    /// As it's stored, without re-encoding (the container chosen by its codec).
    Same,
    Mp3,
    M4a,
    Opus,
    Flac,
    Wav,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Task {
    /// From `start` to `end` (seconds): copied as stored (cut at the keyframe at or before the start), or
    /// re-encoded to start on the very frame.
    Trim { start: f64, end: f64, exact: bool },
    /// A video in another format, at a quality or a size, its height capped.
    Convert { format: VideoFormat, quality: Quality, height: Option<u32> },
    /// The sound alone.
    Audio { format: AudioFormat },
    /// An animated GIF (or WebP), `width` pixels wide at `fps`, of a part or all.
    Gif { width: u32, fps: u32, start: Option<f64>, end: Option<f64>, webp: bool },
    /// What's said, as an .srt beside it, in its language or translated to English.
    Subtitles { translate: bool },
    /// The volume evened out (EBU R128, measured then adjusted in one even step): speech and podcasts at
    /// -16 LUFS, music at -14.
    Loudness { music: bool },
}

impl Task {
    fn tag(&self) -> &'static str {
        match self {
            Task::Trim { .. } => "trim",
            Task::Convert { .. } => "convert",
            Task::Audio { .. } => "audio",
            Task::Gif { .. } => "gif",
            Task::Subtitles { .. } => "subtitles",
            Task::Loudness { .. } => "loudness",
        }
    }
}

/// Runs `task` on each of `files`, a job a file, as `output` says.
#[tauri::command]
pub async fn media_run(window: Window, files: Vec<String>, task: Task, output: Output, on_progress: Channel<Progress>) -> Result<Report, String> {
    let owner = crate::shell::owner(&window);
    STOP.store(false, Ordering::SeqCst);
    blocking(move || {
        ffmpeg()?;
        if matches!(task, Task::Subtitles { .. }) && !crate::speech::installed() {
            return Err("Subtitles need speech search's model: turn on speech search in Settings → Search, or install it from here.".into());
        }
        let jobs: Vec<Job> = files.iter().map(Job::one).collect();
        // Videos keep the CPU (or the GPU) busy on their own: one at a time; sounds, a few.
        let at_once = match task {
            Task::Audio { .. } | Task::Loudness { .. } => 3,
            Task::Trim { exact: false, .. } => 2,
            _ => 1,
        };
        actions::run(owner, task.tag(), &jobs, &output, at_once, &STOP, &|p| drop(on_progress.send(p)), |job, part, work| make(&task, &job.inputs[0], part, work))
    })
    .await
}

/// Runs one job of `task` on `file` into `work`.
fn make(task: &Task, file: &Path, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<Vec<PathBuf>, String> {
    let info = probe(file)?;
    match *task {
        Task::Trim { start, end, exact } => trim(file, &info, start, end, exact, part, work).map(|f| vec![f]),
        Task::Convert { format, quality, height } => convert(file, &info, format, quality, height, part, work).map(|f| vec![f]),
        Task::Audio { format } => audio(file, &info, format, part, work).map(|f| vec![f]),
        Task::Gif { width, fps, start, end, webp } => gif(file, &info, width, fps, start, end, webp, part, work).map(|f| vec![f]),
        Task::Subtitles { translate } => subtitles(file, &info, translate, part, work).map(|f| vec![f]),
        Task::Loudness { music } => loudness(file, &info, music, part, work).map(|f| vec![f]),
    }
}

/// FFmpeg with what every run gets: no prompts, quiet, progress as key=value lines on stdout.
fn base() -> Result<Command, String> {
    let mut command = ffmpeg()?;
    command.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-progress", "pipe:1", "-nostats"]);
    Ok(command)
}

/// Runs FFmpeg, turning its `out_time_us` into how far along `seconds` of output it is.
fn run(command: Command, seconds: f64, part: &(dyn Fn(f32) + Sync)) -> Result<(), String> {
    actions::run_tool(command, "FFmpeg", &STOP, |line| {
        if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
            if seconds > 0.0 {
                part((us / 1e6 / seconds) as f32);
            }
        }
    })
    .map(drop)
}

fn trim(file: &Path, info: &Probe, start: f64, end: f64, exact: bool, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<PathBuf, String> {
    let (start, end) = (start.max(0.0), if info.duration > 0.0 { end.min(info.duration) } else { end });
    if end - start < 0.05 {
        return Err("The part to keep is empty".into());
    }
    let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "mp4".into());
    let video = !info.video.is_empty();
    let mut command = base()?;
    let out;
    if exact && video {
        out = actions::renamed(work, file, "mp4");
        let encoders = encoders()?;
        let encoder = encoders.h264.clone().ok_or("This FFmpeg has no H.264 encoder")?;
        command.args(["-ss", &secs(start), "-i"]).arg(file).args(["-t", &secs(end - start), "-map", "0:v:0", "-map", "0:a:0?", "-map_metadata", "0"]);
        command.args(video_args(&encoder, Quality::High, None, info));
        command.args(["-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"]).arg(&out);
    } else {
        out = actions::renamed(work, file, &ext);
        command.args(["-ss", &secs(start), "-to", &secs(end), "-i"]).arg(file);
        command.args(["-map", "0:v?", "-map", "0:a?", "-map", "0:s?", "-c", "copy", "-map_metadata", "0", "-avoid_negative_ts", "make_zero"]);
        if matches!(ext.as_str(), "mp4" | "m4v" | "mov" | "m4a") {
            command.args(["-movflags", "+faststart"]);
        }
        command.arg(&out);
    }
    run(command, end - start, part)?;
    Ok(out)
}

fn secs(s: f64) -> String {
    format!("{s:.3}")
}

/// The video encoder's arguments for a quality (or a bitrate, for a size), the height capped at `height`.
fn video_args(encoder: &str, quality: Quality, bitrate: Option<u64>, info: &Probe) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["-c:v".into(), encoder.into()];
    let high = quality == Quality::High;
    let family = encoder.rsplit('_').next().unwrap_or(encoder);
    match (bitrate, family) {
        (Some(b), _) if encoder.starts_with("lib") => args.extend(["-b:v".into(), b.to_string().into()]),
        (Some(b), "nvenc") => args.extend(["-rc".into(), "vbr".into(), "-b:v".into(), b.to_string().into(), "-maxrate".into(), b.to_string().into(), "-bufsize".into(), (b * 2).to_string().into()]),
        (Some(b), "amf") => args.extend(["-rc".into(), "vbr_peak".into(), "-b:v".into(), b.to_string().into(), "-maxrate".into(), b.to_string().into()]),
        (Some(b), _) => args.extend(["-b:v".into(), b.to_string().into(), "-maxrate".into(), b.to_string().into()]),
        (None, "libx264") => args.extend(["-crf".into(), if high { "19" } else { "23" }.into(), "-preset".into(), if high { "slow" } else { "medium" }.into()]),
        (None, "libx265") => args.extend(["-crf".into(), if high { "23" } else { "28" }.into(), "-preset".into(), "medium".into()]),
        (None, "libsvtav1") => args.extend(["-crf".into(), if high { "28" } else { "35" }.into(), "-preset".into(), "8".into()]),
        (None, "nvenc") => args.extend(["-rc".into(), "vbr".into(), "-cq".into(), if high { "21" } else { "27" }.into(), "-b:v".into(), "0".into(), "-preset".into(), "p5".into()]),
        (None, "qsv") => args.extend(["-global_quality".into(), if high { "21" } else { "27" }.into()]),
        (None, "amf") => {
            let qp = if high { "20" } else { "26" };
            args.extend(["-rc".into(), "cqp".into(), "-qp_i".into(), qp.into(), "-qp_p".into(), qp.into()]);
        }
        _ => {}
    }
    if encoder == "libx264" {
        args.extend(["-pix_fmt".into(), "yuv420p".into()]);
    } else if encoder.ends_with("_qsv") {
        args.extend(["-pix_fmt".into(), "nv12".into()]);
    }
    // Apple's players want HEVC tagged hvc1.
    if encoder.contains("265") || encoder.starts_with("hevc") {
        args.extend(["-tag:v".into(), "hvc1".into()]);
    }
    let _ = info;
    args
}

fn convert(file: &Path, info: &Probe, format: VideoFormat, quality: Quality, height: Option<u32>, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<PathBuf, String> {
    if info.video.is_empty() {
        return Err("It has no video in it".into());
    }
    let encoders = encoders()?;
    let encoder = match format {
        VideoFormat::H264 => encoders.h264.clone(),
        VideoFormat::Hevc => encoders.hevc.clone(),
        VideoFormat::Av1 => encoders.av1.clone(),
    }
    .ok_or("This FFmpeg has no encoder for that format")?;
    let out = actions::renamed(work, file, "mp4");
    // A height only ever shrinks it.
    let scale = height.filter(|h| info.height > *h).map(|h| format!("scale=-2:{h}"));
    let (bitrate, audio_rate) = match quality {
        Quality::Size { mb } => {
            if info.duration <= 0.0 {
                return Err("Its length isn't known, so its size can't be worked out".into());
            }
            let rates = bitrates(mb, info.duration);
            (Some(rates.0), rates.1)
        }
        _ => (None, 160_000),
    };
    let encode = |bitrate: Option<u64>, pass: Option<u8>| -> Result<(), String> {
        let mut command = base()?;
        command.arg("-i").arg(file).args(["-map", "0:v:0", "-map_metadata", "0"]);
        if pass != Some(1) {
            command.args(["-map", "0:a:0?"]);
        }
        if let Some(s) = &scale {
            command.args(["-vf", s]);
        }
        command.args(video_args(&encoder, quality, bitrate, info));
        if let Some(p) = pass {
            command.args(["-pass", &p.to_string(), "-passlogfile"]).arg(work.join("pass"));
        }
        if pass == Some(1) {
            command.args(["-an", "-f", "null", "NUL"]);
        } else {
            command.args(["-c:a", "aac", "-b:a", &audio_rate.to_string(), "-movflags", "+faststart"]).arg(&out);
        }
        let share = |p: f32| match pass {
            Some(1) => part(p * 0.5),
            Some(_) => part(0.5 + p * 0.5),
            None => part(p),
        };
        run(command, info.duration, &share)
    };
    match bitrate {
        // x264 and x265 reach a size best in two passes; the hardware ones in one, checked after.
        Some(b) if encoder.starts_with("lib") && encoder != "libsvtav1" => {
            encode(Some(b), Some(1))?;
            encode(Some(b), Some(2))?;
        }
        Some(b) => {
            encode(Some(b), None)?;
            let target = target_bytes(quality);
            let got = fs::metadata(&out).map_or(0, |m| m.len());
            if target > 0 && got > target {
                // Over: once more, as much smaller as it was over (and a little more).
                let lower = (b as f64 * target as f64 / got as f64 * 0.95) as u64;
                encode(Some(lower.max(50_000)), None)?;
            }
        }
        None => encode(None, None)?,
    }
    Ok(out)
}

fn target_bytes(quality: Quality) -> u64 {
    match quality {
        Quality::Size { mb } => (mb * 1024.0 * 1024.0) as u64,
        _ => 0,
    }
}

/// The video's and the sound's bits a second for `mb` MiB over `seconds`, a little kept back for the
/// container (and never below what still makes a picture).
fn bitrates(mb: f64, seconds: f64) -> (u64, u64) {
    let total = mb * 1024.0 * 1024.0 * 8.0 * 0.96 / seconds;
    let audio = if total < 600_000.0 { 64_000.0 } else if total < 1_500_000.0 { 96_000.0 } else { 128_000.0 };
    ((total - audio).max(60_000.0) as u64, audio as u64)
}

/// Two passes of FFmpeg's loudnorm: the first measures (its numbers come as JSON on its error output, at
/// the info level), the second applies them as one even change of volume (`linear`), so nothing pumps.
/// Video is copied as it is; the sound is written as the file had it where FFmpeg writes that, else AAC.
fn loudness(file: &Path, info: &Probe, music: bool, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<PathBuf, String> {
    if info.audio.is_empty() {
        return Err("It has no sound in it".into());
    }
    let target = format!("loudnorm=I={}:TP=-1.5:LRA=11", if music { -14 } else { -16 });
    let mut measure = ffmpeg()?;
    measure.args(["-nostdin", "-hide_banner", "-loglevel", "info", "-progress", "pipe:1", "-nostats", "-i"]).arg(file);
    measure.args(["-map", "0:a:0", "-af", &format!("{target}:print_format=json"), "-f", "null", "NUL"]);
    let said = actions::run_tool(measure, "FFmpeg", &STOP, |line| {
        if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
            if info.duration > 0.0 {
                part((us / 1e6 / info.duration * 0.5) as f32);
            }
        }
    })?;
    let measured = loudness_measured(&said.stderr).ok_or("FFmpeg didn't say how loud it is")?;
    let ext = file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let video = !info.video.is_empty();
    let (out_ext, codec): (&str, &[&str]) = match ext.as_str() {
        _ if video => (if matches!(ext.as_str(), "mkv" | "webm" | "mov") { ext.as_str() } else { "mp4" }, &["-c:a", "aac", "-b:a", "192k"]),
        "mp3" => ("mp3", &["-c:a", "libmp3lame", "-q:a", "2"]),
        "flac" => ("flac", &["-c:a", "flac"]),
        "wav" => ("wav", &["-c:a", "pcm_s16le"]),
        "ogg" | "oga" => ("ogg", &["-c:a", "libvorbis", "-q:a", "6"]),
        "opus" => ("opus", &["-c:a", "libopus", "-b:a", "128k"]),
        _ => ("m4a", &["-c:a", "aac", "-b:a", "192k"]),
    };
    let out = actions::renamed(work, file, out_ext);
    let mut apply = base()?;
    apply.arg("-i").arg(file).args(["-map", "0:v?", "-map", "0:a:0", "-map_metadata", "0", "-c:v", "copy", "-af"]).arg(format!("{target}:{measured}:linear=true:print_format=none"));
    // loudnorm works at 192 kHz: back to the rate songs and videos use.
    apply.args(codec).args(["-ar", "48000"]);
    if out_ext == "mp4" || out_ext == "m4a" {
        apply.args(["-movflags", "+faststart"]);
    }
    apply.arg(&out);
    run(apply, info.duration, &|p| part(0.5 + p * 0.5))?;
    Ok(out)
}

/// The first pass's numbers as loudnorm's second pass takes them, from the JSON at the end of its output.
fn loudness_measured(stderr: &str) -> Option<String> {
    let start = stderr.rfind('{')?;
    let end = stderr[start..].find('}')? + start;
    let value: serde_json::Value = serde_json::from_str(&stderr[start..=end]).ok()?;
    let get = |key: &str| value[key].as_str().filter(|v| v.parse::<f64>().is_ok_and(f64::is_finite));
    Some(format!("measured_I={}:measured_TP={}:measured_LRA={}:measured_thresh={}:offset={}", get("input_i")?, get("input_tp")?, get("input_lra")?, get("input_thresh")?, get("target_offset")?))
}

fn audio(file: &Path, info: &Probe, format: AudioFormat, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<PathBuf, String> {
    if info.audio.is_empty() {
        return Err("It has no sound in it".into());
    }
    let (ext, codec): (&str, &[&str]) = match format {
        AudioFormat::Same => (same_container(&info.audio).ok_or_else(|| format!("Its sound ({}) can't be kept as it is: pick a format", info.audio))?, &["-c:a", "copy"]),
        AudioFormat::Mp3 => ("mp3", &["-c:a", "libmp3lame", "-q:a", "2"]),
        AudioFormat::M4a => ("m4a", &["-c:a", "aac", "-b:a", "192k"]),
        AudioFormat::Opus => ("opus", &["-c:a", "libopus", "-b:a", "128k"]),
        AudioFormat::Flac => ("flac", &["-c:a", "flac"]),
        AudioFormat::Wav => ("wav", &["-c:a", "pcm_s16le"]),
    };
    let out = actions::renamed(work, file, ext);
    let mut command = base()?;
    command.arg("-i").arg(file).args(["-vn", "-map", "0:a:0", "-map_metadata", "0"]).args(codec).arg(&out);
    run(command, info.duration, part)?;
    Ok(out)
}

/// The file type a sound codec is kept in as it is.
fn same_container(codec: &str) -> Option<&'static str> {
    Some(match codec {
        "aac" | "alac" => "m4a",
        "mp3" => "mp3",
        "opus" => "opus",
        "vorbis" => "ogg",
        "flac" => "flac",
        "ac3" => "ac3",
        "eac3" => "eac3",
        "dts" => "dts",
        c if c.starts_with("pcm_") => "wav",
        _ => return None,
    })
}

#[allow(clippy::too_many_arguments)]
fn gif(file: &Path, info: &Probe, width: u32, fps: u32, start: Option<f64>, end: Option<f64>, webp: bool, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<PathBuf, String> {
    if info.video.is_empty() {
        return Err("It has no video in it".into());
    }
    let (fps, width) = (fps.clamp(1, 50), width.clamp(16, 3840));
    let start = start.unwrap_or(0.0).max(0.0);
    let end = end.filter(|e| *e > start).unwrap_or(info.duration);
    let length = (end - start).max(0.0);
    let mut command = base()?;
    command.args(["-ss", &secs(start), "-t", &secs(length), "-i"]).arg(file);
    let out = if webp {
        let out = actions::renamed(work, file, "webp");
        command.args(["-vf", &format!("fps={fps},scale={width}:-2:flags=lanczos"), "-c:v", "libwebp", "-lossless", "0", "-q:v", "75", "-loop", "0", "-an"]).arg(&out);
        out
    } else {
        let out = actions::renamed(work, file, "gif");
        let filter = format!("fps={fps},scale={width}:-2:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=sierra2_4a");
        command.args(["-filter_complex", &filter, "-loop", "0", "-an"]).arg(&out);
        out
    };
    run(command, length, part)?;
    Ok(out)
}

fn subtitles(file: &Path, info: &Probe, translate: bool, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<PathBuf, String> {
    if info.audio.is_empty() {
        return Err("It has no sound in it".into());
    }
    let wav = work.join("sound.wav");
    let mut command = base()?;
    command.arg("-i").arg(file).args(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"]).arg(&wav);
    run(command, info.duration, &|p| part(p * 0.1))?;
    let model = crate::speech::model_path().ok_or("Speech search's model isn't installed")?;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "subtitles".into());
    let base_out = work.join(&stem);
    let mut whisper = crate::tools::command("whisper")?;
    whisper.arg("-m").arg(&model).arg("-f").arg(&wav).args(["-l", "auto", "-osrt", "-pp", "-t", &threads.to_string()]).arg("-of").arg(&base_out);
    if translate {
        whisper.arg("-tr");
    }
    // whisper-cli prints "whisper_print_progress_callback: progress =  45%" on its error output, so the
    // sound's length stands in: progress is shown from the parts it prints as it goes.
    let length = info.duration.max(1.0);
    actions::run_tool(whisper, "whisper.cpp", &STOP, |line| {
        // "[00:01:02.000 --> 00:01:05.000]  text"
        if let Some(at) = line.strip_prefix('[').and_then(|l| l.split(" --> ").nth(1)).and_then(|l| l.split(']').next()).and_then(clock_secs) {
            part(0.1 + 0.9 * (at / length) as f32);
        }
    })?;
    let made = base_out.with_extension("srt");
    if !made.is_file() {
        return Err("whisper.cpp made no subtitles".into());
    }
    // Named as video players look for it: beside the video, with its name.
    let named = work.join(format!("{stem}{}.srt", if translate { ".en" } else { "" }));
    if named != made {
        fs::rename(&made, &named).map_err(|e| e.to_string())?;
    }
    Ok(named)
}

/// "00:01:02.500" as seconds.
fn clock_secs(text: &str) -> Option<f64> {
    let mut total = 0.0;
    for part in text.trim().split(':') {
        total = total * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(total)
}

/// A copy of the video at `path` that WebView2 plays, made once (and kept while its source is
/// unchanged): its streams remuxed into MP4 as they are when WebView2 decodes them, else converted
/// (H.264 and AAC, 1080p at most, quickly). What it says on the way is how far along it is.
#[tauri::command]
pub async fn media_playable(path: String, on_progress: Channel<f32>) -> Result<String, String> {
    STOP.store(false, Ordering::SeqCst);
    blocking(move || playable(Path::new(&path), &|p| drop(on_progress.send(p)))).await
}

fn playable(path: &Path, part: &(dyn Fn(f32) + Sync)) -> Result<String, String> {
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    let info = probe(path)?;
    if info.video.is_empty() && info.audio.is_empty() {
        return Err("FFmpeg finds no video or sound in it".into());
    }
    let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let key = format!("{:x}", seahash(&format!("{}|{}|{modified}", path.to_string_lossy().to_lowercase(), meta.len())));
    let dir = std::env::temp_dir().join("Glint").join("playable");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let audio_only = info.video.is_empty();
    let out = dir.join(format!("{key}.{}", if audio_only { "m4a" } else { "mp4" }));
    if out.is_file() {
        return Ok(out.to_string_lossy().into_owned());
    }
    let partial = dir.join(format!("{key}.part.{}", if audio_only { "m4a" } else { "mp4" }));
    let video_copies = info.video.is_empty() || PLAYS_VIDEO.contains(&info.video.as_str());
    let audio_copies = info.audio.is_empty() || ["aac", "mp3", "opus", "flac"].contains(&info.audio.as_str());
    let mut command = base()?;
    command.arg("-i").arg(path);
    if !audio_only {
        command.args(["-map", "0:v:0"]);
    }
    command.args(["-map", "0:a:0?"]);
    if video_copies {
        command.args(["-c:v", "copy"]);
    } else {
        let encoder = encoders()?.h264.unwrap_or_else(|| "libx264".into());
        command.args(video_args(&encoder, Quality::Good, None, &info));
        if encoder == "libx264" {
            command.args(["-preset", "veryfast"]);
        }
        if info.height > 1080 {
            command.args(["-vf", "scale=-2:1080"]);
        }
    }
    if audio_copies {
        command.args(["-c:a", "copy"]);
    } else {
        command.args(["-c:a", "aac", "-b:a", "192k"]);
    }
    command.args(["-movflags", "+faststart"]).arg(&partial);
    if let Err(e) = run(command, info.duration, part) {
        let _ = fs::remove_file(&partial);
        return Err(e);
    }
    fs::rename(&partial, &out).map_err(|e| e.to_string())?;
    Ok(out.to_string_lossy().into_owned())
}

/// A small, stable hash for cache names.
pub(crate) fn seahash(text: &str) -> u64 {
    // FNV-1a: enough to name cache files.
    text.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_loudnorms_numbers() {
        let stderr = "[Parsed_loudnorm_0 @ 000001] 
{
	\"input_i\" : \"-27.61\",
	\"input_tp\" : \"-4.47\",
	\"input_lra\" : \"18.06\",
	\"input_thresh\" : \"-39.20\",
	\"output_i\" : \"-16.58\",
	\"target_offset\" : \"0.58\"
}
";
        assert_eq!(loudness_measured(stderr).as_deref(), Some("measured_I=-27.61:measured_TP=-4.47:measured_LRA=18.06:measured_thresh=-39.20:offset=0.58"));
        // Silence measures as -inf: no second pass from that.
        assert_eq!(loudness_measured("{\"input_i\" : \"-inf\", \"input_tp\" : \"-inf\", \"input_lra\" : \"0.00\", \"input_thresh\" : \"-70.00\", \"target_offset\" : \"inf\"}"), None);
    }

    #[test]
    fn reads_what_ffprobe_says() {
        let json = r#"{"streams":[{"codec_type":"video","codec_name":"hevc","width":3840,"height":2160,"avg_frame_rate":"30000/1001"},{"codec_type":"audio","codec_name":"aac"},{"codec_type":"video","codec_name":"mjpeg","disposition":{"attached_pic":1}}],"format":{"duration":"62.5","format_name":"mov,mp4,m4a,3gp,3g2,mj2","bit_rate":"8000000"}}"#;
        let p = read_probe(json, "mp4");
        assert_eq!((p.width, p.height, p.video.as_str(), p.audio.as_str()), (3840, 2160, "hevc", "aac"));
        assert!((p.fps - 29.97).abs() < 0.01 && (p.duration - 62.5).abs() < 1e-9);
        // HEVC needs Windows' extension: not counted as playable.
        assert!(!p.playable);
        let h264 = json.replace("hevc", "h264");
        assert!(read_probe(&h264, "mp4").playable);
        // An MKV isn't played whatever it holds.
        assert!(!read_probe(&h264, "mkv").playable);
        let song = r#"{"streams":[{"codec_type":"audio","codec_name":"mp3"}],"format":{"duration":"200"}}"#;
        assert!(read_probe(song, "mp3").playable);
    }

    #[test]
    fn works_out_bitrates_for_a_size() {
        // 25 MiB over 100 s: about 2 Mb/s in all, 128 kb/s of it sound.
        let (video, audio) = bitrates(25.0, 100.0);
        assert_eq!(audio, 128_000);
        assert!((1_850_000..2_000_000).contains(&video), "{video}");
        // Tiny targets keep a picture and give the sound less.
        let (video, audio) = bitrates(1.0, 600.0);
        assert_eq!((video, audio), (60_000, 64_000));
    }

    #[test]
    fn picks_encoder_settings() {
        let info = Probe::default();
        let line = |e: &str, q: Quality, b: Option<u64>| video_args(e, q, b, &info).iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
        assert_eq!(line("libx264", Quality::Good, None), "-c:v libx264 -crf 23 -preset medium -pix_fmt yuv420p");
        assert_eq!(line("h264_nvenc", Quality::High, None), "-c:v h264_nvenc -rc vbr -cq 21 -b:v 0 -preset p5");
        assert_eq!(line("hevc_qsv", Quality::Good, None), "-c:v hevc_qsv -global_quality 27 -pix_fmt nv12 -tag:v hvc1");
        assert_eq!(line("libx265", Quality::Size { mb: 10.0 }, Some(900_000)), "-c:v libx265 -b:v 900000 -tag:v hvc1");
        assert_eq!(line("h264_amf", Quality::Size { mb: 10.0 }, Some(900_000)), "-c:v h264_amf -rc vbr_peak -b:v 900000 -maxrate 900000");
    }

    #[test]
    fn keeps_sound_as_it_is_where_it_can() {
        assert_eq!(same_container("aac"), Some("m4a"));
        assert_eq!(same_container("pcm_s24le"), Some("wav"));
        assert_eq!(same_container("wmav2"), None);
        assert_eq!(clock_secs("00:01:02.500"), Some(62.5));
        assert_eq!(rate("0/0"), None);
    }
}
