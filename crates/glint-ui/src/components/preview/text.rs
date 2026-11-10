//! Text and code, highlighted with the kit's Editor (read-only).
//! Ported from FilePreview.tsx's `TextPreview` and `useText`, src/lib/highlight.tsx and src/lib/highlighter.ts.
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                                   | Here                                                          |
//! |----------------------------------------------|---------------------------------------------------------------|
//! | `useText(entry, encoding)`                   | [`TextPreview::read`], through a [`TextSource`]                |
//! | `TEXT_BYTES`, `TEXT_LINES`, `SHOWS_START`    | [`TEXT_BYTES`], [`TEXT_LINES`], the note under the text        |
//! | `SyntaxHighlighter` (Prism)                  | the kit's Editor, read-only, highlighted by syntect            |
//! | `LANGUAGE_BY_EXT`, `languageFor`             | [`language_for`], [`LANGUAGE_NAMES`]                           |
//! | the wrap, encoding and copy buttons          | the toolbar at the top corner                                  |
//! | `ENCODINGS`, `encodingName`, `encodingMenu`  | [`ENCODINGS`], [`encoding_name`], the encoding menu            |
//! | `NCDU_EXPORT` and its banner                 | [`is_ncdu_export`], the banner above the text                  |
//! | a binary file                                | a [`HexPreview`] in its place (Windows' own previewer is the frame's) |
//! | `Waiting`, `Centered`, `BigIcon`             | [`waiting`], [`note_centered`], [`big_icon`] (the other previewers use them) |
//!
//! The file is read in the background (the first 256 KB, in its own encoding or the one chosen), and only
//! the first 2,000 lines are shown, with the note that says so.
//!
//! Not ported: "Convert to UTF-8" in the encoding menu (it runs glint-core's `text_to_utf8` action with
//! progress and an Undo, which the interface has no shared helper for yet).

use super::hex::HexPreview;
use crate::kit::prelude::*;
use crate::kit::{Editor, EditorState};
use crate::{actions, core_bridge, i18n, state, t, theme};
use glint_core::fsx::TextPreview as TextRead;
use gpui_kit::{
    anchored, deferred, Anchor, ClipboardItem, DismissEvent, ScrollHandle, Task, UniformListScrollHandle, WeakEntity,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How much of a file is read for its text preview (`TEXT_BYTES`).
pub const TEXT_BYTES: usize = 256 * 1024;
/// How many lines of it are shown (`TEXT_LINES`).
pub const TEXT_LINES: usize = 2000;

/// How long a read may take before the spinner shows: a spinner is only for what takes a moment (`Waiting`).
#[cfg(not(test))]
pub(super) const SPINNER_DELAY: Duration = Duration::from_millis(300);
/// Tests show the spinner at once.
#[cfg(test)]
pub(super) const SPINNER_DELAY: Duration = Duration::ZERO;

// ---- Languages --------------------------------------------------------------------------------------------

/// The languages Glint highlights, by Prism's names (`LANGUAGE_NAMES` in highlight.tsx): anything else
/// previews as plain text.
pub const LANGUAGE_NAMES: [&str; 20] = [
    "bash",
    "c",
    "cpp",
    "csharp",
    "css",
    "go",
    "java",
    "javascript",
    "json",
    "jsx",
    "markdown",
    "markup",
    "powershell",
    "python",
    "rust",
    "sql",
    "toml",
    "tsx",
    "typescript",
    "yaml",
];

/// A file extension's language, by Prism's name (`languageFor`): "text" for the rest.
pub fn language_for(ext: &str) -> &'static str {
    match ext {
        "sh" | "bash" => "bash",
        "c" | "h" => "c",
        "cc" | "cpp" | "hpp" => "cpp",
        "cs" => "csharp",
        "css" | "scss" => "css",
        "go" => "go",
        "java" => "java",
        "js" | "mjs" | "cjs" => "javascript",
        "json" => "json",
        "jsx" => "jsx",
        "md" => "markdown",
        "html" | "htm" | "xml" | "svg" | "xaml" | "csproj" | "vue" => "markup",
        "ps1" => "powershell",
        "py" => "python",
        "rs" => "rust",
        "sql" => "sql",
        "toml" => "toml",
        "tsx" => "tsx",
        "ts" | "mts" => "typescript",
        "yml" | "yaml" => "yaml",
        _ => "text",
    }
}

/// The name the editor's highlighter (syntect's grammars) knows a Prism language by; None for plain text.
/// Syntect finds a grammar by file extension or by name, so these are the extensions the grammars list.
pub fn editor_language(prism: &str) -> Option<&'static str> {
    Some(match prism {
        "bash" => "sh",
        "c" => "c",
        "cpp" => "cpp",
        "csharp" => "cs",
        "css" => "css",
        "go" => "go",
        "java" => "java",
        "javascript" => "js",
        "json" => "json",
        "jsx" => "jsx",
        "markdown" => "md",
        "markup" => "html",
        "powershell" => "ps1",
        "python" => "py",
        "rust" => "rs",
        "sql" => "sql",
        "toml" => "toml",
        "tsx" => "tsx",
        "typescript" => "ts",
        "yaml" => "yaml",
        _ => return None,
    })
}

// ---- Encodings --------------------------------------------------------------------------------------------

/// Encodings to read a text file as (encoding_rs's names), with what people call them (`ENCODINGS`).
pub const ENCODINGS: [(&str, &str); 22] = [
    ("UTF-8", "Unicode (UTF-8)"),
    ("windows-1256", "Arabic (Windows-1256)"),
    ("ISO-8859-6", "Arabic (ISO-8859-6)"),
    ("windows-1252", "Western (Windows-1252)"),
    ("windows-1250", "Central European (Windows-1250)"),
    ("windows-1251", "Cyrillic (Windows-1251)"),
    ("KOI8-R", "Cyrillic (KOI8-R)"),
    ("IBM866", "Cyrillic (DOS 866)"),
    ("windows-1253", "Greek (Windows-1253)"),
    ("windows-1254", "Turkish (Windows-1254)"),
    ("windows-1255", "Hebrew (Windows-1255)"),
    ("windows-1257", "Baltic (Windows-1257)"),
    ("windows-1258", "Vietnamese (Windows-1258)"),
    ("windows-874", "Thai (Windows-874)"),
    ("Shift_JIS", "Japanese (Shift_JIS)"),
    ("EUC-JP", "Japanese (EUC-JP)"),
    ("gb18030", "Chinese, simplified (GB18030)"),
    ("GBK", "Chinese, simplified (GBK)"),
    ("Big5", "Chinese, traditional (Big5)"),
    ("EUC-KR", "Korean (EUC-KR)"),
    ("UTF-16LE", "Unicode (UTF-16)"),
    ("UTF-16BE", "Unicode (UTF-16, big-endian)"),
];

/// What an encoding is called (`encodingName`): its entry in [`ENCODINGS`], or the label as it is.
pub fn encoding_name(label: &str) -> String {
    ENCODINGS
        .iter()
        .find(|(l, _)| l.eq_ignore_ascii_case(label))
        .map(|(_, name)| (*name).to_string())
        .unwrap_or_else(|| label.to_string())
}

// ---- ncdu exports -----------------------------------------------------------------------------------------

/// The start of an ncdu export (`ncdu -o`): `[1,` its minor version, then its metadata naming the program
/// (`NCDU_EXPORT`, `/^\uFEFF?\s*\[\s*1\s*,\s*\d+\s*,\s*\{\s*"progname"/`, written out).
pub fn is_ncdu_export(text: &str) -> bool {
    fn after<'a>(text: &'a str, literal: &str) -> Option<&'a str> {
        text.trim_start_matches(char::is_whitespace).strip_prefix(literal)
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = after(text, "[") else { return false };
    let Some(rest) = after(rest, "1") else { return false };
    let Some(rest) = after(rest, ",") else { return false };
    let rest = rest.trim_start_matches(char::is_whitespace);
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return false;
    }
    let Some(rest) = rest.get(digits..) else { return false };
    let Some(rest) = after(rest, ",") else { return false };
    let Some(rest) = after(rest, "{") else { return false };
    after(rest, "\"progname\"").is_some()
}

// ---- What the previewers share ----------------------------------------------------------------------------

/// A file's extension as the listing keeps it: lowercase, without the dot, empty for a name that starts with
/// its only dot (`.gitignore`) or has none.
pub(super) fn ext_of(path: &str) -> String {
    let name = glint_model::paths::base_name(path);
    match name.rfind('.') {
        Some(dot) if dot > 0 => name.get(dot + 1..).unwrap_or("").to_lowercase(),
        _ => String::new(),
    }
}

/// A file's name as the accessibility tree reads it.
pub(super) fn name_of(path: &str) -> SharedString {
    SharedString::from(glint_model::paths::base_name(path).to_string())
}

/// The listing's entry for a path we only know the name of, for the kind of file it is.
pub(super) fn entry_of(path: &str) -> glint_model::entries::FileEntry {
    glint_model::entries::with_details(
        glint_model::entries::RawEntry {
            name: glint_model::paths::base_name(path).to_string(),
            path: Some(path.to_string()),
            ..Default::default()
        },
        "",
    )
}

/// Whether the file is read out of an archive or from a remote (what `isArchived(entry)` said): its parent
/// is in an archive, or it is on a connection. A zip file itself is a file like any other.
pub(super) fn is_archived_path(path: &str) -> bool {
    use glint_model::paths::{in_archive_path, is_remote_path, parent_path};
    is_remote_path(path) || in_archive_path(&parent_path(path))
}

/// What is still being read (`Waiting`): nothing at first, a spinner once it has taken a moment.
pub(super) fn waiting(started: Instant, what: Option<SharedString>, cx: &App) -> AnyElement {
    if started.elapsed() < SPINNER_DELAY {
        return div().size_full().into_any_element();
    }
    let t = theme::tokens(cx);
    let label = what.clone().unwrap_or_else(|| t!("Loading…"));
    col()
        .id("preview-waiting")
        .role(Role::Status)
        .aria_label(label)
        .test_support()
        .size_full()
        .items_center()
        .justify_center()
        .gap(space(3.))
        .p(space(6.))
        .text_size(theme::size::SM)
        .text_color(t.muted_foreground)
        .child(Spinner::new().color(t.primary))
        .children(what.map(|what| div().text_center().child(what)))
        .into_any_element()
}

/// Redraws `view` once the spinner's delay has passed. Nothing in tests, where the spinner shows at once.
pub(super) fn redraw_after_delay<V: 'static>(cx: &mut Context<V>) {
    if SPINNER_DELAY.is_zero() {
        return;
    }
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(SPINNER_DELAY).await;
        this.update(cx, |_, cx| cx.notify()).ok();
    })
    .detach();
}

/// A message in the middle of the pane (`Centered`), named for screen readers by the message itself.
pub(super) fn note_centered(id: &'static str, text: SharedString, cx: &App) -> AnyElement {
    let t = theme::tokens(cx);
    col()
        .id(id)
        .role(Role::Status)
        .aria_label(text.clone())
        .test_support()
        .size_full()
        .items_center()
        .justify_center()
        .gap(space(3.))
        .p(space(6.))
        .text_center()
        .text_size(theme::size::SM)
        .text_color(t.muted_foreground)
        .child(text)
        .into_any_element()
}

/// The file's kind, large, with why it isn't shown (`BigIcon`).
///
/// PORT ME(coordinator): the archive password form (`PasswordForm`) that the TypeScript showed under a note
/// asking for one; the password dialog (`dialogs::password`) is the place it opens from.
pub(super) fn big_icon(path: &str, note: Option<SharedString>, cx: &App) -> AnyElement {
    let kind = glint_model::file_types::kind_of(&entry_of(path));
    let tone = theme::kind_tone(kind, cx);
    let t = theme::tokens(cx);
    col()
        .id("preview-big-icon")
        .role(Role::Group)
        .aria_label(note.clone().unwrap_or_else(|| name_of(path)))
        .test_support()
        .size_full()
        .items_center()
        .justify_center()
        .gap(space(3.))
        .p(space(6.))
        .text_center()
        .text_size(theme::size::SM)
        .text_color(t.muted_foreground)
        .child(
            icon(crate::kit::kind_icon(kind.icon))
                .size(space(16.))
                .text_color(tone.color),
        )
        .children(note.map(|note| div().max_w(space(96.)).child(note)))
        .into_any_element()
}

/// A read of a file's start as text: the path and the encoding to read it as (None: as detected).
pub type TextSource =
    Arc<dyn Fn(String, Option<String>) -> Pin<Box<dyn Future<Output = Result<TextRead, String>> + Send>> + Send + Sync>;

/// glint-core's `read_text_preview`, as a [`TextSource`] (the first [`TEXT_BYTES`] of the file).
pub fn core_text_source() -> TextSource {
    Arc::new(|path, encoding| Box::pin(glint_core::fsx::read_text_preview(path, TEXT_BYTES, encoding)))
}

/// Scrolls a scroll handle's content a page (0.85 of what shows), as Page Down and Page Up do in Quick Look.
/// Returns whether it moved.
pub(super) fn page_scroll(handle: &ScrollHandle, down: bool) -> bool {
    let max = handle.max_offset().y;
    let viewport = handle.bounds().size.height;
    if max <= px(0.) {
        return false;
    }
    let offset = handle.offset();
    let step = viewport * 0.85;
    let top = if down { offset.y - step } else { offset.y + step }.clamp(-max, px(0.));
    if top == offset.y {
        return false;
    }
    handle.set_offset(point(offset.x, top));
    true
}

/// [`page_scroll`] for a `uniform_list`'s handle.
pub(super) fn page_list(handle: &UniformListScrollHandle, down: bool) -> bool {
    let base = handle.0.borrow().base_handle.clone();
    page_scroll(&base, down)
}

/// `px` from the reading direction's end edge, for something placed over a corner of its parent.
pub(super) fn at_end(element: Div, offset: Rems) -> Div {
    if i18n::is_rtl() {
        element.left(offset)
    } else {
        element.right(offset)
    }
}

// ---- The previewer ----------------------------------------------------------------------------------------

/// What the previewer is showing.
enum Load {
    /// Still reading.
    Waiting,
    /// The read failed, with why.
    Failed(SharedString),
    /// Not text: its bytes (`HexView`; Windows' previewer for it is the frame's to offer).
    Binary(Entity<HexPreview>),
    /// "This file is empty."
    Empty,
    /// The text, in the editor.
    Shown(Shown),
}

/// A file's text on screen.
struct Shown {
    editor: Entity<EditorState>,
    /// All that was read, for Copy (the editor holds the first [`TEXT_LINES`] lines).
    full_text: SharedString,
    /// The read stopped at [`TEXT_BYTES`].
    truncated: bool,
    /// More than [`TEXT_LINES`] lines were read.
    cut: bool,
    /// What it was read as, when not plain UTF-8.
    encoding: Option<String>,
    /// Whether that was a guess.
    guessed: bool,
    /// An ncdu export: its folders can be browsed.
    ncdu: bool,
}

/// The encoding menu while it is open.
struct OpenMenu {
    at: Point<Pixels>,
    menu: Entity<PopupMenu>,
    _dismissed: Subscription,
}

/// Text and code, highlighted with the kit's Editor (read-only).
pub struct TextPreview {
    path: String,
    source: TextSource,
    state: Load,
    started: Instant,
    /// The encoding picked from the menu (None: as detected).
    encoding: Option<String>,
    wrap: bool,
    copied: bool,
    menu: Option<OpenMenu>,
    /// Which read is current: an answer for an older one is dropped.
    generation: u64,
    _read: Task<()>,
    _copied_reset: Option<Task<()>>,
    _settings: Subscription,
}

impl TextPreview {
    /// The previewer for the file at `path`. Reading happens in the background; the view shows its loading state
    /// until then.
    pub fn new(path: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_source(path, core_text_source(), window, cx)
    }

    /// [`TextPreview::new`] reading through `source` (tests pass fakes).
    pub fn with_source(path: String, source: TextSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings =
            cx.observe_global_in::<state::UiState>(window, |this, window, cx| this.apply_settings(window, cx));
        let mut this = Self {
            path,
            source,
            state: Load::Waiting,
            started: Instant::now(),
            encoding: None,
            wrap: false,
            copied: false,
            menu: None,
            generation: 0,
            _read: Task::ready(()),
            _copied_reset: None,
            _settings: settings,
        };
        this.read(window, cx);
        this
    }

    /// The file shown.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows another file (the pane's selection moved): the encoding choice starts over, the wrap stays.
    pub fn set_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.path == path {
            return;
        }
        self.path = path;
        self.encoding = None;
        self.menu = None;
        self.read(window, cx);
    }

    /// Reads the file again (it changed on disk: the TypeScript re-read when `entry.modified` changed).
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.read(window, cx);
    }

    /// Scrolls the text a page (Page Down and Page Up in Quick Look). Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        match &self.state {
            Load::Shown(shown) => shown.editor.update(cx, |editor, cx| {
                let offset = editor.scroll_offset();
                let step = editor.input_bounds().size.height * 0.85;
                let y = (if down { offset.y - step } else { offset.y + step }).min(px(0.));
                if y == offset.y {
                    return false;
                }
                editor.set_scroll_offset(point(offset.x, y), cx);
                true
            }),
            Load::Binary(hex) => hex.update(cx, |hex, cx| hex.page(down, cx)),
            _ => false,
        }
    }

    /// Whether long lines wrap (off at first, as the TypeScript's `wrap`).
    pub fn wrapping(&self) -> bool {
        self.wrap
    }

    /// Wraps long lines or scrolls them sideways.
    pub fn set_wrap(&mut self, wrap: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.wrap = wrap;
        if let Load::Shown(shown) = &self.state {
            shown
                .editor
                .update(cx, |editor, cx| editor.set_soft_wrap(wrap, window, cx));
        }
        cx.notify();
    }

    /// Reads the file as `label` says (an encoding's name from [`ENCODINGS`]).
    pub fn set_encoding(&mut self, label: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.encoding = label;
        self.menu = None;
        self.read(window, cx);
    }

    /// What the file was read as, when it isn't plain UTF-8, and whether that was a guess.
    pub fn encoding_read(&self) -> Option<(&str, bool)> {
        match &self.state {
            Load::Shown(shown) => shown.encoding.as_deref().map(|e| (e, shown.guessed)),
            _ => None,
        }
    }

    /// The lines on screen, when there are some.
    pub fn shown_text(&self, cx: &App) -> Option<SharedString> {
        match &self.state {
            Load::Shown(shown) => Some(shown.editor.read(cx).value()),
            _ => None,
        }
    }

    fn read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.state = Load::Waiting;
        self.started = Instant::now();
        let read = (self.source)(self.path.clone(), self.encoding.clone());
        self._read = cx.spawn_in(window, async move |this, cx| {
            // Two layers: the core task may panic (outer Err), the file may not read (inner Err).
            let result = core_bridge::run(read).await.and_then(|inner| inner);
            this.update_in(cx, |this, window, cx| {
                if this.generation == generation {
                    this.arrived(result, window, cx);
                }
            })
            .ok();
        });
        redraw_after_delay(cx);
        cx.notify();
    }

    /// The read has an answer.
    fn arrived(&mut self, result: Result<TextRead, String>, window: &mut Window, cx: &mut Context<Self>) {
        self.state = match result {
            Err(reason) => Load::Failed(reason.into()),
            Ok(read) if read.binary => {
                let path = self.path.clone();
                Load::Binary(cx.new(|cx| HexPreview::new(path, window, cx)))
            }
            Ok(read) if read.text.is_empty() => Load::Empty,
            Ok(read) => Load::Shown(self.show(read, window, cx)),
        };
        cx.notify();
    }

    /// The editor for a read.
    fn show(&self, read: TextRead, window: &mut Window, cx: &mut Context<Self>) -> Shown {
        let lines = read.text.split('\n').count();
        let cut = lines > TEXT_LINES;
        // Lines end with \n in the editor; the \r of a Windows file isn't a character to draw.
        let shown: String = if cut {
            read.text.split('\n').take(TEXT_LINES).collect::<Vec<_>>().join("\n")
        } else {
            read.text.clone()
        };
        let shown = shown.replace("\r\n", "\n");
        let language = editor_language(language_for(&ext_of(&self.path)));
        let numbers = state::settings(cx).show_line_numbers;
        let wrap = self.wrap;
        let editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .line_number(numbers)
                .soft_wrap(wrap)
                .searchable(false)
                .folding(false);
            if let Some(language) = language {
                editor = editor.language(language);
            }
            editor.default_value(shown)
        });
        editor.update(cx, |editor, cx| editor.set_readonly(true, cx));
        Shown {
            editor,
            ncdu: !is_archived_path(&self.path) && is_ncdu_export(&read.text),
            full_text: read.text.into(),
            truncated: read.truncated,
            cut,
            encoding: read.encoding,
            guessed: read.guessed,
        }
    }

    /// A setting changed: the line numbers follow `showLineNumbers`.
    fn apply_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let numbers = state::settings(cx).show_line_numbers;
        if let Load::Shown(shown) = &self.state {
            shown
                .editor
                .update(cx, |editor, cx| editor.set_line_number(numbers, window, cx));
        }
        cx.notify();
    }

    /// Copy: all the text that was read, to the clipboard, with a check mark for a moment.
    fn copy_text(&mut self, cx: &mut Context<Self>) {
        let Load::Shown(shown) = &self.state else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(shown.full_text.to_string()));
        self.copied = true;
        cx.notify();
        if SPINNER_DELAY.is_zero() {
            // Tests: no timer; the mark stays so it can be looked at.
            return;
        }
        self._copied_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1400)).await;
            this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Opens the encoding menu below the click.
    fn open_encoding_menu(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Shown(shown) = &self.state else { return };
        let current = shown.encoding.clone().unwrap_or_default();
        let this: WeakEntity<Self> = cx.weak_entity();
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            let mut menu = menu.item(PopupMenuItem::label(t!("Read as")));
            for (label, name) in ENCODINGS {
                let this = this.clone();
                menu = menu.item(
                    PopupMenuItem::new(name)
                        .checked(label.eq_ignore_ascii_case(&current))
                        .on_click(move |_, window, cx| {
                            this.update(cx, |this, cx| this.set_encoding(Some(label.to_string()), window, cx))
                                .ok();
                        }),
                );
            }
            // PORT ME(coordinator): "Convert to UTF-8" above the list (glint-core's text_to_utf8 action with
            // its progress and Undo), for a file on disk that isn't UTF-8 already.
            menu
        });
        window.focus(&menu.focus_handle(cx), cx);
        let dismissed = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.menu = None;
            cx.notify();
        });
        self.menu = Some(OpenMenu {
            at,
            menu,
            _dismissed: dismissed,
        });
        cx.notify();
    }

    /// The buttons over the text's corner: encoding, wrap, copy.
    fn toolbar(&self, shown: &Shown, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let encoding = shown.encoding.as_ref().map(|label| {
            let name = encoding_name(label);
            let tip = if shown.guessed {
                t!("Detected as {name}: choose another", name = name)
            } else {
                t!("Read as {name}: choose another", name = name)
            };
            Button::new("text-encoding")
                .custom(
                    ButtonCustomVariant::new(cx)
                        .color(t.muted.opacity(0.))
                        .foreground(
                            theme::tailwind(if t.dark { "amber-400" } else { "amber-700" }).unwrap_or(t.foreground),
                        )
                        .hover(t.muted)
                        .active(t.muted),
                )
                .compact()
                .icon(IconName::Languages)
                .label(isolate(name))
                .tooltip(tip.clone())
                .accessibility_label(tip)
                .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                    let at = event.position() + point(px(0.), px(16.));
                    this.open_encoding_menu(at, window, cx);
                }))
        });
        let wrap = self.wrap;
        let wrap_tip = if wrap {
            t!("Unwrap lines (scroll horizontally)")
        } else {
            t!("Wrap long lines")
        };
        let wrap_button = Button::new("text-wrap")
            .ghost()
            .compact()
            .icon(IconName::TextWrap)
            .selected(wrap)
            .tooltip(wrap_tip.clone())
            .accessibility_label(wrap_tip)
            .on_click(cx.listener(|this, _, window, cx| {
                let wrap = !this.wrap;
                this.set_wrap(wrap, window, cx);
            }));
        let copy_tip = t!("Copy text to clipboard");
        let copy_button = Button::new("text-copy")
            .ghost()
            .compact()
            .icon(if self.copied { IconName::Check } else { IconName::Copy })
            .tooltip(copy_tip.clone())
            .accessibility_label(copy_tip)
            .on_click(cx.listener(|this, _, _, cx| this.copy_text(cx)));
        row_start()
            .id("text-toolbar")
            .role(Role::Toolbar)
            .aria_label(t!("Text tools"))
            .test_support()
            .occlude()
            .gap(space(0.5))
            .p(space(0.5))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.border)
            .bg(t.card.opacity(0.9))
            .children(encoding)
            .child(wrap_button)
            .child(copy_button)
    }

    /// The banner over an ncdu export: its folders can be browsed (`NCDU_EXPORT`).
    fn ncdu_banner(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        row_start()
            .id("text-ncdu")
            .role(Role::Group)
            .aria_label(t!("An ncdu export: its folders and their sizes can be browsed."))
            .test_support()
            .items_center()
            .gap(space(3.))
            .px(space(4.))
            .py(space(2.))
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(t!("An ncdu export: its folders and their sizes can be browsed.")),
            )
            .child(
                Button::new("text-ncdu-open")
                    .outline()
                    .small()
                    .icon(IconName::FolderOpen)
                    .label(t!("Open like a folder"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let path = this.path.clone();
                        if let Some(handle) = cx.try_global::<crate::explorer::ExplorerHandle>() {
                            handle
                                .0
                                .clone()
                                .update(cx, |explorer, cx| explorer.go_to(&path, cx).detach());
                        }
                    })),
            )
    }
}

impl Render for TextPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let root = col().id("text-preview").size_full().min_h_0();
        let body = match &self.state {
            Load::Waiting => waiting(self.started, None, cx),
            Load::Failed(reason) => big_icon(&self.path, Some(reason.clone()), cx),
            Load::Binary(hex) => hex.clone().into_any_element(),
            Load::Empty => note_centered("text-empty", t!("This file is empty."), cx),
            Load::Shown(shown) => {
                let toolbar = at_end(
                    div().absolute().top(space(2.)).child(self.toolbar(shown, &t, cx)),
                    space(3.),
                );
                let note = (shown.cut || shown.truncated).then(|| {
                    let text = t!("Preview shows the beginning of the file. Open it to see the rest.");
                    div()
                        .id("text-shows-start")
                        .role(Role::Status)
                        .aria_label(text.clone())
                        .test_support()
                        .px(space(4.))
                        .pb(space(3.))
                        .pt(space(1.))
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(text)
                });
                col()
                    .size_full()
                    .min_h_0()
                    .children(shown.ncdu.then(|| self.ncdu_banner(&t, cx)))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .child(
                                Editor::new(&shown.editor)
                                    .readonly(true)
                                    .appearance(false)
                                    .bordered(false)
                                    .aria_label(name_of(&self.path))
                                    .text_size(rems(0.8125))
                                    .size_full(),
                            )
                            .child(toolbar),
                    )
                    .children(note)
                    .into_any_element()
            }
        };
        let menu = self.menu.as_ref().map(|open| {
            deferred(
                anchored()
                    .position(open.at)
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(open.menu.clone()),
            )
            .with_priority(1)
        });
        root.text_start()
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollDown, _, cx| {
                this.page(true, cx);
            }))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollUp, _, cx| {
                this.page(false, cx);
            }))
            .child(body)
            .children(menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;

    fn read_of(text: &str) -> TextRead {
        TextRead {
            text: text.to_string(),
            truncated: false,
            binary: false,
            encoding: None,
            guessed: false,
        }
    }

    /// What a fake source was asked: the path and the encoding.
    type Asked = Arc<Mutex<Vec<(String, Option<String>)>>>;

    /// A source answering with `answer`, remembering what it was asked.
    fn fake(answer: Result<TextRead, String>) -> (TextSource, Asked) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let log = asked.clone();
        let source: TextSource = Arc::new(move |path, encoding| {
            if let Ok(mut log) = log.lock() {
                log.push((path, encoding));
            }
            let answer = match &answer {
                Ok(read) => Ok(TextRead {
                    text: read.text.clone(),
                    truncated: read.truncated,
                    binary: read.binary,
                    encoding: read.encoding.clone(),
                    guessed: read.guessed,
                }),
                Err(reason) => Err(reason.clone()),
            };
            Box::pin(async move { answer })
        });
        (source, asked)
    }

    /// Opens a previewer for `path` reading through `source`, in a window of its own.
    fn preview(cx: &mut TestAppContext, path: &str, source: TextSource) -> (Entity<TextPreview>, VisualTestContext) {
        cx.update(testing::init);
        let path = path.to_string();
        let (visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| TextPreview::with_source(path, source, window, cx))
        });
        (view, visual)
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the text never arrived");
    }

    #[test]
    fn languages_follow_the_typescript_table() {
        for (ext, language) in [
            ("rs", "rust"),
            ("ts", "typescript"),
            ("mts", "typescript"),
            ("tsx", "tsx"),
            ("jsx", "jsx"),
            ("js", "javascript"),
            ("mjs", "javascript"),
            ("cjs", "javascript"),
            ("py", "python"),
            ("sh", "bash"),
            ("ps1", "powershell"),
            ("html", "markup"),
            ("svg", "markup"),
            ("csproj", "markup"),
            ("vue", "markup"),
            ("scss", "css"),
            ("cs", "csharp"),
            ("hpp", "cpp"),
            ("h", "c"),
            ("yml", "yaml"),
            ("md", "markdown"),
            ("toml", "toml"),
            ("sql", "sql"),
            ("txt", "text"),
            ("", "text"),
            ("RS", "text"),
        ] {
            assert_eq!(language_for(ext), language, "{ext}");
        }
        // Every language the TypeScript bundled has a grammar name here; plain text has none.
        for name in LANGUAGE_NAMES {
            assert!(editor_language(name).is_some(), "{name}");
        }
        assert_eq!(editor_language("text"), None);
    }

    #[test]
    fn encodings_are_named_as_people_call_them() {
        assert_eq!(encoding_name("windows-1256"), "Arabic (Windows-1256)");
        assert_eq!(encoding_name("WINDOWS-1256"), "Arabic (Windows-1256)");
        assert_eq!(encoding_name("UTF-16LE"), "Unicode (UTF-16)");
        assert_eq!(encoding_name("x-unknown"), "x-unknown");
        assert_eq!(ENCODINGS.len(), 22);
    }

    #[test]
    fn an_ncdu_export_is_told_by_its_first_bytes() {
        assert!(is_ncdu_export(
            r#"[1,2,{"progname":"ncdu","progver":"2.3"},[{"name":"/"}]]"#
        ));
        assert!(is_ncdu_export("\u{feff}  [ 1 , 0 , { \"progname\" : \"ncdu\" }"));
        assert!(!is_ncdu_export(r#"[2,0,{"progname":"ncdu"}]"#));
        assert!(!is_ncdu_export(r#"[1,{"progname":"ncdu"}]"#));
        assert!(!is_ncdu_export(r#"[1,2,{"name":"x"}]"#));
        assert!(!is_ncdu_export(r#"{"a":1}"#));
        assert!(!is_ncdu_export(""));
    }

    #[test]
    fn paths_give_their_extension_and_whether_they_are_in_an_archive() {
        assert_eq!(ext_of("C:\\src\\Main.RS"), "rs");
        assert_eq!(ext_of("C:\\src\\.gitignore"), "");
        assert_eq!(ext_of("C:\\src\\Makefile"), "");
        assert_eq!(ext_of("C:\\notes\\تقرير.md"), "md");
        assert!(!is_archived_path("C:\\files\\a.zip"));
        assert!(is_archived_path("C:\\files\\a.zip\\inside\\b.txt"));
    }

    #[gpui_kit::test]
    fn the_text_shows_in_a_read_only_editor_named_for_the_file(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (source, asked) = fake(Ok(read_of("fn main() {\n    println!(\"hi\");\n}\n")));
        let (view, mut visual) = preview(cx, "C:\\src\\main.rs", source);
        // The read is under way: the UI thread didn't wait for it.
        assert!(view.read_with(&visual, |v, _| matches!(v.state, Load::Waiting)));
        assert!(has(&mut visual, "Loading…"));
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.state, Load::Shown(_)))
        });
        assert_eq!(
            asked.lock().map(|a| a.clone()).unwrap_or_default(),
            vec![("C:\\src\\main.rs".to_string(), None)]
        );
        assert!(has(&mut visual, "main.rs"), "the editor is named for the file");
        let (text, language) = view.read_with(&visual, |v, cx| match &v.state {
            Load::Shown(shown) => (shown.editor.read(cx).value(), shown.editor.read(cx).language_name()),
            _ => Default::default(),
        });
        assert_eq!(text.as_ref(), "fn main() {\n    println!(\"hi\");\n}\n");
        assert_eq!(language.as_ref(), "rs", "highlighted as Rust");
        assert!(!has(
            &mut visual,
            "Preview shows the beginning of the file. Open it to see the rest."
        ));
        // The toolbar's buttons are named.
        for label in ["Wrap long lines", "Copy text to clipboard"] {
            assert!(has(&mut visual, label), "{label}");
        }
        // No encoding button for plain UTF-8.
        assert!(view.read_with(&visual, |v, _| v.encoding_read().is_none()));
        // The line numbers follow the setting, and the view keeps drawing.
        visual.update(|_, cx| state::update(cx, |settings| settings.show_line_numbers = false));
        visual.run_until_parked();
        assert!(has(&mut visual, "main.rs"));
        visual.update(|_, cx| state::update(cx, |settings| settings.show_line_numbers = true));
        visual.run_until_parked();
        assert!(has(&mut visual, "main.rs"));
    }

    #[gpui_kit::test]
    fn a_big_file_shows_its_first_lines_and_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let text: String = (0..3000).map(|i| format!("line {i}\n")).collect();
        let mut read = read_of(&text);
        read.truncated = true;
        let (source, _) = fake(Ok(read));
        let (view, mut visual) = preview(cx, "C:\\logs\\big.log", source);
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.state, Load::Shown(_)))
        });
        let shown = view.read_with(&visual, |v, cx| v.shown_text(cx)).unwrap_or_default();
        // 2,000 lines, not 3,000 (the 2,001st would be "line 2000").
        assert_eq!(shown.split('\n').count(), TEXT_LINES);
        assert!(shown.ends_with("line 1999"));
        assert!(!shown.contains("line 2000"));
        assert!(has(
            &mut visual,
            "Preview shows the beginning of the file. Open it to see the rest."
        ));
        // Copy has everything that was read.
        visual.update(|window, cx| window.click("text-copy", cx));
        visual.run_until_parked();
        let copied = visual.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some(text.as_str()));
        // A short file that stopped at the byte limit says so too.
        let mut short = read_of("a\nb\n");
        short.truncated = true;
        let (source, _) = fake(Ok(short));
        let (view2, mut visual2) = preview(cx, "C:\\logs\\cut.log", source);
        settle(&mut visual2, |cx| {
            view2.read_with(cx, |v, _| matches!(v.state, Load::Shown(_)))
        });
        assert!(has(
            &mut visual2,
            "Preview shows the beginning of the file. Open it to see the rest."
        ));
    }

    #[gpui_kit::test]
    fn arabic_text_shows_in_the_arabic_interface(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        let arabic = "# تقرير الربع الأول\nمرحبا بالعالم\n";
        let (source, _) = fake(Ok(read_of(arabic)));
        let (view, mut visual) = preview(cx, "C:\\مستندات\\ملاحظات.md", source);
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.state, Load::Shown(_)))
        });
        assert!(has(&mut visual, "ملاحظات.md"));
        assert_eq!(
            view.read_with(&visual, |v, cx| v.shown_text(cx)).as_deref(),
            Some(arabic)
        );
        // The toolbar is named in Arabic and still there (placed at the end edge, which is the left).
        assert!(has(&mut visual, &t!("Wrap long lines")));
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn a_failed_read_a_binary_file_and_an_empty_one_each_say_what_they_are(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        // Failed: the kind's icon and why.
        let (source, _) = fake(Err("Access is denied. (os error 5)".into()));
        let (view, mut visual) = preview(cx, "C:\\locked\\a.txt", source);
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.state, Load::Failed(_)))
        });
        assert!(has(&mut visual, "Access is denied. (os error 5)"));
        // Empty.
        let (source, _) = fake(Ok(read_of("")));
        let (view, mut visual) = preview(cx, "C:\\empty\\a.txt", source);
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.state, Load::Empty))
        });
        assert!(has(&mut visual, "This file is empty."));
        // Binary: the bytes take its place.
        let mut binary = read_of("");
        binary.binary = true;
        let (source, _) = fake(Ok(binary));
        let (view, mut visual) = preview(cx, "C:\\bin\\a.dat", source);
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.state, Load::Binary(_)))
        });
        assert_eq!(
            view.read_with(&visual, |v, cx| match &v.state {
                Load::Binary(hex) => hex.read(cx).path().to_string(),
                _ => String::new(),
            }),
            "C:\\bin\\a.dat"
        );
    }

    #[gpui_kit::test]
    fn another_encoding_reads_the_file_again_and_wrap_toggles(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut read = read_of("مرحبا");
        read.encoding = Some("windows-1256".into());
        read.guessed = true;
        let (source, asked) = fake(Ok(read));
        let (view, mut visual) = preview(cx, "C:\\old\\a.txt", source);
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.state, Load::Shown(_)))
        });
        assert_eq!(
            view.read_with(&visual, |v, _| v.encoding_read().map(|(e, g)| (e.to_string(), g))),
            Some(("windows-1256".into(), true))
        );
        // The encoding button says what it was detected as.
        assert!(has(&mut visual, "Detected as Arabic (Windows-1256): choose another"));
        // The button opens the encodings to choose from.
        visual.update(|window, cx| window.click("text-encoding", cx));
        visual.run_until_parked();
        assert!(
            view.read_with(&visual, |v, _| v.menu.is_some()),
            "the encoding menu is open"
        );
        // Choosing one reads again, asking for it.
        let before = asked.lock().map(|a| a.len()).unwrap_or(0);
        visual.update(|window, cx| view.update(cx, |v, cx| v.set_encoding(Some("ISO-8859-6".into()), window, cx)));
        settle(&mut visual, |_| asked.lock().map(|a| a.len()).unwrap_or(0) > before);
        assert_eq!(
            asked.lock().ok().and_then(|a| a.last().cloned()),
            Some(("C:\\old\\a.txt".to_string(), Some("ISO-8859-6".to_string())))
        );
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.state, Load::Shown(_)))
        });
        // The wrap button flips, and its name with it.
        assert!(has(&mut visual, "Wrap long lines"));
        visual.update(|window, cx| window.click("text-wrap", cx));
        visual.run_until_parked();
        assert!(view.read_with(&visual, |v, _| v.wrapping()));
        assert!(has(&mut visual, "Unwrap lines (scroll horizontally)"));
    }

    #[gpui_kit::test]
    fn the_real_core_reads_a_file_from_disk(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let dir = std::env::temp_dir().join(format!("glint-text-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("the temp folder");
        let file = dir.join("ملف.txt");
        std::fs::write(&file, "سطر أول\nline two\n").expect("the temp file");
        let (mut visual, view) = {
            cx.update(testing::init);
            let path = file.to_string_lossy().into_owned();
            testing::open_window(cx, move |window, cx| cx.new(|cx| TextPreview::new(path, window, cx)))
        };
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.state, Load::Shown(_)))
        });
        assert_eq!(
            view.read_with(&visual, |v, cx| v.shown_text(cx)).as_deref(),
            Some("سطر أول\nline two\n")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_icon_the_previewer_draws_is_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        let used = [
            IconName::Languages,
            IconName::TextWrap,
            IconName::Copy,
            IconName::Check,
            IconName::FolderOpen,
        ];
        for name in used {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }
}
