//! Which previewer a file gets: the chain of `EntryPreview` in FilePreview.tsx, as a pure function.
//!
//! The TypeScript decided in the middle of a component; here the decision is a function of the entry and the
//! chosen view, so it is tested without a window. [`choose`] says what is drawn ([`Body`]); where the
//! TypeScript had a previewer that the port doesn't have yet, the `later` field names it (the `PORT ME` kinds)
//! and the body is the TypeScript's own fallback for it (Windows' previewer over the file's icon, or the
//! file as text). No kind is waiting on a previewer through it now (the MOBI family was the last), so `later` is always `None`.

use super::player::Kind as PlayerKind;
use crate::dialogs::data_tools::DUCK_READS;
use crate::t;
use glint_model::entries::{
    is_archive_file, is_archived, is_closed, is_list_only, is_locked, is_online_only, FileEntry,
};
use glint_model::file_types::{data_format, is_known_type, is_markdown, kind_of, PreviewKind};
use glint_model::small_kinds::{small_kind_of, SmallKind};
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

/// A file's other views (`View`): as it is, as written, what Git changed in it, SQL over it, each column of it
/// described, or its bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    /// The file as it reads (Rendered, Tree) or as it is.
    #[default]
    File,
    /// The file as written (Markdown's source, a tree's text, a workbook in Windows' previewer).
    Source,
    /// What changed since the last commit (Git).
    Changes,
    /// SQL over a table or a database, reading only.
    Query,
    /// Each column of a data file described from all its rows (DuckDB).
    Profile,
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
    /// A SQLite database: its tables and SQL (`preview::sqlite`).
    Sqlite,
    /// A workbook or Parquet file as a table (`preview::sheet`).
    Sheet,
    /// A Jupyter notebook as it reads (`preview::notebook`).
    Notebook,
    /// A program's sections, imports and exports (`preview::program`).
    Program,
    /// Contacts, calendars, mail, design files, Graphviz and Mermaid sources (`preview::small`).
    Small(SmallKind),
    /// What Git changed in the file since the last commit (`git::changes`).
    Changes,
    /// SQL over the file (`dialogs::data_tools::QueryView`).
    Query,
    /// Each column of the file described (`dialogs::data_tools::ProfileView`).
    Profile,
    /// The bytes (`preview::hex`).
    Hex,
    /// Video or sound played with Media Foundation (`preview::player`; a copy first for one in an archive).
    Player(PlayerKind),
    /// PDF pages drawn with Windows.Data.Pdf (`preview::pdf`; a copy first for one in an archive).
    Pdf,
    /// A book read page by page: EPUB, MOBI/AZW/AZW3, FB2, comics (`preview::book`, `preview::mobi`).
    Book,
    /// A Word document laid out without Office (`preview::docx`).
    Docx,
    /// A 3D model drawn and turned (`preview::model3d`).
    Model,
    /// An asciinema recording played (`preview::cast`).
    Cast,
    /// Windows' own previewer for the type, over `fallback`.
    Shell(Fallback),
    /// The kind's icon and nothing else.
    Icon,
}

/// A body, and the TypeScript previewer it stands in for until that part is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub body: Body,
    /// `PORT ME(<kind>)`: the TypeScript showed something better for this file; the body is its fallback. Nothing
    /// is waiting on a previewer now (the MOBI family was the last), so it is always `None`; a kind that is not
    /// ported yet sets it with `Choice { body, later: Some("kind") }`.
    pub later: Option<&'static str>,
}

impl Choice {
    fn of(body: Body) -> Self {
        Choice { body, later: None }
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

/// A Jupyter notebook, to read as it reads (a readable file on disk).
fn is_notebook(entry: &FileEntry) -> bool {
    readable(entry) && !is_archived(entry) && entry.ext == "ipynb"
}

/// A contact, calendar entry, message, design file or diagram source, read from disk.
fn small_of(entry: &FileEntry) -> Option<SmallKind> {
    if readable(entry) && !is_archived(entry) {
        small_kind_of(&entry.ext)
    } else {
        None
    }
}

/// A workbook or Parquet file, read from disk (`sheet`).
fn is_sheet(entry: &FileEntry) -> bool {
    readable(entry) && !is_archived(entry) && one_of(entry, &SHEETS)
}

/// A workbook, which Windows' previewer shows too (Excel's, with Office installed), but not Parquet (`workbook`).
fn is_workbook(entry: &FileEntry) -> bool {
    is_sheet(entry) && entry.ext != "parquet"
}

/// Tables and databases take SQL, from disk (`queryable`).
pub fn is_queryable(entry: &FileEntry) -> bool {
    readable(entry) && !is_archived(entry) && (is_sheet(entry) || one_of(entry, &TABULAR) || one_of(entry, &SQLITE))
}

/// Data files DuckDB describes column by column (`DUCK_READS`, except plain text).
pub fn is_profilable(entry: &FileEntry) -> bool {
    readable(entry) && !is_archived(entry) && entry.ext != "txt" && DUCK_READS.contains(&entry.ext.as_str())
}

/// The views the header offers (more than one shows the "Show" group): the file as it reads, as written, what
/// Git changed in it (`changed`: Git has a change for it, see `git::has_changes_view`), SQL over it, each column
/// of it, its bytes (`views`).
pub fn views(entry: &FileEntry, changed: bool) -> Vec<ViewChoice> {
    let mut views = Vec::new();
    let notebook = is_notebook(entry);
    let small = small_of(entry);
    let sheet = is_sheet(entry);
    // E-books and comics, 3D models and asciinema recordings have one view of their own, named for what it does.
    let book = readable(entry) && !is_archived(entry) && one_of(entry, &BOOKS);
    let model = readable(entry) && !is_archived(entry) && one_of(entry, &MODELS);
    let cast = readable(entry) && entry.ext == "cast";
    let choice = |view, label: SharedString, title: SharedString| ViewChoice { view, label, title };
    if notebook {
        views.push(choice(
            View::File,
            t!("Notebook"),
            t!("The notebook as it reads, its outputs included"),
        ));
        views.push(choice(View::Source, t!("Source"), t!("The notebook's JSON as written")));
    } else if book {
        views.push(choice(View::File, t!("Book"), t!("The book, to read")));
    } else if small.is_some() {
        views.push(choice(
            View::File,
            t!("Preview"),
            t!("What it holds, as Glint reads it"),
        ));
        // Outlook messages and design files are not text.
        if !matches!(small, Some(SmallKind::Outlook | SmallKind::Design)) {
            views.push(choice(View::Source, t!("Text"), t!("The file as written")));
        }
    } else if model {
        views.push(choice(View::File, t!("Model"), t!("The model, drawn")));
    } else if rendered_markdown(entry) {
        views.push(choice(View::File, t!("Rendered"), t!("The Markdown as it reads")));
        views.push(choice(View::Source, t!("Source"), t!("The Markdown as written")));
    } else if data_kind(entry) {
        let tree = data_format(entry).is_some();
        views.push(choice(
            View::File,
            if tree { t!("Tree") } else { t!("Table") },
            if tree {
                t!("Its values as a tree, with a jq filter")
            } else {
                t!("Its rows, read by Glint")
            },
        ));
        views.push(choice(View::Source, t!("Text"), t!("The file as written")));
    } else if sheet {
        views.push(choice(View::File, t!("Table"), t!("Its rows, read by Glint")));
        if is_workbook(entry) {
            views.push(choice(
                View::Source,
                t!("Previewer"),
                t!("As Windows' previewer for it shows it (Excel's, with Office installed)"),
            ));
        }
    } else if cast {
        views.push(choice(View::File, t!("Play"), t!("The recording, played")));
        views.push(choice(View::Source, t!("Text"), t!("The file as written")));
    } else {
        views.push(choice(View::File, t!("File"), t!("The file as it is")));
    }
    if changed && !entry.dir && !is_archived(entry) {
        views.push(choice(
            View::Changes,
            t!("Changes"),
            t!("What changed since the last commit (Git)"),
        ));
    }
    if is_queryable(entry) {
        views.push(choice(View::Query, t!("Query"), t!("SQL over it, reading only")));
    }
    if is_profilable(entry) {
        views.push(choice(
            View::Profile,
            t!("Profile"),
            t!("Each column described from all its rows (DuckDB)"),
        ));
    }
    if readable(entry) {
        views.push(choice(View::Hex, t!("Hex"), t!("Its bytes, in hexadecimal")));
    }
    views
}

/// The view shown: the one chosen when this file has it, else the file as it is (`view`).
pub fn effective_view(entry: &FileEntry, chosen: View, changed: bool) -> View {
    if views(entry, changed).iter().any(|v| v.view == chosen) {
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
    // The views that are the same tool whatever the file is; `effective_view` only passes the ones it has.
    match view {
        View::Changes if !entry.dir && !archived => return Choice::of(Body::Changes),
        View::Query if is_queryable(entry) => return Choice::of(Body::Query),
        View::Profile if is_profilable(entry) => return Choice::of(Body::Profile),
        _ => {}
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
    // Every e-book, the MOBI family (MOBI, AZW, AZW3) included: `preview::mobi` reads them for `preview::book`.
    if readable && !archived && one_of(entry, &BOOKS) && super::book::can_read(&entry.ext) {
        return Choice::of(Body::Book);
    }
    // Its Text view is the file as it is, which goes on down the chain.
    if let Some(kind) = small_of(entry).filter(|_| !source) {
        return Choice::of(Body::Small(kind));
    }
    if is_archive_file(entry) {
        return Choice::of(Body::Archive);
    }
    // In an archive, pictures, video, sound and PDFs are shown from a temp copy; text is read straight out of it.
    match kind_of(entry).preview {
        Some(PreviewKind::Image) => return Choice::of(Body::Picture),
        // The player, with Trim, the waveform and the transcript (VideoPreview, AudioPreview).
        Some(PreviewKind::Video) => return Choice::of(Body::Player(PlayerKind::Video)),
        Some(PreviewKind::Audio) => return Choice::of(Body::Player(PlayerKind::Audio)),
        // PDF pages drawn natively (PdfPreview).
        Some(PreviewKind::Pdf) => return Choice::of(Body::Pdf),
        _ => {}
    }
    if !archived && one_of(entry, &SQLITE) {
        return Choice::of(Body::Sqlite);
    }
    if !archived && one_of(entry, &TABULAR) && data_format(entry).is_none() {
        return Choice::of(if source { Body::Text } else { Body::Data });
    }
    if is_sheet(entry) {
        // A workbook's Previewer view is Windows' own, over its icon.
        return Choice::of(if source && is_workbook(entry) {
            Body::Shell(Fallback::Icon)
        } else {
            Body::Sheet
        });
    }
    if is_notebook(entry) {
        return Choice::of(if source { Body::Text } else { Body::Notebook });
    }
    if readable && !archived && one_of(entry, &MODELS) {
        return Choice::of(Body::Model);
    }
    if rendered_markdown(entry) {
        return Choice::of(if source { Body::Text } else { Body::Markdown });
    }
    if data_kind(entry) {
        return Choice::of(if source { Body::Text } else { Body::Data });
    }
    if readable && entry.ext == "cast" {
        return Choice::of(if source { Body::Text } else { Body::Cast });
    }
    if readable && !archived && one_of(entry, &DOCX_EXTS) && entry.size <= DOCX_MAX {
        return Choice::of(Body::Docx);
    }
    // Unknown extensions get a text preview attempt; the probe spots binary files.
    if kind_of(entry).preview == Some(PreviewKind::Text) || !is_known_type(entry) {
        return Choice::of(Body::Text);
    }
    if !archived && one_of(entry, &PE) {
        return Choice::of(Body::Program);
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
        choose(&entry, effective_view(&entry, View::File, false)).body
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
        assert_eq!(body("tool.exe"), Body::Program);
        assert_eq!(body("lib.dll"), Body::Program);
        assert_eq!(body("app.db"), Body::Sqlite);
        assert_eq!(body("book.xlsx"), Body::Sheet);
        assert_eq!(body("rows.parquet"), Body::Sheet);
        assert_eq!(body("lab.ipynb"), Body::Notebook);
        assert_eq!(body("card.vcf"), Body::Small(SmallKind::Contact));
        assert_eq!(body("invite.ics"), Body::Small(SmallKind::Calendar));
        assert_eq!(body("mail.eml"), Body::Small(SmallKind::Email));
        assert_eq!(body("mail.msg"), Body::Small(SmallKind::Outlook));
        assert_eq!(body("art.kra"), Body::Small(SmallKind::Design));
        assert_eq!(body("graph.dot"), Body::Small(SmallKind::Graphviz));
        assert_eq!(body("flow.mmd"), Body::Small(SmallKind::Mermaid));
        assert_eq!(body("letter.doc"), Body::Shell(Fallback::Icon));
        assert_eq!(body("font.ttf"), Body::Shell(Fallback::Icon));
    }

    #[test]
    fn no_kind_is_waiting_on_a_previewer_and_the_mobi_family_has_the_book_view() {
        let later = |name: &str| choose(&file("C:\\x", name, 10, 1), View::File);
        // The MOBI family has the book view too, not Windows' previewer over the icon.
        for book in ["book.mobi", "book.azw", "book.azw3"] {
            assert_eq!(later(book).body, Body::Book, "{book}");
            assert_eq!(later(book).later, None, "{book}");
        }
        assert_eq!(later("book.fb2").body, Body::Book);
        assert_eq!(later("book.fbz").body, Body::Book);
        // What has its previewer now says nothing is missing.
        assert_eq!(later("movie.mp4").body, Body::Player(PlayerKind::Video));
        assert_eq!(later("song.mp3").body, Body::Player(PlayerKind::Audio));
        assert_eq!(later("paper.pdf").body, Body::Pdf);
        assert_eq!(later("book.epub").body, Body::Book);
        assert_eq!(later("comic.cbz").body, Body::Book);
        assert_eq!(later("model.stl").body, Body::Model);
        assert_eq!(later("letter.docx").body, Body::Docx);
        for ready in [
            "notes.md",
            "sheet.xlsx",
            "lab.ipynb",
            "card.vcf",
            "tool.exe",
            "app.sqlite",
            "movie.mp4",
            "song.mp3",
            "paper.pdf",
            "book.epub",
            "model.stl",
            "letter.docx",
        ] {
            assert_eq!(later(ready).later, None, "{ready}");
        }
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
        // Video, sound and PDFs are played or drawn from a copy the view extracts itself.
        assert_eq!(
            choose(&inside("a.mp4"), View::File).body,
            Body::Player(PlayerKind::Video)
        );
        assert_eq!(choose(&inside("a.pdf"), View::File).body, Body::Pdf);
        // A table in an archive is shown as the text it is.
        assert_eq!(choose(&inside("a.csv"), View::File).body, Body::Text);
    }

    #[test]
    fn markdown_and_data_have_source_and_hex_views() {
        let md = file("C:\\x", "a.md", 10, 1);
        let labels: Vec<_> = views(&md, false)
            .iter()
            .map(|v| (v.view, v.label.to_string()))
            .collect();
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
        assert_eq!(views(&json, false)[0].label, "Tree");
        assert_eq!(choose(&json, View::Source).body, Body::Text);
        let csv = file("C:\\x", "a.csv", 10, 1);
        assert_eq!(views(&csv, false)[0].label, "Table");
        assert_eq!(choose(&csv, View::Source).body, Body::Text);
        // A plain file has File and Hex: a chosen Source view isn't one of them, so it is shown as it is.
        let txt = file("C:\\x", "a.txt", 10, 1);
        assert_eq!(views(&txt, false).len(), 2);
        assert_eq!(effective_view(&txt, View::Source, false), View::File);
        assert_eq!(effective_view(&txt, View::Hex, false), View::Hex);
        // A folder has no bytes to show.
        assert_eq!(effective_view(&folder("C:\\x", "d"), View::Hex, false), View::File);
    }

    #[test]
    fn a_big_json_file_is_not_a_tree() {
        let mut big = file("C:\\x", "huge.json", 10, 1);
        big.size = 65 * 1024 * 1024;
        assert_eq!(choose(&big, View::File).body, Body::Text);
        // No tree, but DuckDB reads a JSON file of any size: File, Profile and Hex.
        let order: Vec<_> = views(&big, false).into_iter().map(|v| v.view).collect();
        assert_eq!(order, [View::File, View::Profile, View::Hex]);
    }

    fn labels(entry: &FileEntry, changed: bool) -> Vec<(View, String)> {
        views(entry, changed)
            .iter()
            .map(|v| (v.view, v.label.to_string()))
            .collect()
    }

    fn pairs(list: &[(View, &str)]) -> Vec<(View, String)> {
        list.iter().map(|(view, label)| (*view, label.to_string())).collect()
    }

    #[test]
    fn a_workbook_has_a_table_and_windows_previewer_and_parquet_only_the_table() {
        let book = file("C:\\x", "a.xlsx", 10, 1);
        assert_eq!(
            labels(&book, false),
            pairs(&[
                (View::File, "Table"),
                (View::Source, "Previewer"),
                (View::Query, "Query"),
                (View::Hex, "Hex")
            ])
        );
        assert_eq!(choose(&book, View::File).body, Body::Sheet);
        assert_eq!(choose(&book, View::Source).body, Body::Shell(Fallback::Icon));
        let parquet = file("C:\\x", "a.parquet", 10, 1);
        assert_eq!(
            labels(&parquet, false),
            pairs(&[
                (View::File, "Table"),
                (View::Query, "Query"),
                (View::Profile, "Profile"),
                (View::Hex, "Hex")
            ])
        );
        assert_eq!(choose(&parquet, View::Source).body, Body::Sheet);
    }

    #[test]
    fn a_notebook_has_its_reading_and_its_json() {
        let book = file("C:\\x", "a.ipynb", 10, 1);
        assert_eq!(
            labels(&book, false),
            pairs(&[(View::File, "Notebook"), (View::Source, "Source"), (View::Hex, "Hex")])
        );
        assert_eq!(views(&book, false)[1].title, "The notebook's JSON as written");
        assert_eq!(choose(&book, View::File).body, Body::Notebook);
        assert_eq!(choose(&book, View::Source).body, Body::Text);
    }

    #[test]
    fn books_models_and_recordings_have_a_view_named_for_what_it_does() {
        for (name, label, title) in [
            ("a.epub", "Book", "The book, to read"),
            ("a.cbz", "Book", "The book, to read"),
            ("m.stl", "Model", "The model, drawn"),
            ("m.glb", "Model", "The model, drawn"),
            ("c.cast", "Play", "The recording, played"),
        ] {
            let entry = file("C:\\x", name, 10, 1);
            let first = &views(&entry, false)[0];
            assert_eq!(first.view, View::File, "{name}");
            assert_eq!(first.label, label, "{name}");
            assert_eq!(first.title, title, "{name}");
        }
        // A recording is also text; a book and a model are read as they are.
        let cast = file("C:\\x", "c.cast", 10, 1);
        assert_eq!(
            labels(&cast, false),
            pairs(&[(View::File, "Play"), (View::Source, "Text"), (View::Hex, "Hex")])
        );
        assert_eq!(choose(&cast, View::File).body, Body::Cast);
        assert_eq!(choose(&cast, View::Source).body, Body::Text);
        assert_eq!(
            labels(&file("C:\\x", "a.epub", 10, 1), false),
            pairs(&[(View::File, "Book"), (View::Hex, "Hex")])
        );
        // Inside an archive they aren't read from disk: the plain File view.
        let inside = marked("a.epub", attr::IN_ARCHIVE);
        assert_eq!(views(&inside, false)[0].label, "File");
    }

    #[test]
    fn small_files_have_a_preview_and_a_text_view_unless_they_are_not_text() {
        let contact = file("C:\\x", "a.vcf", 10, 1);
        assert_eq!(
            labels(&contact, false),
            pairs(&[(View::File, "Preview"), (View::Source, "Text"), (View::Hex, "Hex")])
        );
        assert_eq!(choose(&contact, View::Source).body, Body::Text);
        for binary in ["a.msg", "a.kra"] {
            let entry = file("C:\\x", binary, 10, 1);
            assert_eq!(
                labels(&entry, false),
                pairs(&[(View::File, "Preview"), (View::Hex, "Hex")]),
                "{binary}"
            );
        }
        // In an archive they are not read from disk, so not as a contact.
        let inside = marked("a.vcf", attr::IN_ARCHIVE);
        assert_ne!(choose(&inside, View::File).body, Body::Small(SmallKind::Contact));
    }

    #[test]
    fn tables_and_databases_take_sql_and_data_files_a_profile() {
        let has = |name: &str, view: View| views(&file("C:\\x", name, 10, 1), false).iter().any(|v| v.view == view);
        for name in ["a.csv", "a.tsv", "a.jsonl", "a.xlsx", "a.parquet", "a.sqlite", "a.db"] {
            assert!(has(name, View::Query), "{name} takes SQL");
        }
        for name in ["a.json", "a.txt", "a.md", "a.exe"] {
            assert!(!has(name, View::Query), "{name} takes no SQL");
        }
        for name in ["a.csv", "a.tsv", "a.json", "a.jsonl", "a.parquet", "a.psv", "a.tab"] {
            assert!(has(name, View::Profile), "{name} is profiled");
        }
        for name in ["a.txt", "a.xlsx", "a.sqlite", "a.md"] {
            assert!(!has(name, View::Profile), "{name} is not profiled");
        }
        let csv = file("C:\\x", "a.csv", 10, 1);
        assert_eq!(choose(&csv, View::Query).body, Body::Query);
        assert_eq!(choose(&csv, View::Profile).body, Body::Profile);
        let db = file("C:\\x", "a.sqlite", 10, 1);
        assert_eq!(choose(&db, View::Query).body, Body::Query);
        // A view the file has not got is not drawn even when asked for.
        assert_eq!(choose(&db, View::Profile).body, Body::Sqlite);
        // Nothing inside an archive is read from disk.
        let inside = marked("a.csv", attr::IN_ARCHIVE);
        assert!(!views(&inside, false).iter().any(|v| v.view == View::Query));
    }

    #[test]
    fn a_changed_file_offers_what_changed_and_a_folder_or_archived_file_never_does() {
        let txt = file("C:\\x", "a.txt", 10, 1);
        assert_eq!(
            labels(&txt, true),
            pairs(&[(View::File, "File"), (View::Changes, "Changes"), (View::Hex, "Hex")])
        );
        assert_eq!(views(&txt, true)[1].title, "What changed since the last commit (Git)");
        assert_eq!(effective_view(&txt, View::Changes, true), View::Changes);
        assert_eq!(effective_view(&txt, View::Changes, false), View::File);
        assert_eq!(choose(&txt, View::Changes).body, Body::Changes);
        // Changes come before Query and Profile.
        let csv = file("C:\\x", "a.csv", 10, 1);
        let order: Vec<_> = labels(&csv, true).into_iter().map(|(view, _)| view).collect();
        assert_eq!(
            order,
            [
                View::File,
                View::Source,
                View::Changes,
                View::Query,
                View::Profile,
                View::Hex
            ]
        );
        assert!(!views(&folder("C:\\x", "d"), true)
            .iter()
            .any(|v| v.view == View::Changes));
        let inside = marked("a.txt", attr::IN_ARCHIVE);
        assert!(!views(&inside, true).iter().any(|v| v.view == View::Changes));
        assert_ne!(choose(&inside, View::Changes).body, Body::Changes);
    }
}
