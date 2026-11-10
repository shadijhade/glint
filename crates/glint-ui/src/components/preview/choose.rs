//! Which previewer a file gets: the chain of `EntryPreview` in FilePreview.tsx, as a pure function.
//!
//! The TypeScript decided in the middle of a component; here the decision is a function of the entry and the
//! chosen view, so it is tested without a window. [`choose`] says what is drawn ([`Body`]); where the
//! TypeScript had a previewer that the port doesn't have yet, the `later` field names it (the `PORT ME` kinds)
//! and the body is the TypeScript's own fallback for it (Windows' previewer over the file's icon, or the
//! file as text).

use crate::t;
use glint_model::entries::{
    is_archive_file, is_archived, is_closed, is_list_only, is_locked, is_online_only, FileEntry,
};
use glint_model::file_types::{data_format, is_known_type, is_markdown, kind_of, PreviewKind};
use gpui_kit::SharedString;

/// Word documents laid out without Office, up to a size (`DOCX_EXTS`, `DOCX_MAX`).
const DOCX_EXTS: [&str; 4] = ["docx", "docm", "dotx", "dotm"];
/// E-books the reader opens (`BOOKS`).
const BOOKS: [&str; 7] = ["epub", "mobi", "azw3", "azw", "fb2", "fbz", "cbz"];
/// 3D formats (`MODEL_EXTS`).
const MODELS: [&str; 6] = ["stl", "obj", "glb", "gltf", "ply", "3mf"];
/// Databases (`SQLITE_EXTS`).
const SQLITE: [&str; 6] = ["sqlite", "sqlite3", "db", "db3", "s3db", "sl3"];
/// Plain tables (`TABULAR_EXTS`).
const TABULAR: [&str; 4] = ["csv", "tsv", "jsonl", "ndjson"];
/// Workbooks and Parquet (`SHEET_EXTS`).
const SHEETS: [&str; 11] = [
    "xlsx", "xlsm", "xltx", "xltm", "xlam", "xlsb", "xls", "xla", "xlt", "ods", "parquet",
];
/// Programs and libraries (`PE_EXTS`).
const PE: [&str; 11] = [
    "exe", "dll", "sys", "ocx", "drv", "cpl", "scr", "efi", "pyd", "node", "mui",
];
/// Word documents bigger than this aren't laid out (`DOCX_MAX`), JSON bigger than `DATA_MAX` isn't a tree.
const DOCX_MAX: u64 = 40 * 1024 * 1024;
const DATA_MAX: u64 = 64 * 1024 * 1024;

/// A file's other views (`View`): as it is, as written, or its bytes. (Git's Changes, SQL's Query and
/// DuckDB's Profile come with their parts.)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    /// The file as it reads (Rendered, Tree) or as it is.
    #[default]
    File,
    /// The file as written (Markdown's source, a tree's text).
    Source,
    /// Its bytes, in hexadecimal.
    Hex,
}

/// One button of the header's "Show" group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewChoice {
    pub view: View,
    pub label: SharedString,
    pub title: SharedString,
}

/// The Windows previewer's stand-in while it loads, and when there is none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// The kind's icon, large.
    Icon,
    /// The file's bytes.
    Hex,
}

/// What is drawn for a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Body {
    /// A folder: what is directly in it.
    Folder,
    /// A OneDrive-style placeholder: opening it downloads it.
    OnlineOnly,
    /// In a folder Windows keeps closed to this account.
    Closed,
    /// In an archive and can't be read: password-protected, or why not.
    Locked,
    /// Listed in an ncdu export: a name, a size and a date.
    ListOnly,
    /// An archive: what is in it, and Open like a folder.
    Archive,
    /// A picture, drawn here (from a temp copy when it is in an archive).
    Picture,
    /// Text and code: the start of the file, highlighted (`preview::text`). A file whose start isn't text goes
    /// to Windows' previewer, or its bytes.
    Text,
    /// Markdown, rendered (`preview::markdown`).
    Markdown,
    /// JSON, YAML and TOML as a tree, CSV and TSV as a table (`preview::data`).
    Data,
    /// The bytes (`preview::hex`).
    Hex,
    /// Windows' own previewer for the type, over `fallback`.
    Shell(Fallback),
    /// The kind's icon and nothing else.
    Icon,
}

/// A body, and the TypeScript previewer it stands in for until that part is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub body: Body,
    /// `PORT ME(<kind>)`: the TypeScript showed something better for this file; the body is its fallback.
    pub later: Option<&'static str>,
}

impl Choice {
    fn of(body: Body) -> Self {
        Choice { body, later: None }
    }

    fn later(body: Body, kind: &'static str) -> Self {
        Choice {
            body,
            later: Some(kind),
        }
    }
}

/// Whether the file's contents can be looked at (`readable`).
pub fn readable(entry: &FileEntry) -> bool {
    !entry.dir && !is_online_only(entry) && !is_closed(entry) && !is_locked(entry) && !is_list_only(entry)
}

fn one_of(entry: &FileEntry, list: &[&str]) -> bool {
    !entry.dir && list.contains(&entry.ext.as_str())
}

/// Markdown that has a rendered view (a readable file).
fn rendered_markdown(entry: &FileEntry) -> bool {
    readable(entry) && is_markdown(entry)
}

/// JSON, YAML, TOML, CSV and TSV as a tree or a table (up to the 64 MB the tree reads).
fn data_kind(entry: &FileEntry) -> bool {
    readable(entry)
        && ((data_format(entry).is_some() && entry.size <= DATA_MAX)
            || (!is_archived(entry) && one_of(entry, &TABULAR)))
}

/// The views the header offers (more than one shows the "Show" group): the file as it reads, as written, its
/// bytes. Only views that can be drawn today are offered.
pub fn views(entry: &FileEntry) -> Vec<ViewChoice> {
    let mut views = Vec::new();
    if rendered_markdown(entry) {
        views.push(ViewChoice {
            view: View::File,
            label: t!("Rendered"),
            title: t!("The Markdown as it reads"),
        });
        views.push(ViewChoice {
            view: View::Source,
            label: t!("Source"),
            title: t!("The Markdown as written"),
        });
    } else if data_kind(entry) {
        views.push(ViewChoice {
            view: View::File,
            label: if data_format(entry).is_some() {
                t!("Tree")
            } else {
                t!("Table")
            },
            title: if data_format(entry).is_some() {
                t!("Its values as a tree, with a jq filter")
            } else {
                t!("Its rows, read by Glint")
            },
        });
        views.push(ViewChoice {
            view: View::Source,
            label: t!("Text"),
            title: t!("The file as written"),
        });
    } else {
        views.push(ViewChoice {
            view: View::File,
            label: t!("File"),
            title: t!("The file as it is"),
        });
    }
    if readable(entry) {
        views.push(ViewChoice {
            view: View::Hex,
            label: t!("Hex"),
            title: t!("Its bytes, in hexadecimal"),
        });
    }
    views
}

/// The view shown: the one chosen when this file has it, else the file as it is (`view`).
pub fn effective_view(entry: &FileEntry, chosen: View) -> View {
    if views(entry).iter().any(|v| v.view == chosen) {
        chosen
    } else {
        View::File
    }
}

/// What to draw for `entry` in `view` (use [`effective_view`] for the view): the chain in `EntryPreview`.
pub fn choose(entry: &FileEntry, view: View) -> Choice {
    let archived = is_archived(entry);
    let readable = readable(entry);
    let source = view == View::Source;
    if view == View::Hex && readable {
        return Choice::of(Body::Hex);
    }
    if entry.dir {
        return Choice::of(Body::Folder);
    }
    if is_online_only(entry) {
        return Choice::of(Body::OnlineOnly);
    }
    if is_closed(entry) {
        return Choice::of(Body::Closed);
    }
    if is_locked(entry) {
        return Choice::of(Body::Locked);
    }
    if is_list_only(entry) {
        return Choice::of(Body::Icon);
    }
    if readable && !archived && one_of(entry, &BOOKS) {
        // PORT ME(book): the reader (BookView.tsx: EPUB, MOBI, FB2, comics).
        return Choice::later(Body::Shell(Fallback::Icon), "book");
    }
    if readable && !archived && glint_model::small_kinds::small_kind_of(&entry.ext).is_some() {
        // PORT ME(small previews): SmallPreviews.tsx (contacts, calendars, mail, design files, Graphviz, Mermaid).
        return Choice::later(
            Body::Shell(Fallback::Icon),
            "small previews: contacts, calendars, mail, design files, Graphviz and Mermaid",
        );
    }
    if is_archive_file(entry) {
        return Choice::of(Body::Archive);
    }
    // In an archive, pictures, video, sound and PDFs are shown from a temp copy; text is read straight out of it.
    match kind_of(entry).preview {
        Some(PreviewKind::Image) => return Choice::of(Body::Picture),
        // PORT ME(video): the player with Trim and the transcript (VideoPreview).
        Some(PreviewKind::Video) => {
            return Choice::later(
                if archived {
                    Body::Icon
                } else {
                    Body::Shell(Fallback::Icon)
                },
                "video",
            )
        }
        // PORT ME(audio): the player with cover, waveform, Trim and transcript (AudioPreview).
        Some(PreviewKind::Audio) => {
            return Choice::later(
                if archived {
                    Body::Icon
                } else {
                    Body::Shell(Fallback::Icon)
                },
                "audio",
            )
        }
        // PORT ME(pdf pages): PDF pages drawn natively (PdfPreview).
        Some(PreviewKind::Pdf) => {
            return Choice::later(
                if archived {
                    Body::Icon
                } else {
                    Body::Shell(Fallback::Icon)
                },
                "pdf pages",
            )
        }
        _ => {}
    }
    if !archived && one_of(entry, &SQLITE) {
        // PORT ME(sqlite): tables and SQL (SqlitePreview).
        return Choice::later(Body::Shell(Fallback::Icon), "sqlite");
    }
    if !archived && one_of(entry, &TABULAR) && data_format(entry).is_none() {
        return Choice::of(if source { Body::Text } else { Body::Data });
    }
    if readable && !archived && one_of(entry, &SHEETS) {
        // PORT ME(sheet): workbooks and Parquet as tables (SheetPreview).
        return Choice::later(Body::Shell(Fallback::Icon), "sheet");
    }
    if readable && !archived && entry.ext == "ipynb" {
        // PORT ME(notebook): the notebook as it reads (NotebookView).
        return Choice::later(Body::Text, "notebook");
    }
    if readable && !archived && entry.ext == "typ" {
        // PORT ME(typst): the document typeset (TypstView).
        return Choice::later(Body::Text, "typst");
    }
    if readable && !archived && one_of(entry, &MODELS) {
        // PORT ME(3d): the model drawn (ModelView).
        return Choice::later(Body::Shell(Fallback::Icon), "3d");
    }
    if rendered_markdown(entry) {
        return Choice::of(if source { Body::Text } else { Body::Markdown });
    }
    if data_kind(entry) {
        return Choice::of(if source { Body::Text } else { Body::Data });
    }
    if readable && entry.ext == "cast" {
        // PORT ME(cast): the recording played (CastPreview).
        return Choice::later(Body::Text, "cast");
    }
    if readable && !archived && one_of(entry, &DOCX_EXTS) && entry.size <= DOCX_MAX {
        // PORT ME(docx): Word documents laid out without Office (DocxView).
        return Choice::later(Body::Shell(Fallback::Icon), "docx");
    }
    // Unknown extensions get a text preview attempt; the probe spots binary files.
    if kind_of(entry).preview == Some(PreviewKind::Text) || !is_known_type(entry) {
        return Choice::of(Body::Text);
    }
    if !archived && one_of(entry, &PE) {
        // PORT ME(program headers): a program's sections, imports and exports (BinaryPreview).
        return Choice::later(Body::Hex, "program headers");
    }
    // Word, Excel, PowerPoint, fonts…: Windows' own previewer, where there's one.
    if !archived {
        return Choice::of(Body::Shell(Fallback::Icon));
    }
    Choice::of(Body::Icon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::{file, folder};
    use glint_model::entries::attr;

    fn body(name: &str) -> Body {
        let entry = file("C:\\x", name, 10, 1);
        choose(&entry, effective_view(&entry, View::File)).body
    }

    fn marked(name: &str, attrs: u32) -> FileEntry {
        let mut entry = file("C:\\x", name, 10, 1);
        entry.attrs = attrs;
        entry
    }

    #[test]
    fn each_kind_of_file_gets_its_previewer() {
        assert_eq!(body("photo.png"), Body::Picture);
        assert_eq!(body("a.jpg"), Body::Picture);
        assert_eq!(body("notes.md"), Body::Markdown);
        assert_eq!(body("data.json"), Body::Data);
        assert_eq!(body("settings.toml"), Body::Data);
        assert_eq!(body("table.csv"), Body::Data);
        assert_eq!(body("table.tsv"), Body::Data);
        assert_eq!(body("main.rs"), Body::Text);
        assert_eq!(body("README"), Body::Text);
        // An extension nobody knows is tried as text; the probe tells binary from text.
        assert_eq!(body("thing.zzqq"), Body::Text);
        assert_eq!(body("pack.zip"), Body::Archive);
        assert_eq!(body("tool.exe"), Body::Hex);
        assert_eq!(body("letter.doc"), Body::Shell(Fallback::Icon));
        assert_eq!(body("font.ttf"), Body::Shell(Fallback::Icon));
    }

    #[test]
    fn kinds_the_port_has_not_reached_say_so_and_fall_back_as_the_typescript_did() {
        let later = |name: &str| choose(&file("C:\\x", name, 10, 1), View::File);
        assert_eq!(later("movie.mp4").later, Some("video"));
        assert_eq!(later("movie.mp4").body, Body::Shell(Fallback::Icon));
        assert_eq!(later("song.mp3").later, Some("audio"));
        assert_eq!(later("paper.pdf").later, Some("pdf pages"));
        assert_eq!(later("book.epub").later, Some("book"));
        assert_eq!(later("model.stl").later, Some("3d"));
        assert_eq!(later("sheet.xlsx").later, Some("sheet"));
        assert_eq!(later("letter.docx").later, Some("docx"));
        assert_eq!(
            later("lab.ipynb"),
            Choice {
                body: Body::Text,
                later: Some("notebook")
            }
        );
        assert!(later("card.vcf").later.is_some());
        assert_eq!(later("notes.md").later, None);
    }

    #[test]
    fn the_states_come_before_the_kind() {
        assert_eq!(choose(&folder("C:\\x", "sub"), View::File).body, Body::Folder);
        assert_eq!(
            choose(&marked("a.png", attr::OFFLINE), View::File).body,
            Body::OnlineOnly
        );
        assert_eq!(choose(&marked("a.png", attr::CLOSED), View::File).body, Body::Closed);
        assert_eq!(choose(&marked("a.png", attr::ENCRYPTED), View::File).body, Body::Locked);
        assert_eq!(
            choose(&marked("a.png", attr::IN_ARCHIVE | attr::LIST_ONLY), View::File).body,
            Body::Icon
        );
    }

    #[test]
    fn files_in_archives_are_shown_from_a_copy_or_as_text_and_never_by_windows() {
        let inside = |name: &str| marked(name, attr::IN_ARCHIVE);
        assert_eq!(choose(&inside("a.png"), View::File).body, Body::Picture);
        assert_eq!(choose(&inside("a.txt"), View::File).body, Body::Text);
        assert_eq!(choose(&inside("a.md"), View::File).body, Body::Markdown);
        assert_eq!(choose(&inside("a.docx"), View::File).body, Body::Icon);
        assert_eq!(choose(&inside("a.mp4"), View::File).body, Body::Icon);
        // A table in an archive is shown as the text it is.
        assert_eq!(choose(&inside("a.csv"), View::File).body, Body::Text);
    }

    #[test]
    fn markdown_and_data_have_source_and_hex_views() {
        let md = file("C:\\x", "a.md", 10, 1);
        let labels: Vec<_> = views(&md).iter().map(|v| (v.view, v.label.to_string())).collect();
        assert_eq!(
            labels,
            [
                (View::File, "Rendered".to_string()),
                (View::Source, "Source".to_string()),
                (View::Hex, "Hex".to_string())
            ]
        );
        assert_eq!(choose(&md, View::Source).body, Body::Text);
        assert_eq!(choose(&md, View::Hex).body, Body::Hex);
        let json = file("C:\\x", "a.json", 10, 1);
        assert_eq!(views(&json)[0].label, "Tree");
        assert_eq!(choose(&json, View::Source).body, Body::Text);
        let csv = file("C:\\x", "a.csv", 10, 1);
        assert_eq!(views(&csv)[0].label, "Table");
        assert_eq!(choose(&csv, View::Source).body, Body::Text);
        // A plain file has File and Hex: a chosen Source view isn't one of them, so it is shown as it is.
        let txt = file("C:\\x", "a.txt", 10, 1);
        assert_eq!(views(&txt).len(), 2);
        assert_eq!(effective_view(&txt, View::Source), View::File);
        assert_eq!(effective_view(&txt, View::Hex), View::Hex);
        // A folder has no bytes to show.
        assert_eq!(effective_view(&folder("C:\\x", "d"), View::Hex), View::File);
    }

    #[test]
    fn a_big_json_file_is_not_a_tree() {
        let mut big = file("C:\\x", "huge.json", 10, 1);
        big.size = 65 * 1024 * 1024;
        assert_eq!(choose(&big, View::File).body, Body::Text);
        assert_eq!(views(&big).len(), 2);
    }
}
