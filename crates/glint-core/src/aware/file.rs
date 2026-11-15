//! What can be found out about one file: one function per part ([`super::Part`]).
//!
//! Each takes what it needs and returns an `Option`: `None` is "not known", which the window draws as nothing
//! (rule R1 of docs/gpui/aware/PLAN.md). None of them panics, whatever the path, and none of them reads a cloud
//! file that isn't on the disk: the probe doesn't ask them to.

use super::{Media, Origin, Signer, Text};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

/// The names of the apps that have the file open, each once, in the order Windows lists them. `Some(empty)`
/// means nobody has it open; `None` that Windows couldn't be asked.
pub fn locked_by(path: &str) -> Option<Vec<String>> {
    let report = crate::locks::who_locks_now(vec![path.to_string()]).ok()?;
    let mut names: Vec<String> = Vec::new();
    for process in report.processes {
        if !process.name.is_empty() && !names.contains(&process.name) {
            names.push(process.name);
        }
    }
    Some(names)
}

/// Where the file came from, from its Mark of the Web. `None` when it has no mark.
pub fn origin(path: &str) -> Option<Origin> {
    let zone = crate::xray::zone_info(path)?;
    let host = zone
        .host_url
        .as_deref()
        .and_then(host_of)
        .or_else(|| zone.referrer_url.as_deref().and_then(host_of))
        .unwrap_or_default();
    Some(Origin { host, blocked: zone.zone_id >= 3 })
}

/// The host of an address, without a leading "www.".
fn host_of(address: &str) -> Option<String> {
    let url = url::Url::parse(address.trim()).ok()?;
    let host = url.host_str()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    (!host.is_empty()).then(|| host.to_string())
}

/// Who signed it: `Signed(name)` when Windows says the signature holds, `Unsigned` when there is none. Anything
/// else (expired, untrusted, not sure) is `None`: Glint doesn't say what it isn't sure of.
pub fn signer(path: &str) -> Option<Signer> {
    let signature = crate::sign::verify(path);
    match signature.state {
        "valid" => signature.signer.filter(|name| !name.is_empty()).map(Signer::Signed),
        "unsigned" => Some(Signer::Unsigned),
        _ => None,
    }
}

/// A picture's or video's size and a video's or song's length, as the media reader has them (zero for what it
/// doesn't say). `None` when it says none of the three.
pub fn media(path: &str) -> Option<Media> {
    let details = crate::media::details_now(path)?;
    if details.width.is_none() && details.height.is_none() && details.duration.is_none() {
        return None;
    }
    let seconds = details.duration.filter(|s| s.is_finite() && *s > 0.0).unwrap_or(0.0);
    Some(Media { width: details.width.unwrap_or(0), height: details.height.unwrap_or(0), seconds })
}

/// How much of a text file is read at a time.
const PIECE: usize = 1 << 20;
/// How much of its start is looked at for a zero byte, which says it isn't text.
const SNIFF: usize = 8 << 10;

/// Reads until `buf` is full or the file ends; how many bytes it got.
fn fill(file: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match file.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(got)
}

/// A file's lines, counted with SIMD in 1 MB pieces, or `binary` when a zero byte is in its first 8 KB. `None`
/// when it can't be read or `stop` was set.
pub fn text(path: &str, stop: &AtomicBool) -> Option<Text> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; PIECE];
    let mut lines = 0u64;
    let mut last = None;
    let mut first = true;
    loop {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        let got = fill(&mut file, &mut buf).ok()?;
        let piece = buf.get(..got)?;
        if piece.is_empty() {
            break;
        }
        if first {
            first = false;
            if memchr::memchr(0, piece.get(..SNIFF.min(piece.len()))?).is_some() {
                return Some(Text { lines: 0, binary: true });
            }
        }
        lines += memchr::memchr_iter(b'\n', piece).count() as u64;
        last = piece.last().copied();
        if got < buf.len() {
            break;
        }
    }
    // A last line with no end of line still counts.
    if last.is_some_and(|byte| byte != b'\n') {
        lines += 1;
    }
    Some(Text { lines, binary: false })
}

/// A PDF's page count, from Windows' own reader. `None` for one with a password or one Windows can't read.
#[cfg(windows)]
pub fn pages(path: &str) -> Option<u32> {
    use windows::core::HSTRING;
    use windows::Data::Pdf::PdfDocument;
    use windows::Storage::StorageFile;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    let path = HSTRING::from(path);
    // A thread of its own in the multithreaded apartment, where the calls' waits are allowed (as ocr.rs does).
    std::thread::scope(|scope| {
        scope
            .spawn(|| -> windows::core::Result<Option<u32>> {
                // SAFETY: plain COM initialisation of this scoped thread.
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                }
                let file = StorageFile::GetFileFromPathAsync(&path)?.join()?;
                let document = PdfDocument::LoadFromFileAsync(&file)?.join()?;
                if document.IsPasswordProtected()? {
                    return Ok(None);
                }
                Ok(Some(document.PageCount()?))
            })
            .join()
            .ok()
    })?
    .ok()?
    .filter(|&n| n > 0)
}

#[cfg(not(windows))]
pub fn pages(_path: &str) -> Option<u32> {
    None
}

/// Where a shortcut points.
pub fn target(path: &str) -> Option<String> {
    crate::recent::link_target(Path::new(path)).filter(|t| !t.is_empty())
}

/// What each extension was asked about already.
static OPENS_WITH: LazyLock<Mutex<HashMap<String, Option<String>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The name of the app that opens this kind of file ("Photos"), asked of Windows once per extension.
pub fn opens_with(path: &str) -> Option<String> {
    let ext = Path::new(path).extension()?.to_string_lossy().to_lowercase();
    if ext.is_empty() {
        return None;
    }
    if let Some(known) = OPENS_WITH.lock().unwrap_or_else(|e| e.into_inner()).get(&ext) {
        return known.clone();
    }
    let name = friendly_name(&ext);
    OPENS_WITH.lock().unwrap_or_else(|e| e.into_inner()).insert(ext, name.clone());
    name
}

#[cfg(windows)]
fn friendly_name(ext: &str) -> Option<String> {
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_NONE, ASSOCSTR_FRIENDLYAPPNAME};

    let dotted = HSTRING::from(format!(".{ext}"));
    let mut buf = [0u16; 260];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` characters and outlives the call; the extension is a valid wide string.
    let asked = unsafe {
        AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_FRIENDLYAPPNAME, &dotted, PCWSTR::null(), Some(PWSTR(buf.as_mut_ptr())), &mut len)
    };
    if asked.is_err() {
        return None;
    }
    // `len` counts the closing zero.
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let name = String::from_utf16_lossy(buf.get(..end)?);
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(not(windows))]
fn friendly_name(_ext: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A folder of the test's own, removed when it goes out of scope.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new() -> Scratch {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!("glint-aware-file-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::create_dir_all(&dir);
            Scratch(dir)
        }

        fn write(&self, name: &str, bytes: &[u8]) -> String {
            let path = self.0.join(name);
            let _ = std::fs::write(&path, bytes);
            path.to_string_lossy().into_owned()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn lines_of(bytes: &[u8]) -> Option<Text> {
        let dir = Scratch::new();
        let path = dir.write("t.txt", bytes);
        text(&path, &AtomicBool::new(false))
    }

    #[test]
    fn a_text_file_has_a_line_for_each_end_of_line_and_one_more_for_an_unended_last_line() {
        assert_eq!(lines_of(b"a\nb\n"), Some(Text { lines: 2, binary: false }));
        assert_eq!(lines_of(b"a\nb"), Some(Text { lines: 2, binary: false }));
        assert_eq!(lines_of(b""), Some(Text { lines: 0, binary: false }));
    }

    #[test]
    fn lines_are_counted_right_across_the_pieces() {
        let mut line = vec![b'x'; 99];
        line.push(b'\n');
        let big: Vec<u8> = line.iter().copied().cycle().take(100 * 30_000).collect();
        assert!(big.len() > 2 * PIECE);
        assert_eq!(lines_of(&big), Some(Text { lines: 30_000, binary: false }));
    }

    #[test]
    fn a_file_with_a_zero_byte_near_its_start_is_binary() {
        let mut bytes = b"MZ\0\0 some program".to_vec();
        bytes.extend(std::iter::repeat_n(b'\n', 50));
        assert_eq!(lines_of(&bytes), Some(Text { lines: 0, binary: true }));
    }

    #[test]
    fn a_stop_flag_set_before_the_call_gives_nothing() {
        let dir = Scratch::new();
        let path = dir.write("t.txt", b"a\nb\n");
        assert_eq!(text(&path, &AtomicBool::new(true)), None);
        assert_eq!(text(&dir.0.join("missing.txt").to_string_lossy(), &AtomicBool::new(false)), None);
    }

    fn mark(dir: &Scratch, name: &str, zone: &str) -> Option<String> {
        let path = dir.write(name, b"data");
        std::fs::write(format!("{path}:Zone.Identifier"), zone).ok()?;
        Some(path)
    }

    #[test]
    fn a_file_from_the_internet_says_where_from_and_is_blocked() {
        let dir = Scratch::new();
        let zone = "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://www.example.com/a/file.zip\r\n";
        let Some(path) = mark(&dir, "a.zip", zone) else { return };
        assert_eq!(origin(&path), Some(Origin { host: "example.com".into(), blocked: true }));
        let Some(path) = mark(&dir, "b.zip", "[ZoneTransfer]\r\nZoneId=1\r\n") else { return };
        assert_eq!(origin(&path), Some(Origin { host: String::new(), blocked: false }));
    }

    #[test]
    fn the_referrer_stands_in_when_there_is_no_host_url() {
        let dir = Scratch::new();
        let zone = "[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl=https://github.com/x/y/releases\r\n";
        let Some(path) = mark(&dir, "a.zip", zone) else { return };
        assert_eq!(origin(&path), Some(Origin { host: "github.com".into(), blocked: true }));
    }

    #[test]
    fn a_file_with_no_mark_has_no_origin() {
        let dir = Scratch::new();
        let path = dir.write("plain.txt", b"hi");
        assert_eq!(origin(&path), None);
    }

    #[test]
    fn a_file_nobody_has_open_is_locked_by_nobody_and_a_missing_one_does_not_panic() {
        let dir = Scratch::new();
        let path = dir.write("free.txt", b"hi");
        if let Some(names) = locked_by(&path) {
            assert!(names.is_empty(), "{names:?}");
        }
        let _ = locked_by(&dir.0.join("missing.txt").to_string_lossy());
    }

    #[test]
    fn a_plain_text_file_is_unsigned_and_notepad_is_not() {
        let dir = Scratch::new();
        let path = dir.write("t.txt", b"hello");
        assert_eq!(signer(&path), Some(Signer::Unsigned));
        let notepad = r"C:\Windows\System32\notepad.exe";
        assert_ne!(signer(notepad), Some(Signer::Unsigned));
    }

    #[test]
    fn a_name_with_no_extension_opens_with_nothing_and_asking_twice_agrees() {
        assert_eq!(opens_with("README"), None);
        assert_eq!(opens_with("C:\\x\\"), None);
        assert_eq!(opens_with("a.txt"), opens_with("b.TXT"));
    }

    #[test]
    fn hosts_lose_their_www() {
        assert_eq!(host_of("https://www.example.com/a"), Some("example.com".into()));
        assert_eq!(host_of("https://cdn.example.com/a"), Some("cdn.example.com".into()));
        assert_eq!(host_of("not an address"), None);
    }
}
