//! PDF pages drawn natively with Windows.Data.Pdf (`PdfPreview` in FilePreview.tsx).
//!
//! The TypeScript handed the file to WebView2's own PDF viewer in an iframe (from a copy in memory that it first
//! checked was a PDF, and not past 256 MB). That viewer is gone with the WebView, so the pages are drawn here:
//! Windows' PDF renderer (Windows.Data.Pdf, the one Edge uses) draws each page to a bitmap that GPUI shows.
//!
//! | TypeScript                                       | Here                                                            |
//! |--------------------------------------------------|-----------------------------------------------------------------|
//! | `PdfPreview`, `tooBig`                           | [`PdfView`], [`MAX_BYTES`]: "Too big to preview here: open it to read it." |
//! | the `application/pdf` check on the copy          | [`WindowsPdf`] looks for `%PDF-` in the first kilobyte: "This file isn't a PDF inside, so it isn't shown." |
//! | the iframe's viewer: scrolling the pages         | pages in one scrolling column ([`Layout`]), the wheel, the scroll bar, dragging, Page Up/Down, Home/End |
//! | the viewer's zoom (Ctrl +, Ctrl -, Ctrl 0, Ctrl wheel), fit to width | the buttons, `+`, `-`, `0` and Ctrl+wheel; [`ZOOM_STEPS`]; fit width is the start |
//! | the viewer's page box                            | "Go to page" and the Previous and Next page buttons              |
//! | the viewer's password prompt                     | an inline form for a password-protected PDF (Windows' loader takes it) |
//! | the viewer's select, copy and find               | Copy text and Find in the toolbar, below                          |
//! | the viewer's error page                          | "This PDF is damaged, so it can't be shown."                      |
//! | `ArchivedMedia` (a temp copy)                    | the archive's copy, made by `Sources::extract`, as the picture preview does |
//!
//! # How a page gets on screen
//!
//! - **Windows' PDF objects live on a thread of their own** ([`work`]): the multithreaded apartment, where their
//!   async calls are waited for, as glint-core's thumbs.rs and ocr.rs read PDFs. The view talks to it through two
//!   channels and never waits: `Request::Want` (the pages it wants drawn, nearest first, replacing the last list)
//!   and `Reply`s (the document opened, the pages' sizes as they are read, a page drawn).
//! - **The renderer is a trait** ([`PdfEngine`], [`Document`]) so tests use a fake; [`WindowsPdf`] is the real one.
//! - **Pages are virtualized.** [`Layout`] knows every page's slot (the sizes are read in the background in
//!   batches, the first page's size standing in until then), the view draws the pages that intersect the box, asks
//!   for those within one box-height of it, and lets go of those more than two and a half box-heights away or
//!   past the [`CACHE_BUDGET`] (the farthest first). A page stays drawn at its old resolution, stretched, until
//!   the one for the new zoom arrives.
//! - **Resolution.** A page is drawn at its size on screen times the window's scale (sharp at 125% and 150%), kept
//!   under [`MAX_PAGE_PIXELS`]. NOTE: zoomed far past that (a poster at 400%) it is drawn softer than the screen
//!   is; Windows can draw a part of a page, and tiling the visible part is the way to fix it.
//! - **The scroll is the view's own** (the `Scroll` handle, also the scroll bar's), because the pages' places are
//!   known exactly and a zoom has to keep the page at the top of the box where it was.
//!
//! # Wiring (for the preview pane)
//!
//! - **What it replaces:** the `Some(PreviewKind::Pdf)` arm of `choose.rs` (`Choice::later(.., "pdf pages")`): a
//!   new `Body::Pdf` for a PDF on disk, and also for one in an archive (the view extracts the copy itself), where
//!   the arm now says `Body::Icon`. The frame's body wrapper scrolls vertically; this view fills it (`size_full`)
//!   and scrolls itself, as the notebook does.
//! - **Make it:** `cx.new(|cx| PdfView::new(entry.clone(), window, cx))`. A PDF is a view per file: make a new one
//!   when the selection moves (the pane already builds a body per entry).
//! - **Quick Look:** `page(down, cx)` scrolls a page (0.85 of the box) and returns whether it moved; add `PdfView`
//!   to `EntryView::page_body`'s list. Give it the keyboard with `window.focus(&view.focus_handle(cx), cx)` for the
//!   zoom keys.
//! - **Icons:** `ChevronUp`, `ChevronDown`, `ZoomIn`, `ZoomOut`, `MoveHorizontal`, `Copy`, `Search`, `X` are used;
//!   add them to the list in kit.rs's icon test (the test here, `the_icons_the_toolbar_draws_are_embedded`, checks
//!   them for now).
//!
//! # Not ported
//!
//! - **Text selection and the viewer's search are not part of a page picture.** A page is a bitmap, so there is
//!   nothing to select and nothing to highlight. What stands in: Copy text (the current page's text, read by
//!   glint-core's document reader, the one Documents search uses) and Find (which pages have the words, with
//!   Previous and Next to jump between them). A scanned page has no text to copy (core's OCR reads whole PDFs,
//!   not a page); that is said, not hidden.
//! - The viewer's Print, Save, Rotate, page thumbnails and outline, form filling, annotations and links: Open (in
//!   the header) hands the PDF to the app Windows has for it.
//! - Tiles for very high zooms (see above) and a zoom about the pointer: the zoom keeps the top of the box.

use super::sources::{sources, Ask};
use super::states::{big_icon, centered, sentence, Waiting};
use crate::kit::prelude::*;
use crate::{actions, t, theme};
use glint_model::entries::{is_archived, FileEntry};
use gpui_kit::{
    canvas, img, point, size, Bounds, ClipboardItem, FocusHandle, Focusable, KeyDownEvent, MouseButton, ObjectFit,
    RenderImage, ScrollEvent, StyledImage as _, Task,
};
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A PDF bigger than this isn't previewed (`tooBig`: 256 MB; the TypeScript held the whole file in memory).
pub const MAX_BYTES: u64 = 256 * 1024 * 1024;
/// The space around the pages, and between them, in pixels.
pub const PAD: f32 = 16.;
pub const GAP: f32 = 12.;
/// The zoom steps of the buttons and keys (Edge's viewer's, with room to see a whole sheet at the small end).
pub const ZOOM_STEPS: [f32; 19] = [
    0.1, 0.15, 0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1., 1.1, 1.25, 1.5, 1.75, 2., 2.5, 3., 4., 5.,
];
/// The zoom limits, and the biggest fit-to-width makes a small page.
pub const MIN_ZOOM: f32 = 0.05;
pub const MAX_ZOOM: f32 = 5.;
const MAX_FIT: f32 = 4.;
/// The most pixels one drawn page has (8 MP is 32 MB), so a zoomed-in page can't take the machine's memory.
pub const MAX_PAGE_PIXELS: u64 = 8_000_000;
/// All the drawn pages together are kept under this; the farthest from the box go first (never one on screen).
pub const CACHE_BUDGET: usize = 160 << 20;
/// Pages are asked for within this many box-heights of the box, and kept within [`KEEP`].
const NEAR: f32 = 1.;
const KEEP: f32 = 2.5;
/// What a wheel notch or an arrow key scrolls.
const LINE: f32 = 48.;
/// The size read pages' sizes for before they are sent in a batch.
const SIZE_BATCH: Duration = Duration::from_millis(25);
/// Letter, in device-independent pixels (96 to the inch), for a page whose size Windows can't tell.
const LETTER: PageSize = PageSize {
    width: 816.,
    height: 1056.,
};
/// A page is always white: the PDF is drawn for paper, so these are not the theme's.
const PAGE_WHITE: u32 = 0xff_ff_ff;
const PAGE_INK: u32 = 0x71_71_7a;

// ---- The renderer, behind a trait ---------------------------------------------------------------------------

/// A page's size in device-independent pixels (96 to the inch: Windows.Data.Pdf's unit, a point is 4/3 of one).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

/// A drawn page: premultiplied BGRA, as GPUI draws pictures.
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Why a PDF isn't shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// It needs a password to open; `wrong` when one was tried.
    Password { wrong: bool },
    /// Not a PDF inside, whatever the name says.
    NotPdf,
    /// Windows can't read it.
    Damaged,
    /// The file couldn't be read at all (the reason).
    Unreadable(String),
}

/// An open PDF. It lives, and is used, only on the thread that opened it ([`work`]).
pub trait Document {
    /// How many pages it has.
    fn page_count(&self) -> usize;
    /// Page `page`'s size (0 is the first), if it can be told.
    fn page_size(&mut self, page: usize) -> Option<PageSize>;
    /// Page `page` drawn `width` by `height` pixels, on white.
    fn render(&mut self, page: usize, width: u32, height: u32) -> Result<Bitmap, String>;
}

/// Where PDFs are opened: Windows' renderer in the app, a fake in tests. `open` runs on the worker thread.
pub trait PdfEngine: Send + Sync + 'static {
    /// Opens the PDF at `path`, with `password` when the person typed one.
    fn open(&self, path: &str, password: Option<&str>) -> Result<Box<dyn Document>, OpenError>;
}

/// Where a PDF's text comes from, for Copy text and Find: the text of each page.
pub trait PdfText: Send + Sync + 'static {
    /// One string a page, in order (a page without text is an empty string).
    fn pages(&self, path: &str) -> Result<Vec<String>, String>;
}

/// glint-core's document reader (pdf-extract in a worker process of its own: a hostile PDF can't take the app down).
pub struct CoreText;

impl PdfText for CoreText {
    fn pages(&self, path: &str) -> Result<Vec<String>, String> {
        let lines = glint_core::doctext::read_isolated(std::path::Path::new(path))?;
        Ok(pages_of(&lines))
    }
}

/// The lines of a document ("p. 3", the line) as one string a page. The labels are the reader's.
pub fn pages_of(lines: &[(String, String)]) -> Vec<String> {
    let mut pages: Vec<String> = Vec::new();
    for (label, text) in lines {
        let Some(page) = label
            .strip_prefix("p. ")
            .and_then(|n| n.trim().parse::<usize>().ok())
            .filter(|&n| n >= 1)
        else {
            continue;
        };
        if pages.len() < page {
            pages.resize(page, String::new());
        }
        let slot = &mut pages[page - 1];
        if !slot.is_empty() {
            slot.push('\n');
        }
        slot.push_str(text);
    }
    pages
}

// ---- Windows' renderer --------------------------------------------------------------------------------------

/// Windows.Data.Pdf: the renderer Edge uses, built into Windows.
pub struct WindowsPdf;

/// ERROR_WRONG_PASSWORD as an HRESULT: what Windows answers when a PDF needs a password, or the one tried is wrong.
const WRONG_PASSWORD: i32 = 0x8007_052B_u32 as i32;

impl PdfEngine for WindowsPdf {
    fn open(&self, path: &str, password: Option<&str>) -> Result<Box<dyn Document>, OpenError> {
        use windows::core::HSTRING;
        use windows::Data::Pdf::PdfDocument;
        use windows::Storage::StorageFile;
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
        // SAFETY: a plain COM call on this thread; it fails harmlessly when the thread is already initialized.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        match looks_like_pdf(path) {
            Ok(true) => {}
            Ok(false) => return Err(OpenError::NotPdf),
            Err(e) => return Err(OpenError::Unreadable(e.to_string())),
        }
        let opened = (|| -> windows::core::Result<PdfDocument> {
            let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path))?.join()?;
            match password {
                Some(password) => PdfDocument::LoadFromFileWithPasswordAsync(&file, &HSTRING::from(password))?.join(),
                None => PdfDocument::LoadFromFileAsync(&file)?.join(),
            }
        })();
        match opened {
            Ok(document) => Ok(Box::new(WindowsDocument { document })),
            Err(e) if e.code().0 == WRONG_PASSWORD => Err(OpenError::Password {
                wrong: password.is_some(),
            }),
            Err(_) => Err(OpenError::Damaged),
        }
    }
}

/// Whether the first kilobyte holds `%PDF-` (the header may follow some junk, within that).
fn looks_like_pdf(path: &str) -> std::io::Result<bool> {
    use std::io::Read;
    let mut head = Vec::with_capacity(1024);
    std::fs::File::open(path)?.take(1024).read_to_end(&mut head)?;
    Ok(head.windows(5).any(|w| w == b"%PDF-"))
}

struct WindowsDocument {
    document: windows::Data::Pdf::PdfDocument,
}

impl Document for WindowsDocument {
    fn page_count(&self) -> usize {
        self.document.PageCount().map_or(0, |n| n as usize)
    }

    fn page_size(&mut self, page: usize) -> Option<PageSize> {
        let size = self.document.GetPage(u32::try_from(page).ok()?).ok()?.Size().ok()?;
        (size.Width > 0. && size.Height > 0.).then_some(PageSize {
            width: size.Width,
            height: size.Height,
        })
    }

    fn render(&mut self, page: usize, width: u32, height: u32) -> Result<Bitmap, String> {
        render_page(&self.document, page, width, height).map_err(|e| e.message())
    }
}

/// Page `page` as BGRA pixels: drawn to a BMP in memory (no compression to undo), decoded to a bitmap, copied out.
fn render_page(
    document: &windows::Data::Pdf::PdfDocument,
    page: usize,
    width: u32,
    height: u32,
) -> windows::core::Result<Bitmap> {
    use windows::Data::Pdf::PdfPageRenderOptions;
    use windows::Graphics::Imaging::{
        BitmapAlphaMode, BitmapDecoder, BitmapEncoder, BitmapPixelFormat, SoftwareBitmap,
    };
    use windows::Storage::Streams::{Buffer, DataReader, InMemoryRandomAccessStream};
    let page = document.GetPage(
        u32::try_from(page).map_err(|_| windows::core::Error::from(windows::Win32::Foundation::E_INVALIDARG))?,
    )?;
    let options = PdfPageRenderOptions::new()?;
    options.SetDestinationWidth(width.max(1))?;
    options.SetDestinationHeight(height.max(1))?;
    options.SetBackgroundColor(windows::UI::Color {
        A: 255,
        R: 255,
        G: 255,
        B: 255,
    })?;
    options.SetBitmapEncoderId(BitmapEncoder::BmpEncoderId()?)?;
    let stream = InMemoryRandomAccessStream::new()?;
    page.RenderWithOptionsToStreamAsync(&stream, &options)?.join()?;
    stream.Seek(0)?;
    let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
    let bitmap = decoder.GetSoftwareBitmapAsync()?.join()?;
    let bitmap = SoftwareBitmap::ConvertWithAlpha(&bitmap, BitmapPixelFormat::Bgra8, BitmapAlphaMode::Premultiplied)?;
    let (width, height) = (bitmap.PixelWidth()? as u32, bitmap.PixelHeight()? as u32);
    let length = width.saturating_mul(height).saturating_mul(4);
    let buffer = Buffer::Create(length)?;
    bitmap.CopyToBuffer(&buffer)?;
    let mut bgra = vec![0u8; length as usize];
    DataReader::FromBuffer(&buffer)?.ReadBytes(&mut bgra)?;
    Ok(Bitmap { width, height, bgra })
}

// ---- The worker thread --------------------------------------------------------------------------------------

/// A page wanted, and the pixel size to draw it at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Want {
    pub page: usize,
    pub width: u32,
    pub height: u32,
}

enum Request {
    /// The pages to draw now, most wanted first: replaces the list from before.
    Want(Vec<Want>),
}

/// What the worker says.
enum Reply {
    Opened {
        pages: usize,
        first: PageSize,
    },
    Failed(OpenError),
    /// The sizes of pages `from..`.
    Sizes {
        from: usize,
        sizes: Vec<PageSize>,
    },
    Page {
        want: Want,
        image: Result<Arc<RenderImage>, String>,
    },
}

/// The worker and the way to it. Dropping it ends the worker (its channel closes).
struct Session {
    requests: async_channel::Sender<Request>,
}

impl Session {
    fn start(
        engine: Arc<dyn PdfEngine>,
        path: String,
        password: Option<String>,
    ) -> (Session, async_channel::Receiver<Reply>) {
        let (requests, wanted) = async_channel::unbounded();
        let (reply, replies) = async_channel::unbounded();
        // If the thread can't start, `replies` closes unanswered: the view says the PDF can't be read.
        let _ = std::thread::Builder::new().name("glint-pdf".into()).spawn(move || {
            work(engine.as_ref(), &path, password.as_deref(), &wanted, &reply);
        });
        (Session { requests }, replies)
    }

    fn want(&self, list: Vec<Want>) {
        let _ = self.requests.try_send(Request::Want(list));
    }
}

/// The worker: opens the PDF, tells how many pages and how big, reads the other pages' sizes between requests,
/// and draws what is wanted. Ends when the view lets go of the session.
fn work(
    engine: &dyn PdfEngine,
    path: &str,
    password: Option<&str>,
    wanted: &async_channel::Receiver<Request>,
    replies: &async_channel::Sender<Reply>,
) {
    let mut document = match engine.open(path, password) {
        Ok(document) => document,
        Err(e) => {
            let _ = replies.send_blocking(Reply::Failed(e));
            return;
        }
    };
    let count = document.page_count();
    let first = if count > 0 { document.page_size(0) } else { None }.unwrap_or(LETTER);
    if replies.send_blocking(Reply::Opened { pages: count, first }).is_err() {
        return;
    }
    let mut next_size = 1;
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
            let image = document
                .render(want.page, want.width, want.height)
                .and_then(|bitmap| image_of(bitmap).ok_or_else(|| "the page's pixels didn't fit its size".to_string()));
            if replies.send_blocking(Reply::Page { want, image }).is_err() {
                return;
            }
            continue;
        }
        if next_size < count {
            let started = Instant::now();
            let from = next_size;
            let mut sizes = Vec::new();
            while next_size < count && sizes.len() < 2048 && started.elapsed() < SIZE_BATCH {
                sizes.push(document.page_size(next_size).unwrap_or(first));
                next_size += 1;
                // A request waiting goes first; the rest of the sizes follow.
                if !wanted.is_empty() {
                    break;
                }
            }
            if replies.send_blocking(Reply::Sizes { from, sizes }).is_err() {
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

/// A drawn page as GPUI's picture type (no copy: the pixels are the picture's).
fn image_of(bitmap: Bitmap) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(bitmap.width, bitmap.height, bitmap.bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

// ---- Where the pages are ------------------------------------------------------------------------------------

/// The pages' places in the scrolling column at one zoom: pure arithmetic, no window.
///
/// Sizes not read yet are the first page's. Every place is in pixels from the top of the column, which starts with
/// [`PAD`] and puts [`GAP`] between pages.
#[derive(Clone, Debug)]
pub struct Layout {
    sizes: Vec<Option<PageSize>>,
    estimate: PageSize,
    zoom: f32,
    tops: Vec<f32>,
    end: f32,
    widest: f32,
}

impl Layout {
    /// `count` pages, the first `first` big, at zoom 1.
    pub fn new(count: usize, first: PageSize) -> Self {
        let mut sizes = vec![None; count];
        if let Some(slot) = sizes.first_mut() {
            *slot = Some(first);
        }
        let mut layout = Layout {
            sizes,
            estimate: first,
            zoom: 1.,
            tops: Vec::new(),
            end: 0.,
            widest: first.width,
        };
        layout.rebuild(1.);
        layout
    }

    /// How many pages.
    pub fn len(&self) -> usize {
        self.sizes.len()
    }

    /// No pages.
    pub fn is_empty(&self) -> bool {
        self.sizes.is_empty()
    }

    /// The zoom the places are worked out for.
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// Notes the sizes of pages `from..`; true when any place moved (a size that is the estimate moves nothing).
    pub fn set_sizes(&mut self, from: usize, sizes: &[PageSize]) -> bool {
        let mut changed = false;
        for (k, size) in sizes.iter().enumerate() {
            if let Some(slot) = self.sizes.get_mut(from + k) {
                changed |= *size != slot.unwrap_or(self.estimate);
                *slot = Some(*size);
            }
        }
        if changed {
            self.rebuild(self.zoom);
        }
        changed
    }

    /// Works the places out again at `zoom`.
    pub fn rebuild(&mut self, zoom: f32) {
        self.zoom = zoom;
        self.widest = self.sizes.iter().flatten().map(|s| s.width).fold(0., f32::max).max(
            if self.sizes.iter().any(Option::is_some) {
                0.
            } else {
                self.estimate.width
            },
        );
        self.tops.clear();
        self.tops.reserve(self.sizes.len());
        let mut y = PAD;
        for i in 0..self.sizes.len() {
            self.tops.push(y);
            y += self.size_of(i).height * zoom + GAP;
        }
        self.end = if self.sizes.is_empty() { 2. * PAD } else { y - GAP + PAD };
    }

    /// Page `i`'s size at zoom 1 (the estimate until it is read).
    pub fn size_of(&self, i: usize) -> PageSize {
        self.sizes.get(i).copied().flatten().unwrap_or(self.estimate)
    }

    /// Page `i`'s width and height on screen.
    pub fn extent(&self, i: usize) -> (f32, f32) {
        let s = self.size_of(i);
        (s.width * self.zoom, s.height * self.zoom)
    }

    /// The top of page `i` in the column.
    pub fn top(&self, i: usize) -> f32 {
        self.tops.get(i).copied().unwrap_or(self.end)
    }

    /// The column's height.
    pub fn height(&self) -> f32 {
        self.end
    }

    /// The column's width when the box is `viewport` wide: the widest page and its margins, at least the box.
    pub fn width(&self, viewport: f32) -> f32 {
        (self.widest * self.zoom + 2. * PAD).max(viewport)
    }

    /// The zoom at which the widest page fills a box `viewport` wide (never bigger than [`MAX_FIT`]).
    pub fn fit_zoom(&self, viewport: f32) -> f32 {
        if self.widest <= 0. || viewport <= 2. * PAD {
            return 1.;
        }
        ((viewport - 2. * PAD) / self.widest).clamp(MIN_ZOOM, MAX_FIT)
    }

    /// The pages that intersect the span `from..to` of the column.
    pub fn visible(&self, from: f32, to: f32) -> Range<usize> {
        let n = self.sizes.len();
        // The first page whose bottom is below `from`; the first whose top is at or below `to`.
        let first = partition(n, |i| self.top(i) + self.extent(i).1 <= from);
        let last = partition(n, |i| self.top(i) < to);
        first..last.max(first)
    }

    /// The page at `y`: the one whose slot (with half the gap either side) holds it; the first or last at the ends.
    pub fn page_at(&self, y: f32) -> usize {
        let n = self.sizes.len();
        if n == 0 {
            return 0;
        }
        partition(n, |i| self.top(i) + self.extent(i).1 + GAP / 2. <= y).min(n - 1)
    }

    /// Where `y` is, as a page and how far down it (in pages' heights, so it survives a zoom).
    pub fn anchor(&self, y: f32) -> Anchor {
        let page = self.page_at(y);
        let height = self.extent(page).1.max(1.);
        Anchor {
            page,
            fraction: (y - self.top(page)) / height,
        }
    }

    /// The `y` an anchor is at now.
    pub fn place(&self, anchor: Anchor) -> f32 {
        self.top(anchor.page) + anchor.fraction * self.extent(anchor.page).1
    }
}

/// The first index in `0..n` for which `before` is false (`before` is true for a prefix).
fn partition(n: usize, before: impl Fn(usize) -> bool) -> usize {
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if before(mid) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// A place in the column that stays put through a zoom: a page and a fraction of its height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub page: usize,
    pub fraction: f32,
}

/// The next zoom step above `zoom` (a hair over, so a step you are on is passed).
pub fn zoom_in(zoom: f32) -> f32 {
    ZOOM_STEPS
        .iter()
        .copied()
        .find(|&s| s > zoom * 1.01)
        .unwrap_or(MAX_ZOOM.max(zoom))
}

/// The next zoom step below `zoom`.
pub fn zoom_out(zoom: f32) -> f32 {
    ZOOM_STEPS
        .iter()
        .rev()
        .copied()
        .find(|&s| s < zoom / 1.01)
        .unwrap_or(zoom.min(ZOOM_STEPS[0]))
}

/// The pixels to draw a page that is `width` by `height` on screen: times the window's `scale`, and cut down (the
/// shape kept) to [`MAX_PAGE_PIXELS`].
pub fn device_size(width: f32, height: f32, scale: f32) -> (u32, u32) {
    let (w, h) = ((width * scale).round().max(1.), (height * scale).round().max(1.));
    let pixels = f64::from(w) * f64::from(h);
    let cap = MAX_PAGE_PIXELS as f64;
    let k = if pixels > cap { (cap / pixels).sqrt() } else { 1. };
    (
        ((f64::from(w) * k).floor().max(1.)) as u32,
        ((f64::from(h) * k).floor().max(1.)) as u32,
    )
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
    /// Being opened.
    Opening,
    /// Needs a password (`wrong`: the one tried didn't open it).
    Password { wrong: bool },
    /// Can't be shown (the sentence).
    Failed(SharedString),
    /// Open: the pages are drawn.
    Ready,
}

/// A page drawn, and the size it was drawn at.
struct Drawn {
    image: Arc<RenderImage>,
    want: (u32, u32),
    bytes: usize,
}

/// The pages' text, once read.
enum Texts {
    Unread,
    Reading,
    Ready(Arc<Vec<String>>),
    Failed(SharedString),
}

/// What to do once the text is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Then {
    Copy,
    Find,
}

/// How the person asked for the zoom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    /// Fit to width: follows the box.
    Fit,
    /// A step (or the fit, once changed): 1 is 96 dots to the inch.
    Factor(f32),
}

/// A PDF's pages, drawn by Windows and shown in a scrolling column with zoom, a page box, Copy text and Find.
pub struct PdfView {
    entry: FileEntry,
    engine: Arc<dyn PdfEngine>,
    text: Arc<dyn PdfText>,
    extract: Option<Ask<Result<String, String>>>,
    state: State,
    waiting: Entity<Waiting>,
    session: Option<Session>,
    /// The path the worker opens (the archive's copy, when it is in one).
    path: String,
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
    focus: FocusHandle,
    go_to: Entity<InputState>,
    password: Entity<InputState>,
    password_busy: bool,
    drag: Option<(Point<Pixels>, (f32, f32))>,
    texts: Texts,
    then: Option<Then>,
    finding: bool,
    find: Entity<InputState>,
    matches: Vec<usize>,
    _task: Option<Task<()>>,
    _pump: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl PdfView {
    /// The pages of the PDF `entry`, drawn by Windows. Opening starts at once, off the UI thread.
    pub fn new(entry: FileEntry, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let extract = is_archived(&entry).then(|| sources(cx).extract.clone());
        Self::with_engine(entry, Arc::new(WindowsPdf), Arc::new(CoreText), extract, window, cx)
    }

    /// [`PdfView::new`] over `engine` and `text` (tests pass fakes); `extract` copies a file out of an archive.
    pub fn with_engine(
        entry: FileEntry,
        engine: Arc<dyn PdfEngine>,
        text: Arc<dyn PdfText>,
        extract: Option<Ask<Result<String, String>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let waiting = cx.new(|cx| Waiting::new(extract.as_ref().map(|_| t!("Extracting from the archive…")), cx));
        let go_to = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Page")));
        let find = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Find in this PDF")));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder(t!("Password")));
        let subscriptions = vec![
            cx.subscribe_in(&go_to, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.go_from_box(window, cx);
                }
            }),
            cx.subscribe_in(&find, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.find_step(true, window, cx),
                InputEvent::Change => this.search(cx),
                _ => {}
            }),
            cx.subscribe_in(
                &password,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.submit_password(window, cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ),
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
        let mut view = PdfView {
            path: entry.path.clone(),
            entry,
            engine,
            text,
            extract,
            state: State::Opening,
            waiting,
            session: None,
            layout: None,
            zoom: Zoom::Fit,
            scroll: Scroll::default(),
            drawn: HashMap::new(),
            failed: HashMap::new(),
            asked: Vec::new(),
            retired: Vec::new(),
            focus: cx.focus_handle(),
            go_to,
            password,
            password_busy: false,
            drag: None,
            texts: Texts::Unread,
            then: None,
            finding: false,
            find,
            matches: Vec::new(),
            _task: None,
            _pump: None,
            _subscriptions: subscriptions,
        };
        view.start(window, cx);
        view
    }

    /// The file shown.
    pub fn entry(&self) -> &FileEntry {
        &self.entry
    }

    /// What the view is doing: opening, asking for a password, failed, or showing pages.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// How many pages the PDF has, once it is open.
    pub fn page_count(&self) -> Option<usize> {
        self.layout.as_ref().map(Layout::len)
    }

    /// The page at the middle of the box (1 is the first), once open.
    pub fn current_page(&self) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let state = self.scroll.get();
        let (_, h) = self.scroll.viewport();
        Some(layout.page_at(state.y + h / 2.) + 1)
    }

    /// The zoom now (1 is 96 dots to the inch, the PDF at its own size).
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

    /// The pages (1 is the first) that Find found words on.
    pub fn found_on(&self) -> Vec<usize> {
        self.matches.iter().map(|p| p + 1).collect()
    }

    /// Where the scroll is: pixels from the top of the first page's margin.
    pub fn scrolled(&self) -> f32 {
        self.scroll.get().y
    }

    // ---- Opening ------------------------------------------------------------------------------------------

    /// Starts over: the archive's copy first, then the worker.
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.entry.size > MAX_BYTES {
            self.state = State::Failed(t!("Too big to preview here: open it to read it."));
            return;
        }
        self.state = State::Opening;
        match self.extract.clone() {
            Some(extract) => {
                let running = extract(&self.entry.path);
                self._task = Some(cx.spawn_in(window, async move |this, cx| {
                    let copy = running.await;
                    this.update_in(cx, |this, window, cx| match copy {
                        Ok(path) => {
                            this.path = path;
                            this.open(None, window, cx);
                        }
                        Err(reason) => this.fail(reason.into(), cx),
                    })
                    .ok();
                }));
            }
            None => self.open(None, window, cx),
        }
    }

    /// Opens the PDF on a worker thread with `password`, and listens for what it says.
    fn open(&mut self, password: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let (session, replies) = Session::start(self.engine.clone(), self.path.clone(), password);
        self.session = Some(session);
        self.asked.clear();
        self._pump = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(reply) = replies.recv().await {
                if this
                    .update_in(cx, |this, window, cx| this.heard(reply, window, cx))
                    .is_err()
                {
                    return;
                }
            }
            // The worker ended without opening it (it could not start, or died).
            this.update_in(cx, |this, _, cx| {
                if this.state == State::Opening {
                    this.fail(t!("This PDF is damaged, so it can't be shown."), cx);
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn fail(&mut self, note: SharedString, cx: &mut Context<Self>) {
        self.state = State::Failed(note);
        self.session = None;
        self.password_busy = false;
        cx.notify();
    }

    fn heard(&mut self, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            Reply::Opened { pages, first } => {
                self.password_busy = false;
                self.password.update(cx, |input, cx| input.set_value("", window, cx));
                if pages == 0 {
                    self.fail(t!("This PDF has no pages."), cx);
                    return;
                }
                self.layout = Some(Layout::new(pages, first));
                self.scroll.edit(|s| {
                    s.x = 0.;
                    s.y = 0.;
                });
                // Not focused: the file list keeps the keyboard while the selection moves; a click on the pages takes it.
                self.state = State::Ready;
            }
            Reply::Failed(error) => {
                self.session = None;
                self.password_busy = false;
                match error {
                    OpenError::Password { wrong } => self.state = State::Password { wrong },
                    OpenError::NotPdf => self.fail(t!("This file isn't a PDF inside, so it isn't shown."), cx),
                    OpenError::Damaged => self.fail(t!("This PDF is damaged, so it can't be shown."), cx),
                    OpenError::Unreadable(reason) => {
                        self.fail(t!("Couldn't read the PDF: {reason}", reason = reason), cx)
                    }
                }
            }
            Reply::Sizes { from, sizes } => {
                let anchor = self.anchor();
                if let Some(layout) = self.layout.as_mut() {
                    if layout.set_sizes(from, &sizes) {
                        self.relayout(anchor);
                    }
                }
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

    // ---- Where things are -------------------------------------------------------------------------------

    /// The place at the top of the box, as a page and a fraction.
    fn anchor(&self) -> Option<Anchor> {
        let layout = self.layout.as_ref()?;
        Some(layout.anchor(self.scroll.get().y))
    }

    /// Puts the scroll back at `anchor` after the places moved, and sets the column's size.
    fn relayout(&mut self, anchor: Option<Anchor>) {
        let Some(layout) = self.layout.as_ref() else { return };
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
            let Some(layout) = self.layout.as_mut() else { return };
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
        let Some(layout) = self.layout.as_ref() else { return };
        let top = self.scroll.get().y;
        let scale = window.scale_factor();
        let near = layout.visible(top - NEAR * vh, top + vh + NEAR * vh);
        let keep = layout.visible(top - KEEP * vh, top + vh + KEEP * vh);
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
                // `*p`: a `&usize` keeps `contains` unambiguous once another crate adds `PartialOrd<_> for usize`.
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
        // The pages' own keys: not for what is typed in the page box or the Find box under them.
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
            ("f", _) if keystroke.modifiers.control || keystroke.modifiers.platform => {
                self.toggle_find(window, cx);
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
        let Some((from, (x, y))) = self.drag else { return };
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

    // ---- The password ----------------------------------------------------------------------------------

    fn submit_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.password.read(cx).value().to_string();
        if typed.is_empty() || self.password_busy || !matches!(self.state, State::Password { .. }) {
            return;
        }
        self.password_busy = true;
        self.open(Some(typed), window, cx);
    }

    // ---- Copy text and Find ------------------------------------------------------------------------------

    /// Reads the pages' text in the background, then does `then`.
    fn read_text(&mut self, then: Then, window: &mut Window, cx: &mut Context<Self>) {
        self.then = Some(then);
        match &self.texts {
            Texts::Ready(_) => return self.text_ready(window, cx),
            Texts::Reading => return,
            Texts::Unread | Texts::Failed(_) => {}
        }
        self.texts = Texts::Reading;
        cx.notify();
        let text = self.text.clone();
        let path = self.path.clone();
        cx.spawn_in(window, async move |this, cx| {
            let read = cx.background_spawn(async move { text.pages(&path) }).await;
            this.update_in(cx, |this, window, cx| {
                this.texts = match read {
                    Ok(pages) => Texts::Ready(Arc::new(pages)),
                    Err(reason) => Texts::Failed(reason.into()),
                };
                this.text_ready(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn text_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let then = self.then.take();
        match (&self.texts, then) {
            (Texts::Failed(reason), Some(_)) => {
                let note = Notification::error(reason.clone()).title(t!("Couldn't read the text"));
                window.push_notification(note, cx);
            }
            (Texts::Ready(pages), Some(Then::Copy)) => {
                let page = self.current_page().unwrap_or(1);
                let text = pages.get(page - 1).map(|s| s.trim().to_string()).unwrap_or_default();
                if text.is_empty() {
                    window.push_notification(
                        Notification::info(t!("No text on this page: it may be a scan, which is only a picture")),
                        cx,
                    );
                } else {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    window.push_notification(Notification::success(t!("Copied the text of page {n}", n = page)), cx);
                }
            }
            (Texts::Ready(_), Some(Then::Find)) => self.search(cx),
            _ => {}
        }
    }

    /// Copy text: the text of the page in the middle of the box.
    pub fn copy_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.layout.is_some() {
            self.read_text(Then::Copy, window, cx);
        }
    }

    fn toggle_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finding = !self.finding;
        if self.finding {
            self.read_text(Then::Find, window, cx);
            self.find.update(cx, |input, cx| input.focus(window, cx));
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// Finds the words in the pages' text: which pages have them (case ignored).
    fn search(&mut self, cx: &mut Context<Self>) {
        let query = self.find.read(cx).value().trim().to_lowercase();
        self.matches = match &self.texts {
            Texts::Ready(pages) if !query.is_empty() => pages
                .iter()
                .enumerate()
                .filter(|(_, text)| text.to_lowercase().contains(&query))
                .map(|(i, _)| i)
                .collect(),
            _ => Vec::new(),
        };
        cx.notify();
    }

    /// Goes to the next (or previous) page that has the words, after the page in view, round to the first.
    fn find_step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.texts, Texts::Unread | Texts::Failed(_)) {
            return self.read_text(Then::Find, window, cx);
        }
        self.search(cx);
        let here = self.current_page().unwrap_or(1) - 1;
        let target = if forward {
            self.matches
                .iter()
                .copied()
                .find(|&p| p > here)
                .or(self.matches.first().copied())
        } else {
            self.matches
                .iter()
                .rev()
                .copied()
                .find(|&p| p < here)
                .or(self.matches.last().copied())
        };
        if let Some(page) = target {
            self.go_to_page(page + 1, cx);
        }
    }
}

impl Focusable for PdfView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

// ---- Drawing --------------------------------------------------------------------------------------------------

impl Render for PdfView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let root = col()
            .id("pdf-view")
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
            State::Failed(note) => return root.child(big_icon(&self.entry, Some(note), None, cx)),
            State::Password { wrong } => return root.child(self.password_card(wrong, &t, cx)),
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
                    .id(("pdf-page", i))
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
        let area = div()
            .id("pdf-pages")
            .role(Role::List)
            .aria_label(t!("Pages of {name}", name = self.entry.name.clone()))
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

        let find = self.finding.then(|| self.find_row(&t, cx));
        root.children(find).child(area).child(self.toolbar(&t, cx))
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

impl PdfView {
    /// A PDF that needs a password: the sentence and the form (the viewer asked for one itself).
    fn password_card(&self, wrong: bool, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let line = t!("This file is password-protected.");
        let empty = self.password.read(cx).value().is_empty();
        let form = col()
            .id("pdf-password")
            .w_full()
            .max_w(space(72.))
            .gap(space(1.5))
            .text_size(theme::size::SM)
            .text_start()
            .child(
                align_row(1.5)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.password).aria_label(t!("Password"))),
                    )
                    .child(
                        Button::new("pdf-password-open")
                            .primary()
                            .label(t!("Open"))
                            .when(!self.password_busy, |b| b.icon(IconName::KeyRound))
                            .loading(self.password_busy)
                            .disabled(empty || self.password_busy)
                            .on_click(cx.listener(|this, _, window, cx| this.submit_password(window, cx))),
                    ),
            )
            .when(wrong, |form| {
                let note = t!("That password didn't open it.");
                form.child(
                    div()
                        .id("pdf-password-error")
                        .role(Role::Alert)
                        .aria_label(note.clone())
                        .test_support()
                        .text_size(theme::size::XS)
                        .text_color(t.destructive)
                        .child(note),
                )
            });
        centered(cx)
            .id("pdf-locked")
            .role(Role::Group)
            .aria_label(line.clone())
            .test_support()
            .child(
                icon(IconName::FileLock)
                    .size(px(56.))
                    .flex_none()
                    .text_color(t.muted_foreground),
            )
            .child(sentence(line))
            .child(div().w_full().flex().justify_center().child(form))
    }

    /// Find: the box, Previous and Next, and what was found.
    fn find_row(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.find.read(cx).value().trim().to_string();
        let status: SharedString = match &self.texts {
            Texts::Reading | Texts::Unread => t!("Reading the text…"),
            Texts::Failed(reason) => reason.clone(),
            Texts::Ready(_) if query.is_empty() => t!("Type the words to look for"),
            Texts::Ready(_) if self.matches.is_empty() => t!("Not found"),
            Texts::Ready(_) if self.matches.len() == 1 => t!("On 1 page"),
            Texts::Ready(_) => t!("On {n} pages", n = self.matches.len()),
        };
        let step = |id: &'static str, glyph: IconName, label: SharedString, forward: bool, cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .small()
                .icon(glyph)
                .accessibility_label(label.clone())
                .tooltip(label)
                .disabled(self.matches.is_empty())
                .on_click(cx.listener(move |this, _, window, cx| this.find_step(forward, window, cx)))
        };
        let previous = step(
            "pdf-find-previous",
            IconName::ChevronUp,
            t!("Previous match"),
            false,
            cx,
        );
        let next = step("pdf-find-next", IconName::ChevronDown, t!("Next match"), true, cx);
        row_start()
            .id("pdf-find")
            .role(Role::Group)
            .aria_label(t!("Find bar"))
            .test_support()
            .flex_none()
            .gap(space(1.5))
            .px(space(3.))
            .py(space(1.5))
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .child(
                div()
                    .w(space(48.))
                    .flex_none()
                    .child(Input::new(&self.find).small().aria_label(t!("Find in this PDF"))),
            )
            .child(previous)
            .child(next)
            .child(
                div()
                    .id("pdf-find-status")
                    .role(Role::Status)
                    .aria_label(status.clone())
                    .test_support()
                    .min_w_0()
                    .truncate()
                    .text_color(t.muted_foreground)
                    .child(status),
            )
    }

    /// The row under the pages: the page box, Previous and Next page, zoom, Find and Copy text.
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
            "pdf-previous",
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
            "pdf-next",
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
        let zoom_out = small("pdf-zoom-out", IconName::ZoomOut, t!("Zoom out"), t!("Zoom out (-)"))
            .on_click(cx.listener(|this, _, _, cx| this.zoom_out(cx)));
        let zoom_in = small("pdf-zoom-in", IconName::ZoomIn, t!("Zoom in"), t!("Zoom in (+)"))
            .on_click(cx.listener(|this, _, _, cx| this.zoom_in(cx)));
        let fit = small(
            "pdf-fit",
            IconName::MoveHorizontal,
            t!("Fit width"),
            t!("Fit the page to the width of the pane (0)"),
        )
        .selected(self.is_fit())
        .on_click(cx.listener(|this, _, _, cx| this.fit_width(cx)));
        let find = small(
            "pdf-find-toggle",
            IconName::Search,
            t!("Find"),
            t!("Find words: which pages have them (a page is a picture, so they aren't marked on it)"),
        )
        .selected(self.finding)
        .on_click(cx.listener(|this, _, window, cx| this.toggle_find(window, cx)));
        let copy = small(
            "pdf-copy-text",
            if matches!(self.texts, Texts::Reading) && self.then == Some(Then::Copy) {
                IconName::LoaderCircle
            } else {
                IconName::Copy
            },
            t!("Copy text"),
            t!("Copy the text of this page (a page is a picture, so it can't be selected on it)"),
        )
        .on_click(cx.listener(|this, _, window, cx| this.copy_text(window, cx)));

        row_start()
            .id("pdf-tools")
            .role(Role::Toolbar)
            .aria_label(t!("PDF tools"))
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
                    .id("pdf-page-status")
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
                    .id("pdf-zoom-level")
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
            .child(find)
            .child(copy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::file;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{ScrollDelta, TestAppContext, VisualTestContext};
    use std::sync::Mutex;

    // ---- Fakes ------------------------------------------------------------------------------------------

    /// A fake renderer: pages of the sizes given, a password if one is set, and a log of what was drawn.
    #[derive(Clone)]
    struct Fake {
        pages: Vec<PageSize>,
        password: Option<&'static str>,
        refuse: Option<OpenError>,
        broken: Vec<usize>,
        /// How long opening takes, to see that the view doesn't wait for it.
        slow: Duration,
        drawn: Arc<Mutex<Vec<(usize, u32, u32)>>>,
        opened: Arc<Mutex<Vec<Option<String>>>>,
    }

    struct FakeDocument {
        fake: Fake,
    }

    impl Fake {
        fn letter(pages: usize) -> Self {
            Fake {
                pages: vec![LETTER; pages],
                password: None,
                refuse: None,
                broken: Vec::new(),
                slow: Duration::ZERO,
                drawn: Arc::default(),
                opened: Arc::default(),
            }
        }

        fn drawn(&self) -> Vec<(usize, u32, u32)> {
            self.drawn.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }

        fn tried(&self) -> Vec<Option<String>> {
            self.opened.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    impl PdfEngine for Fake {
        fn open(&self, _: &str, password: Option<&str>) -> Result<Box<dyn Document>, OpenError> {
            self.opened
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(password.map(str::to_string));
            std::thread::sleep(self.slow);
            if let Some(error) = &self.refuse {
                return Err(error.clone());
            }
            match (self.password, password) {
                (Some(want), Some(given)) if want == given => {}
                (Some(_), given) => return Err(OpenError::Password { wrong: given.is_some() }),
                (None, _) => {}
            }
            Ok(Box::new(FakeDocument { fake: self.clone() }))
        }
    }

    impl Document for FakeDocument {
        fn page_count(&self) -> usize {
            self.fake.pages.len()
        }

        fn page_size(&mut self, page: usize) -> Option<PageSize> {
            self.fake.pages.get(page).copied()
        }

        fn render(&mut self, page: usize, width: u32, height: u32) -> Result<Bitmap, String> {
            self.fake
                .drawn
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((page, width, height));
            if self.fake.broken.contains(&page) {
                return Err("the page is broken".into());
            }
            Ok(Bitmap {
                width,
                height,
                bgra: vec![255; (width * height * 4) as usize],
            })
        }
    }

    struct FakeText(Result<Vec<String>, String>);

    impl PdfText for FakeText {
        fn pages(&self, _: &str) -> Result<Vec<String>, String> {
            self.0.clone()
        }
    }

    fn texts(pages: &[&str]) -> FakeText {
        FakeText(Ok(pages.iter().map(|p| p.to_string()).collect()))
    }

    fn open_named(
        cx: &mut TestAppContext,
        name: &str,
        size: u64,
        fake: Fake,
        text: FakeText,
    ) -> (Entity<PdfView>, VisualTestContext) {
        cx.update(testing::init);
        let entry = file("C:\\docs", name, size, 1);
        let (visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| PdfView::with_engine(entry, Arc::new(fake), Arc::new(text), None, window, cx))
        });
        (view, visual)
    }

    fn open(cx: &mut TestAppContext, fake: Fake) -> (Entity<PdfView>, VisualTestContext) {
        open_named(cx, "report.pdf", 1000, fake, texts(&[]))
    }

    /// Frames and parked work until `done`.
    fn until(visual: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..600 {
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

    fn frames(visual: &mut VisualTestContext) {
        for _ in 0..4 {
            visual.update(|window, cx| window.render_frame(cx));
            visual.run_until_parked();
        }
    }

    /// Open, with the first page drawn.
    fn ready(view: &Entity<PdfView>, visual: &mut VisualTestContext) {
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
        visual.update(|window, _| window.find_by_label(label).role())
    }

    fn x_of(visual: &mut VisualTestContext, label: &str) -> f32 {
        visual.update(|window, _| f32::from(window.find_by_label(label).bounds().origin.x))
    }

    fn click(visual: &mut VisualTestContext, id: &'static str) {
        visual.update(|window, cx| window.click(id, cx));
        frames(visual);
    }

    fn focus(view: &Entity<PdfView>, visual: &mut VisualTestContext) {
        visual.update(|window, cx| {
            let handle = view.read(cx).focus.clone();
            window.focus(&handle, cx);
        });
    }

    fn press(visual: &mut VisualTestContext, key: &str) {
        visual.update(|window, cx| window.press(key, cx));
        frames(visual);
    }

    fn viewport(view: &Entity<PdfView>, visual: &VisualTestContext) -> (f32, f32) {
        view.read_with(visual, |v, _| v.scroll.viewport())
    }

    fn sheet(width: f32, height: f32) -> PageSize {
        PageSize { width, height }
    }

    // ---- The arithmetic ---------------------------------------------------------------------------------

    #[test]
    fn pages_are_stacked_with_a_margin_and_a_gap_and_found_by_the_box() {
        let mut layout = Layout::new(10, sheet(100., 200.));
        layout.rebuild(1.);
        assert_eq!(layout.top(0), PAD);
        assert_eq!(layout.top(1), PAD + 200. + GAP);
        assert_eq!(layout.height(), PAD + 10. * 200. + 9. * GAP + PAD);
        assert_eq!(layout.extent(3), (100., 200.));
        // The pages that meet the span.
        assert_eq!(layout.visible(0., 100.), 0..1);
        assert_eq!(layout.visible(0., PAD + 200. + GAP + 1.), 0..2);
        assert_eq!(layout.visible(PAD + 199., PAD + 200.), 0..1, "the last pixel of page 1");
        assert!(
            layout.visible(PAD + 201., PAD + 202.).is_empty(),
            "the gap holds no page"
        );
        assert_eq!(layout.visible(-500., -1.), 0..0);
        assert_eq!(layout.visible(layout.height(), layout.height() + 100.), 10..10);
        assert_eq!(layout.visible(0., layout.height()), 0..10);
        // The page at a place: the half gaps belong to the page before and after.
        assert_eq!(layout.page_at(-10.), 0);
        assert_eq!(layout.page_at(PAD + 205.), 0);
        assert_eq!(layout.page_at(PAD + 200. + GAP), 1);
        assert_eq!(layout.page_at(1e9), 9);
        // The zoom scales the pages, not the margin or the gap.
        layout.rebuild(2.);
        assert_eq!(layout.extent(0), (200., 400.));
        assert_eq!(layout.top(1), PAD + 400. + GAP);
        assert_eq!(layout.zoom(), 2.);
        assert!(Layout::new(0, sheet(1., 1.)).is_empty());
        assert_eq!(Layout::new(0, sheet(1., 1.)).page_at(5.), 0);
    }

    #[test]
    fn sizes_arriving_later_move_the_pages_only_when_they_differ_from_the_estimate() {
        let mut layout = Layout::new(4, sheet(100., 200.));
        layout.rebuild(1.);
        let before = layout.height();
        assert!(
            !layout.set_sizes(1, &[sheet(100., 200.), sheet(100., 200.)]),
            "the estimate again"
        );
        assert_eq!(layout.height(), before);
        // A landscape page in the middle is wider and shorter.
        assert!(layout.set_sizes(2, &[sheet(300., 100.)]));
        assert_eq!(layout.size_of(2), sheet(300., 100.));
        assert_eq!(layout.height(), before - 100.);
        assert_eq!(
            layout.width(0.),
            300. + 2. * PAD,
            "the column is as wide as its widest page"
        );
        assert_eq!(layout.width(1000.), 1000.);
        // Sizes past the last page are ignored.
        assert!(!layout.set_sizes(9, &[sheet(5., 5.)]));
    }

    #[test]
    fn fit_width_fills_the_box_and_the_steps_walk_the_ladder() {
        let layout = Layout::new(3, sheet(800., 1000.));
        assert!(
            (layout.fit_zoom(832.) - 1.).abs() < 1e-6,
            "a box 32 wider than the page"
        );
        assert!((layout.fit_zoom(432.) - 0.5).abs() < 1e-6);
        assert_eq!(
            layout.fit_zoom(100000.),
            MAX_FIT,
            "a small page isn't blown up past 400%"
        );
        assert_eq!(layout.fit_zoom(10.), 1., "no box yet");
        assert_eq!(zoom_in(1.), 1.1);
        assert_eq!(zoom_in(1.186), 1.25, "from fit, to the next step up");
        assert_eq!(zoom_in(5.), 5.);
        assert_eq!(zoom_out(1.), 0.9);
        assert_eq!(zoom_out(1.186), 1.1);
        assert_eq!(zoom_out(0.1), 0.1);
        assert_eq!(zoom_out(0.07), 0.07, "never up when zooming out");
        assert!(ZOOM_STEPS.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn a_page_is_drawn_at_its_size_on_screen_times_the_scale_and_kept_under_the_cap() {
        assert_eq!(device_size(816., 1056., 1.), (816, 1056));
        assert_eq!(device_size(816., 1056., 1.25), (1020, 1320));
        assert_eq!(device_size(0., 0., 1.), (1, 1));
        let (w, h) = device_size(4080., 5280., 1.5);
        assert!(u64::from(w) * u64::from(h) <= MAX_PAGE_PIXELS, "{w} x {h}");
        // The shape is kept.
        assert!((w as f32 / h as f32 - 4080. / 5280.).abs() < 0.001);
    }

    #[test]
    fn a_zoom_keeps_the_place_at_the_top_of_the_box() {
        let mut layout = Layout::new(20, sheet(600., 800.));
        layout.rebuild(1.);
        let y = layout.top(7) + 200.;
        let anchor = layout.anchor(y);
        assert_eq!(anchor.page, 7);
        assert!((anchor.fraction - 0.25).abs() < 1e-6);
        layout.rebuild(2.);
        assert!((layout.place(anchor) - (layout.top(7) + 400.)).abs() < 1e-3);
    }

    #[test]
    fn text_comes_back_a_string_a_page() {
        let lines = vec![
            ("p. 1".to_string(), "first".to_string()),
            ("p. 1".to_string(), "second".to_string()),
            ("p. 3".to_string(), "تقرير".to_string()),
            ("line 4".to_string(), "not a page".to_string()),
        ];
        assert_eq!(pages_of(&lines), ["first\nsecond", "", "تقرير"]);
        assert!(pages_of(&[]).is_empty());
    }

    #[test]
    fn the_icons_the_toolbar_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::ChevronUp,
            IconName::ChevronDown,
            IconName::ZoomIn,
            IconName::ZoomOut,
            IconName::MoveHorizontal,
            IconName::Copy,
            IconName::Search,
            IconName::FileLock,
            IconName::KeyRound,
            IconName::LoaderCircle,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    // ---- The view ---------------------------------------------------------------------------------------

    #[gpui_kit::test]
    fn pages_are_drawn_near_the_box_only_and_the_ui_does_not_wait(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut fake = Fake::letter(500);
        // Opening takes a while on the worker: the UI thread doesn't wait for it, it shows the wait.
        fake.slow = Duration::from_millis(300);
        let (view, mut visual) = open(cx, fake.clone());
        assert_eq!(view.read_with(&visual, |v, _| v.state().clone()), State::Opening);
        assert!(view.read_with(&visual, |v, _| v.drawn_pages().is_empty()));
        ready(&view, &mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.state().clone()), State::Ready);
        assert_eq!(view.read_with(&visual, |v, _| v.page_count()), Some(500));
        // Fit to width is where it starts.
        let (vw, _) = viewport(&view, &visual);
        let zoom = view.read_with(&visual, |v, _| v.zoom());
        assert!(view.read_with(&visual, |v, _| v.is_fit()));
        assert!(
            (zoom - (vw - 2. * PAD) / 816.).abs() < 1e-3,
            "{zoom} in a box {vw} wide"
        );
        assert!(has(&mut visual, "Pages of report.pdf"));
        assert!(has(&mut visual, "Page 1"));
        assert!(has(&mut visual, "Page 1 of 500"));
        assert!(!has(&mut visual, "Page 40"), "a page far away isn't there");
        let asked: Vec<usize> = fake.drawn().iter().map(|d| d.0).collect();
        assert!(
            !asked.is_empty() && asked.iter().all(|&p| p < 4),
            "only the first pages were drawn: {asked:?}"
        );
        let on_screen = visual.update(|window, _| window.find_all_by_role(Role::Image).len());
        assert!(on_screen <= 4, "{on_screen} pages in the tree");
    }

    #[gpui_kit::test]
    fn scrolling_far_draws_there_and_lets_go_of_the_pages_left_behind(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(cx, Fake::letter(500));
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
        // And back: the first page is drawn again, the far ones are let go.
        view.update(&mut visual, |v, cx| assert!(v.go_to_page(1, cx)));
        until(&mut visual, "page 1 is drawn again", |cx| {
            view.read_with(cx, |v, _| v.drawn_pages().contains(&1))
        });
        frames(&mut visual);
        assert!(view.read_with(&visual, |v, _| v.drawn_pages().iter().all(|&p| p < 10)));
        // No such page.
        view.update(&mut visual, |v, cx| {
            assert!(!v.go_to_page(0, cx) && !v.go_to_page(501, cx));
        });
    }

    #[gpui_kit::test]
    fn a_page_let_go_of_leaves_the_gpu_atlas_too(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(cx, Fake::letter(200));
        ready(&view, &mut visual);
        let first = view.read_with(&visual, |v, _| v.drawn.get(&0).map(|d| d.image.clone()));
        let first = first.expect("page 1 is drawn");
        assert!(
            visual.update(|window, _| window.has_image_atlas_entry(&first)),
            "a page on screen is in the atlas"
        );
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
    fn the_drawn_pages_stay_within_the_budget_and_never_lose_one_on_screen(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(cx, Fake::letter(30));
        ready(&view, &mut visual);
        // Zoom out until many pages are near, then squeeze the budget: only what is on screen stays.
        for _ in 0..8 {
            click(&mut visual, "pdf-zoom-out");
        }
        until(&mut visual, "several pages are drawn", |cx| {
            view.read_with(cx, |v, _| v.drawn_pages().len() >= 3)
        });
        frames(&mut visual);
        assert!(view.read_with(&visual, |v, _| v.drawn_bytes()) <= CACHE_BUDGET);
        view.update(&mut visual, |v, cx| {
            // Each page now weighs half the budget, so the budget holds only two.
            for drawn in v.drawn.values_mut() {
                drawn.bytes = CACHE_BUDGET / 2 + 1;
            }
            cx.notify();
        });
        frames(&mut visual);
        let held = view.read_with(&visual, |v, _| v.drawn_pages());
        let top = view.read_with(&visual, |v, _| v.scrolled());
        let (_, vh) = viewport(&view, &visual);
        let visible: Vec<usize> = view.read_with(&visual, |v, _| {
            let range = v.layout.as_ref().map(|l| l.visible(top, top + vh)).unwrap_or(0..0);
            range.map(|p| p + 1).collect()
        });
        assert!(
            visible.iter().all(|p| held.contains(p)),
            "pages on screen {visible:?} are all kept; kept {held:?}"
        );
        assert!(
            held.len() <= visible.len().max(1),
            "{held:?} against {visible:?}: the rest were let go"
        );
    }

    #[gpui_kit::test]
    fn zoom_buttons_and_keys_redraw_the_pages_at_the_new_size_and_keep_the_place(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let fake = Fake::letter(40);
        let (view, mut visual) = open(cx, fake.clone());
        ready(&view, &mut visual);
        view.update(&mut visual, |v, cx| {
            v.go_to_page(10, cx);
        });
        frames(&mut visual);
        let page_before = view.read_with(&visual, |v, _| v.current_page());
        assert_eq!(page_before, Some(10));
        // Zoom in from fit (about 119%) goes to the next step up.
        click(&mut visual, "pdf-zoom-in");
        assert_eq!(view.read_with(&visual, |v, _| v.zoom()), 1.25);
        assert!(!view.read_with(&visual, |v, _| v.is_fit()));
        assert!(has(&mut visual, "Zoom: 125%"));
        assert_eq!(
            view.read_with(&visual, |v, _| v.current_page()),
            page_before,
            "the same page is in the middle"
        );
        // At the window's scale: 816 x 1056 at 125%.
        let scale = visual.update(|window, _| window.scale_factor());
        let (w, h) = device_size(816. * 1.25, 1056. * 1.25, scale);
        until(&mut visual, "the pages are drawn at 125%", |_| {
            fake.drawn().contains(&(9, w, h))
        });
        // The keys, once the pages have the keyboard.
        focus(&view, &mut visual);
        press(&mut visual, "-");
        assert_eq!(view.read_with(&visual, |v, _| v.zoom()), 1.1);
        press(&mut visual, "=");
        assert_eq!(view.read_with(&visual, |v, _| v.zoom()), 1.25);
        press(&mut visual, "0");
        assert!(view.read_with(&visual, |v, _| v.is_fit()));
        click(&mut visual, "pdf-zoom-in");
        click(&mut visual, "pdf-fit");
        assert!(view.read_with(&visual, |v, _| v.is_fit()), "the Fit width button");
        // Zoom out as far as it goes.
        for _ in 0..30 {
            click(&mut visual, "pdf-zoom-out");
        }
        assert_eq!(view.read_with(&visual, |v, _| v.zoom()), 0.1);
        assert!(has(&mut visual, "Zoom: 10%"));
    }

    #[gpui_kit::test]
    fn the_wheel_scrolls_and_the_page_box_and_buttons_go_to_pages(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(cx, Fake::letter(40));
        ready(&view, &mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.scrolled()), 0.);
        visual.update(|window, cx| window.scroll("pdf-pages", ScrollDelta::Pixels(point(px(0.), px(-300.))), cx));
        assert_eq!(view.read_with(&visual, |v, _| v.scrolled()), 300.);
        visual.update(|window, cx| window.scroll("pdf-pages", ScrollDelta::Pixels(point(px(0.), px(100.))), cx));
        assert_eq!(view.read_with(&visual, |v, _| v.scrolled()), 200.);
        visual.update(|window, cx| window.scroll("pdf-pages", ScrollDelta::Pixels(point(px(0.), px(5000.))), cx));
        assert_eq!(view.read_with(&visual, |v, _| v.scrolled()), 0., "not past the top");
        // Page Down scrolls 0.85 of the box.
        let (_, vh) = viewport(&view, &visual);
        assert!(view.update(&mut visual, |v, cx| v.page(true, cx)));
        assert!((view.read_with(&visual, |v, _| v.scrolled()) - vh * 0.85).abs() < 0.01);
        assert!(view.update(&mut visual, |v, cx| v.page(false, cx)));
        assert!(
            !view.update(&mut visual, |v, cx| v.page(false, cx)),
            "already at the top"
        );
        // The next and previous page buttons.
        assert_eq!(view.read_with(&visual, |v, _| v.current_page()), Some(1));
        click(&mut visual, "pdf-next");
        assert_eq!(view.read_with(&visual, |v, _| v.current_page()), Some(2));
        click(&mut visual, "pdf-previous");
        assert_eq!(view.read_with(&visual, |v, _| v.current_page()), Some(1));
        // The page box takes a number, 1 to the last page.
        visual.update(|window, cx| {
            let go_to = view.read(cx).go_to.clone();
            go_to.update(cx, |input, cx| input.set_value("25", window, cx));
            view.update(cx, |v, cx| v.go_from_box(window, cx));
        });
        frames(&mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.current_page()), Some(25));
        assert_eq!(
            view.read_with(&visual, |v, cx| v.go_to.read(cx).value().to_string()),
            ""
        );
        visual.update(|window, cx| {
            let go_to = view.read(cx).go_to.clone();
            go_to.update(cx, |input, cx| input.set_value("41", window, cx));
            view.update(cx, |v, cx| v.go_from_box(window, cx));
        });
        frames(&mut visual);
        assert_eq!(
            view.read_with(&visual, |v, _| v.current_page()),
            Some(25),
            "there is no page 41"
        );
        // Home and End, with the keyboard.
        focus(&view, &mut visual);
        press(&mut visual, "home");
        assert_eq!(view.read_with(&visual, |v, _| v.scrolled()), 0.);
        press(&mut visual, "end");
        assert_eq!(view.read_with(&visual, |v, _| v.current_page()), Some(40));
        press(&mut visual, "pageup");
        assert!(view.read_with(&visual, |v, _| v.scrolled()) > 0.);
    }

    #[gpui_kit::test]
    fn a_password_protected_pdf_says_so_asks_for_the_password_and_opens_with_the_right_one(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut fake = Fake::letter(3);
        fake.password = Some("secret");
        let (view, mut visual) = open(cx, fake.clone());
        until(&mut visual, "the password is asked for", |cx| {
            view.read_with(cx, |v, _| matches!(v.state(), State::Password { .. }))
        });
        assert_eq!(
            view.read_with(&visual, |v, _| v.state().clone()),
            State::Password { wrong: false }
        );
        assert!(has(&mut visual, "This file is password-protected."));
        assert!(has(&mut visual, "Password"));
        assert!(!has(&mut visual, "That password didn't open it."));
        let type_password = |visual: &mut VisualTestContext, text: &'static str| {
            visual.update(|window, cx| {
                let input = view.read(cx).password.clone();
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            });
            frames(visual);
        };
        // A wrong one is said so, and the form stays.
        type_password(&mut visual, "nope");
        click(&mut visual, "pdf-password-open");
        until(&mut visual, "the wrong password is refused", |cx| {
            view.read_with(cx, |v, _| v.state() == &State::Password { wrong: true })
        });
        assert!(has(&mut visual, "That password didn't open it."));
        assert_eq!(role_of(&mut visual, "That password didn't open it."), Some(Role::Alert));
        // The right one opens it.
        type_password(&mut visual, "secret");
        click(&mut visual, "pdf-password-open");
        until(&mut visual, "the PDF opens", |cx| {
            view.read_with(cx, |v, _| v.state() == &State::Ready)
        });
        ready(&view, &mut visual);
        assert!(has(&mut visual, "Page 1"));
        assert_eq!(
            fake.tried(),
            [None, Some("nope".to_string()), Some("secret".to_string())]
        );
        // The password isn't left in the box.
        assert_eq!(
            view.read_with(&visual, |v, cx| v.password.read(cx).value().to_string()),
            ""
        );
    }

    #[gpui_kit::test]
    fn what_cannot_be_shown_says_why(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let refused = |error: OpenError| Fake {
            refuse: Some(error),
            ..Fake::letter(1)
        };
        for (error, sentence) in [
            (OpenError::NotPdf, "This file isn't a PDF inside, so it isn't shown."),
            (OpenError::Damaged, "This PDF is damaged, so it can't be shown."),
            (
                OpenError::Unreadable("access denied".into()),
                "Couldn't read the PDF: access denied",
            ),
        ] {
            let (view, mut visual) = open(cx, refused(error));
            until(&mut visual, "it fails", |cx| {
                view.read_with(cx, |v, _| matches!(v.state(), State::Failed(_)))
            });
            assert!(has(&mut visual, sentence), "{sentence}");
            assert_eq!(role_of(&mut visual, sentence), Some(Role::Status));
        }
        // No pages at all.
        let (view, mut visual) = open(cx, Fake::letter(0));
        until(&mut visual, "it fails", |cx| {
            view.read_with(cx, |v, _| matches!(v.state(), State::Failed(_)))
        });
        assert!(has(&mut visual, "This PDF has no pages."));
        // Too big isn't even opened (the TypeScript's 256 MB).
        let big = Fake::letter(3);
        let (view, mut visual) = open_named(cx, "huge.pdf", MAX_BYTES + 1, big.clone(), texts(&[]));
        assert!(matches!(
            view.read_with(&visual, |v, _| v.state().clone()),
            State::Failed(_)
        ));
        assert!(has(&mut visual, "Too big to preview here: open it to read it."));
        assert!(big.tried().is_empty());
    }

    #[gpui_kit::test]
    fn a_page_that_will_not_draw_is_left_blank_and_not_asked_for_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut fake = Fake::letter(5);
        fake.broken = vec![0];
        let (view, mut visual) = open(cx, fake.clone());
        until(&mut visual, "page 2 is drawn", |cx| {
            view.read_with(cx, |v, _| v.drawn_pages().contains(&2))
        });
        frames(&mut visual);
        std::thread::sleep(Duration::from_millis(50));
        frames(&mut visual);
        let tries = fake.drawn().iter().filter(|d| d.0 == 0).count();
        assert_eq!(tries, 1, "the broken page was tried once");
        assert!(has(&mut visual, "Page 1"), "its place is still there");
        assert!(!view.read_with(&visual, |v, _| v.drawn_pages().contains(&1)));
    }

    #[gpui_kit::test]
    fn different_sized_pages_are_laid_out_as_their_sizes_arrive(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut fake = Fake::letter(6);
        // Page 3 is a wide sheet, 5 a small card.
        fake.pages[2] = sheet(1200., 600.);
        fake.pages[4] = sheet(400., 300.);
        let (view, mut visual) = open(cx, fake);
        until(&mut visual, "every size is read", |cx| {
            view.read_with(cx, |v, _| {
                v.layout
                    .as_ref()
                    .is_some_and(|l| l.size_of(2) == sheet(1200., 600.) && l.size_of(4) == sheet(400., 300.))
            })
        });
        frames(&mut visual);
        // Fit follows the widest page.
        let (vw, _) = viewport(&view, &visual);
        let zoom = view.read_with(&visual, |v, _| v.zoom());
        assert!((zoom - (vw - 2. * PAD) / 1200.).abs() < 1e-3, "{zoom}");
        // The column is as tall as the sheets are.
        let height = view.read_with(&visual, |v, _| v.layout.as_ref().map(Layout::height));
        let expected = PAD * 2. + GAP * 5. + zoom * (1056. * 4. + 600. + 300.);
        assert!(
            (height.unwrap_or(0.) - expected).abs() < 0.5,
            "{height:?} against {expected}"
        );
    }

    #[gpui_kit::test]
    fn copy_text_and_find_use_the_pages_text_and_say_when_there_is_none(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let pages = ["Quarterly report alpha", "", "beta ALPHA gamma", "تقرير ربع سنوي"];
        let (view, mut visual) = open_named(cx, "report.pdf", 1000, Fake::letter(4), texts(&pages));
        ready(&view, &mut visual);
        // Copy text copies the page in the middle of the box.
        click(&mut visual, "pdf-copy-text");
        until(&mut visual, "the text is copied", |cx| {
            cx.read_from_clipboard().and_then(|item| item.text()).as_deref() == Some("Quarterly report alpha")
        });
        // The "Copied" notice sits at the bottom end, over the toolbar, until it fades: a person waits it out.
        visual.update(|window, cx| window.clear_notifications(cx));
        visual.run_until_parked();
        // Find: which pages have the words, case ignored; Next goes round.
        click(&mut visual, "pdf-find-toggle");
        assert!(has(&mut visual, "Find in this PDF"));
        // Typed, as a person would: the box has the focus, its text is replaced.
        let search = |visual: &mut VisualTestContext, text: &'static str| {
            visual.update(|window, cx| {
                let input = view.read(cx).find.clone();
                input.update(cx, |input, cx| input.focus(window, cx));
            });
            press(visual, "ctrl-a");
            visual.update(|window, cx| window.input(text, cx));
            frames(visual);
        };
        until(&mut visual, "the text is read", |cx| {
            view.read_with(cx, |v, _| matches!(v.texts, Texts::Ready(_)))
        });
        search(&mut visual, "alpha");
        assert_eq!(view.read_with(&visual, |v, _| v.found_on()), [1, 3]);
        assert!(has(&mut visual, "On 2 pages"));
        click(&mut visual, "pdf-find-next");
        assert_eq!(view.read_with(&visual, |v, _| v.current_page()), Some(3));
        click(&mut visual, "pdf-find-next");
        assert_eq!(
            view.read_with(&visual, |v, _| v.current_page()),
            Some(1),
            "round to the first"
        );
        click(&mut visual, "pdf-find-next");
        click(&mut visual, "pdf-find-previous");
        assert_eq!(view.read_with(&visual, |v, _| v.current_page()), Some(1));
        search(&mut visual, "zzz");
        assert!(has(&mut visual, "Not found"));
        // Arabic words are found as Arabic.
        search(&mut visual, "ربع");
        assert_eq!(view.read_with(&visual, |v, _| v.found_on()), [4]);
        // A page without text can't be copied.
        view.update(&mut visual, |v, cx| {
            v.go_to_page(2, cx);
        });
        frames(&mut visual);
        visual.write_to_clipboard(ClipboardItem::new_string("before".into()));
        click(&mut visual, "pdf-copy-text");
        visual.run_until_parked();
        assert_eq!(
            visual.read_from_clipboard().and_then(|item| item.text()).as_deref(),
            Some("before"),
            "a scanned page copies nothing"
        );
    }

    #[gpui_kit::test]
    fn text_that_cannot_be_read_is_said(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open_named(
            cx,
            "report.pdf",
            1000,
            Fake::letter(2),
            FakeText(Err("the reader died".into())),
        );
        ready(&view, &mut visual);
        click(&mut visual, "pdf-find-toggle");
        until(&mut visual, "the reading fails", |cx| {
            view.read_with(cx, |v, _| matches!(v.texts, Texts::Failed(_)))
        });
        assert!(has(&mut visual, "the reader died"));
    }

    #[gpui_kit::test]
    fn the_tools_are_named_and_mirror_in_arabic_with_an_arabic_file_name(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (view, mut visual) = open_named(cx, "تقرير الربع.pdf", 1000, Fake::letter(3), texts(&[]));
        ready(&view, &mut visual);
        // (Opening a test window starts in English; the direction is turned after, and the view drawn again.)
        testing::set_rtl(true);
        view.update(&mut visual, |_, cx| cx.notify());
        frames(&mut visual);
        assert!(has(&mut visual, "Pages of تقرير الربع.pdf"));
        // In Arabic a name is the catalog's where it has one ("PDF tools" does), else the English.
        for name in [
            t!("Previous page"),
            t!("Next page"),
            t!("Go to page"),
            t!("Zoom out"),
            t!("Zoom in"),
            t!("Fit width"),
            t!("Find"),
            t!("Copy text"),
            t!("PDF tools"),
            t!("Page {n} of {count}", n = 1, count = 3),
        ] {
            assert!(has(&mut visual, &name), "{name}");
        }
        assert_eq!(role_of(&mut visual, "Zoom in"), Some(Role::Button));
        assert_eq!(role_of(&mut visual, "Page 1"), Some(Role::Image));
        assert_eq!(role_of(&mut visual, "Pages of تقرير الربع.pdf"), Some(Role::List));
        assert_eq!(role_of(&mut visual, &t!("PDF tools")), Some(Role::Toolbar));
        // Right to left: the previous page button is at the start, which is the right.
        let (previous, next) = (x_of(&mut visual, "Previous page"), x_of(&mut visual, "Next page"));
        assert!(previous > next, "previous at {previous}, next at {next}");
        // A page is a page whatever the direction: the first one is still centred.
        let (vw, _) = viewport(&view, &visual);
        let page = visual.update(|w, _| w.find_by_label("Page 1").bounds());
        let middle = f32::from(page.origin.x) + f32::from(page.size.width) / 2.;
        assert!((middle - vw / 2.).abs() < 2., "{middle} against {}", vw / 2.);
        testing::set_rtl(false);
        view.update(&mut visual, |_, cx| cx.notify());
        frames(&mut visual);
        assert!(x_of(&mut visual, "Previous page") < x_of(&mut visual, "Next page"));
    }

    #[gpui_kit::test]
    fn no_accessible_name_appears_twice(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(cx, Fake::letter(2));
        ready(&view, &mut visual);
        click(&mut visual, "pdf-find-toggle");
        let names = visual.update(|window, _| {
            let mut names: Vec<String> = [Role::Button, Role::Image, Role::Status, Role::Group, Role::List]
                .into_iter()
                .flat_map(|role| window.find_all_by_role(role))
                .filter_map(|e| e.label().map(str::to_string))
                .collect();
            names.sort();
            names
        });
        let mut deduped = names.clone();
        deduped.dedup();
        assert_eq!(names, deduped, "a name twice");
    }

    // ---- Windows' own renderer ----------------------------------------------------------------------------

    /// A PDF written here: `pages` pages, each `width` by `height` points with a blue rectangle in it.
    fn written_pdf(pages: usize, width: u32, height: u32) -> Vec<u8> {
        let content = "0 0 1 rg 20 20 160 60 re f";
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            format!(
                "<< /Type /Pages /Kids [{}] /Count {pages} >>",
                (0..pages)
                    .map(|i| format!("{} 0 R", 3 + 2 * i))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        ];
        for i in 0..pages {
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Contents {} 0 R >>",
                4 + 2 * i
            ));
            objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
        }
        with_cross_references(&objects, "")
    }

    /// A PDF whose page has no content, locked to open with the password "secret" (40-bit RC4, revision 2; the
    /// owner password is "owner"). The O and U entries were worked out once, by the standard security handler's
    /// algorithms 3 and 4, with a script; nothing else in the file needs encrypting.
    fn locked_pdf() -> Vec<u8> {
        let o = "92fe0f4454ad4c9644693f33c07cb54f587dce1e2682fe9ecea6107a1ef630dd";
        let u = "49ecc2152a5aa9a4640923d0daf38ac2ceab69380b5caf562e5b9c72ee266a25";
        let id = "0123456789abcdef0123456789abcdef";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] >>".to_string(),
            format!("<< /Filter /Standard /V 1 /R 2 /O <{o}> /U <{u}> /P -4 >>"),
        ];
        with_cross_references(&objects, &format!("/Encrypt 4 0 R /ID [<{id}><{id}>]"))
    }

    /// The objects as a file, its cross-references worked out as it is written.
    fn with_cross_references(objects: &[String], trailer: &str) -> Vec<u8> {
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf += &format!("{} 0 obj\n{object}\nendobj\n", i + 1);
        }
        let xref = pdf.len();
        pdf += &format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
        for offset in offsets {
            pdf += &format!("{offset:010} 00000 n \n");
        }
        pdf += &format!(
            "trailer\n<< /Size {} /Root 1 0 R {trailer} >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        );
        pdf.into_bytes()
    }

    fn scratch_folder() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("glint-pdf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp folder");
        dir
    }

    /// Page one drawn: a blue rectangle on white, read back from the pixels.
    #[test]
    #[ignore = "needs Windows' PDF renderer (Windows.Data.Pdf)"]
    fn windows_draws_page_one_of_a_pdf_written_here() {
        let dir = scratch_folder();
        let path = dir.join("two.pdf");
        std::fs::write(&path, written_pdf(2, 200, 100)).expect("a temp file");
        let path = path.to_string_lossy().to_string();
        // Windows' PDF objects are used on a thread of their own, as the view does.
        let drawn = std::thread::spawn(move || {
            let mut document = WindowsPdf.open(&path, None).map_err(|e| format!("{e:?}"))?;
            let sizes = (
                document.page_count(),
                document.page_size(0),
                document.page_size(1),
                document.page_size(2),
            );
            let page = document.render(0, 200, 100)?;
            Ok::<_, String>((sizes, page))
        })
        .join();
        let _ = std::fs::remove_dir_all(&dir);
        let ((count, first, second, third), page) = drawn.expect("the thread ran").expect("it drew");
        assert_eq!(count, 2);
        // 200 x 100 points are 266.67 x 133.33 device-independent pixels.
        let first = first.expect("page 1 has a size");
        assert!(
            (first.width - 266.67).abs() < 0.1 && (first.height - 133.33).abs() < 0.1,
            "{first:?}"
        );
        assert_eq!(second, Some(first));
        assert_eq!(third, None, "no third page");
        assert_eq!((page.width, page.height), (200, 100));
        assert_eq!(page.bgra.len(), 200 * 100 * 4);
        let pixel = |x: usize, y: usize| {
            let i = (y * 200 + x) * 4;
            [page.bgra[i], page.bgra[i + 1], page.bgra[i + 2], page.bgra[i + 3]]
        };
        // The PDF fills (20, 20) to (180, 80) of 200 x 100 points in blue; the page is white.
        assert_eq!(pixel(100, 50), [255, 0, 0, 255], "blue in the middle (B, G, R, A)");
        assert_eq!(pixel(2, 2), [255, 255, 255, 255], "white at the corner");
        assert_eq!(pixel(100, 5), [255, 255, 255, 255], "white above the rectangle");
    }

    /// The whole way: a PDF on disk, Windows' renderer on its worker thread, the page as a picture in the view.
    #[gpui_kit::test]
    #[ignore = "needs Windows' PDF renderer (Windows.Data.Pdf)"]
    fn the_view_shows_a_pdf_written_here_through_windows(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let dir = scratch_folder();
        let path = dir.join("تقرير.pdf");
        std::fs::write(&path, written_pdf(3, 200, 100)).expect("a temp file");
        cx.update(testing::init);
        let mut entry = file(&dir.to_string_lossy(), "تقرير.pdf", 1000, 1);
        entry.path = path.to_string_lossy().to_string();
        let (mut visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| PdfView::with_engine(entry, Arc::new(WindowsPdf), Arc::new(texts(&[])), None, window, cx))
        });
        ready(&view, &mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.page_count()), Some(3));
        assert!(has(&mut visual, "Pages of تقرير.pdf"));
        // The page is the picture Windows drew: blue in the middle of the rectangle, white at the corner.
        let (w, h, centre, corner) = view.read_with(&visual, |v, _| {
            let drawn = v.drawn.get(&0).expect("page 1 is drawn");
            let bytes = drawn.image.as_bytes(0).expect("pixels");
            let (w, h) = drawn.want;
            let at = |x: u32, y: u32| {
                let i = ((y * w + x) * 4) as usize;
                [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]
            };
            (w, h, at(w / 2, h / 2), at(2, 2))
        });
        assert_eq!(centre, [255, 0, 0, 255], "blue in the middle of a {w} x {h} page");
        assert_eq!(corner, [255, 255, 255, 255]);
        assert!(
            (w as f32 / h as f32 - 2.).abs() < 0.01,
            "the page is twice as wide as it is tall"
        );
        drop(view);
        drop(visual);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "needs Windows' PDF renderer (Windows.Data.Pdf)"]
    fn windows_tells_a_locked_pdf_a_wrong_password_a_damaged_file_and_a_file_that_is_no_pdf() {
        let dir = scratch_folder();
        let locked = dir.join("locked.pdf");
        std::fs::write(&locked, locked_pdf()).expect("a temp file");
        let damaged = dir.join("damaged.pdf");
        let mut bytes = written_pdf(1, 200, 100);
        bytes.truncate(bytes.len() / 2);
        std::fs::write(&damaged, bytes).expect("a temp file");
        let not_pdf = dir.join("page.pdf");
        std::fs::write(&not_pdf, "<html><script>alert(1)</script></html>").expect("a temp file");
        let text = |p: &std::path::Path| p.to_string_lossy().to_string();
        let (locked, damaged, not_pdf, missing) = (
            text(&locked),
            text(&damaged),
            text(&not_pdf),
            text(&dir.join("missing.pdf")),
        );
        let answers = std::thread::spawn(move || {
            let kind = |result: Result<Box<dyn Document>, OpenError>| result.err();
            let none = kind(WindowsPdf.open(&locked, None));
            let wrong = kind(WindowsPdf.open(&locked, Some("nope")));
            let right = WindowsPdf
                .open(&locked, Some("secret"))
                .map(|d| d.page_count())
                .map_err(|e| format!("{e:?}"));
            let damaged = kind(WindowsPdf.open(&damaged, None));
            let not_pdf = kind(WindowsPdf.open(&not_pdf, None));
            let missing = kind(WindowsPdf.open(&missing, None));
            (none, wrong, right, damaged, not_pdf, missing)
        })
        .join();
        let _ = std::fs::remove_dir_all(&dir);
        let (none, wrong, right, damaged, not_pdf, missing) = answers.expect("the thread ran");
        assert_eq!(none, Some(OpenError::Password { wrong: false }));
        assert_eq!(wrong, Some(OpenError::Password { wrong: true }));
        assert_eq!(right, Ok(1), "the right password opens it");
        assert_eq!(damaged, Some(OpenError::Damaged));
        assert_eq!(not_pdf, Some(OpenError::NotPdf));
        assert!(matches!(missing, Some(OpenError::Unreadable(_))), "{missing:?}");
    }
}
