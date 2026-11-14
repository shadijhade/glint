//! A file's metadata, read and cleaned (MetadataDialog.tsx).
//!
//! Ported from src/components/explorer/MetadataDialog.tsx. The run, its progress, result and buttons, the
//! "Where" boxes and the choices are `dialogs::action_run`'s (ActionParts.tsx).
//!
//! | TypeScript                               | Here                                                       |
//! |------------------------------------------|------------------------------------------------------------|
//! | `openMetadata(entries, mode)`            | [`open`] (the menus' `RemovePersonalInfo` and `ChangeDateTaken`) |
//! | `MetadataDialog` / `MetadataModal`       | [`MetadataDialog`]                                         |
//! | `canClean`, `canRedate`                  | [`can_clean`], [`can_redate`] (by name; the menus know the rest) |
//! | `exifDate`                               | [`exif_date`]                                              |
//! | `invoke("meta_read")`, `meta_run`, `meta_cancel`, `useTool("exiftool")` | [`Backend`] (a fake in the tests) |
//!
//! **How the coordinator wires it.** `MenuEvent::RemovePersonalInfo { paths }`: `dialogs::metadata::open(window,
//! cx, paths, Mode::Scrub)`. `MenuEvent::ChangeDateTaken { paths }`: `open(window, cx, paths, Mode::Date)`. Each
//! keeps the files its mode can change and says so in a notification when none is left, as the TypeScript's
//! toast did. The menus have already checked that the paths are on disk.
//!
//! NOTE(port): the date box is text ("2024-06-01 12:00"), checked as a real date and time: the kit has no
//! date-and-time picker, and the TypeScript's `datetime-local` input made the same kind of value.

use super::action_run::{self, Choice, Choices, FooterSpec, Launch, ResultWords, Tooling, Where, WhereChooser};
use super::action_run::{Fut, Run};
use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_core::actions::{Output, Progress, Report};
use glint_core::metadata::{MetaTask, Tag};
use glint_model::format::{format_count, plural};
use glint_model::paths::base_name;
use std::sync::Arc;

/// What ExifTool writes (metadata.rs's `EXIFTOOL_WRITES`).
const EXIFTOOL: &[&str] = &[
    "jpg", "jpeg", "jpe", "png", "tif", "tiff", "webp", "heic", "heif", "avif", "jxl", "gif", "dng", "cr2", "cr3",
    "nef", "arw", "orf", "rw2", "raf", "psd", "mp4", "m4v", "mov", "3gp", "m4a", "pdf",
];
/// Office's files, cleaned in Glint itself.
const OFFICE: &[&str] = &["docx", "docm", "xlsx", "xlsm", "pptx", "pptm"];
/// Photos and videos whose date taken can be set.
const DATED: &[&str] = &[
    "jpg", "jpeg", "jpe", "png", "tif", "tiff", "webp", "heic", "heif", "avif", "jxl", "dng", "cr2", "cr3", "nef",
    "arw", "orf", "rw2", "raf", "mp4", "m4v", "mov", "3gp",
];

/// The lower-case extension of a file's name, "" when it has none.
fn ext_of(path: &str) -> String {
    match base_name(path).rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_lowercase(),
        _ => String::new(),
    }
}

/// A file whose personal info Glint can take out (`canClean`, by its name).
pub fn can_clean(path: &str) -> bool {
    let ext = ext_of(path);
    EXIFTOOL.contains(&ext.as_str()) || OFFICE.contains(&ext.as_str())
}

/// A photo or video whose date taken can be set (`canRedate`, by its name).
pub fn can_redate(path: &str) -> bool {
    DATED.contains(&ext_of(path).as_str())
}

/// What the dialog does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Take out where, who and with what (`"scrub"`).
    Scrub,
    /// Set or move the date taken (`"date"`).
    Date,
}

/// How the date taken is changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateMode {
    /// Moved by hours and minutes (a camera clock that was off).
    Shift,
    /// Set to one date and time.
    Set,
}

// ---- Dates ----------------------------------------------------------------------------------------------------

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => 29,
        _ => 28,
    }
}

/// "2024-06-01 12:00" (or with a "T", or with seconds) as ExifTool's "2024:06:01 12:00:00"; `None` when it isn't
/// a real date and time (`exifDate`, which took the `datetime-local` input's text).
pub fn exif_date(text: &str) -> Option<String> {
    let text = text.trim();
    let (date, time) = text.split_once(['T', ' '])?;
    let number = |part: &str, len: usize| -> Option<u32> {
        (part.len() == len && part.bytes().all(|b| b.is_ascii_digit())).then(|| part.parse().ok())?
    };
    let mut d = date.split('-');
    let (year, month, day) = (number(d.next()?, 4)?, number(d.next()?, 2)?, number(d.next()?, 2)?);
    if d.next().is_some() || !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    let mut c = time.trim().split(':');
    let (hour, minute) = (number(c.next()?, 2)?, number(c.next()?, 2)?);
    let second = match c.next() {
        Some(second) => number(second, 2)?,
        None => 0,
    };
    if c.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(format!(
        "{year:04}:{month:02}:{day:02} {hour:02}:{minute:02}:{second:02}"
    ))
}

/// A whole number from a box: `Math.trunc(Number(text) || 0)`.
fn whole(text: &str) -> i64 {
    let n = text.trim().parse::<f64>().ok().filter(|n| n.is_finite()).unwrap_or(0.);
    n.trunc().clamp(-1e12, 1e12) as i64
}

// ---- What each tag tells --------------------------------------------------------------------------------------

/// What a detail tells about the file (`Tag["kind"]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagKind {
    /// Where it was taken.
    Place,
    /// Who made it.
    People,
    /// The camera, phone or app.
    Device,
    /// Its editing history.
    History,
    /// When.
    Dates,
    /// Everything else.
    Other,
}

impl TagKind {
    /// The groups in the order the TypeScript showed them (`KINDS`).
    pub const ALL: [TagKind; 6] = [
        TagKind::Place,
        TagKind::People,
        TagKind::Device,
        TagKind::History,
        TagKind::Dates,
        TagKind::Other,
    ];

    /// The kind core's word names.
    pub fn parse(word: &str) -> TagKind {
        match word {
            "place" => TagKind::Place,
            "people" => TagKind::People,
            "device" => TagKind::Device,
            "history" => TagKind::History,
            "dates" => TagKind::Dates,
            _ => TagKind::Other,
        }
    }

    fn label(self) -> SharedString {
        match self {
            TagKind::Place => t!("Where it was"),
            TagKind::People => t!("Who made it"),
            TagKind::Device => t!("The camera, phone or app"),
            TagKind::History => t!("Its editing history"),
            TagKind::Dates => t!("When"),
            TagKind::Other => t!("Everything else"),
        }
    }

    /// Open to begin with: the groups that say where, who and with what.
    fn open_at_first(self) -> bool {
        matches!(self, TagKind::Place | TagKind::People | TagKind::Device)
    }

    /// Whether it says something about someone (`personal`: not "other" and not "dates").
    fn personal(self) -> bool {
        !matches!(self, TagKind::Other | TagKind::Dates)
    }
}

/// How many rows of a group are listed (`slice(0, 40)`).
const LISTED: usize = 40;

// ---- What the dialog asks of the PC ---------------------------------------------------------------------------

/// The calls into glint-core's metadata. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// What a file carries (`meta_read`).
    fn read(&self, path: String) -> Fut<Vec<Tag>>;
    /// Runs `task` on each of `files` as `output` says, reporting progress on `progress` (`meta_run`).
    fn run(
        &self,
        files: Vec<String>,
        task: MetaTask,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report>;
    /// Stops the run under way (`meta_cancel`).
    fn cancel(&self);
    /// What the install card asks about tools (none: every tool counts as there).
    fn tools(&self) -> Option<Arc<dyn crate::components::settings::Backend>>;
}

/// glint-core's metadata, owned by the main window as the shell dialogs it opens are.
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
    fn read(&self, path: String) -> Fut<Vec<Tag>> {
        action_run::on_core(glint_core::metadata::meta_read(path))
    }

    fn run(
        &self,
        files: Vec<String>,
        task: MetaTask,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report> {
        let app = self.app.clone();
        action_run::on_core(async move {
            let window = action_run::main_window(&app)?;
            glint_core::metadata::meta_run(window, files, task, output, progress).await
        })
    }

    fn cancel(&self) {
        glint_core::metadata::meta_cancel();
    }

    fn tools(&self) -> Option<Arc<dyn crate::components::settings::Backend>> {
        Some(Arc::new(crate::components::settings::CoreBackend::new(
            self.app.clone(),
        )))
    }
}

/// Stands in when core isn't started (never in the app).
struct Unavailable;

impl Backend for Unavailable {
    fn read(&self, _: String) -> Fut<Vec<Tag>> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn run(&self, _: Vec<String>, _: MetaTask, _: Output, _: glint_core::host::ipc::Channel<Progress>) -> Fut<Report> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn cancel(&self) {}
    fn tools(&self) -> Option<Arc<dyn crate::components::settings::Backend>> {
        None
    }
}

// ---- The dialog -----------------------------------------------------------------------------------------------

/// "Remove personal info" or "Change when it was taken" for some files: what they carry, the choices, where the
/// results go, the run and its result.
pub struct MetadataDialog {
    files: Vec<String>,
    mode: Mode,
    /// What the one file carries, once read (`None` while reading; empty if it couldn't be).
    tags: Option<Vec<Tag>>,
    asked: bool,
    open: [bool; 6],
    date_mode: DateMode,
    when: Entity<InputState>,
    hours: Entity<InputState>,
    minutes: Entity<InputState>,
    place: Entity<WhereChooser>,
    run: Run,
    backend: Arc<dyn Backend>,
    gate: Tooling,
    needs_tool: bool,
    _subscriptions: Vec<Subscription>,
}

impl MetadataDialog {
    /// A dialog for `files` (all changeable in `mode`), calling `backend`.
    pub fn new(
        files: Vec<String>,
        mode: Mode,
        backend: Arc<dyn Backend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let when = cx.new(|cx| InputState::new(window, cx).placeholder("2024-06-01 12:00"));
        let hours = cx.new(|cx| InputState::new(window, cx).default_value("0"));
        let minutes = cx.new(|cx| InputState::new(window, cx).default_value("0"));
        let start = match mode {
            Mode::Scrub => Where::Beside {
                suffix: " (clean)".to_string(),
            },
            Mode::Date => Where::Replace,
        };
        let place = cx.new(|cx| WhereChooser::starting_at("meta-where", &start, window, cx));
        if mode == Mode::Scrub {
            place.update(cx, |place, cx| {
                place.set_note(
                    t!("A copy to share keeps the original as it is; in their place, Ctrl+Z puts the originals back."),
                    cx,
                )
            });
        }
        let gate = Tooling::from_backend(backend.tools(), window, cx);
        let mut subscriptions = vec![cx.observe(&place, |_, _, cx| cx.notify())];
        for input in [&when, &hours, &minutes] {
            subscriptions.push(cx.subscribe_in(input, window, |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        subscriptions.extend(gate.observe(cx, |this, cx| {
            this.look(cx);
            cx.notify();
        }));
        let needs_tool = files.iter().any(|f| !OFFICE.contains(&ext_of(f).as_str()));
        let mut this = Self {
            files,
            mode,
            tags: None,
            asked: false,
            open: TagKind::ALL.map(TagKind::open_at_first),
            date_mode: DateMode::Shift,
            when,
            hours,
            minutes,
            place,
            run: Run::Setup,
            backend,
            gate,
            needs_tool,
            _subscriptions: subscriptions,
        };
        this.look(cx);
        this
    }

    /// The files being changed.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// What the dialog does.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Where the run stands.
    #[cfg(test)]
    pub fn run_state(&self) -> &Run {
        &self.run
    }

    /// What the one file carries, once read.
    pub fn tags(&self) -> Option<&[Tag]> {
        self.tags.as_deref()
    }

    /// The boxes: the date and time to set, the hours and the minutes to move by.
    #[cfg(test)]
    pub fn date_inputs(&self) -> (&Entity<InputState>, &Entity<InputState>, &Entity<InputState>) {
        (&self.when, &self.hours, &self.minutes)
    }

    /// The chooser of where results go.
    pub fn place(&self) -> &Entity<WhereChooser> {
        &self.place
    }

    fn run_mut(&mut self) -> &mut Run {
        &mut self.run
    }

    /// Whether ExifTool, if one file needs it, is there (`ready`).
    pub fn ready(&self, cx: &App) -> bool {
        !self.needs_tool || self.gate.ready("exiftool", cx)
    }

    /// Reads the one file's details once the tool is there (the TypeScript's effect on `mode`, `files`, `ready`).
    fn look(&mut self, cx: &mut Context<Self>) {
        if self.asked || self.mode != Mode::Scrub || self.files.len() != 1 || !self.ready(cx) {
            return;
        }
        self.asked = true;
        let read = self.backend.read(self.files[0].clone());
        cx.spawn(async move |this, cx| {
            let read = read.await;
            let _ = this.update(cx, |this, cx| {
                this.tags = Some(read.unwrap_or_default());
                cx.notify();
            });
        })
        .detach();
    }

    /// Minutes to move by (`shift`).
    pub fn shift(&self, cx: &App) -> i64 {
        whole(&self.hours.read(cx).value()) * 60 + whole(&self.minutes.read(cx).value())
    }

    /// Whether the date choices aren't complete (`dateBad`).
    pub fn date_bad(&self, cx: &App) -> bool {
        self.mode == Mode::Date
            && match self.date_mode {
                DateMode::Set => exif_date(&self.when.read(cx).value()).is_none(),
                DateMode::Shift => self.shift(cx) == 0,
            }
    }

    /// What to ask core to do.
    pub fn task(&self, cx: &App) -> MetaTask {
        match (self.mode, self.date_mode) {
            (Mode::Scrub, _) => MetaTask::Scrub,
            (Mode::Date, DateMode::Set) => MetaTask::SetDate {
                when: exif_date(&self.when.read(cx).value()).unwrap_or_default(),
            },
            (Mode::Date, DateMode::Shift) => MetaTask::ShiftDate {
                minutes: self.shift(cx),
            },
        }
    }

    /// Whether the action can be pressed: the tool is there, the output usable and the date complete.
    pub fn can_start(&self, cx: &App) -> bool {
        matches!(self.run, Run::Setup) && self.ready(cx) && !self.place.read(cx).bad(cx) && !self.date_bad(cx)
    }

    /// Starts the run on core's runtime; progress and the result come back here (`start`).
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_start(cx) {
            return;
        }
        let task = self.task(cx);
        let output = self.place.read(cx).choice(cx).output();
        let (channel, receiver) = core_bridge::stream::<Progress>();
        let call = self.backend.run(self.files.clone(), task, output, channel);
        let scrub = self.mode == Mode::Scrub;
        action_run::drive(
            self,
            window,
            cx,
            Launch {
                total: self.files.len(),
                call,
                progress: receiver,
            },
            Self::run_mut,
            move |report| {
                let what = plural(report.made.len(), "file");
                if scrub {
                    t!("cleaning of {what}", what = what).to_string()
                } else {
                    t!("date change of {what}", what = what).to_string()
                }
            },
        );
    }

    fn stop(&mut self) {
        self.backend.cancel();
    }
}

// ---- Drawing --------------------------------------------------------------------------------------------------

impl MetadataDialog {
    /// What the one file carries, in groups that open and close.
    fn tags_view(&self, tags: &[Tag], t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let personal = tags.iter().filter(|tag| TagKind::parse(tag.kind).personal()).count();
        let summary = if tags.is_empty() {
            t!("It carries no details to take out")
        } else {
            t!(
                "{what} in it, {n} about where, who or with what",
                what = plural(tags.len(), "detail"),
                n = personal
            )
        };
        let groups = TagKind::ALL
            .into_iter()
            .enumerate()
            .map(|(ix, kind)| {
                (
                    ix,
                    kind,
                    tags.iter()
                        .filter(|tag| TagKind::parse(tag.kind) == kind)
                        .collect::<Vec<_>>(),
                )
            })
            .filter(|(_, _, list)| !list.is_empty())
            .map(|(ix, kind, list)| {
                let open = self.open[ix];
                let title = t!("{kind} ({n})", kind = kind.label(), n = list.len());
                col()
                    .gap(space(1.))
                    .child(
                        Button::new(("meta-group", ix))
                            .ghost()
                            .compact()
                            .role(Role::Button)
                            .toggled(open)
                            .icon(if open { IconName::ChevronDown } else { forward_chevron() })
                            .label(title)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open[ix] = !this.open[ix];
                                cx.notify();
                            })),
                    )
                    .when(open, |this| {
                        this.child(
                            col()
                                .id(("meta-tags", ix))
                                .role(Role::List)
                                .aria_label(kind.label())
                                .test_support()
                                .children(list.iter().take(LISTED).enumerate().map(|(row, tag)| {
                                    align_row(2.)
                                        .id((SharedString::from(format!("meta-tag-{ix}")), row))
                                        .role(Role::ListItem)
                                        .aria_label(format!("{}: {}", tag.name, tag.value))
                                        .test_support()
                                        .py(space(0.5))
                                        .text_size(theme::size::XS)
                                        .child(
                                            div()
                                                .w(space(40.))
                                                .flex_none()
                                                .truncate()
                                                .text_color(t.muted_foreground)
                                                .child(tag.name.clone()),
                                        )
                                        .child(div().flex_1().min_w_0().truncate().child(isolate(tag.value.clone())))
                                })),
                        )
                    })
            })
            .collect::<Vec<_>>();
        col()
            .id("meta-found")
            .role(Role::Group)
            .aria_label(summary.clone())
            .test_support()
            .gap(space(2.))
            .p(space(2.))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.border)
            .child(
                align_row(1.5)
                    .text_size(theme::size::XS)
                    .font_weight(FontWeight::MEDIUM)
                    .child(icon(IconName::MapPin).size_4().flex_none().text_color(t.primary))
                    .child(div().flex_1().min_w_0().child(summary)),
            )
            .child(
                div()
                    .id("meta-found-list")
                    .max_h(space(56.))
                    .overflow_y_scrollbar()
                    .child(col().gap(space(2.)).children(groups)),
            )
    }

    fn scrub_options(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        col()
            .gap(space(4.))
            .child(div().text_color(t.muted_foreground).child(t!(
                "Takes out where a photo was taken, the camera and its serial number, names, the apps used and editing history. Pictures keep their orientation and colors; nothing in what they show changes."
            )))
            .when_some(self.tags.as_ref().filter(|_| self.files.len() == 1), |this, tags| {
                this.child(self.tags_view(tags, t, cx))
            })
            .child(self.place.clone())
    }

    fn date_options(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let bad_when = self.date_mode == DateMode::Set
            && !self.when.read(cx).value().trim().is_empty()
            && exif_date(&self.when.read(cx).value()).is_none();
        col()
            .gap(space(4.))
            .child(
                Choices::new("meta-how", t!("How"), self.date_mode)
                    .options(vec![
                        Choice::new(DateMode::Shift, t!("Move it"))
                            .hint(t!("A camera clock that was off: every one moved the same")),
                        Choice::new(DateMode::Set, t!("Set it")).hint(t!("The same date and time for each")),
                    ])
                    .on_pick(cx.listener(|this, mode: &DateMode, _, cx| {
                        this.date_mode = *mode;
                        cx.notify();
                    })),
            )
            .child(match self.date_mode {
                DateMode::Set => action_run::field(
                    t!("Taken"),
                    col()
                        .gap(space(1.))
                        .child(
                            div()
                                .w(space(48.))
                                .child(Input::new(&self.when).small().aria_label(t!("Date taken"))),
                        )
                        .when(bad_when, |this| {
                            this.child(
                                div()
                                    .id("meta-when-bad")
                                    .role(Role::Alert)
                                    .aria_label(t!("Not a date and time"))
                                    .test_support()
                                    .text_size(theme::size::XS)
                                    .text_color(t.destructive)
                                    .child(t!("Not a date and time")),
                            )
                        }),
                    t,
                )
                .into_any_element(),
                DateMode::Shift => align_row(2.)
                    .child(
                        div()
                            .w(space(20.))
                            .flex_none()
                            .text_size(theme::size::XS)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.muted_foreground)
                            .child(t!("By")),
                    )
                    .child(
                        div()
                            .w(space(20.))
                            .child(Input::new(&self.hours).small().aria_label(t!("Hours"))),
                    )
                    .child(div().text_color(t.muted_foreground).child(t!("hours")))
                    .child(
                        div()
                            .w(space(20.))
                            .child(Input::new(&self.minutes).small().aria_label(t!("Minutes"))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(t.muted_foreground)
                            .child(t!("minutes (less than 0 for earlier)")),
                    )
                    .into_any_element(),
            })
            .child(self.place.clone())
    }
}

/// The arrow of a closed group: points at the reading direction's end.
fn forward_chevron() -> IconName {
    if crate::i18n::is_rtl() {
        IconName::ChevronLeft
    } else {
        IconName::ChevronRight
    }
}

/// The dialog's title: "Remove personal info: name" or "When it was taken: 3 files".
pub fn title(files: &[String], mode: Mode) -> SharedString {
    let what: SharedString = match files {
        [one] => base_name(one).to_string().into(),
        _ => t!("{n} files", n = format_count(files.len())),
    };
    match mode {
        Mode::Scrub => t!("Remove personal info: {what}", what = what),
        Mode::Date => t!("When it was taken: {what}", what = what),
    }
}

impl Render for MetadataDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let scrub = self.mode == Mode::Scrub;
        let body = match self.run.clone() {
            Run::Setup if scrub => self.scrub_options(&t, cx).into_any_element(),
            Run::Setup => self.date_options(&t, cx).into_any_element(),
            Run::Running(progress) => {
                let verb = if scrub { t!("Cleaning") } else { t!("Changing") };
                action_run::run_progress("meta-progress", &verb, &progress, &t).into_any_element()
            }
            Run::Done(report) => {
                action_run::result_view("meta-result", &report, &ResultWords::files(false), &t).into_any_element()
            }
            Run::Failed(reason) => {
                action_run::error_view("meta-error", t!("Couldn't change it"), reason, &t).into_any_element()
            }
        };
        let card = self.needs_tool.then(|| {
            self.gate.card(
                "exiftool",
                t!("ExifTool reads and takes out what photos, videos and PDFs say about where, who and with what. Nothing is sent anywhere."),
                IconName::ShieldCheck,
                cx,
            )
        });
        let footer = action_run::footer(
            FooterSpec {
                prefix: "meta",
                start_label: if scrub {
                    t!("Remove personal info")
                } else {
                    t!("Change the date")
                },
                start_icon: if scrub {
                    IconName::EyeOff
                } else {
                    IconName::CalendarClock
                },
                can_start: self.can_start(cx),
                stoppable: true,
                show: self.run.first_made(),
            },
            &self.run,
            Self::start,
            |this, _, _| this.stop(),
            &t,
            cx,
        );
        col()
            .id("meta")
            .role(Role::Group)
            .aria_label(title(&self.files, self.mode))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .child(action_run::folder_line("meta-folder", &self.files, &t))
            .child(
                div()
                    .id("meta-body")
                    .max_h(action_run::body_height(window))
                    .overflow_y_scrollbar()
                    .child(col().gap(space(4.)).children(card.flatten()).child(body)),
            )
            .child(footer)
    }
}

// ---- Opening it -----------------------------------------------------------------------------------------------

/// Opens Remove personal info (or the date taken) on the files among `paths` it can change (`openMetadata`). When
/// none is left it says so in a notification instead.
pub fn open(window: &mut Window, cx: &mut App, paths: Vec<String>, mode: Mode) {
    let files: Vec<String> = paths
        .into_iter()
        .filter(|p| {
            if mode == Mode::Scrub {
                can_clean(p)
            } else {
                can_redate(p)
            }
        })
        .collect();
    if files.is_empty() {
        let note = match mode {
            Mode::Scrub => Notification::info(t!("Photos, videos, PDFs and Office files."))
                .title(t!("Nothing chosen that Glint can clean")),
            Mode::Date => Notification::info(t!("No photos or videos chosen")),
        };
        window.push_notification(note, cx);
        return;
    }
    let backend: Arc<dyn Backend> = match cx.try_global::<core_bridge::Core>() {
        Some(core) => Arc::new(CoreBackend::new(core.app.clone())),
        None => Arc::new(Unavailable),
    };
    open_with(window, cx, backend, files, mode);
}

/// [`open`] for a given backend, without filtering `files` (tests pass a fake). Returns the dialog's view.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    backend: Arc<dyn Backend>,
    files: Vec<String>,
    mode: Mode,
) -> Entity<MetadataDialog> {
    let shown = isolate(title(&files, mode));
    let view = cx.new(|cx| MetadataDialog::new(files, mode, backend, window, cx));
    super::show(window, cx, shown, px(576.), view.clone());
    view
}

// ---- Tests ----------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A run the fake was asked for.
    type RunCall = (Vec<String>, MetaTask, Output);

    /// A backend with canned answers.
    struct Fake {
        tags: Vec<Tag>,
        report: Report,
        reads: Mutex<usize>,
        calls: Mutex<Vec<RunCall>>,
        cancelled: Mutex<usize>,
    }

    impl Fake {
        fn new(tags: Vec<Tag>, made: &[&str]) -> Self {
            Self {
                tags,
                report: Report {
                    made: made.iter().map(|s| (*s).to_string()).collect(),
                    elapsed_ms: 640.,
                    ..Default::default()
                },
                reads: Mutex::new(0),
                calls: Mutex::new(Vec::new()),
                cancelled: Mutex::new(0),
            }
        }

        fn reads(&self) -> usize {
            self.reads.lock().map(|r| *r).unwrap_or(0)
        }

        fn calls(&self) -> Vec<RunCall> {
            self.calls.lock().map(|c| c.clone()).unwrap_or_default()
        }
    }

    impl Backend for Fake {
        fn read(&self, _: String) -> Fut<Vec<Tag>> {
            if let Ok(mut reads) = self.reads.lock() {
                *reads += 1;
            }
            let tags = self.tags.clone();
            Box::pin(async move { Ok(tags) })
        }

        fn run(
            &self,
            files: Vec<String>,
            task: MetaTask,
            output: Output,
            progress: glint_core::host::ipc::Channel<Progress>,
        ) -> Fut<Report> {
            let _ = progress.send(Progress {
                done: 0,
                total: files.len(),
                current: "a.jpg".into(),
                part: 0.,
            });
            if let Ok(mut calls) = self.calls.lock() {
                calls.push((files, task, output));
            }
            let report = self.report.clone();
            Box::pin(async move { Ok(report) })
        }

        fn cancel(&self) {
            if let Ok(mut n) = self.cancelled.lock() {
                *n += 1;
            }
        }

        fn tools(&self) -> Option<Arc<dyn crate::components::settings::Backend>> {
            None
        }
    }

    fn tag(group: &str, name: &str, value: &str, kind: &'static str) -> Tag {
        Tag {
            group: group.into(),
            name: name.into(),
            value: value.into(),
            kind,
        }
    }

    fn photo_tags() -> Vec<Tag> {
        vec![
            tag("GPS", "GPSLatitude", "51.50722", "place"),
            tag("EXIF", "Model", "Pixel 8", "device"),
            tag("EXIF", "CreateDate", "2024:06:01 12:00:00", "dates"),
        ]
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
        panic!("the metadata dialog never reached the state the test waits for");
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
            if text.is_empty() {
                window.press("backspace", cx);
            } else {
                window.input(&text, cx);
            }
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

    fn open_view(cx: &mut VisualTestContext, files: &[&str], mode: Mode, fake: &Arc<Fake>) -> Entity<MetadataDialog> {
        let backend: Arc<dyn Backend> = fake.clone();
        let files = strings(files);
        let view = cx.update(|window, cx| open_with(window, cx, backend, files, mode));
        cx.run_until_parked();
        view
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::EyeOff,
            IconName::CalendarClock,
            IconName::MapPin,
            IconName::ShieldCheck,
            IconName::ChevronDown,
            IconName::ChevronRight,
            IconName::ChevronLeft,
            IconName::Square,
            IconName::FolderInput,
            IconName::CircleCheck,
            IconName::TriangleAlert,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[test]
    fn what_can_be_cleaned_or_redated_is_told_by_the_name() {
        for yes in [
            "C:\\p\\a.JPG",
            "C:\\p\\a.heic",
            "C:\\p\\a.pdf",
            "C:\\p\\a.mp4",
            "C:\\p\\a.DOCX",
            "C:\\p\\a.xlsm",
        ] {
            assert!(can_clean(yes), "{yes}");
        }
        for no in [
            "C:\\p\\a.txt",
            "C:\\p\\a.mkv",
            "C:\\p\\jpg",
            "C:\\p\\.jpg",
            "C:\\p\\a.docx.bak",
        ] {
            assert!(!can_clean(no), "{no}");
        }
        assert!(can_redate("C:\\p\\a.cr3") && can_redate("C:\\p\\a.MOV") && can_redate("C:\\p\\a.png"));
        // Dates can't be set in these, though they can be cleaned.
        assert!(!can_redate("C:\\p\\a.pdf") && !can_redate("C:\\p\\a.gif") && !can_redate("C:\\p\\a.docx"));
        assert!(!can_redate("C:\\p\\a.m4a") && can_clean("C:\\p\\a.m4a"));
    }

    #[test]
    fn dates_are_checked_and_written_the_way_exiftool_reads_them() {
        assert_eq!(exif_date("2024-06-01 12:00").as_deref(), Some("2024:06:01 12:00:00"));
        assert_eq!(exif_date("2024-06-01T12:00").as_deref(), Some("2024:06:01 12:00:00"));
        assert_eq!(
            exif_date(" 2024-06-01T23:59:58 ").as_deref(),
            Some("2024:06:01 23:59:58")
        );
        assert_eq!(
            exif_date("2024-02-29 00:00").as_deref(),
            Some("2024:02:29 00:00:00"),
            "a leap day"
        );
        for bad in [
            "",
            "2024-06-01",
            "12:00",
            "2024-13-01 12:00",
            "2024-00-10 12:00",
            "2023-02-29 12:00",
            "2024-04-31 12:00",
            "2024-06-01 24:00",
            "2024-06-01 12:60",
            "2024-06-01 12:00:60",
            "24-06-01 12:00",
            "2024-6-1 12:00",
            "2024-06-01 12",
            "2024-06-01 12:00:00:00",
            "soon",
            "2024-06-01 ab:cd",
        ] {
            assert_eq!(exif_date(bad), None, "{bad:?}");
        }
        assert_eq!(whole("3"), 3);
        assert_eq!(whole(" -2 "), -2);
        assert_eq!(whole("2.9"), 2);
        assert_eq!(whole("-2.9"), -2);
        assert_eq!((whole(""), whole("x"), whole("inf")), (0, 0, 0));
        assert_eq!(TagKind::parse("place"), TagKind::Place);
        assert_eq!(TagKind::parse("whatever"), TagKind::Other);
        assert!(TagKind::Place.personal() && !TagKind::Dates.personal() && !TagKind::Other.personal());
    }

    #[gpui_kit::test]
    fn removing_personal_info_lists_what_a_photo_carries_and_runs_beside_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(photo_tags(), &["C:\\p\\a (clean).jpg"]));
        let view = open_view(&mut visual, &["C:\\p\\a.jpg"], Mode::Scrub, &fake);
        assert_eq!(role_of(&mut visual, "Remove personal info: a.jpg"), Some(Role::Group));
        settle(&mut visual, |cx| {
            has(cx, "3 details in it, 2 about where, who or with what")
        });
        assert_eq!(fake.reads(), 1);
        // Where it was and the camera are open; the dates are closed until asked for.
        assert_eq!(role_of(&mut visual, "GPSLatitude: 51.50722"), Some(Role::ListItem));
        assert!(has(&mut visual, "Model: Pixel 8"));
        assert!(!has(&mut visual, "CreateDate: 2024:06:01 12:00:00"));
        click(&mut visual, ("meta-group", 4_usize));
        assert!(has(&mut visual, "CreateDate: 2024:06:01 12:00:00"));
        click(&mut visual, ("meta-group", 0_usize));
        assert!(!has(&mut visual, "GPSLatitude: 51.50722"));
        assert_eq!(view.read_with(&visual, |v, _| v.tags().map(<[_]>::len)), Some(3));
        // Beside the file with " (clean)" to begin with; nothing else is asked.
        assert_eq!(
            view.read_with(&visual, |v, cx| v.place().read(cx).choice(cx)),
            Where::Beside {
                suffix: " (clean)".into()
            }
        );
        assert!(!disabled(&mut visual, "meta-start"));
        click(&mut visual, "meta-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        let calls = fake.calls();
        assert_eq!(calls[0].0, ["C:\\p\\a.jpg"]);
        assert_eq!(calls[0].1, MetaTask::Scrub);
        assert_eq!(
            calls[0].2,
            Output::Beside {
                suffix: " (clean)".into()
            }
        );
        assert_eq!(role_of(&mut visual, "Made 1 file"), Some(Role::Status));
    }

    #[gpui_kit::test]
    fn a_photo_with_nothing_personal_says_so_and_many_files_list_nothing(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(vec![], &[]));
        let _view = open_view(&mut visual, &["C:\\p\\a.png"], Mode::Scrub, &fake);
        settle(&mut visual, |cx| has(cx, "It carries no details to take out"));
        visual.update(|window, cx| window.close_dialog(cx));
        visual.run_until_parked();

        let fake = Arc::new(Fake::new(photo_tags(), &[]));
        let _view = open_view(&mut visual, &["C:\\p\\a.png", "C:\\p\\b.jpg"], Mode::Scrub, &fake);
        assert!(has(&mut visual, "Remove personal info: 2 files"));
        assert!(!has(&mut visual, "It carries no details to take out"));
        assert_eq!(fake.reads(), 0, "only one file is read");
    }

    #[gpui_kit::test]
    fn exiftool_gates_the_start_unless_every_file_is_an_office_one(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(photo_tags(), &["a"]));
        let view = open_view(&mut visual, &["C:\\p\\a.jpg"], Mode::Scrub, &fake);
        // ExifTool isn't known to be there: nothing starts, and nothing is read.
        view.update(&mut visual, |v, cx| {
            v.gate = Tooling::never();
            v.asked = false;
            v.tags = None;
            cx.notify();
        });
        assert!(disabled(&mut visual, "meta-start"));
        visual.update(|window, cx| view.update(cx, |v, cx| v.start(window, cx)));
        assert!(fake.calls().is_empty());
        visual.update(|window, cx| window.close_dialog(cx));
        visual.run_until_parked();

        // Office files are cleaned in Glint itself: no tool is needed.
        let view = open_view(&mut visual, &["C:\\p\\a.docx", "C:\\p\\b.xlsx"], Mode::Scrub, &fake);
        view.update(&mut visual, |v, cx| {
            v.gate = Tooling::never();
            cx.notify();
        });
        assert!(!disabled(&mut visual, "meta-start"));
        click(&mut visual, "meta-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        assert_eq!(fake.calls()[0].0, ["C:\\p\\a.docx", "C:\\p\\b.xlsx"]);
    }

    #[gpui_kit::test]
    fn the_date_taken_is_moved_or_set_and_replaced_in_place(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(vec![], &["C:\\p\\a.jpg", "C:\\p\\b.jpg"]));
        let view = open_view(&mut visual, &["C:\\p\\a.jpg", "C:\\p\\b.jpg"], Mode::Date, &fake);
        assert_eq!(role_of(&mut visual, "When it was taken: 2 files"), Some(Role::Group));
        assert_eq!(
            view.read_with(&visual, |v, cx| v.place().read(cx).choice(cx)),
            Where::Replace
        );
        assert_eq!(role_of(&mut visual, "How"), Some(Role::RadioGroup));
        // Moving by nothing changes nothing.
        assert!(disabled(&mut visual, "meta-start"));
        let (when, hours, minutes) = view.read_with(&visual, |v, _| {
            let (w, h, m) = v.date_inputs();
            (w.clone(), h.clone(), m.clone())
        });
        type_into(&mut visual, &hours, "-2");
        type_into(&mut visual, &minutes, "30");
        assert_eq!(view.read_with(&visual, |v, cx| v.shift(cx)), -90);
        assert!(!disabled(&mut visual, "meta-start"));
        // Setting it: a real date and time.
        click(&mut visual, ("meta-how", 1_usize));
        assert!(disabled(&mut visual, "meta-start"));
        type_into(&mut visual, &when, "2024-02-30 10:00");
        assert_eq!(role_of(&mut visual, "Not a date and time"), Some(Role::Alert));
        assert!(disabled(&mut visual, "meta-start"));
        type_into(&mut visual, &when, "2024-06-01 12:00");
        assert!(!has(&mut visual, "Not a date and time") && !disabled(&mut visual, "meta-start"));
        click(&mut visual, "meta-start");
        settle(&mut visual, |cx| has(cx, "Made 2 files"));
        let calls = fake.calls();
        assert_eq!(
            calls[0].1,
            MetaTask::SetDate {
                when: "2024:06:01 12:00:00".into()
            }
        );
        assert_eq!(calls[0].2, Output::Replace);
        visual.update(|window, cx| window.close_dialog(cx));
        visual.run_until_parked();

        // Moved: hours and minutes together.
        let fake = Arc::new(Fake::new(vec![], &["C:\\p\\a.jpg"]));
        let view = open_view(&mut visual, &["C:\\p\\a.jpg"], Mode::Date, &fake);
        let (hours, minutes) = view.read_with(&visual, |v, _| {
            let (_, h, m) = v.date_inputs();
            (h.clone(), m.clone())
        });
        type_into(&mut visual, &hours, "1");
        type_into(&mut visual, &minutes, "5");
        click(&mut visual, "meta-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        assert_eq!(fake.calls()[0].1, MetaTask::ShiftDate { minutes: 65 });
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_work_through_a_clean(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let mut tags = photo_tags();
        tags.push(tag("XMP", "الكاتب", "محمد", "people"));
        let mut partial = Fake::new(tags, &["C:\\ص\\صورة (clean).jpg"]);
        partial.report.failed = vec![("C:\\ص\\ملف.pdf".into(), "Couldn't read it".into())];
        let fake = Arc::new(partial);
        let _view = open_view(&mut visual, &["C:\\ص\\صورة.jpg"], Mode::Scrub, &fake);
        assert_eq!(role_of(&mut visual, "C:\\ص"), Some(Role::Group));
        settle(&mut visual, |cx| {
            has(cx, "4 details in it, 3 about where, who or with what")
        });
        assert_eq!(role_of(&mut visual, "الكاتب: محمد"), Some(Role::ListItem));
        click(&mut visual, "meta-start");
        settle(&mut visual, |cx| has(cx, "Made 1 file"));
        assert_eq!(role_of(&mut visual, "ملف.pdf: Couldn't read it"), Some(Role::ListItem));
        testing::set_rtl(false);
    }

    #[test]
    fn titles_name_the_file_or_how_many() {
        let _guard = testing::lock();
        assert_eq!(
            title(&strings(&["C:\\p\\a.jpg"]), Mode::Scrub),
            "Remove personal info: a.jpg"
        );
        assert_eq!(
            title(&strings(&["C:\\p\\a.jpg", "C:\\p\\b.jpg"]), Mode::Date),
            "When it was taken: 2 files"
        );
    }
}
