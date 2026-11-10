//! Typst documents typeset (`TypstView` in TypstView.tsx).
//!
//! The TypeScript asked the `typst` tool for a PDF of the `.typ` file (docconvert.rs's `typst_preview`, kept in
//! Glint's cache) and handed it to the PDF preview. Typst is linked in now ([`crate::typst_engine`]), so the pages
//! are drawn straight from the laid out document and no PDF is made for a preview.
//!
//! | TypeScript                                        | Here                                                          |
//! |---------------------------------------------------|---------------------------------------------------------------|
//! | `TypstView`, `invoke("typst_preview")`            | [`TypstView`]: compiles on a thread of its own ([`Typesetter`]) |
//! | `useEffect(..., [entry.path, entry.modified])`    | [`TypstView::set_entry`] (the frame's entry changed) and a slow check of the file and what it read ([`WATCH`]) |
//! | `<ToolNeeded id="typst">`                         | gone: nothing to download                                     |
//! | "Typesetting…" with a spinner                     | the same words, in [`Waiting`]'s way (a spinner after 300 ms) |
//! | "Typst couldn't typeset it" and the error text    | the same words, then each message with its file, line and column and the line itself ([`Diagnostic`]) |
//! | the PDF preview's pages, zoom, page box           | pages in one scrolling column ([`pdf::Layout`](super::pdf::Layout)), zoom, fit width, previous and next page |
//!
//! # How a page gets on screen
//!
//! Like the PDF view's: a worker thread (a big stack: Typst recurses deeply) compiles, tells the view the pages'
//! sizes, then draws the pages the view asks for, nearest first, replacing the last list. The view keeps the
//! pages near the box drawn at the size they are shown (times the window's scale, kept under
//! [`MAX_PAGE_PIXELS`](super::pdf::MAX_PAGE_PIXELS)) and lets go of the rest ([`CACHE_BUDGET`]). The UI thread
//! never compiles, never draws a page and never reads the disk.
//!
//! # Changes on disk
//!
//! When the file, or anything it read (an imported `.typ`, a picture, a data file), is saved, the document is
//! typeset again and the pages replace the old ones where the person was reading (the old pages stay until the new
//! ones are ready; a failed compile shows its errors instead). The check is a stat of those files every
//! [`WATCH`], off the UI thread, and only while the view exists.
//!
//! # Wiring (for the preview pane)
//!
//! - **What it replaces:** the `entry.ext == "typ"` arm of `choose.rs` (`Choice::later(Body::Text, "typst")`): a new
//!   `Body::Typst` for a readable `.typ` on disk (not in an archive); with `View::Source` chosen the arm stays
//!   `Body::Text`. `views()` gets the TypeScript's two buttons for it: `View::File` labelled "Typeset" (title "The
//!   document as Typst typesets it") and `View::Source` labelled "Source" (title "The Typst source as written").
//!   The frame's body wrapper scrolls vertically; this view fills it (`size_full`) and scrolls itself, as the PDF
//!   view does. `EntryView::build` gets `Body::Typst => cx.new(|cx| TypstView::new(entry.clone(), window, cx)).into()`.
//! - **Make it:** `cx.new(|cx| TypstView::new(entry.clone(), window, cx))`. When the frame gets a newer entry for the
//!   same path (the list refreshed), call `view.update(cx, |v, cx| v.set_entry(entry, window, cx))`; the view also
//!   notices a save by itself within [`WATCH`], so this only makes it quicker.
//! - **Quick Look:** `page(down, cx)` scrolls a page (0.85 of the box) and returns whether it moved; add `TypstView`
//!   to `EntryView::page_body`'s list. Give it the keyboard with `window.focus(&view.focus_handle(cx), cx)`.
//! - **Warm-up:** `typst_engine::warm_fonts()` at start-up (a background thread) saves the first document the
//!   wait for the font scan.
//! - **Icons:** `TriangleAlert`, `ChevronUp`, `ChevronDown`, `ZoomIn`, `ZoomOut`, `MoveHorizontal`, `RefreshCw`;
//!   add them to the list in kit.rs's icon test (the test here checks them for now).
//! - **New words** (no Arabic in the catalog yet, English shows): "Typeset again", "Typst's messages",
//!   "Typesetting again…", "{n} warnings", "1 warning", "Warnings", "Too big to typeset here: open it to read it.",
//!   "This document has no pages.", "Typst stopped unexpectedly on this document", "Hint: {hint}", "and {n} more",
//!   "Typst tools", "Typst's warnings about the document", "Typeset the document again (F5)".
//!
//! # Not ported
//!
//! - **Text selection, copy and find on a page.** A page is a picture (as in the PDF view). The file's Source view
//!   is the text; Copy of a page's text is not offered.
//! - **A caret under the error's column** when the line has right to left text (the picture of where would be
//!   wrong once the line is reordered): the line and "line:column" are shown, without the caret.
//! - **The Typst CLI's `--input`, fonts folders and PDF standards** (the TypeScript passed none).
//! - Typst files that import packages (`@preview/...`): the error says why, as the TypeScript's "downloads from
//!   the internet" did.

use super::pdf::{device_size, zoom_in, zoom_out, Layout, PageSize, Want, CACHE_BUDGET, MAX_ZOOM, MIN_ZOOM, PAD};
use super::states::Waiting;
use crate::kit::prelude::*;
use crate::typst_engine::{self, Diagnostic, Failure, Limits, PageInfo, Rendered, Severity, PREVIEW_BUDGET};
use crate::{actions, t, theme};
use glint_model::entries::FileEntry;
use gpui_kit::{
    canvas, img, point, size, Bounds, FocusHandle, Focusable, KeyDownEvent, MouseButton, ObjectFit, RenderImage,
    ScrollEvent, StyledImage as _, Task,
};
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// A document bigger than this isn't typeset here (a hand-written `.typ` is a few kilobytes; this stops a data
/// file with the extension from being fed to the compiler).
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;
/// How often the file and what it read are looked at for a save.
pub const WATCH: Duration = Duration::from_millis(1500);
/// A point on paper is 4/3 of a device-independent pixel (96 to the inch; a point is 1/72).
const DIP_PER_PT: f32 = 4. / 3.;
/// The most messages the failure view lists (the rest are counted).
const MAX_LISTED: usize = 100;
/// What a wheel notch or an arrow key scrolls.
const LINE: f32 = 48.;
/// A page is always white: the document is typeset for paper, so these are not the theme's.
const PAGE_WHITE: u32 = 0xff_ff_ff;
const PAGE_INK: u32 = 0x71_71_7a;

// ---- The compiler, behind a trait ---------------------------------------------------------------------------

/// A typeset document, used on the worker thread that made it.
pub trait Typeset {
    /// The pages' sizes in points.
    fn pages(&self) -> Vec<PageInfo>;
    /// What Typst warned about.
    fn warnings(&self) -> Vec<Diagnostic>;
    /// The files it read besides itself.
    fn dependencies(&self) -> Vec<PathBuf>;
    /// Page `page` at `pixels_per_pt` device pixels to the point.
    fn render(&self, page: usize, pixels_per_pt: f64) -> Option<Rendered>;
}

/// Where documents are typeset: Typst in the app, a fake in tests. `typeset` runs on the worker thread.
pub trait Typesetter: Send + Sync + 'static {
    /// Typesets the file at `path`, giving up when `limits` say so.
    fn typeset(&self, path: &Path, limits: &Limits) -> Result<Box<dyn Typeset>, Failure>;
}

impl Typeset for typst_engine::Compiled {
    fn pages(&self) -> Vec<PageInfo> {
        typst_engine::Compiled::pages(self).to_vec()
    }

    fn warnings(&self) -> Vec<Diagnostic> {
        self.warnings.clone()
    }

    fn dependencies(&self) -> Vec<PathBuf> {
        self.dependencies.clone()
    }

    fn render(&self, page: usize, pixels_per_pt: f64) -> Option<Rendered> {
        typst_engine::Compiled::render(self, page, pixels_per_pt)
    }
}

/// Typst, linked in.
pub struct InProcess;

impl Typesetter for InProcess {
    fn typeset(&self, path: &Path, limits: &Limits) -> Result<Box<dyn Typeset>, Failure> {
        let compiled = typst_engine::compile(typst_engine::Input::File(path.to_path_buf()), limits)?;
        Ok(Box::new(compiled))
    }
}

// ---- The worker thread --------------------------------------------------------------------------------------

/// A file and when it was last written, as the check for a save compares them.
type Stamp = (PathBuf, Option<SystemTime>);

fn written(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

enum Request {
    /// The pages to draw now, most wanted first: replaces the list from before.
    Want(Vec<Want>),
}

enum Reply {
    Opened {
        pages: Vec<PageInfo>,
        warnings: Vec<Diagnostic>,
        stamps: Vec<Stamp>,
    },
    Failed {
        failure: Failure,
        stamps: Vec<Stamp>,
    },
    Page {
        want: Want,
        image: Result<Arc<RenderImage>, String>,
    },
}

/// The worker and the way to it. Dropping it stops the compile (if it is still under way) and ends the thread.
struct Session {
    requests: async_channel::Sender<Request>,
    limits: Limits,
}

impl Session {
    fn start(typesetter: Arc<dyn Typesetter>, path: PathBuf) -> (Session, async_channel::Receiver<Reply>) {
        let (requests, wanted) = async_channel::unbounded();
        let (reply, replies) = async_channel::unbounded();
        let limits = Limits::new(PREVIEW_BUDGET);
        let worker_limits = limits.clone();
        // If the thread can't start, `replies` closes unanswered: the view says it couldn't be typeset.
        let _ = typst_engine::run_big("glint-typst-view", move || {
            work(typesetter.as_ref(), &path, &worker_limits, &wanted, &reply);
        });
        (Session { requests, limits }, replies)
    }

    fn want(&self, list: Vec<Want>) {
        let _ = self.requests.try_send(Request::Want(list));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.limits.stop();
    }
}

/// The files besides `main` with when they were last written.
fn stamps_of(dependencies: &[PathBuf], main: &Path) -> Vec<Stamp> {
    dependencies
        .iter()
        .filter(|dependency| dependency.as_path() != main)
        .map(|dependency| (dependency.clone(), written(dependency)))
        .collect()
}

/// The worker: typesets the file, tells the pages' sizes, and draws what is wanted until the view lets go.
fn work(
    typesetter: &dyn Typesetter,
    path: &Path,
    limits: &Limits,
    wanted: &async_channel::Receiver<Request>,
    replies: &async_channel::Sender<Reply>,
) {
    // The file's time before it is read, so a save during the compile is noticed afterwards.
    let before = written(path);
    let document = match typesetter.typeset(path, limits) {
        Ok(document) => document,
        Err(failure) => {
            // A failed document is typeset again when it is fixed: the files it read are watched too.
            let mut stamps = stamps_of(&failure.dependencies, path);
            stamps.push((path.to_path_buf(), before));
            let _ = replies.send_blocking(Reply::Failed { failure, stamps });
            return;
        }
    };
    if limits.stopped() {
        return;
    }
    let pages = document.pages();
    let mut stamps = stamps_of(&document.dependencies(), path);
    stamps.push((path.to_path_buf(), before));
    let opened = Reply::Opened {
        pages: pages.clone(),
        warnings: document.warnings(),
        stamps,
    };
    if replies.send_blocking(opened).is_err() {
        return;
    }
    let mut todo: VecDeque<Want> = VecDeque::new();
    loop {
        // The newest list of wanted pages replaces the one before, so a fast scroll skips what it passed.
        loop {
            match wanted.try_recv() {
                Ok(Request::Want(list)) => todo = list.into(),
                Err(async_channel::TryRecvError::Empty) => break,
                Err(async_channel::TryRecvError::Closed) => return,
            }
        }
        if let Some(want) = todo.pop_front() {
            let image = draw(document.as_ref(), &pages, want);
            if replies.send_blocking(Reply::Page { want, image }).is_err() {
                return;
            }
            continue;
        }
        match wanted.recv_blocking() {
            Ok(Request::Want(list)) => todo = list.into(),
            Err(_) => return,
        }
    }
}

/// One page drawn at the pixel size asked for, as GPUI's picture type.
fn draw(document: &dyn Typeset, pages: &[PageInfo], want: Want) -> Result<Arc<RenderImage>, String> {
    let info = pages
        .get(want.page)
        .ok_or_else(|| "there is no such page".to_string())?;
    if info.width_pt <= 0. {
        return Err("the page has no width".into());
    }
    let pixels_per_pt = f64::from(want.width) / f64::from(info.width_pt);
    let rendered = document
        .render(want.page, pixels_per_pt)
        .ok_or_else(|| "the page couldn't be drawn".to_string())?;
    let (width, height) = (rendered.width, rendered.height);
    image::RgbaImage::from_raw(width, height, rendered.into_bgra())
        .map(|buffer| Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
        .ok_or_else(|| "the page's pixels didn't fit its size".to_string())
}

// ---- The scroll ----------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct ScrollState {
    x: f32,
    y: f32,
    viewport: Bounds<Pixels>,
    content: (f32, f32),
}

/// The view's scroll position, shared with the scroll bar (which drags it) and the measuring canvas.
#[derive(Clone, Default)]
struct Scroll(Rc<Cell<ScrollState>>);

impl Scroll {
    fn get(&self) -> ScrollState {
        self.0.get()
    }

    fn edit(&self, f: impl FnOnce(&mut ScrollState)) {
        let mut state = self.0.get();
        f(&mut state);
        self.0.set(state);
    }

    fn viewport(&self) -> (f32, f32) {
        let s = self.get().viewport.size;
        (f32::from(s.width), f32::from(s.height))
    }

    /// The farthest the column can be scrolled either way.
    fn max(&self) -> (f32, f32) {
        let s = self.get();
        let (w, h) = self.viewport();
        ((s.content.0 - w).max(0.), (s.content.1 - h).max(0.))
    }

    /// Moves to `(x, y)`, kept inside the column; true when that moved it.
    fn set(&self, x: f32, y: f32) -> bool {
        let (mx, my) = self.max();
        let before = self.get();
        let (x, y) = (x.clamp(0., mx), y.clamp(0., my));
        self.edit(|s| {
            s.x = x;
            s.y = y;
        });
        (before.x - x).abs() > 0.01 || (before.y - y).abs() > 0.01
    }

    fn by(&self, dx: f32, dy: f32) -> bool {
        let s = self.get();
        self.set(s.x + dx, s.y + dy)
    }
}

impl gpui_kit::base::ScrollbarHandle for Scroll {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.get().viewport
    }

    fn offset(&self) -> Point<Pixels> {
        let s = self.get();
        point(px(-s.x), px(-s.y))
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        self.set(-f32::from(offset.x), -f32::from(offset.y));
    }

    fn content_size(&self) -> Size<Pixels> {
        let s = self.get();
        size(px(s.content.0), px(s.content.1))
    }
}

// ---- The view ------------------------------------------------------------------------------------------------

/// What the view is doing with the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Being typeset for the first time.
    Opening,
    /// Typst couldn't typeset it (the headline; the messages are in [`TypstView::errors`]).
    Failed(SharedString),
    /// Typeset: the pages are drawn.
    Ready,
}

/// How the person asked for the zoom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    /// Fit to width: follows the box.
    Fit,
    /// A step (or the fit, once changed): 1 is 96 dots to the inch.
    Factor(f32),
}

/// A page drawn, and the size it was drawn at.
struct Drawn {
    image: Arc<RenderImage>,
    want: (u32, u32),
    bytes: usize,
}

/// A Typst document's pages in a scrolling column with zoom and a page box, and its messages when it has errors.
pub struct TypstView {
    entry: FileEntry,
    typesetter: Arc<dyn Typesetter>,
    state: State,
    waiting: Entity<Waiting>,
    session: Option<Session>,
    /// A new typeset is under way (the old pages, if any, stay until it is done).
    compiling: bool,
    layout: Option<Layout>,
    zoom: Zoom,
    scroll: Scroll,
    drawn: HashMap<usize, Drawn>,
    /// Pages that failed to draw at a size, so they aren't asked for again at it.
    failed: HashMap<usize, (u32, u32)>,
    /// The last list sent to the worker.
    asked: Vec<Want>,
    /// Pictures let go of, until a frame can drop them from the GPU's atlas.
    retired: Vec<Arc<RenderImage>>,
    /// The errors of the last typeset, when it failed; the warnings of the last that worked.
    errors: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,
    show_warnings: bool,
    /// The files whose saving typesets it again.
    stamps: Vec<Stamp>,
    focus: FocusHandle,
    go_to: Entity<InputState>,
    drag: Option<(Point<Pixels>, (f32, f32))>,
    _pump: Option<Task<()>>,
    _watch: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl TypstView {
    /// The pages of the Typst file `entry`. Typesetting starts at once, off the UI thread.
    pub fn new(entry: FileEntry, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_typesetter(entry, Arc::new(InProcess), window, cx)
    }

    /// [`TypstView::new`] over `typesetter` (tests pass fakes).
    pub fn with_typesetter(
        entry: FileEntry,
        typesetter: Arc<dyn Typesetter>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let waiting = cx.new(|cx| Waiting::new(Some(t!("Typesetting…")), cx));
        let go_to = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Page")));
        let subscriptions = vec![
            cx.subscribe_in(&go_to, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.go_from_box(window, cx);
                }
            }),
            // The pictures let go of the GPU's atlas, window or no window left.
            cx.on_release_in(window, |this, window, _| {
                for (_, drawn) in this.drawn.drain() {
                    window.drop_image(drawn.image).ok();
                }
                for image in this.retired.drain(..) {
                    window.drop_image(image).ok();
                }
            }),
        ];
        let mut view = TypstView {
            entry,
            typesetter,
            state: State::Opening,
            waiting,
            session: None,
            compiling: false,
            layout: None,
            zoom: Zoom::Fit,
            scroll: Scroll::default(),
            drawn: HashMap::new(),
            failed: HashMap::new(),
            asked: Vec::new(),
            retired: Vec::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            show_warnings: false,
            stamps: Vec::new(),
            focus: cx.focus_handle(),
            go_to,
            drag: None,
            _pump: None,
            _watch: None,
            _subscriptions: subscriptions,
        };
        view.start(window, cx);
        view
    }

    /// The file shown.
    pub fn entry(&self) -> &FileEntry {
        &self.entry
    }

    /// What the view is doing: typesetting, failed, or showing pages.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Whether a new typeset is under way.
    pub fn is_compiling(&self) -> bool {
        self.compiling
    }

    /// Typst's errors for the document, when it failed.
    pub fn errors(&self) -> &[Diagnostic] {
        &self.errors
    }

    /// Typst's warnings for the last document that was typeset.
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }

    /// How many pages the document has, once it is typeset.
    pub fn page_count(&self) -> Option<usize> {
        self.layout.as_ref().map(Layout::len)
    }

    /// The page at the middle of the box (1 is the first), once typeset.
    pub fn current_page(&self) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let state = self.scroll.get();
        let (_, h) = self.scroll.viewport();
        Some(layout.page_at(state.y + h / 2.) + 1)
    }

    /// The zoom now (1 is 96 dots to the inch, the page at its own size).
    pub fn zoom(&self) -> f32 {
        match (self.zoom, &self.layout) {
            (Zoom::Factor(zoom), _) => zoom,
            (Zoom::Fit, Some(layout)) => layout.fit_zoom(self.scroll.viewport().0),
            (Zoom::Fit, None) => 1.,
        }
    }

    /// Whether the zoom follows the width of the box.
    pub fn is_fit(&self) -> bool {
        self.zoom == Zoom::Fit
    }

    /// The pages held drawn now (1 is the first), for the tests of what is let go.
    pub fn drawn_pages(&self) -> Vec<usize> {
        let mut pages: Vec<usize> = self.drawn.keys().map(|p| p + 1).collect();
        pages.sort_unstable();
        pages
    }

    /// The bytes of the pages held drawn.
    pub fn drawn_bytes(&self) -> usize {
        self.drawn.values().map(|d| d.bytes).sum()
    }

    /// Where the scroll is: pixels from the top of the first page's margin.
    pub fn scrolled(&self) -> f32 {
        self.scroll.get().y
    }

    // ---- Typesetting --------------------------------------------------------------------------------------

    /// Starts over: the first typeset of a file.
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.entry.size > MAX_BYTES {
            self.state = State::Failed(t!("Too big to typeset here: open it to read it."));
            self.errors.clear();
            return;
        }
        self.state = State::Opening;
        self.begin(window, cx);
    }

    /// Starts a typeset on the worker and listens for what it says. What is shown stays until it answers.
    fn begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (session, replies) = Session::start(self.typesetter.clone(), PathBuf::from(&self.entry.path));
        self.session = Some(session);
        self.compiling = true;
        self.asked.clear();
        self.stamps.clear();
        self._pump = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(reply) = replies.recv().await {
                if this.update(cx, |this, cx| this.heard(reply, cx)).is_err() {
                    return;
                }
            }
            // The worker ended without an answer (the thread could not start, or died).
            this.update_in(cx, |this, _, cx| {
                if this.compiling {
                    this.fail(
                        Failure::message(t!("Typst stopped unexpectedly on this document").to_string()),
                        cx,
                    );
                }
            })
            .ok();
        }));
        self.watch(window, cx);
        cx.notify();
    }

    /// Typesets the file again (a save, or the person asked); the old pages stay until the new ones are ready.
    pub fn retypeset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.entry.size > MAX_BYTES {
            return;
        }
        self.begin(window, cx);
    }

    /// The frame has a new entry for the view: another file starts over; the same file saved again is typeset again.
    pub fn set_entry(&mut self, entry: FileEntry, window: &mut Window, cx: &mut Context<Self>) {
        let same = glint_model::paths::same_path(&entry.path, &self.entry.path);
        let changed = entry.modified != self.entry.modified || entry.size != self.entry.size;
        self.entry = entry;
        if !same {
            self.reset(cx);
            self.start(window, cx);
        } else if changed {
            self.retypeset(window, cx);
        }
    }

    /// Lets go of everything shown (another file).
    fn reset(&mut self, cx: &mut Context<Self>) {
        self.session = None;
        self._pump = None;
        self._watch = None;
        self.compiling = false;
        self.layout = None;
        self.errors.clear();
        self.warnings.clear();
        self.show_warnings = false;
        self.stamps.clear();
        self.asked.clear();
        self.failed.clear();
        for (_, drawn) in self.drawn.drain() {
            self.retired.push(drawn.image);
        }
        self.scroll.edit(|s| {
            s.x = 0.;
            s.y = 0.;
            s.content = (0., 0.);
        });
        cx.notify();
    }

    fn fail(&mut self, failure: Failure, cx: &mut Context<Self>) {
        self.compiling = false;
        self.session = None;
        self.layout = None;
        for (_, drawn) in self.drawn.drain() {
            self.retired.push(drawn.image);
        }
        self.asked.clear();
        self.errors = failure.diagnostics;
        self.state = State::Failed(t!("Typst couldn't typeset it"));
        cx.notify();
    }

    fn heard(&mut self, reply: Reply, cx: &mut Context<Self>) {
        match reply {
            Reply::Opened {
                pages,
                warnings,
                stamps,
            } => {
                self.compiling = false;
                self.errors.clear();
                self.warnings = warnings;
                self.stamps = stamps;
                if pages.is_empty() {
                    self.fail(Failure::message(t!("This document has no pages.").to_string()), cx);
                    return;
                }
                let sizes: Vec<PageSize> = pages
                    .iter()
                    .map(|p| PageSize {
                        width: p.width_pt * DIP_PER_PT,
                        height: p.height_pt * DIP_PER_PT,
                    })
                    .collect();
                // Where the person was reading stays (a page and how far down it), when there was a document before.
                let anchor = self.anchor();
                let mut layout = Layout::new(sizes.len(), sizes[0]);
                layout.set_sizes(0, &sizes);
                self.layout = Some(layout);
                for (_, drawn) in self.drawn.drain() {
                    self.retired.push(drawn.image);
                }
                self.failed.clear();
                self.asked.clear();
                self.state = State::Ready;
                self.relayout(anchor.filter(|a| a.page < sizes.len()));
            }
            Reply::Failed { failure, stamps } => {
                self.fail(failure, cx);
                self.stamps = stamps;
                return;
            }
            Reply::Page { want, image } => match image {
                Ok(image) => {
                    let bytes = image.as_bytes(0).map_or(0, <[u8]>::len);
                    let drawn = Drawn {
                        image,
                        want: (want.width, want.height),
                        bytes,
                    };
                    if let Some(old) = self.drawn.insert(want.page, drawn) {
                        self.retired.push(old.image);
                    }
                    self.failed.remove(&want.page);
                }
                Err(_) => {
                    self.failed.insert(want.page, (want.width, want.height));
                }
            },
        }
        cx.notify();
    }

    /// Looks every [`WATCH`] whether the file or what it read was saved, and typesets again when so.
    fn watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._watch = Some(cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(WATCH).await;
            let Ok(stamps) = this.read_with(cx, |this, _| (!this.compiling).then(|| this.stamps.clone())) else {
                return;
            };
            let Some(stamps) = stamps.filter(|s| !s.is_empty()) else {
                continue;
            };
            let saved = cx
                .background_spawn(async move { stamps.iter().any(|(path, time)| written(path) != *time) })
                .await;
            if saved
                && this
                    .update_in(cx, |this, window, cx| {
                        if !this.compiling {
                            this.retypeset(window, cx);
                        }
                    })
                    .is_err()
            {
                return;
            }
        }));
    }

    // ---- Where things are ---------------------------------------------------------------------------------

    /// The place at the top of the box, as a page and a fraction.
    fn anchor(&self) -> Option<super::pdf::Anchor> {
        let layout = self.layout.as_ref()?;
        Some(layout.anchor(self.scroll.get().y))
    }

    /// Puts the scroll back at `anchor` after the places moved, and sets the column's size.
    fn relayout(&mut self, anchor: Option<super::pdf::Anchor>) {
        let Some(layout) = self.layout.as_ref() else {
            return;
        };
        let (vw, _) = self.scroll.viewport();
        let content = (layout.width(vw), layout.height());
        self.scroll.edit(|s| s.content = content);
        if let Some(anchor) = anchor {
            let y = layout.place(anchor);
            let x = self.scroll.get().x;
            self.scroll.set(x, y);
        }
    }

    /// Brings the places up to date with the zoom and the box, and works out which pages to draw, ask for and let
    /// go of. Called as the view is drawn; sends to the worker but never notifies.
    fn sync(&mut self, window: &mut Window) {
        let (vw, vh) = self.scroll.viewport();
        if vw <= 0. || vh <= 0. {
            return;
        }
        let zoom = self.zoom();
        let anchor = self.anchor();
        let moved = {
            let Some(layout) = self.layout.as_mut() else {
                return;
            };
            let moved = (layout.zoom() - zoom).abs() > f32::EPSILON;
            if moved {
                layout.rebuild(zoom);
            }
            moved
        };
        let content = self.layout.as_ref().map(|l| (l.width(vw), l.height()));
        if let Some(content) = content {
            if moved || self.scroll.get().content != content {
                self.scroll.edit(|s| s.content = content);
                if moved {
                    self.relayout(anchor);
                } else {
                    // Only the box changed: keep the scroll inside the column.
                    let s = self.scroll.get();
                    self.scroll.set(s.x, s.y);
                }
            }
        }
        let Some(layout) = self.layout.as_ref() else {
            return;
        };
        let top = self.scroll.get().y;
        let scale = window.scale_factor();
        let near = layout.visible(top - vh, top + vh + vh);
        let keep = layout.visible(top - 2.5 * vh, top + vh + 2.5 * vh);
        let on_screen = layout.visible(top, top + vh);
        let middle = top + vh / 2.;

        // Let go of what is far.
        let far: Vec<usize> = self.drawn.keys().copied().filter(|p| !keep.contains(p)).collect();
        for page in far {
            if let Some(drawn) = self.drawn.remove(&page) {
                self.retired.push(drawn.image);
            }
        }
        self.failed.retain(|p, _| keep.contains(p));
        // And what is over the budget, the farthest first (never a page on screen).
        let mut bytes: usize = self.drawn.values().map(|d| d.bytes).sum();
        if bytes > CACHE_BUDGET {
            let mut by_distance: Vec<(usize, f32)> = self
                .drawn
                .keys()
                .filter(|p| !on_screen.contains(*p))
                .map(|&p| {
                    let (_, h) = layout.extent(p);
                    (p, (layout.top(p) + h / 2. - middle).abs())
                })
                .collect();
            by_distance.sort_by(|a, b| b.1.total_cmp(&a.1));
            for (page, _) in by_distance {
                if bytes <= CACHE_BUDGET {
                    break;
                }
                if let Some(drawn) = self.drawn.remove(&page) {
                    bytes = bytes.saturating_sub(drawn.bytes);
                    self.retired.push(drawn.image);
                }
            }
        }

        // Ask for what is near and not drawn at the size it will be shown, nearest the middle first.
        let mut wanted: Vec<(Want, f32)> = Vec::new();
        for page in near {
            let (w, h) = layout.extent(page);
            let (dw, dh) = device_size(w, h, scale);
            let current = |(cw, ch): (u32, u32)| cw.abs_diff(dw) <= 1 && ch.abs_diff(dh) <= 1;
            if self.drawn.get(&page).is_some_and(|d| current(d.want))
                || self.failed.get(&page).is_some_and(|&f| current(f))
            {
                continue;
            }
            wanted.push((
                Want {
                    page,
                    width: dw,
                    height: dh,
                },
                (layout.top(page) + h / 2. - middle).abs(),
            ));
        }
        wanted.sort_by(|a, b| a.1.total_cmp(&b.1));
        let wanted: Vec<Want> = wanted.into_iter().map(|(want, _)| want).collect();
        if wanted != self.asked {
            if let Some(session) = &self.session {
                session.want(wanted.clone());
            }
            self.asked = wanted;
        }
        for image in self.retired.drain(..) {
            window.drop_image(image).ok();
        }
    }

    // ---- Scrolling and zooming -----------------------------------------------------------------------------

    /// Scrolls the pages a page (Page Down and Page Up in Quick Look: 0.85 of the box). Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let (_, vh) = self.scroll.viewport();
        let step = vh * 0.85;
        let moved = self.scroll.by(0., if down { step } else { -step });
        if moved {
            cx.notify();
        }
        moved
    }

    /// Scrolls so that page `n` (1 is the first) is at the top; false when there is no such page.
    pub fn go_to_page(&mut self, n: usize, cx: &mut Context<Self>) -> bool {
        let Some(layout) = self.layout.as_ref().filter(|l| n >= 1 && n <= l.len()) else {
            return false;
        };
        let y = layout.top(n - 1) - PAD;
        let x = self.scroll.get().x;
        self.scroll.set(x, y);
        cx.notify();
        true
    }

    /// The page box: a number, 1 to the last page.
    fn go_from_box(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.go_to.read(cx).value().to_string();
        let count = self.page_count().unwrap_or(0);
        match text.trim().parse::<usize>() {
            Ok(n) if n >= 1 && n <= count => {
                self.go_to_page(n, cx);
                self.go_to.update(cx, |input, cx| input.set_value("", window, cx));
                window.focus(&self.focus, cx);
            }
            _ => window.push_notification(Notification::error(t!("Type a page number, 1 to {n}", n = count)), cx),
        }
    }

    fn set_zoom(&mut self, zoom: Zoom, cx: &mut Context<Self>) {
        if let Zoom::Factor(f) = zoom {
            self.zoom = Zoom::Factor(f.clamp(MIN_ZOOM, MAX_ZOOM));
        } else {
            self.zoom = zoom;
        }
        cx.notify();
    }

    /// The next zoom step up.
    pub fn zoom_in(&mut self, cx: &mut Context<Self>) {
        self.set_zoom(Zoom::Factor(zoom_in(self.zoom())), cx);
    }

    /// The next zoom step down.
    pub fn zoom_out(&mut self, cx: &mut Context<Self>) {
        self.set_zoom(Zoom::Factor(zoom_out(self.zoom())), cx);
    }

    /// Fit to the width of the box, and follow it.
    pub fn fit_width(&mut self, cx: &mut Context<Self>) {
        self.set_zoom(Zoom::Fit, cx);
    }

    fn wheel(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(LINE));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        if event.modifiers.control || event.modifiers.platform {
            if dy > 0. {
                self.zoom_in(cx);
            } else if dy < 0. {
                self.zoom_out(cx);
            }
        } else if event.modifiers.shift {
            // Shift turns the wheel sideways.
            if self.scroll.by(-(dx + dy), 0.) {
                cx.notify();
            }
        } else if self.scroll.by(-dx, -dy) {
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // The pages' own keys: not for what is typed in the page box under them.
        if !self.focus.is_focused(window) {
            return;
        }
        let keystroke = &event.keystroke;
        let key = keystroke.key.as_str();
        let typed = keystroke.key_char.as_deref().unwrap_or("");
        let (_, vh) = self.scroll.viewport();
        let moved = match (key, typed) {
            ("=" | "+" | "plus", _) | (_, "+") => {
                self.zoom_in(cx);
                true
            }
            ("-" | "minus", _) | (_, "-") => {
                self.zoom_out(cx);
                true
            }
            ("0", _) => {
                self.fit_width(cx);
                true
            }
            ("pagedown", _) => self.page(true, cx),
            ("pageup", _) => self.page(false, cx),
            ("down", _) => self.scroll.by(0., LINE),
            ("up", _) => self.scroll.by(0., -LINE),
            ("right", _) => self.scroll.by(LINE, 0.),
            ("left", _) => self.scroll.by(-LINE, 0.),
            ("home", _) => self.scroll.set(self.scroll.get().x, 0.),
            ("end", _) => {
                let (_, my) = self.scroll.max();
                self.scroll.set(self.scroll.get().x, my.max(vh))
            }
            ("f5", _) => {
                self.retypeset(window, cx);
                true
            }
            _ => return,
        };
        if moved {
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn pointer_down(&mut self, event: &gpui_kit::MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let s = self.scroll.get();
        self.drag = Some((event.position, (s.x, s.y)));
    }

    fn pointer_moved(&mut self, event: &gpui_kit::MouseMoveEvent, cx: &mut Context<Self>) {
        let Some((from, (x, y))) = self.drag else {
            return;
        };
        if !event.dragging() {
            self.drag = None;
            return;
        }
        let (dx, dy) = (
            f32::from(event.position.x - from.x),
            f32::from(event.position.y - from.y),
        );
        if self.scroll.set(x - dx, y - dy) {
            cx.notify();
        }
    }
}

impl Focusable for TypstView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

// ---- Drawing --------------------------------------------------------------------------------------------------

impl Render for TypstView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let root = col()
            .id("typst-view")
            .size_full()
            .min_h_0()
            .min_w_0()
            .text_start()
            .track_focus(&self.focus)
            .tab_stop(true)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollDown, _, cx| {
                this.page(true, cx);
            }))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollUp, _, cx| {
                this.page(false, cx);
            }));
        match self.state.clone() {
            State::Opening => return root.child(self.waiting.clone()),
            State::Failed(headline) => return root.child(self.failure_view(headline, &t, cx)),
            State::Ready => {}
        }
        self.sync(window);
        let Some(layout) = self.layout.as_ref() else {
            return root;
        };

        let scroll = self.scroll.get();
        let (vw, vh) = self.scroll.viewport();
        let content_w = layout.width(vw);
        let on_screen = layout.visible(scroll.y, scroll.y + vh);
        let pages: Vec<AnyElement> = on_screen
            .map(|i| {
                let (w, h) = layout.extent(i);
                let x = (content_w - w) / 2. - scroll.x;
                let y = layout.top(i) - scroll.y;
                let picture = self.drawn.get(&i).map(|d| d.image.clone());
                div()
                    .id(("typst-page", i))
                    .role(Role::Image)
                    .aria_label(t!("Page {n}", n = i + 1))
                    .test_support()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(w))
                    .h(px(h))
                    .bg(gpui_kit::rgb(PAGE_WHITE))
                    .border_1()
                    .border_color(t.border)
                    .shadow_sm()
                    .overflow_hidden()
                    .map(|page| match picture {
                        Some(image) => page.child(img(image).object_fit(ObjectFit::Fill).size_full()),
                        None => page.child(centered_number(i + 1)),
                    })
                    .into_any_element()
            })
            .collect();

        let area = self.scroll.clone();
        let id = cx.entity_id();
        let measuring = canvas(
            move |bounds, window, _| {
                if area.get().viewport != bounds {
                    let resized = area.get().viewport.size != bounds.size;
                    area.edit(|s| s.viewport = bounds);
                    if resized {
                        // Not `cx.notify` here: GPUI drops a redraw asked for during a draw. After this frame it counts.
                        window.on_next_frame(move |_, cx| cx.notify(id));
                    }
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        let overflows = scroll.content.0 > vw + 0.5;
        let name = SharedString::from(self.entry.name.clone());
        let area = div()
            .id("typst-pages")
            .role(Role::List)
            .aria_label(t!("Pages of {name}", name = name))
            .test_support()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .bg(t.muted)
            .when(overflows, |area| area.cursor_grab())
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                this.wheel(event, cx);
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui_kit::MouseDownEvent, window, cx| this.pointer_down(event, window, cx)),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui_kit::MouseMoveEvent, _, cx| this.pointer_moved(event, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.drag = None))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, _| this.drag = None))
            .child(measuring)
            .children(pages)
            .child(Scrollbar::new(&self.scroll).viewport_from_layout());

        let warnings = (self.show_warnings && !self.warnings.is_empty()).then(|| self.warnings_view(&t, cx));
        root.child(area).children(warnings).child(self.toolbar(&t, cx))
    }
}

/// The page's number, in the middle of a page not drawn yet.
fn centered_number(n: usize) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(gpui_kit::rgb(PAGE_INK))
        .text_size(theme::size::SM)
        .child(n.to_string())
}

/// Whether `text` has letters that read right to left (so a caret under a column would point at the wrong letter).
fn has_right_to_left(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(
            c,
            '\u{0590}'..='\u{08FF}' | '\u{FB1D}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}' | '\u{200F}' | '\u{202B}'
                | '\u{202E}' | '\u{2067}'
        )
    })
}

/// "    ^" under the character at `column` (1 is the first) of `line`, or None when it can't be drawn right.
pub fn caret_under(line: &str, column: usize) -> Option<String> {
    if has_right_to_left(line) || column == 0 {
        return None;
    }
    // Tabs keep their width in the caret line so the two line up.
    let lead: String = line
        .chars()
        .take(column - 1)
        .map(|c| if c == '\t' { '\t' } else { ' ' })
        .collect();
    Some(format!("{lead}^"))
}

impl TypstView {
    /// One message: where, what, the line itself, and Typst's hints.
    fn message(&self, n: usize, d: &Diagnostic, t: &theme::Tokens, cx: &App) -> impl IntoElement {
        let mono = theme::mono_font(cx);
        let tone = if d.severity == Severity::Error {
            t.destructive
        } else {
            t.warning
        };
        let place = d.place().map(SharedString::from);
        let label: SharedString = d.to_string().into();
        let snippet = d.line_text.clone().filter(|l| !l.trim().is_empty());
        col()
            .id(("typst-message", n))
            .role(Role::ListItem)
            .aria_label(label)
            .test_support()
            .w_full()
            .min_w_0()
            .gap(space(1.))
            .p(space(2.))
            .rounded(theme::size::RADIUS_SM)
            .border_1()
            .border_color(t.border)
            .bg(t.card)
            .text_start()
            .children(place.map(|p| {
                div()
                    .font_family(mono.clone())
                    .text_size(theme::size::XS)
                    .text_color(tone)
                    .child(isolate_path(&p))
            }))
            .child(
                div()
                    .min_w_0()
                    .text_size(theme::size::SM)
                    .text_color(t.foreground)
                    .child(d.message.clone()),
            )
            .children(snippet.map(|line| {
                let caret = d.column.and_then(|c| caret_under(&line, c));
                col()
                    .min_w_0()
                    .overflow_hidden()
                    .p(space(1.5))
                    .rounded(theme::size::RADIUS_SM)
                    .bg(t.muted)
                    .font_family(mono.clone())
                    .text_size(theme::size::XS)
                    .text_color(t.foreground)
                    // Source code reads left to right whatever the interface does.
                    .text_left()
                    .child(div().whitespace_nowrap().child(line))
                    .children(caret.map(|c| div().whitespace_nowrap().text_color(tone).child(c)))
            }))
            .children(d.hints.iter().map(|hint| {
                div()
                    .min_w_0()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(t!("Hint: {hint}", hint = hint.clone()))
            }))
    }

    /// The messages as a list (the first [`MAX_LISTED`], the rest counted).
    fn messages(
        &self,
        list: &[Diagnostic],
        id: &'static str,
        label: SharedString,
        t: &theme::Tokens,
        cx: &App,
    ) -> impl IntoElement {
        let more = list.len().saturating_sub(MAX_LISTED);
        col()
            .id(id)
            .role(Role::List)
            .aria_label(label)
            .test_support()
            .w_full()
            .min_w_0()
            .gap(space(1.5))
            .children(
                list.iter()
                    .take(MAX_LISTED)
                    .enumerate()
                    .map(|(n, d)| self.message(n, d, t, cx)),
            )
            .children((more > 0).then(|| {
                let line = t!("and {n} more", n = more);
                div()
                    .id("typst-more")
                    .role(Role::Status)
                    .aria_label(line.clone())
                    .test_support()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(line)
            }))
    }

    /// Typst couldn't typeset it: the headline, then each message with its place (`<p>Typst couldn't typeset it</p>`).
    fn failure_view(&self, headline: SharedString, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let errors = self.messages(&self.errors, "typst-errors", t!("Typst's messages"), t, cx);
        col()
            .id("typst-failed")
            .size_full()
            .min_h_0()
            .gap(space(3.))
            .p(space(4.))
            .text_start()
            .child(
                row_start()
                    .flex_none()
                    .items_center()
                    .gap(space(2.))
                    .child(icon(IconName::TriangleAlert).flex_none().text_color(t.warning))
                    .child(
                        div()
                            .id("typst-headline")
                            .role(Role::Alert)
                            .aria_label(headline.clone())
                            .test_support()
                            .min_w_0()
                            .text_size(theme::size::SM)
                            .font_weight(FontWeight::MEDIUM)
                            .child(headline),
                    ),
            )
            .child(
                div()
                    .id("typst-errors-scroll")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .child(errors),
            )
            .child(
                row_start().flex_none().child(
                    Button::new("typst-retry")
                        .ghost()
                        .small()
                        .icon(IconName::RefreshCw)
                        .label(t!("Typeset again"))
                        .on_click(cx.listener(|this, _, window, cx| this.retypeset(window, cx))),
                ),
            )
    }

    /// The warnings of the document that was typeset, over the toolbar.
    fn warnings_view(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("typst-warnings-scroll")
            .flex_none()
            .max_h(space(56.))
            .w_full()
            .overflow_y_scroll()
            .p(space(2.))
            .border_t_1()
            .border_color(t.border)
            .child(self.messages(&self.warnings, "typst-warnings", t!("Warnings"), t, cx))
    }

    /// The row under the pages: the page box, Previous and Next page, zoom, and the warnings.
    fn toolbar(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.page_count().unwrap_or(0);
        let current = self.current_page().unwrap_or(1);
        let zoom = self.zoom();
        let percent = format!("{}%", (zoom * 100.).round());
        let small = |id: &'static str, glyph: IconName, label: SharedString, hint: SharedString| {
            Button::new(id)
                .ghost()
                .small()
                .icon(glyph)
                .accessibility_label(label)
                .tooltip(hint)
        };
        let previous = small(
            "typst-previous",
            IconName::ChevronUp,
            t!("Previous page"),
            t!("The previous page (Page Up scrolls a screen)"),
        )
        .disabled(current <= 1)
        .on_click(cx.listener(|this, _, _, cx| {
            let n = this.current_page().unwrap_or(1);
            this.go_to_page(n.saturating_sub(1).max(1), cx);
        }));
        let next = small(
            "typst-next",
            IconName::ChevronDown,
            t!("Next page"),
            t!("The next page (Page Down scrolls a screen)"),
        )
        .disabled(current >= count)
        .on_click(cx.listener(|this, _, _, cx| {
            let n = this.current_page().unwrap_or(1);
            this.go_to_page(n + 1, cx);
        }));
        let status: SharedString = t!("Page {n} of {count}", n = current, count = count);
        let zoom_out = small("typst-zoom-out", IconName::ZoomOut, t!("Zoom out"), t!("Zoom out (-)"))
            .on_click(cx.listener(|this, _, _, cx| this.zoom_out(cx)));
        let zoom_in = small("typst-zoom-in", IconName::ZoomIn, t!("Zoom in"), t!("Zoom in (+)"))
            .on_click(cx.listener(|this, _, _, cx| this.zoom_in(cx)));
        let fit = small(
            "typst-fit",
            IconName::MoveHorizontal,
            t!("Fit width"),
            t!("Fit the page to the width of the pane (0)"),
        )
        .selected(self.is_fit())
        .on_click(cx.listener(|this, _, _, cx| this.fit_width(cx)));
        let again = small(
            "typst-again",
            IconName::RefreshCw,
            t!("Typeset again"),
            t!("Typeset the document again (F5)"),
        )
        .on_click(cx.listener(|this, _, window, cx| this.retypeset(window, cx)));
        let warned = (!self.warnings.is_empty()).then(|| {
            let n = self.warnings.len();
            let label: SharedString = if n == 1 {
                t!("1 warning")
            } else {
                t!("{n} warnings", n = n)
            };
            Button::new("typst-warnings-toggle")
                .ghost()
                .small()
                .icon(IconName::TriangleAlert)
                .label(label.clone())
                .tooltip(t!("Typst's warnings about the document"))
                .selected(self.show_warnings)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_warnings = !this.show_warnings;
                    cx.notify();
                }))
        });
        let busy = self.compiling.then(|| {
            let line = t!("Typesetting again…");
            div()
                .id("typst-busy")
                .role(Role::Status)
                .aria_label(line.clone())
                .test_support()
                .flex_none()
                .text_color(t.muted_foreground)
                .child(line)
        });

        row_start()
            .id("typst-tools")
            .role(Role::Toolbar)
            .aria_label(t!("Typst tools"))
            .test_support()
            .flex_none()
            .flex_wrap()
            .justify_center()
            .items_center()
            .gap(space(1.5))
            .px(space(3.))
            .py(space(1.5))
            .border_t_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .child(previous)
            .child(
                div()
                    .w(space(14.))
                    .flex_none()
                    .child(Input::new(&self.go_to).small().aria_label(t!("Go to page"))),
            )
            .child(
                div()
                    .id("typst-page-status")
                    .role(Role::Status)
                    .aria_label(status.clone())
                    .test_support()
                    .flex_none()
                    .child(status),
            )
            .child(next)
            .child(zoom_out)
            .child(
                div()
                    .id("typst-zoom-level")
                    .role(Role::Status)
                    .aria_label(t!("Zoom: {percent}", percent = percent.clone()))
                    .test_support()
                    .flex_none()
                    .min_w(space(9.))
                    .text_center()
                    .child(percent),
            )
            .child(zoom_in)
            .child(fit)
            .child(again)
            .children(warned)
            .children(busy)
    }
}

/// A path or place for display: file names with Arabic letters stay in their own order inside the sentence.
fn isolate_path(text: &str) -> SharedString {
    isolate(text.to_string())
}

// ---- Tests ----------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::file;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Instant;

    // ---- Fakes ------------------------------------------------------------------------------------------

    /// A fake compiler: pages of the sizes given, errors if asked, a log of what was typeset and drawn.
    #[derive(Clone)]
    struct Fake {
        pages: Arc<Mutex<Vec<PageInfo>>>,
        failure: Arc<Mutex<Option<Failure>>>,
        /// How long typesetting takes, to see that the view doesn't wait for it.
        slow: Arc<Mutex<Duration>>,
        /// Block until the stop flag is set, then count it.
        until_stopped: bool,
        stopped: Arc<AtomicUsize>,
        warnings: Vec<Diagnostic>,
        dependencies: Vec<PathBuf>,
        runs: Arc<Mutex<Vec<String>>>,
        drawn: Arc<Mutex<Vec<(usize, u32, u32)>>>,
    }

    struct FakeDocument {
        fake: Fake,
        pages: Vec<PageInfo>,
    }

    /// A4 in points.
    const A4: PageInfo = PageInfo {
        width_pt: 595.3,
        height_pt: 841.9,
    };

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    impl Fake {
        fn a4(pages: usize) -> Self {
            Fake {
                pages: Arc::new(Mutex::new(vec![A4; pages])),
                failure: Arc::default(),
                slow: Arc::default(),
                until_stopped: false,
                stopped: Arc::default(),
                warnings: Vec::new(),
                dependencies: Vec::new(),
                runs: Arc::default(),
                drawn: Arc::default(),
            }
        }

        fn runs(&self) -> usize {
            lock(&self.runs).len()
        }

        fn drawn(&self) -> Vec<(usize, u32, u32)> {
            lock(&self.drawn).clone()
        }

        fn set_pages(&self, pages: usize) {
            *lock(&self.pages) = vec![A4; pages];
        }

        fn set_failure(&self, failure: Option<Failure>) {
            *lock(&self.failure) = failure;
        }

        fn set_slow(&self, slow: Duration) {
            *lock(&self.slow) = slow;
        }
    }

    impl Typesetter for Fake {
        fn typeset(&self, path: &Path, limits: &Limits) -> Result<Box<dyn Typeset>, Failure> {
            lock(&self.runs).push(path.to_string_lossy().into_owned());
            let slow = *lock(&self.slow);
            std::thread::sleep(slow);
            if self.until_stopped {
                let began = Instant::now();
                while !limits.stopped() && began.elapsed() < Duration::from_secs(20) {
                    std::thread::sleep(Duration::from_millis(2));
                }
                if limits.stopped() {
                    self.stopped.fetch_add(1, Ordering::SeqCst);
                }
                return Err(Failure::message("stopped"));
            }
            if let Some(failure) = lock(&self.failure).clone() {
                return Err(failure);
            }
            let pages = lock(&self.pages).clone();
            Ok(Box::new(FakeDocument {
                fake: self.clone(),
                pages,
            }))
        }
    }

    impl Typeset for FakeDocument {
        fn pages(&self) -> Vec<PageInfo> {
            self.pages.clone()
        }

        fn warnings(&self) -> Vec<Diagnostic> {
            self.fake.warnings.clone()
        }

        fn dependencies(&self) -> Vec<PathBuf> {
            self.fake.dependencies.clone()
        }

        fn render(&self, page: usize, pixels_per_pt: f64) -> Option<Rendered> {
            let info = self.pages.get(page)?;
            let width = (f64::from(info.width_pt) * pixels_per_pt).round() as u32;
            let height = (f64::from(info.height_pt) * pixels_per_pt).round() as u32;
            lock(&self.fake.drawn).push((page, width, height));
            Some(Rendered {
                width,
                height,
                rgba: vec![255; (width * height * 4) as usize],
            })
        }
    }

    fn error_at(file: &str, line: usize, column: usize, message: &str, text: &str) -> Diagnostic {
        Diagnostic {
            severity: Severity::Error,
            message: message.into(),
            hints: Vec::new(),
            file: Some(file.into()),
            line: Some(line),
            column: Some(column),
            line_text: Some(text.into()),
        }
    }

    fn failing(list: Vec<Diagnostic>) -> Failure {
        Failure {
            reason: crate::typst_engine::Reason::Errors,
            diagnostics: list,
            dependencies: Vec::new(),
        }
    }

    // ---- Driving ----------------------------------------------------------------------------------------

    fn open_entry(cx: &mut TestAppContext, entry: FileEntry, fake: Fake) -> (Entity<TypstView>, VisualTestContext) {
        cx.update(testing::init);
        let (visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| TypstView::with_typesetter(entry, Arc::new(fake), window, cx))
        });
        (view, visual)
    }

    fn open(cx: &mut TestAppContext, fake: Fake) -> (Entity<TypstView>, VisualTestContext) {
        open_entry(cx, file("C:\\docs", "report.typ", 1000, 1), fake)
    }

    /// Frames and parked work until `done` (up to `seconds`).
    fn until_for(
        visual: &mut VisualTestContext,
        seconds: u64,
        what: &str,
        mut done: impl FnMut(&mut VisualTestContext) -> bool,
    ) {
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(seconds) {
            visual.run_until_parked();
            visual.update(|window, cx| window.render_frame(cx));
            visual.run_until_parked();
            if done(visual) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("never happened: {what}");
    }

    fn until(visual: &mut VisualTestContext, what: &str, done: impl FnMut(&mut VisualTestContext) -> bool) {
        until_for(visual, 10, what, done);
    }

    fn frames(visual: &mut VisualTestContext) {
        for _ in 0..4 {
            visual.update(|window, cx| window.render_frame(cx));
            visual.run_until_parked();
        }
    }

    /// Typeset, with the first page drawn.
    fn ready(view: &Entity<TypstView>, visual: &mut VisualTestContext) {
        until(visual, "the first page is drawn", |cx| {
            view.read_with(cx, |v, _| v.drawn_pages().contains(&1))
        });
        frames(visual);
    }

    fn has(visual: &mut VisualTestContext, label: &str) -> bool {
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn role_of(visual: &mut VisualTestContext, label: &str) -> Option<Role> {
        visual.update(|window, _| window.try_find_by_label(label).and_then(|e| e.role()))
    }

    fn click(visual: &mut VisualTestContext, id: &'static str) {
        visual.update(|window, cx| window.click(id, cx));
        frames(visual);
    }

    fn focus(view: &Entity<TypstView>, visual: &mut VisualTestContext) {
        visual.update(|window, cx| {
            let handle = view.read(cx).focus.clone();
            window.focus(&handle, cx);
        });
    }

    fn press(visual: &mut VisualTestContext, key: &str) {
        visual.update(|window, cx| window.press(key, cx));
        frames(visual);
    }

    /// A folder of its own under the temp folder, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "glint-typst-view-{}-{tag}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("making {}: {e}", dir.display()));
            Scratch(dir)
        }

        fn file(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, text).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The entry of a file really on disk.
    fn entry_of(path: &Path) -> FileEntry {
        let dir = path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let modified = written(path)
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(1, |d| d.as_millis() as i64);
        file(&dir, &name, 100, modified)
    }

    fn touch(path: &Path, seconds_ahead: u64) {
        let later = SystemTime::now() + Duration::from_secs(seconds_ahead);
        std::fs::File::options()
            .write(true)
            .open(path)
            .and_then(|f| f.set_modified(later))
            .unwrap_or_else(|e| panic!("touching {}: {e}", path.display()));
    }

    fn state_of(view: &Entity<TypstView>, visual: &VisualTestContext) -> State {
        view.read_with(visual, |v, _| v.state().clone())
    }

    fn failed(view: &Entity<TypstView>, cx: &VisualTestContext) -> bool {
        view.read_with(cx, |v, _| matches!(v.state(), State::Failed(_)))
    }

    // ---- Pure ---------------------------------------------------------------------------------------------

    #[test]
    fn a_caret_is_drawn_under_the_column_unless_the_line_reads_right_to_left() {
        assert_eq!(caret_under("  #nosuch(1)", 4).as_deref(), Some("   ^"));
        assert_eq!(caret_under("x", 1).as_deref(), Some("^"));
        assert_eq!(caret_under("\t#f()", 2).as_deref(), Some("\t^"));
        assert_eq!(caret_under("anything", 0), None);
        // Arabic and Hebrew lines are reordered when drawn, so a column would point at the wrong letter.
        assert_eq!(caret_under("مرحبا #غير_موجود", 8), None);
        assert_eq!(caret_under("שלום #x", 6), None);
        // A column past the end still lines up (an error at the end of the line).
        assert_eq!(caret_under("ab", 3).as_deref(), Some("  ^"));
    }

    #[test]
    fn the_icons_the_view_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::TriangleAlert,
            IconName::ChevronUp,
            IconName::ChevronDown,
            IconName::ZoomIn,
            IconName::ZoomOut,
            IconName::MoveHorizontal,
            IconName::RefreshCw,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[test]
    fn a_point_is_four_thirds_of_a_device_independent_pixel() {
        // A4 is 794 x 1123 px at 96 dpi.
        assert_eq!((A4.width_pt * DIP_PER_PT).round() as u32, 794);
        assert_eq!((A4.height_pt * DIP_PER_PT).round() as u32, 1123);
    }

    // ---- The view with a fake compiler ---------------------------------------------------------------------

    #[gpui_kit::test]
    fn pages_are_drawn_near_the_box_only_and_the_ui_does_not_wait(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let fake = Fake::a4(300);
        fake.set_slow(Duration::from_millis(300));
        let (view, mut visual) = open(cx, fake.clone());
        // Typesetting takes a while on the worker: the UI thread doesn't wait for it, it shows the wait.
        assert_eq!(state_of(&view, &visual), State::Opening);
        assert!(view.read_with(&visual, |v, _| v.is_compiling()));
        assert!(view.read_with(&visual, |v, _| v.drawn_pages().is_empty()));
        ready(&view, &mut visual);
        assert_eq!(state_of(&view, &visual), State::Ready);
        assert!(!view.read_with(&visual, |v, _| v.is_compiling()));
        assert_eq!(view.read_with(&visual, |v, _| v.page_count()), Some(300));
        assert!(view.read_with(&visual, |v, _| v.is_fit()));
        assert_eq!(role_of(&mut visual, "Pages of report.typ"), Some(Role::List));
        assert_eq!(role_of(&mut visual, "Page 1"), Some(Role::Image));
        assert!(has(&mut visual, "Page 1 of 300"));
        assert!(!has(&mut visual, "Page 40"), "a page far away isn't there");
        let asked: Vec<usize> = fake.drawn().iter().map(|d| d.0).collect();
        assert!(
            !asked.is_empty() && asked.iter().all(|&p| p < 4),
            "only the first pages were drawn: {asked:?}"
        );
        assert_eq!(fake.runs(), 1);
        let on_screen = visual.update(|window, _| window.find_all_by_role(Role::Image).len());
        assert!(on_screen <= 4, "{on_screen} pages in the tree");
    }

    #[gpui_kit::test]
    fn scrolling_far_draws_there_and_lets_go_of_the_pages_left_behind(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(cx, Fake::a4(500));
        ready(&view, &mut visual);
        view.update(&mut visual, |v, cx| assert!(v.go_to_page(300, cx)));
        until(&mut visual, "page 300 is drawn", |cx| {
            view.read_with(cx, |v, _| v.drawn_pages().contains(&300))
        });
        frames(&mut visual);
        let held = view.read_with(&visual, |v, _| v.drawn_pages());
        assert!(held.iter().all(|p| (290..=310).contains(p)), "pages kept: {held:?}");
        assert!(has(&mut visual, "Page 300"));
        assert!(!has(&mut visual, "Page 1"));
        view.update(&mut visual, |v, cx| assert!(v.go_to_page(1, cx)));
        until(&mut visual, "page 1 is drawn again", |cx| {
            view.read_with(cx, |v, _| v.drawn_pages().contains(&1))
        });
        frames(&mut visual);
        assert!(view.read_with(&visual, |v, _| v.drawn_pages().iter().all(|&p| p < 10)));
        view.update(&mut visual, |v, cx| {
            assert!(!v.go_to_page(0, cx) && !v.go_to_page(501, cx));
        });
    }

    #[gpui_kit::test]
    fn a_page_let_go_of_leaves_the_gpu_atlas_too(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(cx, Fake::a4(200));
        ready(&view, &mut visual);
        let first = view
            .read_with(&visual, |v, _| v.drawn.get(&0).map(|d| d.image.clone()))
            .unwrap_or_else(|| panic!("page 1 is drawn"));
        assert!(visual.update(|window, _| window.has_image_atlas_entry(&first)));
        view.update(&mut visual, |v, cx| {
            v.go_to_page(150, cx);
        });
        until(&mut visual, "page 150 is drawn", |cx| {
            view.read_with(cx, |v, _| v.drawn_pages().contains(&150))
        });
        frames(&mut visual);
        assert!(!view.read_with(&visual, |v, _| v.drawn_pages().contains(&1)));
        assert!(
            !visual.update(|window, _| window.has_image_atlas_entry(&first)),
            "a page let go of is dropped from the atlas"
        );
    }

    #[gpui_kit::test]
    fn zoom_buttons_and_keys_redraw_the_pages_at_the_new_size_and_keep_the_place(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let fake = Fake::a4(40);
        let (view, mut visual) = open(cx, fake.clone());
        ready(&view, &mut visual);
        view.update(&mut visual, |v, cx| {
            v.go_to_page(10, cx);
        });
        frames(&mut visual);
        let page_before = view.read_with(&visual, |v, _| v.current_page());
        assert_eq!(page_before, Some(10));
        click(&mut visual, "typst-zoom-in");
        let zoom = view.read_with(&visual, |v, _| v.zoom());
        assert!(zoom > 1., "zoom in from fit goes to the next step up: {zoom}");
        assert!(!view.read_with(&visual, |v, _| v.is_fit()));
        assert_eq!(view.read_with(&visual, |v, _| v.current_page()), page_before);
        // At the window's scale: the page's size at this zoom.
        let scale = visual.update(|window, _| window.scale_factor());
        let (w, h) = device_size(A4.width_pt * DIP_PER_PT * zoom, A4.height_pt * DIP_PER_PT * zoom, scale);
        until(&mut visual, "the pages are drawn at the new zoom", |_| {
            fake.drawn()
                .iter()
                .any(|&(p, dw, dh)| p == 9 && dw.abs_diff(w) <= 1 && dh.abs_diff(h) <= 1)
        });
        focus(&view, &mut visual);
        press(&mut visual, "-");
        assert!(view.read_with(&visual, |v, _| v.zoom()) < zoom);
        press(&mut visual, "=");
        assert!((view.read_with(&visual, |v, _| v.zoom()) - zoom).abs() < 1e-6);
        press(&mut visual, "0");
        assert!(view.read_with(&visual, |v, _| v.is_fit()));
        click(&mut visual, "typst-zoom-out");
        assert!(!view.read_with(&visual, |v, _| v.is_fit()));
        click(&mut visual, "typst-fit");
        assert!(view.read_with(&visual, |v, _| v.is_fit()));
        let label = format!("Zoom: {}%", (view.read_with(&visual, |v, _| v.zoom()) * 100.).round());
        assert!(has(&mut visual, &label), "{label}");
    }

    #[gpui_kit::test]
    fn previous_and_next_page_and_a_page_down_move_between_pages(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(cx, Fake::a4(12));
        ready(&view, &mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.current_page()), Some(1));
        click(&mut visual, "typst-next");
        assert_eq!(view.read_with(&visual, |v, _| v.current_page()), Some(2));
        click(&mut visual, "typst-next");
        click(&mut visual, "typst-previous");
        assert_eq!(view.read_with(&visual, |v, _| v.current_page()), Some(2));
        assert!(has(&mut visual, "Page 2 of 12"));
        // Page Down in Quick Look scrolls a screen and says it moved; at the top, up does not.
        view.update(&mut visual, |v, cx| assert!(v.go_to_page(1, cx)));
        frames(&mut visual);
        let moved = view.update(&mut visual, |v, cx| v.page(false, cx));
        assert!(!moved, "nothing above the first page");
        let moved = view.update(&mut visual, |v, cx| v.page(true, cx));
        assert!(moved);
        assert!(view.read_with(&visual, |v, _| v.scrolled()) > 100.);
    }

    #[gpui_kit::test]
    fn errors_are_listed_with_their_place_and_the_line_and_a_fix_brings_the_pages(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let fake = Fake::a4(1);
        let mut hinted = error_at("parts/intro.typ", 2, 3, "unknown variable: x", "  x + 1");
        hinted.hints.push("if you meant a literal, quote it".into());
        fake.set_failure(Some(failing(vec![
            error_at("report.typ", 4, 4, "unknown function: nosuch", "  #nosuch(1)"),
            hinted,
            Diagnostic::error("something with no place"),
        ])));
        let (view, mut visual) = open(cx, fake.clone());
        until(&mut visual, "the errors show", |cx| failed(&view, cx));
        frames(&mut visual);
        assert_eq!(
            state_of(&view, &visual),
            State::Failed("Typst couldn't typeset it".into())
        );
        assert_eq!(role_of(&mut visual, "Typst couldn't typeset it"), Some(Role::Alert));
        assert_eq!(role_of(&mut visual, "Typst's messages"), Some(Role::List));
        assert_eq!(
            role_of(&mut visual, "report.typ:4:4: unknown function: nosuch"),
            Some(Role::ListItem)
        );
        assert!(has(
            &mut visual,
            "parts/intro.typ:2:3: unknown variable: x (hint: if you meant a literal, quote it)"
        ));
        assert!(has(&mut visual, "something with no place"));
        assert_eq!(view.read_with(&visual, |v, _| v.errors().len()), 3);
        // Typeset again after a fix: the pages come.
        fake.set_failure(None);
        click(&mut visual, "typst-retry");
        ready(&view, &mut visual);
        assert_eq!(state_of(&view, &visual), State::Ready);
        assert!(view.read_with(&visual, |v, _| v.errors().is_empty()));
        assert_eq!(fake.runs(), 2);
    }

    #[gpui_kit::test]
    fn a_hundred_messages_are_listed_and_the_rest_counted(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let fake = Fake::a4(1);
        fake.set_failure(Some(failing(
            (0..130).map(|i| error_at("big.typ", i + 1, 1, "oops", "x")).collect(),
        )));
        let (view, mut visual) = open(cx, fake);
        until(&mut visual, "the errors show", |cx| failed(&view, cx));
        frames(&mut visual);
        assert!(has(&mut visual, "big.typ:100:1: oops"));
        assert!(!has(&mut visual, "big.typ:101:1: oops"));
        assert!(has(&mut visual, "and 30 more"));
    }

    #[gpui_kit::test]
    fn arabic_files_and_lines_are_listed_in_the_arabic_interface(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::a4(1);
        fake.set_failure(Some(failing(vec![error_at(
            "تقرير.typ",
            1,
            15,
            "unknown variable: هذا",
            "مرحبا بالعالم #هذا",
        )])));
        testing::set_rtl(true);
        let (view, mut visual) = open_entry(cx, file("C:\\docs", "تقرير.typ", 1000, 1), fake);
        until(&mut visual, "the errors show", |cx| failed(&view, cx));
        frames(&mut visual);
        assert_eq!(
            role_of(&mut visual, "تقرير.typ:1:15: unknown variable: هذا"),
            Some(Role::ListItem)
        );
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn warnings_are_counted_and_listed_on_request(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut fake = Fake::a4(1);
        let mut warning = error_at(
            "report.typ",
            1,
            1,
            "unknown font family: cascadia mono",
            "#set text(font: 1)",
        );
        warning.severity = Severity::Warning;
        fake.warnings = vec![warning];
        let (view, mut visual) = open(cx, fake);
        ready(&view, &mut visual);
        assert!(has(&mut visual, "1 warning"));
        assert!(!has(&mut visual, "Warnings"));
        click(&mut visual, "typst-warnings-toggle");
        assert_eq!(role_of(&mut visual, "Warnings"), Some(Role::List));
        assert!(has(&mut visual, "report.typ:1:1: unknown font family: cascadia mono"));
        click(&mut visual, "typst-warnings-toggle");
        assert!(!has(&mut visual, "Warnings"));
    }

    #[gpui_kit::test]
    fn a_newer_entry_typesets_again_and_keeps_the_pages_and_the_place_until_it_is_done(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let fake = Fake::a4(20);
        let (view, mut visual) = open(cx, fake.clone());
        ready(&view, &mut visual);
        view.update(&mut visual, |v, cx| {
            v.go_to_page(7, cx);
        });
        frames(&mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.current_page()), Some(7));
        // Saved: the document grew to 25 pages, and typesetting it takes a while.
        fake.set_pages(25);
        fake.set_slow(Duration::from_millis(400));
        visual.update(|window, cx| {
            let newer = file("C:\\docs", "report.typ", 1000, 2);
            view.update(cx, |v, cx| v.set_entry(newer, window, cx));
        });
        assert!(view.read_with(&visual, |v, _| v.is_compiling()));
        assert_eq!(state_of(&view, &visual), State::Ready);
        frames(&mut visual);
        assert!(has(&mut visual, "Typesetting again…"));
        assert!(has(&mut visual, "Page 7 of 20"), "the old pages stay while it works");
        until(&mut visual, "the new pages arrive", |cx| {
            view.read_with(cx, |v, _| v.page_count() == Some(25))
        });
        frames(&mut visual);
        assert!(!view.read_with(&visual, |v, _| v.is_compiling()));
        assert_eq!(
            view.read_with(&visual, |v, _| v.current_page()),
            Some(7),
            "the place stays"
        );
        assert_eq!(fake.runs(), 2);
        // The same entry again typesets nothing.
        visual.update(|window, cx| {
            let same = file("C:\\docs", "report.typ", 1000, 2);
            view.update(cx, |v, cx| v.set_entry(same, window, cx));
        });
        frames(&mut visual);
        assert_eq!(fake.runs(), 2);
        // Another file starts over.
        fake.set_slow(Duration::ZERO);
        visual.update(|window, cx| {
            let other = file("C:\\docs", "other.typ", 1000, 5);
            view.update(cx, |v, cx| v.set_entry(other, window, cx));
        });
        assert_eq!(state_of(&view, &visual), State::Opening);
        assert_eq!(view.read_with(&visual, |v, _| v.page_count()), None);
        ready(&view, &mut visual);
        assert_eq!(fake.runs(), 3);
        assert!(has(&mut visual, "Pages of other.typ"));
    }

    #[gpui_kit::test]
    fn a_failed_typeset_after_a_good_one_shows_the_errors(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let fake = Fake::a4(2);
        let (view, mut visual) = open(cx, fake.clone());
        ready(&view, &mut visual);
        fake.set_failure(Some(failing(vec![error_at("report.typ", 1, 1, "broken now", "#oops")])));
        visual.update(|window, cx| {
            view.update(cx, |v, cx| v.retypeset(window, cx));
        });
        until(&mut visual, "the errors replace the pages", |cx| failed(&view, cx));
        frames(&mut visual);
        assert!(has(&mut visual, "report.typ:1:1: broken now"));
        assert_eq!(view.read_with(&visual, |v, _| v.page_count()), None);
        assert!(view.read_with(&visual, |v, _| v.drawn_pages().is_empty()));
    }

    #[gpui_kit::test]
    fn a_save_of_the_file_or_of_something_it_read_is_noticed_without_the_frame(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let dir = Scratch::new("watch");
        let main = dir.file("main.typ", "= One\n");
        let part = dir.file("part.typ", "a part\n");
        let mut fake = Fake::a4(1);
        fake.dependencies = vec![part.clone()];
        let (view, mut visual) = open_entry(cx, entry_of(&main), fake.clone());
        ready(&view, &mut visual);
        assert_eq!(fake.runs(), 1);
        // Nothing changed: nothing is typeset, however long it waits.
        for _ in 0..3 {
            visual.executor().advance_clock(WATCH);
            until(&mut visual, "the check ran", |_| true);
        }
        assert_eq!(fake.runs(), 1);
        // The main file is saved.
        touch(&main, 30);
        until(&mut visual, "the save is noticed", |cx| {
            cx.executor().advance_clock(WATCH);
            fake.runs() >= 2 && view.read_with(cx, |v, _| !v.is_compiling())
        });
        let after_main = fake.runs();
        // A file it read is saved.
        touch(&part, 90);
        until(&mut visual, "the save of the part is noticed", |cx| {
            cx.executor().advance_clock(WATCH);
            fake.runs() > after_main && view.read_with(cx, |v, _| !v.is_compiling())
        });
    }

    #[gpui_kit::test]
    fn a_failed_document_is_typeset_again_when_it_is_fixed_and_saved(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let dir = Scratch::new("fix");
        let main = dir.file(
            "main.typ", "#oops
",
        );
        let fake = Fake::a4(1);
        fake.set_failure(Some(failing(vec![error_at(
            "main.typ",
            1,
            2,
            "unknown variable: oops",
            "#oops",
        )])));
        let (view, mut visual) = open_entry(cx, entry_of(&main), fake.clone());
        until(&mut visual, "the errors show", |cx| failed(&view, cx));
        assert_eq!(fake.runs(), 1);
        // Fixed and saved: the pages come without anyone asking.
        fake.set_failure(None);
        touch(&main, 30);
        until(&mut visual, "the fix is noticed", |cx| {
            cx.executor().advance_clock(WATCH);
            view.read_with(cx, |v, _| matches!(v.state(), State::Ready))
        });
        ready(&view, &mut visual);
        assert_eq!(fake.runs(), 2);
        assert!(view.read_with(&visual, |v, _| v.errors().is_empty()));
    }

    #[gpui_kit::test]
    fn leaving_the_file_stops_a_typeset_that_is_still_going(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut fake = Fake::a4(1);
        fake.until_stopped = true;
        let (view, mut visual) = open(cx, fake.clone());
        until(&mut visual, "typesetting began", |_| fake.runs() == 1);
        assert_eq!(fake.stopped.load(Ordering::SeqCst), 0);
        // Another file: the first compile is told to stop.
        visual.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.set_entry(file("C:\\docs", "other.typ", 1000, 1), window, cx)
            });
        });
        until(&mut visual, "the first typeset was stopped", |_| {
            fake.stopped.load(Ordering::SeqCst) >= 1
        });
    }

    #[gpui_kit::test]
    fn a_document_too_big_is_not_fed_to_the_compiler(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let fake = Fake::a4(1);
        let (view, mut visual) = open_entry(cx, file("C:\\docs", "huge.typ", MAX_BYTES + 1, 1), fake.clone());
        frames(&mut visual);
        assert_eq!(
            state_of(&view, &visual),
            State::Failed("Too big to typeset here: open it to read it.".into())
        );
        assert_eq!(fake.runs(), 0);
    }

    // ---- The view with Typst itself -------------------------------------------------------------------------

    fn open_real(cx: &mut TestAppContext, path: &Path) -> (Entity<TypstView>, VisualTestContext) {
        cx.update(testing::init);
        let entry = entry_of(path);
        let (visual, view) = testing::open_window(cx, move |window, cx| cx.new(|cx| TypstView::new(entry, window, cx)));
        (view, visual)
    }

    #[gpui_kit::test]
    fn a_real_document_is_typeset_and_its_pages_are_drawn(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let dir = Scratch::new("real");
        let path = dir.file(
            "paper.typ",
            "= A title\n\nSome words.\n\n#pagebreak()\n\nSecond page.\n",
        );
        let (view, mut visual) = open_real(cx, &path);
        until_for(&mut visual, 120, "the first page is drawn by Typst", |cx| {
            view.read_with(cx, |v, _| v.drawn_pages().contains(&1))
        });
        frames(&mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.page_count()), Some(2));
        assert!(has(&mut visual, "Pages of paper.typ"));
        assert!(has(&mut visual, "Page 1 of 2"));
        // The picture really is a page: white with ink on it.
        let image = view
            .read_with(&visual, |v, _| v.drawn.get(&0).map(|d| d.image.clone()))
            .unwrap_or_else(|| panic!("page 1 is drawn"));
        let bytes = image.as_bytes(0).unwrap_or(&[]).to_vec();
        assert!(bytes.len() > 4);
        assert_eq!(&bytes[..4], &[255, 255, 255, 255]);
        assert!(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[0] < 100 && p[1] < 100 && p[2] < 100),
            "ink on the page"
        );
    }

    #[gpui_kit::test]
    fn a_real_error_shows_its_line_and_a_package_says_it_cannot_go_online(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let dir = Scratch::new("real-errors");
        let broken = dir.file("broken.typ", "= Fine\n\n#nosuchfunction(1)\n");
        let (view, mut visual) = open_real(cx, &broken);
        until_for(&mut visual, 120, "Typst's errors show", |cx| failed(&view, cx));
        frames(&mut visual);
        let errors = view.read_with(&visual, |v, _| v.errors().to_vec());
        assert_eq!(errors[0].file.as_deref(), Some("broken.typ"));
        assert_eq!(errors[0].line, Some(3));
        // The identifier after the `#`.
        assert_eq!(errors[0].column, Some(2));
        assert!(has(&mut visual, &errors[0].to_string()));
        // A package import.
        let uses = dir.file("uses.typ", "#import \"@preview/cetz:0.3.4\": canvas\n");
        visual.update(|window, cx| {
            let entry = entry_of(&uses);
            view.update(cx, |v, cx| v.set_entry(entry, window, cx));
        });
        until_for(&mut visual, 60, "the package error shows", |cx| {
            view.read_with(cx, |v, _| v.errors().iter().any(|e| e.message.contains("package")))
        });
        let message = view.read_with(&visual, |v, _| v.errors()[0].to_string());
        assert!(message.contains("without going online"), "{message}");
        assert!(message.starts_with("uses.typ:1:"), "{message}");
    }

    #[gpui_kit::test]
    fn an_arabic_document_is_typeset_and_drawn_in_the_arabic_interface(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        let dir = Scratch::new("real-arabic");
        let path = dir.file(
            "تقرير.typ",
            "#set text(lang: \"ar\", font: (\"Segoe UI\", \"Arial\", \"Tahoma\"))\n= تقرير الرحلة\n\nمرحبا بالعالم، هذا نص عربي.\n",
        );
        let (view, mut visual) = open_real(cx, &path);
        until_for(&mut visual, 120, "the Arabic page is drawn", |cx| {
            view.read_with(cx, |v, _| v.drawn_pages().contains(&1))
        });
        frames(&mut visual);
        assert!(has(&mut visual, "Pages of تقرير.typ"));
        assert!(has(&mut visual, "Page 1 of 1"));
        testing::set_rtl(false);
    }
}
