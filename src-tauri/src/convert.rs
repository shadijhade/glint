//! Convert pictures (Convert pictures… on a picture's menu, the Tools menu and Ctrl+K) with rimage,
//! installed on demand: to JPEG (MozJPEG), PNG (OxiPNG, lossless), WebP or AVIF, at a quality, made
//! smaller if asked, their metadata kept or taken out. Each picture is a rimage process of its own, a few
//! at a time, into a hidden folder beside it; then each result goes next to its picture with a suffix,
//! into a folder there, or in its place, the pictures recycled together (the UI takes it all back as one
//! step, from what this says it did).

use crate::fsx::{self, blocking};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tauri::Window;

/// What rimage reads.
pub const READS: &[&str] = &["avif", "bmp", "ff", "hdr", "jpg", "jpeg", "jpe", "jfif", "jxl", "png", "ppm", "pgm", "pbm", "psd", "qoi", "svg", "tif", "tiff", "webp"];

/// The most pictures converted at once (each a rimage process, holding one picture in memory).
const AT_ONCE: usize = 4;

#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Jpeg,
    Png,
    Webp,
    Avif,
}

impl Format {
    /// rimage's encoder for it, and the extension it gives what it makes.
    fn codec(self) -> (&'static str, &'static str) {
        match self {
            Format::Jpeg => ("mozjpeg", "jpg"),
            Format::Png => ("oxipng", "png"),
            Format::Webp => ("webp", "webp"),
            Format::Avif => ("avif", "avif"),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Resize {
    /// This much of its width and height.
    Percent { value: u32 },
    /// Its longest side this many pixels at most (a smaller picture keeps its size).
    Longest { value: u32 },
}

/// Where what's made goes.
#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Output {
    /// Next to the picture, its name with `suffix` after it.
    Beside { suffix: String },
    /// Into a folder of this name next to the picture.
    Folder { name: String },
    /// In the picture's place, the picture recycled.
    Replace,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConvertOptions {
    pub format: Format,
    /// 1 to 100; PNG has none (OxiPNG is lossless).
    pub quality: u8,
    pub resize: Option<Resize>,
    /// Takes out what the picture says of itself: the camera, the place, when it was taken.
    pub strip: bool,
    pub output: Output,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConvertProgress {
    pub done: usize,
    pub total: usize,
    /// The picture just finished.
    pub current: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Converted {
    pub from: String,
    pub to: String,
    /// Where the picture went in the Recycle Bin, when what was made took its place.
    pub recycled: Option<String>,
    pub before: u64,
    pub after: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConvertReport {
    pub converted: Vec<Converted>,
    /// Each picture that wasn't converted, and why.
    pub failed: Vec<(String, String)>,
    pub stopped: bool,
    pub elapsed_ms: f64,
}

static STOP: AtomicBool = AtomicBool::new(false);

/// Stops the conversion running: the pictures being converted are left as they were.
#[tauri::command]
pub fn convert_cancel() {
    STOP.store(true, Ordering::SeqCst);
}

/// Converts `pictures` as `options` say, a few at a time, saying how far it is after each.
#[tauri::command]
pub async fn convert_images(window: Window, pictures: Vec<String>, options: ConvertOptions, on_progress: Channel<ConvertProgress>) -> Result<ConvertReport, String> {
    let owner = crate::shell::owner(&window);
    STOP.store(false, Ordering::SeqCst);
    blocking(move || convert(owner, &pictures, &options, &|progress| drop(on_progress.send(progress)))).await
}

/// A name Windows takes for a file or a folder (`suffix` may be empty).
fn valid_name(name: &str, empty: bool) -> bool {
    (empty || !name.trim().is_empty()) && !name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) && !name.ends_with(['.', ' '])
}

/// rimage's arguments for `picture`, into the folder `out`.
fn arguments(options: &ConvertOptions, picture: &Path, out: &Path) -> Vec<OsString> {
    let (codec, _) = options.format.codec();
    let mut args: Vec<OsString> = vec![codec.into()];
    if options.format != Format::Png {
        args.extend(["-q".into(), options.quality.clamp(1, 100).to_string().into()]);
    }
    match options.resize {
        Some(Resize::Percent { value }) => args.extend(["--resize".into(), format!("{}%", value.clamp(1, 1000)).into()]),
        Some(Resize::Longest { value }) => args.extend(["--resize".into(), format!("{}l", value.clamp(1, 65_535)).into(), "--reduce-only".into()]),
        None => {}
    }
    if options.strip {
        args.push("-x".into());
    }
    // Its date modified kept; the picture last, after "--" in case its name starts with "-".
    args.extend(["-p".into(), "--no-progress".into(), "-d".into(), out.into(), "--".into(), picture.into()]);
    args
}

/// What rimage said went wrong, in its words, without the path it names.
fn said(stderr: &str, picture: &Path) -> Option<String> {
    let path = picture.to_string_lossy();
    stderr
        .lines()
        .filter(|l| l.contains("ERROR"))
        .map(|l| l.split_once("] ").map_or(l, |(_, rest)| rest).trim())
        .map(|l| l.replace(&format!("input: {path} "), "").replace(path.as_ref(), "the picture"))
        .map(|l| l.split(" [").next().unwrap_or_default().trim().to_string())
        .find(|l| !l.is_empty() && !l.contains("file(s) failed"))
}

/// Converts one picture into `work` (a folder of its own): what it made.
fn convert_one(options: &ConvertOptions, picture: &Path, work: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let mut rimage = crate::tools::command("rimage")?;
    rimage.args(arguments(options, picture, work)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = rimage.spawn().map_err(|e| format!("Couldn't start rimage: {e}"))?;
    let mut stderr = child.stderr.take().ok_or("rimage has no output")?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let status = loop {
        if STOP.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cancelled".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let stderr = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(said(&stderr, picture).unwrap_or_else(|| format!("rimage stopped with exit code {}", status.code().unwrap_or(-1))));
    }
    let stem = picture.file_stem().ok_or("The picture has no name")?;
    let made = work.join(stem).with_extension(options.format.codec().1);
    if made.is_file() {
        Ok(made)
    } else {
        Err("rimage made nothing".into())
    }
}

/// The folder and name what's made of `picture` goes to (made free with " (2)" and so on when placed).
fn destination(picture: &Path, options: &ConvertOptions) -> (PathBuf, String) {
    let folder = picture.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = picture.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = options.format.codec().1;
    match &options.output {
        Output::Beside { suffix } => (folder, format!("{stem}{suffix}.{ext}")),
        Output::Folder { name } => (folder.join(name), format!("{stem}.{ext}")),
        Output::Replace => (folder, format!("{stem}.{ext}")),
    }
}

/// Puts `made` at `folder\name`, or at "name (2)" and so on when that's taken: where it went.
fn place(made: &Path, folder: &Path, name: &str) -> Result<String, String> {
    fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    // A file of the name, made new (so nothing's overwritten), that the rename then replaces.
    let to = fsx::create_numbered(&folder.to_string_lossy(), name, false)?;
    if let Err(e) = fs::rename(made, &to) {
        let _ = fs::remove_file(&to);
        return Err(fsx::file_error(e));
    }
    Ok(to)
}

fn convert(owner: isize, pictures: &[String], options: &ConvertOptions, progress: &(dyn Fn(ConvertProgress) + Sync)) -> Result<ConvertReport, String> {
    let started = Instant::now();
    match &options.output {
        Output::Beside { suffix } if !valid_name(suffix, true) => return Err(format!("“{suffix}” can't be part of a file name")),
        Output::Folder { name } if !valid_name(name, false) => return Err(format!("“{name}” can't be a folder's name")),
        _ => {}
    }
    crate::tools::command("rimage")?;
    // Each picture into a folder of its own, in a hidden one beside it (the same drive, so it's moved
    // into place rather than copied), which goes once it's done.
    let work_of = |picture: &Path| picture.parent().unwrap_or(Path::new(".")).join(format!(".glint-convert-{}", std::process::id()));
    let sizes: Vec<u64> = pictures.iter().map(|p| fs::metadata(p).map_or(0, |m| m.len())).collect();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Result<PathBuf, String>)>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..AT_ONCE.min(pictures.len()) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(picture) = pictures.get(i).map(Path::new) else { break };
                if STOP.load(Ordering::SeqCst) {
                    break;
                }
                let ext = picture.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                let made = if READS.contains(&ext.as_str()) {
                    let work = work_of(picture);
                    if fs::create_dir_all(&work).is_ok() {
                        hide(&work);
                    }
                    convert_one(options, picture, &work.join(i.to_string()))
                } else {
                    Err(format!("rimage doesn't read .{ext} files"))
                };
                if made.as_ref().err().is_some_and(|e| e == "cancelled") {
                    break;
                }
                results.lock().unwrap_or_else(|e| e.into_inner()).push((i, made));
                let current = picture.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                progress(ConvertProgress { done: done.fetch_add(1, Ordering::SeqCst) + 1, total: pictures.len(), current });
            });
        }
    });
    let stopped = STOP.load(Ordering::SeqCst);
    let mut results = results.into_inner().unwrap_or_else(|e| e.into_inner());
    results.sort_by_key(|(i, _)| *i);

    // In their place: the pictures converted are recycled together first, so their names are free.
    let mut recycled: Vec<Option<String>> = vec![None; pictures.len()];
    if matches!(options.output, Output::Replace) {
        let originals: Vec<String> = results.iter().filter(|(_, made)| made.is_ok()).map(|(i, _)| pictures[*i].clone()).collect();
        if !originals.is_empty() {
            crate::native_preview::release(&originals);
            if let Ok(performed) = crate::shell::recycle(owner, originals) {
                for done in performed.items {
                    if let Some(i) = pictures.iter().position(|p| p.eq_ignore_ascii_case(&done.from)) {
                        recycled[i] = done.to;
                    }
                }
            }
        }
    }

    let (mut converted, mut failed) = (Vec::new(), Vec::new());
    for (i, made) in results {
        let picture = Path::new(&pictures[i]);
        let placed = made.and_then(|made| {
            let (folder, name) = destination(picture, options);
            // Not recycled (it couldn't be, or Windows was told no): beside it after all, with a name of its own.
            place(&made, &folder, &name)
        });
        match placed {
            Ok(to) => {
                let after = fs::metadata(&to).map_or(0, |m| m.len());
                converted.push(Converted { from: pictures[i].clone(), to, recycled: recycled[i].take(), before: sizes[i], after });
            }
            Err(why) => failed.push((pictures[i].clone(), why)),
        }
    }
    for work in pictures.iter().map(|p| work_of(Path::new(p))).collect::<std::collections::BTreeSet<_>>() {
        let _ = fs::remove_dir_all(work);
    }
    Ok(ConvertReport { converted, failed, stopped, elapsed_ms: started.elapsed().as_secs_f64() * 1000.0 })
}

/// Hides the work folder, which only lasts as long as the conversion.
#[cfg(windows)]
fn hide(folder: &Path) {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN};
    let _ = unsafe { SetFileAttributesW(&HSTRING::from(folder.as_os_str()), FILE_ATTRIBUTE_HIDDEN) };
}

#[cfg(not(windows))]
fn hide(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(format: Format, resize: Option<Resize>, output: Output) -> ConvertOptions {
        ConvertOptions { format, quality: 80, resize, strip: false, output }
    }

    #[test]
    fn asks_rimage_for_what_was_chosen() {
        let (picture, out) = (Path::new(r"C:\Photos\-beach.png"), Path::new(r"C:\Photos\.work\0"));
        let line = |o: &ConvertOptions| arguments(o, picture, out).iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
        assert_eq!(line(&options(Format::Jpeg, None, Output::Replace)), r"mozjpeg -q 80 -p --no-progress -d C:\Photos\.work\0 -- C:\Photos\-beach.png");
        // PNG is lossless: no quality. Smaller by a share, or to a longest side (never larger).
        assert_eq!(line(&options(Format::Png, Some(Resize::Percent { value: 50 }), Output::Replace)), r"oxipng --resize 50% -p --no-progress -d C:\Photos\.work\0 -- C:\Photos\-beach.png");
        let mut stripped = options(Format::Avif, Some(Resize::Longest { value: 1920 }), Output::Replace);
        stripped.strip = true;
        stripped.quality = 0;
        assert_eq!(line(&stripped), r"avif -q 1 --resize 1920l --reduce-only -x -p --no-progress -d C:\Photos\.work\0 -- C:\Photos\-beach.png");
    }

    #[test]
    fn names_what_it_makes() {
        let picture = Path::new(r"C:\Photos\beach.png");
        assert_eq!(destination(picture, &options(Format::Webp, None, Output::Beside { suffix: "-small".into() })), (PathBuf::from(r"C:\Photos"), "beach-small.webp".into()));
        assert_eq!(destination(picture, &options(Format::Jpeg, None, Output::Folder { name: "Converted".into() })), (PathBuf::from(r"C:\Photos\Converted"), "beach.jpg".into()));
        assert_eq!(destination(picture, &options(Format::Avif, None, Output::Replace)), (PathBuf::from(r"C:\Photos"), "beach.avif".into()));
        assert!(valid_name("", true) && valid_name(" (web)", true) && !valid_name("", false) && !valid_name("a/b", false) && !valid_name("x.", true));
    }

    #[test]
    fn says_what_rimage_said() {
        let picture = Path::new(r"C:\x\not-an-image.png");
        let stderr = "[ERROR rimage::error] input: C:\\x\\not-an-image.png is a png file, but no decoder for this format is implemented [input.unsupported-format]\n[ERROR rimage::error]   hint: convert the file\n[ERROR rimage] 1/1 file(s) failed (1 reading, 0 writing).";
        assert_eq!(said(stderr, picture).as_deref(), Some("is a png file, but no decoder for this format is implemented"));
        assert_eq!(said("[WARN  rimage] C:\\x\\a.png: failed to read EXIF metadata", picture), None);
    }

    #[test]
    fn places_without_overwriting() {
        let dir = std::env::temp_dir().join(format!("glint-convert-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("work")).unwrap();
        fs::write(dir.join("beach.jpg"), "the one there").unwrap();
        let made = dir.join("work").join("beach.jpg");
        fs::write(&made, "made").unwrap();
        let to = place(&made, &dir, "beach.jpg").unwrap();
        assert!(to.ends_with("beach (2).jpg"), "{to}");
        assert_eq!(fs::read_to_string(dir.join("beach.jpg")).unwrap(), "the one there");
        assert_eq!(fs::read_to_string(&to).unwrap(), "made");
        assert!(!made.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
