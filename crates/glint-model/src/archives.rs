//! Ported from src/lib/archives.ts.
//!
//! The archives Glint reads (glint-core's archive/read.rs): which file names Enter opens like folders.
//!
//! NOTE(port): the TypeScript file also held three zustand stores (`usePasswordAsk`, `useCompressAsk`,
//! `useUnlocks`) and `askArchivePassword`. They are live UI state, so the GPUI layer owns them and they
//! are not ported here.

/// Disk images, installers and packages 7-Zip reads (archive/image.rs): Enter browses these...
pub const IMAGES_BROWSED: [&str; 25] = [
    "dmg", "wim", "esd", "swm", "vmdk", "qcow2", "qcow", "vdi", "squashfs", "sfs", "deb", "rpm", "cpio", "chm", "xar",
    "apfs", "hfs", "hfsx", "ext2", "ext3", "ext4", "lzh", "lha", "arj", "cramfs",
];

/// ...and these it looks inside on request (Windows mounts discs and disks, and runs programs).
pub const IMAGES_ON_REQUEST: [&str; 5] = ["iso", "img", "vhd", "vhdx", "exe"];

/// Extensions Enter opens like a folder: archives, tars (.tar.gz ends in "gz" like a lone .gz), single
/// compressed files, and ncdu exports. This is the TypeScript `BROWSED` list: its own names, then
/// `IMAGES_BROWSED`.
pub const BROWSED_OWN: [&str; 16] = [
    "zip", "7z", "rar", "tar", "tgz", "txz", "tbz", "tbz2", "tzst", "gz", "xz", "bz2", "zst", "lz4", "cab", "ncdu",
];

/// Whether Enter opens a file with this extension (no dot, any case) like a folder. Covers `BROWSED`.
pub fn is_browsed_ext(ext: &str) -> bool {
    let lower = ext.to_ascii_lowercase();
    BROWSED_OWN.contains(&lower.as_str()) || IMAGES_BROWSED.contains(&lower.as_str())
}

/// Formats that hold one compressed file (shown as a folder of one): Extract here writes the file itself
/// next to them.
pub const SINGLE_FILE: [&str; 5] = ["gz", "xz", "bz2", "zst", "lz4"];

/// Files that are archives by another name (a .docx, a .jar, an .msi...): Enter opens them in their app
/// (or runs the installer), and "Open as archive" looks inside. This is the TypeScript `ON_REQUEST` list:
/// its own names, then `IMAGES_ON_REQUEST`.
pub const ON_REQUEST_OWN: [&str; 38] = [
    "docx",
    "docm",
    "dotx",
    "dotm",
    "xlsx",
    "xlsm",
    "xltx",
    "xltm",
    "pptx",
    "pptm",
    "potx",
    "potm",
    "odt",
    "ods",
    "odp",
    "odg",
    "ott",
    "ots",
    "otp",
    "jar",
    "war",
    "ear",
    "apk",
    "aab",
    "nupkg",
    "vsix",
    "whl",
    "epub",
    "msix",
    "msixbundle",
    "appx",
    "appxbundle",
    "xpi",
    "kmz",
    "3mf",
    "ipa",
    "msi",
    "msm",
];

/// Whether a file with this extension (no dot, any case) is an archive by another name. Covers `ON_REQUEST`.
pub fn is_on_request_ext(ext: &str) -> bool {
    let lower = ext.to_ascii_lowercase();
    ON_REQUEST_OWN.contains(&lower.as_str()) || IMAGES_ON_REQUEST.contains(&lower.as_str())
}

/// Whether `path` goes into an archive, by its names: a dot, one of the extensions of `BROWSED` or
/// `ON_REQUEST` (any case), and then a separator or the end. This is the TypeScript `ARCHIVE_IN_PATH`
/// regular expression, written out: every `.` is tried, and what follows it up to the next separator
/// must be exactly one of the extensions (so ".tar.gz" counts through its ".gz", and ".zipper" does not).
pub fn archive_in_path(path: &str) -> bool {
    path.split(['\\', '/']).any(|segment| {
        segment.match_indices('.').any(|(at, _)| {
            segment
                .get(at + 1..)
                .is_some_and(|ext| is_browsed_ext(ext) || is_on_request_ext(ext))
        })
    })
}

/// Whether an error from an archive asks for its password: password-protected, or the one given doesn't
/// open it.
pub fn needs_password(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("password-protected") || lower.contains("password doesn't open")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_paths_by_their_names() {
        assert!(archive_in_path("C:\\a\\report.docx\\word"));
        assert!(archive_in_path("C:\\a\\b.7z"));
        assert!(!archive_in_path("C:\\a\\docx\\b"));
        assert!(!archive_in_path("C:\\a\\b.zipper"));
    }

    #[test]
    fn archive_paths_edge_cases() {
        assert!(archive_in_path("C:\\a\\B.ZIP/inside"));
        assert!(archive_in_path("C:\\a\\x.tar.gz"));
        assert!(archive_in_path("C:\\a\\setup.msm"));
        assert!(archive_in_path("C:\\a\\disk.ncdu\\x"));
        assert!(!archive_in_path(""));
        assert!(!archive_in_path("C:\\a\\zip"));
        assert!(!archive_in_path("C:\\a\\.zip.txt"));
        assert!(archive_in_path("C:\\D\u{f6}\\photos.zip\\2024\\old.7z\\summer"));
    }

    #[test]
    fn passwords_are_asked_for() {
        assert!(needs_password("This archive is Password-Protected"));
        assert!(needs_password("The password doesn't open it"));
        assert!(!needs_password("not found"));
    }

    #[test]
    fn extension_lists_cover_the_typescript_arrays() {
        assert!(is_browsed_ext("DMG") && is_browsed_ext("ncdu") && is_browsed_ext("tgz"));
        assert!(!is_browsed_ext("docx"));
        assert!(is_on_request_ext("iso") && is_on_request_ext("EXE") && is_on_request_ext("msi"));
        assert!(!is_on_request_ext("zip"));
    }
}
