//! Text in whatever encoding it was saved in. UTF-8 and UTF-16 (by their byte order marks, or UTF-16's
//! zero bytes) are read as they are; anything else is an older encoding, which Firefox's detector
//! (chardetng) guesses, with this PC's own code page as the hint (Arabic Windows-1256 on an Arabic
//! Windows, Cyrillic on a Russian one), and encoding_rs decodes. Convert to UTF-8 rewrites such files.

use crate::actions::{self, Job, Output, Progress, Report};
use encoding_rs::Encoding;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::host::ipc::Channel;

/// Text, and what it was read as.
pub struct Decoded {
    pub text: String,
    /// encoding_rs's name ("UTF-8", "UTF-16LE", "windows-1256"…).
    pub encoding: &'static str,
    /// Whether the encoding was guessed (an older one, without a mark to say so).
    pub guessed: bool,
    /// It isn't text.
    pub binary: bool,
}

/// How much of a file the guess looks at.
const LOOK: usize = 256 * 1024;

/// Decodes `bytes` as the text they are. `truncated` says they're only the start of the file (a
/// character cut at the end then isn't an error).
pub fn decode(bytes: &[u8], truncated: bool) -> Decoded {
    if let Some((encoding, bom)) = Encoding::for_bom(bytes) {
        let (text, _) = encoding.decode_without_bom_handling(&bytes[bom..]);
        return Decoded { text: text.into_owned(), encoding: encoding.name(), guessed: false, binary: false };
    }
    let head = &bytes[..bytes.len().min(8000)];
    if head.contains(&0) {
        // UTF-16 without a mark: most of one byte of each pair is zero in Latin text.
        if let Some(encoding) = utf16_without_bom(head) {
            let (text, _) = encoding.decode_without_bom_handling(bytes);
            return Decoded { text: text.into_owned(), encoding: encoding.name(), guessed: true, binary: false };
        }
        return Decoded { text: String::new(), encoding: "", guessed: false, binary: true };
    }
    if utf8_valid(bytes, truncated) {
        return Decoded { text: String::from_utf8_lossy(bytes).into_owned(), encoding: "UTF-8", guessed: false, binary: false };
    }
    let encoding = guess(bytes, truncated);
    let (text, _) = encoding.decode_without_bom_handling(bytes);
    Decoded { text: text.into_owned(), encoding: encoding.name(), guessed: true, binary: false }
}

/// Decodes `bytes` as `label` says (an encoding's name), for choosing another than the guess.
pub fn decode_as(bytes: &[u8], label: &str) -> Option<String> {
    let encoding = Encoding::for_label(label.as_bytes())?;
    let (bom_encoding, bom) = Encoding::for_bom(bytes).unwrap_or((encoding, 0));
    let body = if bom_encoding == encoding { &bytes[bom..] } else { bytes };
    Some(encoding.decode_without_bom_handling(body).0.into_owned())
}

/// Valid UTF-8, but for a character the cut at the end may have split.
fn utf8_valid(bytes: &[u8], truncated: bool) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(e) => truncated && e.error_len().is_none() && bytes.len() - e.valid_up_to() < 4,
    }
}

fn utf16_without_bom(head: &[u8]) -> Option<&'static Encoding> {
    let pairs = head.len() / 2;
    if pairs < 8 {
        return None;
    }
    let pairs_of = || head.as_chunks::<2>().0.iter();
    let zero_odd = pairs_of().filter(|p| p[1] == 0 && p[0] != 0).count();
    let zero_even = pairs_of().filter(|p| p[0] == 0 && p[1] != 0).count();
    if zero_odd * 10 >= pairs * 4 && zero_even * 10 < pairs {
        Some(encoding_rs::UTF_16LE)
    } else if zero_even * 10 >= pairs * 4 && zero_odd * 10 < pairs {
        Some(encoding_rs::UTF_16BE)
    } else {
        None
    }
}

/// chardetng's guess for an older encoding, hinted by this PC's code page.
fn guess(bytes: &[u8], truncated: bool) -> &'static Encoding {
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    let look = &bytes[..bytes.len().min(LOOK)];
    detector.feed(look, !truncated && look.len() == bytes.len());
    detector.guess(region_hint(), chardetng::Utf8Detection::Deny)
}

/// A country domain chardetng takes as the region's hint, from Windows' code page for programs that
/// aren't Unicode (what old files on this PC were most likely saved in).
fn region_hint() -> Option<&'static [u8]> {
    #[cfg(windows)]
    let page = unsafe { windows::Win32::Globalization::GetACP() };
    #[cfg(not(windows))]
    let page = 1252u32;
    Some(match page {
        1256 => b"eg",
        1251 => b"ru",
        1253 => b"gr",
        1255 => b"il",
        932 => b"jp",
        936 => b"cn",
        949 => b"kr",
        950 => b"tw",
        874 => b"th",
        1254 => b"tr",
        1250 => b"cz",
        1257 => b"lt",
        1258 => b"vn",
        _ => return None,
    })
}

static STOP: AtomicBool = AtomicBool::new(false);

pub fn text_cancel() {
    STOP.store(true, Ordering::SeqCst);
}

/// Convert to UTF-8: each file read as `encoding` (or as guessed, when None) and written as UTF-8,
/// with a byte order mark for CSV and TSV files (Excel reads UTF-8 by it). Line endings and the date stay.
pub async fn text_to_utf8(window: crate::host::Window, files: Vec<String>, encoding: Option<String>, output: Output, on_progress: Channel<Progress>) -> Result<Report, String> {
    STOP.store(false, Ordering::SeqCst);
    let owner = crate::shell::owner(&window);
    crate::fsx::blocking(move || {
        let jobs: Vec<Job> = files.iter().map(Job::one).collect();
        actions::run(owner, "utf8", &jobs, &output, 4, &STOP, &|p| {
            let _ = on_progress.send(p);
        }, |job, _part, work| {
            let path = &job.inputs[0];
            let bytes = std::fs::read(path).map_err(crate::fsx::file_error)?;
            let text = match &encoding {
                Some(label) => decode_as(&bytes, label).ok_or_else(|| format!("Glint doesn't know the encoding “{label}”"))?,
                None => {
                    let decoded = decode(&bytes, false);
                    if decoded.binary {
                        return Err("It isn't a text file".into());
                    }
                    if decoded.encoding == "UTF-8" {
                        return Err("It's UTF-8 already".into());
                    }
                    decoded.text
                }
            };
            let made = work.join(path.file_name().ok_or("No name")?);
            let mut out = Vec::with_capacity(text.len() + 3);
            if bom_wanted(path) {
                out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            }
            out.extend_from_slice(text.as_bytes());
            std::fs::write(&made, out).map_err(crate::fsx::file_error)?;
            if let Ok(modified) = std::fs::metadata(path).and_then(|m| m.modified()) {
                let _ = std::fs::File::options().write(true).open(&made).and_then(|f| f.set_modified(modified));
            }
            Ok(vec![made])
        })
    })
    .await
}

fn bom_wanted(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("csv") || e.eq_ignore_ascii_case("tsv"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_text_in_its_own_encoding() {
        // "مرحبا بالعالم، هذا ملف نصي قديم" in Windows-1256, as an Arabic Notepad saved it.
        let arabic = "مرحبا بالعالم، هذا ملف نصي قديم كتب بترميز ويندوز العربي.\r\nالسطر الثاني من الملف.";
        let (bytes, _, _) = encoding_rs::WINDOWS_1256.encode(arabic);
        let decoded = decode(&bytes, false);
        assert_eq!((decoded.encoding, decoded.guessed, decoded.binary), ("windows-1256", true, false));
        assert_eq!(decoded.text, arabic);

        // UTF-8, with and without its mark; UTF-16 with and without.
        let d = decode("نص عربي".as_bytes(), false);
        assert_eq!((d.encoding, d.guessed, d.text.as_str()), ("UTF-8", false, "نص عربي"));
        let mut marked = vec![0xEF, 0xBB, 0xBF];
        marked.extend_from_slice("abc".as_bytes());
        assert_eq!(decode(&marked, false).text, "abc");
        let utf16: Vec<u8> = "Hello, plain old text in UTF-16".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let d = decode(&utf16, false);
        assert_eq!((d.encoding, d.text.as_str()), ("UTF-16LE", "Hello, plain old text in UTF-16"));
        let mut bom16 = vec![0xFF, 0xFE];
        bom16.extend_from_slice(&utf16);
        assert_eq!(decode(&bom16, false).encoding, "UTF-16LE");

        // A character cut at the end of a preview is still UTF-8.
        let cut = &"عربي".as_bytes()[..7];
        assert_eq!(decode(cut, true).encoding, "UTF-8");

        // Binary stays binary; a chosen encoding decodes as told.
        assert!(decode(&[0x4D, 0x5A, 0x90, 0, 3, 0, 0, 0, 4, 0, 0, 0, 0xFF, 0xFF, 0, 0], false).binary);
        assert_eq!(decode_as(&bytes, "windows-1256").unwrap(), arabic);
        assert!(decode_as(&bytes, "no-such-encoding").is_none());
    }
}
