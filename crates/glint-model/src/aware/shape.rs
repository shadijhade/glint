//! What a selection is, in one word, and what is worth finding out about one item.
//!
//! Pure: nothing here reads the disk or the clock.

use super::probed::Want;
use crate::entries::{is_archive_file, is_archived, is_closed, is_online_only, is_recycled, is_remote, FileEntry};
use crate::file_types::{type_group, TYPE_GROUPS};

/// What a menu's targets (or the selection) are: what the suggestions and the habits are kept under.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Shape {
    /// No targets: the folder's own menu.
    #[default]
    Nothing,
    Folder,
    Folders,
    Archive,
    Archives,
    DiskImage,
    Program,
    Installer,
    Script,
    Shortcut,
    Picture,
    Pictures,
    Video,
    Videos,
    Song,
    Songs,
    Pdf,
    Pdfs,
    Document,
    Documents,
    Data,
    Code,
    Font,
    /// One file of no kind above.
    File,
    /// Several files of different kinds.
    Files,
    /// Files and folders together.
    Mixed,
}

/// What one file is, for telling a selection's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum One {
    Archive,
    DiskImage,
    Program,
    Installer,
    Script,
    Shortcut,
    Pdf,
    Data,
    Picture,
    Video,
    Song,
    Document,
    Code,
    Font,
    Other,
}

/// The class of one file: the first row that matches wins (so a `.json` is Data, not Code).
fn class_of(e: &FileEntry) -> One {
    if is_archive_file(e) {
        return One::Archive;
    }
    match e.ext.as_str() {
        "iso" | "img" | "vhd" | "vhdx" => return One::DiskImage,
        "exe" | "com" | "scr" | "msc" => return One::Program,
        "msi" | "msix" | "msixbundle" | "appx" | "appxbundle" => return One::Installer,
        "bat" | "cmd" | "ps1" | "vbs" | "wsf" => return One::Script,
        "lnk" | "url" => return One::Shortcut,
        "pdf" => return One::Pdf,
        "csv" | "tsv" | "tab" | "psv" | "json" | "jsonl" | "ndjson" | "parquet" => return One::Data,
        _ => {}
    }
    let group = *type_group(e);
    if group == TYPE_GROUPS.images {
        One::Picture
    } else if group == TYPE_GROUPS.video {
        One::Video
    } else if group == TYPE_GROUPS.audio {
        One::Song
    } else if group == TYPE_GROUPS.documents {
        One::Document
    } else if group == TYPE_GROUPS.code {
        One::Code
    } else if group == TYPE_GROUPS.fonts {
        One::Font
    } else {
        One::Other
    }
}

impl Shape {
    /// What `targets` are.
    pub fn of(targets: &[FileEntry]) -> Shape {
        let Some(first) = targets.first() else {
            return Shape::Nothing;
        };
        let folders = targets.iter().filter(|e| e.dir).count();
        if folders == targets.len() {
            return if targets.len() == 1 { Shape::Folder } else { Shape::Folders };
        }
        if folders > 0 {
            return Shape::Mixed;
        }
        let class = class_of(first);
        if targets.iter().any(|e| class_of(e) != class) {
            return Shape::Files;
        }
        let many = targets.len() > 1;
        match class {
            One::Archive if many => Shape::Archives,
            One::Archive => Shape::Archive,
            One::Picture if many => Shape::Pictures,
            One::Picture => Shape::Picture,
            One::Video if many => Shape::Videos,
            One::Video => Shape::Video,
            One::Song if many => Shape::Songs,
            One::Song => Shape::Song,
            One::Pdf if many => Shape::Pdfs,
            One::Pdf => Shape::Pdf,
            One::Document if many => Shape::Documents,
            One::Document => Shape::Document,
            One::Data => Shape::Data,
            One::Code => Shape::Code,
            _ if many => Shape::Files,
            One::DiskImage => Shape::DiskImage,
            One::Program => Shape::Program,
            One::Installer => Shape::Installer,
            One::Script => Shape::Script,
            One::Shortcut => Shape::Shortcut,
            One::Font => Shape::Font,
            _ => Shape::File,
        }
    }

    /// A stable word for it ("picture", "pictures"): what habits are saved under. Never changes once shipped.
    pub fn key(self) -> &'static str {
        match self {
            Shape::Nothing => "nothing",
            Shape::Folder => "folder",
            Shape::Folders => "folders",
            Shape::Archive => "archive",
            Shape::Archives => "archives",
            Shape::DiskImage => "disk-image",
            Shape::Program => "program",
            Shape::Installer => "installer",
            Shape::Script => "script",
            Shape::Shortcut => "shortcut",
            Shape::Picture => "picture",
            Shape::Pictures => "pictures",
            Shape::Video => "video",
            Shape::Videos => "videos",
            Shape::Song => "song",
            Shape::Songs => "songs",
            Shape::Pdf => "pdf",
            Shape::Pdfs => "pdfs",
            Shape::Document => "document",
            Shape::Documents => "documents",
            Shape::Data => "data",
            Shape::Code => "code",
            Shape::Font => "font",
            Shape::File => "file",
            Shape::Files => "files",
            Shape::Mixed => "mixed",
        }
    }

    /// Whether it is several things.
    pub fn is_many(self) -> bool {
        matches!(
            self,
            Shape::Folders
                | Shape::Archives
                | Shape::Pictures
                | Shape::Videos
                | Shape::Songs
                | Shape::Pdfs
                | Shape::Documents
                | Shape::Files
                | Shape::Mixed
        )
    }
}

/// The one extension every target has, lower case; None when they differ, there is none, or a folder is among them.
pub fn shared_ext(targets: &[FileEntry]) -> Option<String> {
    let first = targets.first()?;
    if first.ext.is_empty() || targets.iter().any(|e| e.dir || e.ext != first.ext) {
        return None;
    }
    Some(first.ext.clone())
}

/// A path that starts with a drive letter, a colon and a backslash (`C:\`).
fn on_a_drive(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

/// What is worth asking about `entry`: nothing for what isn't a plain file or folder on one of this PC's drives.
pub fn want_for(entry: &FileEntry) -> Want {
    if is_archived(entry)
        || is_remote(entry)
        || is_recycled(entry)
        || is_closed(entry)
        || is_online_only(entry)
        || !on_a_drive(&entry.path)
    {
        return Want::default();
    }
    if entry.dir {
        return Want { inside: true, ..Want::default() };
    }
    let class = class_of(entry);
    let small_enough = entry.size <= 64 * 1024 * 1024;
    Want {
        origin: true,
        locked_by: true,
        opens_with: true,
        signer: matches!(class, One::Program | One::Installer),
        media: matches!(class, One::Picture | One::Video | One::Song),
        text: small_enough && (class == One::Code || (class == One::Data && entry.ext != "parquet")),
        pages: class == One::Pdf,
        target: entry.ext == "lnk",
        twins: (1024..=256 * 1024 * 1024).contains(&entry.size),
        ..Want::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entries::attr;

    fn file(name: &str, size: u64) -> FileEntry {
        let lower = name.to_lowercase();
        let ext = match lower.rfind('.') {
            Some(dot) if dot > 0 => lower[dot + 1..].to_string(),
            _ => String::new(),
        };
        FileEntry {
            name: name.to_string(),
            path: format!("C:\\x\\{name}"),
            dir: false,
            size,
            modified: 0,
            attrs: 0,
            lower,
            ext,
            origin: None,
        }
    }

    fn folder(name: &str) -> FileEntry {
        FileEntry { dir: true, attrs: attr::DIRECTORY, ext: String::new(), ..file(name, 0) }
    }

    fn shape_of_name(name: &str) -> Shape {
        Shape::of(&[file(name, 10)])
    }

    #[test]
    fn nothing_folders_and_a_mix_are_told_apart() {
        assert_eq!(Shape::of(&[]), Shape::Nothing);
        assert_eq!(Shape::of(&[folder("a")]), Shape::Folder);
        assert_eq!(Shape::of(&[folder("a"), folder("b")]), Shape::Folders);
        assert_eq!(Shape::of(&[folder("a"), file("b.txt", 1)]), Shape::Mixed);
    }

    #[test]
    fn each_class_gives_its_shape_for_one_file() {
        assert_eq!(shape_of_name("a.zip"), Shape::Archive);
        assert_eq!(shape_of_name("a.iso"), Shape::DiskImage);
        assert_eq!(shape_of_name("a.exe"), Shape::Program);
        assert_eq!(shape_of_name("a.msi"), Shape::Installer);
        assert_eq!(shape_of_name("a.ps1"), Shape::Script);
        assert_eq!(shape_of_name("a.lnk"), Shape::Shortcut);
        assert_eq!(shape_of_name("a.pdf"), Shape::Pdf);
        assert_eq!(shape_of_name("a.csv"), Shape::Data);
        assert_eq!(shape_of_name("a.png"), Shape::Picture);
        assert_eq!(shape_of_name("a.mp4"), Shape::Video);
        assert_eq!(shape_of_name("a.mp3"), Shape::Song);
        assert_eq!(shape_of_name("a.docx"), Shape::Document);
        assert_eq!(shape_of_name("a.rs"), Shape::Code);
        assert_eq!(shape_of_name("a.ttf"), Shape::Font);
        assert_eq!(shape_of_name("a.xyz"), Shape::File);
    }

    #[test]
    fn a_json_file_is_data_not_code() {
        assert_eq!(shape_of_name("a.json"), Shape::Data);
    }

    #[test]
    fn several_files_of_one_class_or_of_two() {
        assert_eq!(Shape::of(&[file("a.zip", 1), file("b.zip", 1)]), Shape::Archives);
        assert_eq!(Shape::of(&[file("a.png", 1), file("b.png", 1)]), Shape::Pictures);
        assert_eq!(Shape::of(&[file("a.exe", 1), file("b.exe", 1)]), Shape::Files);
        assert_eq!(Shape::of(&[file("a.png", 1), file("b.pdf", 1)]), Shape::Files);
        assert_eq!(Shape::of(&[file("a.rs", 1), file("b.rs", 1)]), Shape::Code);
    }

    #[test]
    fn every_key_is_unique_lower_case_and_has_no_space() {
        let all = [
            Shape::Nothing,
            Shape::Folder,
            Shape::Folders,
            Shape::Archive,
            Shape::Archives,
            Shape::DiskImage,
            Shape::Program,
            Shape::Installer,
            Shape::Script,
            Shape::Shortcut,
            Shape::Picture,
            Shape::Pictures,
            Shape::Video,
            Shape::Videos,
            Shape::Song,
            Shape::Songs,
            Shape::Pdf,
            Shape::Pdfs,
            Shape::Document,
            Shape::Documents,
            Shape::Data,
            Shape::Code,
            Shape::Font,
            Shape::File,
            Shape::Files,
            Shape::Mixed,
        ];
        let mut seen = std::collections::HashSet::new();
        for shape in all {
            let key = shape.key();
            assert!(seen.insert(key), "{key} is used twice");
            assert_eq!(key, key.to_lowercase());
            assert!(!key.contains(' '));
        }
        assert_eq!(seen.len(), 26);
    }

    #[test]
    fn a_shape_is_many_only_for_the_plural_ones() {
        assert!(Shape::Pictures.is_many() && Shape::Mixed.is_many() && Shape::Files.is_many());
        assert!(!Shape::Picture.is_many() && !Shape::Nothing.is_many() && !Shape::Code.is_many());
    }

    #[test]
    fn shared_ext_wants_every_target_to_have_the_same_one() {
        assert_eq!(shared_ext(&[file("a.png", 1), file("b.PNG", 1)]), Some("png".to_string()));
        assert_eq!(shared_ext(&[file("a.png", 1), file("b.jpg", 1)]), None);
        assert_eq!(shared_ext(&[file("a.png", 1), folder("d")]), None);
        assert_eq!(shared_ext(&[]), None);
        assert_eq!(shared_ext(&[file("README", 1)]), None);
    }

    #[test]
    fn want_for_asks_what_suits_the_kind() {
        let dir = want_for(&folder("d"));
        assert_eq!(dir, Want { inside: true, ..Want::default() });
        assert!(want_for(&file("a.exe", 5000)).signer);
        let png = want_for(&file("a.png", 5000));
        assert!(png.media && !png.text);
        assert!(!want_for(&file("a.rs", 100 * 1024 * 1024)).text);
        assert!(want_for(&file("a.rs", 5000)).text);
        assert!(!want_for(&file("a.parquet", 5000)).text);
        assert!(!want_for(&file("a.txt", 100)).twins);
        assert!(want_for(&file("a.txt", 5000)).twins);
        assert!(want_for(&file("a.lnk", 5000)).target);
        assert!(want_for(&file("a.pdf", 5000)).pages);
        let any = want_for(&file("a.txt", 5000));
        assert!(any.origin && any.locked_by && any.opens_with);
    }

    #[test]
    fn want_for_asks_nothing_for_what_is_not_a_plain_file_on_a_drive() {
        for bits in [attr::IN_ARCHIVE, attr::REMOTE, attr::RECYCLED, attr::CLOSED, attr::OFFLINE] {
            let mut e = file("a.txt", 5000);
            e.attrs = bits;
            assert_eq!(want_for(&e), Want::default(), "attrs {bits:#x}");
        }
        let mut unc = file("a.txt", 5000);
        unc.path = "\\\\server\\share\\a.txt".to_string();
        assert_eq!(want_for(&unc), Want::default());
    }
}
