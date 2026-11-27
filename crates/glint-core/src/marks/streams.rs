//! A file's own marks: its `glint.tags`, `glint.label` and `glint.comment` streams (PLAN.md 5.1), each plain
//! UTF-8 a script can read (`Get-Content f -Stream glint.comment`). Read with one listing of the file's streams,
//! then one open for each `glint.*` it has, so a file with none costs one open and one query; written with the
//! file's dates kept (M3). `glint.tags` keeps 0.16's name and format (M8).

use glint_model::marks::Marks;

/// The marks in `path`'s streams; None when it has no `glint.*` stream, or they can't be read.
#[cfg(windows)]
pub fn read(path: &str) -> Option<Marks> {
    use glint_model::marks::{COMMENT_CAP, LABEL_CAP};
    if is_root(path) {
        return None;
    }
    let found = listing(path)?;
    let tags = if found[0] { crate::tags::read(path) } else { Vec::new() };
    let label = if found[1] { read_stream(path, STREAM_LABEL, LABEL_CAP).and_then(|b| label_of(&b)) } else { None };
    let comment = if found[2] { read_stream(path, STREAM_COMMENT, COMMENT_CAP).and_then(|b| comment_of(&b)) } else { None };
    let marks = Marks { label, tags, comment };
    (!marks.is_empty()).then_some(marks)
}

/// Writes `marks` into `path`'s streams, each alone (an empty one's stream goes), with the file's dates kept.
#[cfg(windows)]
pub fn write(path: &str, marks: &Marks) -> Result<(), String> {
    use glint_model::marks::{COMMENT_CAP, LABEL_CAP, TAGS_CAP};
    use std::io::Write;
    if marks.label.as_ref().is_some_and(|l| l.len() > LABEL_CAP) {
        return Err("A label's name can be 256 bytes at most".into());
    }
    let tags: Vec<String> = marks.tags.iter().map(|t| t.replace(['\r', '\n'], " ").trim().to_string()).filter(|t| !t.is_empty()).collect();
    let tags_text = if tags.is_empty() { None } else { Some(format!("{}\n", tags.join("\n"))) };
    if tags_text.as_ref().is_some_and(|t| t.len() > TAGS_CAP) {
        return Err("These are more tags than a file can hold (4 KB)".into());
    }
    if marks.comment.as_ref().is_some_and(|c| c.len() > COMMENT_CAP) {
        return Err("A comment can be 64 KB at most".into());
    }
    if is_root(path) {
        return Err("A drive itself can't be marked".into());
    }
    // What is written always reads back: a label is one line, a comment has plain line ends.
    let label_text = marks.label.as_ref().map(|l| l.replace(['\r', '\n'], " ").trim().to_string()).filter(|l| !l.is_empty()).map(|l| format!("{l}\n"));
    let comment_text = marks.comment.as_ref().map(|c| c.replace("\r\n", "\n").trim_end().to_string()).filter(|c| !c.trim().is_empty()).map(|c| format!("{c}\n"));

    let now = read(path).unwrap_or_default();
    let now_tags: Vec<String> = now.tags.clone();
    let mut jobs: Vec<(&str, Option<String>)> = Vec::new();
    if tags != now_tags {
        jobs.push((STREAM_TAGS, tags_text));
    }
    if label_text.as_deref().map(str::trim_end) != now.label.as_deref() {
        jobs.push((STREAM_LABEL, label_text));
    }
    if comment_text.as_deref().map(str::trim_end) != now.comment.as_deref() {
        jobs.push((STREAM_COMMENT, comment_text));
    }
    if jobs.is_empty() {
        return Ok(());
    }
    // The file's dates are taken once before the first stream is written and put back once after the last.
    let restore = crate::tags::keep_times(path);
    let mut result = Ok(());
    for (stream, text) in jobs {
        let target = stream_of(path, stream);
        let done = match text {
            None => match std::fs::remove_file(&target) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
            Some(text) => std::fs::File::create(&target).and_then(|mut f| f.write_all(text.as_bytes())),
        };
        if let Err(e) = done {
            result = Err(e.to_string());
            break;
        }
    }
    if let Some(restore) = restore {
        restore();
    }
    result
}

#[cfg(not(windows))]
pub fn read(_path: &str) -> Option<Marks> {
    None
}

#[cfg(not(windows))]
pub fn write(_path: &str, _marks: &Marks) -> Result<(), String> {
    Err("Marks in the file need Windows".into())
}

#[cfg(windows)]
use glint_model::marks::{STREAM_COMMENT, STREAM_LABEL, STREAM_TAGS};

/// The path of one of the item's streams, as `tags::stream_of` makes it.
#[cfg(windows)]
fn stream_of(path: &str, stream: &str) -> String {
    format!("{}:{stream}", path.trim_end_matches(['\\', '/']))
}

/// A drive's root (`C:\`, `\\server\share\`): it takes no marks.
#[cfg(windows)]
fn is_root(path: &str) -> bool {
    let p = path.trim_end_matches(['\\', '/']);
    if let Some(unc) = p.strip_prefix(r"\\") {
        return !unc.starts_with(['?', '.']) && unc.split(['\\', '/']).filter(|s| !s.is_empty()).count() <= 2;
    }
    let b = p.as_bytes();
    b.len() == 2 && b[1] == b':'
}

/// The label from a stream's bytes: its first line with anything in it.
#[cfg(windows)]
fn label_of(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    text.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

/// The comment from a stream's bytes (cut at the cap, so a broken last character goes).
#[cfg(windows)]
fn comment_of(bytes: &[u8]) -> Option<String> {
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t.to_string(),
        Err(e) if e.error_len().is_none() => String::from_utf8_lossy(bytes.get(..e.valid_up_to())?).into_owned(),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    };
    let text = text.replace("\r\n", "\n");
    let text = text.trim_end();
    (!text.is_empty()).then(|| text.to_string())
}

/// At most `cap` bytes of one of the item's streams.
#[cfg(windows)]
fn read_stream(path: &str, stream: &str, cap: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(stream_of(path, stream)).ok()?.take(cap as u64).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Which of [tags, label, comment] the item has a stream for: one query of its stream listing.
#[cfg(windows)]
fn listing(path: &str) -> Option<[bool; 3]> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::{ERROR_MORE_DATA, HANDLE};
    use windows::Win32::Storage::FileSystem::{FileStreamInfo, GetFileInformationByHandleEx, FILE_FLAG_BACKUP_SEMANTICS};
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    let file = std::fs::OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES).custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0).open(path).ok()?;
    // 4 KB stack buffer (8-byte aligned as FileStreamInfo requires) handles 99.9%+ of files without heap allocation.
    let mut stack = [0u64; 512];
    let got = unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle()),
            FileStreamInfo,
            stack.as_mut_ptr().cast(),
            (stack.len() * 8) as u32,
        )
    };
    match got {
        Ok(()) => {
            let bytes = unsafe { std::slice::from_raw_parts(stack.as_ptr().cast::<u8>(), stack.len() * 8) };
            return Some(walk(bytes));
        }
        Err(e) if e.code() == ERROR_MORE_DATA.to_hresult() => {}
        Err(_) => return None, // no streams at all answers ERROR_HANDLE_EOF
    }
    let mut size = 16 * 1024usize;
    while size <= 256 * 1024 {
        // A Vec<u64> keeps the buffer aligned to 8 bytes, as the API wants.
        let mut buf = vec![0u64; size / 8];
        // SAFETY: the buffer is `size` bytes long and the handle is open for the call.
        let got = unsafe { GetFileInformationByHandleEx(HANDLE(file.as_raw_handle()), FileStreamInfo, buf.as_mut_ptr().cast(), size as u32) };
        match got {
            Ok(()) => {
                // SAFETY: the same buffer seen as bytes: u64 has no padding and u8 no alignment.
                let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), size) };
                return Some(walk(bytes));
            }
            Err(e) if e.code() == ERROR_MORE_DATA.to_hresult() => size *= 4,
            Err(_) => return None, // no streams at all answers ERROR_HANDLE_EOF
        }
    }
    None
}

/// Whether `raw` (a stream's name as `FILE_STREAM_INFO` gives it: UTF-16LE) is `:{stream}:$DATA`, in any case.
#[cfg(windows)]
fn names_stream(raw: &[u8], stream: &str) -> bool {
    let (units, rest) = raw.as_chunks::<2>();
    let wanted = [":", stream, ":$DATA"].into_iter().flat_map(str::bytes);
    rest.is_empty()
        && units.len() == stream.len() + 7
        && units.iter().zip(wanted).all(|(unit, b)| u16::from_le_bytes(*unit) < 0x80 && (u16::from_le_bytes(*unit) as u8).eq_ignore_ascii_case(&b))
}

/// Walks `FILE_STREAM_INFO` entries: next offset (u32), name length in bytes (u32), two sizes (i64 each), the name.
#[cfg(windows)]
fn walk(bytes: &[u8]) -> [bool; 3] {
    let u32_at = |at: usize| bytes.get(at..at.checked_add(4)?).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let mut found = [false; 3];
    let mut off = 0usize;
    while let (Some(next), Some(len)) = (u32_at(off), u32_at(off + 4)) {
        let Some(raw) = off.checked_add(24).and_then(|s| bytes.get(s..s.checked_add(len)?)) else { break };
        // Compared as the UTF-16 it is: every file has its unnamed `::$DATA`, and rows are read by the screenful.
        for (i, stream) in [STREAM_TAGS, STREAM_LABEL, STREAM_COMMENT].into_iter().enumerate() {
            if names_stream(raw, stream) {
                found[i] = true;
            }
        }
        if next == 0 {
            break;
        }
        off = match off.checked_add(next) {
            Some(n) => n,
            None => break,
        };
    }
    found
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::marks::caps;
    use glint_model::marks::{COMMENT_CAP, LABEL_CAP};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// A folder of the test's own, removed when it drops (a read-only file inside loses the attribute first).
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Ok(entries) = fs::read_dir(&self.0) {
                for e in entries.flatten() {
                    if let Ok(m) = e.metadata() {
                        let mut p = m.permissions();
                        #[allow(clippy::permissions_set_readonly_false)] // a temp file about to be deleted
                        p.set_readonly(false);
                        let _ = fs::set_permissions(e.path(), p);
                    }
                }
            }
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A fresh folder, or None when the temp drive keeps no streams (the test then skips).
    fn scratch() -> Option<Scratch> {
        let dir = std::env::temp_dir().join(format!("glint-streams-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst)));
        fs::create_dir_all(&dir).ok()?;
        let s = Scratch(dir);
        caps::has_streams(&s.0.to_string_lossy()).then_some(s)
    }

    fn make(s: &Scratch, name: &str) -> String {
        let p = s.0.join(name);
        fs::write(&p, b"contents").unwrap();
        p.to_string_lossy().into_owned()
    }

    fn all() -> Marks {
        Marks { label: Some("Important".into()), tags: vec!["Work".into(), "Red".into()], comment: Some("mode 4\nLACP rate fast".into()) }
    }

    fn stream(path: &str, name: &str) -> std::io::Result<String> {
        fs::read_to_string(format!("{path}:{name}"))
    }

    #[test]
    fn three_marks_written_are_read_back_and_the_file_is_untouched() {
        let Some(s) = scratch() else { return };
        let path = make(&s, "a.txt");
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&path, &all()).unwrap();
        assert_eq!(read(&path), Some(all()));
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before, "Date modified stays");
        assert_eq!(fs::read(&path).unwrap(), b"contents");
        assert_eq!(stream(&path, "glint.tags").unwrap(), "Work\nRed\n");
        assert_eq!(stream(&path, "glint.label").unwrap(), "Important\n");
        assert_eq!(stream(&path, "glint.comment").unwrap(), "mode 4\nLACP rate fast\n");
    }

    #[test]
    fn a_file_with_no_marks_and_a_missing_path_read_as_none() {
        let Some(s) = scratch() else { return };
        let path = make(&s, "a.txt");
        assert_eq!(read(&path), None);
        assert_eq!(read(&s.0.join("nope.txt").to_string_lossy()), None);
    }

    #[test]
    fn writing_nothing_takes_all_three_streams_away() {
        let Some(s) = scratch() else { return };
        let path = make(&s, "a.txt");
        write(&path, &all()).unwrap();
        write(&path, &Marks::default()).unwrap();
        for name in ["glint.tags", "glint.label", "glint.comment"] {
            let e = fs::File::open(format!("{path}:{name}")).unwrap_err();
            assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "{name}");
        }
        assert_eq!(read(&path), None);
    }

    #[test]
    fn only_what_differs_is_written() {
        let Some(s) = scratch() else { return };
        let path = make(&s, "a.txt");
        write(&path, &all()).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&path, perms).unwrap();
        assert!(write(&path, &all()).is_ok());
        let other = Marks { label: Some("Pending".into()), ..all() };
        assert!(write(&path, &other).is_err());
    }

    #[test]
    fn changing_the_label_leaves_the_comments_stream_as_it_was() {
        let Some(s) = scratch() else { return };
        let path = make(&s, "a.txt");
        write(&path, &all()).unwrap();
        let before = fs::read(format!("{path}:glint.comment")).unwrap();
        write(&path, &Marks { label: Some("Pending".into()), ..all() }).unwrap();
        assert_eq!(fs::read(format!("{path}:glint.comment")).unwrap(), before);
        assert_eq!(read(&path).unwrap().label.as_deref(), Some("Pending"));
    }

    #[test]
    fn a_folder_takes_marks_too() {
        let Some(s) = scratch() else { return };
        let folder = s.0.join("sub");
        fs::create_dir_all(&folder).unwrap();
        let path = folder.to_string_lossy().into_owned();
        write(&path, &all()).unwrap();
        assert_eq!(read(&path), Some(all()));
        assert_eq!(read(&format!("{path}\\")), Some(all()));
    }

    #[test]
    fn what_a_script_wrote_reads_tidy() {
        let Some(s) = scratch() else { return };
        let path = make(&s, "a.txt");
        fs::write(format!("{path}:glint.comment"), "one\r\ntwo").unwrap();
        fs::write(format!("{path}:glint.label"), "  Pending  \r\nmore\r\n").unwrap();
        let m = read(&path).unwrap();
        assert_eq!(m.comment.as_deref(), Some("one\ntwo"));
        assert_eq!(m.label.as_deref(), Some("Pending"));
    }

    #[test]
    fn tags_are_0_16s_both_ways() {
        let Some(s) = scratch() else { return };
        let path = make(&s, "a.txt");
        write(&path, &Marks { tags: vec!["a".into(), "b".into()], ..Marks::default() }).unwrap();
        assert_eq!(crate::tags::read(&path), ["a", "b"]);
        fs::write(format!("{path}:glint.tags"), "x\ny\n").unwrap();
        assert_eq!(read(&path).unwrap().tags, ["x", "y"]);
    }

    #[test]
    fn too_much_is_refused_with_words_and_nothing_is_written() {
        let Some(s) = scratch() else { return };
        let path = make(&s, "a.txt");
        let e = write(&path, &Marks { comment: Some("x".repeat(COMMENT_CAP + 1)), ..Marks::default() }).unwrap_err();
        assert!(e.contains("64 KB"), "{e}");
        assert!(fs::File::open(format!("{path}:glint.comment")).is_err());
        let e = write(&path, &Marks { label: Some("x".repeat(LABEL_CAP + 1)), ..Marks::default() }).unwrap_err();
        assert!(e.contains("256 bytes"), "{e}");
        let e = write(&path, &Marks { tags: vec!["t".repeat(5000)], ..Marks::default() }).unwrap_err();
        assert!(e.contains("4 KB"), "{e}");
    }

    #[test]
    fn a_long_comment_written_by_something_else_is_cut_on_a_whole_character() {
        let Some(s) = scratch() else { return };
        let path = make(&s, "a.txt");
        let mut bytes = vec![b'a'; COMMENT_CAP - 1];
        bytes.extend("\u{e9}".as_bytes()); // two bytes across the cap
        bytes.resize(70_000, b'b');
        fs::write(format!("{path}:glint.comment"), &bytes).unwrap();
        let c = read(&path).unwrap().comment.unwrap();
        assert!(c.len() <= COMMENT_CAP);
        assert!(c.ends_with('a') && !c.contains('\u{fffd}'));
    }

    #[test]
    fn a_stream_named_in_another_case_is_the_label() {
        let Some(s) = scratch() else { return };
        let path = make(&s, "a.txt");
        fs::write(format!("{path}:Glint.Label"), "Red\n").unwrap();
        assert_eq!(read(&path).unwrap().label.as_deref(), Some("Red"));
    }

    #[test]
    fn an_arabic_name_and_a_long_path_work() {
        let Some(s) = scratch() else { return };
        let arabic = make(&s, "\u{645}\u{644}\u{641}.txt");
        write(&arabic, &all()).unwrap();
        assert_eq!(read(&arabic), Some(all()));
        let mut deep = s.0.clone();
        while deep.to_string_lossy().len() < 300 {
            deep.push("d".repeat(40));
        }
        if fs::create_dir_all(&deep).is_err() {
            return;
        }
        let file = deep.join("f.txt");
        if fs::write(&file, b"x").is_err() {
            return;
        }
        let path = file.to_string_lossy().into_owned();
        write(&path, &all()).unwrap();
        assert_eq!(read(&path), Some(all()));
    }

    #[test]
    fn eighty_other_streams_do_not_hide_the_marks() {
        let Some(s) = scratch() else { return };
        let path = make(&s, "a.txt");
        for i in 0..80 {
            fs::write(format!("{path}:{i:0>60}"), b"x").unwrap();
        }
        write(&path, &all()).unwrap();
        assert_eq!(read(&path), Some(all()));
    }

    #[test]
    fn a_drive_itself_is_not_marked() {
        assert_eq!(read("C:\\"), None);
        assert!(write("C:\\", &all()).is_err());
        assert!(write(r"\\server\share\", &all()).is_err());
    }
}
