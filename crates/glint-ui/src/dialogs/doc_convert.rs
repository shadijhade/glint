//! Converting documents: "Convert document…" (pandoc) and "Export as PDF" (Typst).
//! Ported from src/components/explorer/DocConvertDialog.tsx, built the way [`super::drive_health`] is.
//!
//! | TypeScript                           | Here                                                              |
//! |--------------------------------------|-------------------------------------------------------------------|
//! | `READS`, `canConvertDoc`, `isTypst`  | [`READS`], [`can_convert_doc`], [`is_typst`] (by name; the menus know the rest) |
//! | `TARGETS`                            | [`targets`]                                                       |
//! | `openDocConvert(entries, target)`    | [`open`]                                                          |
//! | `exportTypstPdf(entries)`            | [`export_pdf`] with `typst` true: Typst in process ([`Backend::export_pdf`]) |
//! | `exportMarkdownPdf(paths)`, print.rs's `markdown_pdf` | [`export_pdf`] with `typst` false: the Markdown as Typst markup, typeset in process |
//! | `DocConvertDialog` / `DocConvertModal` | [`DocConvertDialog`]                                            |
//! | `invoke("doc_convert")`, `doc_cancel`| [`Backend::convert`], [`Backend::cancel`] ([`CoreBackend`] is glint-core) |
//! | `useActionRun`, `WhereChooser`, `RunProgress`, `RunResult`, `RunFooter` | [`action_run`]                   |
//! | `ToolNeeded id="pandoc"` and `"typst"` | `settings::tools::needed`, through [`action_run::Tooling`]      |
//!
//! **How the coordinator wires it.**
//! * `MenuEvent::ConvertDocument { paths }` is `dialogs::doc_convert::open(window, cx, paths, None)`. It keeps the
//!   documents among `paths` by their names and says so in a notification when none is left.
//! * `MenuEvent::ExportPdf { paths, typst }` is `dialogs::doc_convert::export_pdf(window, cx, paths, typst)`.
//!   It makes a PDF beside each file, as the TypeScript did (Ctrl+Z takes it back), with no dialog and no tool to
//!   download: Typst is linked in (`crate::typst_engine`, the `typst` feature).
//!
//! **Export as PDF without a WebView.** `.typ` files are typeset by Typst in process (the TypeScript ran the
//! `typst` tool). Markdown printed the preview with WebView2's print-to-PDF (`markdown_pdf`, print.rs), which can't
//! move to glint-core because there is no WebView; it is now `typst_engine::from_markdown`, the Markdown written
//! as Typst markup (headings, paragraphs, emphasis, code, lists, tables, pictures beside the file, links, quotes,
//! footnotes, escaped; Arabic and Hebrew paragraphs typeset right to left) and typeset the same way. The page
//! looks like Typst's, not exactly like the preview. Both go through `actions::run`, so the PDF lands beside the
//! file (a name that's taken gets "(2)") and Undo takes it back, as before.
//!
//! **Math and diagrams in the Markdown PDF.** The print page of the TypeScript drew `$..$` and `$$..$$` math and
//! Mermaid blocks; here the same crates as the preview draw them (RaTeX for math, merman for Mermaid, layout-rs
//! for Graphviz) into SVG, and Typst gets each SVG as an image served from memory, never written to disk
//! (`from_markdown::Options::graphics`). Inline math sits on the text's baseline; display math, ```` ```math ````
//! and diagram blocks are centred. A formula or diagram that doesn't draw (or that Typst can't read) stays as its
//! source text and never fails the export. Without the `typst` feature the old paths stay: `.typ` files go through
//! core's `doc_convert` (the Typst tool) and Markdown opens this dialog with PDF chosen.
//!
//! Enter in the dialog doesn't start the conversion (the TypeScript's modal submitted on Enter); the buttons do.

use super::action_run;

use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use action_run::{
    choices, error_view, footer, help, on_core, progress_text, progress_view, result_view, section_title, shell, whole,
    ActionRun, Choice, FooterSpec, Fut, ResultWords, Run, Tooling, WhereChooser,
};
use glint_core::actions::{Output, Progress, Report};
use glint_core::docconvert::Target;
use glint_model::format::{format_count, plural};
use glint_model::paths::base_name;
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// What pandoc reads (docconvert.rs's reader).
pub const READS: &[&str] = &[
    "docx", "odt", "md", "markdown", "mdown", "mkd", "html", "htm", "xhtml", "epub", "tex", "latex", "rtf", "rst",
    "org", "textile", "ipynb", "typ", "fb2",
];

/// The lowercase extension of the file called `path` ("" for none).
fn ext_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Whether Convert document takes the file called `path`, by its extension (`canConvertDoc` minus the entry's
/// state: a file on disk, there to be read, is for the menus to know).
pub fn can_convert_doc(path: &str) -> bool {
    READS.contains(&ext_of(path).as_str())
}

/// A Typst file, for Export as PDF (`isTypst`).
pub fn is_typst(path: &str) -> bool {
    ext_of(path) == "typ"
}

// ---- The calls --------------------------------------------------------------------------------------------

/// The calls a conversion makes. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// Converts `files` into `target` (`doc_convert`), reporting progress on `progress`.
    fn convert(
        &self,
        files: Vec<String>,
        target: Target,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report>;
    /// Stops the conversion under way (`doc_cancel`).
    fn cancel(&self);
    /// Makes a PDF beside each of `files`, which are Typst files or Markdown files (by their names): the whole of
    /// Export as PDF. With the `typst` feature Typst runs in this process; without it core's `doc_convert` does
    /// (the Typst tool, and only for Typst files).
    fn export_pdf(&self, files: Vec<String>) -> Fut<Report>;
}

/// glint-core's converter, owned by the main window as the shell dialogs it opens are.
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
    fn convert(
        &self,
        files: Vec<String>,
        target: Target,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report> {
        match action_run::main_window(&self.app) {
            Ok(window) => on_core(glint_core::docconvert::doc_convert(
                window, files, target, output, progress,
            )),
            Err(reason) => Box::pin(async move { Err(reason) }),
        }
    }

    fn cancel(&self) {
        glint_core::docconvert::doc_cancel();
    }

    fn export_pdf(&self, files: Vec<String>) -> Fut<Report> {
        in_process::export(files)
    }
}

/// Stands in when core isn't started (never in the app).
struct Unavailable;

impl Backend for Unavailable {
    fn convert(
        &self,
        _: Vec<String>,
        _: Target,
        _: Output,
        _: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report> {
        action_run::no_core()
    }

    fn cancel(&self) {}

    fn export_pdf(&self, _: Vec<String>) -> Fut<Report> {
        action_run::no_core()
    }
}

fn core_backend(cx: &App) -> Arc<dyn Backend> {
    match cx.try_global::<core_bridge::Core>() {
        Some(core) => Arc::new(CoreBackend::new(core.app.clone())),
        None => Arc::new(Unavailable),
    }
}

// ---- What can be chosen -----------------------------------------------------------------------------------

/// One format a document can become (`TARGETS`).
#[derive(Clone, Debug)]
pub struct TargetInfo {
    pub target: Target,
    pub label: SharedString,
    pub hint: SharedString,
    /// The extensions of files already in this format.
    pub ext: &'static [&'static str],
}

/// The formats offered, in the TypeScript's order.
pub fn targets() -> Vec<TargetInfo> {
    let one = |target, label: SharedString, hint: SharedString, ext| TargetInfo {
        target,
        label,
        hint,
        ext,
    };
    vec![
        one(Target::Docx, t!("Word"), ".docx".into(), &["docx"]),
        one(Target::Pdf, t!("PDF"), t!("Typeset by Typst"), &["pdf"]),
        one(
            Target::Markdown,
            t!("Markdown"),
            ".md".into(),
            &["md", "markdown", "mdown", "mkd"],
        ),
        one(
            Target::Html,
            t!("Web page"),
            t!(".html, pictures inside"),
            &["html", "htm", "xhtml"],
        ),
        one(Target::Odt, t!("OpenDocument"), ".odt".into(), &["odt"]),
        one(Target::Epub, t!("E-book"), ".epub".into(), &["epub"]),
        one(Target::Pptx, t!("PowerPoint"), t!("Each heading a slide"), &["pptx"]),
        one(Target::Latex, t!("LaTeX"), ".tex".into(), &["tex", "latex"]),
        one(Target::Rtf, t!("Rich text"), ".rtf".into(), &["rtf"]),
        one(Target::Text, t!("Plain text"), ".txt".into(), &["txt"]),
        one(Target::Rst, t!("reStructuredText"), ".rst".into(), &["rst"]),
        one(Target::Org, t!("Org"), ".org".into(), &["org"]),
        one(Target::Typst, t!("Typst"), ".typ".into(), &["typ"]),
        one(Target::Ipynb, t!("Notebook"), ".ipynb".into(), &["ipynb"]),
    ]
}

/// The format to start with: PDF for Typst files, Markdown for Word files, else Word (`openDocConvert`).
pub fn first_target(files: &[String], asked: Option<Target>) -> Target {
    if let Some(target) = asked {
        return target;
    }
    let exts: HashSet<String> = files.iter().map(|f| ext_of(f)).collect();
    if exts.len() == 1 && exts.contains("typ") {
        Target::Pdf
    } else if exts.len() == 1 && exts.contains("docx") {
        Target::Markdown
    } else {
        Target::Docx
    }
}

/// Whether every one of `files` is already in the format `info` (no choice).
fn already(files: &[String], info: &TargetInfo) -> bool {
    let exts: HashSet<String> = files.iter().map(|f| ext_of(f)).collect();
    exts.iter().all(|e| info.ext.contains(&e.as_str()))
}

/// A text format: its pictures go into a folder beside each (the TypeScript's list for the extra sentence).
fn writes_text(target: Target) -> bool {
    matches!(
        target,
        Target::Markdown | Target::Latex | Target::Rst | Target::Org | Target::Typst | Target::Text
    )
}

// ---- The dialog -------------------------------------------------------------------------------------------

/// "name" or "3 documents": what the dialog is called after.
pub fn what(files: &[String]) -> SharedString {
    match files {
        [one] => isolate(base_name(one).to_string()),
        _ => t!("{n} documents", n = format_count(files.len())),
    }
}

/// Converting some documents: the format, where the results go, the run and its result.
pub struct DocConvertDialog {
    files: Vec<String>,
    backend: Arc<dyn Backend>,
    tooling: Tooling,
    target: Target,
    where_: Entity<WhereChooser>,
    run: Entity<ActionRun>,
    _subscriptions: Vec<Subscription>,
}

impl DocConvertDialog {
    /// A dialog for `files`, the format `first` chosen, calling `backend`, its install cards reading `tooling`.
    pub fn new(
        files: Vec<String>,
        first: Target,
        backend: Arc<dyn Backend>,
        tooling: Tooling,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let where_ = cx.new(|cx| {
            WhereChooser::new(
                "doc-convert-where",
                t!("document"),
                t!("A name that's taken gets “(2)”, and Ctrl+Z takes it all back."),
                true,
                ("", "Converted"),
                window,
                cx,
            )
        });
        let run = cx.new(|_| ActionRun::new());
        let mut subscriptions = vec![
            cx.observe(&where_, |_, _, cx| cx.notify()),
            cx.observe(&run, |_, _, cx| cx.notify()),
        ];
        subscriptions.extend(action_run::watch_tools(&tooling, cx));
        Self {
            files,
            backend,
            tooling,
            target: first,
            where_,
            run,
            _subscriptions: subscriptions,
        }
    }

    /// The documents being converted.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// The format chosen.
    pub fn target(&self) -> Target {
        self.target
    }

    /// Where the run stands.
    #[cfg(test)]
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

    /// Whether the chosen format is one every document is in already.
    fn target_taken(&self) -> bool {
        targets()
            .iter()
            .find(|info| info.target == self.target)
            .is_some_and(|info| already(&self.files, info))
    }

    /// pandoc is there, and Typst too when the format is PDF (`ready`).
    pub fn ready(&self, cx: &App) -> bool {
        self.tooling.ready("pandoc", cx) && (self.target != Target::Pdf || self.tooling.ready("typst", cx))
    }

    /// Whether Convert can be pressed: the tools are there, the names usable and the format a choice.
    pub fn can_start(&self, cx: &App) -> bool {
        !self.run.read(cx).state().started()
            && self.ready(cx)
            && !self.where_.read(cx).choice(cx).bad()
            && !self.target_taken()
    }

    /// Starts the conversion on core's runtime; progress and the result come back here.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if !self.can_start(cx) {
            return;
        }
        let output = self.where_.read(cx).choice(cx).output();
        let (channel, receiver) = core_bridge::stream::<Progress>();
        let call = self.backend.convert(self.files.clone(), self.target, output, channel);
        let total = self.files.len();
        self.run.update(cx, |run, cx| {
            run.start(
                total,
                call,
                receiver,
                |report| t!("conversion of {what}", what = plural(report.made.len(), "document")).to_string(),
                cx,
            )
        });
        cx.notify();
    }

    fn stop(&mut self) {
        self.backend.cancel();
    }
}

impl Render for DocConvertDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let state = self.run.read(cx).state().clone();
        let target = self.target;
        let exts: HashSet<String> = self.files.iter().map(|f| ext_of(f)).collect();
        let from_word = exts.contains("docx") || exts.contains("odt");

        let body = match state.clone() {
            Run::Setup => {
                let mut parts: Vec<AnyElement> = Vec::new();
                parts.extend(self.tooling.card(
                    "pandoc",
                    t!("pandoc converts documents on this PC, reading only the document it's given."),
                    IconName::FileType,
                    cx,
                ));
                if target == Target::Pdf {
                    parts.extend(self.tooling.card(
                        "typst",
                        t!("Typst typesets the PDF, without LaTeX or Word."),
                        IconName::FileType,
                        cx,
                    ));
                }
                let files = self.files.clone();
                parts.push(
                    col()
                        .gap(space(1.5))
                        .child(section_title(t!("Into"), &t))
                        .child(choices::<Self, Target>(
                            "doc-convert-target",
                            t!("Into"),
                            target,
                            targets()
                                .into_iter()
                                .map(|info| {
                                    let taken = already(&files, &info);
                                    Choice::new(info.target, info.label).hint(info.hint).disabled(taken)
                                })
                                .collect(),
                            3,
                            |this, picked, _, _| this.target = picked,
                            &t,
                            cx,
                        ))
                        .into_any_element(),
                );
                let mut note =
                    t!("Text, headings, lists, tables, pictures, footnotes and links come across").to_string();
                if from_word {
                    note.push_str(&t!(
                        "; Word's exact page layout doesn't (an exact copy of a Word file is Word's Save as PDF)"
                    ));
                }
                note.push('.');
                if target == Target::Pptx {
                    note.push(' ');
                    note.push_str(&t!("Each heading becomes a slide."));
                }
                if writes_text(target) {
                    note.push(' ');
                    note.push_str(&t!("Pictures go into a folder beside each."));
                }
                note.push(' ');
                note.push_str(&t!(
                    "A document written right to left (Arabic, Hebrew) stays right to left."
                ));
                parts.push(
                    help(note, &t)
                        .id("doc-convert-note")
                        .role(Role::Group)
                        .aria_label(t!("What comes across"))
                        .test_support()
                        .into_any_element(),
                );
                parts.push(self.where_.clone().into_any_element());
                col().gap(space(4.)).children(parts).into_any_element()
            }
            Run::Running(progress) => progress_view(
                "doc-convert-progress",
                progress_text(&t!("Converting"), &progress, false, false),
                whole(&progress),
                &t,
            )
            .into_any_element(),
            Run::Done(report) => {
                result_view("doc-convert-result", &report, &ResultWords::files(false), &t).into_any_element()
            }
            Run::Failed(reason) => {
                error_view("doc-convert-error", t!("Couldn't convert"), reason, &t).into_any_element()
            }
        };

        let spec = FooterSpec {
            prefix: "doc-convert",
            start_label: t!("Convert"),
            start_icon: IconName::FileType,
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
        shell(
            "doc-convert",
            t!("Convert {what}", what = what(&self.files)),
            action_run::where_they_are(&self.files),
            body,
            footer,
            window,
            &t,
        )
    }
}

/// Opens Convert document on the documents among `paths` in `window`, `target` chosen if given (`openDocConvert`).
/// When none is left it says so in a notification instead.
pub fn open(window: &mut Window, cx: &mut App, paths: Vec<String>, target: Option<Target>) {
    let files: Vec<String> = paths.into_iter().filter(|p| can_convert_doc(p)).collect();
    if files.is_empty() {
        window.push_notification(
            Notification::info(t!(
                "Convert document takes Word, OpenDocument, Markdown, HTML, EPUB, LaTeX, RTF and other documents"
            )),
            cx,
        );
        return;
    }
    let backend = core_backend(cx);
    let tooling = Tooling::new(action_run::tools_model(window, cx));
    let first = first_target(&files, target);
    open_with(window, cx, backend, tooling, files, first);
}

/// [`open`] for a given backend and tools, without filtering `files` (tests pass fakes). Returns the dialog.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    tooling: Tooling,
    files: Vec<String>,
    first: Target,
) -> Entity<DocConvertDialog> {
    let view = cx.new(|cx| DocConvertDialog::new(files, first, backend, tooling, window, cx));
    super::show(window, cx, t!("Convert document"), px(576.), view.clone());
    view
}

/// A Markdown file, for Export as PDF (`isMarkdown`).
pub fn is_markdown(path: &str) -> bool {
    matches!(ext_of(path).as_str(), "md" | "markdown" | "mdown" | "mkd" | "mkdn")
}

/// "Export as PDF" (`exportTypstPdf`, and Markdown's `pdfFromMarkdown`): a PDF beside each of `paths`, with no
/// dialog, and Ctrl+Z takes them back. `typst` says the menu was for Typst files; otherwise it was for Markdown.
///
/// Without the `typst` feature Markdown opens Convert document with PDF chosen instead (the old stand-in for the
/// WebView print, which needs pandoc and the Typst tool).
pub fn export_pdf(window: &mut Window, cx: &mut App, paths: Vec<String>, typst: bool) {
    let backend = core_backend(cx);
    if typst {
        export_typst_with(window, cx, backend, paths);
    } else {
        export_markdown_with(window, cx, backend, paths);
    }
}

/// [`export_pdf`] for Typst files with a backend of your own (tests pass a fake).
pub fn export_typst_with(window: &mut Window, cx: &mut App, backend: Arc<dyn Backend>, paths: Vec<String>) {
    let files: Vec<String> = paths.into_iter().filter(|p| is_typst(p)).collect();
    if files.is_empty() {
        return;
    }
    run_export(window, cx, backend.export_pdf(files));
}

/// [`export_pdf`] for Markdown files with a backend of your own. Says it has begun, as the TypeScript's loading
/// toast did: “name” for one file, “3 files” for more.
pub fn export_markdown_with(window: &mut Window, cx: &mut App, backend: Arc<dyn Backend>, paths: Vec<String>) {
    let files: Vec<String> = paths.into_iter().filter(|p| is_markdown(p)).collect();
    if files.is_empty() {
        return;
    }
    let loading = match files.as_slice() {
        [one] => t!("Exporting “{name}” as PDF…", name = isolate(base_name(one).to_string())),
        _ => t!("Exporting {n} files as PDF…", n = format_count(files.len())),
    };
    window.push_notification(Notification::info(loading), cx);
    run_export(window, cx, backend.export_pdf(files));
}

/// Waits for an export and tells the person what it made, with Undo behind Ctrl+Z, or why it didn't.
fn run_export(window: &mut Window, cx: &mut App, call: Fut<Report>) {
    window
        .spawn(cx, async move |cx| {
            let result = call.await;
            let _ = cx.update(|window, cx| match result {
                Ok(report) => {
                    if !report.made.is_empty() {
                        let what = t!("PDF export of {what}", what = plural(report.made.len(), "file"));
                        action_run::remember_undo(&report, &what, cx);
                        window.push_notification(Notification::success(what).message(t!("Ctrl+Z takes it back")), cx);
                    }
                    if let Some((_, why)) = report.failed.first() {
                        window.push_notification(Notification::error(why.clone()).title(t!("Couldn't export it")), cx);
                    }
                }
                Err(reason) => {
                    window.push_notification(Notification::error(reason).title(t!("Couldn't export it")), cx);
                }
            });
        })
        .detach();
}

/// The exports in this process: Typst files and Markdown files typeset by `typst_engine`, placed by core's
/// `actions::run` (beside the file, numbered when the name is taken, Undo-able).
pub mod in_process {
    use super::*;
    use crate::typst_engine::{self, from_markdown, Failure, Input, Limits, EXPORT_BUDGET};
    use glint_core::actions::Job;
    use std::path::PathBuf;

    /// A Markdown file bigger than this isn't turned into a document (a data file with the wrong extension).
    pub const MAX_MARKDOWN_BYTES: u64 = 64 * 1024 * 1024;

    /// A PDF beside each of `files`, on core's blocking threads (see [`run`]).
    pub fn export(files: Vec<String>) -> Fut<Report> {
        on_core(glint_core::fsx::blocking(move || run(&files)))
    }

    /// A PDF beside each of `files`: Typst files as they are, Markdown as Typst markup. Blocks until done, so it
    /// is for a thread that may wait (never the UI thread).
    pub fn run(files: &[String]) -> Result<Report, String> {
        if files.is_empty() {
            return Err(t!("None of them is a file that becomes a PDF").to_string());
        }
        let jobs: Vec<Job> = files.iter().map(Job::one).collect();
        // Nothing stops an export from outside, as the TypeScript's didn't: the jobs run to the end.
        let stop = AtomicBool::new(false);
        glint_core::actions::run(
            0,
            "pdf-export",
            &jobs,
            &Output::Beside { suffix: String::new() },
            2,
            &stop,
            &|_| {},
            |job, part, work| {
                let file = job.inputs.first().ok_or_else(|| "A job has no files".to_string())?;
                make_pdf(file, part, work).map(|made| vec![made])
            },
        )
    }

    /// One file's PDF, made in `work`.
    fn make_pdf(file: &Path, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<PathBuf, String> {
        let name = file.to_string_lossy();
        let limits = Limits::new(EXPORT_BUDGET);
        let pdf = if is_typst(&name) {
            part(0.2);
            typst_engine::pdf_of(Input::File(file.to_path_buf()), &limits)
        } else if is_markdown(&name) {
            let (text, options) = markdown_source(file)?;
            part(0.2);
            from_markdown::pdf_of_markdown(&text, &options, &limits)
        } else {
            return Err(t!("It isn't a Typst or Markdown file").to_string());
        }
        .map_err(failure_text)?;
        part(0.9);
        let out = glint_core::actions::renamed(work, file, "pdf");
        std::fs::write(&out, pdf).map_err(|e| format!("{}: {e}", out.display()))?;
        Ok(out)
    }

    /// The Markdown text at `file`, and how to typeset it: pictures found beside the file, math and diagrams drawn.
    pub(super) fn markdown_source(file: &Path) -> Result<(String, from_markdown::Options), String> {
        let meta = std::fs::metadata(file).map_err(|e| format!("{}: {e}", file.display()))?;
        if meta.len() > MAX_MARKDOWN_BYTES {
            return Err(t!("It is too big to make a document of").to_string());
        }
        let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let dir = file
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let options = from_markdown::Options {
            title: None,
            base_dir: Some(dir),
            graphics: true,
            ..from_markdown::Options::default()
        };
        Ok((text.to_string(), options))
    }

    /// What a failed export says for its file: the first error with its place, and how many more there were.
    fn failure_text(failure: Failure) -> String {
        let errors = failure
            .diagnostics
            .iter()
            .filter(|d| d.severity == typst_engine::Severity::Error)
            .count();
        let first = failure.summary();
        if errors > 1 {
            format!("{first} ({})", t!("and {n} more", n = errors - 1))
        } else {
            first
        }
    }
}

// ---- Tests -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A backend that answers from memory and remembers what it was asked.
    struct Fake {
        calls: Mutex<Vec<(Vec<String>, Target, Output)>>,
        report: Report,
        error: Option<String>,
        cancelled: Mutex<usize>,
        exports: Mutex<Vec<Vec<String>>>,
    }

    impl Fake {
        fn new(report: Report) -> Arc<Fake> {
            Arc::new(Fake {
                calls: Mutex::new(Vec::new()),
                report,
                error: None,
                cancelled: Mutex::new(0),
                exports: Mutex::new(Vec::new()),
            })
        }

        fn exports(&self) -> Vec<Vec<String>> {
            self.exports.lock().map(|e| e.clone()).unwrap_or_default()
        }

        fn calls(&self) -> Vec<(Vec<String>, Target, Output)> {
            self.calls.lock().map(|c| c.clone()).unwrap_or_default()
        }
    }

    impl Backend for Fake {
        fn convert(
            &self,
            files: Vec<String>,
            target: Target,
            output: Output,
            progress: glint_core::host::ipc::Channel<Progress>,
        ) -> Fut<Report> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push((files, target, output));
            }
            let _ = progress.send(Progress {
                done: 0,
                total: 1,
                current: "a.docx".into(),
                part: 0.5,
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

        fn cancel(&self) {
            if let Ok(mut n) = self.cancelled.lock() {
                *n += 1;
            }
        }

        fn export_pdf(&self, files: Vec<String>) -> Fut<Report> {
            if let Ok(mut exports) = self.exports.lock() {
                exports.push(files);
            }
            let report = self.report.clone();
            let error = self.error.clone();
            Box::pin(async move {
                match error {
                    Some(error) => Err(error),
                    None => Ok(report),
                }
            })
        }
    }

    fn report(made: &[&str]) -> Report {
        Report {
            made: made.iter().map(|s| (*s).to_string()).collect(),
            elapsed_ms: 1500.,
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
        panic!("the document dialog never reached the state the test waits for");
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

    fn open_view(
        cx: &mut VisualTestContext,
        files: &[&str],
        first: Target,
        fake: &Arc<Fake>,
        tools: &[&'static str],
    ) -> Entity<DocConvertDialog> {
        let backend: Arc<dyn Backend> = fake.clone();
        let files = strings(files);
        let tooling = Tooling::with(tools);
        let view = cx.update(|window, cx| open_with(window, cx, backend, tooling, files, first));
        cx.run_until_parked();
        view
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::FileType,
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
    fn what_it_takes_and_which_format_comes_first_read_as_the_typescript_did() {
        assert!(
            can_convert_doc("C:\\d\\a.DOCX") && can_convert_doc("C:\\d\\notes.md") && can_convert_doc("C:\\d\\p.typ")
        );
        assert!(
            !can_convert_doc("C:\\d\\a.pdf") && !can_convert_doc("C:\\d\\Folder") && !can_convert_doc("C:\\d\\a.txt")
        );
        assert!(is_typst("C:\\d\\p.TYP") && !is_typst("C:\\d\\a.docx"));
        // Typst files become PDFs, Word files Markdown, everything else Word; a request wins.
        assert_eq!(
            first_target(&strings(&["C:\\d\\a.typ", "C:\\d\\b.typ"]), None),
            Target::Pdf
        );
        assert_eq!(first_target(&strings(&["C:\\d\\a.docx"]), None), Target::Markdown);
        assert_eq!(
            first_target(&strings(&["C:\\d\\a.docx", "C:\\d\\b.md"]), None),
            Target::Docx
        );
        assert_eq!(first_target(&strings(&["C:\\d\\a.md"]), None), Target::Docx);
        assert_eq!(
            first_target(&strings(&["C:\\d\\a.docx"]), Some(Target::Pdf)),
            Target::Pdf
        );
        let _guard = testing::lock();
        assert_eq!(targets().len(), 14);
        // A format every chosen file is already in is no choice.
        let docx = targets().into_iter().find(|t| t.target == Target::Docx);
        assert!(docx.is_some_and(|info| already(&strings(&["C:\\d\\a.docx"]), &info)));
        let md = targets().into_iter().find(|t| t.target == Target::Markdown);
        assert!(md.is_some_and(|info| !already(&strings(&["C:\\d\\a.md", "C:\\d\\b.docx"]), &info)));
        assert_eq!(what(&strings(&["C:\\d\\a.docx"])), "a.docx");
        assert_eq!(what(&strings(&["C:\\d\\a.docx", "C:\\d\\b.docx"])), "2 documents");
    }

    #[gpui_kit::test]
    fn pdf_wants_typst_too_and_the_run_reports_what_it_made(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(report(&["C:\\d\\a.md", "C:\\d\\b.md"]));
        let view = open_view(
            &mut visual,
            &["C:\\d\\a.docx", "C:\\d\\b.docx"],
            Target::Markdown,
            &fake,
            &["pandoc"],
        );
        visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
        assert_eq!(role_of(&mut visual, "Convert 2 documents"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "C:\\d"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Into"), Some(Role::RadioGroup));
        assert_eq!(role_of(&mut visual, "Where"), Some(Role::RadioGroup));
        assert_eq!(role_of(&mut visual, "Markdown. .md"), Some(Role::RadioButton));
        // Word files are in Word already: that is no choice.
        assert!(disabled(&mut visual, ("doc-convert-target", 0_usize)));
        assert!(!disabled(&mut visual, "doc-convert-start"));
        assert_eq!(role_of(&mut visual, "What comes across"), Some(Role::Group));
        // PDF needs Typst, which isn't there.
        click(&mut visual, ("doc-convert-target", 1_usize));
        assert_eq!(view.read_with(&visual, |v, _| v.target()), Target::Pdf);
        assert!(disabled(&mut visual, "doc-convert-start"), "Typst typesets the PDF");
        // Back to Markdown, and run.
        click(&mut visual, ("doc-convert-target", 2_usize));
        assert!(!disabled(&mut visual, "doc-convert-start"));
        click(&mut visual, "doc-convert-start");
        settle(&mut visual, |cx| has(cx, "Made 2 files"));
        assert_eq!(role_of(&mut visual, "Made 2 files"), Some(Role::Status));
        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, ["C:\\d\\a.docx", "C:\\d\\b.docx"]);
        assert_eq!(calls[0].1, Target::Markdown);
        assert_eq!(calls[0].2, Output::Beside { suffix: String::new() });
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("doc-convert-show").is_some());
            window.click("doc-convert-close", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn progress_stop_and_failures_are_drawn_with_arabic_names(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(report(&[]));
        let view = open_view(&mut visual, &["C:\\d\\a.md"], Target::Docx, &fake, &["pandoc"]);
        let run = view.read_with(&visual, |v, _| v.run().clone());
        visual.update(|_, cx| {
            run.update(cx, |run, cx| {
                run.set(
                    Run::Running(Progress {
                        done: 0,
                        total: 1,
                        current: "a.md".into(),
                        part: 0.5,
                    }),
                    cx,
                )
            })
        });
        // One document: no "1 of 1", but the file and how far it is.
        assert_eq!(
            role_of(&mut visual, "Converting · a.md · 50%"),
            Some(Role::ProgressIndicator)
        );
        click(&mut visual, "doc-convert-stop");
        assert_eq!(fake.cancelled.lock().map(|n| *n).unwrap_or(0), 1);
        testing::set_rtl(true);
        let mut failed = report(&[]);
        failed.failed = vec![("C:\\d\\تقرير.docx".into(), "pandoc stopped".into())];
        visual.update(|_, cx| run.update(cx, |run, cx| run.set(Run::Done(failed), cx)));
        assert_eq!(role_of(&mut visual, "Nothing was made"), Some(Role::Status));
        assert_eq!(role_of(&mut visual, "1 file not done:"), Some(Role::List));
        assert_eq!(role_of(&mut visual, "تقرير.docx: pandoc stopped"), Some(Role::ListItem));
        visual.update(|_, cx| run.update(cx, |run, cx| run.set(Run::Failed("pandoc isn't installed".into()), cx)));
        assert_eq!(
            role_of(&mut visual, "Couldn't convert. pandoc isn't installed"),
            Some(Role::Alert)
        );
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn exporting_typst_files_runs_without_a_dialog_and_other_files_open_nothing(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(report(&["C:\\d\\paper.pdf"]));
        let backend: Arc<dyn Backend> = fake.clone();
        visual.update(|window, cx| {
            export_typst_with(window, cx, backend, strings(&["C:\\d\\paper.typ", "C:\\d\\notes.md"]));
        });
        settle(&mut visual, |_| !fake.exports().is_empty());
        assert_eq!(fake.exports(), [["C:\\d\\paper.typ"]], "only the Typst file");
        assert!(fake.calls().is_empty(), "the converter and its tools are not asked");
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "no dialog for Typst files"));
        // It said what it made: the PDF export, and that Ctrl+Z takes it back.
        visual.update(|window, cx| assert_eq!(window.notifications(cx).len(), 1));
        // Nothing it takes: it says so, and nothing opens.
        visual.update(|window, cx| open(window, cx, strings(&["C:\\d\\a.pdf"]), None));
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn exporting_markdown_says_it_began_then_what_it_made_and_leaves_other_files_alone(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let mut made = report(&["C:\\d\\notes.pdf"]);
        made.failed = vec![("C:\\d\\bad.md".into(), "bad.md: no such file".into())];
        let fake = Fake::new(made);
        let backend: Arc<dyn Backend> = fake.clone();
        visual.update(|window, cx| {
            export_markdown_with(
                window,
                cx,
                backend,
                strings(&["C:\\d\\notes.MD", "C:\\d\\bad.md", "C:\\d\\paper.typ", "C:\\d\\a.docx"]),
            );
            // The loading notice is there at once, before the work is done.
            assert_eq!(window.notifications(cx).len(), 1);
        });
        settle(&mut visual, |_| !fake.exports().is_empty());
        assert_eq!(
            fake.exports(),
            [["C:\\d\\notes.MD", "C:\\d\\bad.md"]],
            "only the Markdown files"
        );
        // The loading notice, the success and the failure.
        settle(&mut visual, |cx| {
            cx.update(|window, cx| window.notifications(cx).len()) == 3
        });
        // No Markdown: nothing begins and nothing is asked.
        let before = fake.exports().len();
        let backend: Arc<dyn Backend> = fake.clone();
        visual.update(|window, cx| export_markdown_with(window, cx, backend, strings(&["C:\\d\\paper.typ"])));
        visual.run_until_parked();
        assert_eq!(fake.exports().len(), before);
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[test]
    fn markdown_names_are_told_by_their_extension() {
        assert!(is_markdown("C:\\d\\a.md") && is_markdown("C:\\d\\A.MARKDOWN") && is_markdown("C:\\d\\a.mkdn"));
        assert!(!is_markdown("C:\\d\\a.txt") && !is_markdown("C:\\d\\a.typ") && !is_markdown("C:\\d\\md"));
    }

    /// The exports for real: Typst and Markdown typeset by Typst itself, placed by core's `actions::run`.
    mod real {
        use super::super::*;
        use crate::testing;
        use std::fs;
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Scratch(PathBuf);

        impl Scratch {
            fn new(tag: &str) -> Self {
                static N: AtomicUsize = AtomicUsize::new(0);
                let dir = std::env::temp_dir().join(format!(
                    "glint-export-test-{}-{tag}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::SeqCst)
                ));
                let _ = fs::remove_dir_all(&dir);
                fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("making {}: {e}", dir.display()));
                Scratch(dir)
            }

            fn file(&self, name: &str, text: &str) -> String {
                let path = self.0.join(name);
                fs::write(&path, text).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
                path.to_string_lossy().into_owned()
            }

            fn names(&self) -> Vec<String> {
                let mut names: Vec<String> = fs::read_dir(&self.0)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                names.sort();
                names
            }
        }

        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        fn pdf_at(path: &str) -> Vec<u8> {
            fs::read(path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
        }

        #[test]
        fn typst_and_markdown_files_become_pdfs_beside_them_and_a_taken_name_gets_a_number() {
            let _guard = testing::lock();
            let dir = Scratch::new("beside");
            let typ = dir.file("paper.typ", "= A paper\n\nSome *words*.\n");
            let md = dir.file(
                "ملاحظات.md",
                "# ملاحظات الرحلة\n\nهذا نص عربي طويل بما يكفي ليُقرأ من اليمين إلى اليسار.\n\n\
                 | الاسم | العدد |\n|---|---|\n| تفاح | 3 |\n\n- بند أول\n- بند ثان\n\n```rust\nlet x = 1;\n```\n",
            );
            let english = dir.file(
                "notes.md",
                "# Notes\n\nA *plain* English note with `code` and a [link](https://example.com).\n",
            );
            let report = in_process::run(&[typ.clone(), md.clone(), english.clone()]).unwrap_or_else(|e| panic!("{e}"));
            assert!(report.failed.is_empty(), "{:?}", report.failed);
            assert_eq!(report.made.len(), 3, "{:?}", report.made);
            assert_eq!(
                dir.names(),
                [
                    "notes.md",
                    "notes.pdf",
                    "paper.pdf",
                    "paper.typ",
                    "ملاحظات.md",
                    "ملاحظات.pdf"
                ],
                "no work folder is left"
            );
            for made in &report.made {
                let bytes = pdf_at(made);
                assert!(bytes.starts_with(b"%PDF-"), "{made}");
                assert!(bytes.len() > 800, "{made}: {} bytes", bytes.len());
            }
            // And again: the names are taken, so they are numbered and the first PDFs are untouched.
            let again = in_process::run(&[typ]).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(again.made.len(), 1);
            assert!(again.made[0].ends_with("paper (2).pdf"), "{:?}", again.made);
            assert!(report.before > 0 && report.after > 0);
        }

        #[test]
        fn what_goes_wrong_is_said_for_its_file_with_its_place() {
            let _guard = testing::lock();
            let dir = Scratch::new("wrong");
            let broken = dir.file("broken.typ", "= Fine\n\n#nosuchfunction(1)\n");
            let package = dir.file("uses.typ", "#import \"@preview/cetz:0.3.4\": canvas\n");
            let text = dir.file("notes.txt", "not a document\n");
            let good = dir.file("good.md", "# Good\n");
            let report = in_process::run(&[broken, package, text, good]).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(report.made.len(), 1, "{:?}", report.made);
            assert!(report.made[0].ends_with("good.pdf"));
            assert_eq!(report.failed.len(), 3, "{:?}", report.failed);
            let why = |name: &str| {
                report
                    .failed
                    .iter()
                    .find(|(file, _)| file.ends_with(name))
                    .map(|(_, why)| why.clone())
                    .unwrap_or_else(|| panic!("{name} didn't fail: {:?}", report.failed))
            };
            assert!(
                why("broken.typ").starts_with("broken.typ:3:2: "),
                "{}",
                why("broken.typ")
            );
            assert!(why("broken.typ").contains("nosuchfunction"));
            assert!(why("uses.typ").contains("without going online"), "{}", why("uses.typ"));
            assert!(why("uses.typ").starts_with("uses.typ:1:"));
            assert_eq!(why("notes.txt"), "It isn't a Typst or Markdown file");
            assert_eq!(
                dir.names(),
                ["broken.typ", "good.md", "good.pdf", "notes.txt", "uses.typ"]
            );
            assert!(in_process::run(&[]).is_err());
        }

        #[test]
        fn markdown_pictures_beside_the_file_are_drawn_and_missing_ones_are_described() {
            let _guard = testing::lock();
            let dir = Scratch::new("pictures");
            let picture = image::RgbaImage::from_pixel(40, 30, image::Rgba([20, 120, 220, 255]));
            let mut png = Vec::new();
            picture
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap_or_else(|e| panic!("png: {e}"));
            fs::write(dir.0.join("blue.png"), &png).unwrap_or_else(|e| panic!("{e}"));
            let md = dir.file(
                "doc.md",
                "# With pictures\n\n![a blue box](blue.png)\n\n![gone](gone.png) and ![far](https://example.com/x.png)\n\n\
                 Text with $ and # and * and _ and <angle> and [brackets] and a\\\\backslash.\n",
            );
            let report = in_process::run(&[md]).unwrap_or_else(|e| panic!("{e}"));
            assert!(report.failed.is_empty(), "{:?}", report.failed);
            let with_picture = pdf_at(&report.made[0]);
            // Without the picture the same document is smaller.
            fs::remove_file(dir.0.join("blue.png")).unwrap_or_else(|e| panic!("{e}"));
            let md = dir.file(
                "plain.md",
                "# With pictures\n\n![a blue box](blue.png)\n\n![gone](gone.png) and ![far](https://example.com/x.png)\n\n\
                 Text with $ and # and * and _ and <angle> and [brackets] and a\\\\backslash.\n",
            );
            let report = in_process::run(&[md]).unwrap_or_else(|e| panic!("{e}"));
            assert!(report.failed.is_empty(), "{:?}", report.failed);
            let without = pdf_at(&report.made[0]);
            assert!(
                with_picture.len() > without.len(),
                "{} against {}",
                with_picture.len(),
                without.len()
            );
        }
    }
}
