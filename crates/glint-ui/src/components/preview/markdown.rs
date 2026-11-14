//! Markdown rendered with the kit's TextView.
//! Ported from MarkdownView.tsx and `MarkdownPreview` in FilePreview.tsx.
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                                       | Here                                                          |
//! |--------------------------------------------------|---------------------------------------------------------------|
//! | `markdownHtml` (comrak) and the allow-list build | the kit's `TextView` parses CommonMark and GitHub's extensions itself (no HTML is made, so nothing can run) |
//! | `Picture` (beside it, same drive, data:)         | [`image_source`]: pictures beside the file, on its drive, or written in it; the web's are not fetched |
//! | `safeHref`, the `follow` of a link               | [`link_action`], run by the TextView's link handler            |
//! | links within the file scroll to their heading    | [`headings`] and `reveal_range`                                |
//! | code blocks highlighted                          | the TextView's own (syntect), by the fence's language          |
//! | `MermaidView` (```mermaid)                       | `diagram::DiagramBlock` (feature `diagrams`): merman, drawn as a picture; the source and the reason when it can't be |
//! | the same for ```dot, ```graphviz, ```gv          | `diagram::DiagramBlock`: layout-rs (new: the TypeScript drew only Mermaid) |
//! | `MathView` (`$…$`, `$$…$$`, ```math)             | `math::MathInline` and `math::MathBlock`: RaTeX, inline on the text's baseline, display centred; the LaTeX in a tinted span when it doesn't parse |
//! | `SHOWS_START`, "This file is empty."             | the same notes                                                 |
//! | a binary file                                    | a [`HexPreview`] in its place                                  |
//!
//! Not ported:
//! - Without the `diagrams` feature a ```mermaid block and a ```math block show as code, and `$…$` shows as its
//!   text. With it they are drawn (see [`super::diagram`], [`super::math`] and [`super::svg_image`] for how, and
//!   for what differs from Mermaid's and Temml's own drawing). The blocks, and the inline formulas, are the
//!   TextView's plugin nodes: selecting and copying them gives their source.
//! - Raw HTML in the Markdown (the TypeScript kept an allow-list of GitHub's tags): the TextView's Markdown
//!   parser shows what it understands. NOTE(port): READMEs that lean on `<div align="center">` and `<img>`
//!   will look plainer; the TextView's HTML mode (`TextView::html`) is the way to bring those back.
//! - Pictures inside an archive (the TypeScript extracted them to a temp copy as they showed).
//! - The repository root as the base of links that start with `/` (the TypeScript asked the Git view): the
//!   drive's root is used.

use super::hex::HexPreview;
use super::text::{big_icon, core_text_source, name_of, note_centered, redraw_after_delay, waiting, TextSource};
use crate::kit::prelude::*;
use crate::kit::{TextView, TextViewState};
use crate::{actions, core_bridge, t, theme};
use glint_core::fsx::TextPreview as TextRead;
use glint_model::markdown::safe_href;
use glint_model::paths::{linked_path, parent_path, volume_of};
use gpui_kit::{EntityId, Image, ImageFormat, ImageSource, Resource, SharedUri, Task, WeakEntity};
use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

// ---- Headings ---------------------------------------------------------------------------------------------

/// A heading of the document: its anchor, and where its text is in the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    /// The anchor GitHub gives it (`#my-title`, with `-1`, `-2` for repeats).
    pub slug: String,
    /// Its text's UTF-8 byte range in the Markdown source.
    pub range: Range<usize>,
}

/// Whether `c` stays in a GitHub anchor: a letter, a digit, a combining mark, `-` or `_`.
fn anchor_char(c: char) -> bool {
    c.is_alphanumeric()
        || c == '-'
        || c == '_'
        || matches!(c as u32, 0x0300..=0x036F | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06ED | 0x0E31 | 0x0E34..=0x0E3A)
}

/// A heading's anchor as GitHub writes it: lower case, spaces as hyphens, punctuation gone (the repeats get their
/// number from [`headings`]).
pub fn slug_of(text: &str) -> String {
    let mut out = String::new();
    for c in plain_heading(text).trim().chars().flat_map(char::to_lowercase) {
        if c == ' ' {
            out.push('-');
        } else if anchor_char(c) {
            out.push(c);
        }
    }
    out
}

/// The words of a heading without the Markdown around them: a link as its text, no emphasis or code marks.
fn plain_heading(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        // [text](url) and ![alt](url): the text.
        if c == '[' || (c == '!' && rest[1..].starts_with('[')) {
            let open = if c == '!' { 1 } else { 0 };
            if let Some(close) = rest[open..].find("](") {
                let label = &rest[open + 1..open + close];
                if let Some(end) = rest[open + close..].find(')') {
                    out.push_str(label);
                    rest = &rest[open + close + end + 1..];
                    continue;
                }
            }
        }
        if !matches!(c, '*' | '`' | '~') {
            out.push(c);
        }
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// The headings of a Markdown source, in order: `# Title` lines and `Title` over `===` or `---`, outside code
/// fences. Repeated titles get `-1`, `-2`, as GitHub numbers them.
pub fn headings(source: &str) -> Vec<Heading> {
    let mut found: Vec<(String, Range<usize>)> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut previous: Option<(Range<usize>, &str)> = None;
    let mut at = 0;
    for line in source.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let trimmed_end = line.trim_end_matches(['\n', '\r']);
        let indent = trimmed_end.len() - trimmed_end.trim_start_matches(' ').len();
        let body = &trimmed_end[indent..];
        // Code fences hide what is inside them.
        let fence_char = body.chars().next().filter(|c| matches!(c, '`' | '~'));
        if let Some(fc) = fence_char {
            let run = body.chars().take_while(|&c| c == fc).count();
            if run >= 3 && indent <= 3 {
                match fence {
                    None => fence = Some((fc, run)),
                    Some((open, n)) if open == fc && run >= n && body[run..].trim().is_empty() => fence = None,
                    _ => {}
                }
                previous = None;
                continue;
            }
        }
        if fence.is_some() {
            continue;
        }
        if indent <= 3 {
            // ATX: 1 to 6 #, then a space.
            let hashes = body.chars().take_while(|&c| c == '#').count();
            if (1..=6).contains(&hashes) && (body.len() == hashes || body[hashes..].starts_with([' ', '\t'])) {
                let text_start = hashes + body[hashes..].len() - body[hashes..].trim_start().len();
                let mut text = body[text_start.min(body.len())..].trim_end();
                // A closing run of #s.
                let without = text.trim_end_matches('#');
                if without.len() < text.len() && (without.is_empty() || without.ends_with([' ', '\t'])) {
                    text = without.trim_end();
                }
                if !text.is_empty() {
                    let from = start + indent + (body.len() - body[text_start.min(body.len())..].len());
                    found.push((slug_of(text), from..from + text.len()));
                }
                previous = None;
                continue;
            }
            // Setext: a line of = or - under a line of text.
            let underline = !body.is_empty()
                && (body.chars().all(|c| c == '=') || body.chars().all(|c| c == '-'))
                && trimmed_end.trim_end() == trimmed_end.trim_end_matches([' ', '\t']);
            if underline {
                if let Some((range, text)) = previous.take() {
                    found.push((slug_of(text.trim()), range));
                    continue;
                }
            }
        }
        previous = if body.is_empty() || indent > 3 || body.starts_with(['>', '-', '*', '+', '|']) {
            None
        } else {
            let from = start + indent;
            Some((from..from + body.trim_end().len(), body.trim_end()))
        };
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    found
        .into_iter()
        .map(|(slug, range)| {
            let count = seen.entry(slug.clone()).or_insert(0);
            let numbered = if *count == 0 { slug } else { format!("{slug}-{count}") };
            *count += 1;
            Heading { slug: numbered, range }
        })
        .collect()
}

// ---- Links ------------------------------------------------------------------------------------------------

/// What following a link does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkAction {
    /// Nothing: a link that goes nowhere (a script, a data or file URL).
    None,
    /// Scroll to this heading's text, a byte range of the source.
    Heading(Range<usize>),
    /// Open the address in the browser or the mail program (`https:`, `http:`, `mailto:`).
    Web(String),
    /// Open the file or folder here, in the explorer.
    Path(String),
}

/// `decodeURIComponent`, or the text as written when it isn't valid.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(byte) = hex {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// What a click on `href` does (`follow` in MarkdownView.tsx): links within the file scroll to their heading,
/// the web's and mail's open outside, and paths open here, relative to the file's folder (or the root, for `/`).
pub fn link_action(href: &str, folder: &str, root: &str, headings: &[Heading]) -> LinkAction {
    let Some(href) = safe_href(href) else {
        return LinkAction::None;
    };
    if let Some(id) = href.strip_prefix('#') {
        let id = percent_decode(id);
        // GitHub's own links to a heading carry its "user-content-" prefix.
        let wanted = [
            id.clone(),
            id.trim_start_matches("user-content-").to_string(),
            id.to_lowercase(),
        ];
        for want in wanted {
            if let Some(heading) = headings.iter().find(|h| h.slug == want) {
                return LinkAction::Heading(heading.range.clone());
            }
        }
        return LinkAction::None;
    }
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("http:") || lower.starts_with("https:") || lower.starts_with("mailto:") {
        return LinkAction::Web(href.to_string());
    }
    match linked_path(href, folder, root) {
        Some(path) => LinkAction::Path(path),
        None => LinkAction::None,
    }
}

// ---- Pictures ---------------------------------------------------------------------------------------------

/// A transparent 1 x 1 picture: what a picture the preview doesn't load shows as, so nothing is fetched.
const BLANK_PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

/// Base64 (standard or URL alphabet, padding optional) as bytes; None when it isn't base64.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut bits, mut have) = (0u32, 0u32);
    for c in text.chars() {
        let value = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32 + 26,
            '0'..='9' => c as u32 - '0' as u32 + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            '=' => break,
            c if c.is_whitespace() => continue,
            _ => return None,
        };
        bits = (bits << 6) | value;
        have += 6;
        if have >= 8 {
            have -= 8;
            out.push(((bits >> have) & 0xFF) as u8);
        }
    }
    Some(out)
}

/// The picture a `data:image/…;base64,…` address holds.
fn data_image(uri: &str) -> Option<Image> {
    let rest = uri
        .get(5..)
        .filter(|_| uri.len() > 5 && uri.get(..5).is_some_and(|head| head.eq_ignore_ascii_case("data:")))?;
    let (head, data) = rest.split_once(',')?;
    let mut parts = head.split(';');
    let mime = parts.next()?.trim().to_ascii_lowercase();
    if !parts.any(|p| p.trim().eq_ignore_ascii_case("base64")) {
        return None;
    }
    let format = match mime.as_str() {
        "image/png" => ImageFormat::Png,
        "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        "image/svg+xml" => ImageFormat::Svg,
        "image/bmp" => ImageFormat::Bmp,
        "image/tiff" => ImageFormat::Tiff,
        _ => return None,
    };
    Some(Image::from_bytes(format, base64_decode(data)?))
}

/// The nothing a refused picture shows.
fn blank() -> Arc<Image> {
    static BLANK: OnceLock<Arc<Image>> = OnceLock::new();
    BLANK
        .get_or_init(|| {
            Arc::new(Image::from_bytes(
                ImageFormat::Png,
                base64_decode(BLANK_PNG).unwrap_or_default(),
            ))
        })
        .clone()
}

/// What a picture address in the document shows (`Picture` in MarkdownView.tsx): a file beside the Markdown or
/// elsewhere on its drive or share (one on another could be a server Windows would sign in to, telling it who
/// you are), or one written in the document (`data:`). Pictures from the web aren't fetched for a preview.
pub fn picture_source(
    src: &str,
    folder: &str,
    root: &str,
    written: &Mutex<HashMap<String, Arc<Image>>>,
) -> ImageSource {
    if src.len() > 5 && src.get(..5).is_some_and(|head| head.eq_ignore_ascii_case("data:")) {
        let Ok(mut cache) = written.lock() else {
            return ImageSource::Image(blank());
        };
        let image = cache
            .entry(src.to_string())
            .or_insert_with(|| data_image(src).map(Arc::new).unwrap_or_else(blank))
            .clone();
        return ImageSource::Image(image);
    }
    match linked_path(src, folder, root) {
        Some(path) if volume_of(&path).eq_ignore_ascii_case(volume_of(folder)) => {
            ImageSource::Resource(Resource::Path(Arc::from(PathBuf::from(path))))
        }
        // The web, another drive or share, a script: nothing is loaded.
        _ => ImageSource::Image(blank()),
    }
}

/// The TextView's picture resolver for a document in `folder`.
fn image_source(folder: String, root: String) -> impl Fn(&SharedUri) -> ImageSource + Send + Sync + 'static {
    let written: Mutex<HashMap<String, Arc<Image>>> = Mutex::new(HashMap::new());
    move |uri| picture_source(uri.as_ref(), &folder, &root, &written)
}

// ---- Diagrams and math --------------------------------------------------------------------------------------

/// The parse of the plugins below never changes with what they capture, so the TextView keeps its parsed
/// document when they are rebuilt every frame (`MarkdownExtensions::parser_revision`).
const HOOKS_REVISION: u64 = 0x474c_494e_5401;

/// The TextView's plugins for ```mermaid, ```dot, ```math and `$…$`, waking the view `wake` when a picture is
/// ready.
pub fn hooks(wake: EntityId) -> gpui_kit::base::text::MarkdownExtensions {
    use super::diagram::DiagramBlock;
    use super::math::{MathBlock, MathInline};
    gpui_kit::base::text::MarkdownExtensions::default()
        .parser_revision(HOOKS_REVISION)
        .plugin(DiagramBlock { wake })
        .plugin(MathBlock { wake })
        .plugin(MathInline { wake })
}

// ---- The previewer ----------------------------------------------------------------------------------------

/// What the previewer is showing.
enum Load {
    Waiting,
    Failed(SharedString),
    /// Not text: its bytes.
    Binary(Entity<HexPreview>),
    /// "This file is empty."
    Empty,
    Shown(Shown),
}

/// The document on screen.
struct Shown {
    state: Entity<TextViewState>,
    headings: Arc<Vec<Heading>>,
    /// The read stopped at the byte limit.
    truncated: bool,
}

/// Markdown rendered with the kit's TextView.
pub struct MarkdownPreview {
    path: String,
    source: TextSource,
    load: Load,
    started: Instant,
    generation: u64,
    _read: Task<()>,
}

impl MarkdownPreview {
    /// The previewer for the file at `path`. Reading happens in the background; the view shows its loading state
    /// until then.
    pub fn new(path: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_source(path, core_text_source(), window, cx)
    }

    /// [`MarkdownPreview::new`] reading through `source` (tests pass fakes).
    pub fn with_source(path: String, source: TextSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            path,
            source,
            load: Load::Waiting,
            started: Instant::now(),
            generation: 0,
            _read: Task::ready(()),
        };
        // Pictures still being drawn for a view that is gone are not wanted.
        {
            let id = cx.entity_id();
            cx.on_release(move |_, _| super::svg_image::forget(id)).detach();
        }
        this.read(window, cx);
        this
    }

    /// The file shown.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows another file (the pane's selection moved).
    pub fn set_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.path != path {
            self.path = path;
            self.read(window, cx);
        }
    }

    /// Reads the file again (it changed on disk).
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.read(window, cx);
    }

    /// Scrolls the document a page (Page Down and Page Up in Quick Look). Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        match &self.load {
            Load::Shown(shown) => shown.state.update(cx, |state, cx| {
                let step = state.bounds().size.height * 0.85;
                state.list_state().scroll_by(if down { step } else { -step });
                cx.notify();
                true
            }),
            Load::Binary(hex) => hex.update(cx, |hex, cx| hex.page(down, cx)),
            _ => false,
        }
    }

    /// The text as the view draws it, once it has been parsed.
    pub fn rendered_text(&self, cx: &App) -> Option<String> {
        match &self.load {
            Load::Shown(shown) => Some(shown.state.read(cx).rendered_text().as_str().to_string()),
            _ => None,
        }
    }

    fn read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        super::svg_image::forget(cx.entity_id());
        self.load = Load::Waiting;
        self.started = Instant::now();
        let read = (self.source)(self.path.clone(), None);
        self._read = cx.spawn_in(window, async move |this, cx| {
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

    fn arrived(&mut self, result: Result<TextRead, String>, window: &mut Window, cx: &mut Context<Self>) {
        self.load = match result {
            Err(reason) => Load::Failed(reason.into()),
            Ok(read) if read.binary => {
                let path = self.path.clone();
                Load::Binary(cx.new(|cx| HexPreview::new(path, window, cx)))
            }
            Ok(read) if read.text.trim().is_empty() => Load::Empty,
            Ok(read) => {
                let headings = Arc::new(headings(&read.text));
                let text = read.text;
                Load::Shown(Shown {
                    state: cx.new(|cx| TextViewState::markdown(&text, cx).selectable(true)),
                    headings,
                    truncated: read.truncated,
                })
            }
        };
        cx.notify();
    }

    /// The document's view: pictures beside it, links that open the web outside and files here, headings that
    /// scroll.
    fn document(&self, shown: &Shown, wake: EntityId) -> TextView {
        let folder = parent_path(&self.path);
        // The Git view's repository root isn't known here: the drive's root, for links that start with /.
        let root = format!("{}\\", volume_of(&self.path));
        let state: WeakEntity<TextViewState> = shown.state.downgrade();
        let headings = shown.headings.clone();
        let (link_folder, link_root) = (folder.clone(), root.clone());
        let view = TextView::new(&shown.state);
        let view = view.markdown_extensions(hooks(wake));
        view.selectable(true)
            .scrollable(true)
            .image_source(image_source(folder, root))
            .on_link_click(
                move |url, _, _, cx| match link_action(url.as_ref(), &link_folder, &link_root, &headings) {
                    LinkAction::None => {}
                    LinkAction::Heading(range) => {
                        state
                            .update(cx, |state, cx| {
                                if let Some(rendered) = state.rendered_text().range_for_source(range) {
                                    let _ = state.reveal_range(rendered, cx);
                                }
                            })
                            .ok();
                    }
                    LinkAction::Web(address) => cx.open_url(&address),
                    LinkAction::Path(path) => {
                        if let Some(handle) = cx.try_global::<crate::explorer::ExplorerHandle>() {
                            handle
                                .0
                                .clone()
                                .update(cx, |explorer, cx| explorer.go_to(&path, cx).detach());
                        }
                    }
                },
            )
    }
}

impl Render for MarkdownPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let body = match &self.load {
            Load::Waiting => waiting(self.started, None, cx),
            Load::Failed(reason) => big_icon(&self.path, Some(reason.clone()), cx),
            Load::Binary(hex) => hex.clone().into_any_element(),
            Load::Empty => note_centered("markdown-empty", t!("This file is empty."), cx),
            Load::Shown(shown) => {
                let note = shown.truncated.then(|| {
                    let text = t!("Preview shows the beginning of the file. Open it to see the rest.");
                    div()
                        .id("markdown-shows-start")
                        .role(Role::Status)
                        .aria_label(text.clone())
                        .test_support()
                        .px(space(5.))
                        .pb(space(3.))
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(text)
                });
                col()
                    .size_full()
                    .min_h_0()
                    .child(
                        div()
                            .id("markdown-document")
                            .role(Role::Document)
                            .aria_label(name_of(&self.path))
                            .test_support()
                            .flex_1()
                            .min_h_0()
                            .px(space(5.))
                            .py(space(4.))
                            .child(self.document(shown, cx.entity_id()).size_full()),
                    )
                    .children(note)
                    .into_any_element()
            }
        };
        col()
            .id("markdown-preview")
            .size_full()
            .min_h_0()
            .text_start()
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollDown, _, cx| {
                this.page(true, cx);
            }))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollUp, _, cx| {
                this.page(false, cx);
            }))
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::time::Duration;

    fn read_of(text: &str) -> TextRead {
        TextRead {
            text: text.to_string(),
            truncated: false,
            binary: false,
            encoding: None,
            guessed: false,
        }
    }

    fn fake(read: TextRead) -> TextSource {
        Arc::new(move |_, _| {
            let answer = Ok(TextRead {
                text: read.text.clone(),
                truncated: read.truncated,
                binary: read.binary,
                encoding: read.encoding.clone(),
                guessed: read.guessed,
            });
            Box::pin(async move { answer })
        })
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn open(cx: &mut TestAppContext, path: &str, source: TextSource) -> (Entity<MarkdownPreview>, VisualTestContext) {
        cx.update(testing::init);
        let path = path.to_string();
        let (visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| MarkdownPreview::with_source(path, source, window, cx))
        });
        (view, visual)
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the Markdown never arrived");
    }

    #[test]
    fn anchors_are_made_the_way_github_makes_them() {
        assert_eq!(slug_of("Getting Started"), "getting-started");
        assert_eq!(slug_of("What's new? (v2.0)"), "whats-new-v20");
        assert_eq!(slug_of("The `fd` tool"), "the-fd-tool");
        assert_eq!(slug_of("**Bold** and [a link](https://example.com)"), "bold-and-a-link");
        assert_eq!(slug_of("snake_case stays"), "snake_case-stays");
        assert_eq!(slug_of("تقرير الربع الأول"), "تقرير-الربع-الأول");
        assert_eq!(slug_of("Ünïcode Ñame"), "ünïcode-ñame");
    }

    #[test]
    fn headings_are_found_outside_code_with_their_places() {
        let source = "# Title\n\nText\n\n## Setup ##\n\n```sh\n# not a heading\n```\n\nSetext one\n==========\n\n### Setup\n\n#hashtag\n";
        let found = headings(source);
        let slugs: Vec<_> = found.iter().map(|h| h.slug.as_str()).collect();
        assert_eq!(slugs, ["title", "setup", "setext-one", "setup-1"]);
        assert_eq!(&source[found[0].range.clone()], "Title");
        assert_eq!(&source[found[1].range.clone()], "Setup");
        assert_eq!(&source[found[2].range.clone()], "Setext one");
        assert_eq!(&source[found[3].range.clone()], "Setup");
        // Arabic headings keep their text and their place (a byte range, not a character one).
        let arabic = "مقدمة\n\n# عنوان رئيسي\n";
        let found = headings(arabic);
        assert_eq!(found.len(), 1);
        assert_eq!(&arabic[found[0].range.clone()], "عنوان رئيسي");
        assert_eq!(found[0].slug, "عنوان-رئيسي");
    }

    #[test]
    fn links_go_where_the_typescript_sent_them() {
        let h = headings("# My Title\n");
        let folder = "C:\\docs\\notes";
        let root = "C:\\";
        assert_eq!(
            link_action("#my-title", folder, root, &h),
            LinkAction::Heading(h[0].range.clone())
        );
        assert_eq!(
            link_action("#user-content-my-title", folder, root, &h),
            LinkAction::Heading(h[0].range.clone())
        );
        assert_eq!(
            link_action("#My-Title", folder, root, &h),
            LinkAction::Heading(h[0].range.clone())
        );
        assert_eq!(link_action("#nowhere", folder, root, &h), LinkAction::None);
        assert_eq!(
            link_action("https://example.com/x", folder, root, &h),
            LinkAction::Web("https://example.com/x".into())
        );
        assert_eq!(
            link_action("mailto:a@b.c", folder, root, &h),
            LinkAction::Web("mailto:a@b.c".into())
        );
        assert_eq!(link_action("javascript:alert(1)", folder, root, &h), LinkAction::None);
        assert_eq!(link_action("file:///C:/Windows", folder, root, &h), LinkAction::None);
        assert_eq!(link_action("", folder, root, &h), LinkAction::None);
        assert_eq!(
            link_action("other.md", folder, root, &h),
            LinkAction::Path("C:\\docs\\notes\\other.md".into())
        );
        assert_eq!(
            link_action("../img/a%20b.png", folder, root, &h),
            LinkAction::Path("C:\\docs\\img\\a b.png".into())
        );
        assert_eq!(
            link_action("/top.md", folder, root, &h),
            LinkAction::Path("C:\\top.md".into())
        );
        assert_eq!(percent_decode("%D8%B9%D9%86%D9%88%D8%A7%D9%86"), "عنوان");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn base64_decodes_what_pictures_are_written_in() {
        assert_eq!(base64_decode("aGVsbG8=").as_deref(), Some(&b"hello"[..]));
        assert_eq!(base64_decode("aGVsbG8").as_deref(), Some(&b"hello"[..]));
        assert_eq!(base64_decode("aGVs bG8=").as_deref(), Some(&b"hello"[..]));
        assert_eq!(base64_decode("").as_deref(), Some(&b""[..]));
        assert_eq!(base64_decode("a*b"), None);
        // The blank picture is a real PNG.
        let png = base64_decode(BLANK_PNG).unwrap_or_default();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    }

    #[test]
    fn pictures_load_from_beside_the_file_and_written_in_it_but_not_from_the_web() {
        let cache = Mutex::new(HashMap::new());
        let folder = "C:\\docs";
        let root = "C:\\";
        let is_file = |source: ImageSource| matches!(source, ImageSource::Resource(Resource::Path(_)));
        assert!(is_file(picture_source("images/a.png", folder, root, &cache)));
        assert!(is_file(picture_source("../other/a.png", folder, root, &cache)));
        assert!(is_file(picture_source("/shared/a.png", folder, root, &cache)));
        // The web, another drive and a share are blank (the blank picture, never a fetch).
        for src in [
            "https://example.com/a.png",
            "http://example.com/a.png",
            "D:\\pics\\a.png",
            "ftp://example.com/a.png",
        ] {
            assert!(
                matches!(picture_source(src, folder, root, &cache), ImageSource::Image(_)),
                "{src}"
            );
        }
        // A document on a share loads what is on that share; a picture on a drive is another volume.
        let share = ("\\\\srv\\share\\docs", "\\\\srv\\share\\");
        assert!(is_file(picture_source("pic.png", share.0, share.1, &cache)));
        assert!(matches!(
            picture_source("C:\\pics\\a.png", share.0, share.1, &cache),
            ImageSource::Image(_)
        ));
        // A picture written in the document decodes once and is kept.
        let data = format!("data:image/png;base64,{BLANK_PNG}");
        let (ImageSource::Image(first), ImageSource::Image(second)) = (
            picture_source(&data, folder, root, &cache),
            picture_source(&data, folder, root, &cache),
        ) else {
            panic!("a data picture is an image");
        };
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(cache.lock().map(|c| c.len()).unwrap_or(0), 1);
        assert!(data_image("data:text/html;base64,PGI+").is_none());
        assert!(data_image("data:image/png,not-base64").is_none());
    }

    #[gpui_kit::test]
    fn the_document_renders_and_is_named_for_the_file(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(
            cx,
            "C:\\docs\\README.md",
            fake(read_of("# Title\n\nSome **bold** text and a [link](https://example.com).\n\n```rust\nfn main() {}\n```\n\n```mermaid\ngraph TD;\n```\n")),
        );
        assert!(has(&mut visual, "Loading…"));
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.load, Load::Shown(_)))
        });
        assert!(has(&mut visual, "README.md"));
        // The parse lands in the background; the text the view draws is the document without its marks.
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, cx| {
                v.rendered_text(cx).is_some_and(|text| text.contains("Some bold text"))
            })
        });
        let drawn = view.read_with(&visual, |v, cx| v.rendered_text(cx)).unwrap_or_default();
        assert!(drawn.contains("Title") && drawn.contains("and a link."), "{drawn}");
        // Code is drawn as code, the Mermaid diagram too (as code, until it can be drawn).
        assert!(drawn.contains("fn main() {}") && drawn.contains("graph TD;"), "{drawn}");
        assert!(!drawn.contains("**") && !drawn.contains("```"), "no marks: {drawn}");
        // A page down on a short document goes nowhere harmful.
        visual.update(|_, cx| {
            view.update(cx, |v, cx| {
                v.page(true, cx);
            })
        });
        assert!(!has(
            &mut visual,
            "Preview shows the beginning of the file. Open it to see the rest."
        ));
    }

    #[gpui_kit::test]
    fn arabic_markdown_renders_in_the_arabic_interface(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (view, mut visual) = open(
            cx,
            "C:\\مستندات\\تقرير.md",
            fake(read_of(
                "# تقرير الربع الأول\n\nمرحبا **بالعالم** و [رابط](#تقرير-الربع-الأول).\n",
            )),
        );
        // After `open`: starting the app's services puts the language back to English.
        testing::set_rtl(true);
        view.update(&mut visual, |_, cx| cx.notify());
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.load, Load::Shown(_)))
        });
        assert!(has(&mut visual, "تقرير.md"));
        let anchors = view.read_with(&visual, |v, _| match &v.load {
            Load::Shown(shown) => shown.headings.iter().map(|h| h.slug.clone()).collect::<Vec<_>>(),
            _ => Vec::new(),
        });
        assert_eq!(anchors, ["تقرير-الربع-الأول"]);
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn truncated_empty_failed_and_binary_files_say_what_they_are(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut cut = read_of("# Long\n");
        cut.truncated = true;
        let (view, mut visual) = open(cx, "C:\\docs\\long.md", fake(cut));
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.load, Load::Shown(_)))
        });
        assert!(has(
            &mut visual,
            "Preview shows the beginning of the file. Open it to see the rest."
        ));

        // Only white space is an empty file.
        let (view, mut visual) = open(cx, "C:\\docs\\empty.md", fake(read_of("  \n\n")));
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.load, Load::Empty))
        });
        assert!(has(&mut visual, "This file is empty."));

        let failing: TextSource =
            Arc::new(|_, _| Box::pin(async { Err("The system cannot find the file specified.".to_string()) }));
        let (view, mut visual) = open(cx, "C:\\docs\\gone.md", failing);
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.load, Load::Failed(_)))
        });
        assert!(has(&mut visual, "The system cannot find the file specified."));

        let mut binary = read_of("");
        binary.binary = true;
        let (view, mut visual) = open(cx, "C:\\docs\\not.md", fake(binary));
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.load, Load::Binary(_)))
        });
    }

    // ---- Each block reads in its own direction ----------------------------------------------------------------

    /// Where a document's text is drawn. The whole text is highlighted (the TextView paints a highlight under
    /// the glyphs it lays out, so its boxes are the glyphs' own, row by row, aligned as they are painted), and
    /// the list markers and code are underlined by the document's style.
    mod direction {
        use super::*;
        use gpui_kit::base::text::{RangeHighlight, TextViewDefaults, TextViewStyle};
        use gpui_kit::{point, px, red, Background, Bounds, Pixels, StyleRefinement, Styled as _};

        /// The slack of comparing positions the layout rounded.
        const SLACK: f32 = 1.5;

        /// Opens the document `text` at `C:\docs\README.md` and waits until it is parsed and drawn, with its
        /// text highlighted and its list markers and code underlined.
        fn show(cx: &mut TestAppContext, text: &str, rtl: bool) -> (Entity<MarkdownPreview>, VisualTestContext) {
            let (view, mut visual) = open(cx, "C:\\docs\\README.md", fake(read_of(text)));
            // After `open`: starting the app's services puts the language back to English.
            testing::set_rtl(rtl);
            view.update(&mut visual, |_, cx| cx.notify());
            visual.update(|_, cx| {
                let style = TextViewStyle::from_theme(&gpui_kit::base::Theme::global(cx))
                    .with_list_style(StyleRefinement::default().underline())
                    .with_code_block(StyleRefinement::default().underline());
                cx.set_global(TextViewDefaults::new().with_style(style));
            });
            settle(&mut visual, |cx| {
                view.read_with(cx, |v, cx| v.rendered_text(cx).is_some_and(|t| !t.trim().is_empty()))
            });
            view.update(&mut visual, |v, cx| {
                if let Load::Shown(shown) = &v.load {
                    shown.state.update(cx, |state, cx| {
                        let len = state.rendered_text().as_str().len();
                        let all = RangeHighlight::new(0..len, red());
                        assert!(state.set_range_highlights([all], cx).is_ok());
                    });
                }
            });
            // The frames that draw the parsed document and then its measured rows.
            for _ in 0..3 {
                has(&mut visual, "README.md");
                visual.run_until_parked();
            }
            (view, visual)
        }

        /// The text area: the document's box without its padding (`space(5.)` = 20 px a side).
        fn area(cx: &mut VisualTestContext) -> Bounds<Pixels> {
            let document = cx.update(|window, cx| {
                window.render_frame(cx);
                window.find_by_label("README.md").bounds()
            });
            Bounds::from_corners(
                point(document.left() + px(20.), document.top()),
                point(document.right() - px(20.), document.bottom()),
            )
        }

        /// `found` joined by row, top to bottom.
        fn joined(mut found: Vec<Bounds<Pixels>>) -> Vec<Bounds<Pixels>> {
            found.sort_by(|a, b| a.top().partial_cmp(&b.top()).unwrap_or(std::cmp::Ordering::Equal));
            let mut rows: Vec<Bounds<Pixels>> = Vec::new();
            for next in found {
                match rows.last_mut() {
                    Some(row) if (row.top() - next.top()).abs() < px(3.) => {
                        *row = Bounds::from_corners(
                            point(row.left().min(next.left()), row.top().min(next.top())),
                            point(row.right().max(next.right()), row.bottom().max(next.bottom())),
                        );
                    }
                    _ => rows.push(next),
                }
            }
            rows
        }

        /// The rows of text, as the glyphs fill them.
        fn rows(cx: &mut VisualTestContext) -> Vec<Bounds<Pixels>> {
            joined(cx.update(|window, cx| {
                window.render_frame(cx);
                let scale = window.scale_factor();
                window
                    .painted_quads()
                    .into_iter()
                    .filter(|quad| quad.background == Background::from(red()))
                    .map(|quad| quad.bounds.map(|value| px(value.0 / scale)))
                    .collect()
            }))
        }

        /// The underlined stretches: list markers and code.
        fn underlined(cx: &mut VisualTestContext) -> Vec<Bounds<Pixels>> {
            joined(cx.update(|window, cx| {
                window.render_frame(cx);
                let scale = window.scale_factor();
                window
                    .painted_underlines()
                    .into_iter()
                    .map(|underline| underline.bounds.map(|value| px(value.0 / scale)))
                    .collect()
            }))
        }

        fn ends_at_right(row: Bounds<Pixels>, area: Bounds<Pixels>) -> bool {
            (area.right() - row.right()).abs() <= px(SLACK)
        }

        fn starts_at_left(row: Bounds<Pixels>, area: Bounds<Pixels>) -> bool {
            (row.left() - area.left()).abs() <= px(SLACK)
        }

        const ARABIC_WORDS: &str = "كلمة عربية طويلة تملأ الأسطر";

        #[gpui_kit::test]
        fn an_arabic_paragraph_ends_at_the_right_edge_in_an_english_window(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            // A short Arabic paragraph, one that wraps over several rows, and an English one for the left edge.
            let long = [ARABIC_WORDS; 40].join(" ");
            let text = format!("مرحبا بالعالم\n\n{long}\n\nHello world\n");
            let (_view, mut visual) = show(cx, &text, false);
            let area = area(&mut visual);
            let rows = rows(&mut visual);
            // The short row hugs the right edge: it ends at the area's right and starts well inside.
            let short = rows[0];
            assert!(ends_at_right(short, area), "{short:?} in {area:?}");
            assert!(short.left() > area.left() + px(300.), "{short:?} in {area:?}");
            // The wrapped paragraph: every row ends at the right edge, the last of them short.
            let wrapped = &rows[1..rows.len() - 1];
            assert!(wrapped.len() >= 2, "it wraps: {rows:?}");
            for row in wrapped {
                assert!(ends_at_right(*row, area), "{row:?} in {area:?}");
            }
            let last = wrapped[wrapped.len() - 1];
            assert!(
                last.size.width < wrapped[0].size.width,
                "the last row is the short one: {wrapped:?}"
            );
            // The English paragraph starts at the left edge, as it always did.
            let english = rows[rows.len() - 1];
            assert!(starts_at_left(english, area), "{english:?} in {area:?}");
            assert!(english.right() < area.left() + px(300.), "{english:?} in {area:?}");
        }

        #[gpui_kit::test]
        fn an_english_paragraph_starts_at_the_left_edge_in_an_arabic_window(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            let text = "Hello world\n\nمرحبا بالعالم\n\n1234 ...\n";
            let (_view, mut visual) = show(cx, text, true);
            let area = area(&mut visual);
            let rows = rows(&mut visual);
            assert_eq!(rows.len(), 3, "{rows:?}");
            // English reads left to right: at the left, though the window starts at the right.
            assert!(starts_at_left(rows[0], area), "{:?} in {area:?}", rows[0]);
            assert!(rows[0].right() < area.left() + px(300.), "{:?} in {area:?}", rows[0]);
            // Arabic is at the right, and a paragraph with no letters takes the window's direction.
            assert!(ends_at_right(rows[1], area), "{:?} in {area:?}", rows[1]);
            assert!(ends_at_right(rows[2], area), "{:?} in {area:?}", rows[2]);
            testing::set_rtl(false);
        }

        #[gpui_kit::test]
        fn english_in_an_english_window_is_laid_out_as_it_was(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            let text = "# Title\n\nSome text, 1234.\n\n- one\n- two\n\n> quoted\n";
            let (_view, mut visual) = show(cx, text, false);
            let area = area(&mut visual);
            for row in rows(&mut visual) {
                assert!(row.left() < area.left() + px(60.), "{row:?} in {area:?}");
            }
            // The markers are on the left, against the items, and the quote's bar on its left.
            for marker in underlined(&mut visual) {
                assert!(marker.left() < area.left() + px(40.), "{marker:?} in {area:?}");
            }
            assert_eq!(quote_bars(&mut visual), [(true, false)]);
        }

        #[gpui_kit::test]
        fn a_heading_takes_its_own_direction_too(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            let text = "# عنوان عربي\n\n## English title\n";
            let (_view, mut visual) = show(cx, text, false);
            let area = area(&mut visual);
            let rows = rows(&mut visual);
            assert_eq!(rows.len(), 2, "{rows:?}");
            assert!(ends_at_right(rows[0], area), "{:?} in {area:?}", rows[0]);
            assert!(starts_at_left(rows[1], area), "{:?} in {area:?}", rows[1]);
        }

        #[gpui_kit::test]
        fn a_paragraph_with_code_in_it_is_set_from_the_right_too(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            // Inline code makes the paragraph a flow of fragments, which are placed one by one.
            let long = [ARABIC_WORDS; 12].join(" ");
            let text = format!("مرحبا `code` بالعالم\n\nHello `code` world\n\n{long} `code` {long}\n");
            let (_view, mut visual) = show(cx, &text, false);
            let area = area(&mut visual);
            let rows = rows(&mut visual);
            assert!(rows.len() >= 4, "{rows:?}");
            assert!(ends_at_right(rows[0], area), "{:?} in {area:?}", rows[0]);
            assert!(rows[0].left() > area.left() + px(300.), "{:?} in {area:?}", rows[0]);
            assert!(starts_at_left(rows[1], area), "{:?} in {area:?}", rows[1]);
            // A long one wraps: its first row spans the area, from its right edge.
            assert!(ends_at_right(rows[2], area), "{:?} in {area:?}", rows[2]);
        }

        #[gpui_kit::test]
        fn a_mixed_list_puts_each_marker_on_its_items_start_side(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            let text = "- English item\n- عنصر عربي\n- Another item\n\n1. First\n2. بند ثان\n";
            let (_view, mut visual) = show(cx, text, false);
            let area = area(&mut visual);
            // The markers are the only underlined text: five rows, one mark each.
            let markers = underlined(&mut visual);
            let texts = rows(&mut visual);
            assert_eq!((markers.len(), texts.len()), (5, 5), "{markers:?} {texts:?}");
            for (ix, rtl) in [(0, false), (1, true), (2, false), (3, false), (4, true)] {
                let (marker, text) = (markers[ix], texts[ix]);
                if rtl {
                    // At the right edge, and the item's text is to its left.
                    assert!(
                        ends_at_right(text, area) || text.right() > area.right() - px(60.),
                        "item {ix}: {text:?}"
                    );
                    assert!(
                        marker.left() > area.right() - px(40.),
                        "item {ix}: {marker:?} in {area:?}"
                    );
                    assert!(marker.left() > text.right(), "item {ix}: {marker:?} before {text:?}");
                } else {
                    assert!(text.left() < area.left() + px(60.), "item {ix}: {text:?}");
                    assert!(
                        marker.left() < area.left() + px(40.),
                        "item {ix}: {marker:?} in {area:?}"
                    );
                    assert!(marker.right() < text.left(), "item {ix}: {marker:?} after {text:?}");
                }
            }
        }

        #[gpui_kit::test]
        fn a_mixed_list_in_an_arabic_window_mirrors_the_same_way(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            let text = "- عنصر عربي\n- English item\n- بند آخر\n";
            let (_view, mut visual) = show(cx, text, true);
            let area = area(&mut visual);
            let markers = underlined(&mut visual);
            let texts = rows(&mut visual);
            assert_eq!((markers.len(), texts.len()), (3, 3), "{markers:?} {texts:?}");
            for (ix, rtl) in [(0, true), (1, false), (2, true)] {
                if rtl {
                    assert!(
                        markers[ix].left() > area.right() - px(40.),
                        "{:?} in {area:?}",
                        markers[ix]
                    );
                    assert!(
                        markers[ix].left() > texts[ix].right(),
                        "{:?} before {:?}",
                        markers[ix],
                        texts[ix]
                    );
                } else {
                    assert!(
                        markers[ix].left() < area.left() + px(40.),
                        "{:?} in {area:?}",
                        markers[ix]
                    );
                    assert!(
                        markers[ix].right() < texts[ix].left(),
                        "{:?} after {:?}",
                        markers[ix],
                        texts[ix]
                    );
                }
            }
            testing::set_rtl(false);
        }

        #[gpui_kit::test]
        fn a_nested_list_indents_from_its_start_side(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            let text = "- عنصر\n  - فرعي\n- English\n  - nested\n";
            let (_view, mut visual) = show(cx, text, false);
            let area = area(&mut visual);
            let texts = rows(&mut visual);
            assert_eq!(texts.len(), 4, "{texts:?}");
            // The sub-item of an Arabic item is indented from the right: it ends before the item above does.
            assert!(texts[0].right() > area.right() - px(60.), "{:?}", texts[0]);
            assert!(
                texts[1].right() < texts[0].right() - px(10.),
                "{:?} under {:?}",
                texts[1],
                texts[0]
            );
            // And the English one from the left.
            assert!(
                texts[3].left() > texts[2].left() + px(10.),
                "{:?} under {:?}",
                texts[3],
                texts[2]
            );
        }

        /// The vertical bars the quotes drew: (left, right) of every quote, top to bottom, each once.
        fn quote_bars(cx: &mut VisualTestContext) -> Vec<(bool, bool)> {
            let mut bars: Vec<(f32, bool, bool)> = cx.update(|window, cx| {
                window.render_frame(cx);
                window
                    .painted_quads()
                    .into_iter()
                    .filter(|quad| quad.border_widths.left.0 > 0. || quad.border_widths.right.0 > 0.)
                    .map(|quad| {
                        (
                            quad.bounds.origin.y.0,
                            quad.border_widths.left.0 > 0.,
                            quad.border_widths.right.0 > 0.,
                        )
                    })
                    .collect()
            });
            bars.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            bars.dedup();
            bars.into_iter().map(|(_, left, right)| (left, right)).collect()
        }

        #[gpui_kit::test]
        fn a_quote_has_its_bar_on_the_side_it_starts_at(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            let text = "> A quotation\n\n> اقتباس عربي\n";
            for rtl in [false, true] {
                let (_view, mut visual) = show(cx, text, rtl);
                let bars = quote_bars(&mut visual);
                assert_eq!(bars, [(true, false), (false, true)], "rtl window: {rtl}");
            }
            testing::set_rtl(false);
        }

        #[gpui_kit::test]
        fn code_stays_left_to_right_in_an_arabic_window(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            // The code block's text is underlined (see `show`), so its rows show where it is drawn.
            let text = "مرحبا\n\n```\nlet x = 1;\n```\n";
            let (view, mut visual) = show(cx, text, true);
            let drawn = view.read_with(&visual, |v, cx| v.rendered_text(cx)).unwrap_or_default();
            assert!(drawn.contains("let x = 1;"), "{drawn}");
            let area = area(&mut visual);
            let code = underlined(&mut visual);
            assert_eq!(code.len(), 1, "{code:?}");
            assert!(code[0].left() < area.left() + px(40.), "{:?} in {area:?}", code[0]);
            assert!(code[0].right() < area.right() - px(300.), "{:?} in {area:?}", code[0]);
            // The Arabic line above it is at the right.
            assert!(ends_at_right(rows(&mut visual)[0], area));
            testing::set_rtl(false);
        }
    }

    // ---- Diagrams and math (feature `diagrams`) ---------------------------------------------------------------

    mod drawn {
        use super::*;
        use crate::components::preview::{diagram, math, svg_image};
        use gpui_kit::Role;

        /// (role, label, description, height) of every element the last frame drew that has a label.
        fn elements(cx: &mut VisualTestContext) -> Vec<(Option<Role>, String, Option<String>, f32)> {
            cx.update(|window, cx| {
                window.render_frame(cx);
                window
                    .elements()
                    .into_iter()
                    .filter_map(|e| {
                        let label = e.label()?.to_string();
                        Some((
                            e.role(),
                            label,
                            e.description().map(str::to_string),
                            f32::from(e.bounds().size.height),
                        ))
                    })
                    .collect()
            })
        }

        fn with_label(
            cx: &mut VisualTestContext,
            role: Role,
            label: impl Fn(&str) -> bool,
        ) -> Vec<(String, Option<String>, f32)> {
            elements(cx)
                .into_iter()
                .filter(|(r, l, _, _)| *r == Some(role) && label(l))
                .map(|(_, l, d, h)| (l, d, h))
                .collect()
        }

        fn wait(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
            for _ in 0..1200 {
                cx.run_until_parked();
                if done(cx) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("never happened: {what}: {:?}", elements(cx));
        }

        fn show(cx: &mut TestAppContext, text: &str) -> (Entity<MarkdownPreview>, VisualTestContext) {
            svg_image::clear();
            let (view, mut visual) = open(cx, "C:\\docs\\README.md", fake(read_of(text)));
            settle(&mut visual, |cx| {
                view.read_with(cx, |v, _| matches!(v.load, Load::Shown(_)))
            });
            (view, visual)
        }

        const FLOW: &str = "flowchart TD\n  A[Start] --> B{Is it ok?}\n  B -->|yes| C[Done]\n  B -->|no| D[Again]";
        const DOT: &str = "digraph G { rankdir=LR; a -> b [label=\"edge\"]; b -> c; c -> a; }";

        #[gpui_kit::test]
        fn mermaid_and_dot_blocks_are_drawn_as_pictures(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            testing::set_rtl(false);
            let text = format!(
                "# Diagrams\n\nBefore.\n\n```mermaid\n{FLOW}\n```\n\nBetween.\n\n```dot\n{DOT}\n```\n\nAfter.\n"
            );
            let (view, mut visual) = show(cx, &text);
            wait(&mut visual, "both pictures", |cx| {
                !with_label(cx, Role::Image, |l| l == "A Mermaid diagram").is_empty()
                    && !with_label(cx, Role::Image, |l| l == "A Graphviz graph").is_empty()
            });
            // The pictures have a size, so the document made room for them.
            for (name, _, height) in with_label(&mut visual, Role::Image, |l| l.starts_with("A ")) {
                assert!(height > 40., "{name} is {height} high");
            }
            // The words around them are still the document's, and the sources are still its text (copyable).
            let drawn = view.read_with(&visual, |v, cx| v.rendered_text(cx)).unwrap_or_default();
            assert!(
                drawn.contains("Before.") && drawn.contains("Between.") && drawn.contains("After."),
                "{drawn}"
            );
            assert!(drawn.contains("flowchart TD") && drawn.contains("digraph G"), "{drawn}");
            // Both are in the cache, as pictures.
            let dark = false;
            assert!(
                svg_image::peek(&diagram::key_of(diagram::Language::Mermaid, FLOW, dark)).is_some_and(|o| o.is_ok())
            );
            assert!(svg_image::peek(&diagram::key_of(diagram::Language::Dot, DOT, dark)).is_some_and(|o| o.is_ok()));
        }

        #[gpui_kit::test]
        fn a_diagram_that_cannot_be_drawn_says_why_and_shows_its_source(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            testing::set_rtl(false);
            let text = "```mermaid\nthis is not a diagram at all ->->\n```\n\n```dot\ndigraph { a -> }\n```\n";
            let (_view, mut visual) = show(cx, text);
            wait(&mut visual, "both banners", |cx| {
                with_label(cx, Role::Status, |l| l.starts_with("Not a diagram Mermaid can draw: ")).len() == 1
                    && with_label(cx, Role::Status, |l| l.starts_with("Not a graph Graphviz can draw: ")).len() == 1
            });
            // Each banner has its diagnostic, and the source is shown below it.
            let sources = with_label(&mut visual, Role::Group, |l| l == "Source");
            assert_eq!(sources.len(), 2, "{:?}", elements(&mut visual));
            // No empty box: nothing is labelled as a picture.
            assert!(with_label(&mut visual, Role::Image, |_| true).is_empty());
        }

        #[gpui_kit::test]
        fn formulas_flow_with_the_text_and_display_formulas_are_blocks(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            testing::set_rtl(false);
            let text = "The area is $x^2 + y^2$ here.\n\n$$\n\\sum_{i=1}^{n} i\n$$\n\n```math\n\\frac{a}{b}\n```\n\nPrices: $5 and $10.\n";
            let (view, mut visual) = show(cx, text);
            wait(&mut visual, "three formulas", |cx| {
                let math = with_label(cx, Role::Math, |_| true);
                math.len() == 3 && svg_image::held().0 >= 3
            });
            let math: Vec<String> = with_label(&mut visual, Role::Math, |_| true)
                .into_iter()
                .map(|(l, _, _)| l)
                .collect();
            assert!(math.contains(&"x^2 + y^2".to_string()), "{math:?}");
            assert!(math.contains(&"\\sum_{i=1}^{n} i".to_string()), "{math:?}");
            assert!(math.contains(&"\\frac{a}{b}".to_string()), "{math:?}");
            // Two prices are not a formula: their dollars are text.
            let drawn = view.read_with(&visual, |v, cx| v.rendered_text(cx)).unwrap_or_default();
            assert!(drawn.contains("Prices: $5 and $10."), "{drawn}");
            // The text around the inline one is still readable text.
            assert!(drawn.contains("The area is") && drawn.contains("here."), "{drawn}");
            // They were made (pictures in the cache), not left as their LaTeX.
            assert!(svg_image::held().0 >= 3);
            assert!(svg_image::peek(&math::key_of("\\frac{a}{b}", true, 16. * 1.15, false)).is_some_and(|o| o.is_ok()));
        }

        #[gpui_kit::test]
        fn a_formula_that_does_not_parse_shows_its_latex_with_the_reason(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            testing::set_rtl(false);
            let (_view, mut visual) = show(cx, "Broken: $\\frac{1}{$ and fine: $x$.\n\n$$\n\\notacommand{x}\n$$\n");
            wait(&mut visual, "the failures", |cx| {
                with_label(cx, Role::Math, |l| l == "\\frac{1}{")
                    .first()
                    .is_some_and(|(_, d, _)| d.is_some())
                    && with_label(cx, Role::Math, |l| l == "\\notacommand{x}")
                        .first()
                        .is_some_and(|(_, d, _)| d.is_some())
            });
            // The one that parses has no reason attached.
            let fine = with_label(&mut visual, Role::Math, |l| l == "x");
            assert!(fine.iter().all(|(_, description, _)| description.is_none()), "{fine:?}");
        }

        #[gpui_kit::test]
        fn arabic_text_labels_and_formulas_in_an_arabic_window(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            testing::set_rtl(true);
            let flow = "flowchart LR\n  A[مرحبا] --> B[بالعالم أهلا وسهلا]";
            let text = format!(
                "# عنوان\n\nالمعادلة $x^2$ هنا.\n\n```mermaid\n{flow}\n```\n\nوهذه معادلة بحروف: $\\text{{مرحبا}}$\n"
            );
            let (view, mut visual) = show(cx, &text);
            wait(&mut visual, "the diagram", |cx| {
                !with_label(cx, Role::Image, |l| l == "A Mermaid diagram").is_empty()
            });
            let svg = diagram::svg_of(diagram::Language::Mermaid, flow, false, None).expect("it draws");
            // The Arabic label is in the picture, and its line was placed without a `dy` (resvg misplaces the first
            // letter of a right-to-left run that has one).
            assert!(svg.contains("مرحبا"));
            let at = svg.find("مرحبا").unwrap_or(0);
            let tag_start = svg[..at].rfind("<tspan").unwrap_or(0);
            assert!(!svg[tag_start..at].contains("dy="), "{}", &svg[tag_start..at]);
            // A formula with Arabic letters inside is shown as its LaTeX, and says why.
            wait(&mut visual, "the Arabic formula", |cx| {
                with_label(cx, Role::Math, |l| l.contains("مرحبا"))
                    .first()
                    .is_some_and(|(_, d, _)| d.as_deref().is_some_and(|d| d.contains("Arabic")))
            });
            let drawn = view.read_with(&visual, |v, cx| v.rendered_text(cx)).unwrap_or_default();
            assert!(drawn.contains("عنوان") && drawn.contains("المعادلة"), "{drawn}");
            testing::set_rtl(false);
        }

        #[gpui_kit::test]
        fn the_same_diagram_is_drawn_once(cx: &mut TestAppContext) {
            let _guard = testing::lock();
            testing::set_rtl(false);
            let text = format!("```mermaid\n{FLOW}\n```\n\nAgain:\n\n```mermaid\n{FLOW}\n```\n");
            let (_view, mut visual) = show(cx, &text);
            wait(&mut visual, "both pictures", |cx| {
                with_label(cx, Role::Image, |l| l == "A Mermaid diagram").len() == 2
            });
            assert_eq!(svg_image::held().0, 1);
        }
    }
}
