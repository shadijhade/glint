//! Ported from src/lib/fileTypes.ts.
//!
//! What kind a file is: which preview opens, how the Type column reads, which icon it gets and which color
//! group it falls in on the disk-usage map.
//!
//! NOTE(port): the TypeScript `Kind` also carried a lucide icon component and Tailwind tone classes. `Kind::icon`
//! is an `Icon` name the UI layer maps to its own icon (`kit::kind_icon`), and `Kind::tone` keeps the classes as
//! text for the UI's theme to map (`theme::kind_tone`).

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::entries::FileEntry;

/// A color on the disk-usage map, shared by related kinds of files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeGroup {
    pub name: &'static str,
    pub hue: u32,
    /// Saturation, in percent.
    pub sat: u32,
}

/// Every group, by the TypeScript's key (`TYPE_GROUPS.images`...).
#[derive(Debug)]
pub struct TypeGroups {
    pub images: TypeGroup,
    pub video: TypeGroup,
    pub audio: TypeGroup,
    pub archives: TypeGroup,
    pub documents: TypeGroup,
    pub code: TypeGroup,
    pub programs: TypeGroup,
    pub system: TypeGroup,
    pub disks: TypeGroup,
    pub data: TypeGroup,
    pub fonts: TypeGroup,
    pub other: TypeGroup,
}

const fn group(name: &'static str, hue: u32, sat: u32) -> TypeGroup {
    TypeGroup { name, hue, sat }
}

/// The groups of the disk-usage map.
pub static TYPE_GROUPS: TypeGroups = TypeGroups {
    images: group(crate::english!("Images"), 199, 78),
    video: group(crate::english!("Videos"), 262, 66),
    audio: group(crate::english!("Audio"), 322, 66),
    archives: group(crate::english!("Archives"), 36, 86),
    documents: group(crate::english!("Documents"), 4, 70),
    code: group(crate::english!("Code and text"), 150, 50),
    programs: group(crate::english!("Programs"), 188, 60),
    system: group(crate::english!("System files"), 216, 46),
    disks: group(crate::english!("Disk images"), 240, 56),
    data: group(crate::english!("Databases"), 172, 56),
    fonts: group(crate::english!("Fonts"), 345, 62),
    other: group(crate::english!("Other files"), 30, 8),
};

/// The icon a kind of file gets. The names are lucide's, which the old UI used; the UI layer maps each to its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    AppWindow,
    BookOpen,
    Database,
    Disc,
    File,
    FileArchive,
    FileAudio,
    FileCode,
    FileCog,
    FileImage,
    FileJson,
    FileLock,
    FileSpreadsheet,
    FileText,
    FileType,
    FileVideo,
    Folder,
    Link,
    Presentation,
    SquareTerminal,
}

/// What the preview pane can show for a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewKind {
    Image,
    Video,
    Audio,
    Text,
    Pdf,
}

/// JSON, YAML or TOML.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataFormat {
    Json,
    Yaml,
    Toml,
}

/// One kind of file: its icon, label, preview and color group.
#[derive(Debug)]
pub struct Kind {
    pub icon: Icon,
    /// Type column text; `%` is the uppercased extension.
    pub label: &'static str,
    pub preview: Option<PreviewKind>,
    pub group: &'static TypeGroup,
    /// The icon's color as the TypeScript's Tailwind classes ("text-sky-500", "text-blue-600
    /// dark:text-blue-400", the folder's fill): the UI maps them to its theme (`theme::kind_tone`).
    pub tone: &'static str,
}

const fn kind(
    icon: Icon,
    label: &'static str,
    preview: Option<PreviewKind>,
    group: &'static TypeGroup,
    tone: &'static str,
) -> Kind {
    Kind {
        icon,
        label,
        preview,
        group,
        tone,
    }
}

const G: &TypeGroups = &TYPE_GROUPS;

const CODE_EXTS: &str = "js mjs cjs ts mts cts tsx jsx rs go py rb java kt kts c h cpp hpp cc cs php swift scala sh bash zsh ps1 psm1 psd1 bat cmd lua sql vue svelte html htm css scss sass less xml xaml csproj sln props targets yml yaml toml ini cfg conf gradle dart r pl ex exs erl hs fs fsx clj zig nim dockerfile makefile cmake";

/// Space-separated extensions and their kind; later rows win for an extension listed twice (as the Map did).
// For translators: the Type column's words; % is the file's extension in capitals ("% image" reads "PNG image").
static KINDS: &[(&str, Kind)] = &[
    (
        "png jpg jpeg gif webp bmp ico svg avif",
        kind(
            Icon::FileImage,
            crate::english!("% image"),
            Some(PreviewKind::Image),
            &G.images,
            "text-sky-500",
        ),
    ),
    // Pictures the webview can't show: Windows' codecs and previewer do (JPEG XL's with its extension).
    (
        "tif tiff heic heif jxl psd raw cr2 nef arw dng",
        kind(
            Icon::FileImage,
            crate::english!("% image"),
            None,
            &G.images,
            "text-sky-500",
        ),
    ),
    (
        "mp4 webm m4v mov ogv",
        kind(
            Icon::FileVideo,
            crate::english!("% video"),
            Some(PreviewKind::Video),
            &G.video,
            "text-violet-500",
        ),
    ),
    (
        "mkv avi wmv flv mpg mpeg 3gp m2ts",
        kind(
            Icon::FileVideo,
            crate::english!("% video"),
            None,
            &G.video,
            "text-violet-500",
        ),
    ),
    (
        "mp3 wav ogg oga opus m4a aac flac",
        kind(
            Icon::FileAudio,
            crate::english!("% audio"),
            Some(PreviewKind::Audio),
            &G.audio,
            "text-pink-500",
        ),
    ),
    (
        "wma aiff aif mid midi",
        kind(
            Icon::FileAudio,
            crate::english!("% audio"),
            None,
            &G.audio,
            "text-pink-500",
        ),
    ),
    (
        "zip rar 7z tar gz tgz txz tbz tbz2 tzst bz2 xz zst lz4 cab",
        kind(
            Icon::FileArchive,
            crate::english!("% archive"),
            None,
            &G.archives,
            "text-amber-600 dark:text-amber-500",
        ),
    ),
    (
        "pdf",
        kind(
            Icon::FileText,
            crate::english!("PDF document"),
            Some(PreviewKind::Pdf),
            &G.documents,
            "text-red-500",
        ),
    ),
    (
        "doc docx odt rtf pages",
        kind(
            Icon::FileText,
            crate::english!("Word document"),
            None,
            &G.documents,
            "text-blue-600 dark:text-blue-400",
        ),
    ),
    (
        "xls xlsx xlsm xlsb xltx ods numbers",
        kind(
            Icon::FileSpreadsheet,
            crate::english!("Spreadsheet"),
            None,
            &G.documents,
            "text-green-600 dark:text-green-500",
        ),
    ),
    (
        "csv tsv",
        kind(
            Icon::FileSpreadsheet,
            crate::english!("% file"),
            Some(PreviewKind::Text),
            &G.documents,
            "text-green-600 dark:text-green-500",
        ),
    ),
    (
        "ppt pptx odp key",
        kind(
            Icon::Presentation,
            crate::english!("Presentation"),
            None,
            &G.documents,
            "text-orange-500",
        ),
    ),
    (
        "json jsonc json5 geojson webmanifest har",
        kind(
            Icon::FileJson,
            crate::english!("JSON file"),
            Some(PreviewKind::Text),
            &G.code,
            "text-yellow-600 dark:text-yellow-500",
        ),
    ),
    (
        CODE_EXTS,
        kind(
            Icon::FileCode,
            crate::english!("% source file"),
            Some(PreviewKind::Text),
            &G.code,
            "text-emerald-500",
        ),
    ),
    (
        "txt md markdown mdown mkd mkdn log nfo text env gitignore gitattributes editorconfig properties lock",
        kind(
            Icon::FileText,
            crate::english!("% file"),
            Some(PreviewKind::Text),
            &G.code,
            "text-muted-foreground",
        ),
    ),
    (
        "exe msi com scr appx msix appinstaller",
        kind(
            Icon::AppWindow,
            crate::english!("Application"),
            None,
            &G.programs,
            "text-slate-500 dark:text-slate-400",
        ),
    ),
    (
        "dll sys so dylib ocx drv cpl mui",
        kind(
            Icon::FileCog,
            crate::english!("% system file"),
            None,
            &G.system,
            "text-slate-500 dark:text-slate-400",
        ),
    ),
    (
        "ttf otf woff woff2 eot fon",
        kind(
            Icon::FileType,
            crate::english!("% font"),
            None,
            &G.fonts,
            "text-rose-500",
        ),
    ),
    (
        "iso img vhd vhdx vmdk dmg",
        kind(
            Icon::Disc,
            crate::english!("Disk image"),
            None,
            &G.disks,
            "text-indigo-500",
        ),
    ),
    (
        "cast",
        kind(
            Icon::SquareTerminal,
            crate::english!("Terminal recording"),
            Some(PreviewKind::Text),
            &G.video,
            "text-emerald-600 dark:text-emerald-400",
        ),
    ),
    (
        "lnk url",
        kind(
            Icon::Link,
            crate::english!("Shortcut"),
            None,
            &G.other,
            "text-cyan-600 dark:text-cyan-400",
        ),
    ),
    (
        "db sqlite sqlite3 mdb accdb",
        kind(
            Icon::Database,
            crate::english!("Database"),
            None,
            &G.data,
            "text-teal-600 dark:text-teal-400",
        ),
    ),
    (
        "parquet",
        kind(
            Icon::Database,
            crate::english!("Parquet file"),
            None,
            &G.data,
            "text-teal-600 dark:text-teal-400",
        ),
    ),
    (
        "epub mobi azw3 cbz cbr",
        kind(
            Icon::BookOpen,
            crate::english!("E-book"),
            None,
            &G.documents,
            "text-amber-700 dark:text-amber-400",
        ),
    ),
    (
        "pem key crt cer pfx p12 gpg asc",
        kind(
            Icon::FileLock,
            crate::english!("Certificate or key"),
            None,
            &G.other,
            "text-yellow-700 dark:text-yellow-500",
        ),
    ),
];

static BY_EXT: LazyLock<HashMap<&'static str, &'static Kind>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    for (exts, kind) in KINDS {
        for ext in exts.split(' ') {
            map.insert(ext, kind);
        }
    }
    map
});

static FOLDER: Kind = kind(
    Icon::Folder,
    crate::english!("File folder"),
    None,
    &G.other,
    "fill-amber-400/80 text-amber-500 dark:fill-amber-400/70",
);
static GENERIC: Kind = kind(
    Icon::File,
    crate::english!("% file"),
    None,
    &G.other,
    "text-muted-foreground",
);

/// Names without an extension that are still text, e.g. Dockerfile or LICENSE.
const TEXT_NAMES: [&str; 8] = [
    "dockerfile",
    "makefile",
    "license",
    "readme",
    "changelog",
    "authors",
    "codeowners",
    ".gitignore",
];

/// The folder's kind (its icon, label and tone), for drawing a folder that isn't an entry (a place, a tab).
pub fn folder_kind() -> &'static Kind {
    &FOLDER
}

/// What kind of file an entry is.
pub fn kind_of(entry: &FileEntry) -> &'static Kind {
    if entry.dir {
        return &FOLDER;
    }
    if let Some(kind) = BY_EXT.get(entry.ext()) {
        return kind;
    }
    if TEXT_NAMES.contains(&entry.lower.as_str()) || entry.lower == ".env" {
        // "txt" is always in the table.
        return BY_EXT.get("txt").copied().unwrap_or(&GENERIC);
    }
    &GENERIC
}

/// The Type column's text, in the interface's language: the kind's label with `%` replaced by the uppercased
/// extension ("File" without one).
pub fn type_label(entry: &FileEntry) -> String {
    let mut out = String::new();
    push_type_label(entry, &mut out);
    out
}

/// [`type_label`] written onto the end of `out`, for the caller that is building a longer line (the list's
/// description of a row, made for every visible row on every frame): no string of its own, and the uppercased
/// extension is written a letter at a time.
pub fn push_type_label(entry: &FileEntry, out: &mut String) {
    let language = crate::i18n::current();
    // A catalog keeps the `%` ("% 图像"): build.rs leaves out a translation that drops it.
    let label = crate::i18n::translate(language, kind_of(entry).label);
    if !label.contains('%') {
        out.push_str(label);
    } else if entry.ext().is_empty() {
        out.push_str(crate::i18n::translate(language, crate::english!("File")));
    } else {
        for (n, part) in label.split('%').enumerate() {
            if n > 0 {
                out.extend(entry.ext().chars().flat_map(char::to_uppercase));
            }
            out.push_str(part);
        }
    }
}

/// What the preview pane can show for a file.
#[cfg(test)]
pub fn preview_kind(entry: &FileEntry) -> Option<PreviewKind> {
    kind_of(entry).preview
}

/// Markdown, which the preview shows as it reads (or as written).
pub fn is_markdown(entry: &FileEntry) -> bool {
    !entry.dir && matches!(entry.ext(), "md" | "markdown" | "mdown" | "mkd" | "mkdn")
}

/// JSON, YAML and TOML files, which the preview shows as a tree to run jq filters over.
pub fn data_format(entry: &FileEntry) -> Option<DataFormat> {
    if entry.dir {
        return None;
    }
    match entry.ext() {
        "json" | "geojson" | "webmanifest" | "har" => Some(DataFormat::Json),
        "yaml" | "yml" => Some(DataFormat::Yaml),
        "toml" => Some(DataFormat::Toml),
        _ => None,
    }
}

/// False for extensions we know nothing about (the preview tries those as text).
pub fn is_known_type(entry: &FileEntry) -> bool {
    !std::ptr::eq(kind_of(entry), &GENERIC)
}

/// A file's color group on the disk-usage map.
pub fn type_group(entry: &FileEntry) -> &'static TypeGroup {
    kind_of(entry).group
}

/// Pictures and videos: what a thumbnail in the list's icon column tells apart better than an icon.
pub fn is_picture_or_video(entry: &FileEntry) -> bool {
    if entry.dir {
        return false;
    }
    let group = kind_of(entry).group;
    *group == G.images || *group == G.video
}

/// A video (by its type), whose tile can be scrubbed.
pub fn is_video_entry(entry: &FileEntry) -> bool {
    !entry.dir && *kind_of(entry).group == G.video
}

/// Filter string (`ext:png;jpg;...`) for all extensions belonging to `group`.
pub fn group_filter(group: &TypeGroup) -> String {
    if *group == G.other {
        return String::new();
    }
    let mut exts: Vec<&str> = Vec::new();
    for (list, kind) in KINDS {
        if *kind.group == *group {
            exts.extend(list.split(' '));
        }
    }
    if exts.is_empty() {
        String::new()
    } else {
        format!("ext:{}", exts.join(";"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entries::{with_details, RawEntry};

    fn entry(name: &str) -> FileEntry {
        let (name, dir) = name.strip_suffix('/').map_or((name, false), |n| (n, true));
        with_details(
            RawEntry {
                name: name.into(),
                dir,
                ..RawEntry::default()
            },
            "C:\\x",
        )
    }

    // explorer.check.ts: type groups
    #[test]
    fn group_filter_lists_the_groups_extensions() {
        assert!(group_filter(type_group(&entry("a.png"))).starts_with("ext:png;jpg;"));
        assert_eq!(group_filter(type_group(&entry("unknown.xyz"))), "");
    }

    #[test]
    fn group_filter_covers_every_row_of_a_group() {
        let docs = group_filter(&TYPE_GROUPS.documents);
        assert!(docs.starts_with("ext:pdf;doc;docx;"));
        assert!(docs.contains(";epub;"));
        assert_eq!(group_filter(&TYPE_GROUPS.other), "");
        let video = group_filter(&TYPE_GROUPS.video);
        assert!(video.contains("mp4;webm") && video.contains(";mkv;") && video.ends_with(";cast"));
    }

    #[test]
    fn type_group_by_extension_and_folder() {
        assert_eq!(type_group(&entry("a.PNG")).name, "Images");
        assert_eq!(type_group(&entry("a.mkv")).name, "Videos");
        assert_eq!(type_group(&entry("a.rs")).name, "Code and text");
        assert_eq!(type_group(&entry("a.dll")).name, "System files");
        assert_eq!(type_group(&entry("dir.png/")).name, "Other files");
        assert_eq!(type_group(&entry("a.xyz")).name, "Other files");
    }

    #[test]
    fn key_extension_belongs_to_the_certificate_row() {
        // "key" is in both the presentation and the certificate rows; the later row wins.
        assert_eq!(type_label(&entry("a.key")), "Certificate or key");
        assert_eq!(type_group(&entry("a.key")).name, "Other files");
    }

    #[test]
    fn kind_of_picks_folder_extension_text_name_or_generic() {
        assert_eq!(kind_of(&entry("src/")).icon, Icon::Folder);
        assert_eq!(kind_of(&entry("a.zip")).icon, Icon::FileArchive);
        assert_eq!(kind_of(&entry("LICENSE")).icon, Icon::FileText);
        assert_eq!(kind_of(&entry("LICENSE")).preview, Some(PreviewKind::Text));
        assert_eq!(kind_of(&entry("Dockerfile")).preview, Some(PreviewKind::Text));
        assert_eq!(kind_of(&entry(".gitignore")).preview, Some(PreviewKind::Text));
        assert_eq!(kind_of(&entry("a.xyz")).icon, Icon::File);
        assert_eq!(kind_of(&entry("noext")).icon, Icon::File);
    }

    #[test]
    fn kind_of_treats_a_dot_env_file_as_text() {
        // ".env" has no extension of its own, so it takes the text kind by name, as ".gitignore" does.
        assert_eq!(kind_of(&entry(".env")).icon, Icon::FileText);
        assert_eq!(kind_of(&entry(".env")).preview, Some(PreviewKind::Text));
        assert_eq!(kind_of(&entry(".ENV")).preview, Some(PreviewKind::Text));
        assert_eq!(kind_of(&entry("prod.env")).preview, Some(PreviewKind::Text));
    }

    #[test]
    fn type_label_fills_in_the_extension() {
        assert_eq!(type_label(&entry("a.png")), "PNG image");
        assert_eq!(type_label(&entry("a.rs")), "RS source file");
        assert_eq!(type_label(&entry("a.csv")), "CSV file");
        assert_eq!(type_label(&entry("a.dll")), "DLL system file");
        assert_eq!(type_label(&entry("a.pdf")), "PDF document");
        assert_eq!(type_label(&entry("a.docx")), "Word document");
        assert_eq!(type_label(&entry("folder/")), "File folder");
        assert_eq!(type_label(&entry("a.xyz")), "XYZ file");
        assert_eq!(type_label(&entry("noext")), "File");
        // A text name with no extension has the "txt" label, which has no extension to show.
        assert_eq!(type_label(&entry("LICENSE")), "File");
        assert_eq!(type_label(&entry("a.JPG")), "JPG image");
    }

    #[test]
    fn a_label_is_pushed_onto_a_line_the_same_as_it_is_made() {
        for name in [
            "a.png",
            "a.pdf",
            "folder/",
            "noext",
            "LICENSE",
            "a.JPG",
            "a.xyz",
            "a.straße",
        ] {
            let e = entry(name);
            let mut line = String::from("Photo, ");
            push_type_label(&e, &mut line);
            assert_eq!(line, format!("Photo, {}", type_label(&e)), "{name}");
        }
    }

    #[test]
    fn preview_kind_by_type() {
        assert_eq!(preview_kind(&entry("a.png")), Some(PreviewKind::Image));
        assert_eq!(preview_kind(&entry("a.tif")), None);
        assert_eq!(preview_kind(&entry("a.mp4")), Some(PreviewKind::Video));
        assert_eq!(preview_kind(&entry("a.mkv")), None);
        assert_eq!(preview_kind(&entry("a.mp3")), Some(PreviewKind::Audio));
        assert_eq!(preview_kind(&entry("a.wma")), None);
        assert_eq!(preview_kind(&entry("a.pdf")), Some(PreviewKind::Pdf));
        assert_eq!(preview_kind(&entry("a.csv")), Some(PreviewKind::Text));
        assert_eq!(preview_kind(&entry("a.json")), Some(PreviewKind::Text));
        assert_eq!(preview_kind(&entry("a.rs")), Some(PreviewKind::Text));
        assert_eq!(preview_kind(&entry("a.exe")), None);
        assert_eq!(preview_kind(&entry("dir/")), None);
    }

    #[test]
    fn is_markdown_checks_extension_and_folders() {
        for name in ["a.md", "a.MD", "a.markdown", "a.mdown", "a.mkd", "a.mkdn"] {
            assert!(is_markdown(&entry(name)), "{name}");
        }
        assert!(!is_markdown(&entry("a.txt")));
        assert!(!is_markdown(&entry("notes.md/")));
    }

    #[test]
    fn data_format_by_extension() {
        assert_eq!(data_format(&entry("a.json")), Some(DataFormat::Json));
        assert_eq!(data_format(&entry("a.geojson")), Some(DataFormat::Json));
        assert_eq!(data_format(&entry("a.webmanifest")), Some(DataFormat::Json));
        assert_eq!(data_format(&entry("a.har")), Some(DataFormat::Json));
        assert_eq!(data_format(&entry("a.yaml")), Some(DataFormat::Yaml));
        assert_eq!(data_format(&entry("a.YML")), Some(DataFormat::Yaml));
        assert_eq!(data_format(&entry("a.toml")), Some(DataFormat::Toml));
        // jsonc and json5 get a text preview but no tree, as in the TypeScript.
        assert_eq!(data_format(&entry("a.jsonc")), None);
        assert_eq!(data_format(&entry("a.txt")), None);
        assert_eq!(data_format(&entry("x.json/")), None);
    }

    #[test]
    fn is_known_type_is_false_only_for_generic_files() {
        assert!(is_known_type(&entry("a.png")));
        assert!(is_known_type(&entry("dir/")));
        assert!(is_known_type(&entry("LICENSE")));
        assert!(!is_known_type(&entry("a.xyz")));
        assert!(!is_known_type(&entry("noext")));
    }

    #[test]
    fn picture_and_video_helpers() {
        assert!(is_picture_or_video(&entry("a.png")));
        assert!(is_picture_or_video(&entry("a.heic")));
        assert!(is_picture_or_video(&entry("a.mp4")));
        assert!(is_picture_or_video(&entry("a.cast")));
        assert!(!is_picture_or_video(&entry("a.mp3")));
        assert!(!is_picture_or_video(&entry("pics.png/")));
        assert!(is_video_entry(&entry("a.mkv")));
        assert!(!is_video_entry(&entry("a.png")));
        assert!(!is_video_entry(&entry("clips.mp4/")));
    }

    #[test]
    fn type_groups_keep_their_colors() {
        assert_eq!(TYPE_GROUPS.images, group("Images", 199, 78));
        assert_eq!(TYPE_GROUPS.other.sat, 8);
        assert_eq!(TYPE_GROUPS.fonts.hue, 345);
    }
}
