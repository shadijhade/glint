//! Writing archives: Compress to zip (Deflate, as Explorer's own zip folders use, with the files'
//! dates; zip64 only where an entry needs it), to 7z (LZMA2) and to tar.zst (Zstandard, fast). A zip
//! or a 7z can be password-protected with AES-256; a 7z's file names are encrypted too, so nothing of
//! what's in it shows without the password. File Explorer's own Compress to can't encrypt.

use super::Skipped;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime};
use glint_model::marks::lines::{write_beside, Beside, Line};
use glint_model::marks::BESIDE_NAME;
use std::collections::BTreeMap;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// What to write.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Format {
    Zip,
    SevenZ,
    TarZst,
}

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

/// The `.glint-marks` of each folder of the archive that has a marked item, as (its name in the archive,
/// its bytes): lines by name, a folder's own marks in its parent's. None for items with no marks.
fn marks_entries(items: &[Item]) -> Vec<(String, Vec<u8>)> {
    #[cfg(test)]
    if tests::SKIP_MARKS.load(Ordering::Relaxed) {
        return Vec::new();
    }
    let paths: Vec<String> = items.iter().map(|i| i.path.to_string_lossy().into_owned()).collect();
    let mut folders: BTreeMap<&str, Vec<Line>> = BTreeMap::new();
    for (item, marks) in items.iter().zip(crate::marks::read_many(&paths)) {
        if !marks.is_empty() {
            let name = item.name.trim_end_matches('/');
            let (folder, leaf) = name.rsplit_once('/').unwrap_or(("", name));
            folders.entry(folder).or_default().push(Line { name: leaf.into(), marks, ..Default::default() });
        }
    }
    folders
        .into_iter()
        .map(|(folder, lines)| {
            let name = if folder.is_empty() { BESIDE_NAME.to_string() } else { format!("{folder}/{BESIDE_NAME}") };
            (name, write_beside(&Beside { version: 1, lines, ..Default::default() }).into_bytes())
        })
        .collect()
}

/// Hidden and archive: the MS-DOS attributes of a `.glint-marks` entry (zip, 7z).
const HIDDEN_ENTRY: u32 = 0x22;

/// Writes `paths` (files and folders on disk) into the zip at `dest`, which exists (empty). Reports
/// (bytes read, bytes in all) as it goes; stops when `cancel` is set. Files that can't be read (in use,
/// no permission) are left out and reported; fails when nothing could be read.
#[cfg(test)]
pub fn compress(paths: &[String], dest: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<(u64, Vec<Skipped>), String> {
    compress_as(paths, dest, Format::Zip, None, cancel, progress)
}

/// `compress`, into `format`, encrypted with `password` (a zip or a 7z) when one is given.
pub fn compress_as(paths: &[String], dest: &Path, format: Format, password: Option<&str>, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<(u64, Vec<Skipped>), String> {
    let mut items = Vec::new();
    let mut skipped = Vec::new();
    for path in paths {
        let path = Path::new(path);
        let name = path.file_name().map_or_else(|| path.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
        gather(path, name, &mut items, &mut skipped);
    }
    // A .glint-marks on disk is not copied: the archive's own is made from the items that are there.
    items.retain(|i| i.dir || !i.name.rsplit('/').next().is_some_and(|n| n.eq_ignore_ascii_case(BESIDE_NAME)));
    let total: u64 = items.iter().map(|i| i.size).sum();
    let password = password.filter(|p| !p.is_empty());
    match format {
        Format::Zip => {}
        Format::SevenZ => return seven_z(&items, skipped, total, dest, password, cancel, progress),
        Format::TarZst => return tar_zst(&items, skipped, total, dest, cancel, progress),
    }
    let mut zip = ZipWriter::new(BufWriter::new(File::create(dest).map_err(|e| e.to_string())?));
    let (mut done, mut written, mut reported) = (0u64, 0usize, Instant::now());
    let mut buf = vec![0u8; 1 << 20];
    for item in &items {
        let mut options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated).large_file(item.size >= u32::MAX as u64);
        if let Some(password) = password {
            options = options.with_aes_encryption(zip::AesMode::Aes256, password);
        }
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
    for (name, bytes) in marks_entries(&items) {
        let mut options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated).system(zip::System::Dos).external_attributes(HIDDEN_ENTRY);
        if let Some(password) = password {
            options = options.with_aes_encryption(zip::AesMode::Aes256, password);
        }
        if let Some(time) = zip_time(SystemTime::now()) {
            options = options.last_modified_time(time);
        }
        zip.start_file(name.as_str(), options).map_err(|e| e.to_string())?;
        zip.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    zip.finish().map_err(|e| e.to_string())?.into_inner().map_err(|e| e.to_string())?.sync_all().map_err(|e| e.to_string())?;
    progress(total, total);
    Ok((total, skipped))
}

/// Reads a file into an archive, counting what it read for the progress and stopping on Cancel.
struct Counted<'a, R> {
    inner: R,
    done: &'a mut u64,
    total: u64,
    cancel: &'a AtomicBool,
    reported: &'a mut Instant,
    progress: &'a mut dyn FnMut(u64, u64),
}

impl<R: Read> Read for Counted<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = self.inner.read(buf)?;
        *self.done += n as u64;
        if self.reported.elapsed().as_millis() >= 100 {
            (self.progress)(*self.done, self.total);
            *self.reported = Instant::now();
        }
        Ok(n)
    }
}

fn cancelled_or(e: impl std::fmt::Display, cancel: &AtomicBool) -> String {
    if cancel.load(Ordering::Relaxed) {
        "cancelled".into()
    } else {
        e.to_string()
    }
}

/// A 7z (LZMA2), its contents and its list of files encrypted with AES-256 when there's a password.
fn seven_z(items: &[Item], mut skipped: Vec<Skipped>, total: u64, dest: &Path, password: Option<&str>, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<(u64, Vec<Skipped>), String> {
    use sevenz_rust2::encoder_options::AesEncoderOptions;
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter, EncoderMethod};
    let mut archive = ArchiveWriter::new(BufWriter::new(File::create(dest).map_err(|e| e.to_string())?)).map_err(|e| e.to_string())?;
    if let Some(password) = password {
        archive.set_content_methods(vec![AesEncoderOptions::new(password.into()).into(), EncoderMethod::LZMA2.into()]);
        archive.set_encrypt_header(true);
    }
    let (mut done, mut reported, mut written) = (0u64, Instant::now(), 0usize);
    for item in items {
        if item.dir {
            archive.push_archive_entry::<File>(ArchiveEntry::from_path(&item.path, item.name.trim_end_matches('/').to_string()), None).map_err(|e| e.to_string())?;
            continue;
        }
        let file = match File::open(&item.path) {
            Ok(file) => file,
            Err(e) => {
                skipped.push(Skipped { path: item.path.to_string_lossy().into_owned(), reason: e.to_string() });
                continue;
            }
        };
        let reader = Counted { inner: file, done: &mut done, total, cancel, reported: &mut reported, progress: &mut *progress };
        archive.push_archive_entry(ArchiveEntry::from_path(&item.path, item.name.clone()), Some(reader)).map_err(|e| cancelled_or(e, cancel))?;
        written += 1;
    }
    if written == 0 && items.iter().any(|i| !i.dir) {
        return Err(skipped.first().map_or_else(|| "Nothing could be read".into(), |s| format!("{}: {}", s.path, s.reason)));
    }
    for (name, bytes) in marks_entries(items) {
        let mut entry = ArchiveEntry::new_file(&name);
        (entry.has_windows_attributes, entry.windows_attributes) = (true, HIDDEN_ENTRY);
        archive.push_archive_entry(entry, Some(std::io::Cursor::new(bytes))).map_err(|e| e.to_string())?;
    }
    archive.finish().map_err(|e| e.to_string())?.into_inner().map_err(|e| e.to_string())?.sync_all().map_err(|e| e.to_string())?;
    progress(total, total);
    Ok((total, skipped))
}

/// Bytes handed from the tar being written to the Zstandard compressor on another thread.
struct Pipe(std::sync::mpsc::SyncSender<Vec<u8>>);

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.send(buf.to_vec()).map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the compressor stopped"))?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Drain {
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    at: usize,
}

impl Read for Drain {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.at == self.chunk.len() {
            match self.rx.recv() {
                Ok(chunk) => (self.chunk, self.at) = (chunk, 0),
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.chunk.len() - self.at);
        buf[..n].copy_from_slice(&self.chunk[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// A tar compressed with Zstandard (its fastest level, about zstd's 1), written as it's made.
fn tar_zst(items: &[Item], mut skipped: Vec<Skipped>, total: u64, dest: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<(u64, Vec<Skipped>), String> {
    let out = File::create(dest).map_err(|e| e.to_string())?;
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
    let (mut done, mut reported, mut written) = (0u64, Instant::now(), 0usize);
    let built: Result<(), String> = std::thread::scope(|scope| {
        let compressor = scope.spawn(move || {
            let mut out = BufWriter::new(out);
            ruzstd::encoding::compress(Drain { rx, chunk: Vec::new(), at: 0 }, &mut out, ruzstd::encoding::CompressionLevel::Fastest);
            out.into_inner().map_err(|e| e.to_string())?.sync_all().map_err(|e| e.to_string())
        });
        let mut tar = tar::Builder::new(Pipe(tx));
        tar.mode(tar::HeaderMode::Deterministic);
        let result = (|| {
            for item in items {
                let name = item.name.trim_end_matches('/');
                if item.dir {
                    tar.append_dir(name, &item.path).map_err(|e| e.to_string())?;
                    continue;
                }
                let file = match File::open(&item.path) {
                    Ok(file) => file,
                    Err(e) => {
                        skipped.push(Skipped { path: item.path.to_string_lossy().into_owned(), reason: e.to_string() });
                        continue;
                    }
                };
                let meta = file.metadata().map_err(|e| e.to_string())?;
                let mut header = tar::Header::new_gnu();
                header.set_metadata_in_mode(&meta, tar::HeaderMode::Deterministic);
                header.set_size(meta.len());
                if let Ok(modified) = meta.modified() {
                    header.set_mtime(modified.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs()));
                }
                let reader = Counted { inner: file, done: &mut done, total, cancel, reported: &mut reported, progress: &mut *progress };
                tar.append_data(&mut header, name, reader).map_err(|e| cancelled_or(e, cancel))?;
                written += 1;
            }
            for (name, bytes) in marks_entries(items) {
                let mut header = tar::Header::new_gnu();
                header.set_size(bytes.len() as u64);
                header.set_mode(0o644);
                header.set_mtime(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs()));
                tar.append_data(&mut header, &name, bytes.as_slice()).map_err(|e| e.to_string())?;
            }
            tar.finish().map_err(|e| e.to_string())
        })();
        // The pipe closes with the tar, so the compressor finishes.
        drop(tar);
        let compressed = compressor.join().map_err(|_| "the compressor failed".to_string())?;
        result.and(compressed)
    });
    built?;
    if written == 0 && items.iter().any(|i| !i.dir) {
        return Err(skipped.first().map_or_else(|| "Nothing could be read".into(), |s| format!("{}: {}", s.path, s.reason)));
    }
    progress(total, total);
    Ok((total, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    use glint_model::marks::lines::read_beside;
    use glint_model::marks::Marks;

    /// Set only by the ignored measurement, to time a zip without the marks' step.
    pub(super) static SKIP_MARKS: AtomicBool = AtomicBool::new(false);

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glint-zipmarks-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mark(path: &Path, label: Option<&str>, tags: &[&str], comment: Option<&str>) {
        let marks = Marks { label: label.map(Into::into), tags: tags.iter().map(|t| t.to_string()).collect(), comment: comment.map(Into::into) };
        crate::marks::set_many(vec![(path.to_string_lossy().into_owned(), glint_model::marks::Change::Put(marks))]);
    }

    fn zip_text(zip: &Path, entry: &str) -> Option<String> {
        let mut zip = zip::ZipArchive::new(File::open(zip).unwrap()).unwrap();
        let mut text = String::new();
        zip.by_name(entry).ok()?.read_to_string(&mut text).unwrap();
        Some(text)
    }

    fn zip_names(zip: &Path) -> Vec<String> {
        let zip = zip::ZipArchive::new(File::open(zip).unwrap()).unwrap();
        let mut names: Vec<String> = zip.file_names().map(|n| n.unwrap().into_owned()).collect();
        names.sort();
        names
    }

    fn lines_of(text: &str) -> Vec<(String, Marks)> {
        read_beside(text).lines.into_iter().map(|l| (l.name, l.marks)).collect()
    }

    fn zip_of(paths: &[&Path], dest: &Path) -> (u64, usize) {
        let paths: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        let (bytes, skipped) = compress(&paths, dest, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
        (bytes, skipped.len())
    }

    #[test]
    fn a_zip_holds_a_marks_file_in_each_folder_with_a_marked_item() {
        let dir = scratch("folder");
        let folder = dir.join("folder");
        fs::create_dir_all(&folder).unwrap();
        for n in ["a.txt", "b.txt", "bare.txt"] {
            fs::write(folder.join(n), "text").unwrap();
        }
        mark(&folder.join("a.txt"), Some("Red"), &[], None);
        mark(&folder.join("b.txt"), None, &["x", "y"], None);
        let dest = dir.join("out.zip");
        assert_eq!(zip_of(&[&folder], &dest), (12, 0), "the marks file counts nothing");
        let lines = lines_of(&zip_text(&dest, "folder/.glint-marks").unwrap());
        assert_eq!(lines.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["a.txt", "b.txt"]);
        assert_eq!(lines[0].1.label.as_deref(), Some("Red"));
        assert_eq!(lines[1].1.tags, ["x", "y"]);
        assert!(zip_text(&dest, "folder/.glint-marks").unwrap().starts_with("{\"glint-marks\":1}\n"));
        assert!(zip_text(&dest, ".glint-marks").is_none(), "nothing at the top: no item there is marked");
        // It is hidden, and the zip's own date is not 1980.
        let mut zip = zip::ZipArchive::new(File::open(&dest).unwrap()).unwrap();
        let entry = zip.by_name("folder/.glint-marks").unwrap();
        assert_eq!(entry.external_attributes() & 0x02, 0x02, "MS-DOS hidden");
        assert_eq!(entry.system(), zip::System::Dos);
        drop(entry);
        assert_eq!(zip.by_name("folder/a.txt").unwrap().external_attributes() & 0x02, 0, "files are not hidden");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folders_own_marks_are_in_its_parents_file_under_its_name() {
        let dir = scratch("own");
        let (photos, notes) = (dir.join("photos"), dir.join("notes.txt"));
        fs::create_dir_all(&photos).unwrap();
        fs::write(&notes, "n").unwrap();
        fs::write(photos.join("p.txt"), "p").unwrap();
        mark(&photos, Some("Blue"), &[], None);
        mark(&notes, None, &["top"], None);
        let dest = dir.join("out.zip");
        zip_of(&[&photos, &notes], &dest);
        let top = lines_of(&zip_text(&dest, ".glint-marks").unwrap());
        assert_eq!(top.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["notes.txt", "photos"]);
        assert_eq!(top[1].1.label.as_deref(), Some("Blue"));
        assert!(zip_text(&dest, "photos/.glint-marks").is_none(), "nothing in photos itself is marked");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_levels_give_the_deep_folder_its_file_and_not_the_one_above() {
        let dir = scratch("deep");
        let deep = dir.join("photos").join("2026");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("a.jpg"), "a").unwrap();
        mark(&deep.join("a.jpg"), None, &["trip"], None);
        let dest = dir.join("out.zip");
        zip_of(&[&dir.join("photos")], &dest);
        assert!(zip_text(&dest, "photos/2026/.glint-marks").is_some());
        assert!(zip_text(&dest, "photos/.glint-marks").is_none());
        assert!(zip_text(&dest, ".glint-marks").is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_zip_of_unmarked_files_has_no_marks_file() {
        let dir = scratch("none");
        fs::create_dir_all(dir.join("f/g")).unwrap();
        fs::write(dir.join("f/g/a.txt"), "a").unwrap();
        fs::write(dir.join("f/b.txt"), "b").unwrap();
        let dest = dir.join("out.zip");
        zip_of(&[&dir.join("f")], &dest);
        assert_eq!(zip_names(&dest), ["f/", "f/b.txt", "f/g/", "f/g/a.txt"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_marks_file_on_disk_is_not_copied_and_the_zips_own_is_made_from_the_items() {
        let dir = scratch("disk");
        let folder = dir.join("f");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("a.txt"), "a").unwrap();
        fs::write(folder.join("b.txt"), "b").unwrap();
        mark(&folder.join("a.txt"), Some("Red"), &[], None);
        // Whatever the folder's file says now, a line for an item that is gone stays out of the zip's.
        let beside = fs::read_to_string(folder.join(".glint-marks")).unwrap_or_default();
        fs::write(folder.join(".glint-marks"), format!("{beside}{{\"name\":\"gone.txt\",\"tags\":[\"ghost\"]}}\n")).unwrap();
        let dest = dir.join("out.zip");
        assert_eq!(zip_of(&[&folder], &dest), (2, 0));
        let names = zip_names(&dest);
        assert_eq!(names.iter().filter(|n| n.ends_with(".glint-marks")).count(), 1, "{names:?}");
        let text = zip_text(&dest, "f/.glint-marks").unwrap();
        assert!(!text.contains("gone.txt") && !text.contains("ghost"), "{text}");
        assert!(text.contains("a.txt"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_comment_with_a_line_break_and_an_arabic_tag_come_out_as_they_went_in() {
        let dir = scratch("text");
        fs::write(dir.join("a.txt"), "a").unwrap();
        mark(&dir.join("a.txt"), None, &["\u{645}\u{631}\u{62d}\u{628}\u{627}"], Some("one\ntwo \"quoted\""));
        let dest = dir.join("out.zip");
        zip_of(&[&dir.join("a.txt")], &dest);
        let lines = lines_of(&zip_text(&dest, ".glint-marks").unwrap());
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].1.tags, ["\u{645}\u{631}\u{62d}\u{628}\u{627}"]);
        assert_eq!(lines[0].1.comment.as_deref(), Some("one\ntwo \"quoted\""));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_password_zip_encrypts_the_marks_file_too() {
        let dir = scratch("locked");
        fs::write(dir.join("a.txt"), "a").unwrap();
        mark(&dir.join("a.txt"), Some("Red"), &[], None);
        let dest = dir.join("out.zip");
        let paths = [dir.join("a.txt").to_string_lossy().into_owned()];
        compress_as(&paths, &dest, Format::Zip, Some("pw"), &AtomicBool::new(false), &mut |_, _| {}).unwrap();
        let mut zip = zip::ZipArchive::new(File::open(&dest).unwrap()).unwrap();
        assert!(zip.by_name(".glint-marks").is_err(), "no password, no reading");
        let mut text = String::new();
        zip.by_name_decrypt(".glint-marks", b"pw").unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(lines_of(&text).len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_7z_and_a_tar_zst_hold_the_marks_file_too() {
        let dir = scratch("others");
        let folder = dir.join("f");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("a.txt"), "a").unwrap();
        fs::write(folder.join("bare.txt"), "b").unwrap();
        mark(&folder.join("a.txt"), Some("Red"), &["t"], None);
        let paths = [folder.to_string_lossy().into_owned()];
        for (format, name) in [(Format::SevenZ, "o.7z"), (Format::TarZst, "o.tar.zst")] {
            let (bytes, skipped) = compress_as(&paths, &dir.join(name), format, None, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
            assert_eq!((bytes, skipped.len()), (2, 0), "{name}");
        }
        let mut seven = sevenz_rust2::ArchiveReader::open(dir.join("o.7z"), sevenz_rust2::Password::empty()).unwrap();
        let text = String::from_utf8(seven.read_file("f/.glint-marks").unwrap()).unwrap();
        assert_eq!(lines_of(&text).iter().map(|(n, m)| (n.as_str(), m.label.as_deref())).collect::<Vec<_>>(), [("a.txt", Some("Red"))]);
        let mut tar_bytes = Vec::new();
        ruzstd::decoding::StreamingDecoder::new(File::open(dir.join("o.tar.zst")).unwrap()).unwrap().read_to_end(&mut tar_bytes).unwrap();
        let mut found = None;
        for entry in tar::Archive::new(tar_bytes.as_slice()).entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.path().unwrap().to_string_lossy().replace('\\', "/") == "f/.glint-marks" {
                let mut text = String::new();
                entry.read_to_string(&mut text).unwrap();
                found = Some(text);
            }
        }
        assert_eq!(lines_of(&found.unwrap())[0].1.tags, ["t"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Measured: `cargo test -p glint-core archive::compress::tests::measure -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn measure_a_zip_of_ten_thousand_unmarked_files() {
        let dir = scratch("measure");
        let folder = dir.join("many");
        fs::create_dir_all(&folder).unwrap();
        for i in 0..10_000 {
            fs::write(folder.join(format!("f{i:05}.bin")), [i as u8; 64]).unwrap();
        }
        let paths = [folder.to_string_lossy().into_owned()];
        let run = |skip: bool| {
            SKIP_MARKS.store(skip, Ordering::Relaxed);
            let t = Instant::now();
            compress(&paths, &dir.join("out.zip"), &AtomicBool::new(false), &mut |_, _| {}).unwrap();
            t.elapsed().as_millis()
        };
        let (mut before, mut after) = (Vec::new(), Vec::new());
        run(true);
        for _ in 0..5 {
            before.push(run(true));
            after.push(run(false));
        }
        SKIP_MARKS.store(false, Ordering::Relaxed);
        before.sort();
        after.sort();
        println!("before (marks' step skipped) {before:?} ms, median {}; after {after:?} ms, median {}", before[2], after[2]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn writes_7z_and_tar_zst_with_and_without_passwords() {
        let dir = std::env::temp_dir().join(format!("glint-compress-formats-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("photos/2024")).unwrap();
        fs::write(dir.join("photos/a.txt"), b"hello").unwrap();
        fs::write(dir.join("photos/2024/b.bin"), vec![7u8; 100_000]).unwrap();
        let paths = vec![dir.join("photos").to_string_lossy().into_owned()];
        let cancel = AtomicBool::new(false);
        for (format, password, name) in [(Format::SevenZ, None, "plain.7z"), (Format::SevenZ, Some("s3cret"), "locked.7z"), (Format::TarZst, None, "t.tar.zst"), (Format::Zip, Some("s3cret"), "locked.zip")] {
            let dest = dir.join(name);
            let (bytes, skipped) = compress_as(&paths, &dest, format, password, &cancel, &mut |_, _| {}).unwrap();
            assert_eq!((bytes, skipped.len()), (100_005, 0), "{name}");
            assert!(fs::metadata(&dest).unwrap().len() > 0, "{name}");
        }
        // The 7z reads back with its password, and not without it.
        let plain = sevenz_rust2::ArchiveReader::open(dir.join("plain.7z"), sevenz_rust2::Password::empty()).unwrap();
        assert!(plain.archive().files.iter().any(|f| f.name().ends_with("b.bin")));
        assert!(sevenz_rust2::ArchiveReader::open(dir.join("locked.7z"), sevenz_rust2::Password::empty()).is_err(), "its list of files is encrypted");
        let locked = sevenz_rust2::ArchiveReader::open(dir.join("locked.7z"), "s3cret".into()).unwrap();
        assert!(locked.archive().files.iter().any(|f| f.name().ends_with("a.txt")));
        // The tar.zst decompresses to a tar holding both files.
        let tar_bytes = {
            let mut decoder = ruzstd::decoding::StreamingDecoder::new(File::open(dir.join("t.tar.zst")).unwrap()).unwrap();
            let mut out = Vec::new();
            decoder.read_to_end(&mut out).unwrap();
            out
        };
        let mut archive = tar::Archive::new(tar_bytes.as_slice());
        let names: Vec<String> = archive.entries().unwrap().map(|e| e.unwrap().path().unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert!(names.iter().any(|n| n == "photos/2024/b.bin") && names.iter().any(|n| n == "photos/a.txt"), "{names:?}");
        let _ = fs::remove_dir_all(&dir);
    }

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
