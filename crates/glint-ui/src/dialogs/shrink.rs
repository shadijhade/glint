//! Making files smaller: "Make smaller…" (videos, photos, PNGs, BMPs, PDFs, Office files) and "Back to JPEG".
//! Ported from src/components/explorer/ShrinkDialog.tsx, built the way [`super::drive_health`] is.
//!
//! | TypeScript                          | Here                                                              |
//! |-------------------------------------|-------------------------------------------------------------------|
//! | `KINDS`, `canShrink`, `isJxl`       | [`kind_of`], [`can_shrink`], [`is_jxl`] (by name; the menus know the rest) |
//! | `openShrink(entries)`               | [`open`]                                                          |
//! | `backToJpeg(entries)`               | [`back_to_jpeg`]                                                  |
//! | `ShrinkDialog` / `ShrinkModal`      | [`ShrinkDialog`]                                                  |
//! | `shrink_run`, `shrink_cancel`, `shrink_engines`, `jxl_to_jpeg` | [`Backend`] ([`CoreBackend`] is glint-core) |
//! | `useActionRun`, `WhereChooser`, `Choices`, `RunProgress`, `RunResult`, `RunFooter` | [`action_run`]    |
//! | `ToolNeeded` for ffmpeg, ab-av1, rimage, libjxl, ghostscript, pdfcpu | `settings::tools::needed`, through [`action_run::Tooling`] |
//!
//! **How the coordinator wires it.**
//! * `MenuEvent::MakeSmaller { paths }` is `dialogs::shrink::open(window, cx, paths, compress)`. The files among
//!   `paths` are the ones Make smaller works on; folders among them (it asks the active folder's listing, as the
//!   menus got them from there) are offered Compress instead. `compress` runs for "Compress to 7z" with the
//!   folders (`compressEntries(folders, "7z")`): `PORT ME(coordinator)`: the same as the Compress > 7z file
//!   menu item. "In place" opens `dialogs::compress_in_place` itself.
//! * `MenuEvent::BackToJpeg { paths }` is `dialogs::shrink::back_to_jpeg(window, cx, paths)`. It makes a JPEG
//!   beside each `.jxl` file, with no dialog, and Ctrl+Z takes them back.
//!
//! Enter in the dialog doesn't start the run (the TypeScript's modal submitted on Enter); the buttons do.

use super::action_run;

use crate::kit::prelude::*;
use crate::{core_bridge, explorer, t, theme};
use action_run::{
    choices, error_view, footer, help, on_core, progress_text, progress_view, result_view, section_title, shell, whole,
    ActionRun, Choice, FooterSpec, Fut, ResultWords, Run, Tooling, WhereChooser,
};
use glint_core::actions::{Output, Progress, Report};
use glint_core::shrink::{Engines, Kind, PdfWay, PhotoWay, ShrinkChoices, VideoCodec};
use glint_model::format::{format_count, plural};
use glint_model::paths::base_name;
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

/// What Make smaller does with the file called `path`, by its extension; `None` for kinds it doesn't take
/// (`kindOf`).
pub fn kind_of(path: &str) -> Option<Kind> {
    glint_core::shrink::kind_of(Path::new(path))
}

/// Whether Make smaller works on the file called `path` (`canShrink` minus the entry's state: a file on disk, there
/// to be read, is for the menus to know).
pub fn can_shrink(path: &str) -> bool {
    kind_of(path).is_some()
}

/// A JPEG XL file, for Back to JPEG (`isJxl`).
pub fn is_jxl(path: &str) -> bool {
    path.to_lowercase().ends_with(".jxl")
}

/// How many of each kind of file there are (`counts`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub video: usize,
    pub jpeg: usize,
    pub bmp: usize,
    pub png: usize,
    pub pdf: usize,
    pub office: usize,
}

/// Counts `files` by kind; files of no kind are left out.
pub fn counts_of(files: &[String]) -> Counts {
    let mut counts = Counts::default();
    for file in files {
        match kind_of(file) {
            Some(Kind::Video) => counts.video += 1,
            Some(Kind::Jpeg) => counts.jpeg += 1,
            Some(Kind::Bmp) => counts.bmp += 1,
            Some(Kind::Png) => counts.png += 1,
            Some(Kind::Pdf) => counts.pdf += 1,
            Some(Kind::Office) => counts.office += 1,
            None => {}
        }
    }
    counts
}

// ---- The calls --------------------------------------------------------------------------------------------

/// The calls Make smaller makes. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// Makes `files` smaller as `choices` say (`shrink_run`), reporting progress on `progress`.
    fn run(
        &self,
        files: Vec<String>,
        choices: ShrinkChoices,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report>;
    /// What this PC's FFmpeg can do (`shrink_engines`).
    fn engines(&self) -> Fut<Engines>;
    /// JPEG XL files as JPEGs (`jxl_to_jpeg`).
    fn jxl_to_jpeg(
        &self,
        files: Vec<String>,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report>;
    /// Stops the run under way (`shrink_cancel`).
    fn cancel(&self);
}

/// glint-core's Make smaller, owned by the main window as the shell dialogs it opens are.
pub struct CoreBackend {
    app: glint_core::host::AppHandle,
}

impl CoreBackend {
    /// The backend over core's handle (from `core_bridge::Core`).
    pub fn new(app: glint_core::host::AppHandle) -> Self {
        Self { app }
    }
}

impl Backend for CoreBackend {
    fn run(
        &self,
        files: Vec<String>,
        choices: ShrinkChoices,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report> {
        match action_run::main_window(&self.app) {
            Ok(window) => on_core(glint_core::shrink::shrink_run(window, files, choices, output, progress)),
            Err(reason) => Box::pin(async move { Err(reason) }),
        }
    }

    fn engines(&self) -> Fut<Engines> {
        on_core(glint_core::shrink::shrink_engines())
    }

    fn jxl_to_jpeg(
        &self,
        files: Vec<String>,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report> {
        match action_run::main_window(&self.app) {
            Ok(window) => on_core(glint_core::shrink::jxl_to_jpeg(window, files, output, progress)),
            Err(reason) => Box::pin(async move { Err(reason) }),
        }
    }

    fn cancel(&self) {
        glint_core::shrink::shrink_cancel();
    }
}

/// Stands in when core isn't started (never in the app).
struct Unavailable;

impl Backend for Unavailable {
    fn run(
        &self,
        _: Vec<String>,
        _: ShrinkChoices,
        _: Output,
        _: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report> {
        action_run::no_core()
    }

    fn engines(&self) -> Fut<Engines> {
        action_run::no_core()
    }

    fn jxl_to_jpeg(&self, _: Vec<String>, _: Output, _: glint_core::host::ipc::Channel<Progress>) -> Fut<Report> {
        action_run::no_core()
    }

    fn cancel(&self) {}
}

fn core_backend(cx: &App) -> Arc<dyn Backend> {
    match cx.try_global::<core_bridge::Core>() {
        Some(core) => Arc::new(CoreBackend::new(core.app.clone())),
        None => Arc::new(Unavailable),
    }
}

// ---- Words ------------------------------------------------------------------------------------------------

/// Why a tool is needed (`WHY`).
fn why(id: &str) -> SharedString {
    match id {
        "ffmpeg" => t!("FFmpeg encodes the videos, on this PC."),
        "ab-av1" => t!("ab-av1 finds the smallest size at which a video still looks the same, from short samples."),
        "rimage" => t!("rimage makes photos and pictures smaller, on this PC."),
        "libjxl" => t!("libjxl repacks JPEGs as JPEG XL without losing a bit."),
        "ghostscript" => t!("Ghostscript rewrites a PDF's pictures for screens or print, on this PC."),
        "pdfcpu" => t!("pdfcpu makes PDFs smaller without changing what's on their pages."),
        _ => SharedString::default(),
    }
}

/// The card's icon: a film for video, a page for PDFs, a picture for the rest.
fn tool_icon(id: &str) -> IconName {
    match id {
        "ffmpeg" | "ab-av1" => IconName::Film,
        "ghostscript" | "pdfcpu" => IconName::FileText,
        _ => IconName::FileImage,
    }
}

/// The dialog's title after "Make smaller: " (`title`): the one name, or what there is of each kind.
pub fn title_of(files: &[String], folders: &[String]) -> SharedString {
    if let (1, Some(only)) = (files.len() + folders.len(), files.first().or(folders.first())) {
        return isolate(base_name(only).to_string());
    }
    let counts = counts_of(files);
    let parts: Vec<String> = [
        (counts.video > 0).then(|| plural(counts.video, "video")),
        (counts.jpeg > 0).then(|| plural(counts.jpeg, "photo")),
        (counts.png + counts.bmp > 0).then(|| plural(counts.png + counts.bmp, "picture")),
        (counts.pdf > 0).then(|| plural(counts.pdf, "PDF")),
        (counts.office > 0).then(|| plural(counts.office, "Office file")),
        (!folders.is_empty()).then(|| plural(folders.len(), "folder")),
    ]
    .into_iter()
    .flatten()
    .collect();
    parts.join(", ").into()
}

// ---- The dialog -------------------------------------------------------------------------------------------

/// Hands folders to Compress to 7z (`compressEntries(folders, "7z")`).
pub type CompressHook = Rc<dyn Fn(&mut Window, &mut App, Vec<String>)>;

/// Making some files smaller: the way for each kind, where the results go, the run and its result.
pub struct ShrinkDialog {
    files: Vec<String>,
    folders: Vec<String>,
    counts: Counts,
    backend: Arc<dyn Backend>,
    tooling: Tooling,
    compress: CompressHook,
    video: VideoCodec,
    photo: PhotoWay,
    pdf: PdfWay,
    quality: u8,
    engines: Option<Engines>,
    engines_asked: bool,
    where_: Entity<WhereChooser>,
    run: Entity<ActionRun>,
    _subscriptions: Vec<Subscription>,
}

impl ShrinkDialog {
    /// A dialog for `files` and `folders`, calling `backend`, its install cards reading `tooling`.
    pub fn new(
        files: Vec<String>,
        folders: Vec<String>,
        backend: Arc<dyn Backend>,
        tooling: Tooling,
        compress: CompressHook,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let where_ = cx.new(|cx| {
            WhereChooser::new(
                "shrink-where",
                t!("file"),
                t!("In their place, the originals go to the Recycle Bin, and Ctrl+Z takes it all back."),
                true,
                (" (smaller)", "Converted"),
                window,
                cx,
            )
        });
        let run = cx.new(|_| ActionRun::new());
        let mut subscriptions = vec![
            cx.observe(&where_, |_, _, cx| cx.notify()),
            cx.observe(&run, |_, _, cx| cx.notify()),
        ];
        if let Some(tools) = tooling.model() {
            // The engines are asked for once FFmpeg is there, whenever that becomes true.
            subscriptions.push(cx.observe(tools, |this, _, cx| {
                this.load_engines(cx);
                cx.notify();
            }));
        }
        let counts = counts_of(&files);
        let mut this = Self {
            files,
            folders,
            counts,
            backend,
            tooling,
            compress,
            video: VideoCodec::H264,
            photo: PhotoWay::Jpeg,
            pdf: PdfWay::Screen,
            quality: 80,
            engines: None,
            engines_asked: false,
            where_,
            run,
            _subscriptions: subscriptions,
        };
        this.load_engines(cx);
        this
    }

    /// Asks which encoders FFmpeg has, once, when there are videos and FFmpeg is there
    /// (`useEffect([ffmpegReady])`). A failed question counts as an FFmpeg with none.
    fn load_engines(&mut self, cx: &mut Context<Self>) {
        if self.counts.video == 0 || self.engines_asked || !self.tooling.ready("ffmpeg", cx) {
            return;
        }
        self.engines_asked = true;
        let asked = self.backend.engines();
        cx.spawn(async move |this, cx| {
            let found = asked.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                // The one the person gets if H.264 isn't there: HEVC, else AV1.
                if !found.h264 {
                    this.video = if found.hevc { VideoCodec::Hevc } else { VideoCodec::Av1 };
                }
                this.engines = Some(found);
                cx.notify();
            });
        })
        .detach();
    }

    /// The files being made smaller.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// The folders offered Compress instead.
    pub fn folders(&self) -> &[String] {
        &self.folders
    }

    /// The video format chosen.
    pub fn video(&self) -> VideoCodec {
        self.video
    }

    /// What this PC's FFmpeg can do, once known.
    pub fn engines(&self) -> Option<&Engines> {
        self.engines.as_ref()
    }

    /// Where the run stands.
    pub fn run_state(&self, cx: &App) -> Run {
        self.run.read(cx).state().clone()
    }

    /// The run, to put in a state by hand in tests.
    pub fn run(&self) -> &Entity<ActionRun> {
        &self.run
    }

    /// Where the results go.
    pub fn where_chooser(&self) -> &Entity<WhereChooser> {
        &self.where_
    }

    /// The tools this selection needs, as chosen: (tool, whether it is there).
    pub fn needs(&self, cx: &App) -> Vec<(&'static str, bool)> {
        let counts = self.counts;
        let mut needs: Vec<&'static str> = Vec::new();
        if counts.video > 0 {
            needs.extend(["ffmpeg", "ab-av1"]);
        }
        if (counts.jpeg > 0 && self.photo == PhotoWay::Jpeg) || counts.png > 0 || counts.bmp > 0 || counts.office > 0 {
            needs.push("rimage");
        }
        if counts.jpeg > 0 && self.photo == PhotoWay::Jxl {
            needs.push("libjxl");
        }
        if counts.pdf > 0 {
            needs.push(if self.pdf == PdfWay::Lossless {
                "pdfcpu"
            } else {
                "ghostscript"
            });
        }
        needs.into_iter().map(|id| (id, self.tooling.ready(id, cx))).collect()
    }

    /// Every tool is there, and the video encoders are known when there are videos (`ready`).
    fn ready(&self, cx: &App) -> bool {
        self.needs(cx).iter().all(|(_, ready)| *ready) && (self.counts.video == 0 || self.engines.is_some())
    }

    /// The chosen video format can be made on this PC (`videoOk`).
    fn video_ok(&self) -> bool {
        self.counts.video == 0
            || self.engines.as_ref().is_none_or(|e| match self.video {
                VideoCodec::H264 => e.h264,
                VideoCodec::Hevc => e.hevc,
                VideoCodec::Av1 => e.av1,
            })
    }

    /// Whether Make smaller can be pressed: something to do, the tools there, the format possible, the names usable.
    pub fn can_start(&self, cx: &App) -> bool {
        !self.run.read(cx).state().started()
            && !self.files.is_empty()
            && self.ready(cx)
            && self.video_ok()
            && !(self.where_.read(cx).choice(cx).bad())
    }

    /// Starts the run on core's runtime; progress and the result come back here.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if !self.can_start(cx) {
            return;
        }
        let output = self.where_.read(cx).choice(cx).output();
        let choices = ShrinkChoices {
            video: self.video,
            photo: self.photo,
            pdf: self.pdf,
            quality: self.quality,
        };
        let (channel, receiver) = core_bridge::stream::<Progress>();
        let call = self.backend.run(self.files.clone(), choices, output, channel);
        let total = self.files.len();
        self.run.update(cx, |run, cx| {
            run.start(
                total,
                call,
                receiver,
                |report| t!("making {what} smaller", what = plural(report.made.len(), "file")).to_string(),
                cx,
            )
        });
        cx.notify();
    }

    fn stop(&mut self) {
        self.backend.cancel();
    }

    /// The folders box: compress them in place, or into one smaller file.
    fn folders_box(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let note = if self.folders.len() == 1 {
            t!(
                "“{name}” is a folder: compress it in place (it still opens normally), or into one smaller file.",
                name = isolate(base_name(&self.folders[0]).to_string())
            )
        } else {
            t!(
                "{n} folders: compress them in place (they still open normally), or into smaller files.",
                n = format_count(self.folders.len())
            )
        };
        let only_folders = self.files.is_empty();
        align_row(3.)
            .id("shrink-folders")
            .role(Role::Group)
            .aria_label(note.clone())
            .test_support()
            .rounded(theme::size::RADIUS)
            .bg(t.muted.opacity(0.4))
            .px(space(3.))
            .py(space(2.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(note),
            )
            .child(
                Button::new("shrink-in-place")
                    .outline()
                    .small()
                    .icon(IconName::Shrink)
                    .label(t!("In place"))
                    .tooltip(t!("Windows stores them compressed; they open as before"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let folders = this.folders.clone();
                        // The dialog layer closes the top dialog: close this one first, then open the next.
                        if only_folders {
                            window.close_dialog(cx);
                        }
                        super::compress_in_place::open(window, cx, folders);
                    })),
            )
            .child(
                Button::new("shrink-compress")
                    .outline()
                    .small()
                    .icon(IconName::FileArchive)
                    .label(t!("Compress to 7z"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let folders = this.folders.clone();
                        let compress = this.compress.clone();
                        if only_folders {
                            window.close_dialog(cx);
                        }
                        compress(window, cx, folders);
                    })),
            )
    }
}

impl Render for ShrinkDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let state = self.run.read(cx).state().clone();
        let counts = self.counts;

        let body = match state.clone() {
            Run::Setup => {
                let mut parts: Vec<AnyElement> = Vec::new();
                parts.push(
                    help(
                        t!("Each file is made smaller by the best tool for its kind. Nothing that would come out bigger is kept, and every copy keeps its original's date."),
                        &t,
                    )
                    .into_any_element(),
                );
                for (id, _) in self.needs(cx) {
                    parts.extend(self.tooling.card(id, why(id), tool_icon(id), cx));
                }
                if counts.video > 0 {
                    let engines = self.engines.clone();
                    let off = |has: fn(&Engines) -> bool| engines.as_ref().is_some_and(|e| !has(e));
                    let scored_by = if engines.as_ref().is_some_and(|e| !e.vmaf) {
                        "XPSNR"
                    } else {
                        "VMAF"
                    };
                    parts.push(
                        col()
                            .gap(space(1.5))
                            .child(section_title(
                                t!("{what}: looks the same, smaller", what = plural(counts.video, "video")),
                                &t,
                            ))
                            .child(choices::<Self, VideoCodec>(
                                "shrink-video",
                                t!("Video format"),
                                self.video,
                                vec![
                                    Choice::new(VideoCodec::H264, "H.264")
                                        .hint(t!("Plays everywhere"))
                                        .disabled(off(|e| e.h264)),
                                    Choice::new(VideoCodec::Hevc, "HEVC")
                                        .hint(t!("Smaller; phones, newer PCs"))
                                        .disabled(off(|e| e.hevc)),
                                    Choice::new(VideoCodec::Av1, "AV1")
                                        .hint(if off(|e| e.av1) {
                                            t!("Needs FFmpeg's full build")
                                        } else {
                                            t!("Smallest; newest devices")
                                        })
                                        .disabled(off(|e| e.av1)),
                                ],
                                3,
                                |this, picked, _, _| this.video = picked,
                                &t,
                                cx,
                            ))
                            .child(help(
                                t!(
                                    "Short samples are encoded and scored by {metric} first, to find the smallest size that still looks the same, then the whole video is encoded: it takes a while. An HDR video keeps its colours as HEVC or AV1.",
                                    metric = scored_by
                                ),
                                &t,
                            ))
                            .into_any_element(),
                    );
                }
                if counts.jpeg > 0 {
                    parts.push(
                        col()
                            .gap(space(1.5))
                            .child(section_title(SharedString::from(plural(counts.jpeg, "photo")), &t))
                            .child(choices::<Self, PhotoWay>(
                                "shrink-photo",
                                t!("Photos"),
                                self.photo,
                                vec![
                                    Choice::new(PhotoWay::Jpeg, t!("A smaller JPEG")).hint(t!("Opens everywhere")),
                                    Choice::new(PhotoWay::Jxl, t!("JPEG XL, losslessly"))
                                        .hint(t!("About a fifth smaller, every bit kept")),
                                ],
                                2,
                                |this, picked, _, _| this.photo = picked,
                                &t,
                                cx,
                            ))
                            .when(self.photo == PhotoWay::Jxl, |this| {
                                this.child(help(
                                    t!("Windows 11 and Chrome open JPEG XL. Back to JPEG, on a .jxl file's menu, gives back the very same JPEG."),
                                    &t,
                                ))
                            })
                            .into_any_element(),
                    );
                }
                if (counts.jpeg > 0 && self.photo == PhotoWay::Jpeg) || counts.office > 0 {
                    let heading = if counts.office > 0 {
                        t!("JPEG quality (pictures in Office files too)")
                    } else {
                        t!("JPEG quality")
                    };
                    parts.push(
                        col()
                            .gap(space(1.5))
                            .child(section_title(heading, &t))
                            .child(choices::<Self, u8>(
                                "shrink-quality",
                                t!("JPEG quality"),
                                self.quality,
                                vec![
                                    Choice::new(70, t!("Smallest")).hint(t!("Quality {n}", n = 70)),
                                    Choice::new(80, t!("Balanced")).hint(t!("Quality {n}", n = 80)),
                                    Choice::new(90, t!("Best")).hint(t!("Quality {n}", n = 90)),
                                ],
                                3,
                                |this, picked, _, _| this.quality = picked,
                                &t,
                                cx,
                            ))
                            .into_any_element(),
                    );
                }
                if counts.pdf > 0 {
                    parts.push(
                        col()
                            .gap(space(1.5))
                            .child(section_title(SharedString::from(plural(counts.pdf, "PDF")), &t))
                            .child(choices::<Self, PdfWay>(
                                "shrink-pdf",
                                t!("PDFs"),
                                self.pdf,
                                vec![
                                    Choice::new(PdfWay::Screen, t!("For screens and mail"))
                                        .hint(t!("Pictures at {n} dpi", n = 150)),
                                    Choice::new(PdfWay::Print, t!("For print"))
                                        .hint(t!("Pictures at {n} dpi", n = 300)),
                                    Choice::new(PdfWay::Lossless, t!("Lossless"))
                                        .hint(t!("Nothing changes on the page")),
                                ],
                                3,
                                |this, picked, _, _| this.pdf = picked,
                                &t,
                                cx,
                            ))
                            .when(self.pdf != PdfWay::Lossless, |this| {
                                this.child(help(
                                    t!("Ghostscript rewrites the PDF: its text and links stay, a digital signature wouldn't."),
                                    &t,
                                ))
                            })
                            .into_any_element(),
                    );
                }
                if counts.png > 0 || counts.bmp > 0 {
                    parts.push(
                        help(
                            t!("PNGs are made smaller losslessly; BMPs become PNGs, the same picture at a fraction of the size."),
                            &t,
                        )
                        .into_any_element(),
                    );
                }
                if counts.office > 0 {
                    parts.push(
                        help(
                            t!("Word, PowerPoint and Excel files get their pictures made smaller (at most 3,000 pixels across); the rest of each file stays as it was."),
                            &t,
                        )
                        .into_any_element(),
                    );
                }
                if !self.folders.is_empty() {
                    parts.push(self.folders_box(&t, cx).into_any_element());
                }
                if !self.files.is_empty() {
                    parts.push(self.where_.clone().into_any_element());
                }
                col().gap(space(4.)).children(parts).into_any_element()
            }
            Run::Running(progress) => progress_view(
                "shrink-progress",
                progress_text(&t!("Making smaller"), &progress, false, false),
                whole(&progress),
                &t,
            )
            .into_any_element(),
            Run::Done(report) => {
                result_view("shrink-result", &report, &ResultWords::files(true), &t).into_any_element()
            }
            Run::Failed(reason) => {
                error_view("shrink-error", t!("Couldn't make them smaller"), reason, &t).into_any_element()
            }
        };

        let spec = FooterSpec {
            prefix: "shrink",
            start_label: t!("Make smaller"),
            start_icon: IconName::Minimize2,
            can_start: self.can_start(cx),
            stoppable: true,
            show: state.first_made(),
        };
        let footer = footer::<Self>(
            spec,
            &state,
            |this, _, cx| this.start(cx),
            |this, _, _| this.stop(),
            &t,
            cx,
        );
        let everything: Vec<String> = self.files.iter().chain(self.folders.iter()).cloned().collect();
        shell(
            "shrink",
            t!("Make smaller: {what}", what = title_of(&self.files, &self.folders)),
            action_run::where_they_are(&everything),
            body,
            footer,
            window,
            &t,
        )
    }
}

/// The folders among `paths` that are not files Make smaller takes: asked of the active folder's listing, where
/// the menus found them (`entries.filter(e => e.dir && onDisk(e))`).
fn folders_among(paths: &[String], cx: &App) -> Vec<String> {
    let wanted: HashSet<&str> = paths.iter().filter(|p| !can_shrink(p)).map(String::as_str).collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let Some(explorer) = explorer::try_explorer(cx) else {
        return Vec::new();
    };
    let explorer = explorer.read(cx);
    let Some(tab) = explorer.active_tab() else {
        return Vec::new();
    };
    tab.entries()
        .iter()
        .filter(|entry| entry.dir && wanted.contains(entry.path.as_str()))
        .map(|entry| entry.path.clone())
        .collect()
}

/// Opens Make smaller on what among `paths` it works on, in `window` (`openShrink`); folders among them are
/// offered Compress instead, `compress` running for "Compress to 7z". When there is neither it says so in a
/// notification.
pub fn open(
    window: &mut Window,
    cx: &mut App,
    paths: Vec<String>,
    compress: impl Fn(&mut Window, &mut App, Vec<String>) + 'static,
) {
    let files: Vec<String> = paths.iter().filter(|p| can_shrink(p)).cloned().collect();
    let folders = folders_among(&paths, cx);
    if files.is_empty() && folders.is_empty() {
        window.push_notification(
            Notification::info(t!(
                "Make smaller works on videos, photos, PNGs, BMPs, PDFs, and Word, PowerPoint and Excel files"
            )),
            cx,
        );
        return;
    }
    let backend = core_backend(cx);
    let tooling = Tooling::new(action_run::tools_model(window, cx));
    open_with(window, cx, backend, tooling, files, folders, Rc::new(compress));
}

/// [`open`] for a given backend and tools, without sorting `paths` (tests pass fakes). Returns the dialog.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    tooling: Tooling,
    files: Vec<String>,
    folders: Vec<String>,
    compress: CompressHook,
) -> Entity<ShrinkDialog> {
    let view = cx.new(|cx| ShrinkDialog::new(files, folders, backend, tooling, compress, window, cx));
    super::show(window, cx, t!("Make smaller"), px(576.), view.clone());
    view
}

/// Back to JPEG: JPEG XL files among `paths` as JPEGs beside them, the very JPEG for ones Make smaller repacked
/// (`backToJpeg`). No dialog; Ctrl+Z takes them back.
pub fn back_to_jpeg(window: &mut Window, cx: &mut App, paths: Vec<String>) {
    let backend = core_backend(cx);
    back_to_jpeg_with(window, cx, backend, paths);
}

/// [`back_to_jpeg`] with a backend of your own (tests pass a fake).
pub fn back_to_jpeg_with(window: &mut Window, cx: &mut App, backend: Arc<dyn Backend>, paths: Vec<String>) {
    let files: Vec<String> = paths.into_iter().filter(|p| is_jxl(p)).collect();
    if files.is_empty() {
        return;
    }
    // The progress has nobody to tell: the TypeScript passed a function that did nothing.
    let (channel, _unheard) = core_bridge::stream::<Progress>();
    let call = backend.jxl_to_jpeg(files, Output::Beside { suffix: String::new() }, channel);
    window
        .spawn(cx, async move |cx| {
            let result = call.await;
            let _ = cx.update(|window, cx| match result {
                Ok(report) => {
                    if !report.made.is_empty() {
                        let what = if report.made.len() > 1 {
                            t!("JPEGs from JPEG XL")
                        } else {
                            t!("JPEG from JPEG XL")
                        };
                        action_run::remember_undo(&report, &what, cx);
                        window.push_notification(Notification::success(what).message(t!("Ctrl+Z takes it back")), cx);
                    }
                    if let Some((_, why)) = report.failed.first() {
                        window.push_notification(
                            Notification::error(why.clone())
                                .title(t!("Couldn't make {what}", what = plural(report.failed.len(), "JPEG"))),
                            cx,
                        );
                    }
                }
                Err(reason) => {
                    window.push_notification(Notification::error(reason).title(t!("Couldn't make JPEGs")), cx);
                }
            });
        })
        .detach();
}

// ---- Tests -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::sync::Mutex;
    use std::time::Duration;

    /// A backend that answers from memory and remembers what it was asked.
    struct Fake {
        runs: Mutex<Vec<(Vec<String>, ShrinkChoices, Output)>>,
        jxl: Mutex<Vec<(Vec<String>, Output)>>,
        engines: Engines,
        report: Report,
        error: Option<String>,
        cancelled: Mutex<usize>,
    }

    impl Fake {
        fn new(engines: Engines, report: Report) -> Arc<Fake> {
            Arc::new(Fake {
                runs: Mutex::new(Vec::new()),
                jxl: Mutex::new(Vec::new()),
                engines,
                report,
                error: None,
                cancelled: Mutex::new(0),
            })
        }

        fn runs(&self) -> Vec<(Vec<String>, ShrinkChoices, Output)> {
            self.runs.lock().map(|r| r.clone()).unwrap_or_default()
        }
    }

    impl Backend for Fake {
        fn run(
            &self,
            files: Vec<String>,
            choices: ShrinkChoices,
            output: Output,
            progress: glint_core::host::ipc::Channel<Progress>,
        ) -> Fut<Report> {
            if let Ok(mut runs) = self.runs.lock() {
                runs.push((files, choices, output));
            }
            let _ = progress.send(Progress {
                done: 0,
                total: 1,
                current: "a.mp4".into(),
                part: 0.25,
            });
            let report = self.report.clone();
            let error = self.error.clone();
            Box::pin(async move {
                match error {
                    Some(error) => Err(error),
                    None => Ok(report),
                }
            })
        }

        fn engines(&self) -> Fut<Engines> {
            let engines = self.engines.clone();
            Box::pin(async move { Ok(engines) })
        }

        fn jxl_to_jpeg(
            &self,
            files: Vec<String>,
            output: Output,
            _: glint_core::host::ipc::Channel<Progress>,
        ) -> Fut<Report> {
            if let Ok(mut jxl) = self.jxl.lock() {
                jxl.push((files, output));
            }
            let report = self.report.clone();
            Box::pin(async move { Ok(report) })
        }

        fn cancel(&self) {
            if let Ok(mut n) = self.cancelled.lock() {
                *n += 1;
            }
        }
    }

    fn all_engines() -> Engines {
        Engines {
            h264: true,
            hevc: true,
            av1: true,
            vmaf: true,
        }
    }

    fn report(made: &[&str], before: u64, after: u64) -> Report {
        Report {
            made: made.iter().map(|s| (*s).to_string()).collect(),
            before,
            after,
            elapsed_ms: 4200.,
            ..Default::default()
        }
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the Make smaller dialog never reached the state the test waits for");
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    fn disabled(cx: &mut VisualTestContext, id: impl Into<ElementId>) -> bool {
        let id = id.into();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.find(id).disabled() == Some(true)
        })
    }

    fn click(cx: &mut VisualTestContext, id: impl Into<ElementId>) {
        let id = id.into();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        cx.run_until_parked();
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn window(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(testing::init);
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
    }

    fn no_compress() -> CompressHook {
        Rc::new(|_, _, _| {})
    }

    fn open_view(
        cx: &mut VisualTestContext,
        files: &[&str],
        folders: &[&str],
        fake: &Arc<Fake>,
        tools: &[&'static str],
        compress: CompressHook,
    ) -> Entity<ShrinkDialog> {
        let backend: Arc<dyn Backend> = fake.clone();
        let (files, folders) = (strings(files), strings(folders));
        let tooling = Tooling::with(tools);
        let view = cx.update(|window, cx| open_with(window, cx, backend, tooling, files, folders, compress));
        cx.run_until_parked();
        view
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::Minimize2,
            IconName::Shrink,
            IconName::FileArchive,
            IconName::Film,
            IconName::FileText,
            IconName::FileImage,
            IconName::Square,
            IconName::FolderInput,
            IconName::CircleCheck,
            IconName::CircleDot,
            IconName::Circle,
            IconName::TriangleAlert,
            IconName::Download,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[test]
    fn kinds_counts_and_titles_read_as_the_typescript_did() {
        assert_eq!(kind_of("C:\\v\\clip.MP4"), Some(Kind::Video));
        assert_eq!(kind_of("C:\\v\\a.jfif"), Some(Kind::Jpeg));
        assert_eq!(kind_of("C:\\v\\a.docm"), Some(Kind::Office));
        assert_eq!(kind_of("C:\\v\\a.txt"), None);
        assert!(can_shrink("C:\\v\\a.pdf") && !can_shrink("C:\\v\\Folder"));
        assert!(is_jxl("C:\\v\\a.JXL") && !is_jxl("C:\\v\\a.jpg"));
        let files = strings(&[
            "C:\\v\\a.mp4",
            "C:\\v\\b.mkv",
            "C:\\v\\c.jpg",
            "C:\\v\\d.png",
            "C:\\v\\e.bmp",
            "C:\\v\\f.pdf",
            "C:\\v\\g.xlsx",
            "C:\\v\\h.txt",
        ]);
        let counts = counts_of(&files);
        assert_eq!(
            counts,
            Counts {
                video: 2,
                jpeg: 1,
                bmp: 1,
                png: 1,
                pdf: 1,
                office: 1
            }
        );
        let _guard = testing::lock();
        assert_eq!(title_of(&strings(&["C:\\v\\a.mp4"]), &[]), "a.mp4");
        assert_eq!(title_of(&[], &strings(&["C:\\v\\Photos"])), "Photos");
        assert_eq!(
            title_of(&files[..7], &strings(&["C:\\v\\Photos"])),
            "2 videos, 1 photo, 2 pictures, 1 PDF, 1 Office file, 1 folder"
        );
    }

    #[gpui_kit::test]
    fn the_run_needs_its_tools_and_the_encoders_and_reports_the_sizes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(all_engines(), report(&["C:\\v\\a (smaller).mp4"], 1_000_000, 400_000));
        // Only FFmpeg: ab-av1 is missing, so nothing starts.
        let view = open_view(&mut visual, &["C:\\v\\a.mp4"], &[], &fake, &["ffmpeg"], no_compress());
        settle(&mut visual, |cx| view.read_with(cx, |v, _| v.engines().is_some()));
        assert_eq!(role_of(&mut visual, "Make smaller: a.mp4"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "C:\\v"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Video format"), Some(Role::RadioGroup));
        assert!(disabled(&mut visual, "shrink-start"));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();

        let view = open_view(
            &mut visual,
            &["C:\\v\\a.mp4"],
            &[],
            &fake,
            &["ffmpeg", "ab-av1"],
            no_compress(),
        );
        settle(&mut visual, |cx| view.read_with(cx, |v, _| v.engines().is_some()));
        assert!(!disabled(&mut visual, "shrink-start"));
        assert_eq!(role_of(&mut visual, "H.264. Plays everywhere"), Some(Role::RadioButton));
        click(&mut visual, ("shrink-video", 1_usize));
        assert_eq!(view.read_with(&visual, |v, _| v.video()), VideoCodec::Hevc);
        click(&mut visual, "shrink-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        let runs = fake.runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, ["C:\\v\\a.mp4"]);
        assert_eq!(runs[0].1.video, VideoCodec::Hevc);
        assert_eq!(runs[0].1.quality, 80);
        assert_eq!(
            runs[0].2,
            Output::Beside {
                suffix: " (smaller)".into()
            }
        );
        // The sizes before and after, and by how much.
        visual.update(|window, _| {
            assert!(window
                .find_all_by_role(Role::Status)
                .iter()
                .any(|e| e.label().is_some_and(|l| l.contains("(\u{2212}60%)"))));
        });
        click(&mut visual, "shrink-close");
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn an_encoder_the_pc_lacks_cannot_be_picked_and_the_first_one_it_has_is(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let engines = Engines {
            h264: false,
            hevc: true,
            av1: false,
            vmaf: false,
        };
        let fake = Fake::new(engines, report(&[], 0, 0));
        let view = open_view(
            &mut visual,
            &["C:\\v\\a.mp4"],
            &[],
            &fake,
            &["ffmpeg", "ab-av1"],
            no_compress(),
        );
        settle(&mut visual, |cx| view.read_with(cx, |v, _| v.engines().is_some()));
        // H.264 isn't there, so HEVC is the one chosen; AV1 says why it can't be.
        assert_eq!(view.read_with(&visual, |v, _| v.video()), VideoCodec::Hevc);
        assert!(disabled(&mut visual, ("shrink-video", 0_usize)));
        assert!(disabled(&mut visual, ("shrink-video", 2_usize)));
        assert!(has(&mut visual, "AV1. Needs FFmpeg's full build"));
        assert!(!disabled(&mut visual, "shrink-start"));
    }

    #[gpui_kit::test]
    fn each_kind_asks_for_its_own_tool_and_choices(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(all_engines(), report(&["x"], 10, 5));
        let view = open_view(
            &mut visual,
            &["C:\\v\\a.jpg", "C:\\v\\b.pdf", "C:\\v\\c.docx"],
            &[],
            &fake,
            &["rimage"],
            no_compress(),
        );
        // A PDF made smaller for screens needs Ghostscript.
        assert_eq!(
            view.read_with(&visual, |v, cx| v.needs(cx)),
            [("rimage", true), ("ghostscript", false)]
        );
        assert!(disabled(&mut visual, "shrink-start"));
        for label in ["Photos", "PDFs", "JPEG quality", "Where"] {
            assert_eq!(role_of(&mut visual, label), Some(Role::RadioGroup), "{label}");
        }
        // Lossless needs pdfcpu instead; JPEG XL needs libjxl in place of rimage for the photo.
        click(&mut visual, ("shrink-pdf", 2_usize));
        assert_eq!(
            view.read_with(&visual, |v, cx| v.needs(cx)),
            [("rimage", true), ("pdfcpu", false)]
        );
        click(&mut visual, ("shrink-photo", 1_usize));
        assert_eq!(
            view.read_with(&visual, |v, cx| v.needs(cx)),
            [("rimage", true), ("libjxl", false), ("pdfcpu", false)]
        );
        // The Office file still wants rimage; the quality choice stays for it.
        assert!(has(&mut visual, "JPEG quality"));
    }

    #[gpui_kit::test]
    fn folders_are_offered_compress_and_a_run_with_only_folders_is_not_possible(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(all_engines(), report(&[], 0, 0));
        let heard: Rc<RefCell<Vec<Vec<String>>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = heard.clone();
        let compress: CompressHook = Rc::new(move |_, _, folders| sink.borrow_mut().push(folders));
        let _view = open_view(&mut visual, &[], &["C:\\v\\Photos"], &fake, &[], compress);
        assert_eq!(role_of(&mut visual, "Make smaller: Photos"), Some(Role::Group));
        assert!(has(
            &mut visual,
            "“Photos” is a folder: compress it in place (it still opens normally), or into one smaller file."
        ));
        assert!(
            disabled(&mut visual, "shrink-start"),
            "nothing but folders: nothing to make smaller"
        );
        // No files, so no "Where" either.
        assert!(!has(&mut visual, "Where"));
        click(&mut visual, "shrink-compress");
        assert_eq!(*heard.borrow(), vec![strings(&["C:\\v\\Photos"])]);
        visual.update(|window, cx| {
            assert!(
                !window.has_active_dialog(cx),
                "the dialog closes when only folders were in it"
            )
        });
    }

    #[gpui_kit::test]
    fn progress_stop_failure_and_back_to_jpeg(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(all_engines(), report(&["C:\\p\\a.jpg"], 100, 50));
        let view = open_view(&mut visual, &["C:\\p\\ملف.png"], &[], &fake, &["rimage"], no_compress());
        let run = view.read_with(&visual, |v, _| v.run().clone());
        testing::set_rtl(true);
        visual.update(|_, cx| {
            run.update(cx, |run, cx| {
                run.set(
                    Run::Running(Progress {
                        done: 0,
                        total: 1,
                        current: "ملف.png".into(),
                        part: 0.4,
                    }),
                    cx,
                )
            })
        });
        assert_eq!(
            role_of(&mut visual, "Making smaller · ملف.png · 40%"),
            Some(Role::ProgressIndicator)
        );
        click(&mut visual, "shrink-stop");
        assert_eq!(fake.cancelled.lock().map(|n| *n).unwrap_or(0), 1);
        visual.update(|_, cx| run.update(cx, |run, cx| run.set(Run::Failed("Ghostscript crashed".into()), cx)));
        assert_eq!(
            role_of(&mut visual, "Couldn't make them smaller. Ghostscript crashed"),
            Some(Role::Alert)
        );
        testing::set_rtl(false);
        // Back to JPEG makes JPEGs beside the .jxl files and nothing of the others.
        let backend: Arc<dyn Backend> = fake.clone();
        visual.update(|window, cx| {
            back_to_jpeg_with(window, cx, backend, strings(&["C:\\p\\a.jxl", "C:\\p\\b.jpg"]));
        });
        settle(&mut visual, |_| fake.jxl.lock().map(|j| !j.is_empty()).unwrap_or(false));
        let jxl = fake.jxl.lock().map(|j| j.clone()).unwrap_or_default();
        assert_eq!(jxl[0].0, ["C:\\p\\a.jxl"]);
        assert_eq!(jxl[0].1, Output::Beside { suffix: String::new() });
    }
}
