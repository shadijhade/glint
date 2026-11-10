//! Typst in process: a World over the disk and the system fonts, compiling to pages and PDF.
//!
//! Ported from the places the TypeScript used the `typst` tool: `TypstView.tsx` (the preview, which asked
//! docconvert.rs's `typst_preview` for a PDF) and docconvert.rs's `compile_typst` (Export as PDF). Both ran
//! `typst compile --root <the file's folder>` as a downloaded program; here the compiler is linked in.
//!
//! | Before (the tool)                          | Here                                                             |
//! |--------------------------------------------|------------------------------------------------------------------|
//! | `typst compile --root <folder> file.typ`   | [`compile`] with [`Input::File`]; the folder is the root          |
//! | the PDF it wrote                           | [`Compiled::pdf`], or [`pdf_of`] for a whole export on a big stack |
//! | the PDF drawn by the PDF preview           | [`Compiled::render`]: a page drawn straight from the laid out page |
//! | `imports_packages` ("downloads from the internet") | an `@preview/...` import is an error that says so, with its line |
//! | the tool's stderr                          | [`Diagnostic`]s with file, line and column                       |
//! | `STOP` (cancel)                            | [`Limits`]: a stop flag and a time budget                         |
//!
//! # How it is made
//!
//! * **The World** ([`typst::World`]) reads files relative to the document's folder and never outside it: a path
//!   that climbs out (`../x`), or a symbolic link that leads out, is refused with a sentence that says why (the
//!   TypeScript passed `--root` for the same reason). The main file can instead be given as text with a folder
//!   to resolve pictures against ([`Input::Text`]); the Markdown export uses that.
//! * **Packages** (`#import "@preview/..."`) are refused: they download from the internet, which Glint never does
//!   for you. The error says so and what to do (copy the package's files next to the document).
//! * **Fonts** are the PC's own, found once by typst-kit's scanner on a thread of its own ([`warm_fonts`]) and
//!   shared by every compile ([`shared_fonts`]). Nothing is bundled. A compile started before the scan ends waits
//!   for it on its own (background) thread; the UI thread never calls [`compile`] or [`shared_fonts`].
//! * **Compiles run off the UI thread**, on threads with a big stack (Typst recurses deeply), and can be
//!   stopped: [`Limits`] holds a stop flag and a deadline that the World checks every time the compiler asks
//!   it for a file, a font or the date. NOTE: Typst has no hook to interrupt layout itself, so a document that
//!   spends a very long time laying out without touching a file or font is stopped when it next does, or when
//!   its thread is let go of (the result is thrown away).
//! * **Output.** [`Compiled`] keeps the laid out document. [`Compiled::render`] draws a page with typst-render
//!   at any scale (premultiplied RGBA; [`Rendered::into_bgra`] is what GPUI draws) and [`Compiled::pdf`] writes
//!   the PDF with typst-pdf.
//!
//! The Markdown to PDF export is [`from_markdown`]: the markdown crate's tree written as Typst markup.

use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::{Duration, Instant};

use typst::diag::{FileError, FileResult, Severity as TypstSeverity, SourceDiagnostic, Warned};
use typst::foundations::{Bytes, Datetime, Duration as TypstDuration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::{LazyHash, Scalar};
use typst::{Library, LibraryExt, World, WorldExt};
use typst_kit::files::{FileLoader, FileStore};
use typst_kit::fonts::FontStore;

/// How long a preview may take to typeset before it is given up on.
pub const PREVIEW_BUDGET: Duration = Duration::from_secs(30);
/// How long an export may take.
pub const EXPORT_BUDGET: Duration = Duration::from_secs(180);
/// The stack of the threads Typst runs on (it recurses deeply on nested content); only the part used is committed.
const STACK_BYTES: usize = 64 * 1024 * 1024;
/// A file the World will read is at most this big (a picture or a data file; a document's own text is far smaller).
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
/// The most pixels [`Compiled::render`] draws for one page (24 MP is 96 MB), so a poster can't take the memory.
pub const MAX_RENDER_PIXELS: f64 = 24_000_000.;

// ---- Stopping -------------------------------------------------------------------------------------------------

/// Why a compile ended without a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The document has errors (see the diagnostics).
    Errors,
    /// Someone asked it to stop ([`Limits::stop`]).
    Cancelled,
    /// It used up its time budget.
    TimedOut,
    /// Typst crashed on the document (a bug in Typst, reported as an error instead of taking Glint down).
    Crashed,
}

/// A stop flag and a time budget for a compile. Clones share the flag, so the view keeps one and the thread that
/// compiles gets another.
#[derive(Clone, Debug)]
pub struct Limits {
    stop: Arc<AtomicBool>,
    deadline: Option<Instant>,
}

impl Limits {
    /// A budget of `budget` from now.
    pub fn new(budget: Duration) -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            deadline: Instant::now().checked_add(budget),
        }
    }

    /// No time budget (a test, or an export that runs until it is stopped).
    pub fn unlimited() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            deadline: None,
        }
    }

    /// Asks every compile holding this flag to give up.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Whether [`Limits::stop`] was called.
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Ok while the compile may go on.
    pub fn check(&self) -> Result<(), Reason> {
        if self.stopped() {
            Err(Reason::Cancelled)
        } else if self.deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            Err(Reason::TimedOut)
        } else {
            Ok(())
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new(PREVIEW_BUDGET)
    }
}

// ---- Diagnostics ----------------------------------------------------------------------------------------------

/// Whether a diagnostic stops the document or only warns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// One thing Typst said about the document, with its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// What is wrong, in Typst's words.
    pub message: String,
    /// Typst's suggestions.
    pub hints: Vec<String>,
    /// The file it is in, as a path under the document's folder ("main.typ", "parts/intro.typ").
    pub file: Option<String>,
    /// The line (1 is the first) and the column (1 is the first character) where it starts.
    pub line: Option<usize>,
    pub column: Option<usize>,
    /// The text of that line, to show where.
    pub line_text: Option<String>,
}

impl Diagnostic {
    /// An error with no place.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            hints: Vec::new(),
            file: None,
            line: None,
            column: None,
            line_text: None,
        }
    }

    /// "main.typ:3:7", or just the file, or nothing.
    pub fn place(&self) -> Option<String> {
        let file = self.file.as_deref()?;
        Some(match (self.line, self.column) {
            (Some(line), Some(column)) => format!("{file}:{line}:{column}"),
            (Some(line), None) => format!("{file}:{line}"),
            _ => file.to_string(),
        })
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(place) = self.place() {
            write!(f, "{place}: ")?;
        }
        f.write_str(&self.message)?;
        for hint in &self.hints {
            write!(f, " (hint: {hint})")?;
        }
        Ok(())
    }
}

/// A compile that produced no document.
#[derive(Clone, Debug)]
pub struct Failure {
    pub reason: Reason,
    pub diagnostics: Vec<Diagnostic>,
    /// The files it read before it failed (the document itself, what it imports), so a save of any of them can be
    /// noticed and the document typeset again. Empty when it failed before reading any.
    pub dependencies: Vec<PathBuf>,
}

impl Failure {
    fn with(reason: Reason, message: impl Into<String>) -> Self {
        Self {
            reason,
            diagnostics: vec![Diagnostic::error(message)],
            dependencies: Vec::new(),
        }
    }

    /// A failure with one error and no place.
    pub fn message(message: impl Into<String>) -> Self {
        Self::with(Reason::Errors, message)
    }

    fn of(reason: Reason) -> Self {
        match reason {
            Reason::Cancelled => Self::with(reason, "Typesetting was stopped"),
            Reason::TimedOut => Self::with(reason, "Typesetting took too long, so it was given up on"),
            Reason::Crashed => Self::with(reason, "Typst stopped unexpectedly on this document"),
            Reason::Errors => Self::with(reason, "Typst couldn't typeset the document"),
        }
    }

    /// The first error, as one line ("main.typ:3:7: unknown variable: x").
    pub fn summary(&self) -> String {
        self.diagnostics
            .iter()
            .find(|d| d.severity == Severity::Error)
            .or(self.diagnostics.first())
            .map(Diagnostic::to_string)
            .unwrap_or_default()
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.summary())
    }
}

// ---- Fonts ----------------------------------------------------------------------------------------------------

/// The fonts of this PC, as Typst's font store holds them (loaded from disk when a document first uses one).
pub struct Fonts {
    store: FontStore,
    faces: usize,
}

impl Fonts {
    /// How many font faces were found.
    pub fn faces(&self) -> usize {
        self.faces
    }
}

static FONTS: OnceLock<Arc<Fonts>> = OnceLock::new();

/// The fonts, found by the first caller and shared by all. This waits for the scan: call it only from a thread
/// that may wait (a compile does), never from the UI thread. See [`fonts_ready`].
pub fn shared_fonts() -> Arc<Fonts> {
    FONTS
        .get_or_init(|| {
            let mut store = FontStore::new();
            let mut faces = 0;
            store.extend(typst_kit::fonts::system().inspect(|_| faces += 1));
            Arc::new(Fonts { store, faces })
        })
        .clone()
}

/// Whether the scan has finished.
pub fn fonts_ready() -> bool {
    FONTS.get().is_some()
}

/// Starts the font scan on a thread of its own, so the first document doesn't wait for it. Safe to call often.
pub fn warm_fonts() {
    if !fonts_ready() {
        let _ = std::thread::Builder::new().name("glint-typst-fonts".into()).spawn(|| {
            shared_fonts();
        });
    }
}

// ---- The World ------------------------------------------------------------------------------------------------

/// Typst's standard library, built once (it takes a few milliseconds).
static LIBRARY: LazyLock<LazyHash<Library>> = LazyLock::new(|| LazyHash::new(<Library as LibraryExt>::default()));

/// What is typeset.
#[derive(Clone, Debug)]
pub enum Input {
    /// A `.typ` file on disk; its folder is the root every path in it is relative to.
    File(PathBuf),
    /// Typst markup in memory, called `name`, with files read from `dir` (the Markdown export).
    Text { dir: PathBuf, name: String, text: String },
}

/// Reads the project's files from a folder, never from outside it.
struct Disk {
    root: PathBuf,
    /// The root with links resolved, to tell when a link leads out.
    real_root: Option<PathBuf>,
    memory: Vec<(FileId, Bytes)>,
    limits: Limits,
}

fn file_error(reason: Reason) -> FileError {
    FileError::Other(Some(
        match reason {
            Reason::TimedOut => "typesetting took too long",
            _ => "typesetting was stopped",
        }
        .into(),
    ))
}

impl FileLoader for Disk {
    fn load(&self, id: FileId) -> FileResult<Bytes> {
        self.limits.check().map_err(file_error)?;
        if let Some((_, bytes)) = self.memory.iter().find(|(known, _)| *known == id) {
            return Ok(bytes.clone());
        }
        if let VirtualRoot::Package(spec) = id.root() {
            return Err(FileError::Other(Some(
                format!(
                    "Glint typesets without going online, so the package {spec} isn't available; \
                     copy its files next to the document and import them by path"
                )
                .into(),
            )));
        }
        let path = id.vpath().realize(&self.root)?;
        let io = |e| FileError::from_io(e, &path);
        // A link inside the folder that leads outside it is a way out too.
        if let Some(real_root) = &self.real_root {
            let real = fs::canonicalize(&path).map_err(io)?;
            if !real.starts_with(real_root) {
                return Err(FileError::Other(Some(
                    "it is outside the document's folder, which Glint doesn't read without asking".into(),
                )));
            }
        }
        let meta = fs::metadata(&path).map_err(io)?;
        if meta.is_dir() {
            return Err(FileError::IsDirectory);
        }
        if meta.len() > MAX_FILE_BYTES {
            return Err(FileError::Other(Some("the file is too big to read".into())));
        }
        fs::read(&path).map(Bytes::new).map_err(io)
    }
}

/// The compiler's view of the world: the files under one folder, the PC's fonts, today's date.
struct GlintWorld {
    fonts: Arc<Fonts>,
    main: FileId,
    files: FileStore<Disk>,
    limits: Limits,
}

impl GlintWorld {
    fn new(input: Input, fonts: Arc<Fonts>, limits: Limits) -> Result<Self, Failure> {
        let (root, name, memory_text) = match input {
            Input::File(path) => {
                let root = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)
                    .ok_or_else(|| Failure::message("The file's name isn't text Typst can use"))?;
                (root, name, None)
            }
            Input::Text { dir, name, text } => (dir, name, Some(text)),
        };
        let vpath = VirtualPath::new(&name).map_err(|e| Failure::message(format!("{name}: {e}")))?;
        let main = RootedPath::new(VirtualRoot::Project, vpath).intern();
        let memory = memory_text
            .map(|text| vec![(main, Bytes::from_string(text))])
            .unwrap_or_default();
        let real_root = fs::canonicalize(&root).ok();
        let disk = Disk {
            root,
            real_root,
            memory,
            limits: limits.clone(),
        };
        Ok(Self {
            fonts,
            main,
            files: FileStore::new(disk),
            limits,
        })
    }

    /// The files read so far, as paths on disk (a document's imports and pictures: what a change to should
    /// typeset it again).
    fn dependencies(&mut self) -> Vec<PathBuf> {
        let (disk, ids) = self.files.dependencies();
        let mut paths: Vec<PathBuf> = ids
            .filter(|id| matches!(id.root(), VirtualRoot::Project))
            .filter(|id| !disk.memory.iter().any(|(known, _)| known == id))
            .filter_map(|id| id.vpath().realize(&disk.root).ok())
            .collect();
        paths.sort();
        paths
    }
}

impl World for GlintWorld {
    fn library(&self) -> &LazyHash<Library> {
        &LIBRARY
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.fonts.store.book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        self.limits.check().map_err(file_error)?;
        self.files.source(id)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.limits.check().map_err(file_error)?;
        self.files.file(id)
    }

    fn font(&self, index: usize) -> Option<Font> {
        if self.limits.check().is_err() {
            return None;
        }
        self.fonts.store.font(index)
    }

    fn today(&self, offset: Option<TypstDuration>) -> Option<Datetime> {
        self.limits.check().ok()?;
        today(offset.map(|o| o.hours()))
    }
}

/// Today's date: local time, or UTC shifted by `hours`.
fn today(hours: Option<f64>) -> Option<Datetime> {
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    // SAFETY: both only fill in a SYSTEMTIME they are given by value.
    let time = unsafe {
        match hours {
            None => GetLocalTime(),
            Some(_) => GetSystemTime(),
        }
    };
    let base = Datetime::from_ymd_hms(
        i32::from(time.wYear),
        u8::try_from(time.wMonth).ok()?,
        u8::try_from(time.wDay).ok()?,
        u8::try_from(time.wHour).ok()?,
        u8::try_from(time.wMinute).ok()?,
        u8::try_from(time.wSecond).ok()?,
    )?;
    let Some(shift) = hours.filter(|h| h.abs() >= 1. && h.abs() < 24.) else {
        return Some(base);
    };
    // A whole-hour offset from UTC: move the day by hand when the hours cross midnight.
    let hour = f64::from(time.wHour) + shift.trunc();
    let (day_shift, hour) = if hour < 0. {
        (-1, hour + 24.)
    } else if hour >= 24. {
        (1, hour - 24.)
    } else {
        (0, hour)
    };
    let (year, month, day) = shifted_day(
        i32::from(time.wYear),
        u32::from(time.wMonth),
        u32::from(time.wDay),
        day_shift,
    );
    Datetime::from_ymd_hms(
        year,
        u8::try_from(month).ok()?,
        u8::try_from(day).ok()?,
        hour as u8,
        u8::try_from(time.wMinute).ok()?,
        u8::try_from(time.wSecond).ok()?,
    )
}

/// The date `by` (-1, 0 or 1) days from `year-month-day`.
fn shifted_day(year: i32, month: u32, day: u32, by: i32) -> (i32, u32, u32) {
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let length = |m: u32| match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    match by {
        1 if day >= length(month) => {
            if month == 12 {
                (year + 1, 1, 1)
            } else {
                (year, month + 1, 1)
            }
        }
        1 => (year, month, day + 1),
        -1 if day <= 1 => {
            if month == 1 {
                (year - 1, 12, 31)
            } else {
                (year, month - 1, length(month - 1))
            }
        }
        -1 => (year, month, day - 1),
        _ => (year, month, day),
    }
}

// ---- Diagnostics from Typst ------------------------------------------------------------------------------------

/// A file's name for people: "main.typ" for the project's, "@preview/x:1.0.0/lib.typ" for a package's.
fn file_label(id: FileId) -> String {
    match id.root() {
        VirtualRoot::Project => id.vpath().get_without_slash().to_string(),
        VirtualRoot::Package(spec) => format!("{spec}/{}", id.vpath().get_without_slash()),
    }
}

/// Typst's diagnostics with their places worked out against `world`'s sources.
fn convert(world: &GlintWorld, list: impl IntoIterator<Item = SourceDiagnostic>) -> Vec<Diagnostic> {
    list.into_iter()
        .map(|d| {
            let mut out = Diagnostic {
                severity: match d.severity {
                    TypstSeverity::Error => Severity::Error,
                    TypstSeverity::Warning => Severity::Warning,
                },
                message: d.message.to_string(),
                hints: d.hints.iter().map(|h| h.v.to_string()).collect(),
                file: None,
                line: None,
                column: None,
                line_text: None,
            };
            if let Some(id) = d.span.id() {
                out.file = Some(file_label(id));
                if let (Some(range), Ok(source)) = (world.range(d.span), world.source(id)) {
                    let lines = source.lines();
                    if let Some((line, column)) = lines.byte_to_line_column(range.start) {
                        out.line = Some(line + 1);
                        out.column = Some(column + 1);
                        out.line_text = lines
                            .line_to_range(line)
                            .and_then(|r| source.text().get(r))
                            .map(|text| text.trim_end_matches(['\r', '\n']).to_string());
                    }
                }
            }
            out
        })
        .collect()
}

// ---- Compiling ------------------------------------------------------------------------------------------------

/// A page's size in typographic points (72 to the inch).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageInfo {
    pub width_pt: f32,
    pub height_pt: f32,
}

/// A page drawn: premultiplied RGBA, row after row.
#[derive(Clone, Debug)]
pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Rendered {
    /// The pixels as GPUI draws them: premultiplied BGRA (red and blue swapped).
    pub fn into_bgra(mut self) -> Vec<u8> {
        for pixel in self.rgba.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        self.rgba
    }
}

/// A typeset document: its pages, drawn or written out on demand. It lives on the thread that made it (it holds
/// the laid out pages, which are not sent anywhere).
pub struct Compiled {
    pages: Vec<PageInfo>,
    /// What Typst warned about (the document was made anyway).
    pub warnings: Vec<Diagnostic>,
    /// The files it read besides itself: imports, pictures, data.
    pub dependencies: Vec<PathBuf>,
    render: Box<dyn Fn(usize, f64) -> Option<Rendered>>,
    pdf: Box<dyn Fn() -> Result<Vec<u8>, Failure>>,
}

impl Compiled {
    /// The pages' sizes.
    pub fn pages(&self) -> &[PageInfo] {
        &self.pages
    }

    /// Page `index` (0 is the first) drawn at `pixels_per_pt` device pixels to the point (4/3 is 96 dpi), cut down
    /// to [`MAX_RENDER_PIXELS`]. None when there is no such page.
    pub fn render(&self, index: usize, pixels_per_pt: f64) -> Option<Rendered> {
        let info = self.pages.get(index)?;
        let area = f64::from(info.width_pt) * f64::from(info.height_pt);
        let wanted = pixels_per_pt.max(0.01);
        let scale = if area * wanted * wanted > MAX_RENDER_PIXELS {
            (MAX_RENDER_PIXELS / area).sqrt()
        } else {
            wanted
        };
        catch_unwind(AssertUnwindSafe(|| (self.render)(index, scale)))
            .ok()
            .flatten()
    }

    /// The document as a PDF.
    pub fn pdf(&self) -> Result<Vec<u8>, Failure> {
        catch_unwind(AssertUnwindSafe(|| (self.pdf)())).unwrap_or_else(|_| Err(Failure::of(Reason::Crashed)))
    }
}

/// Typesets `input`. Call it on a background thread (see [`run_big`]); it waits for the font scan, then typesets.
pub fn compile(input: Input, limits: &Limits) -> Result<Compiled, Failure> {
    limits.check().map_err(Failure::of)?;
    let fonts = shared_fonts();
    limits.check().map_err(Failure::of)?;
    let world = GlintWorld::new(input, fonts, limits.clone())?;
    let made = catch_unwind(AssertUnwindSafe(|| build(world)));
    // Typst memoizes what it computes in a cache shared by every compile; without this the cache only grows
    // (typst-cli evicts after each compile too).
    typst::comemo::evict(10);
    let outcome = made.map_err(|_| Failure::of(Reason::Crashed))?;
    match outcome {
        Ok(compiled) => Ok(compiled),
        Err((diagnostics, dependencies)) => {
            // A stop or a timeout looks like a file that couldn't be read; say what really happened.
            let reason = limits.check().err().unwrap_or(Reason::Errors);
            if reason == Reason::Errors {
                Err(Failure {
                    reason,
                    diagnostics,
                    dependencies,
                })
            } else {
                Err(Failure::of(reason))
            }
        }
    }
}

/// What a compile that failed knows: Typst's messages, and the files it read.
type Rejected = (Vec<Diagnostic>, Vec<PathBuf>);

/// The document's creation time for the PDF's metadata: now, in UTC.
fn pdf_timestamp() -> Option<typst_pdf::Timestamp> {
    today(Some(0.)).map(typst_pdf::Timestamp::new_utc)
}

/// Compiles `world` and wraps the document. The document's type (`PagedDocument`, from a crate this one doesn't
/// name) is worked out by the compiler from how it is used below.
fn build(mut world: GlintWorld) -> Result<Compiled, Rejected> {
    let Warned { output, warnings } = typst::compile(&world);
    let doc = match output {
        Ok(doc) => Arc::new(doc),
        Err(errors) => {
            let diagnostics = convert(&world, errors);
            return Err((diagnostics, world.dependencies()));
        }
    };
    let warnings = convert(&world, warnings);
    let dependencies = world.dependencies();
    let world = Arc::new(world);

    // The PDF closure comes first: it is what tells the compiler the document's type.
    let for_pdf = Arc::clone(&doc);
    let for_errors = Arc::clone(&world);
    let pdf = Box::new(move || {
        let options = typst_pdf::PdfOptions {
            timestamp: pdf_timestamp(),
            ..Default::default()
        };
        typst_pdf::pdf(&for_pdf, &options).map_err(|errors| Failure {
            reason: Reason::Errors,
            diagnostics: convert(&for_errors, errors),
            dependencies: Vec::new(),
        })
    });

    let pages: Vec<PageInfo> = doc
        .pages()
        .iter()
        .map(|page| {
            let size = page.frame.size();
            PageInfo {
                width_pt: size.x.to_pt() as f32,
                height_pt: size.y.to_pt() as f32,
            }
        })
        .collect();

    let for_render = doc;
    let render = Box::new(move |index: usize, pixels_per_pt: f64| {
        let page = for_render.pages().get(index)?;
        let options = typst_render::RenderOptions {
            pixel_per_pt: Scalar::new(pixels_per_pt),
            render_bleed: false,
        };
        let pixmap = typst_render::render(page, &options);
        Some(Rendered {
            width: pixmap.width(),
            height: pixmap.height(),
            rgba: pixmap.data().to_vec(),
        })
    });

    Ok(Compiled {
        pages,
        warnings,
        dependencies,
        render,
        pdf,
    })
}

// ---- Threads --------------------------------------------------------------------------------------------------

/// Starts `work` on a thread with a stack big enough for Typst, called `name`.
pub fn run_big<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<T>> {
    std::thread::Builder::new()
        .name(name.into())
        .stack_size(STACK_BYTES)
        .spawn(work)
}

/// Typesets `input` and writes the PDF: the whole export, on a thread of its own, waiting for it. Blocks the
/// calling thread, which must not be the UI thread.
pub fn pdf_of(input: Input, limits: &Limits) -> Result<Vec<u8>, Failure> {
    let limits = limits.clone();
    let handle = run_big("glint-typst-pdf", move || {
        compile(input, &limits).and_then(|doc| doc.pdf())
    })
    .map_err(|e| Failure::message(format!("Couldn't start Typst: {e}")))?;
    handle.join().unwrap_or_else(|_| Err(Failure::of(Reason::Crashed)))
}

// ---- Markdown as Typst ----------------------------------------------------------------------------------------

/// Markdown written as Typst markup, for Export as PDF (replacing the WebView print of the preview).
pub mod from_markdown {
    use super::*;
    use glint_model::markdown::{markdown_title, safe_href};
    use markdown::mdast::{AlignKind, Node};
    use markdown::{Constructs, ParseOptions};
    use std::collections::HashMap;

    /// What the conversion needs to know besides the text.
    #[derive(Clone, Debug, Default)]
    pub struct Options {
        /// The document's title for the PDF's properties; the first heading when none is given.
        pub title: Option<String>,
        /// The folder pictures are found in. Without it every relative picture is trusted to be there; with it a
        /// picture that isn't there (or isn't a kind Typst draws) is replaced by its description.
        pub base_dir: Option<PathBuf>,
    }

    /// A reading direction and the language Typst should typeset it as.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Dir {
        Ltr,
        /// Right to left in this language ("ar", "he").
        Rtl(&'static str),
    }

    impl Dir {
        /// `dir: rtl, lang: "ar"`, or `dir: ltr, lang: "en"`.
        fn settings(self) -> String {
            match self {
                Dir::Ltr => "dir: ltr, lang: \"en\"".to_string(),
                Dir::Rtl(lang) => format!("dir: rtl, lang: \"{lang}\""),
            }
        }
    }

    /// The direction `text` is mostly written in, or None when it has no letters to tell by. Arabic script (also
    /// Persian and Urdu) is "ar", Hebrew "he"; a tie goes to left to right (docconvert.rs's `direction_of`, which
    /// only asks about whole documents).
    pub fn direction_of(text: &str) -> Option<Dir> {
        let (mut arabic, mut hebrew, mut other) = (0usize, 0usize, 0usize);
        for c in text.chars() {
            match c {
                '\u{0600}'..='\u{06FF}'
                | '\u{0750}'..='\u{077F}'
                | '\u{08A0}'..='\u{08FF}'
                | '\u{FB50}'..='\u{FDFF}'
                | '\u{FE70}'..='\u{FEFF}' => {
                    if c.is_alphabetic() {
                        arabic += 1;
                    }
                }
                '\u{0590}'..='\u{05FF}' | '\u{FB1D}'..='\u{FB4F}' => hebrew += 1,
                c if c.is_alphabetic() => other += 1,
                _ => {}
            }
        }
        if arabic + hebrew + other == 0 {
            None
        } else if arabic > other && arabic >= hebrew {
            Some(Dir::Rtl("ar"))
        } else if hebrew > other {
            Some(Dir::Rtl("he"))
        } else {
            Some(Dir::Ltr)
        }
    }

    /// Typst string syntax for `text`: quoted, with the characters that need it escaped.
    pub fn string_literal(text: &str) -> String {
        let mut out = String::with_capacity(text.len() + 2);
        out.push('"');
        for c in text.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    /// `text` as Typst markup that reads as exactly that text. `start` says the text begins a line, where `=`,
    /// `-`, `+`, `/` and `1.` would start a heading, a list or a term. A soft line break stays a line break, so
    /// the next line is a start too.
    pub fn escape_markup(text: &str, start: bool) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len() + 8);
        let mut at_start = start;
        // Where the current line begins in `chars`; the first line begins a line only when `start` says so.
        let mut line_start = 0usize;
        for (i, &c) in chars.iter().enumerate() {
            let next = chars.get(i + 1).copied();
            let before = i.checked_sub(1).and_then(|j| chars.get(j)).copied();
            let escape = match c {
                '\\' | '#' | '$' | '*' | '_' | '`' | '@' | '<' | '>' | '[' | ']' | '~' => true,
                '=' | '+' | '-' | '/' if at_start => true,
                // "--" and "---" become dashes; "//" and "/*" start comments.
                '-' => next == Some('-') || before == Some('-'),
                '/' => next == Some('/') || next == Some('*'),
                // "..." becomes an ellipsis: keep it three dots.
                '.' => before != Some('.') && next == Some('.') && chars.get(i + 2) == Some(&'.'),
                _ => false,
            };
            // "12." at the start of a line is an enumeration.
            let numbered = c == '.'
                && (line_start > 0 || start)
                && i > line_start
                && chars[line_start..i].iter().all(char::is_ascii_digit);
            if escape || numbered {
                out.push('\\');
            }
            out.push(c);
            if c == '\n' {
                at_start = true;
                line_start = i + 1;
            } else if !c.is_whitespace() {
                at_start = false;
            }
        }
        out
    }

    /// The text of `nodes` for telling the direction (not code).
    fn plain_text(nodes: &[Node], out: &mut String) {
        for node in nodes {
            match node {
                Node::Text(t) => {
                    out.push_str(&t.value);
                    out.push(' ');
                }
                Node::Image(i) => out.push_str(&i.alt),
                Node::Code(_) | Node::InlineCode(_) | Node::Html(_) | Node::Math(_) | Node::InlineMath(_) => {}
                other => {
                    if let Some(children) = other.children() {
                        plain_text(children, out);
                    }
                }
            }
        }
    }

    fn text_of(nodes: &[Node]) -> String {
        let mut text = String::new();
        plain_text(nodes, &mut text);
        text
    }

    /// What was made: the document's direction and the markup of its body.
    pub struct Converted {
        pub dir: Dir,
        pub title: Option<String>,
        pub body: String,
    }

    /// Typst markup for the Markdown `source`, with the page set-up first ([`preamble`]).
    pub fn to_typst(source: &str, options: &Options) -> String {
        let converted = convert(source, options);
        let mut out = preamble(converted.dir, converted.title.as_deref());
        out.push_str(&converted.body);
        out.push('\n');
        out
    }

    /// The body alone: what [`to_typst`] writes after the page set-up.
    pub fn convert(source: &str, options: &Options) -> Converted {
        let parse = ParseOptions {
            constructs: Constructs {
                frontmatter: true,
                ..Constructs::gfm()
            },
            ..ParseOptions::default()
        };
        let tree = match markdown::to_mdast(source, &parse) {
            Ok(tree) => tree,
            // Plain Markdown can't fail to parse; if it ever does, the text is shown as it is.
            Err(_) => {
                return Converted {
                    dir: Dir::Ltr,
                    title: options.title.clone(),
                    body: format!("#raw(block: true, {})", string_literal(source)),
                }
            }
        };
        let mut converter = Converter {
            options,
            definitions: HashMap::new(),
            footnotes: HashMap::new(),
            doc: Dir::Ltr,
        };
        converter.collect(&tree);
        let children: &[Node] = tree.children().map_or(&[], Vec::as_slice);
        converter.doc = direction_of(&text_of(children)).unwrap_or(Dir::Ltr);
        let body = converter.blocks(children);
        let title = options
            .title
            .clone()
            .or_else(|| Some(markdown_title(source)).filter(|t| !t.is_empty()));
        Converted {
            dir: converter.doc,
            title,
            body,
        }
    }

    /// The page, the fonts and the look of each part: a document that reads like the preview (sans type, grey code
    /// blocks, a rule beside quotes). `dir` is the document's reading direction.
    pub fn preamble(dir: Dir, title: Option<&str>) -> String {
        let mut out = String::new();
        if let Some(title) = title.filter(|t| !t.trim().is_empty()) {
            out.push_str(&format!("#set document(title: {})\n", string_literal(title)));
        }
        out.push_str("#set page(paper: \"a4\", margin: (x: 2.2cm, y: 2.4cm), numbering: \"1\")\n");
        let direction = match dir {
            Dir::Ltr => String::new(),
            Dir::Rtl(_) => format!(", {}", dir.settings()),
        };
        out.push_str(&format!(
            "#set text(font: (\"Segoe UI\", \"Arial\", \"Tahoma\", \"Segoe UI Symbol\", \"Segoe UI Emoji\"), size: 11pt{direction})\n"
        ));
        out.push_str(
            "#set par(leading: 0.68em, spacing: 1.1em)\n\
             #show heading: set block(above: 1.5em, below: 0.8em)\n\
             #show heading.where(level: 1): set text(size: 22pt)\n\
             #show heading.where(level: 2): set text(size: 17pt)\n\
             #show heading.where(level: 3): set text(size: 14pt)\n\
             #show heading.where(level: 4): set text(size: 12pt)\n\
             #show link: set text(fill: rgb(\"#0b62d6\"))\n\
             #show raw: set text(font: (\"Cascadia Mono\", \"Consolas\", \"Courier New\"), size: 0.92em, dir: ltr)\n\
             #show raw.where(block: true): it => block(width: 100%, fill: luma(245), inset: 9pt, radius: 4pt, it)\n\
             #show raw.where(block: false): box.with(fill: luma(238), inset: (x: 3pt, y: 0pt), outset: (y: 3pt), radius: 2pt)\n\
             #show table.cell.where(y: 0): set text(weight: \"bold\")\n\n",
        );
        out
    }

    struct Converter<'a> {
        options: &'a Options,
        /// `[label]: url "title"` definitions, by identifier.
        definitions: HashMap<String, (String, Option<String>)>,
        /// Footnote definitions' content, by identifier.
        footnotes: HashMap<String, Vec<Node>>,
        doc: Dir,
    }

    impl Converter<'_> {
        /// Finds the definitions anywhere in the tree.
        fn collect(&mut self, node: &Node) {
            match node {
                Node::Definition(d) => {
                    self.definitions
                        .entry(d.identifier.clone())
                        .or_insert((d.url.clone(), d.title.clone()));
                }
                Node::FootnoteDefinition(d) => {
                    self.footnotes
                        .entry(d.identifier.clone())
                        .or_insert_with(|| d.children.clone());
                }
                _ => {}
            }
            if let Some(children) = node.children() {
                for child in children {
                    self.collect(child);
                }
            }
        }

        // ---- Blocks ----

        /// Blocks one after the other, a blank line between.
        fn blocks(&self, nodes: &[Node]) -> String {
            nodes
                .iter()
                .filter_map(|node| self.block(node))
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n\n")
        }

        /// `body` as a block that reads in `dir` when that isn't the document's.
        fn directed(&self, text: &str, body: String) -> String {
            match direction_of(text) {
                Some(dir) if dir != self.doc => {
                    format!("#block(width: 100%)[\n#set text({})\n{body}\n]", dir.settings())
                }
                _ => body,
            }
        }

        fn block(&self, node: &Node) -> Option<String> {
            Some(match node {
                Node::Paragraph(p) => {
                    let body = self.inlines(&p.children);
                    self.directed(&text_of(&p.children), body)
                }
                Node::Heading(h) => {
                    let marks = "=".repeat(usize::from(h.depth.clamp(1, 6)));
                    let body = self.inlines(&h.children).replace('\n', " ");
                    self.directed(&text_of(&h.children), format!("{marks} {body}"))
                }
                Node::Blockquote(q) => {
                    let side = if matches!(self.quote_dir(&q.children), Dir::Rtl(_)) {
                        "right"
                    } else {
                        "left"
                    };
                    let body = self.blocks(&q.children);
                    self.directed(
                        &text_of(&q.children),
                        format!(
                            "#block(inset: ({side}: 1em, y: 0.2em), stroke: ({side}: 2.5pt + luma(170)), width: 100%)[\n{body}\n]"
                        ),
                    )
                }
                Node::List(list) => {
                    let items: Vec<String> = list.children.iter().map(|item| self.list_item(item)).collect();
                    let head = if list.ordered {
                        match list.start.filter(|&n| n != 1) {
                            Some(n) => format!("#enum(start: {n}, tight: {},\n", !list.spread),
                            None => format!("#enum(tight: {},\n", !list.spread),
                        }
                    } else {
                        format!("#list(tight: {},\n", !list.spread)
                    };
                    let body = format!("{head}{}\n)", items.join(",\n"));
                    self.directed(&text_of(&list.children), body)
                }
                Node::Code(code) => {
                    let lang = code
                        .lang
                        .as_deref()
                        .and_then(|l| l.split_whitespace().next())
                        .filter(|l| !l.is_empty());
                    match lang {
                        Some(lang) => format!(
                            "#raw(block: true, lang: {}, {})",
                            string_literal(lang),
                            string_literal(&code.value)
                        ),
                        None => format!("#raw(block: true, {})", string_literal(&code.value)),
                    }
                }
                Node::Math(math) => format!("#raw(block: true, {})", string_literal(&math.value)),
                Node::ThematicBreak(_) => "#line(length: 100%, stroke: 0.6pt + luma(170))".to_string(),
                Node::Table(table) => self.table(table),
                Node::Html(html) => {
                    let text = html.value.trim();
                    if text.is_empty() || (text.starts_with("<!--") && text.ends_with("-->")) {
                        return None;
                    }
                    format!("#raw(block: true, lang: \"html\", {})", string_literal(text))
                }
                // Reference material, front matter and MDX: nothing to print.
                Node::Definition(_)
                | Node::FootnoteDefinition(_)
                | Node::Yaml(_)
                | Node::Toml(_)
                | Node::MdxjsEsm(_)
                | Node::MdxFlowExpression(_)
                | Node::MdxJsxFlowElement(_) => return None,
                // Anything phrasing that sits where a block is expected is a paragraph of its own.
                other => self.inlines(std::slice::from_ref(other)),
            })
        }

        /// The direction a quote reads in (its own, or the document's).
        fn quote_dir(&self, children: &[Node]) -> Dir {
            direction_of(&text_of(children)).unwrap_or(self.doc)
        }

        fn list_item(&self, item: &Node) -> String {
            let Node::ListItem(item) = item else {
                return "[]".to_string();
            };
            let mut body = self.blocks(&item.children);
            if let Some(checked) = item.checked {
                let mark = if checked { "☑" } else { "☐" };
                body = if body.is_empty() {
                    mark.to_string()
                } else {
                    format!("{mark} {body}")
                };
            }
            format!("[{body}]")
        }

        fn table(&self, table: &markdown::mdast::Table) -> String {
            let columns = table
                .children
                .iter()
                .filter_map(|row| row.children().map(Vec::len))
                .max()
                .unwrap_or(0);
            if columns == 0 {
                return String::new();
            }
            let align: Vec<&str> = (0..columns)
                .map(|i| match table.align.get(i) {
                    Some(AlignKind::Left) => "left",
                    Some(AlignKind::Right) => "right",
                    Some(AlignKind::Center) => "center",
                    _ => "auto",
                })
                .collect();
            let mut rows: Vec<String> = Vec::new();
            for (n, row) in table.children.iter().enumerate() {
                let cells: Vec<String> = (0..columns)
                    .map(|i| {
                        let content = row
                            .children()
                            .and_then(|cells| cells.get(i))
                            .map(|cell| self.inlines(cell.children().map_or(&[], Vec::as_slice)))
                            .unwrap_or_default();
                        format!("[{content}]")
                    })
                    .collect();
                rows.push(if n == 0 {
                    format!("  table.header({}),", cells.join(", "))
                } else {
                    format!("  {},", cells.join(", "))
                });
            }
            let body = format!(
                "#table(\n  columns: {columns},\n  align: ({}),\n  stroke: 0.5pt + luma(190),\n  inset: 6pt,\n{}\n)",
                align.join(", ") + if columns == 1 { "," } else { "" },
                rows.join("\n")
            );
            let text: String = table
                .children
                .iter()
                .map(|row| text_of(std::slice::from_ref(row)))
                .collect();
            self.directed(&text, body)
        }

        // ---- Inline content ----

        /// Phrasing content as markup, starting at the start of a line.
        fn inlines(&self, nodes: &[Node]) -> String {
            let mut out = String::new();
            let mut start = true;
            self.inlines_into(nodes, &mut out, &mut start);
            out
        }

        fn inlines_into(&self, nodes: &[Node], out: &mut String, start: &mut bool) {
            for node in nodes {
                match node {
                    Node::Text(t) => {
                        out.push_str(&escape_markup(&t.value, *start));
                        if t.value.contains(|c: char| !c.is_whitespace()) || t.value.ends_with('\n') {
                            *start = t.value.trim_end_matches([' ', '\t']).ends_with('\n');
                        }
                    }
                    Node::Break(_) => {
                        out.push_str("\\\n");
                        *start = true;
                    }
                    Node::InlineCode(c) => {
                        out.push_str(&format!("#raw({})", string_literal(&c.value)));
                        *start = false;
                    }
                    Node::InlineMath(m) => {
                        out.push_str(&format!("#raw({})", string_literal(&m.value)));
                        *start = false;
                    }
                    Node::Emphasis(e) => {
                        out.push_str(&format!("#emph[{}]", self.inlines(&e.children)));
                        *start = false;
                    }
                    Node::Strong(s) => {
                        out.push_str(&format!("#strong[{}]", self.inlines(&s.children)));
                        *start = false;
                    }
                    Node::Delete(d) => {
                        out.push_str(&format!("#strike[{}]", self.inlines(&d.children)));
                        *start = false;
                    }
                    Node::Link(l) => {
                        out.push_str(&self.link(&l.url, &l.children));
                        *start = false;
                    }
                    Node::LinkReference(r) => {
                        match self.definitions.get(&r.identifier) {
                            Some((url, _)) => out.push_str(&self.link(url, &r.children)),
                            // No such definition: the brackets were just text.
                            None => out.push_str(&self.inlines(&r.children)),
                        }
                        *start = false;
                    }
                    Node::Image(i) => {
                        out.push_str(&self.image(&i.url, &i.alt));
                        *start = false;
                    }
                    Node::ImageReference(i) => {
                        match self.definitions.get(&i.identifier) {
                            Some((url, _)) => out.push_str(&self.image(url, &i.alt)),
                            None => out.push_str(&escape_markup(&i.alt, false)),
                        }
                        *start = false;
                    }
                    Node::FootnoteReference(r) => {
                        if let Some(content) = self.footnotes.get(&r.identifier) {
                            out.push_str(&format!("#footnote[{}]", self.blocks(content)));
                        }
                        *start = false;
                    }
                    Node::Html(h) => {
                        // Inline HTML: a line break becomes one, every other tag is dropped and its text stays.
                        let tag = h.value.trim().to_ascii_lowercase();
                        if matches!(tag.as_str(), "<br>" | "<br/>" | "<br />") {
                            out.push_str("\\\n");
                            *start = true;
                        }
                    }
                    other => {
                        if let Some(children) = other.children() {
                            self.inlines_into(children, out, start);
                        }
                    }
                }
            }
        }

        /// `children` as a link to `url` when it is one a PDF may carry (web and e-mail addresses); else the text.
        fn link(&self, url: &str, children: &[Node]) -> String {
            let text = self.inlines(children);
            let lower = url.trim().to_ascii_lowercase();
            match safe_href(url) {
                Some(href)
                    if lower.starts_with("http:") || lower.starts_with("https:") || lower.starts_with("mailto:") =>
                {
                    format!("#link({})[{text}]", string_literal(href))
                }
                _ => text,
            }
        }

        /// A picture from a file beside the document, else its description.
        fn image(&self, url: &str, alt: &str) -> String {
            let described = || {
                if alt.trim().is_empty() {
                    String::new()
                } else {
                    format!("#emph[{}]", escape_markup(alt, false))
                }
            };
            let path = percent_decode(url.trim().trim_start_matches('<').trim_end_matches('>'));
            let lower = path.to_ascii_lowercase();
            let drawn = ["png", "jpg", "jpeg", "gif", "svg", "webp"]
                .iter()
                .any(|ext| lower.ends_with(&format!(".{ext}")));
            // Pictures online are not fetched; Typst draws only these kinds.
            if path.is_empty() || path.contains("://") || path.starts_with("data:") || !drawn {
                return described();
            }
            let relative = path.replace('\\', "/");
            if relative.starts_with('/') || relative.contains(':') {
                return described();
            }
            if let Some(dir) = &self.options.base_dir {
                let candidate = dir.join(&relative);
                if !candidate.is_file() {
                    return described();
                }
            }
            if alt.trim().is_empty() {
                format!("#image({})", string_literal(&relative))
            } else {
                format!("#image({}, alt: {})", string_literal(&relative), string_literal(alt))
            }
        }
    }

    /// `%20` and its kind undone; a stray `%` stays.
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
        String::from_utf8_lossy(&out).into_owned()
    }
}

// ---- Tests ----------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::from_markdown::{self, Dir, Options};
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A folder of its own under the temp folder, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "glint-typst-test-{}-{tag}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("making {}: {e}", dir.display()));
            Scratch(dir)
        }

        fn file(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.join(name);
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::write(&path, text).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Compiles on a big stack, as the app does, and hands back what the test checks (so `Compiled` need not move).
    fn on_big<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        run_big("test", work)
            .unwrap_or_else(|e| panic!("thread: {e}"))
            .join()
            .unwrap_or_else(|_| panic!("the test's thread panicked"))
    }

    /// The left-most and right-most columns that hold ink (anything not white) on a rendered page.
    fn ink_columns(page: &Rendered) -> Option<(u32, u32)> {
        let mut first: Option<u32> = None;
        let mut last: Option<u32> = None;
        for (i, px) in page.rgba.as_chunks::<4>().0.iter().enumerate() {
            if px[0] < 200 || px[1] < 200 || px[2] < 200 {
                let x = (i as u32) % page.width;
                first = Some(first.map_or(x, |f| f.min(x)));
                last = Some(last.map_or(x, |l| l.max(x)));
            }
        }
        first.zip(last)
    }

    #[test]
    fn the_pcs_fonts_are_found_once_and_shared() {
        let a = shared_fonts();
        let b = shared_fonts();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(a.faces() > 0, "this PC has fonts");
        assert!(fonts_ready());
    }

    #[test]
    fn a_document_becomes_pdf_bytes() {
        let dir = Scratch::new("pdf");
        let path = dir.file("hello.typ", "= Hello\nSome *typeset* text, and a list:\n- one\n- two\n");
        let bytes = on_big(move || pdf_of(Input::File(path), &Limits::unlimited()));
        let bytes = bytes.unwrap_or_else(|f| panic!("{f}"));
        assert!(bytes.starts_with(b"%PDF-"), "{:?}", &bytes[..bytes.len().min(8)]);
        assert!(bytes.len() > 500);
        assert!(bytes.windows(5).any(|w| w == b"%%EOF"));
    }

    #[test]
    fn a_page_is_drawn_with_ink_on_white() {
        let dir = Scratch::new("render");
        let path = dir.file("page.typ", "= Title\nA line of text.\n");
        let (pages, drawn) = on_big(move || {
            let compiled = compile(Input::File(path), &Limits::unlimited()).unwrap_or_else(|f| panic!("{f}"));
            let pages = compiled.pages().to_vec();
            (pages, compiled.render(0, 1.0))
        });
        assert_eq!(pages.len(), 1);
        // A4 in points.
        assert!((pages[0].width_pt - 595.3).abs() < 1.0 && (pages[0].height_pt - 841.9).abs() < 1.0);
        let page = drawn.unwrap_or_else(|| panic!("page 1 wasn't drawn"));
        assert_eq!((page.width, page.height), (595, 842));
        assert_eq!(page.rgba.len(), 595 * 842 * 4);
        assert!(ink_columns(&page).is_some(), "the text left ink on the page");
        // White paper in the corner.
        assert_eq!(&page.rgba[..4], &[255, 255, 255, 255]);
        assert!(page
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[0] < 100 && p[1] < 100 && p[2] < 100));
    }

    #[test]
    fn the_pixels_gpui_draws_are_blue_green_red() {
        let page = Rendered {
            width: 2,
            height: 1,
            rgba: vec![10, 20, 30, 255, 1, 2, 3, 4],
        };
        assert_eq!(page.into_bgra(), [30, 20, 10, 255, 3, 2, 1, 4]);
    }

    #[test]
    fn a_red_box_is_red_in_the_pixels_and_blue_first_after_the_swap() {
        let dir = Scratch::new("red");
        let path = dir.file(
            "red.typ",
            "#set page(width: 4cm, height: 4cm, margin: 0pt)
#rect(width: 100%, height: 100%, fill: rgb(\"#ff0000\"), stroke: none)
",
        );
        let page = on_big(move || {
            let compiled = compile(Input::File(path), &Limits::unlimited()).unwrap_or_else(|f| panic!("{f}"));
            compiled.render(0, 1.0)
        })
        .unwrap_or_else(|| panic!("drawn"));
        let middle = (page.width / 2 + (page.height / 2) * page.width) as usize * 4;
        assert_eq!(&page.rgba[middle..middle + 4], &[255, 0, 0, 255]);
        let bgra = page.into_bgra();
        assert_eq!(&bgra[middle..middle + 4], &[0, 0, 255, 255]);
    }

    #[test]
    fn many_pages_have_their_own_sizes_and_a_page_past_the_end_is_none() {
        let dir = Scratch::new("pages");
        let path = dir.file(
            "pages.typ",
            "#set page(width: 10cm, height: 6cm)\nFirst\n#pagebreak()\n#set page(width: 20cm, height: 8cm)\nSecond\n",
        );
        let pages = on_big(move || {
            let compiled = compile(Input::File(path), &Limits::unlimited()).unwrap_or_else(|f| panic!("{f}"));
            assert!(compiled.render(5, 1.0).is_none());
            compiled.pages().to_vec()
        });
        assert_eq!(pages.len(), 2);
        assert!((pages[0].width_pt - 283.46).abs() < 0.5);
        assert!((pages[1].width_pt - 566.93).abs() < 0.5, "{pages:?}");
    }

    #[test]
    fn a_huge_page_is_cut_down_to_the_pixel_budget() {
        let dir = Scratch::new("huge");
        let path = dir.file("huge.typ", "#set page(width: 100cm, height: 100cm)\nPoster\n");
        let page = on_big(move || {
            let compiled = compile(Input::File(path), &Limits::unlimited()).unwrap_or_else(|f| panic!("{f}"));
            compiled.render(0, 8.0)
        });
        let page = page.unwrap_or_else(|| panic!("drawn"));
        assert!(f64::from(page.width) * f64::from(page.height) <= MAX_RENDER_PIXELS * 1.01);
        assert!(page.width > 1000);
    }

    #[test]
    fn errors_say_where_in_the_file() {
        let dir = Scratch::new("errors");
        let path = dir.file("broken.typ", "= Fine\n\nThis is fine.\n  #nosuchfunction(1)\n");
        let failure = on_big(move || compile(Input::File(path), &Limits::unlimited()).err());
        let failure = failure.unwrap_or_else(|| panic!("it should not compile"));
        assert_eq!(failure.reason, Reason::Errors);
        let first = &failure.diagnostics[0];
        assert_eq!(first.severity, Severity::Error);
        assert!(first.message.contains("nosuchfunction"), "{}", first.message);
        assert_eq!(first.file.as_deref(), Some("broken.typ"));
        assert_eq!(first.line, Some(4));
        // The column counts characters from 1: two spaces, then "#".
        assert_eq!(first.column, Some(4), "{first:?}");
        assert_eq!(first.line_text.as_deref(), Some("  #nosuchfunction(1)"));
        assert_eq!(first.place().as_deref(), Some("broken.typ:4:4"));
        assert!(failure.summary().starts_with("broken.typ:4:4: "));
    }

    #[test]
    fn an_error_in_an_imported_file_names_that_file() {
        let dir = Scratch::new("import");
        dir.file("parts/inner.typ", "#let ok = 1\n#oops()\n");
        let path = dir.file("main.typ", "#include \"parts/inner.typ\"\n");
        let failure = on_big(move || compile(Input::File(path), &Limits::unlimited()).err());
        let failure = failure.unwrap_or_else(|| panic!("it should not compile"));
        let at = failure
            .diagnostics
            .iter()
            .find(|d| d.file.as_deref() == Some("parts/inner.typ"))
            .unwrap_or_else(|| panic!("{:?}", failure.diagnostics));
        assert_eq!(at.line, Some(2));
        // A failed document still says what it read, so a fix to any of it can be noticed.
        assert!(
            failure.dependencies.iter().any(|d| d.ends_with("inner.typ")),
            "{:?}",
            failure.dependencies
        );
        assert!(
            failure.dependencies.iter().any(|d| d.ends_with("main.typ")),
            "{:?}",
            failure.dependencies
        );
    }

    #[test]
    fn arabic_error_lines_keep_their_text_and_columns_count_characters() {
        let dir = Scratch::new("arabic-error");
        let path = dir.file("خطأ.typ", "مرحبا بالعالم #هذا_غير_موجود\n");
        let failure = on_big(move || compile(Input::File(path), &Limits::unlimited()).err());
        let failure = failure.unwrap_or_else(|| panic!("it should not compile"));
        let first = &failure.diagnostics[0];
        assert_eq!(first.file.as_deref(), Some("خطأ.typ"));
        assert_eq!(first.line, Some(1));
        // "مرحبا بالعالم " is 14 characters, so the error starts at column 15 (or inside the identifier).
        assert!(first.column.is_some_and(|c| c >= 15), "{first:?}");
        assert_eq!(first.line_text.as_deref(), Some("مرحبا بالعالم #هذا_غير_موجود"));
    }

    #[test]
    fn a_package_import_is_a_clear_error_and_never_goes_online() {
        let dir = Scratch::new("package");
        let path = dir.file("uses.typ", "#import \"@preview/cetz:0.3.4\": canvas\n= Drawing\n");
        let failure = on_big(move || compile(Input::File(path), &Limits::unlimited()).err());
        let failure = failure.unwrap_or_else(|| panic!("a package can't be imported"));
        let text = failure
            .diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("package"), "{text}");
        assert!(text.contains("without going online"), "{text}");
        assert!(text.contains("@preview/cetz:0.3.4"), "{text}");
        assert_eq!(failure.diagnostics[0].line, Some(1));
    }

    #[test]
    fn files_beside_the_document_are_read_and_files_outside_are_not() {
        // The secret is in the folder above the document's.
        let outer = Scratch::new("root");
        outer.file("secret.txt", "secret");
        outer.file("inner/data.txt", "beside it");
        outer.file("inner/sub/more.typ", "included text");
        let ok = outer.file(
            "inner/ok.typ",
            "#read(\"data.txt\")

#include \"sub/more.typ\"
",
        );
        let outside = outer.file(
            "inner/bad.typ",
            "#read(\"../secret.txt\")
",
        );
        let (ok_result, bad) = on_big(move || {
            (
                compile(Input::File(ok), &Limits::unlimited()).map(|c| c.dependencies),
                compile(Input::File(outside), &Limits::unlimited()).err(),
            )
        });
        let dependencies = ok_result.unwrap_or_else(|f| panic!("{f}"));
        // The document itself, the file it reads and the one it includes.
        assert_eq!(dependencies.len(), 3, "{dependencies:?}");
        assert!(dependencies.iter().any(|d| d.ends_with("ok.typ")));
        assert!(dependencies.iter().any(|d| d.ends_with("data.txt")));
        assert!(dependencies.iter().any(|d| d.ends_with("more.typ")));
        let bad = bad.unwrap_or_else(|| panic!("a file outside the folder must not be read"));
        let text = bad.to_string();
        assert!(text.contains("escapes") || text.contains("outside"), "{text}");
    }

    #[test]
    fn a_stop_or_an_old_deadline_ends_the_compile_before_it_starts() {
        let dir = Scratch::new("limits");
        let path = dir.file("a.typ", "= A\n");
        let stopped = Limits::unlimited();
        stopped.stop();
        let a = path.clone();
        let result = on_big(move || compile(Input::File(a), &stopped).err());
        assert_eq!(result.map(|f| f.reason), Some(Reason::Cancelled));
        let late = Limits::new(Duration::ZERO);
        let result = on_big(move || compile(Input::File(path), &late).err());
        assert_eq!(result.map(|f| f.reason), Some(Reason::TimedOut));
    }

    #[test]
    fn a_stop_while_it_works_is_noticed_the_next_time_a_file_is_read() {
        let dir = Scratch::new("midway");
        for i in 0..40 {
            dir.file(&format!("p{i}.typ"), &format!("part {i}\n"));
        }
        let includes: String = (0..40).map(|i| format!("#include \"p{i}.typ\"\n")).collect();
        let path = dir.file("many.typ", &includes);
        let limits = Limits::unlimited();
        let stopper = limits.clone();
        let result = on_big(move || {
            // Stop it as soon as it has begun reading.
            let flag = stopper.clone();
            let watcher = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1));
                flag.stop();
            });
            let result = compile(Input::File(path), &stopper).err();
            let _ = watcher.join();
            result
        });
        // Either it was stopped, or it was quick enough to finish first; it never reports a made-up error.
        if let Some(failure) = result {
            assert_eq!(failure.reason, Reason::Cancelled, "{failure}");
        }
    }

    #[test]
    fn today_is_a_real_date_and_the_shifted_day_wraps() {
        assert!(today(None).is_some());
        assert!(today(Some(0.)).is_some());
        assert!(today(Some(5.)).is_some());
        assert!(today(Some(-5.)).is_some());
        assert_eq!(shifted_day(2025, 12, 31, 1), (2026, 1, 1));
        assert_eq!(shifted_day(2026, 1, 1, -1), (2025, 12, 31));
        assert_eq!(shifted_day(2024, 2, 28, 1), (2024, 2, 29));
        assert_eq!(shifted_day(2025, 2, 28, 1), (2025, 3, 1));
        assert_eq!(shifted_day(2025, 3, 1, -1), (2025, 2, 28));
        assert_eq!(shifted_day(2025, 6, 15, 0), (2025, 6, 15));
    }

    #[test]
    fn an_arabic_paragraph_reads_from_the_right_and_a_latin_one_from_the_left() {
        let dir = Scratch::new("rtl");
        let arabic = dir.file(
            "ar.typ",
            "#set text(lang: \"ar\", font: (\"Segoe UI\", \"Arial\", \"Tahoma\"))\nمرحبا بالعالم\n",
        );
        let latin = dir.file("en.typ", "Hello world\n");
        let (ar, en) = on_big(move || {
            let draw = |path: PathBuf| {
                let compiled = compile(Input::File(path), &Limits::unlimited()).unwrap_or_else(|f| panic!("{f}"));
                compiled.render(0, 1.0).unwrap_or_else(|| panic!("drawn"))
            };
            (draw(arabic), draw(latin))
        });
        let (ar_first, ar_last) = ink_columns(&ar).unwrap_or_else(|| panic!("Arabic left no ink"));
        let (en_first, en_last) = ink_columns(&en).unwrap_or_else(|| panic!("Latin left no ink"));
        // Margins are 2.5 cm of 21 cm: the Latin line starts at the left margin, the Arabic one ends at the right.
        assert!(en_first < ar.width / 4, "Latin starts at the left: {en_first}");
        assert!(en_last < ar.width / 2, "Latin is short: {en_last}");
        assert!(ar_last > ar.width * 3 / 4, "Arabic ends at the right margin: {ar_last}");
        assert!(ar_first > ar.width / 2, "Arabic is short: {ar_first}");
    }

    // ---- Markdown as Typst ----

    fn md(source: &str) -> String {
        from_markdown::convert(source, &Options::default()).body
    }

    #[test]
    fn markup_is_escaped_so_it_reads_as_the_text_it_was() {
        use from_markdown::escape_markup as esc;
        assert_eq!(esc("plain text", true), "plain text");
        assert_eq!(esc("a # b $ c * d _ e", false), "a \\# b \\$ c \\* d \\_ e");
        assert_eq!(
            esc("`tick` @who <tag> [x]", false),
            "\\`tick\\` \\@who \\<tag\\> \\[x\\]"
        );
        assert_eq!(esc("back\\slash ~ tilde", false), "back\\\\slash \\~ tilde");
        // Line starts: heading, list, term and enumeration markers.
        assert_eq!(esc("= not a heading", true), "\\= not a heading");
        assert_eq!(esc("- not a list", true), "\\- not a list");
        assert_eq!(esc("+ not a list", true), "\\+ not a list");
        assert_eq!(esc("/ not: a term", true), "\\/ not: a term");
        assert_eq!(esc("1. not an enum", true), "1\\. not an enum");
        assert_eq!(esc("12. not an enum", true), "12\\. not an enum");
        // The same characters in the middle of a line are only characters.
        assert_eq!(esc("a = b - c + d / e", false), "a = b - c + d / e");
        assert_eq!(esc("1986. a year", false), "1986. a year");
        assert_eq!(esc("a1. b", true), "a1. b");
        // A soft line break makes the next line a start.
        assert_eq!(esc("one\n- two", false), "one\n\\- two");
        // Dashes, comments and ellipses keep their characters.
        assert_eq!(esc("a -- b --- c", false), "a \\-\\- b \\-\\-\\- c");
        assert_eq!(esc("ftp://host/*x*", false), "ftp:\\//host\\/\\*x\\*");
        assert_eq!(esc("wait...", false), "wait\\...");
        assert_eq!(esc("a. b.. c", false), "a. b.. c");
        // Quotes stay quotes (Typst makes them typographic) and Arabic is untouched.
        assert_eq!(esc("\"quoted\" it's", false), "\"quoted\" it's");
        assert_eq!(esc("مرحبا # بالعالم", false), "مرحبا \\# بالعالم");
    }

    #[test]
    fn strings_are_quoted_for_typst() {
        use from_markdown::string_literal as s;
        assert_eq!(s("plain"), "\"plain\"");
        assert_eq!(s("a \"b\" \\ c"), "\"a \\\"b\\\" \\\\ c\"");
        assert_eq!(s("line\nbreak\ttab\r"), "\"line\\nbreak\\ttab\\r\"");
        assert_eq!(s("\u{7}"), "\"\\u{7}\"");
        assert_eq!(s("مرحبا"), "\"مرحبا\"");
    }

    #[test]
    fn direction_follows_the_letters() {
        use from_markdown::direction_of as dir;
        assert_eq!(dir("Hello world"), Some(Dir::Ltr));
        assert_eq!(dir("مرحبا بالعالم"), Some(Dir::Rtl("ar")));
        assert_eq!(dir("שלום עולם"), Some(Dir::Rtl("he")));
        // Mostly Arabic with a few English words reads right to left; the reverse reads left to right.
        assert_eq!(dir("تقرير الرحلة هذا نص عربي with English"), Some(Dir::Rtl("ar")));
        assert_eq!(dir("A long English report with one word: مرحبا"), Some(Dir::Ltr));
        // Nothing to tell by.
        assert_eq!(dir("123 + 456 = ?"), None);
        assert_eq!(dir(""), None);
    }

    #[test]
    fn markdown_blocks_become_typst_markup() {
        assert_eq!(md("# Title\n\nA paragraph."), "= Title\n\nA paragraph.");
        assert_eq!(md("## Two\n### Three"), "== Two\n\n=== Three");
        assert_eq!(
            md("Some *emphasis*, **strong**, ~~gone~~ and `code`."),
            "Some #emph[emphasis], #strong[strong], #strike[gone] and #raw(\"code\")."
        );
        assert_eq!(md("---"), "#line(length: 100%, stroke: 0.6pt + luma(170))");
        assert_eq!(
            md("```rust\nfn main() {}\n```"),
            "#raw(block: true, lang: \"rust\", \"fn main() {}\")"
        );
        assert_eq!(md("```\nno language\n```"), "#raw(block: true, \"no language\")");
        assert_eq!(md("line one  \nline two"), "line one\\\nline two");
        assert_eq!(md("<!-- a comment -->\n\nText"), "Text");
        assert_eq!(md("---\ntitle: x\n---\n\nBody"), "Body");
    }

    #[test]
    fn markdown_lists_quotes_and_tables_become_typst_markup() {
        assert_eq!(
            md("- one\n- two\n  - nested"),
            "#list(tight: true,\n[one],\n[two\n\n#list(tight: true,\n[nested]\n)]\n)"
        );
        assert_eq!(
            md("3. three\n4. four"),
            "#enum(start: 3, tight: true,\n[three],\n[four]\n)"
        );
        assert_eq!(md("1. a\n2. b"), "#enum(tight: true,\n[a],\n[b]\n)");
        assert_eq!(md("- a\n\n- b"), "#list(tight: false,\n[a],\n[b]\n)");
        assert_eq!(
            md("- [x] done\n- [ ] todo"),
            "#list(tight: true,\n[☑ done],\n[☐ todo]\n)"
        );
        assert_eq!(
            md("> quoted *text*"),
            "#block(inset: (left: 1em, y: 0.2em), stroke: (left: 2.5pt + luma(170)), width: 100%)[\nquoted #emph[text]\n]"
        );
        assert_eq!(
            md("| a | b |\n|:--|--:|\n| 1 | 2 |"),
            "#table(\n  columns: 2,\n  align: (left, right),\n  stroke: 0.5pt + luma(190),\n  inset: 6pt,\n  table.header([a], [b]),\n  [1], [2],\n)"
        );
    }

    #[test]
    fn markdown_links_pictures_and_footnotes_become_typst_markup() {
        assert_eq!(
            md("[the site](https://example.com/a?b=1&c=\"2\")"),
            "#link(\"https://example.com/a?b=1&c=\\\"2\\\"\")[the site]"
        );
        assert_eq!(md("[mail](mailto:a@b.co)"), "#link(\"mailto:a@b.co\")[mail]");
        // A script, a file link or an anchor goes nowhere: the text stays.
        assert_eq!(md("[x](javascript:alert(1))"), "x");
        assert_eq!(md("[x](#section)"), "x");
        // A reference resolves to its definition.
        assert_eq!(
            md("[docs][d]\n\n[d]: https://docs.rs"),
            "#link(\"https://docs.rs\")[docs]"
        );
        // Brackets with nothing defined for them are only brackets.
        assert_eq!(md("[nothing][missing]"), r"\[nothing\]\[missing\]");
        // A literal address in the text is a link too.
        assert_eq!(
            md("see https://example.com now"),
            "see #link(\"https://example.com\")[https:\\//example.com] now"
        );
        // Pictures online aren't fetched; a local one is drawn, with its description.
        assert_eq!(md("![a cat](https://x.org/cat.png)"), "#emph[a cat]");
        assert_eq!(
            md("![a cat](img/my%20cat.png)"),
            "#image(\"img/my cat.png\", alt: \"a cat\")"
        );
        assert_eq!(md("![](pic.jpg)"), "#image(\"pic.jpg\")");
        assert_eq!(md("![odd](pic.bmp)"), "#emph[odd]");
        assert_eq!(md("![abs](C:\\x\\pic.png)"), "#emph[abs]");
        assert_eq!(md("Note[^1].\n\n[^1]: The note."), "Note#footnote[The note.].");
        assert_eq!(md("<b>bold</b> and<br>break"), "bold and\\\nbreak");
    }

    #[test]
    fn a_picture_that_is_not_there_is_replaced_by_its_description() {
        let dir = Scratch::new("pictures");
        dir.file("here.png", "not really a png");
        let options = Options {
            title: None,
            base_dir: Some(dir.0.clone()),
        };
        let out = from_markdown::convert("![here](here.png) ![gone](gone.png)", &options).body;
        assert_eq!(out, "#image(\"here.png\", alt: \"here\") #emph[gone]");
    }

    #[test]
    fn arabic_markdown_is_typeset_right_to_left_and_english_inside_it_left_to_right() {
        let arabic = "# تقرير الرحلة\n\nهذا نص عربي طويل بما يكفي ليكون الاتجاه من اليمين إلى اليسار.\n\n- بند أول\n- بند ثان\n\nA short English line.\n\n```rust\nlet x = 1;\n```\n";
        let out = from_markdown::convert(arabic, &Options::default());
        assert_eq!(out.dir, Dir::Rtl("ar"));
        assert!(out.body.starts_with("= تقرير الرحلة\n\nهذا نص عربي"), "{}", out.body);
        // The English paragraph is the odd one out: it says so.
        assert!(
            out.body
                .contains("#block(width: 100%)[\n#set text(dir: ltr, lang: \"en\")\nA short English line.\n]"),
            "{}",
            out.body
        );
        // The code stays as it is.
        assert!(out.body.contains("#raw(block: true, lang: \"rust\", \"let x = 1;\")"));
        let full = from_markdown::to_typst(arabic, &Options::default());
        assert!(full.contains("#set text(font: (\"Segoe UI\""), "{full}");
        assert!(full.contains("dir: rtl, lang: \"ar\")"), "{full}");
        assert!(full.contains("#set document(title: \"تقرير الرحلة\")"), "{full}");
        // And the other way: an English document with an Arabic paragraph.
        let english = from_markdown::convert(
            "An English document with enough words in it.\n\nمرحبا بكم في هذا المستند العربي\n",
            &Options::default(),
        );
        assert_eq!(english.dir, Dir::Ltr);
        assert!(
            english
                .body
                .contains("#block(width: 100%)[\n#set text(dir: rtl, lang: \"ar\")\nمرحبا بكم"),
            "{}",
            english.body
        );
    }

    /// The bands of rows that hold ink on a page, top to bottom, with the columns each spans.
    fn ink_bands(page: &Rendered) -> Vec<(u32, u32, u32, u32)> {
        let mut bands: Vec<(u32, u32, u32, u32)> = Vec::new();
        for y in 0..page.height {
            let row = &page.rgba[(y * page.width * 4) as usize..((y + 1) * page.width * 4) as usize];
            let inked: Vec<u32> = row
                .as_chunks::<4>()
                .0
                .iter()
                .enumerate()
                .filter(|(_, p)| p[0] < 200 || p[1] < 200 || p[2] < 200)
                .map(|(x, _)| x as u32)
                .collect();
            let (Some(&first), Some(&last)) = (inked.first(), inked.last()) else {
                continue;
            };
            match bands.last_mut() {
                // Rows within a few pixels of each other are one line of text.
                Some(band) if y <= band.1 + 3 => {
                    band.1 = y;
                    band.2 = band.2.min(first);
                    band.3 = band.3.max(last);
                }
                _ => bands.push((y, y, first, last)),
            }
        }
        bands
    }

    #[test]
    fn an_arabic_markdown_document_reads_from_the_right_and_its_english_line_from_the_left() {
        let dir = Scratch::new("md-rtl");
        let source = "هذا نص عربي قصير يكفي ليكون اتجاه الصفحة من اليمين إلى اليسار.

A short English line.
";
        let typst = from_markdown::to_typst(source, &Options::default());
        let path = dir.file("rtl.typ", &typst);
        let page = on_big(move || {
            let compiled = compile(Input::File(path), &Limits::unlimited()).unwrap_or_else(|f| {
                panic!(
                    "{f}
{}",
                    f.diagnostics.iter().map(ToString::to_string).collect::<Vec<_>>().join(
                        "
"
                    )
                )
            });
            compiled.render(0, 1.0)
        })
        .unwrap_or_else(|| panic!("drawn"));
        let bands = ink_bands(&page);
        // The Arabic line, the English line and the page number under them.
        assert!(bands.len() >= 3, "{bands:?}");
        let arabic = bands[0];
        let english = bands[bands.len() - 2];
        assert!(
            arabic.3 > page.width * 4 / 5,
            "the Arabic line ends at the right margin: {arabic:?}"
        );
        assert!(
            english.2 < page.width / 5,
            "the English line starts at the left margin: {english:?}"
        );
        assert!(english.3 < page.width / 2, "and is short: {english:?}");
    }

    /// The page of the Markdown `source` typeset, and the bands of ink on it.
    fn markdown_page(tag: &str, source: &str) -> (Rendered, Vec<(u32, u32, u32, u32)>) {
        let dir = Scratch::new(tag);
        let typst = from_markdown::to_typst(source, &Options::default());
        let path = dir.file("doc.typ", &typst);
        let page = on_big(move || {
            let compiled = compile(Input::File(path), &Limits::unlimited()).unwrap_or_else(|f| {
                panic!(
                    "{f}
{}",
                    f.diagnostics.iter().map(ToString::to_string).collect::<Vec<_>>().join(
                        "
"
                    )
                )
            });
            compiled.render(0, 1.0)
        })
        .unwrap_or_else(|| panic!("drawn"));
        let bands = ink_bands(&page);
        (page, bands)
    }

    #[test]
    fn an_arabic_list_has_its_bullets_on_the_right_and_an_arabic_paragraph_in_english_text_reads_from_the_right() {
        let (page, bands) = markdown_page(
            "md-list",
            "- بند أول
- بند ثان

هذا نص عربي كافٍ لتقرير الاتجاه من اليمين إلى اليسار في المستند.
",
        );
        // Bullets, then the text to their left: the list's lines reach the right margin.
        assert!(bands[0].3 > page.width * 4 / 5, "{bands:?}");
        assert!(bands[0].2 > page.width / 2, "{bands:?}");
        // An English document with one Arabic line: the Arabic line ends at the right margin, the English one starts at the left.
        let (page, bands) = markdown_page(
            "md-english",
            "An English paragraph, long enough to settle which way this document reads.

مرحبا بالعالم
",
        );
        assert!(bands.len() >= 3, "{bands:?}");
        assert!(bands[0].2 < page.width / 5, "English starts at the left: {bands:?}");
        let arabic = bands[bands.len() - 2];
        assert!(
            arabic.3 > page.width * 4 / 5,
            "the Arabic line ends at the right margin: {arabic:?}"
        );
        assert!(arabic.2 > page.width / 2, "and is short: {arabic:?}");
    }

    #[test]
    fn a_title_comes_from_the_options_or_the_first_heading() {
        let converted = from_markdown::convert("# Read me\n\ntext", &Options::default());
        assert_eq!(converted.title.as_deref(), Some("Read me"));
        let options = Options {
            title: Some("Given".into()),
            base_dir: None,
        };
        assert_eq!(
            from_markdown::convert("# Read me", &options).title.as_deref(),
            Some("Given")
        );
        assert!(from_markdown::to_typst("# \"Quoted\" title", &Options::default())
            .contains("#set document(title: \"\\\"Quoted\\\" title\")"));
    }

    /// Every construct, tricky characters included, typeset by the real compiler: the mapping must never make
    /// markup Typst rejects.
    const EVERYTHING: &str = "---\ntitle: front matter is dropped\n---\n\n# Everything *at* `once` #1\n\n\
Text with # hash, $ dollars, * stars, _ unders_, `ticks`, @at, <angle>, [brackets], ~tilde, back\\\\slash, a -- dash, \
wait... and http://plain.example/*x* // not a comment.\n\n\
= not a heading\n\n- dash line\n\n+ plus line\n\n/ term: not a term\n\n1. enum line\n\n\
Hard  \nbreak, **bold *nested emphasis* here**, ~~struck~~, <b>html</b> and <br> a break.\n\n\
> A quote\n> with **two** lines\n>\n> > nested\n\n\
1. first\n2. second\n   - nested bullet\n   - another\n3. third\n\n\
- [x] done\n- [ ] not done\n\n\
| Name | Qty | Note |\n|:-----|:---:|-----:|\n| apple | 3 | `a|b` |\n| pear | 10 | [link](https://example.com) |\n| só | ü | 日本語 |\n\n\
```python\nprint(\"hi # $ \\\\ \")\n```\n\n```\nplain \"code\" with `ticks` and \\\\ slashes\n```\n\n\
Footnote here[^n] and a [reference][r].\n\n[^n]: The footnote, with *emphasis*.\n\n[r]: https://example.com/ref\n\n---\n\n\
<div>block html</div>\n\n\
![nowhere](https://example.com/x.png) and ![missing](missing.png)\n\n\
مرحبا بالعالم: هذا نص عربي مع # و $ و * و _ و `code`.\n\n| عمود | قيمة |\n|---|---|\n| واحد | 1 |\n";

    #[test]
    fn a_markdown_document_with_every_construct_typesets() {
        let dir = Scratch::new("everything");
        let options = Options {
            title: None,
            base_dir: Some(dir.0.clone()),
        };
        let typst = from_markdown::to_typst(EVERYTHING, &options);
        let path = dir.file("everything.typ", &typst);
        let result = on_big(move || {
            let compiled = compile(Input::File(path), &Limits::unlimited())?;
            let pdf = compiled.pdf()?;
            Ok::<_, Failure>((compiled.pages().len(), pdf))
        });
        let (pages, pdf) = result.unwrap_or_else(|f| {
            panic!(
                "{f}\n{}",
                f.diagnostics
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        });
        assert!(pages >= 1);
        assert!(pdf.starts_with(b"%PDF-"));
    }

    #[test]
    fn markdown_goes_to_pdf_from_memory_with_pictures_beside_it() {
        let dir = Scratch::new("memory");
        // A real 2 x 2 PNG, written by the image crate.
        let mut png = Vec::new();
        let picture = image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 30, 30, 255]));
        picture
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap_or_else(|e| panic!("png: {e}"));
        fs::write(dir.0.join("red.png"), &png).unwrap_or_else(|e| panic!("{e}"));
        let options = Options {
            title: None,
            base_dir: Some(dir.0.clone()),
        };
        let typst = from_markdown::to_typst("# Picture\n\n![red square](red.png)\n\nAfter.\n", &options);
        assert!(typst.contains("#image(\"red.png\", alt: \"red square\")"));
        let root = dir.0.clone();
        let result = on_big(move || {
            pdf_of(
                Input::Text {
                    dir: root,
                    name: "document.typ".into(),
                    text: typst,
                },
                &Limits::unlimited(),
            )
        });
        let pdf = result.unwrap_or_else(|f| panic!("{f}"));
        assert!(pdf.starts_with(b"%PDF-"));
    }
}
