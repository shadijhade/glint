//! Which items a file gets in the menus: the predicates the TypeScript took from each tool's dialog.
//!
//! Ported from `canConvert` (ConvertDialog.tsx), `isPdfFile` (PdfDialog.tsx), `isDataFile` (DataTools.tsx),
//! `canClean` / `canRedate` (MetadataDialog.tsx), `canEncrypt` / `isAgeFile` (VaultDialog.tsx), `canShrink` /
//! `isJxl` (ShrinkDialog.tsx), `canCompressInPlace`, `canConvertDoc` / `isTypst` (DocConvertDialog.tsx),
//! `isSong`, `canBackUp`, `isMediaFile` / `isVideoFile` (studio.ts) and `canWallpaper` / `canRotate`
//! (Explorer.tsx).
//!
//! TODO(coordinator): each of these belongs to the dialog that does the work; when that dialog is ported, its
//! module should export the predicate and this file should call it instead of keeping a copy. The extension
//! lists are the dialogs' own and must stay in step with them.

use glint_model::entries::{is_archived, is_closed, is_online_only, is_recycled, is_remote, FileEntry};

/// What rimage reads (convert.rs's `READS`).
const CONVERTS: &[&str] = &[
    "avif", "bmp", "ff", "hdr", "jpg", "jpeg", "jpe", "jfif", "jxl", "png", "ppm", "pgm", "pbm", "psd", "qoi", "svg",
    "tif", "tiff", "webp",
];
/// What ExifTool writes (metadata.rs), for "Remove personal info".
const EXIFTOOL: &[&str] = &[
    "jpg", "jpeg", "jpe", "png", "tif", "tiff", "webp", "heic", "heif", "avif", "jxl", "gif", "dng", "cr2", "cr3",
    "nef", "arw", "orf", "rw2", "raf", "psd", "mp4", "m4v", "mov", "3gp", "m4a", "pdf",
];
/// Office's files, cleaned in Glint itself.
const OFFICE: &[&str] = &["docx", "docm", "xlsx", "xlsm", "pptx", "pptm"];
/// Photos and videos whose date taken can be set.
const DATED: &[&str] = &[
    "jpg", "jpeg", "jpe", "png", "tif", "tiff", "webp", "heic", "heif", "avif", "jxl", "dng", "cr2", "cr3", "nef",
    "arw", "orf", "rw2", "raf", "mp4", "m4v", "mov", "3gp",
];
/// What DuckDB reads (data_lens.rs). `txt` is read but never offered a data menu.
const DUCK_READS: &[&str] = &["csv", "tsv", "tab", "psv", "json", "jsonl", "ndjson", "parquet"];
/// What pandoc reads (docconvert.rs's reader).
const DOC_READS: &[&str] = &[
    "docx", "odt", "md", "markdown", "mdown", "mkd", "html", "htm", "xhtml", "epub", "tex", "latex", "rtf", "rst",
    "org", "textile", "ipynb", "typ", "fb2",
];
/// What Make smaller does something with (shrink.rs's `kind_of`).
const SHRINKS: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "mts", "m2ts", "ts", "3gp", "mpg", "mpeg", "jpg", "jpeg",
    "jpe", "jfif", "bmp", "png", "pdf", "docx", "docm", "pptx", "pptm", "xlsx", "xlsm",
];
/// What lofty reads and writes details in (songtags.rs).
const SONGS: &[&str] = &[
    "mp3", "flac", "m4a", "m4b", "aac", "ogg", "oga", "opus", "wav", "aif", "aiff", "ape", "wv", "mpc", "spx",
];
/// What FFmpeg reads as video (studio.ts's `VIDEO_EXTS`).
const VIDEO: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "ts", "mts", "m2ts", "mpg", "mpeg", "3gp", "ogv", "vob",
    "asf", "f4v",
];
/// What FFmpeg reads as audio (studio.ts's `AUDIO_EXTS`).
const AUDIO: &[&str] = &[
    "mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "wma", "amr", "aiff", "aif", "ape", "mka", "ac3", "dts", "caf",
];
/// Pictures Windows can set as the desktop background (`WALLPAPER_EXTS`).
const WALLPAPER: &[&str] = &["jpg", "jpeg", "png", "bmp", "gif", "tif", "tiff", "jfif", "webp", "jxr"];
/// Pictures Glint turns in place, losslessly (`ROTATE_EXTS`).
const ROTATE: &[&str] = &["jpg", "jpeg", "png", "bmp", "tif", "tiff", "jfif"];

fn has(list: &[&str], ext: &str) -> bool {
    list.contains(&ext)
}

/// On disk, where Windows' own tools can reach it: not in an archive, a remote, the Recycle Bin or a closed
/// folder (Explorer.tsx's `onDisk`).
pub fn on_disk(e: &FileEntry) -> bool {
    !is_archived(e) && !is_remote(e) && !is_recycled(e) && !is_closed(e)
}

/// A path that starts with a drive letter and a backslash (`/^[a-z]:\\/i` in the TypeScript).
pub fn on_a_drive(path: &str) -> bool {
    let mut chars = path.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(letter), Some(':'), Some('\\')) if letter.is_ascii_alphabetic()
    )
}

/// There to be read: on disk, and not a cloud placeholder (the dialogs' own `onDisk`).
fn readable(e: &FileEntry) -> bool {
    !is_archived(e) && !is_closed(e) && !is_recycled(e) && !is_online_only(e)
}

/// The extension of a name, lower case, as the dialogs' `extOf` has it (a dot-file has one: ".gitignore" is "gitignore").
fn ext_of(name: &str) -> String {
    name.rfind('.')
        .and_then(|dot| name.get(dot + 1..))
        .unwrap_or("")
        .to_lowercase()
}

/// A picture Convert can read (`canConvert`).
pub fn can_convert(e: &FileEntry) -> bool {
    !e.dir && has(CONVERTS, &e.ext) && readable(e)
}

/// A PDF on disk, there to be read (`isPdfFile`).
pub fn is_pdf_file(e: &FileEntry) -> bool {
    !e.dir && e.ext == "pdf" && readable(e)
}

/// A data file DuckDB reads (`isDataFile`).
pub fn is_data_file(e: &FileEntry) -> bool {
    !e.dir && has(DUCK_READS, &e.ext) && readable(e)
}

/// A file whose personal info Glint can take out (`canClean`).
pub fn can_clean(e: &FileEntry) -> bool {
    !e.dir && readable(e) && (has(EXIFTOOL, &e.ext) || has(OFFICE, &e.ext))
}

/// A photo or video whose date taken can be set (`canRedate`).
pub fn can_redate(e: &FileEntry) -> bool {
    !e.dir && readable(e) && has(DATED, &e.ext)
}

/// What Encrypt takes: files and folders on disk (`canEncrypt`).
pub fn can_encrypt(e: &FileEntry) -> bool {
    readable(e) && !e.name.to_lowercase().ends_with(".age")
}

/// An age file, to decrypt (`isAgeFile`).
pub fn is_age_file(e: &FileEntry) -> bool {
    readable(e) && !e.dir && e.name.to_lowercase().ends_with(".age")
}

/// A file Make smaller works on (`canShrink`).
pub fn can_shrink(e: &FileEntry) -> bool {
    readable(e) && !e.dir && has(SHRINKS, &ext_of(&e.name))
}

/// A JPEG XL file, for Back to JPEG (`isJxl`).
pub fn is_jxl(e: &FileEntry) -> bool {
    readable(e) && !e.dir && ext_of(&e.name) == "jxl"
}

/// Files and folders Compress in place takes: on the PC itself (`canCompressInPlace`).
pub fn can_compress_in_place(e: &FileEntry) -> bool {
    readable(e) && !is_remote(e) && !e.path.starts_with("\\\\")
}

/// A document Convert document takes (`canConvertDoc`).
pub fn can_convert_doc(e: &FileEntry) -> bool {
    readable(e) && !e.dir && has(DOC_READS, &ext_of(&e.name))
}

/// A song whose details Glint edits (`isSong`).
pub fn is_song(e: &FileEntry) -> bool {
    !e.dir && readable(e) && has(SONGS, &ext_of(&e.name))
}

/// A folder Back up takes: one on this PC's drives (`canBackUp`).
pub fn can_back_up(e: &FileEntry) -> bool {
    let mut chars = e.path.chars();
    let drive_path = matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(letter), Some(':'), Some('\\')) if letter.is_ascii_alphabetic()
    );
    e.dir && readable(e) && drive_path
}

/// A video FFmpeg reads, on disk (`isVideoFile`).
pub fn is_video_file(e: &FileEntry) -> bool {
    !e.dir && readable(e) && has(VIDEO, &e.ext)
}

/// A video or a recording FFmpeg reads, on disk (`isMediaFile`).
pub fn is_media_file(e: &FileEntry) -> bool {
    is_video_file(e) || (!e.dir && readable(e) && has(AUDIO, &e.ext))
}

/// A picture Windows can set as the desktop background (`canWallpaper`).
pub fn can_wallpaper(e: &FileEntry) -> bool {
    !e.dir && on_disk(e) && has(WALLPAPER, &e.ext)
}

/// A picture Glint turns in place (`canRotate`).
pub fn can_rotate(e: &FileEntry) -> bool {
    !e.dir && on_disk(e) && has(ROTATE, &e.ext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::{file, folder};
    use glint_model::entries::attr;

    #[test]
    fn pictures_pdfs_and_data_are_told_by_extension_and_where_they_are() {
        assert!(can_convert(&file("C:\\x", "a.PNG", 1, 0)));
        assert!(!can_convert(&file("C:\\x", "a.txt", 1, 0)));
        assert!(!can_convert(&folder("C:\\x", "pics.png")));
        assert!(is_pdf_file(&file("C:\\x", "a.pdf", 1, 0)));
        assert!(is_data_file(&file("C:\\x", "a.csv", 1, 0)));
        assert!(
            !is_data_file(&file("C:\\x", "a.txt", 1, 0)),
            "txt is read by DuckDB but never offered"
        );
        let mut inside = file("C:\\x", "a.pdf", 1, 0);
        inside.attrs |= attr::IN_ARCHIVE;
        assert!(
            !is_pdf_file(&inside),
            "what is inside an archive is not there to be read"
        );
        let mut placeholder = file("C:\\x", "a.pdf", 1, 0);
        placeholder.attrs |= attr::OFFLINE;
        assert!(
            !is_pdf_file(&placeholder),
            "a cloud placeholder would be downloaded to be read"
        );
    }

    #[test]
    fn media_is_video_or_audio_and_wallpapers_are_pictures_on_disk() {
        assert!(is_video_file(&file("C:\\x", "a.mkv", 1, 0)));
        assert!(!is_video_file(&file("C:\\x", "a.mp3", 1, 0)));
        assert!(is_media_file(&file("C:\\x", "a.mp3", 1, 0)));
        assert!(can_wallpaper(&file("C:\\x", "a.jpg", 1, 0)));
        assert!(!can_wallpaper(&file("C:\\x", "a.svg", 1, 0)));
        assert!(can_rotate(&file("C:\\x", "a.png", 1, 0)));
        assert!(!can_rotate(&file("C:\\x", "a.gif", 1, 0)));
        let mut remote = file("C:\\x", "a.jpg", 1, 0);
        remote.attrs |= attr::REMOTE;
        assert!(!can_wallpaper(&remote) && !can_rotate(&remote));
    }

    #[test]
    fn encryption_skips_age_files_and_back_up_wants_a_drive_folder() {
        assert!(can_encrypt(&file("C:\\x", "a.txt", 1, 0)));
        assert!(can_encrypt(&folder("C:\\x", "docs")));
        assert!(!can_encrypt(&file("C:\\x", "a.txt.AGE", 1, 0)));
        assert!(is_age_file(&file("C:\\x", "a.txt.age", 1, 0)));
        assert!(!is_age_file(&folder("C:\\x", "x.age")));
        assert!(can_back_up(&folder("C:\\x", "docs")));
        assert!(!can_back_up(&file("C:\\x", "a.txt", 1, 0)));
        let mut share = folder("C:\\x", "docs");
        share.path = "\\\\server\\share\\docs".into();
        assert!(!can_back_up(&share));
        assert!(!can_compress_in_place(&share));
    }

    #[test]
    fn documents_songs_and_shrinkables_use_the_dialogs_lists() {
        assert!(can_convert_doc(&file("C:\\x", "a.docx", 1, 0)));
        assert!(!can_convert_doc(&file("C:\\x", "a.png", 1, 0)));
        assert!(is_song(&file("C:\\x", "a.flac", 1, 0)));
        assert!(can_shrink(&file("C:\\x", "a.mp4", 1, 0)));
        assert!(!can_shrink(&file("C:\\x", "a.txt", 1, 0)));
        assert!(is_jxl(&file("C:\\x", "a.jxl", 1, 0)));
        assert!(can_clean(&file("C:\\x", "a.docx", 1, 0)) && can_redate(&file("C:\\x", "a.heic", 1, 0)));
        assert!(!can_redate(&file("C:\\x", "a.pdf", 1, 0)));
    }
}
