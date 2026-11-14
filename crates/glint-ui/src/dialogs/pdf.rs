//! PDF tools: merge, split, keep or take out pages, turn pages, make smaller, save the pictures, add or take off a
//! password, stamp. Ported from src/components/explorer/PdfDialog.tsx, built the way [`super::drive_health`] is.
//!
//! | TypeScript                           | Here                                                              |
//! |--------------------------------------|-------------------------------------------------------------------|
//! | `PdfMode`, `MODES`                   | [`PdfMode`], [`modes`]                                            |
//! | `isPdfFile`                          | [`is_pdf`] (by name; the menus know the rest)                     |
//! | `openPdfTools(entries, mode)`        | [`open`]                                                          |
//! | `validPages`                         | `glint_core::pdf_tools::valid_pages`, through [`pages_bad`]       |
//! | `PdfDialog` / `PdfModal`             | [`PdfDialog`]                                                     |
//! | `pdf_run`, `pdf_info`, `pdf_cancel`  | [`Backend::run`], [`Backend::info`], [`Backend::cancel`] ([`CoreBackend`] is glint-core) |
//! | `useActionRun`, `WhereChooser`, `RunProgress`, `RunResult`, `RunFooter` | [`action_run`]                   |
//! | `ToolNeeded id="pdfcpu"`             | `settings::tools::needed`, through [`action_run::Tooling`]        |
//!
//! **How the coordinator wires it.** `MenuEvent::PdfTools { paths, tool }` is
//! `dialogs::pdf::open(window, cx, paths, tool.into())`: [`PdfMode`] converts from the menus' `PdfTool` (their
//! "Keep or take out pages…" is `Keep`; Take out pages is a choice inside the dialog). It keeps the `.pdf` files
//! among `paths` and says so in a notification when none is left. Merge with fewer than two PDFs opens at Split.
//!
//! Where the TypeScript had a slider for the stamp's strength and the number of pages in a part as a number box,
//! this has a box with minus and plus (see [`action_run::NumberField`]). Enter in the dialog doesn't start the run.

use super::action_run;

use crate::components::menus::PdfTool;
use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use action_run::{
    choices, error_view, field, footer, help, on_core, progress_text, progress_view, result_view, shell, whole,
    ActionRun, Choice, FooterSpec, Fut, NumberField, ResultWords, Run, Tooling, WhereChooser, WhereKind,
};
use glint_core::actions::{Output, Progress, Report};
use glint_core::pdf_tools::{PdfInfo, PdfTask};
use glint_model::format::{format_count, plural};
use glint_model::paths::base_name;
use gpui_kit::UniformListScrollHandle;
use std::ops::Range;
use std::sync::Arc;

/// What to do with the PDFs (`PdfMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PdfMode {
    Merge,
    Split,
    Keep,
    Remove,
    Rotate,
    Optimize,
    Images,
    Encrypt,
    Decrypt,
    Stamp,
}

impl From<PdfTool> for PdfMode {
    fn from(tool: PdfTool) -> Self {
        match tool {
            PdfTool::Merge => PdfMode::Merge,
            PdfTool::Split => PdfMode::Split,
            PdfTool::Keep => PdfMode::Keep,
            PdfTool::Rotate => PdfMode::Rotate,
            PdfTool::Optimize => PdfMode::Optimize,
            PdfTool::Images => PdfMode::Images,
            PdfTool::Encrypt => PdfMode::Encrypt,
            PdfTool::Decrypt => PdfMode::Decrypt,
            PdfTool::Stamp => PdfMode::Stamp,
        }
    }
}

/// One mode as the dialog lists it (`MODES`).
#[derive(Clone, Debug)]
pub struct ModeInfo {
    pub mode: PdfMode,
    pub label: SharedString,
    pub icon: IconName,
    pub hint: SharedString,
}

/// The modes, in the TypeScript's order.
pub fn modes() -> Vec<ModeInfo> {
    let one = |mode, label: SharedString, icon, hint: SharedString| ModeInfo {
        mode,
        label,
        icon,
        hint,
    };
    vec![
        one(
            PdfMode::Merge,
            t!("Merge"),
            IconName::FileStack,
            t!("The PDFs as one, in the order below"),
        ),
        one(
            PdfMode::Split,
            t!("Split"),
            IconName::Scissors,
            t!("Into parts of so many pages"),
        ),
        one(
            PdfMode::Keep,
            t!("Keep pages"),
            IconName::FileText,
            t!("A new PDF of only the pages listed"),
        ),
        one(
            PdfMode::Remove,
            t!("Take out pages"),
            IconName::X,
            t!("A new PDF without the pages listed"),
        ),
        one(
            PdfMode::Rotate,
            t!("Turn pages"),
            IconName::RotateCw,
            t!("Pages turned a quarter or half"),
        ),
        one(
            PdfMode::Optimize,
            t!("Make smaller"),
            IconName::Minimize2,
            t!("What's repeated or unused taken out"),
        ),
        one(
            PdfMode::Images,
            t!("Pictures"),
            IconName::Images,
            t!("Every picture in it, as files"),
        ),
        one(
            PdfMode::Encrypt,
            t!("Add a password"),
            IconName::KeyRound,
            t!("Needed to open it (AES-256)"),
        ),
        one(
            PdfMode::Decrypt,
            t!("Take the password off"),
            IconName::LockOpen,
            t!("With the password it has"),
        ),
        one(
            PdfMode::Stamp,
            t!("Stamp"),
            IconName::Stamp,
            t!("Text across every page: DRAFT, COPY…"),
        ),
    ]
}

/// Whether the file called `path` is a PDF (`isPdfFile` minus the entry's state: a file on disk, there to be read,
/// is for the menus to know).
pub fn is_pdf(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("pdf"))
}

/// Whether the page list can't be used (`pagesBad`): Keep and Take out need one; Turn takes none or one.
pub fn pages_bad(mode: PdfMode, pages: &str) -> bool {
    let needed = matches!(mode, PdfMode::Keep | PdfMode::Remove);
    let valid = glint_core::pdf_tools::valid_pages(pages);
    (needed && !valid) || (mode == PdfMode::Rotate && !pages.trim().is_empty() && !valid)
}

/// Whether the password can't be used (`passwordBad`): Add needs two that agree; Take off needs the one.
pub fn password_bad(mode: PdfMode, password: &str, again: &str) -> bool {
    (mode == PdfMode::Encrypt && (password.is_empty() || password != again))
        || (mode == PdfMode::Decrypt && password.is_empty())
}

/// Whether the merged PDF's name can't be used (`nameBad`).
pub fn name_bad(mode: PdfMode, name: &str) -> bool {
    mode == PdfMode::Merge && (name.trim().is_empty() || action_run::not_in_names(name))
}

/// Where a mode's results go by default (the TypeScript's effect on `mode`): parts and pictures are many files,
/// so into a folder named after the PDF; the rest beside each.
pub fn default_place(mode: PdfMode, files: &[String]) -> (WhereKind, Option<String>) {
    match mode {
        PdfMode::Split | PdfMode::Images => {
            let split = mode == PdfMode::Split;
            let name = match files {
                [one] => {
                    let name = base_name(one);
                    let stem = if name.to_lowercase().ends_with(".pdf") {
                        &name[..name.len() - 4]
                    } else {
                        name
                    };
                    if split {
                        t!("{name} parts", name = stem).to_string()
                    } else {
                        t!("{name} pictures", name = stem).to_string()
                    }
                }
                _ if split => t!("Parts").to_string(),
                _ => t!("Pictures").to_string(),
            };
            (WhereKind::Folder, Some(name))
        }
        _ => (WhereKind::Beside, None),
    }
}

// ---- The calls --------------------------------------------------------------------------------------------

/// The calls PDF tools makes. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// Runs `task` on `files` (`pdf_run`), reporting progress on `progress`.
    fn run(
        &self,
        files: Vec<String>,
        task: PdfTask,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report>;
    /// How many pages a PDF has (`pdf_info`).
    fn info(&self, path: String) -> Fut<PdfInfo>;
    /// Stops the run under way (`pdf_cancel`).
    fn cancel(&self);
}

/// glint-core's PDF tools, owned by the main window as the shell dialogs it opens are.
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
        task: PdfTask,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report> {
        match action_run::main_window(&self.app) {
            Ok(window) => on_core(glint_core::pdf_tools::pdf_run(window, files, task, output, progress)),
            Err(reason) => Box::pin(async move { Err(reason) }),
        }
    }

    fn info(&self, path: String) -> Fut<PdfInfo> {
        on_core(glint_core::pdf_tools::pdf_info(path))
    }

    fn cancel(&self) {
        glint_core::pdf_tools::pdf_cancel();
    }
}

/// Stands in when core isn't started (never in the app).
struct Unavailable;

impl Backend for Unavailable {
    fn run(&self, _: Vec<String>, _: PdfTask, _: Output, _: glint_core::host::ipc::Channel<Progress>) -> Fut<Report> {
        action_run::no_core()
    }

    fn info(&self, _: String) -> Fut<PdfInfo> {
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

// ---- The dialog -------------------------------------------------------------------------------------------

/// One row of the merge list is this tall.
const ROW: f32 = 2.25;

/// "name.pdf" or "3 PDFs": what the dialog is called after.
pub fn what(files: &[String]) -> SharedString {
    match files {
        [one] => isolate(base_name(one).to_string()),
        _ => t!("{n} PDFs", n = format_count(files.len())),
    }
}

/// PDF tools on some PDFs: what to do, its settings, where the results go, the run and its result.
pub struct PdfDialog {
    files: Vec<String>,
    backend: Arc<dyn Backend>,
    tooling: Tooling,
    mode: PdfMode,
    /// The order the PDFs merge in.
    order: Vec<String>,
    name: Entity<InputState>,
    every: Entity<NumberField>,
    pages: Entity<InputState>,
    degrees: u32,
    password: Entity<InputState>,
    again: Entity<InputState>,
    text: Entity<InputState>,
    strength: Entity<NumberField>,
    diagonal: bool,
    /// How many pages the one PDF has, once known.
    count: Option<u32>,
    info_asked: bool,
    where_: Entity<WhereChooser>,
    run: Entity<ActionRun>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl PdfDialog {
    /// A dialog for `files` at `mode`, calling `backend`, its install card reading `tooling`.
    pub fn new(
        files: Vec<String>,
        mode: PdfMode,
        backend: Arc<dyn Backend>,
        tooling: Tooling,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).default_value("Merged"));
        let pages = cx.new(|cx| InputState::new(window, cx).placeholder("1-3, 5, 8-"));
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let again = cx.new(|cx| InputState::new(window, cx).masked(true));
        let text = cx.new(|cx| InputState::new(window, cx).default_value("DRAFT"));
        let every = cx.new(|cx| {
            NumberField::new(
                "pdf-every",
                t!("Pages in each part"),
                SharedString::default(),
                (1, 100_000, 1),
                1,
                window,
                cx,
            )
        });
        let strength =
            cx.new(|cx| NumberField::new("pdf-strength", t!("Strength"), "%".into(), (10, 100, 5), 30, window, cx));
        let where_ = cx.new(|cx| {
            WhereChooser::new(
                "pdf-where",
                t!("PDF"),
                t!("A name that's taken gets “(2)”, and Ctrl+Z takes it all back."),
                true,
                ("", "Converted"),
                window,
                cx,
            )
        });
        let run = cx.new(|_| ActionRun::new());
        let mut subscriptions = vec![
            cx.observe(&every, |_, _, cx| cx.notify()),
            cx.observe(&strength, |_, _, cx| cx.notify()),
            cx.observe(&where_, |_, _, cx| cx.notify()),
            cx.observe(&run, |_, _, cx| cx.notify()),
        ];
        for input in [&name, &pages, &password, &again, &text] {
            subscriptions.push(cx.subscribe(input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        if let Some(tools) = tooling.model() {
            // The page count is asked for once pdfcpu is there, whenever that becomes true.
            subscriptions.push(cx.observe(tools, |this, _, cx| {
                this.load_info(cx);
                cx.notify();
            }));
        }
        let mut this = Self {
            order: files.clone(),
            files,
            backend,
            tooling,
            mode,
            name,
            every,
            pages,
            degrees: 90,
            password,
            again,
            text,
            strength,
            diagonal: true,
            count: None,
            info_asked: false,
            where_,
            run,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        this.apply_mode(window, cx);
        this.load_info(cx);
        this
    }

    /// Asks how many pages the one PDF has, once, when pdfcpu is there (`pdf_info`).
    fn load_info(&mut self, cx: &mut Context<Self>) {
        if self.files.len() != 1 || self.info_asked || !self.tooling.ready("pdfcpu", cx) {
            return;
        }
        self.info_asked = true;
        let asked = self.backend.info(self.files[0].clone());
        cx.spawn(async move |this, cx| {
            let pages = asked.await.map(|info| info.pages).unwrap_or(0);
            let _ = this.update(cx, |this, cx| {
                this.count = (pages > 0).then_some(pages);
                cx.notify();
            });
        })
        .detach();
    }

    /// The mode changed: where results go by default, whether "In their place" is offered, and what the pages
    /// box asks for.
    fn apply_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (kind, folder) = default_place(self.mode, &self.files);
        let replace = !matches!(self.mode, PdfMode::Merge | PdfMode::Split | PdfMode::Images);
        self.where_.update(cx, |chooser, cx| {
            chooser.offer_replace(replace, cx);
            chooser.set(kind, Some(""), folder.as_deref(), window, cx);
        });
        let placeholder = if self.mode == PdfMode::Rotate {
            t!("All of them")
        } else {
            "1-3, 5, 8-".into()
        };
        self.pages
            .update(cx, |input, cx| input.set_placeholder(placeholder, window, cx));
        cx.notify();
    }

    /// The PDFs being worked on.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// What it does now.
    pub fn mode(&self) -> PdfMode {
        self.mode
    }

    /// The order the PDFs would merge in.
    pub fn order(&self) -> &[String] {
        &self.order
    }

    /// How many pages the one PDF has, once known.
    pub fn count(&self) -> Option<u32> {
        self.count
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

    /// The pages box.
    #[cfg(test)]
    pub fn pages_input(&self) -> &Entity<InputState> {
        &self.pages
    }

    /// The merged PDF's name box.
    #[cfg(test)]
    pub fn name_input(&self) -> &Entity<InputState> {
        &self.name
    }

    /// The first password box.
    pub fn password_input(&self) -> &Entity<InputState> {
        &self.password
    }

    /// The second password box.
    #[cfg(test)]
    pub fn again_input(&self) -> &Entity<InputState> {
        &self.again
    }

    /// The task for what is chosen (`task`).
    pub fn task(&self, cx: &App) -> PdfTask {
        let pages = self.pages.read(cx).value().to_string();
        let password = self.password.read(cx).value().to_string();
        match self.mode {
            PdfMode::Merge => PdfTask::Merge {
                name: self.name.read(cx).value().trim().to_string(),
            },
            PdfMode::Split => PdfTask::Split {
                every: self.every.read(cx).value(cx).clamp(1, i64::from(u32::MAX)) as u32,
            },
            PdfMode::Keep => PdfTask::Keep { pages },
            PdfMode::Remove => PdfTask::Remove { pages },
            PdfMode::Rotate => PdfTask::Rotate {
                pages,
                degrees: self.degrees,
            },
            PdfMode::Optimize => PdfTask::Optimize,
            PdfMode::Images => PdfTask::Images,
            PdfMode::Encrypt => PdfTask::Encrypt { password },
            PdfMode::Decrypt => PdfTask::Decrypt { password },
            PdfMode::Stamp => PdfTask::Stamp {
                text: self.text.read(cx).value().to_string(),
                opacity: self.strength.read(cx).value(cx) as f32 / 100.,
                diagonal: self.diagonal,
            },
        }
    }

    /// Whether the settings of the mode are complete.
    fn settings_bad(&self, cx: &App) -> bool {
        pages_bad(self.mode, &self.pages.read(cx).value())
            || password_bad(self.mode, &self.password.read(cx).value(), &self.again.read(cx).value())
            || name_bad(self.mode, &self.name.read(cx).value())
            || (self.mode == PdfMode::Stamp && self.text.read(cx).value().trim().is_empty())
    }

    /// Whether the action can be pressed: pdfcpu is there and the settings and the names are usable.
    pub fn can_start(&self, cx: &App) -> bool {
        !self.run.read(cx).state().started()
            && self.tooling.ready("pdfcpu", cx)
            && !self.where_.read(cx).choice(cx).bad()
            && !self.settings_bad(cx)
    }

    /// Starts the run on core's runtime; progress and the result come back here.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if !self.can_start(cx) {
            return;
        }
        let merging = self.mode == PdfMode::Merge;
        let list = if merging {
            self.order.clone()
        } else {
            self.files.clone()
        };
        let task = self.task(cx);
        let mut output = self.where_.read(cx).choice(cx);
        // Merging one PDF out of several can't replace them: it goes beside the first.
        if merging && output == action_run::Where::Replace {
            output = action_run::Where::Beside { suffix: String::new() };
        }
        let (channel, receiver) = core_bridge::stream::<Progress>();
        let call = self.backend.run(list.clone(), task, output.output(), channel);
        let total = if merging { 1 } else { list.len() };
        let label = modes()
            .into_iter()
            .find(|m| m.mode == self.mode)
            .map(|m| m.label.to_lowercase())
            .unwrap_or_default();
        self.run.update(cx, |run, cx| {
            run.start(
                total,
                call,
                receiver,
                move |report| format!("{label} ({})", plural(report.made.len(), "file")),
                cx,
            )
        });
        cx.notify();
    }

    fn stop(&mut self) {
        self.backend.cancel();
    }

    /// Moves a PDF in the merge list by `by` places (`move`).
    pub fn move_by(&mut self, ix: usize, by: isize, cx: &mut Context<Self>) {
        if ix >= self.order.len() {
            return;
        }
        let item = self.order.remove(ix);
        let to = (ix as isize + by).clamp(0, self.order.len() as isize) as usize;
        self.order.insert(to, item);
        cx.notify();
    }

    /// One row of the merge list: its place, its name, and Earlier and Later.
    fn order_row(&self, ix: usize, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let Some(path) = self.order.get(ix).cloned() else {
            return div().into_any_element();
        };
        let last = self.order.len() - 1;
        let name: SharedString = base_name(&path).to_string().into();
        let shown = path.clone();
        align_row(2.)
            .id(("pdf-order-row", ix))
            .role(Role::ListItem)
            .aria_label(name.clone())
            .test_support()
            .h(rems(ROW))
            .px(space(1.5))
            .rounded(theme::size::RADIUS_SM)
            .hover(|style| style.bg(t.accent.opacity(0.5)))
            .child(
                div()
                    .w(space(5.))
                    .flex_none()
                    .text_end()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child((ix + 1).to_string()),
            )
            .child(
                div()
                    .id(("pdf-order-name", ix))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .tooltip(move |window, cx| crate::kit::tooltip(shown.clone().into(), window, cx))
                    .child(isolate(name)),
            )
            .child(
                Button::new(("pdf-earlier", ix))
                    .ghost()
                    .compact()
                    .icon(IconName::ArrowUp)
                    .accessibility_label(t!("Earlier"))
                    .tooltip(t!("Earlier"))
                    .disabled(ix == 0)
                    .on_click(cx.listener(move |this, _, _, cx| this.move_by(ix, -1, cx))),
            )
            .child(
                Button::new(("pdf-later", ix))
                    .ghost()
                    .compact()
                    .icon(IconName::ArrowDown)
                    .accessibility_label(t!("Later"))
                    .tooltip(t!("Later"))
                    .disabled(ix == last)
                    .on_click(cx.listener(move |this, _, _, cx| this.move_by(ix, 1, cx))),
            )
            .into_any_element()
    }

    /// The row of modes: an icon over a name each.
    fn mode_grid(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("pdf-mode")
            .role(Role::RadioGroup)
            .aria_label(t!("What to do"))
            .test_support()
            .grid()
            .grid_cols(5)
            .gap(space(1.5))
            .children(modes().into_iter().enumerate().map(|(ix, info)| {
                let chosen = self.mode == info.mode;
                let blocked = info.mode == PdfMode::Merge && self.files.len() < 2;
                let picked = info.mode;
                Button::new(("pdf-mode", ix))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(if chosen {
                                t.primary.opacity(0.1)
                            } else {
                                t.background.opacity(0.)
                            })
                            .foreground(t.foreground)
                            .hover(if chosen { t.primary.opacity(0.15) } else { t.accent })
                            .active(t.accent),
                    )
                    .min_w_0()
                    .h_auto()
                    .border_1()
                    .border_color(if chosen { t.primary } else { t.border })
                    .px(space(1.))
                    .py(space(2.))
                    .role(Role::RadioButton)
                    .toggled(chosen)
                    .disabled(blocked)
                    .tooltip(info.hint.clone())
                    .accessibility_label(format!("{}. {}", info.label, info.hint))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.mode = picked;
                        this.apply_mode(window, cx);
                    }))
                    .child(
                        col()
                            .w_full()
                            .min_w_0()
                            .items_center()
                            .gap(space(1.))
                            .text_center()
                            .child(icon(info.icon).size_4())
                            .child(
                                div()
                                    .text_size(theme::size::XXS)
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(info.label),
                            ),
                    )
            }))
    }

    /// A text box in a labelled row, red when `bad`.
    fn text_row(
        label: SharedString,
        input: &Entity<InputState>,
        aria: SharedString,
        bad: bool,
        t: &theme::Tokens,
    ) -> Div {
        field(
            label,
            div()
                .when(bad, |d| {
                    d.border_1().border_color(t.destructive).rounded(theme::size::RADIUS)
                })
                .child(Input::new(input).aria_label(aria)),
            t,
        )
    }

    /// The settings of the mode, under the modes.
    fn settings(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut parts: Vec<AnyElement> = Vec::new();
        let pages_value = self.pages.read(cx).value();
        let pages_wrong = !pages_value.is_empty() && pages_bad(self.mode, &pages_value);
        match self.mode {
            PdfMode::Merge => {
                let count = self.order.len();
                let rows = self.order.clone();
                let list = uniform_list(
                    "pdf-order",
                    rows.len(),
                    cx.processor({
                        let theme = t.clone();
                        move |this, range: Range<usize>, _, cx| {
                            range.map(|ix| this.order_row(ix, &theme, cx)).collect::<Vec<_>>()
                        }
                    }),
                )
                .track_scroll(&self.scroll)
                .size_full();
                parts.push(
                    div()
                        .relative()
                        .h(rems(ROW * count.clamp(2, 8) as f32 + 0.75))
                        .rounded(theme::size::RADIUS)
                        .border_1()
                        .border_color(t.border)
                        .p(space(1.5))
                        .child(
                            div()
                                .id("pdf-order-list")
                                .role(Role::List)
                                .aria_label(t!("The PDFs, in the order they merge"))
                                .test_support()
                                .size_full()
                                .child(list),
                        )
                        .child(Scrollbar::vertical(&self.scroll).viewport_from_layout())
                        .into_any_element(),
                );
                let bad = name_bad(self.mode, &self.name.read(cx).value());
                parts.push(
                    align_row(2.)
                        .child(div().flex_1().min_w_0().child(Self::text_row(
                            t!("Named"),
                            &self.name,
                            t!("Name"),
                            bad,
                            t,
                        )))
                        .child(div().text_color(t.muted_foreground).child(".pdf"))
                        .into_any_element(),
                );
            }
            PdfMode::Split => {
                let every = self.every.read(cx).value(cx);
                parts.push(
                    field(
                        t!("Each part"),
                        align_row(2.)
                            .child(self.every.clone())
                            .child(div().text_color(t.muted_foreground).child(if every == 1 {
                                t!("page: a PDF a page")
                            } else {
                                t!("pages")
                            })),
                        t,
                    )
                    .into_any_element(),
                );
            }
            PdfMode::Keep | PdfMode::Remove | PdfMode::Rotate => {
                parts.push(Self::text_row(t!("Pages"), &self.pages, t!("Pages"), pages_wrong, t).into_any_element());
                parts.push(
                    help(
                        t!("Numbers and ranges with commas: 1-3, 5, 8- (to the end); even, odd, l (the last page)."),
                        t,
                    )
                    .ps(23.)
                    .into_any_element(),
                );
                if self.mode == PdfMode::Rotate {
                    parts.push(
                        field(
                            t!("Turn"),
                            choices::<Self, u32>(
                                "pdf-turn",
                                t!("Turn"),
                                self.degrees,
                                vec![
                                    Choice::new(90, t!("A quarter clockwise")),
                                    Choice::new(180, t!("Upside down")),
                                    Choice::new(270, t!("A quarter anticlockwise")),
                                ],
                                3,
                                |this, picked, _, _| this.degrees = picked,
                                t,
                                cx,
                            ),
                            t,
                        )
                        .items_start()
                        .into_any_element(),
                    );
                }
            }
            PdfMode::Encrypt | PdfMode::Decrypt => {
                let again_wrong = !self.again.read(cx).value().is_empty()
                    && self.again.read(cx).value() != self.password.read(cx).value();
                parts.push(
                    col()
                        .gap(space(2.))
                        .child(Self::text_row(t!("Password"), &self.password, t!("Password"), false, t))
                        .when(self.mode == PdfMode::Encrypt, |this| {
                            this.child(Self::text_row(
                                t!("Again"),
                                &self.again,
                                t!("The password again"),
                                again_wrong,
                                t,
                            ))
                            .child(
                                help(
                                    t!("Nobody can open it without this password, Glint included: keep it somewhere safe."),
                                    t,
                                )
                                .ps(23.),
                            )
                        })
                        .into_any_element(),
                );
            }
            PdfMode::Stamp => {
                parts.push(
                    col()
                        .gap(space(2.))
                        .child(Self::text_row(t!("Text"), &self.text, t!("Stamp text"), false, t))
                        .child(field(t!("Strength"), self.strength.clone(), t))
                        .child(
                            div().ps(23.).child(
                                Checkbox::new("pdf-diagonal")
                                    .label(t!("Across, corner to corner"))
                                    .checked(self.diagonal)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.diagonal = *checked;
                                        cx.notify();
                                    })),
                            ),
                        )
                        .into_any_element(),
                );
            }
            PdfMode::Optimize | PdfMode::Images => {}
        }
        parts
    }
}

impl Render for PdfDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let state = self.run.read(cx).state().clone();
        let current = modes().into_iter().find(|m| m.mode == self.mode);

        let body = match state.clone() {
            Run::Setup => {
                let mut parts: Vec<AnyElement> = Vec::new();
                parts.extend(self.tooling.card(
                    "pdfcpu",
                    t!("pdfcpu merges, splits, shrinks and locks PDFs on this PC: nothing is uploaded anywhere."),
                    IconName::FileText,
                    cx,
                ));
                parts.push(self.mode_grid(&t, cx).into_any_element());
                if let Some(current) = &current {
                    parts.push(
                        help(format!("{}.", current.hint), &t)
                            .id("pdf-hint")
                            .role(Role::Status)
                            .aria_label(format!("{}.", current.hint))
                            .test_support()
                            .into_any_element(),
                    );
                }
                parts.extend(self.settings(&t, cx));
                parts.push(self.where_.clone().into_any_element());
                col().gap(space(4.)).children(parts).into_any_element()
            }
            Run::Running(progress) => progress_view(
                "pdf-progress",
                progress_text(&t!("Working on"), &progress, false, false),
                whole(&progress),
                &t,
            )
            .into_any_element(),
            Run::Done(report) => result_view(
                "pdf-result",
                &report,
                &ResultWords::files(self.mode == PdfMode::Optimize),
                &t,
            )
            .into_any_element(),
            Run::Failed(reason) => error_view("pdf-error", t!("Couldn't do it"), reason, &t).into_any_element(),
        };

        let spec = FooterSpec {
            prefix: "pdf",
            start_label: current.as_ref().map(|c| c.label.clone()).unwrap_or_default(),
            start_icon: current.as_ref().map_or(IconName::FileText, |c| c.icon),
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
        let mut subtitle = action_run::where_they_are(&self.files).to_string();
        if let Some(pages) = self.count {
            subtitle.push_str(&format!(" · {}", plural(pages as usize, "page")));
        }
        shell(
            "pdf",
            t!("PDF tools: {what}", what = what(&self.files)),
            subtitle.into(),
            body,
            footer,
            window,
            &t,
        )
    }
}

/// Opens PDF tools on the PDFs among `paths` in `window`, at `mode` (`openPdfTools`). When none is left it says so
/// in a notification instead.
pub fn open(window: &mut Window, cx: &mut App, paths: Vec<String>, mode: PdfMode) {
    let files: Vec<String> = paths.into_iter().filter(|p| is_pdf(p)).collect();
    if files.is_empty() {
        window.push_notification(Notification::info(t!("No PDFs chosen")), cx);
        return;
    }
    let backend = core_backend(cx);
    let tooling = Tooling::new(action_run::tools_model(window, cx));
    open_with(window, cx, backend, tooling, files, mode);
}

/// [`open`] for a given backend and tools, without filtering `files` (tests pass fakes). Merge with fewer than two
/// PDFs opens at Split. Returns the dialog.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    tooling: Tooling,
    files: Vec<String>,
    mode: PdfMode,
) -> Entity<PdfDialog> {
    let mode = if mode == PdfMode::Merge && files.len() < 2 {
        PdfMode::Split
    } else {
        mode
    };
    let shown = t!("PDF tools: {what}", what = what(&files));
    let view = cx.new(|cx| PdfDialog::new(files, mode, backend, tooling, window, cx));
    super::show(window, cx, shown, px(672.), view.clone());
    view
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
        runs: Mutex<Vec<(Vec<String>, PdfTask, Output)>>,
        pages: u32,
        report: Report,
        error: Option<String>,
        cancelled: Mutex<usize>,
    }

    impl Fake {
        fn new(report: Report) -> Arc<Fake> {
            Arc::new(Fake {
                runs: Mutex::new(Vec::new()),
                pages: 12,
                report,
                error: None,
                cancelled: Mutex::new(0),
            })
        }

        fn runs(&self) -> Vec<(Vec<String>, PdfTask, Output)> {
            self.runs.lock().map(|r| r.clone()).unwrap_or_default()
        }
    }

    impl Backend for Fake {
        fn run(
            &self,
            files: Vec<String>,
            task: PdfTask,
            output: Output,
            progress: glint_core::host::ipc::Channel<Progress>,
        ) -> Fut<Report> {
            if let Ok(mut runs) = self.runs.lock() {
                runs.push((files, task, output));
            }
            let _ = progress.send(Progress {
                done: 0,
                total: 1,
                current: "a.pdf".into(),
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

        fn info(&self, _: String) -> Fut<PdfInfo> {
            let pages = self.pages;
            Box::pin(async move {
                Ok(PdfInfo {
                    pages,
                    encrypted: false,
                    version: "1.7".into(),
                })
            })
        }

        fn cancel(&self) {
            if let Ok(mut n) = self.cancelled.lock() {
                *n += 1;
            }
        }
    }

    fn report(made: &[&str]) -> Report {
        Report {
            made: made.iter().map(|s| (*s).to_string()).collect(),
            before: 2_000_000,
            after: 1_000_000,
            elapsed_ms: 800.,
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
        panic!("the PDF dialog never reached the state the test waits for");
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
        files: &[&str],
        mode: PdfMode,
        fake: &Arc<Fake>,
        tools: &[&'static str],
    ) -> Entity<PdfDialog> {
        let backend: Arc<dyn Backend> = fake.clone();
        let files = strings(files);
        let tooling = Tooling::with(tools);
        let view = cx.update(|window, cx| open_with(window, cx, backend, tooling, files, mode));
        cx.run_until_parked();
        view
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::FileStack,
            IconName::Scissors,
            IconName::FileText,
            IconName::X,
            IconName::RotateCw,
            IconName::Minimize2,
            IconName::Images,
            IconName::KeyRound,
            IconName::LockOpen,
            IconName::Stamp,
            IconName::ArrowUp,
            IconName::ArrowDown,
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
    fn what_each_mode_needs_reads_as_the_typescript_did() {
        assert!(
            is_pdf("C:\\p\\a.PDF") && !is_pdf("C:\\p\\a.pdfx") && !is_pdf("C:\\p\\.pdf") && !is_pdf("C:\\p\\Folder")
        );
        // Keep and Take out need a page list; Turn takes none or a good one.
        assert!(
            pages_bad(PdfMode::Keep, "") && pages_bad(PdfMode::Remove, "x") && !pages_bad(PdfMode::Keep, "1-3, 5, 8-")
        );
        assert!(
            !pages_bad(PdfMode::Rotate, "  ")
                && pages_bad(PdfMode::Rotate, "2-3-4")
                && !pages_bad(PdfMode::Rotate, "even")
        );
        assert!(!pages_bad(PdfMode::Optimize, "nonsense"));
        // Add needs two that agree; Take off needs one.
        assert!(password_bad(PdfMode::Encrypt, "", "") && password_bad(PdfMode::Encrypt, "a", "b"));
        assert!(!password_bad(PdfMode::Encrypt, "a", "a") && password_bad(PdfMode::Decrypt, "", ""));
        assert!(!password_bad(PdfMode::Decrypt, "a", "") && !password_bad(PdfMode::Stamp, "", ""));
        assert!(name_bad(PdfMode::Merge, " ") && name_bad(PdfMode::Merge, "a/b") && name_bad(PdfMode::Merge, "a."));
        assert!(!name_bad(PdfMode::Merge, "Merged") && !name_bad(PdfMode::Split, "a/b"));
        let _guard = testing::lock();
        assert_eq!(modes().len(), 10);
        assert_eq!(PdfMode::from(PdfTool::Keep), PdfMode::Keep);
        assert_eq!(PdfMode::from(PdfTool::Stamp), PdfMode::Stamp);
        // Parts and pictures go into a folder named after the PDF; the rest beside each.
        assert_eq!(
            default_place(PdfMode::Split, &strings(&["C:\\p\\Annual Report.PDF"])),
            (WhereKind::Folder, Some("Annual Report parts".to_string()))
        );
        assert_eq!(
            default_place(PdfMode::Images, &strings(&["C:\\p\\a.pdf", "C:\\p\\b.pdf"])),
            (WhereKind::Folder, Some("Pictures".to_string()))
        );
        assert_eq!(
            default_place(PdfMode::Optimize, &strings(&["C:\\p\\a.pdf"])),
            (WhereKind::Beside, None)
        );
        assert_eq!(
            what(&strings(&["C:\\p\\a.pdf", "C:\\p\\b.pdf", "C:\\q\\c.pdf"])),
            "3 PDFs"
        );
    }

    #[gpui_kit::test]
    fn the_dialog_names_its_modes_knows_the_pages_and_runs_a_task(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(report(&["C:\\p\\Report (2).pdf"]));
        let view = open_view(
            &mut visual,
            &["C:\\p\\Report.pdf"],
            PdfMode::Optimize,
            &fake,
            &["pdfcpu"],
        );
        visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
        assert_eq!(role_of(&mut visual, "PDF tools: Report.pdf"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "What to do"), Some(Role::RadioGroup));
        assert_eq!(role_of(&mut visual, "Where"), Some(Role::RadioGroup));
        assert_eq!(
            role_of(&mut visual, "Split. Into parts of so many pages"),
            Some(Role::RadioButton)
        );
        // One PDF can't be merged with itself; the page count is read once pdfcpu is there.
        assert!(disabled(&mut visual, ("pdf-mode", 0_usize)));
        settle(&mut visual, |cx| view.read_with(cx, |v, _| v.count().is_some()));
        assert!(has(&mut visual, "C:\\p · 12 pages"));
        assert_eq!(
            role_of(&mut visual, "What's repeated or unused taken out."),
            Some(Role::Status)
        );
        assert!(!disabled(&mut visual, "pdf-start"));

        // Pages to keep: nothing listed is no task; a bad list is red; a good one goes to core.
        click(&mut visual, ("pdf-mode", 2_usize));
        assert_eq!(view.read_with(&visual, |v, _| v.mode()), PdfMode::Keep);
        assert!(disabled(&mut visual, "pdf-start"));
        let pages = view.read_with(&visual, |v, _| v.pages_input().clone());
        type_into(&mut visual, &pages, "2-x");
        assert!(disabled(&mut visual, "pdf-start"));
        type_into(&mut visual, &pages, "1-3, 5, 8-");
        assert!(!disabled(&mut visual, "pdf-start"));
        click(&mut visual, "pdf-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        let runs = fake.runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, ["C:\\p\\Report.pdf"]);
        assert_eq!(
            runs[0].1,
            PdfTask::Keep {
                pages: "1-3, 5, 8-".into()
            }
        );
        assert_eq!(runs[0].2, Output::Beside { suffix: String::new() });
        // Show and Close after it, nothing else.
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("pdf-show").is_some());
            assert!(window.try_find("pdf-start").is_none());
        });
        click(&mut visual, "pdf-close");
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn splitting_goes_into_a_folder_and_passwords_must_agree(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(report(&["C:\\p\\x.pdf"]));
        let view = open_view(&mut visual, &["C:\\p\\Book.pdf"], PdfMode::Split, &fake, &["pdfcpu"]);
        // Split puts the parts into "<name> parts" and doesn't offer replacing.
        let chooser = view.read_with(&visual, |v, _| v.where_chooser().clone());
        assert_eq!(chooser.read_with(&visual, |c, _| c.kind()), WhereKind::Folder);
        assert!(!chooser.read_with(&visual, |c, _| c.replace_offered()));
        assert_eq!(
            chooser.read_with(&visual, |c, cx| c.choice(cx)),
            action_run::Where::Folder {
                name: "Book parts".into()
            }
        );
        click(&mut visual, ("pdf-every", 1_usize));
        click(&mut visual, ("pdf-every", 1_usize));
        click(&mut visual, "pdf-start");
        settle(&mut visual, |_| !fake.runs().is_empty());
        let runs = fake.runs();
        assert_eq!(runs[0].1, PdfTask::Split { every: 3 });
        assert_eq!(
            runs[0].2,
            Output::Folder {
                name: "Book parts".into()
            }
        );
        visual.update(|window, cx| {
            window.press("escape", cx);
        });
        visual.run_until_parked();

        // Add a password: two that agree.
        let fake = Fake::new(report(&["C:\\p\\x.pdf"]));
        let view = open_view(&mut visual, &["C:\\p\\Book.pdf"], PdfMode::Encrypt, &fake, &["pdfcpu"]);
        assert!(disabled(&mut visual, "pdf-start"));
        let (first, second) = view.read_with(&visual, |v, _| (v.password_input().clone(), v.again_input().clone()));
        type_into(&mut visual, &first, "hunter2");
        type_into(&mut visual, &second, "hunter3");
        assert!(disabled(&mut visual, "pdf-start"));
        type_into(&mut visual, &second, "hunter2");
        assert!(!disabled(&mut visual, "pdf-start"));
        // "In their place" is offered for this one.
        let chooser = view.read_with(&visual, |v, _| v.where_chooser().clone());
        assert!(chooser.read_with(&visual, |c, _| c.replace_offered()));
        click(&mut visual, "pdf-start");
        settle(&mut visual, |_| !fake.runs().is_empty());
        assert_eq!(
            fake.runs()[0].1,
            PdfTask::Encrypt {
                password: "hunter2".into()
            }
        );
    }

    #[gpui_kit::test]
    fn merging_follows_the_order_the_list_is_put_in(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(report(&["C:\\p\\Merged.pdf"]));
        let view = open_view(
            &mut visual,
            &["C:\\p\\a.pdf", "C:\\p\\b.pdf", "C:\\p\\c.pdf"],
            PdfMode::Merge,
            &fake,
            &["pdfcpu"],
        );
        assert_eq!(
            role_of(&mut visual, "The PDFs, in the order they merge"),
            Some(Role::List)
        );
        assert_eq!(role_of(&mut visual, "a.pdf"), Some(Role::ListItem));
        assert!(disabled(&mut visual, ("pdf-earlier", 0_usize)));
        assert!(disabled(&mut visual, ("pdf-later", 2_usize)));
        // The first one goes last.
        click(&mut visual, ("pdf-later", 0_usize));
        click(&mut visual, ("pdf-later", 1_usize));
        assert_eq!(
            view.read_with(&visual, |v, _| v.order().to_vec()),
            ["C:\\p\\b.pdf", "C:\\p\\c.pdf", "C:\\p\\a.pdf"]
        );
        let name = view.read_with(&visual, |v, _| v.name_input().clone());
        type_into(&mut visual, &name, "a:b");
        assert!(disabled(&mut visual, "pdf-start"), "a colon can't be in a name");
        type_into(&mut visual, &name, "All three");
        click(&mut visual, "pdf-start");
        settle(&mut visual, |_| !fake.runs().is_empty());
        let runs = fake.runs();
        assert_eq!(runs[0].0, ["C:\\p\\b.pdf", "C:\\p\\c.pdf", "C:\\p\\a.pdf"]);
        assert_eq!(
            runs[0].1,
            PdfTask::Merge {
                name: "All three".into()
            }
        );
    }

    #[gpui_kit::test]
    fn pdfcpu_is_needed_and_progress_stop_and_failures_are_drawn(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(report(&[]));
        // pdfcpu isn't there: nothing starts.
        let _missing = open_view(&mut visual, &["C:\\p\\a.pdf"], PdfMode::Optimize, &fake, &[]);
        assert!(disabled(&mut visual, "pdf-start"));
        visual.update(|window, cx| {
            window.press("escape", cx);
        });
        visual.run_until_parked();

        let view = open_view(&mut visual, &["C:\\p\\ملف.pdf"], PdfMode::Optimize, &fake, &["pdfcpu"]);
        let run = view.read_with(&visual, |v, _| v.run().clone());
        testing::set_rtl(true);
        visual.update(|_, cx| {
            run.update(cx, |run, cx| {
                run.set(
                    Run::Running(Progress {
                        done: 0,
                        total: 1,
                        current: "ملف.pdf".into(),
                        part: 0.,
                    }),
                    cx,
                )
            })
        });
        assert_eq!(
            role_of(&mut visual, "Working on · ملف.pdf"),
            Some(Role::ProgressIndicator)
        );
        click(&mut visual, "pdf-stop");
        assert_eq!(fake.cancelled.lock().map(|n| *n).unwrap_or(0), 1);
        let mut failed = report(&["C:\\p\\ok.pdf"]);
        failed.failed = vec![("C:\\p\\ملف.pdf".into(), "It is locked".into())];
        visual.update(|_, cx| run.update(cx, |run, cx| run.set(Run::Done(failed), cx)));
        assert_eq!(role_of(&mut visual, "Made 1 file"), Some(Role::Status));
        assert_eq!(role_of(&mut visual, "ملف.pdf: It is locked"), Some(Role::ListItem));
        visual.update(|_, cx| run.update(cx, |run, cx| run.set(Run::Failed("pdfcpu failed".into()), cx)));
        assert_eq!(role_of(&mut visual, "Couldn't do it. pdfcpu failed"), Some(Role::Alert));
        testing::set_rtl(false);
        visual.update(|window, cx| open(window, cx, strings(&["C:\\p\\a.docx"]), PdfMode::Split));
        visual.run_until_parked();
    }
}
