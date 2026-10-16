//! Where extracted entries go, and writing them there. Everything lands under one folder in the
//! user's temp folder, `%TEMP%\Glint\archives`:
//!
//! - `s<pid>-<n>`: this run's copies, for previews, thumbnails, opening a file, archives inside
//!   archives, and copies to the other pane on their way through Windows' file engine. A lock file
//!   in it, held open while the app runs, keeps another running Glint from deleting it. It goes when
//!   the app closes; whatever was still open elsewhere then goes the next time it starts.
//! - `out\<time>-<n>`: copies put on the clipboard or dragged out, which another app reads after
//!   Glint has handed them over (pasting can happen long after copying, even after Glint closes).
//!   They go once they're no longer on the clipboard: the next time Glint starts, or an hour after
//!   they were made, when the next one is.

use super::Failure;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

/// Disk space left free on the drive an extraction writes to.
pub const SPACE_MARGIN: u64 = 1 << 30;

fn root() -> PathBuf {
    std::env::temp_dir().join("Glint").join("archives")
}

struct Session {
    path: PathBuf,
    /// Held open without sharing for as long as the app runs: see the module's comment.
    _lock: File,
}

static SESSION: OnceLock<Result<Session, String>> = OnceLock::new();
static SEQ: AtomicU64 = AtomicU64::new(0);

fn session() -> Result<&'static Path, Failure> {
    let session = SESSION.get_or_init(|| {
        let path = root().join(format!("s{}-{}", std::process::id(), now_ms()));
        fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        let lock = lock_file(&path.join("lock")).map_err(|e| e.to_string())?;
        Ok(Session { path, _lock: lock })
    });
    session.as_ref().map(|s| s.path.as_path()).map_err(|e| Failure::Io(e.clone()))
}

#[cfg(windows)]
fn lock_file(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    fs::OpenOptions::new().write(true).create(true).truncate(true).share_mode(0).open(path)
}

#[cfg(not(windows))]
fn lock_file(path: &Path) -> io::Result<File> {
    File::create(path)
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

/// A new empty folder for one extraction: in this run's folder under `kind` (see the module's
/// comment), or under `out` for copies handed to other apps.
pub fn new_folder(kind: &str) -> Result<PathBuf, Failure> {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = if kind == "out" { root().join("out").join(format!("{}-{n}", now_ms())) } else { session()?.join(kind).join(n.to_string()) };
    fs::create_dir_all(&path)?;
    Ok(path)
}

/// Whether `path` is one of the folders extractions go to (or in one), so it may be deleted.
pub fn is_ours(path: &Path) -> bool {
    let root = root();
    path.starts_with(&root) && path != root && !path.components().any(|c| matches!(c, Component::ParentDir))
}

/// Deletes a folder (or file) and everything in it, read-only copies included. What's in use
/// elsewhere stays, to go another time.
pub fn remove_tree(path: &Path) {
    if fs::remove_dir_all(path).is_ok() || !path.exists() {
        return;
    }
    // Read-only files (copies opened in other apps are made read-only) need that cleared first.
    let mut stack = vec![path.to_path_buf()];
    while let Some(at) = stack.pop() {
        if let Ok(meta) = fs::symlink_metadata(&at) {
            if meta.is_dir() {
                if let Ok(entries) = fs::read_dir(&at) {
                    stack.extend(entries.flatten().map(|e| e.path()));
                }
            } else if meta.permissions().readonly() {
                let mut permissions = meta.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                permissions.set_readonly(false);
                let _ = fs::set_permissions(&at, permissions);
            }
        }
    }
    let _ = fs::remove_dir_all(path);
}

/// At start, off the UI's way: deletes what earlier runs left (those that aren't running any more),
/// and copies handed to other apps that aren't on the clipboard.
pub fn sweep(on_clipboard: &[String]) {
    let root = root();
    let Ok(entries) = fs::read_dir(&root) else { return };
    let ours = SESSION.get().and_then(|s| s.as_ref().ok()).map(|s| s.path.as_path());
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "out" {
            sweep_out(on_clipboard, Duration::ZERO);
        } else if name.starts_with('s') && Some(path.as_path()) != ours {
            // The lock only goes when its run has ended.
            if fs::remove_file(path.join("lock")).is_ok() || !path.join("lock").exists() {
                remove_tree(&path);
            }
        }
    }
}

/// Deletes copies handed to other apps that are older than `age` and not on the clipboard.
pub fn sweep_out(on_clipboard: &[String], age: Duration) {
    let Ok(entries) = fs::read_dir(root().join("out")) else { return };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let old = entry.metadata().and_then(|m| m.modified()).map_or(true, |t| now.duration_since(t).unwrap_or_default() >= age);
        let wanted = on_clipboard.iter().any(|p| Path::new(p).starts_with(&path));
        if old && !wanted {
            remove_tree(&path);
        }
    }
}

/// When the app closes: this run's copies go (what's still open in another app stays, for the next
/// start to delete).
pub fn finish() {
    if let Some(Ok(session)) = SESSION.get() {
        for entry in fs::read_dir(&session.path).into_iter().flatten().flatten() {
            if entry.file_name() != "lock" {
                remove_tree(&entry.path());
            }
        }
    }
}

/// Free space on the drive `dir` is on.
#[cfg(windows)]
pub fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None) }.ok()?;
    Some(free)
}

#[cfg(not(windows))]
pub fn free_space(_: &Path) -> Option<u64> {
    None
}

/// Fails unless `bytes` fit on the drive `dir` is on, with room to spare.
pub fn check_space(dir: &Path, bytes: u64) -> Result<(), Failure> {
    match free_space(dir) {
        Some(free) if bytes.saturating_add(SPACE_MARGIN) > free => Err(Failure::NoSpace { need: bytes, free }),
        _ => Ok(()),
    }
}

/// Writes one entry's bytes from `reader` to a new file at `dest`, never more than the `declared`
/// size the archive gave (a bomb could claim one size and unpack to far more) and failing if it
/// gives fewer. `progress` gets the bytes as they're written and stops the copy by returning false.
/// The file gets the entry's date, and is made read-only when it's a copy for opening. On any
/// failure, the partial file is deleted.
pub fn write_entry(reader: &mut dyn Read, dest: &Path, declared: u64, modified: i64, read_only: bool, progress: &mut dyn FnMut(u64) -> bool) -> Result<(), Failure> {
    let result = (|| {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = File::create(dest)?;
        let mut buf = vec![0u8; 256 << 10];
        let mut written = 0u64;
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(read_failure(e)),
            };
            written += n as u64;
            if written > declared {
                return Err(Failure::Bomb("it holds more data than the archive says".into()));
            }
            file.write_all(&buf[..n])?;
            if !progress(n as u64) {
                return Err(Failure::Cancelled);
            }
        }
        if written != declared {
            return Err(Failure::Damaged(format!("it ends after {written} of its {declared} bytes")));
        }
        if modified > 0 {
            let _ = file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_millis(modified as u64));
        }
        drop(file);
        if read_only {
            let mut permissions = fs::metadata(dest)?.permissions();
            permissions.set_readonly(true);
            fs::set_permissions(dest, permissions)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result
}

/// A read error from a decompressor: a checksum that doesn't match means the data is damaged.
fn read_failure(e: io::Error) -> Failure {
    let text = e.to_string();
    let lower = text.to_lowercase();
    if lower.contains("checksum") || lower.contains("crc") {
        Failure::Damaged("its checksum doesn't match".into())
    } else if lower.contains("password") {
        Failure::Encrypted
    } else if matches!(e.kind(), io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput | io::ErrorKind::UnexpectedEof | io::ErrorKind::Other) {
        // A decompressor's complaint ("corrupt deflate stream"), kept in brackets for whoever needs it.
        Failure::Damaged(format!("this file's data can't be decompressed ({text})"))
    } else {
        Failure::Io(text)
    }
}

/// `dest` joined below `root` from names that are already safe (see `tree::clean_name`), checked
/// once more: nothing may land outside the folder it's extracted to.
pub fn inside(root: &Path, names: &[&str]) -> Result<PathBuf, Failure> {
    let mut path = root.to_path_buf();
    for name in names {
        let part = Path::new(name);
        let mut parts = part.components();
        if !matches!((parts.next(), parts.next()), (Some(Component::Normal(_)), None)) {
            return Err(Failure::Damaged(format!("an entry has the unsafe name “{name}”")));
        }
        path.push(part);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glint-extract-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_exactly_what_the_archive_declares() {
        let dir = scratch("write");
        let dest = dir.join("sub").join("a.txt");
        let mut seen = 0;
        write_entry(&mut &b"hello"[..], &dest, 5, 1_700_000_000_000, true, &mut |n| {
            seen += n;
            true
        })
        .unwrap();
        assert_eq!((fs::read(&dest).unwrap(), seen), (b"hello".to_vec(), 5));
        assert!(fs::metadata(&dest).unwrap().permissions().readonly());
        let modified = fs::metadata(&dest).unwrap().modified().unwrap().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis();
        assert_eq!(modified, 1_700_000_000_000);
        // More than declared: a bomb's trick. Fewer: damaged. Either way nothing's left behind.
        let more = dir.join("more.bin");
        assert!(matches!(write_entry(&mut &[0u8; 10][..], &more, 4, 0, false, &mut |_| true), Err(Failure::Bomb(_))));
        assert!(!more.exists());
        assert!(matches!(write_entry(&mut &[0u8; 3][..], &more, 4, 0, false, &mut |_| true), Err(Failure::Damaged(_))));
        assert!(matches!(write_entry(&mut &[0u8; 4][..], &more, 4, 0, false, &mut |_| false), Err(Failure::Cancelled)));
        assert!(!more.exists());
        remove_tree(&dir);
        assert!(!dir.exists(), "read-only copies are deleted too");
    }

    #[test]
    fn keeps_extraction_inside_its_folder() {
        let root = Path::new(r"C:\x");
        assert_eq!(inside(root, &["a", "b.txt"]).unwrap(), Path::new(r"C:\x\a\b.txt"));
        assert!(inside(root, &[".."]).is_err());
        assert!(inside(root, &["a\\..\\.."]).is_err());
        assert!(inside(root, &["C:\\Windows"]).is_err());
        assert!(inside(root, &["\\\\server\\share"]).is_err());
        assert!(inside(root, &[""]).is_err());
    }

    #[test]
    fn deletes_only_its_own_folders() {
        assert!(is_ours(&root().join("out").join("1-2")));
        assert!(!is_ours(&root()));
        assert!(!is_ours(&root().join("..").join("x")));
        assert!(!is_ours(Path::new(r"C:\Users")));
    }
}
