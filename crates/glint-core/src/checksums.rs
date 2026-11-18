//! Checksums of files: making them, and checking a folder's files against a sums file (`SHA256SUMS`, `foo.sha256`).
//!
//! Aware uses [`digest`] to compare a file with a checksum copied from a download page; the "Check files in this
//! folder" row uses [`check`] on a `.sha256`, `.sha512` or `.b3sum` file. Hashing is RustCrypto's SHA-2 and
//! BLAKE3, which the engine already builds for File X-Ray and the duplicate finder. MD5 and SHA-1 sums files are
//! not checked: no hashing crate the engine depends on makes them.

use glint_model::aware::{Algo, Sums, SumsWant};
use rayon::prelude::*;
use sha2::{Digest, Sha256, Sha512};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// How much of a file is read at once; `stop` is looked at between pieces.
const PIECE: usize = 1024 * 1024;

/// The checksums `want` asks for, made in one pass over the file, in lower-case hexadecimal. None when the file
/// can't be read, isn't `most` bytes long or less (when given), changed length while it was read, `stop` was set,
/// or nothing was asked for.
pub fn digest(path: &Path, want: SumsWant, most: Option<u64>, stop: &AtomicBool) -> Option<Sums> {
    if want.is_empty() {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    if most.is_some_and(|most| size > most) {
        return None;
    }
    let (mut sha256, mut sha512, mut blake3) = (Sha256::new(), Sha512::new(), blake3::Hasher::new());
    let mut buffer = vec![0u8; PIECE];
    let mut total: u64 = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        let piece = buffer.get(..read)?;
        if want.sha256 {
            sha256.update(piece);
        }
        if want.sha512 {
            sha512.update(piece);
        }
        if want.blake3 {
            blake3.update(piece);
        }
        total = total.saturating_add(read as u64);
    }
    // A file that grew or shrank while it was read has no checksum worth comparing.
    if total != size {
        return None;
    }
    Some(Sums {
        sha256: want.sha256.then(|| hex(&sha256.finalize())),
        sha512: want.sha512.then(|| hex(&sha512.finalize())),
        blake3: want.blake3.then(|| blake3.finalize().to_hex().to_string()),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The checksum of the one kind `algo` is, for `want`'s pick.
fn want_of(algo: Algo) -> SumsWant {
    SumsWant {
        sha256: algo == Algo::Sha256,
        sha512: algo == Algo::Sha512,
        blake3: algo == Algo::Blake3,
    }
}

/// The way a sums file's name says its checksums were made: by its extension. None for what can't be checked here.
pub fn algo_of(sums_file: &str) -> Option<Algo> {
    let ext = Path::new(sums_file).extension()?.to_string_lossy().to_lowercase();
    match ext.as_str() {
        "sha256" | "sha256sum" => Some(Algo::Sha256),
        "sha512" | "sha512sum" => Some(Algo::Sha512),
        "b3sum" | "blake3" => Some(Algo::Blake3),
        _ => None,
    }
}

/// Whether Glint offers to check the files of a sums file with this name: one whose checksums it can make.
pub fn is_sums_file(name: &str) -> bool {
    algo_of(name).is_some()
}

/// One line of a sums file: a checksum, and the file it is for (relative to the sums file's folder).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub hex: String,
    pub name: String,
}

/// The lines of a sums file made with `algo`: GNU's (`<hex>  <name>`, `<hex> *<name>`) and BSD's
/// (`SHA256 (<name>) = <hex>`). Blank lines and `#` comments are skipped. `skipped` counts the lines that were
/// neither: a checksum of the wrong length or kind, or a name that isn't a file below the sums file's folder.
pub fn parse(text: &str, algo: Algo) -> (Vec<Line>, usize) {
    let (mut lines, mut skipped) = (Vec::new(), 0);
    for raw in text.trim_start_matches('\u{feff}').lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let parsed = parse_gnu(line, algo).or_else(|| parse_bsd(line, algo));
        match parsed {
            Some(line) if below(&line.name) => lines.push(line),
            _ => skipped += 1,
        }
    }
    (lines, skipped)
}

fn parse_gnu(line: &str, algo: Algo) -> Option<Line> {
    // sha256sum writes a backslash first when a name needs escaping; such names aren't read.
    let (hex, rest) = line.split_once(char::is_whitespace)?;
    if hex.len() != algo.hex_len() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    // One space and then a space (text) or an asterisk (binary), or a tab.
    let name = rest
        .strip_prefix(' ')
        .or_else(|| rest.strip_prefix('\t'))
        .unwrap_or(rest);
    let name = name.strip_prefix(['*', ' ']).unwrap_or(name);
    (!name.is_empty()).then(|| Line {
        hex: hex.to_ascii_lowercase(),
        name: name.to_string(),
    })
}

fn parse_bsd(line: &str, algo: Algo) -> Option<Line> {
    let (tag, rest) = line.split_once(" (")?;
    let tag = tag.trim().replace('-', "").to_ascii_uppercase();
    let wanted = match algo {
        Algo::Sha256 => "SHA256",
        Algo::Sha512 => "SHA512",
        Algo::Blake3 => "BLAKE3",
        Algo::Md5 | Algo::Sha1 => return None,
    };
    if tag != wanted {
        return None;
    }
    let (name, hex) = rest.rsplit_once(") = ")?;
    if hex.len() != algo.hex_len() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) || name.is_empty() {
        return None;
    }
    Some(Line {
        hex: hex.to_ascii_lowercase(),
        name: name.to_string(),
    })
}

/// Whether `name` is a relative path that stays below the folder it is read in: no drive, no network path, no
/// `..`. A sums file is somebody else's text, and it doesn't get to make Glint read anywhere else.
fn below(name: &str) -> bool {
    let name = name.replace('/', "\\");
    if name.starts_with('\\') || name.contains(':') {
        return false;
    }
    name.split('\\').all(|part| !part.is_empty() && part != "..")
}

/// What checking a sums file's files found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// How the checksums were made.
    pub algo: Option<Algo>,
    /// The files it lists.
    pub listed: usize,
    /// Those whose checksum is the listed one.
    pub ok: usize,
    /// Those whose checksum isn't (names as the sums file gives them).
    pub mismatched: Vec<String>,
    /// Those that aren't there.
    pub missing: Vec<String>,
    /// Those that are there and couldn't be read.
    pub unreadable: Vec<String>,
    /// Lines that weren't checked: not a checksum for this kind of file, or a name outside the folder.
    pub skipped: usize,
}

impl Report {
    /// Everything it lists is there and matches (and something was listed).
    pub fn all_good(&self) -> bool {
        self.listed > 0
            && self.ok == self.listed
            && self.mismatched.is_empty()
            && self.missing.is_empty()
            && self.unreadable.is_empty()
    }
}

/// How one listed file turned out.
enum Outcome {
    Ok,
    Mismatch,
    Missing,
    Unreadable,
    /// `stop` was set.
    Stopped,
}

/// Checks every file the sums file at `sums_file` lists, in its folder. Err for what can't be checked at all (not
/// a sums file Glint can check, can't be read, stopped).
pub fn check(sums_file: &str, stop: &AtomicBool) -> Result<Report, String> {
    let algo = algo_of(sums_file).ok_or_else(|| "Glint can't check this kind of checksum file".to_string())?;
    let bytes = std::fs::read(sums_file).map_err(|e| format!("Couldn't read the checksum file: {e}"))?;
    // A real sums file is a few kilobytes; one of megabytes lists a very large folder, and that is fine too.
    let text = String::from_utf8_lossy(&bytes);
    let (lines, skipped) = parse(&text, algo);
    let folder: PathBuf = Path::new(sums_file).parent().map(Path::to_path_buf).unwrap_or_default();
    let want = want_of(algo);
    let outcomes: Vec<Outcome> = lines
        .par_iter()
        .map(|line| {
            let path = folder.join(line.name.replace('/', "\\"));
            if !path.exists() {
                return Outcome::Missing;
            }
            if stop.load(Ordering::Relaxed) {
                return Outcome::Stopped;
            }
            match digest(&path, want, None, stop) {
                Some(made) => match made.get(algo) {
                    Some(sum) if sum == line.hex => Outcome::Ok,
                    _ => Outcome::Mismatch,
                },
                None if stop.load(Ordering::Relaxed) => Outcome::Stopped,
                None => Outcome::Unreadable,
            }
        })
        .collect();
    if outcomes.iter().any(|outcome| matches!(outcome, Outcome::Stopped)) {
        return Err("Stopped".to_string());
    }
    let mut report = Report {
        algo: Some(algo),
        listed: lines.len(),
        skipped,
        ..Report::default()
    };
    for (line, outcome) in lines.into_iter().zip(outcomes) {
        match outcome {
            Outcome::Ok => report.ok += 1,
            Outcome::Mismatch => report.mismatched.push(line.name),
            Outcome::Missing => report.missing.push(line.name),
            Outcome::Unreadable => report.unreadable.push(line.name),
            Outcome::Stopped => {}
        }
    }
    Ok(report)
}

/// [`check`] on a thread of the engine's, for a window to wait for (nothing here is slow enough to hold the UI
/// thread, and nothing here is asked to be stopped: closing the window just stops waiting).
pub async fn check_sums(sums_file: String) -> Result<Report, String> {
    crate::fsx::blocking(move || check(&sums_file, &AtomicBool::new(false))).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A folder of the test's own, removed when it goes out of scope.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Scratch {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!("glint-checksums-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::create_dir_all(&dir);
            Scratch(dir)
        }

        fn write(&self, name: &str, bytes: &[u8]) -> String {
            let path = self.0.join(name);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, bytes);
            path.to_string_lossy().into_owned()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const BLAKE3_ABC: &str = "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85";
    const SHA512_ABC: &str = "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f";

    fn never() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn one_pass_makes_the_checksums_asked_for_and_only_those() {
        let dir = Scratch::new();
        let file = dir.write("abc.txt", b"abc");
        let all = SumsWant {
            sha256: true,
            sha512: true,
            blake3: true,
        };
        let made = digest(Path::new(&file), all, None, &never()).expect("readable");
        assert_eq!(made.sha256.as_deref(), Some(SHA256_ABC));
        assert_eq!(made.sha512.as_deref(), Some(SHA512_ABC));
        assert_eq!(made.blake3.as_deref(), Some(BLAKE3_ABC));
        let one = digest(
            Path::new(&file),
            SumsWant {
                blake3: true,
                ..SumsWant::default()
            },
            None,
            &never(),
        );
        assert_eq!(
            one,
            Some(Sums {
                blake3: Some(BLAKE3_ABC.into()),
                ..Sums::default()
            })
        );
        assert_eq!(
            digest(Path::new(&file), SumsWant::default(), None, &never()),
            None,
            "nothing asked"
        );
    }

    #[test]
    fn a_file_too_big_missing_or_stopped_has_no_checksum() {
        let dir = Scratch::new();
        let file = dir.write("abc.txt", b"abc");
        let want = SumsWant {
            sha256: true,
            ..SumsWant::default()
        };
        assert!(
            digest(Path::new(&file), want, Some(3), &never()).is_some(),
            "exactly the most is allowed"
        );
        assert_eq!(digest(Path::new(&file), want, Some(2), &never()), None);
        assert_eq!(digest(&dir.0.join("gone.txt"), want, None, &never()), None);
        assert_eq!(digest(Path::new(&file), want, None, &AtomicBool::new(true)), None);
    }

    #[test]
    fn a_file_of_several_pieces_is_hashed_the_same_as_in_one() {
        let dir = Scratch::new();
        let bytes: Vec<u8> = (0..(PIECE * 2 + 17)).map(|i| (i % 251) as u8).collect();
        let file = dir.write("big.bin", &bytes);
        let made = digest(
            Path::new(&file),
            SumsWant {
                sha256: true,
                blake3: true,
                ..SumsWant::default()
            },
            None,
            &never(),
        );
        let made = made.expect("readable");
        assert_eq!(made.sha256.as_deref(), Some(hex(&Sha256::digest(&bytes)).as_str()));
        assert_eq!(made.blake3.as_deref(), Some(blake3::hash(&bytes).to_hex().as_str()));
    }

    #[test]
    fn sums_files_are_told_by_their_extension_and_md5_is_not_one_we_check() {
        assert_eq!(algo_of("C:\\d\\a.SHA256"), Some(Algo::Sha256));
        assert_eq!(algo_of("a.sha512"), Some(Algo::Sha512));
        assert_eq!(algo_of("a.b3sum"), Some(Algo::Blake3));
        for no in ["a.md5", "a.sha1", "a.txt", "SHA256SUMS", ""] {
            assert_eq!(algo_of(no), None, "{no}");
            assert!(!is_sums_file(no));
        }
    }

    #[test]
    fn gnu_and_bsd_lines_are_read_and_comments_and_foreign_names_are_not() {
        let text = format!(
            "# made by hand\n\n{SHA256_ABC}  abc.txt\n{} *sub/dir/b.bin\nSHA256 (c d.txt) = {}\nSHA512 (x) = {SHA512_ABC}\n\
             {SHA256_ABC}  ..\\up.txt\n{SHA256_ABC}  C:\\win.txt\n{SHA256_ABC}  \\\\srv\\share\\f\n{SHA256_ABC}  /abs\n\
             tooshort  f\n{SHA256_ABC}\n",
            SHA256_ABC.to_uppercase(),
            "0".repeat(64),
        );
        let (lines, skipped) = parse(&text, Algo::Sha256);
        let names: Vec<&str> = lines.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["abc.txt", "sub/dir/b.bin", "c d.txt"]);
        assert_eq!(lines[1].hex, SHA256_ABC, "capitals are lowered");
        assert_eq!(
            skipped, 7,
            "another kind of sum, four names outside the folder, two broken lines"
        );
    }

    #[test]
    fn a_folder_is_checked_against_its_sums_file_and_every_kind_of_trouble_is_named() {
        let dir = Scratch::new();
        dir.write("abc.txt", b"abc");
        dir.write("sub\\inner.txt", b"abc");
        dir.write("changed.txt", b"not abc");
        dir.write("locked-ish", b"abc");
        let wrong = "1".repeat(64);
        let sums = dir.write(
            "pack.sha256",
            format!(
                "{SHA256_ABC}  abc.txt\n{SHA256_ABC}  sub/inner.txt\n{SHA256_ABC}  changed.txt\n{SHA256_ABC}  gone.txt\n{wrong}  locked-ish\n{SHA256_ABC}  ..\\x\nbad line\n"
            )
            .as_bytes(),
        );
        let report = check(&sums, &never()).expect("a sums file");
        assert_eq!(report.algo, Some(Algo::Sha256));
        assert_eq!((report.listed, report.ok, report.skipped), (5, 2, 2));
        assert_eq!(report.mismatched, ["changed.txt", "locked-ish"]);
        assert_eq!(report.missing, ["gone.txt"]);
        assert!(report.unreadable.is_empty());
        assert!(!report.all_good());
    }

    #[test]
    fn a_folder_that_all_matches_is_all_good_and_a_directory_in_a_file_name_is_unreadable() {
        let dir = Scratch::new();
        dir.write("abc.txt", b"abc");
        let _ = std::fs::create_dir_all(dir.0.join("folder"));
        let sums = dir.write("ok.sha256", format!("{SHA256_ABC}  abc.txt\n").as_bytes());
        let good = check(&sums, &never()).expect("a sums file");
        assert!(good.all_good(), "{good:?}");
        let mixed = dir.write("dir.sha256", format!("{SHA256_ABC}  folder\n").as_bytes());
        let report = check(&mixed, &never()).expect("a sums file");
        assert_eq!(report.unreadable, ["folder"]);
        let empty = dir.write("none.sha256", b"# nothing\n");
        assert!(
            !check(&empty, &never()).expect("a sums file").all_good(),
            "nothing listed is not good"
        );
    }

    #[test]
    fn blake3_sums_files_are_checked_and_what_cannot_be_checked_is_an_error() {
        let dir = Scratch::new();
        dir.write("abc.txt", b"abc");
        let b3 = dir.write("x.b3sum", format!("{BLAKE3_ABC}  abc.txt\n").as_bytes());
        assert!(check(&b3, &never()).expect("a sums file").all_good());
        let md5 = dir.write("x.md5", b"d41d8cd98f00b204e9800998ecf8427e  abc.txt\n");
        assert!(check(&md5, &never()).is_err());
        assert!(check(&dir.0.join("gone.sha256").to_string_lossy(), &never()).is_err());
        let sums = dir.write("s.sha256", format!("{SHA256_ABC}  abc.txt\n").as_bytes());
        assert_eq!(check(&sums, &AtomicBool::new(true)), Err("Stopped".to_string()));
    }
}
