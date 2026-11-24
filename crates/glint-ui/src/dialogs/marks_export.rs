//! Export marks… (docs/gpui/marks, F24, F33): a folder's marks below it into one file, Glint's lines or XYplorer's
//! format. The file's name starts as the folder's own; the work is core's (on its runtime, ended by Stop).

use super::action_run::{choices, Choice, Fut};
use crate::kit::prelude::*;
use crate::platform::pickers::{self, Options};
use crate::{core_bridge, state, t, theme};
use glint_core::marks::Format;
use glint_model::format::plural;
use glint_model::marks::Label;
use glint_model::paths::base_name;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// What the dialog asks of the world, so tests give it a fake.
pub trait Backend: Send + Sync {
    /// Windows' Save as: where the file goes (None: cancelled). `owner` is the window's handle.
    fn pick_save(&self, owner: isize, folder: String, name: String) -> Fut<Option<String>>;
    /// Writes the marks of everything below `root` to `to`; how many items. `stop` ends it.
    fn export(&self, root: String, to: String, format: Format, labels: Vec<Label>, stop: Arc<AtomicBool>) -> Fut<u64>;
}

/// Core's export and Windows' picker.
pub struct CoreBackend;

impl Backend for CoreBackend {
    fn pick_save(&self, owner: isize, folder: String, name: String) -> Fut<Option<String>> {
        let options = Options::new().title(t!("Export marks")).start(folder).owner(owner);
        pickers::save_file(options, &name)
    }

    fn export(&self, root: String, to: String, format: Format, labels: Vec<Label>, stop: Arc<AtomicBool>) -> Fut<u64> {
        super::action_run::on_core(glint_core::marks::export::marks_export_with(root, to, format, labels, stop))
    }
}

/// Where the dialog stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Run {
    /// Asking what to export and where.
    Setup,
    Running,
    /// How many items were written (0: no file).
    Done(u64),
    Failed(SharedString),
}

/// The file's name for a format: the folder's own for Glint's lines, the name XYplorer's tagload is told for its.
fn file_name(name: &str, format: Format) -> String {
    match format {
        Format::Glint => format!("{name}.glint-marks.jsonl"),
        Format::Xyplorer => "XYplorerTag.dat".to_string(),
    }
}

/// Export marks… of one folder.
pub struct ExportDialog {
    backend: Arc<dyn Backend>,
    folder: String,
    /// The folder's name, for the file's and the words.
    name: String,
    format: Format,
    /// The file's path.
    to: String,
    /// The person chose the path: a change of format leaves it.
    chosen: bool,
    /// The last path chosen was beside the folder, not in it or above it: it wasn't taken.
    refused: bool,
    run: Run,
    stop: Arc<AtomicBool>,
}

impl ExportDialog {
    fn new(backend: Arc<dyn Backend>, folder: String, cx: &mut Context<Self>) -> Self {
        let base = base_name(&folder);
        let name = if base.is_empty() { "marks".to_string() } else { base.to_string() };
        let mut this = Self {
            backend,
            folder,
            name,
            format: Format::Glint,
            to: String::new(),
            chosen: false,
            refused: false,
            run: Run::Setup,
            stop: Arc::new(AtomicBool::new(false)),
        };
        this.to = this.default_to();
        // Leaving the dialog while it works ends the work.
        cx.on_release(|this, _| this.stop.store(true, Ordering::Relaxed)).detach();
        this
    }

    fn default_to(&self) -> String {
        format!("{}\\{}", self.folder.trim_end_matches(['\\', '/']), file_name(&self.name, self.format))
    }

    /// The file's path.
    pub fn to(&self) -> &str {
        &self.to
    }

    /// Where the dialog stands.
    pub fn run(&self) -> &Run {
        &self.run
    }

    fn set_format(&mut self, format: Format) {
        self.format = format;
        if !self.chosen {
            self.to = self.default_to();
        }
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = core_bridge::hwnd_of(window).map_or(0, |h| h.0 as isize);
        let name = base_name(&self.to).to_string();
        let start = self.folder.clone();
        let picked = self.backend.pick_save(owner, start, name);
        cx.spawn(async move |this, cx| {
            if let Ok(Some(path)) = picked.await {
                let _ = this.update(cx, |this, cx| {
                    // The paths are written from where the file is (core says the same): in the folder or above it.
                    let base = std::path::Path::new(&path).parent().map(|p| p.to_string_lossy().into_owned());
                    this.refused = base.and_then(|base| glint_core::marks::export::rest_under(&base, &this.folder)).is_none();
                    if !this.refused {
                        this.to = path;
                        this.chosen = true;
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if self.run != Run::Setup {
            return;
        }
        self.run = Run::Running;
        self.stop = Arc::new(AtomicBool::new(false));
        let labels = state::settings(cx).labels.clone();
        let done = self.backend.export(self.folder.clone(), self.to.clone(), self.format, labels, self.stop.clone());
        cx.spawn(async move |this, cx| {
            let result = done.await;
            let _ = this.update(cx, |this, cx| {
                this.run = match result {
                    Ok(n) => Run::Done(n),
                    Err(reason) => Run::Failed(reason.into()),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.stop.store(true, Ordering::Relaxed);
        cx.notify();
    }
}

impl Render for ExportDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let (running, finished) = (self.run == Run::Running, matches!(self.run, Run::Done(_) | Run::Failed(_)));
        // Enter is the footer's main button.
        if finished {
            super::main_action(cx, true, |_, window, cx| window.close_dialog(cx));
        } else {
            super::main_action(cx, !running, |this, _, cx| this.start(cx));
        }
        let name = isolate(self.name.clone());
        let status = |id: &'static str, role: Role, line: SharedString, color: Hsla| {
            div()
                .id(id)
                .role(role)
                .aria_label(line.clone())
                .test_support()
                .py(space(4.))
                .text_color(color)
                .child(line)
        };

        let body = match &self.run {
            Run::Setup => col()
                .gap(space(4.))
                .child({
                    let line = t!("Everything marked in “{name}” and below it, into one file.", name = name.clone());
                    div().id("marks-export-about").role(Role::Group).aria_label(line.clone()).test_support().child(line)
                })
                .child(choices::<Self, Format>(
                    "marks-export-format",
                    t!("Format"),
                    self.format,
                    vec![
                        Choice::new(Format::Glint, t!("Glint"))
                            .hint(t!("Text, one line an item: read by Import marks… on any PC")),
                        Choice::new(Format::Xyplorer, t!("XYplorer"))
                            .hint(t!("An XYplorerTag.dat that XYplorer's tagload merges")),
                    ],
                    2,
                    |this, format, _, _| this.set_format(format),
                    &t,
                    cx,
                ))
                .child(
                    align_row(2.)
                        .child(
                            div()
                                .id("marks-export-path")
                                .role(Role::Group)
                                .aria_label(self.to.clone())
                                .test_support()
                                .flex_1()
                                .min_w_0()
                                // A long path is cut in its middle: the drive and the file's name stay.
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis_middle()
                                .text_size(theme::size::XS)
                                .text_color(t.muted_foreground)
                                .child(isolate(self.to.clone())),
                        )
                        .child(
                            Button::new("marks-export-choose")
                                .outline()
                                .label(t!("Choose…"))
                                .on_click(cx.listener(|this, _, window, cx| this.choose(window, cx))),
                        ),
                )
                .children(self.refused.then(|| {
                    let line = t!(
                        "Keep the file in “{name}” or a folder above it: the paths in it are written from where it is",
                        name = name.clone()
                    );
                    div()
                        .id("marks-export-refused")
                        .role(Role::Alert)
                        .aria_label(line.clone())
                        .test_support()
                        .text_size(theme::size::XS)
                        .text_color(t.destructive)
                        .child(line)
                }))
                .into_any_element(),
            Run::Running => align_row(2.)
                .py(space(4.))
                .text_color(t.muted_foreground)
                .child(Spinner::new())
                .child(
                    div()
                        .id("marks-export-running")
                        .role(Role::Status)
                        .aria_label(t!("Exporting…"))
                        .test_support()
                        .child(t!("Exporting…")),
                )
                .into_any_element(),
            Run::Done(0) => status(
                "marks-export-done",
                Role::Status,
                t!("Nothing below “{name}” is marked", name = name),
                t.foreground,
            )
            .into_any_element(),
            Run::Done(n) => status(
                "marks-export-done",
                Role::Status,
                t!("Exported the marks of {n}", n = plural(usize::try_from(*n).unwrap_or(usize::MAX), "item")),
                t.foreground,
            )
            .into_any_element(),
            Run::Failed(reason) => {
                status("marks-export-error", Role::Alert, reason.clone(), t.destructive).into_any_element()
            }
        };

        let footer = row_start()
            .justify_end()
            .gap(space(2.))
            .pt(space(3.))
            .border_t_1()
            .border_color(t.border)
            .when(running, |this| {
                this.child(
                    Button::new("marks-export-stop")
                        .outline()
                        .icon(IconName::Square)
                        .label(t!("Stop"))
                        .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
                )
            })
            .when(finished, |this| {
                this.child(
                    Button::new("marks-export-close")
                        .primary()
                        .label(t!("Close"))
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                )
            })
            .when(!running && !finished, |this| {
                this.child(
                    Button::new("marks-export-cancel")
                        .outline()
                        .label(t!("Cancel"))
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                )
                .child(
                    Button::new("marks-export-start")
                        .primary()
                        .label(t!("Export"))
                        .on_click(cx.listener(|this, _, _, cx| this.start(cx))),
                )
            });

        col()
            .id("marks-export")
            .role(Role::Group)
            .aria_label(t!("Export marks"))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .pt(space(1.))
            .child(body)
            .child(footer)
    }
}

/// Opens the export of everything marked below `folder`.
pub fn open(window: &mut Window, cx: &mut App, folder: String) {
    open_with(window, cx, Arc::new(CoreBackend), folder);
}

/// [`open`] on a backend of the caller's (tests pass a fake). Returns the dialog.
pub fn open_with(window: &mut Window, cx: &mut App, backend: Arc<dyn Backend>, folder: String) -> Entity<ExportDialog> {
    let view = cx.new(|cx| ExportDialog::new(backend, folder, cx));
    super::show(window, cx, t!("Export marks"), px(520.), view.clone());
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;

    type Asked = (String, String, Format, Vec<Label>, Arc<AtomicBool>);

    /// A backend that answers as told and records what it was asked.
    struct Fake {
        answer: Mutex<Result<u64, String>>,
        hang: bool,
        picks: Option<String>,
        asked: Mutex<Vec<Asked>>,
    }

    impl Fake {
        fn new(answer: Result<u64, String>, hang: bool, picks: Option<&str>) -> Arc<Self> {
            Arc::new(Self {
                answer: Mutex::new(answer),
                hang,
                picks: picks.map(String::from),
                asked: Mutex::new(Vec::new()),
            })
        }

        fn asked(&self) -> Vec<Asked> {
            self.asked.lock().map(|a| a.clone()).unwrap_or_default()
        }
    }

    impl Backend for Fake {
        fn pick_save(&self, _: isize, _: String, _: String) -> Fut<Option<String>> {
            let picked = self.picks.clone();
            Box::pin(async move { Ok(picked) })
        }

        fn export(&self, root: String, to: String, format: Format, labels: Vec<Label>, stop: Arc<AtomicBool>) -> Fut<u64> {
            if let Ok(mut asked) = self.asked.lock() {
                asked.push((root, to, format, labels, stop));
            }
            if self.hang {
                return Box::pin(std::future::pending());
            }
            let answer = self.answer.lock().map(|a| a.clone()).unwrap_or(Ok(0));
            Box::pin(async move { answer })
        }
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the export dialog never reached the state the test waits for");
    }

    fn opened(cx: &mut TestAppContext, fake: &Arc<Fake>, folder: &str) -> (VisualTestContext, Entity<ExportDialog>) {
        cx.update(testing::init);
        let mut visual = testing::open_window(cx, |_, cx| cx.new(|_| Host)).0;
        let backend: Arc<dyn Backend> = fake.clone();
        let folder = folder.to_string();
        let view = visual.update(|window, cx| open_with(window, cx, backend, folder));
        (visual, view)
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

    fn click(cx: &mut VisualTestContext, id: impl Into<ElementId>) {
        let id = id.into();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        cx.run_until_parked();
    }

    fn press(cx: &mut VisualTestContext, key: &str) {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        cx.run_until_parked();
    }

    fn to(cx: &mut VisualTestContext, view: &Entity<ExportDialog>) -> String {
        view.read_with(cx, |view, _| view.to().to_string())
    }

    const JSONL: &str = "C:\\Docs\\Docs.glint-marks.jsonl";
    const XY: &str = "C:\\Docs\\XYplorerTag.dat";

    #[gpui_kit::test]
    fn it_opens_on_glints_format_and_the_name_follows_the_format_until_the_person_chooses_one(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(Ok(1), false, Some("C:\\mine.jsonl"));
        let (mut visual, view) = opened(cx, &fake, "C:\\Docs");
        assert_eq!(to(&mut visual, &view), JSONL);
        click(&mut visual, ("marks-export-format", 1_usize));
        assert_eq!(to(&mut visual, &view), XY);
        click(&mut visual, ("marks-export-format", 0_usize));
        assert_eq!(to(&mut visual, &view), JSONL);
        click(&mut visual, "marks-export-choose");
        settle(&mut visual, |cx| to(cx, &view) == "C:\\mine.jsonl");
        click(&mut visual, ("marks-export-format", 1_usize));
        assert_eq!(to(&mut visual, &view), "C:\\mine.jsonl", "a name the person chose stays");
    }

    #[gpui_kit::test]
    fn a_file_chosen_beside_the_folder_is_not_taken_and_says_where_it_can_go(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(Ok(1), false, Some("D:\\mine\\x.jsonl"));
        let (mut visual, view) = opened(cx, &fake, "C:\\Docs");
        let said = "Keep the file in “Docs” or a folder above it: the paths in it are written from where it is";
        assert!(!has(&mut visual, said));
        click(&mut visual, "marks-export-choose");
        settle(&mut visual, |cx| has(cx, said));
        assert_eq!(role_of(&mut visual, said), Some(Role::Alert));
        assert_eq!(to(&mut visual, &view), JSONL, "the path stays the one in the folder");
    }

    #[gpui_kit::test]
    fn export_asks_once_and_the_answer_is_said_in_words(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        for (answer, said) in [
            (Ok(12), "Exported the marks of 12 items"),
            (Ok(1), "Exported the marks of 1 item"),
            (Ok(0), "Nothing below “Docs” is marked"),
            (Err("access denied".to_string()), "access denied"),
        ] {
            let fake = Fake::new(answer, false, None);
            let (mut visual, view) = opened(cx, &fake, "C:\\Docs");
            click(&mut visual, ("marks-export-format", 1_usize));
            click(&mut visual, "marks-export-start");
            settle(&mut visual, |cx| has(cx, said));
            let asked = fake.asked();
            assert_eq!(asked.len(), 1);
            let labels = visual.update(|_, cx| state::settings(cx).labels.clone());
            assert_eq!((asked[0].0.as_str(), asked[0].1.as_str(), asked[0].2), ("C:\\Docs", XY, Format::Xyplorer));
            assert_eq!(asked[0].3, labels);
            assert!(has(&mut visual, said));
            assert!(matches!(view.read_with(&visual, |v, _| v.run().clone()), Run::Done(_) | Run::Failed(_)));
            click(&mut visual, "marks-export-close");
            visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        }
    }

    #[gpui_kit::test]
    fn stop_sets_the_flag_the_backend_was_given(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(Ok(0), true, None);
        let (mut visual, _view) = opened(cx, &fake, "C:\\Docs");
        click(&mut visual, "marks-export-start");
        settle(&mut visual, |_| !fake.asked().is_empty());
        assert!(has(&mut visual, "Exporting…"));
        let flag = fake.asked()[0].4.clone();
        assert!(!flag.load(Ordering::Relaxed));
        click(&mut visual, "marks-export-stop");
        assert!(flag.load(Ordering::Relaxed));
    }

    #[gpui_kit::test]
    fn everything_pressable_has_a_role_and_a_name_and_enter_exports_and_escape_closes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(Ok(3), false, None);
        let (mut visual, _view) = opened(cx, &fake, "C:\\Docs");
        assert_eq!(role_of(&mut visual, "Export marks"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Format"), Some(Role::RadioGroup));
        assert_eq!(
            role_of(&mut visual, "Glint. Text, one line an item: read by Import marks… on any PC"),
            Some(Role::RadioButton)
        );
        assert_eq!(
            role_of(&mut visual, "XYplorer. An XYplorerTag.dat that XYplorer's tagload merges"),
            Some(Role::RadioButton)
        );
        assert_eq!(role_of(&mut visual, JSONL), Some(Role::Group));
        visual.update(|window, _| {
            for id in ["marks-export-choose", "marks-export-cancel", "marks-export-start"] {
                let _ = window.find(id);
            }
        });
        press(&mut visual, "enter");
        settle(&mut visual, |cx| has(cx, "Exported the marks of 3 items"));
        assert_eq!(fake.asked().len(), 1);
        press(&mut visual, "escape");
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn escape_closes_before_anything_is_asked_and_an_arabic_folder_mirrors(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(Ok(0), false, None);
        testing::set_rtl(true);
        let (mut visual, view) = opened(cx, &fake, "C:\\ملفات\\تقارير");
        assert_eq!(to(&mut visual, &view), "C:\\ملفات\\تقارير\\تقارير.glint-marks.jsonl");
        assert!(has(&mut visual, "Everything marked in “\u{2067}تقارير\u{2069}” and below it, into one file."));
        assert!(has(&mut visual, "Export marks"));
        press(&mut visual, "escape");
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        assert!(fake.asked().is_empty());
        testing::set_rtl(false);
    }
}
