//! Writing zip files: Compress to zip. Deflate, as Explorer's own zip folders use, with the files'
//! dates; zip64 only where an entry needs it.

use super::Skipped;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// One thing to put in the zip: where it is, its name in the zip (`/` between folders, a folder's
/// ending in `/`), and its size.
struct Item {
    path: PathBuf,
    name: String,
    dir: bool,
    size: u64,
}

/// Each of `paths` with everything under it, named from the folder it's in. A junction or a symlinked
/// folder isn't followed (it can lead back up, or anywhere).
fn gather(path: &Path, name: String, out: &mut Vec<Item>, skipped: &mut Vec<Skipped>) {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) => return skipped.push(Skipped { path: path.to_string_lossy().into_owned(), reason: e.to_string() }),
    };
    #[cfg(windows)]
    let link = std::os::windows::fs::MetadataExt::file_attributes(&meta) & 0x400 != 0; // FILE_ATTRIBUTE_REPARSE_POINT
    #[cfg(not(windows))]
    let link = meta.file_type().is_symlink();
    if meta.is_dir() || (link && path.is_dir()) {
        if link {
            return;
        }
        out.push(Item { path: path.into(), name: format!("{name}/"), dir: true, size: 0 });
        match fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    gather(&entry.path(), format!("{name}/{}", entry.file_name().to_string_lossy()), out, skipped);
                }
            }
            Err(e) => skipped.push(Skipped { path: path.to_string_lossy().into_owned(), reason: e.to_string() }),
        }
    } else {
        let size = fs::metadata(path).map_or(0, |m| m.len());
        out.push(Item { path: path.into(), name, dir: false, size });
    }
}

/// The zip's date for a file: local time, as the format keeps it (1980 to 2107).
fn zip_time(modified: SystemTime) -> Option<zip::DateTime> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
        use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
        let ticks = modified.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_nanos() as u64 / 100 + 116_444_736_000_000_000;
        let filetime = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
        let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
        unsafe {
            FileTimeToSystemTime(&filetime, &mut utc).ok()?;
            SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
        }
        zip::DateTime::from_date_and_time(local.wYear, local.wMonth as u8, local.wDay as u8, local.wHour as u8, local.wMinute as u8, local.wSecond as u8).ok()
    }
    #[cfg(not(windows))]
    {
        let _ = modified;
        None
    }
}

/// Writes `paths` (files and folders on disk) into the zip at `dest`, which exists (empty). Reports
/// (bytes read, bytes in all) as it goes; stops when `cancel` is set. Files that can't be read (in use,
/// no permission) are left out and reported; fails when nothing could be read.
pub fn compress(paths: &[String], dest: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<(u64, Vec<Skipped>), String> {
    let mut items = Vec::new();
    let mut skipped = Vec::new();
    for path in paths {
        let path = Path::new(path);
        let name = path.file_name().map_or_else(|| path.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
        gather(path, name, &mut items, &mut skipped);
    }
    let total: u64 = items.iter().map(|i| i.size).sum();
    let mut zip = ZipWriter::new(BufWriter::new(File::create(dest).map_err(|e| e.to_string())?));
    let (mut done, mut written, mut reported) = (0u64, 0usize, Instant::now());
    let mut buf = vec![0u8; 1 << 20];
    for item in &items {
        let mut options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated).large_file(item.size >= u32::MAX as u64);
        if let Some(time) = fs::metadata(&item.path).and_then(|m| m.modified()).ok().and_then(zip_time) {
            options = options.last_modified_time(time);
        }
        if item.dir {
            zip.add_directory(item.name.as_str(), options).map_err(|e| e.to_string())?;
            continue;
        }
        let mut file = match File::open(&item.path) {
            Ok(file) => file,
            Err(e) => {
                skipped.push(Skipped { path: item.path.to_string_lossy().into_owned(), reason: e.to_string() });
                continue;
            }
        };
        zip.start_file(item.name.as_str(), options).map_err(|e| e.to_string())?;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let n = match file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                // Gone or locked halfway: what's in the zip of it is cut short, so the zip fails.
                Err(e) => return Err(format!("{}: {e}", item.path.display())),
            };
            zip.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            if reported.elapsed().as_millis() >= 100 {
                progress(done, total);
                reported = Instant::now();
            }
        }
        written += 1;
    }
    if written == 0 && items.iter().any(|i| !i.dir) {
        return Err(skipped.first().map_or_else(|| "Nothing could be read".into(), |s| format!("{}: {}", s.path, s.reason)));
    }
    zip.finish().map_err(|e| e.to_string())?.into_inner().map_err(|e| e.to_string())?.sync_all().map_err(|e| e.to_string())?;
    progress(total, total);
    Ok((total, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zips_files_and_folders_that_read_back() {
        let dir = std::env::temp_dir().join(format!("glint-compress-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("photos").join("2026")).unwrap();
        fs::write(dir.join("photos").join("2026").join("a.txt"), "hello hello hello").unwrap();
        fs::write(dir.join("notes é.txt"), "note").unwrap();
        let dest = dir.join("out.zip");
        let paths = [dir.join("photos"), dir.join("notes é.txt")].map(|p| p.to_string_lossy().into_owned());
        let (bytes, skipped) = compress(&paths, &dest, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
        assert_eq!((bytes, skipped.len()), (21, 0));
        let mut zip = zip::ZipArchive::new(File::open(&dest).unwrap()).unwrap();
        let mut names: Vec<String> = zip.file_names().map(|n| n.unwrap().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["notes é.txt", "photos/", "photos/2026/", "photos/2026/a.txt"]);
        let mut text = String::new();
        zip.by_name("photos/2026/a.txt").unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(text, "hello hello hello");
        assert!(compress(&paths, &dest, &AtomicBool::new(true), &mut |_, _| {}).is_err(), "a cancelled one fails");
        fs::remove_dir_all(&dir).unwrap();
    }
}
