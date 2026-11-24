//! Finding XYplorer's `tag.dat` (PLAN.md 7.1, F27), reading only: `%APPDATA%\XYplorer`, a portable copy's `Data`
//! folder, or where its `Startup.ini` says (`[Appdata] Path=…`). XYplorer's files are only read (M7): opened with
//! sharing that lets XYplorer write, read whole, and closed again; never written, moved or locked.

use glint_model::marks::xyplorer::{self, XY_MAX_BYTES};
use std::io::Read;
use std::path::{Path, PathBuf};

/// A `tag.dat` found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub path: String,
    /// How many items it marks.
    pub items: u64,
    /// When it last changed, Unix ms.
    pub changed: i64,
}

/// A `Startup.ini` is a few lines; one bigger than this is not read.
const INI_MAX_BYTES: usize = 64 << 10;

/// Where XYplorer's `tag.dat` is, if it has one.
pub fn xyplorer_found() -> Option<Found> {
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    found_in(appdata.as_deref(), &programs())
}

/// The first `tag.dat` that exists and reads as one: in `XYplorer` under `appdata` (the `%APPDATA%` folder), then,
/// for each folder in `programs` (where an `XYplorer.exe` is), where its `Startup.ini` says, then its `Data` folder.
pub fn found_in(appdata: Option<&Path>, programs: &[PathBuf]) -> Option<Found> {
    let mut places: Vec<PathBuf> = appdata.map(|a| a.join("XYplorer")).into_iter().collect();
    for program in programs {
        places.extend(startup_dir(program));
        places.push(program.join("Data"));
    }
    places.iter().find_map(|dir| found_at(&dir.join("tag.dat")))
}

/// XYplorer's own folder for storage modes 1 and 2 (7.3): two folders above a portable copy's `Data\tag.dat`
/// when `XYplorer.exe` is there; else the one Windows knows it by; else None.
pub fn base_of(tag_dat: &Path) -> Option<PathBuf> {
    let portable = tag_dat.parent().and_then(Path::parent).filter(|p| p.join("XYplorer.exe").is_file());
    portable.map(Path::to_path_buf).or_else(|| programs().into_iter().next())
}

/// A file read whole, at most `cap` bytes plus one (so the caller can tell it was more). Opened for reading with
/// every kind of sharing, so XYplorer can write while this reads; closed when it returns.
pub(crate) fn read_shared(path: &Path, cap: usize) -> std::io::Result<Vec<u8>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE.
        options.share_mode(7);
    }
    let mut bytes = Vec::new();
    options.open(path)?.take(cap as u64 + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// `path` as a `Found`, when it is a `tag.dat` of a size that is read.
fn found_at(path: &Path) -> Option<Found> {
    let bytes = read_shared(path, XY_MAX_BYTES).ok()?;
    let file = xyplorer::read(&bytes).ok()?;
    Some(Found {
        path: path.to_string_lossy().into_owned(),
        items: file.items.len() as u64,
        changed: modified_ms(path),
    })
}

/// When a file last changed, Unix ms (0 when it can't be told).
pub(crate) fn modified_ms(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// A file's text: UTF-16 by its mark or when its second byte is 0, else UTF-8 (what isn't is replaced).
fn text_of(bytes: &[u8]) -> String {
    let utf16 = |b: &[u8]| String::from_utf16_lossy(&b.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect::<Vec<_>>());
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ if bytes.get(1) == Some(&0) => utf16(bytes),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// The folder `program\Startup.ini` gives as XYplorer's app data (`[Appdata]` `Path=`): `<xypath>` and
/// `%VARIABLES%` filled in, a relative one taken from the program's folder.
fn startup_dir(program: &Path) -> Option<PathBuf> {
    let bytes = read_shared(&program.join("Startup.ini"), INI_MAX_BYTES).ok().filter(|b| b.len() <= INI_MAX_BYTES)?;
    let text = text_of(&bytes);
    let mut inside = false;
    let mut path = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line.eq_ignore_ascii_case("[appdata]");
        } else if let (true, Some((key, value))) = (inside, line.split_once('=')) {
            if key.trim().eq_ignore_ascii_case("path") {
                path = Some(value.trim().trim_matches('"').to_string());
            }
        }
    }
    let path = path.filter(|p| !p.is_empty())?;
    let home = program.to_string_lossy();
    let lower = path.to_lowercase();
    let path = match lower.find("<xypath>") {
        // `find` on the lower-cased copy: the tag is ASCII, so its place is the same in both.
        Some(at) => format!("{}{}{}", &path[..at], home.trim_end_matches('\\'), &path[at + 8..]),
        None => path,
    };
    let path = PathBuf::from(crate::fsx::expand_vars(&path));
    Some(if path.is_absolute() { path } else { program.join(path) })
}

/// The folders where Windows says an `XYplorer.exe` is (App Paths and the uninstall entries), each with the
/// program in it. Registry reads only.
#[cfg(windows)]
fn programs() -> Vec<PathBuf> {
    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ,
        RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    };

    /// A string value (environment variables filled in) of `key` under `root`; the key's own when `name` is empty.
    fn string(root: HKEY, key: &str, name: &str) -> Option<String> {
        let (key, name) = (HSTRING::from(key), HSTRING::from(name));
        let mut buf = vec![0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: `buf` holds `size` bytes; the call writes at most that many and says how many it wrote.
        let got = unsafe {
            RegGetValueW(root, &key, &name, RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut size))
        };
        if got != ERROR_SUCCESS {
            return None;
        }
        let units = buf.get(..(size as usize / 2))?;
        let text = String::from_utf16_lossy(&units[..units.iter().position(|&u| u == 0).unwrap_or(units.len())]);
        Some(text).filter(|t| !t.trim().is_empty())
    }

    /// The names of `key`'s subkeys (at most 2,000).
    fn subkeys(root: HKEY, key: &str) -> Vec<String> {
        let mut opened = HKEY::default();
        // SAFETY: `opened` is a valid out-parameter; it is closed below.
        if unsafe { RegOpenKeyExW(root, &HSTRING::from(key), None, KEY_READ, &mut opened) } != ERROR_SUCCESS {
            return Vec::new();
        }
        let mut names = Vec::new();
        for i in 0..2000 {
            let mut buf = [0u16; 256];
            let mut len = buf.len() as u32;
            // SAFETY: `buf` holds `len` units; the call writes at most that many.
            let got = unsafe { RegEnumKeyExW(opened, i, Some(PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None) };
            if got == ERROR_NO_MORE_ITEMS {
                break;
            }
            if got == ERROR_SUCCESS {
                names.push(String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]));
            }
        }
        // SAFETY: `opened` was opened above and is closed once.
        let _ = unsafe { RegCloseKey(opened) };
        names
    }

    /// A program path as a registry holds it: quoted, with arguments or an icon number after it.
    fn exe_folder(value: &str) -> Option<PathBuf> {
        let v = value.trim();
        let v = match v.strip_prefix('"') {
            Some(rest) => rest.split('"').next().unwrap_or(""),
            None => v.split(',').next().unwrap_or(""),
        };
        Path::new(v.trim()).parent().map(Path::to_path_buf)
    }

    let mut found: Vec<PathBuf> = Vec::new();
    let paths = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\XYplorer.exe";
    let paths32 = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths\XYplorer.exe";
    let uninstall = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
    let uninstall32 = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for key in [paths, paths32] {
            found.extend(string(root, key, "").and_then(|v| exe_folder(&v)));
        }
        for base in [uninstall, uninstall32] {
            for sub in subkeys(root, base) {
                let key = format!(r"{base}\{sub}");
                let named = string(root, &key, "DisplayName").is_some_and(|n| n.to_lowercase().starts_with("xyplorer"));
                if !named {
                    continue;
                }
                found.extend(string(root, &key, "InstallLocation").map(PathBuf::from));
                found.extend(string(root, &key, "DisplayIcon").and_then(|v| exe_folder(&v)));
            }
        }
    }
    let mut seen: Vec<String> = Vec::new();
    found.retain(|p| {
        let key = p.to_string_lossy().to_lowercase();
        let new = !seen.contains(&key) && p.join("XYplorer.exe").is_file();
        seen.push(key);
        new
    });
    found
}

#[cfg(not(windows))]
fn programs() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/../../tests/fixtures/marks/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap_or_default()
    }

    /// A folder of the test's own, empty.
    fn folder() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("glint-xyfind-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::create_dir_all(&d);
        d
    }

    fn put(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = name.split('/').fold(dir.to_path_buf(), |p, part| p.join(part));
        let _ = std::fs::create_dir_all(p.parent().unwrap_or(dir));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn a_tag_dat_in_the_app_data_folder_is_found_with_its_items_and_date() {
        let dir = folder();
        let tag = put(&dir, "XYplorer/tag.dat", &fixture("v5-storage0-bom-crlf.dat"));
        let found = found_in(Some(&dir), &[]).unwrap();
        assert_eq!(found.path, tag.to_string_lossy());
        assert_eq!(found.items, 7);
        assert_eq!(found.changed, modified_ms(&tag));
        assert!(found.changed > 1_600_000_000_000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_portable_copys_data_folder_is_found() {
        let dir = folder();
        let program = dir.join("portable");
        let tag = put(&program, "Data/tag.dat", &fixture("real-v5-zh.dat"));
        let found = found_in(Some(&dir.join("no-appdata")), &[program]).unwrap();
        assert_eq!((found.path, found.items), (tag.to_string_lossy().into_owned(), 2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn where_a_startup_ini_points_is_found_before_data_beside_the_program() {
        let dir = folder();
        let program = dir.join("prog");
        put(&program, "Data/tag.dat", &fixture("v3-storage0.dat"));
        let there = put(&dir, "elsewhere/tag.dat", &fixture("real-v5-zh.dat"));
        let ini = format!("[General]\r\nPath=wrong\r\n[Appdata]\r\nPath={}\r\n", dir.join("elsewhere").display());
        put(&program, "Startup.ini", ini.as_bytes());
        let found = found_in(None, std::slice::from_ref(&program)).unwrap();
        assert_eq!(found.path, there.to_string_lossy());
        // <xypath> and a relative path are taken from the program's folder, and the file may be UTF-16.
        let ini16: Vec<u8> = "[appdata]\r\npath=<xypath>\\Own\r\n".encode_utf16().flat_map(u16::to_le_bytes).collect();
        put(&program, "Startup.ini", &ini16);
        let own = put(&program, "Own/tag.dat", &fixture("v5-storage2.dat"));
        assert_eq!(found_in(None, std::slice::from_ref(&program)).unwrap().path, own.to_string_lossy());
        put(&program, "Startup.ini", b"[Appdata]\nPath=Own\n");
        assert_eq!(found_in(None, &[program]).unwrap().path, own.to_string_lossy());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_anywhere_and_files_that_are_not_tag_databases_are_not_found() {
        let dir = folder();
        assert_eq!(found_in(Some(&dir), &[dir.join("none")]), None);
        assert_eq!(found_in(None, &[]), None);
        put(&dir, "XYplorer/tag.dat", b"just some text");
        assert_eq!(found_in(Some(&dir), &[]), None);
        put(&dir, "XYplorer/tag.dat", &[]);
        assert_eq!(found_in(Some(&dir), &[]), None);
        // A damaged one is passed over for the next place.
        let good = put(&dir, "p/Data/tag.dat", &fixture("v5-storage2.dat"));
        assert_eq!(found_in(Some(&dir), &[dir.join("p")]).unwrap().path, good.to_string_lossy());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_it_leaves_its_bytes_and_date_as_they_were() {
        let dir = folder();
        let tag = put(&dir, "XYplorer/tag.dat", &fixture("v5-storage0-bom-crlf.dat"));
        let (bytes, changed) = (std::fs::read(&tag).unwrap(), modified_ms(&tag));
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(found_in(Some(&dir), &[]).is_some());
        assert_eq!((std::fs::read(&tag).unwrap(), modified_ms(&tag)), (bytes, changed));
        // Not held open: it can be removed at once.
        std::fs::remove_file(&tag).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_over_the_cap_is_read_one_byte_past_it_and_no_more() {
        let dir = folder();
        let big = put(&dir, "big.bin", &[b'x'; 5000]);
        assert_eq!(read_shared(&big, 1000).unwrap().len(), 1001);
        assert_eq!(read_shared(&big, 5000).unwrap().len(), 5000);
        assert!(read_shared(&dir.join("missing"), 10).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_portable_copys_folder_is_xyplorers_own() {
        let dir = folder();
        let tag = put(&dir, "app/Data/tag.dat", &fixture("v5-storage2.dat"));
        put(&dir, "app/XYplorer.exe", b"MZ");
        assert_eq!(base_of(&tag), Some(dir.join("app")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
