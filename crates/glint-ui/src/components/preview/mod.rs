//! The preview pane and Quick Look: which previewer a file gets, the pane beside the files, the Quick Look
//! overlay (Space), pictures, and Windows' own preview handlers (FilePreview.tsx, Preview.tsx,
//! PreviewState.tsx, QuickLook.tsx, ShellPreview.tsx; SmallPreviews.tsx's pictures).
//!
//! | TypeScript                                   | Here                                                       |
//! |----------------------------------------------|------------------------------------------------------------|
//! | `PreviewPane` (Explorer.tsx), `FilePreview`  | [`PreviewPane`] (`pane.rs`): the selection as an item, a deck, or the folder's card |
//! | `EntryPreview`                               | [`EntryView`] (`frame.rs`): header, previewer, details; [`choose`] says which previewer |
//! | `usePreviewView`                             | [`chosen`]: the view kept from file to file                |
//! | `ImagePreview`, `ArchivedMedia`              | [`picture::PictureView`] (`picture.rs`)                    |
//! | `ShellPreview`                               | [`shell::ShellHost`] over `platform::preview_host`         |
//! | `TextPreview`/`MarkdownPreview` around the text | [`probe::TextProbe`] around the preview-text agent's views |
//! | `FolderPreview`, `ArchivePreview`, `LockedPreview` | `cards.rs`                                           |
//! | `Centered`, `BigIcon`, `Waiting`             | `states.rs`                                                |
//! | `QuickLook`                                  | [`quick_look`]: [`quick_look::toggle`]                     |
//!
//! # Wiring (for the workspace)
//!
//! - **The pane.** Make one with `cx.new(|cx| PreviewPane::new(window, cx))` and place it at the end edge of the
//!   files while [`should_show`] is true (the setting is on, and the tab isn't a bare This PC). Drop it while it
//!   isn't: Windows' previewers are windows of their own, kept over the box the pane draws, and let go only when
//!   the pane's view is dropped. Its width is
//!   [`pane_width`] (the saved `preview_width`, else 28% up to 28 rem, kept inside its limits); the handle that
//!   resizes it saves `max(MIN_WIDTH, width)` to `settings.preview_width`, and a double-click resets it to `None`.
//!   The `TogglePreview` action calls [`toggle_pane`].
//! - **Quick Look.** The `QuickLook` action (Space in the list) calls [`quick_look::toggle`]. The keys inside it
//!   are the actions already bound in context "QuickLook" and are handled here. After explorer events the
//!   workspace calls [`quick_look::sync`] so a Recycle Bin item the model set `quick_look` for gets looked at.
//! - **Menus.** `MenuEvent::Shown` calls [`cover`], `MenuEvent::Closed` calls [`uncover`]: Windows' previewers
//!   draw above GPUI, so they are hidden while a menu is open. Dialogs (the palette, questions, Quick Look
//!   itself) are noticed on their own.
//! - **Start-up.** Nothing: [`sources::sources`] makes glint-core's on first use, and the preview host's own
//!   `init` is called by `platform::preview_host::show`.
//!
//! # Not ported yet
//!
//! Nothing is marked `PORT ME(<kind>)` in the previewers now: a file the TypeScript had a previewer for either has
//! its native one or gets what the TypeScript showed when its own previewer failed (Windows' previewer over the
//! file's icon, or the file as text). (Wave 9 brought tables, notebooks, program headers, contacts, calendars, mail,
//! the Changes, Query and Profile views; wave 10 the player, PDF pages, books, Word documents and 3D models; wave 11
//! Mermaid and Graphviz diagrams, math, Typst documents and cast recordings; wave 15 the MOBI, AZW and AZW3 books,
//! read by [`mobi`], and HTML mail laid out as a page in [`small`], with the pictures from the internet off until
//! asked and a switch to the plain version.)

/// JSON, YAML and TOML as a tree; CSV and TSV as a table (DataPreview.tsx, DataTree.tsx).
pub mod data;
/// Bytes as hex (HexView.tsx, BinaryPreview.tsx).
pub mod hex;
/// Markdown, rendered (MarkdownView.tsx without Mermaid and math, which come later).
pub mod markdown;
/// Text and code, highlighted (FilePreview.tsx's text view, highlighter.ts).
pub mod text;

/// Books: EPUB, MOBI/AZW, FB2 and comics, read page by page (BookView.tsx).
pub mod book;
pub mod cards;
/// asciinema recordings played (CastPreview.tsx).
pub mod cast;
pub mod choose;
pub mod details;
/// Mermaid and Graphviz diagrams, in Markdown and as files (merman, layout-rs).
pub mod diagram;
/// Word documents laid out without Office (DocxView.tsx).
pub mod docx;
pub mod frame;
/// Math in Markdown, LaTeX syntax (RaTeX).
pub mod math;
/// MOBI, AZW and AZW3 books: PalmDOC, HUFF/CDIC, the MOBI and EXTH headers and KF8, read for the book view (foliate's mobi.js).
pub mod mobi;
/// 3D models: STL, OBJ, PLY, glTF/GLB and 3MF, drawn and turned (ModelView.tsx).
pub mod model3d;
/// Jupyter notebooks as they read (NotebookView.tsx).
pub mod notebook;
pub mod pane;
/// PDF pages drawn natively with Windows.Data.Pdf (PdfPreview in FilePreview.tsx).
pub mod pdf;
pub mod picture;
/// Video and audio played with Media Foundation, with Trim, the waveform and scrubbing (VideoPreview, AudioPreview, Waveform.tsx, VideoScrub.tsx).
pub mod player;
pub mod probe;
/// A program's sections, imports and exports (BinaryPreview.tsx).
pub mod program;
pub mod quick_look;
/// The rich text the book and Word previews share: blocks and runs, right to left (previews-native.md sections 2-3).
pub mod rich;
/// Workbooks and Parquet as tables (SheetPreview in FilePreview.tsx).
pub mod sheet;
pub mod shell;
/// Who signed a program, and whether Windows trusts it (SignatureChip.tsx).
pub mod signature;
/// Contacts, calendars, mail (HTML laid out as a page, pictures from the internet only when asked), design files (SmallPreviews.tsx).
pub mod small;
pub mod sources;
/// SQLite databases: their tables and SQL (SqlitePreview in FilePreview.tsx).
pub mod sqlite;
pub mod states;
/// SVG drawn as a GPUI image, cached by its source and size: the pipeline diagrams and math share (previews-native.md sections 4-7).
pub mod svg_image;
#[cfg(test)]
mod tests;
/// Typst documents typeset (TypstView.tsx).
pub mod typst_view;

pub use choose::{choose, effective_view, views, Body, Choice, Fallback, View};
pub use frame::{chosen, Chosen, EntryEvent, EntryView, Place};
pub use pane::{resolve, should_show, toggle as toggle_pane, Many, PreviewPane, Shown, Summary};
pub use shell::{cover, uncover};
pub use sources::{init_with, sources, Native, Sources};

/// The narrowest the resize handle drags the pane (`Math.max(256, …)`).
pub const MIN_WIDTH: f32 = 256.;

/// The pane's width in pixels: the saved width, else `min(28%, 28rem)` of `container`, kept between
/// `min(16rem, container - the list's share)` and `min(60%, container - the list's share)` so the list keeps
/// 16 rem beside the sidebar (3 rem as a rail, 14 rem from `md`): `19rem` of the container, `30rem` when
/// `medium`. As in CSS, the minimum wins over the maximum.
pub fn pane_width(saved: Option<f64>, container: f32, rem: f32, medium: bool) -> f32 {
    let list = if medium { 30. } else { 19. } * rem;
    let min = (16. * rem).min(container - list).max(0.);
    let max = (0.6 * container).min(container - list);
    let wanted = saved.map_or_else(|| (0.28 * container).min(28. * rem), |width| width as f32);
    wanted.min(max).max(min)
}
