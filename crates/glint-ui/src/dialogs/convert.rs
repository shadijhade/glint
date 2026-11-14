//! Converting pictures: "Convert picture…" and "Convert N pictures…".
//! Ported from src/components/explorer/ConvertDialog.tsx, built the way [`super::drive_health`] is.
//!
//! | TypeScript                         | Here                                                                |
//! |------------------------------------|---------------------------------------------------------------------|
//! | `CONVERTS`, `canConvert`           | [`CONVERTS`], [`can_convert`] (by name; the menus know the rest)    |
//! | `convertPictures(entries)`         | [`open`]                                                            |
//! | `ConvertDialog` / `ConvertModal`   | [`ConvertDialog`]                                                   |
//! | `convertImages`, `convertCancel`   | [`Backend::convert`], [`Backend::cancel`] ([`CoreBackend`] is glint-core) |
//! | `last` (the choices of last time)  | [`Last`], kept in a `Global` while Glint runs                       |
//! | `rememberUndo` + `offerUndo`       | [`action_run::remember_undo`] (Ctrl+Z)                              |
//! | `ToolNeeded id="rimage"`           | `settings::tools::needed`, through [`action_run::Tooling`]          |
//!
//! **How the coordinator wires it.**
//! * `MenuEvent::ConvertPictures { paths }` is `dialogs::convert::open(window, cx, paths)`. It keeps the pictures
//!   among `paths` by their names (the menus have checked that they are on disk) and says so in a notification
//!   when none is left.
//!
//! Where the TypeScript had a drop-down for the size and a slider for the quality, this has a group of choices
//! and a box with minus and plus (see [`action_run::NumberField`]).

use super::action_run;

use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use action_run::{
    choices, error_view, field, footer, on_core, progress_text, progress_view, result_view, section_title, shell,
    whole, ActionRun, Choice, FooterSpec, Fut, NumberField, ResultWords, Run, Tooling, Where, WhereChooser, WhereKind,
};
use glint_core::actions::{Progress, Recycled, Report};
use glint_core::convert::{ConvertOptions, ConvertProgress, ConvertReport, Format, Output as ConvertOutput, Resize};
use glint_model::format::{format_count, plural};
use glint_model::paths::{base_name, parent_path};
use std::path::Path;
use std::sync::Arc;

/// What rimage reads (convert.rs's `READS`).
pub const CONVERTS: &[&str] = glint_core::convert::READS;

/// Whether Convert takes the file called `path`, by its extension (`canConvert` minus the entry's state: a file
/// on disk, there to be read, is for the menus to know).
pub fn can_convert(path: &str) -> bool {
    Path::new(path)
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .is_some_and(|ext| CONVERTS.contains(&ext.as_str()))
}

// ---- The calls --------------------------------------------------------------------------------------------

/// The calls a conversion makes. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// Converts `pictures` as `options` say (`convert_images`), reporting progress on `progress`.
    fn convert(
        &self,
        pictures: Vec<String>,
        options: ConvertOptions,
        progress: glint_core::host::ipc::Channel<ConvertProgress>,
    ) -> Fut<ConvertReport>;
    /// Stops the conversion under way (`convert_cancel`).
    fn cancel(&self);
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
        pictures: Vec<String>,
        options: ConvertOptions,
        progress: glint_core::host::ipc::Channel<ConvertProgress>,
    ) -> Fut<ConvertReport> {
        match action_run::main_window(&self.app) {
            Ok(window) => on_core(glint_core::convert::convert_images(window, pictures, options, progress)),
            Err(reason) => Box::pin(async move { Err(reason) }),
        }
    }

    fn cancel(&self) {
        glint_core::convert::convert_cancel();
    }
}

/// Stands in when core isn't started (never in the app).
struct Unavailable;

impl Backend for Unavailable {
    fn convert(
        &self,
        _: Vec<String>,
        _: ConvertOptions,
        _: glint_core::host::ipc::Channel<ConvertProgress>,
    ) -> Fut<ConvertReport> {
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

// ---- What can be chosen -----------------------------------------------------------------------------------

/// The formats offered (`FORMATS`): the format, its name and what it is good for.
fn formats() -> Vec<Choice<Format>> {
    vec![
        Choice::new(Format::Jpeg, "JPEG").hint(t!("Photos, opened by everything (MozJPEG)")),
        Choice::new(Format::Png, "PNG").hint(t!("Lossless: screenshots and drawings (OxiPNG)")),
        Choice::new(Format::Webp, "WebP").hint(t!("Smaller than JPEG, for the web")),
        Choice::new(Format::Avif, "AVIF").hint(t!("The smallest, for newer apps and browsers")),
    ]
}

/// The sizes offered (`SIZES`), in the TypeScript's order: what is shown and what core is told.
pub fn sizes() -> Vec<(SharedString, Option<Resize>)> {
    vec![
        (t!("Keep their size"), None),
        ("75%".into(), Some(Resize::Percent { value: 75 })),
        ("50%".into(), Some(Resize::Percent { value: 50 })),
        ("25%".into(), Some(Resize::Percent { value: 25 })),
        (
            t!("Longest side {n} px", n = 3840),
            Some(Resize::Longest { value: 3840 }),
        ),
        (
            t!("Longest side {n} px", n = 2560),
            Some(Resize::Longest { value: 2560 }),
        ),
        (
            t!("Longest side {n} px", n = 1920),
            Some(Resize::Longest { value: 1920 }),
        ),
        (
            t!("Longest side {n} px", n = 1280),
            Some(Resize::Longest { value: 1280 }),
        ),
    ]
}

/// The choices made last time, for the next (while Glint runs): `last` of the TypeScript.
#[derive(Clone, Debug, PartialEq)]
pub struct Last {
    pub format: Format,
    pub quality: i64,
    /// An index into [`sizes`].
    pub size: usize,
    pub strip: bool,
    pub kind: WhereKind,
    pub suffix: String,
    pub folder: String,
}

impl Default for Last {
    fn default() -> Self {
        Self {
            format: Format::Webp,
            quality: 80,
            size: 0,
            strip: false,
            kind: WhereKind::Beside,
            suffix: String::new(),
            folder: "Converted".into(),
        }
    }
}

struct LastChoices(Last);

impl Global for LastChoices {}

fn last(cx: &App) -> Last {
    cx.try_global::<LastChoices>().map(|l| l.0.clone()).unwrap_or_default()
}

/// core's options for a set of choices (`start`).
pub fn options_of(last: &Last, output: &Where) -> ConvertOptions {
    ConvertOptions {
        format: last.format,
        quality: last.quality.clamp(1, 100) as u8,
        resize: sizes().get(last.size).and_then(|(_, resize)| resize.clone()),
        strip: last.strip,
        output: match output {
            Where::Beside { suffix } => ConvertOutput::Beside { suffix: suffix.clone() },
            Where::Folder { name } => ConvertOutput::Folder {
                name: name.trim().to_string(),
            },
            Where::Replace => ConvertOutput::Replace,
        },
    }
}

/// What a conversion did, as the file actions' report: what was made, the pictures it replaced and where they
/// went in the Recycle Bin, the sizes.
pub fn report_of(done: ConvertReport) -> Report {
    Report {
        made: done.converted.iter().map(|c| c.to.clone()).collect(),
        recycled: done
            .converted
            .iter()
            .filter_map(|c| {
                Some(Recycled {
                    path: c.from.clone(),
                    bin: c.recycled.clone()?,
                })
            })
            .collect(),
        failed: done.failed,
        before: done.converted.iter().map(|c| c.before).sum(),
        after: done.converted.iter().map(|c| c.after).sum(),
        stopped: done.stopped,
        elapsed_ms: done.elapsed_ms,
    }
}

// ---- The dialog -------------------------------------------------------------------------------------------

/// "Convert “name”" or "Convert 3 pictures": the dialog's title.
pub fn title(pictures: &[String]) -> SharedString {
    match pictures {
        [one] => t!("Convert “{name}”", name = isolate(base_name(one).to_string())),
        _ => t!("Convert {n} pictures", n = format_count(pictures.len())),
    }
}

/// Converting some pictures: the format, quality, size and where the results go; then the run and its result.
pub struct ConvertDialog {
    pictures: Vec<String>,
    backend: Arc<dyn Backend>,
    tooling: Tooling,
    format: Format,
    size: usize,
    strip: bool,
    quality: Entity<NumberField>,
    where_: Entity<WhereChooser>,
    run: Entity<ActionRun>,
    /// What was asked for, once it started (Show decides by it where to look).
    started_with: Option<Where>,
    _subscriptions: Vec<Subscription>,
}

impl ConvertDialog {
    /// A dialog for `pictures`, calling `backend`, its install card reading `tooling`.
    pub fn new(
        pictures: Vec<String>,
        backend: Arc<dyn Backend>,
        tooling: Tooling,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let previous = last(cx);
        let quality = cx.new(|cx| {
            NumberField::new(
                "convert-quality",
                t!("Quality"),
                SharedString::default(),
                (1, 100, 5),
                previous.quality,
                window,
                cx,
            )
        });
        let where_ = cx.new(|cx| {
            WhereChooser::new(
                "convert-where",
                t!("picture"),
                t!("A name that's taken gets “(2)”. Each keeps its picture's date modified, and Ctrl+Z takes it all back."),
                true,
                (&previous.suffix, &previous.folder),
                window,
                cx,
            )
        });
        where_.update(cx, |chooser, cx| {
            chooser.set(previous.kind, None, None, window, cx);
        });
        let run = cx.new(|_| ActionRun::new());
        let mut subscriptions = vec![
            cx.observe(&quality, |_, _, cx| cx.notify()),
            cx.observe(&where_, |_, _, cx| cx.notify()),
            cx.observe(&run, |_, _, cx| cx.notify()),
        ];
        subscriptions.extend(action_run::watch_tools(&tooling, cx));
        Self {
            pictures,
            backend,
            tooling,
            format: previous.format,
            size: previous.size.min(sizes().len() - 1),
            strip: previous.strip,
            quality,
            where_,
            run,
            started_with: None,
            _subscriptions: subscriptions,
        }
    }

    /// The pictures being converted.
    pub fn pictures(&self) -> &[String] {
        &self.pictures
    }

    /// The format chosen.
    pub fn format(&self) -> Format {
        self.format
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

    /// The quality box.
    #[cfg(test)]
    pub fn quality_field(&self) -> &Entity<NumberField> {
        &self.quality
    }

    /// Where the results go.
    pub fn where_chooser(&self) -> &Entity<WhereChooser> {
        &self.where_
    }

    /// Everything chosen now, as the next dialog starts from it.
    fn choices_now(&self, cx: &App) -> Last {
        let chooser = self.where_.read(cx);
        Last {
            format: self.format,
            quality: self.quality.read(cx).value(cx),
            size: self.size,
            strip: self.strip,
            kind: chooser.kind(),
            suffix: chooser.suffix_input().read(cx).value().to_string(),
            folder: chooser.folder_input().read(cx).value().to_string(),
        }
    }

    fn remember(&self, cx: &mut Context<Self>) {
        let now = self.choices_now(cx);
        cx.set_global(LastChoices(now));
    }

    /// Whether Convert can be pressed: rimage is there, the names are usable and the quality is a number.
    pub fn can_start(&self, cx: &App) -> bool {
        !self.run.read(cx).state().started()
            && self.tooling.ready("rimage", cx)
            && !self.where_.read(cx).choice(cx).bad()
            && (self.format == Format::Png || self.quality.read(cx).valid(cx))
    }

    /// Starts the conversion on core's runtime; progress and the result come back here.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if !self.can_start(cx) {
            return;
        }
        self.remember(cx);
        let output = self.where_.read(cx).choice(cx);
        let options = options_of(&self.choices_now(cx), &output);
        self.started_with = Some(output);
        let (channel, receiver) = core_bridge::stream::<ConvertProgress>();
        let call = self.backend.convert(self.pictures.clone(), options, channel);
        let call: Fut<Report> = Box::pin(async move { call.await.map(report_of) });
        let total = self.pictures.len();
        self.run.update(cx, |run, cx| {
            run.start_mapped(
                total,
                call,
                receiver,
                |p| Progress {
                    done: p.done,
                    total: p.total,
                    current: p.current,
                    part: 0.,
                },
                |report| t!("conversion of {what}", what = plural(report.made.len(), "picture")).to_string(),
                cx,
            )
        });
        cx.notify();
    }

    fn stop(&mut self) {
        self.backend.cancel();
    }

    /// What Show reveals: the first picture made, selected, or its folder when it went into one of its own.
    fn show(&self, run: &Run) -> Option<String> {
        let made = run.first_made()?;
        match self.started_with {
            Some(Where::Folder { .. }) => Some(parent_path(&made)),
            _ => Some(made),
        }
    }

    fn words() -> ResultWords {
        ResultWords {
            made: |n| t!("Converted {what}", what = plural(n, "picture")),
            nothing: t!("Nothing was converted"),
            failed: |n| t!("{what} not converted:", what = plural(n, "picture")),
            sizes: true,
        }
    }
}

impl Render for ConvertDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let state = self.run.read(cx).state().clone();
        let format = self.format;

        let body = match state.clone() {
            Run::Setup => {
                let mut parts: Vec<AnyElement> = Vec::new();
                parts.extend(self.tooling.card(
                    "rimage",
                    t!("rimage converts pictures with the best encoders there are: MozJPEG, OxiPNG, WebP and AVIF."),
                    IconName::ImageDown,
                    cx,
                ));
                parts.push(
                    col()
                        .gap(space(1.5))
                        .child(section_title(t!("Into"), &t))
                        .child(choices::<Self, Format>(
                            "convert-format",
                            t!("Format"),
                            format,
                            formats(),
                            2,
                            |this, picked, _, cx| {
                                this.format = picked;
                                this.remember(cx);
                            },
                            &t,
                            cx,
                        ))
                        .into_any_element(),
                );
                if format != Format::Png {
                    parts.push(field(t!("Quality"), self.quality.clone(), &t).into_any_element());
                }
                let shown = sizes();
                parts.push(
                    field(
                        t!("Size"),
                        choices::<Self, usize>(
                            "convert-size",
                            t!("Size"),
                            self.size,
                            shown
                                .into_iter()
                                .enumerate()
                                .map(|(ix, (label, _))| Choice::new(ix, label))
                                .collect(),
                            2,
                            |this, picked, _, cx| {
                                this.size = picked;
                                this.remember(cx);
                            },
                            &t,
                            cx,
                        ),
                        &t,
                    )
                    .items_start()
                    .into_any_element(),
                );
                parts.push(
                    div()
                        .ps(23.)
                        .child(
                            Checkbox::new("convert-strip")
                                .label(t!("Take out the camera, the place and when it was taken"))
                                .checked(self.strip)
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.strip = *checked;
                                    this.remember(cx);
                                    cx.notify();
                                })),
                        )
                        .into_any_element(),
                );
                parts.push(self.where_.clone().into_any_element());
                col().gap(space(4.)).children(parts).into_any_element()
            }
            Run::Running(progress) => progress_view(
                "convert-progress",
                progress_text(&t!("Converting"), &progress, true, true),
                whole(&progress),
                &t,
            )
            .into_any_element(),
            Run::Done(report) => result_view("convert-result", &report, &Self::words(), &t).into_any_element(),
            Run::Failed(reason) => error_view("convert-error", t!("Couldn't convert"), reason, &t).into_any_element(),
        };

        let spec = FooterSpec {
            prefix: "convert",
            start_label: t!("Convert"),
            start_icon: IconName::ImageDown,
            can_start: self.can_start(cx),
            stoppable: true,
            show: self.show(&state),
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
            "convert",
            title(&self.pictures),
            action_run::where_they_are(&self.pictures),
            body,
            footer,
            window,
            &t,
        )
    }
}

/// Opens Convert on the pictures among `paths` in `window` (`convertPictures`). When none is left it says so in a
/// notification instead.
pub fn open(window: &mut Window, cx: &mut App, paths: Vec<String>) {
    let pictures: Vec<String> = paths.into_iter().filter(|p| can_convert(p)).collect();
    if pictures.is_empty() {
        window.push_notification(
            Notification::info(t!(
                "Convert reads JPEG, PNG, WebP, AVIF, TIFF, BMP, PSD, SVG and JPEG XL files."
            ))
            .title(t!("No pictures to convert")),
            cx,
        );
        return;
    }
    let backend = core_backend(cx);
    let tooling = Tooling::new(action_run::tools_model(window, cx));
    open_with(window, cx, backend, tooling, pictures);
}

/// [`open`] for a given backend and tools, without filtering `pictures` (tests pass fakes). Returns the dialog.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    tooling: Tooling,
    pictures: Vec<String>,
) -> Entity<ConvertDialog> {
    let shown = title(&pictures);
    let view = cx.new(|cx| ConvertDialog::new(pictures, backend, tooling, window, cx));
    super::show(window, cx, shown, px(576.), view.clone());
    view
}

// ---- Tests -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use glint_core::convert::Converted;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A backend that answers from memory and remembers what it was asked.
    struct Fake {
        calls: Mutex<Vec<(Vec<String>, ConvertOptions)>>,
        report: ConvertReport,
        error: Option<String>,
        cancelled: Mutex<usize>,
    }

    impl Fake {
        fn new(report: ConvertReport) -> Arc<Fake> {
            Arc::new(Fake {
                calls: Mutex::new(Vec::new()),
                report,
                error: None,
                cancelled: Mutex::new(0),
            })
        }
    }

    impl Backend for Fake {
        fn convert(
            &self,
            pictures: Vec<String>,
            options: ConvertOptions,
            progress: glint_core::host::ipc::Channel<ConvertProgress>,
        ) -> Fut<ConvertReport> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push((pictures, options));
            }
            let _ = progress.send(ConvertProgress {
                done: 1,
                total: 2,
                current: "a.png".into(),
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
    }

    fn converted(from: &str, to: &str, before: u64, after: u64) -> Converted {
        Converted {
            from: from.into(),
            to: to.into(),
            recycled: None,
            before,
            after,
        }
    }

    fn report() -> ConvertReport {
        ConvertReport {
            converted: vec![
                converted("C:\\p\\a.png", "C:\\p\\a.webp", 1000, 400),
                converted("C:\\p\\b.png", "C:\\p\\b.webp", 3000, 1200),
            ],
            failed: Vec::new(),
            stopped: false,
            elapsed_ms: 2500.,
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
        panic!("the convert dialog never reached the state the test waits for");
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

    fn disabled(cx: &mut VisualTestContext, id: &'static str) -> bool {
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

    fn type_into(cx: &mut VisualTestContext, input: &Entity<InputState>, text: &str) {
        let input = input.clone();
        let text = text.to_string();
        cx.update(|window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx));
            window.press("ctrl-a", cx);
            window.input(&text, cx);
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
        pictures: &[&str],
        fake: &Arc<Fake>,
        tools: &[&'static str],
    ) -> Entity<ConvertDialog> {
        let backend: Arc<dyn Backend> = fake.clone();
        let pictures = strings(pictures);
        let tooling = Tooling::with(tools);
        let view = cx.update(|window, cx| open_with(window, cx, backend, tooling, pictures));
        cx.run_until_parked();
        view
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::ImageDown,
            IconName::Minus,
            IconName::Plus,
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
    fn what_it_takes_and_what_core_is_told_read_as_the_typescript_did() {
        assert!(can_convert("C:\\p\\photo.JPG") && can_convert("C:\\p\\a.avif") && can_convert("C:\\p\\scan.tiff"));
        assert!(!can_convert("C:\\p\\notes.txt") && !can_convert("C:\\p\\Folder") && !can_convert("C:\\p\\.png"));
        let _guard = testing::lock();
        let sized = sizes();
        assert_eq!(sized.len(), 8);
        assert!(sized[0].1.is_none());
        assert_eq!(sized[4].0, "Longest side 3840 px");
        let last = Last {
            format: Format::Avif,
            quality: 60,
            size: 5,
            strip: true,
            ..Last::default()
        };
        let options = options_of(
            &last,
            &Where::Folder {
                name: "  Small  ".into(),
            },
        );
        assert_eq!(options.format, Format::Avif);
        assert_eq!(options.quality, 60);
        assert!(matches!(options.resize, Some(Resize::Longest { value: 2560 })));
        assert!(options.strip);
        assert!(matches!(options.output, ConvertOutput::Folder { ref name } if name == "Small"));
        assert!(matches!(
            options_of(&Last::default(), &Where::Replace).output,
            ConvertOutput::Replace
        ));
        // The report as the file actions have it: what was made, the pictures replaced, the sizes added up.
        let mut done = report();
        done.converted[0].recycled = Some("C:\\$Recycle.Bin\\x".into());
        done.failed = vec![("C:\\p\\c.png".into(), "Broken".into())];
        let mapped = report_of(done);
        assert_eq!(mapped.made, ["C:\\p\\a.webp", "C:\\p\\b.webp"]);
        assert_eq!(mapped.recycled.len(), 1);
        assert_eq!(mapped.recycled[0].path, "C:\\p\\a.png");
        assert_eq!((mapped.before, mapped.after), (4000, 1600));
        assert_eq!(mapped.failed.len(), 1);
    }

    #[test]
    fn titles_follow_the_typescript() {
        let _guard = testing::lock();
        assert_eq!(title(&strings(&["C:\\p\\a.png"])), "Convert “a.png”");
        assert_eq!(title(&strings(&["C:\\p\\a.png", "C:\\q\\b.png"])), "Convert 2 pictures");
        assert_eq!(action_run::change_text(4000, 1600), "\u{2212}60%");
        assert_eq!(action_run::change_text(1000, 1250), "+25%");
    }

    #[gpui_kit::test]
    fn the_choices_convert_and_the_result_read_as_the_typescript_did(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(report());
        let view = open_view(&mut visual, &["C:\\p\\a.png", "C:\\p\\b.png"], &fake, &["rimage"]);
        visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
        assert_eq!(role_of(&mut visual, "Convert 2 pictures"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "C:\\p"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Format"), Some(Role::RadioGroup));
        assert_eq!(role_of(&mut visual, "Size"), Some(Role::RadioGroup));
        assert_eq!(role_of(&mut visual, "Where"), Some(Role::RadioGroup));
        assert!(has(&mut visual, "Quality"));
        // WebP is the first choice; the quality shows for it and goes for PNG.
        assert_eq!(
            role_of(&mut visual, "WebP. Smaller than JPEG, for the web"),
            Some(Role::RadioButton)
        );
        assert!(!disabled(&mut visual, "convert-start"));
        click(&mut visual, ("convert-format", 1_usize));
        assert!(!has(&mut visual, "Quality"), "PNG has no quality");
        click(&mut visual, ("convert-format", 3_usize));
        assert!(has(&mut visual, "Quality"));
        assert_eq!(view.read_with(&visual, |v, _| v.format()), Format::Avif);
        // A size, and the quality stepped down.
        click(&mut visual, ("convert-size", 4_usize));
        click(&mut visual, ("convert-quality", 0_usize));
        // Into a folder of its own (its radio is below the fold of a window this small).
        let chooser = view.read_with(&visual, |v, _| v.where_chooser().clone());
        visual.update(|window, cx| chooser.update(cx, |c, cx| c.set(WhereKind::Folder, None, None, window, cx)));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("convert-start", cx);
        });
        settle(&mut visual, |cx| has(cx, "Converted 2 pictures"));
        assert_eq!(role_of(&mut visual, "Converted 2 pictures"), Some(Role::Status));
        // The sizes line is there, whatever way the locale writes the numbers.
        visual.update(|window, _| {
            assert!(window
                .find_all_by_role(Role::Status)
                .iter()
                .any(|e| e.label().is_some_and(|l| l.contains("(\u{2212}60%)"))));
        });
        let calls = fake.calls.lock().map(|c| c.clone()).unwrap_or_default();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, ["C:\\p\\a.png", "C:\\p\\b.png"]);
        let options = &calls[0].1;
        assert_eq!(options.format, Format::Avif);
        assert_eq!(options.quality, 75);
        assert!(matches!(options.resize, Some(Resize::Longest { value: 3840 })));
        assert!(matches!(&options.output, ConvertOutput::Folder { name } if name == "Converted"));
        // After a run: Show and Close, no Cancel and no Convert.
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("convert-show").is_some());
            assert!(window.try_find("convert-start").is_none());
            assert!(window.try_find("convert-cancel").is_none());
            window.click("convert-close", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        // The next dialog starts from the same choices.
        let again = open_view(&mut visual, &["C:\\p\\c.png"], &Fake::new(report()), &["rimage"]);
        assert_eq!(again.read_with(&visual, |v, _| v.format()), Format::Avif);
        assert_eq!(
            again.read_with(&visual, |v, cx| v.where_chooser().read(cx).kind()),
            WhereKind::Folder
        );
    }

    #[gpui_kit::test]
    fn convert_waits_for_rimage_and_for_names_windows_can_hold(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(report());
        // rimage isn't there: Convert is off.
        let _view = open_view(&mut visual, &["C:\\p\\a.png"], &fake, &[]);
        assert!(disabled(&mut visual, "convert-start"));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));

        let view = open_view(&mut visual, &["C:\\p\\a.png"], &fake, &["rimage"]);
        assert!(!disabled(&mut visual, "convert-start"));
        let suffix = view.read_with(&visual, |v, cx| v.where_chooser().read(cx).suffix_input().clone());
        type_into(&mut visual, &suffix, "a:b");
        assert!(disabled(&mut visual, "convert-start"), "a colon can't be in a name");
        type_into(&mut visual, &suffix, "-small");
        assert!(!disabled(&mut visual, "convert-start"));
        // A quality out of range is no number to convert with.
        let quality = view.read_with(&visual, |v, cx| v.quality_field().read(cx).input().clone());
        type_into(&mut visual, &quality, "300");
        assert!(disabled(&mut visual, "convert-start"));
        type_into(&mut visual, &quality, "90");
        assert!(!disabled(&mut visual, "convert-start"));
        // The suffix was typed, so "Next to each picture" is the radio on and the suffix is what core gets.
        click(&mut visual, "convert-start");
        settle(&mut visual, |cx| has(cx, "Converted 2 pictures"));
        let calls = fake.calls.lock().map(|c| c.clone()).unwrap_or_default();
        assert!(matches!(&calls[0].1.output, ConvertOutput::Beside { suffix } if suffix == "-small"));
        assert_eq!(calls[0].1.quality, 90);
    }

    #[gpui_kit::test]
    fn progress_stop_failures_and_arabic_names_are_drawn(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(report());
        let view = open_view(
            &mut visual,
            &["C:\\p\\a.png", "C:\\p\\b.png", "C:\\p\\c.png"],
            &fake,
            &["rimage"],
        );
        let run = view.read_with(&visual, |v, _| v.run().clone());
        visual.update(|_, cx| {
            run.update(cx, |run, cx| {
                run.set(
                    Run::Running(Progress {
                        done: 1,
                        total: 3,
                        current: "a.png".into(),
                        part: 0.,
                    }),
                    cx,
                )
            })
        });
        assert_eq!(
            role_of(&mut visual, "Converting 2 of 3 · a.png done"),
            Some(Role::ProgressIndicator)
        );
        assert!(disabled(&mut visual, "convert-cancel"));
        click(&mut visual, "convert-stop");
        assert_eq!(fake.cancelled.lock().map(|n| *n).unwrap_or(0), 1);

        // A whole failure, in Arabic with an Arabic name.
        testing::set_rtl(true);
        visual.update(|_, cx| run.update(cx, |run, cx| run.set(Run::Failed("الملف تالف".into()), cx)));
        assert_eq!(role_of(&mut visual, "Couldn't convert. الملف تالف"), Some(Role::Alert));
        // Some made, some not: the failed ones listed by name, six at most.
        let mut partial = Report {
            made: vec!["C:\\p\\a.webp".into()],
            elapsed_ms: 900.,
            stopped: true,
            ..Default::default()
        };
        partial.failed = (0..8)
            .map(|i| (format!("C:\\p\\صورة{i}.png"), "Couldn't read it".to_string()))
            .collect();
        visual.update(|_, cx| run.update(cx, |run, cx| run.set(Run::Done(partial), cx)));
        assert_eq!(
            role_of(&mut visual, "Converted 1 picture before it was stopped"),
            Some(Role::Status)
        );
        assert_eq!(role_of(&mut visual, "8 pictures not converted:"), Some(Role::List));
        assert_eq!(
            role_of(&mut visual, "صورة0.png: Couldn't read it"),
            Some(Role::ListItem)
        );
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find_all_by_role(Role::ListItem).len(), 6);
        });
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn a_failed_call_is_reported_and_nothing_to_convert_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let mut value = Fake::new(report());
        if let Some(inner) = Arc::get_mut(&mut value) {
            inner.error = Some("rimage isn't working".into());
        }
        let _view = open_view(&mut visual, &["C:\\p\\a.png"], &value, &["rimage"]);
        click(&mut visual, "convert-start");
        settle(&mut visual, |cx| has(cx, "Couldn't convert. rimage isn't working"));
        // Opening it on files it doesn't read opens nothing.
        visual.update(|window, cx| {
            window.close_dialog(cx);
            open(window, cx, strings(&["C:\\p\\notes.txt"]));
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }
}
