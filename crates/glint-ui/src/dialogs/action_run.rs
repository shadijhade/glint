//! What the file-action dialogs share: Convert pictures, Convert document, Make smaller, PDF tools and Convert data.
//! Ported from src/components/explorer/ActionParts.tsx and src/lib/actions.ts.
//!
//! | TypeScript                       | Here                                                              |
//! |----------------------------------|-------------------------------------------------------------------|
//! | `ActionOutput`, `badOutput`      | [`Where`], [`Where::bad`]                                         |
//! | `WhereChooser`                   | [`WhereChooser`], a view of its own                               |
//! | `useActionRun`                   | [`ActionRun`], a model entity (`Setup`, `Running`, `Done`, `Failed`) |
//! | `RunProgress`                    | [`progress_view`]                                                 |
//! | `RunResult`                      | [`result_view`], [`error_view`]                                   |
//! | `RunFooter`                      | [`footer`]                                                        |
//! | `Choices`                        | [`choices`]                                                       |
//! | `offerActionUndo`                | [`remember_undo`] (Ctrl+Z; see the note there)                    |
//! | `revealInApp`                    | [`reveal`]                                                        |
//! | `ToolNeeded`                     | [`tool_card`] over `settings::tools::needed`                      |
//! | a slider (quality, strength)     | [`NumberField`]: a box with minus and plus (`PORT ME(coordinator)`: kit::Slider) |
//!
//! **One family.** Convert pictures, Convert document, Make smaller, PDF tools and Convert data keep their run in
//! an [`ActionRun`] entity; the media, media-tools, song and metadata dialogs (MediaDialog.tsx, MediaTools.tsx,
//! SongDialog.tsx, MetadataDialog.tsx) keep it in a field of the dialog and start it with [`drive`] (their tests
//! and `run_state()` read it there). The pieces they draw with are the same: [`Where`], [`WhereChooser`],
//! [`Choices`], [`progress_view`], [`result_view`], [`error_view`], [`footer`], [`Tooling`]. Two shapes differ,
//! and stay here: [`drive`] runs in a view with a window so it can offer Undo in a notification
//! ([`offer_undo`]), while [`ActionRun::start`] has no window and keeps Ctrl+Z only ([`remember_undo`]).

use crate::components::settings::tools::{self, Tools};
use crate::components::settings::{Backend as SettingsBackend, CoreBackend as SettingsCoreBackend};
use crate::kit::prelude::*;
use crate::{core_bridge, explorer, t, theme};
use glint_core::actions::{Output, Progress, Report};
use glint_core::host::Manager as _;
use glint_model::format::{format_count, format_duration, format_size, plural};
use glint_model::paths::{base_name, parent_path};
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

/// A boxed core call.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// Runs `future` on core's runtime and flattens the join error into the call's own.
pub fn on_core<T: Send + 'static>(future: impl Future<Output = Result<T, String>> + Send + 'static) -> Fut<T> {
    let joined = core_bridge::run(future);
    Box::pin(async move { joined.await.and_then(|inner| inner) })
}

/// The main window core's calls take as their owner, or why there is none.
pub fn main_window(app: &glint_core::host::AppHandle) -> Result<glint_core::host::Window, String> {
    app.get_webview_window(crate::workspace::MAIN)
        .ok_or_else(|| t!("Glint's window isn't open").to_string())
}

/// What core says when it isn't started (never in the app).
pub fn no_core<T: Send + 'static>() -> Fut<T> {
    Box::pin(async { Err("Glint's core isn't running".to_string()) })
}

// ---- Where what is made goes (WhereChooser, badOutput) --------------------------------------------------

/// Where what a file action makes goes (`ActionOutput`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Where {
    /// Next to each file, its name and `suffix`.
    Beside { suffix: String },
    /// Into a folder of this name next to them.
    Folder { name: String },
    /// In their place, the files moved to the Recycle Bin.
    Replace,
}

/// Whether a suffix or folder name can't be used: a character Windows names can't hold, or ending in a dot or
/// space (`NOT_IN_NAMES`).
pub fn not_in_names(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        || text.ends_with(['.', ' '])
}

impl Where {
    /// core's `Output`.
    pub fn output(&self) -> Output {
        match self {
            Where::Beside { suffix } => Output::Beside { suffix: suffix.clone() },
            Where::Folder { name } => Output::Folder { name: name.clone() },
            Where::Replace => Output::Replace,
        }
    }

    /// Whether the choice can't be used (`badOutput`).
    pub fn bad(&self) -> bool {
        match self {
            Where::Beside { suffix } => not_in_names(suffix),
            Where::Folder { name } => name.trim().is_empty() || not_in_names(name),
            Where::Replace => false,
        }
    }
}

/// Which of the three the person picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhereKind {
    /// Next to each file.
    Beside,
    /// Into a folder.
    Folder,
    /// In their place.
    Replace,
}

/// The "Where" section: three choices, the first with a box for what is added to the name, the second with a
/// box for the folder's name. Observe it to hear changes; read the choice with [`WhereChooser::choice`].
pub struct WhereChooser {
    id: &'static str,
    noun: SharedString,
    replace: bool,
    note: SharedString,
    kind: WhereKind,
    suffix: Entity<InputState>,
    folder: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl WhereChooser {
    /// A chooser named `id` ("convert-where"), for files called `noun`, with `note` under it. `replace` is
    /// whether "In their place" is offered. The boxes start with `suffix` and `folder`.
    pub fn new(
        id: &'static str,
        noun: SharedString,
        note: SharedString,
        replace: bool,
        (suffix, folder): (&str, &str),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let suffix_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("nothing"))
                .default_value(suffix.to_string())
        });
        let folder_input = cx.new(|cx| InputState::new(window, cx).default_value(folder.to_string()));
        // Typing in a box is choosing its radio (the TypeScript's `onChange` set `where`).
        let subscriptions = vec![
            cx.subscribe(&suffix_input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.kind = WhereKind::Beside;
                    cx.notify();
                }
            }),
            cx.subscribe(&folder_input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.kind = WhereKind::Folder;
                    cx.notify();
                }
            }),
        ];
        Self {
            id,
            noun,
            replace,
            note,
            kind: WhereKind::Beside,
            suffix: suffix_input,
            folder: folder_input,
            _subscriptions: subscriptions,
        }
    }

    /// A chooser starting at `initial` for "file"s, its note the default (a taken name gets "(2)"). The folder
    /// box starts as "Converted" unless `initial` is a folder.
    pub fn starting_at(id: &'static str, initial: &Where, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (kind, suffix, folder) = match initial {
            Where::Beside { suffix } => (WhereKind::Beside, suffix.as_str(), "Converted"),
            Where::Folder { name } => (WhereKind::Folder, "", name.as_str()),
            Where::Replace => (WhereKind::Replace, "", "Converted"),
        };
        let mut this = Self::new(
            id,
            t!("file"),
            t!("A name that's taken gets “(2)”, and Ctrl+Z takes it all back."),
            true,
            (suffix, folder),
            window,
            cx,
        );
        // Starting text isn't typing: the radio is the one asked for.
        this.kind = kind;
        this
    }

    /// Whether what is chosen can't be used (`badOutput`).
    pub fn bad(&self, cx: &App) -> bool {
        self.choice(cx).bad()
    }

    /// "Next to each {noun}…" and "the {noun}s moved to the Recycle Bin".
    pub fn set_noun(&mut self, noun: SharedString, cx: &mut Context<Self>) {
        if self.noun != noun {
            self.noun = noun;
            cx.notify();
        }
    }

    /// The line under the choices.
    pub fn set_note(&mut self, note: SharedString, cx: &mut Context<Self>) {
        self.note = note;
        cx.notify();
    }

    /// Changes the suffix when "Next to" is the choice (the dialog's mode asks for another).
    ///
    /// NOTE(port): the TypeScript changed its output but left the box showing the old suffix (the box kept its
    /// own state from when it was made). Here the box shows what is used.
    pub fn set_suffix(&mut self, suffix: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.kind == WhereKind::Beside {
            self.suffix.update(cx, |input, cx| input.set_value(suffix, window, cx));
            cx.notify();
        }
    }

    /// What is chosen now.
    pub fn choice(&self, cx: &App) -> Where {
        match self.kind {
            WhereKind::Beside => Where::Beside {
                suffix: self.suffix.read(cx).value().to_string(),
            },
            WhereKind::Folder => Where::Folder {
                name: self.folder.read(cx).value().to_string(),
            },
            WhereKind::Replace => Where::Replace,
        }
    }

    /// Which radio is on.
    pub fn kind(&self) -> WhereKind {
        self.kind
    }

    /// The box for what is added to the name.
    pub fn suffix_input(&self) -> &Entity<InputState> {
        &self.suffix
    }

    /// The box for the folder's name.
    pub fn folder_input(&self) -> &Entity<InputState> {
        &self.folder
    }

    /// Picks `kind` and fills the boxes (PDF tools puts parts and pictures into a folder named after the PDF).
    pub fn set(
        &mut self,
        kind: WhereKind,
        suffix: Option<&str>,
        folder: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(suffix) = suffix {
            self.suffix.update(cx, |input, cx| input.set_value(suffix, window, cx));
        }
        if let Some(folder) = folder {
            self.folder.update(cx, |input, cx| input.set_value(folder, window, cx));
        }
        // Setting a box's text picked its radio; the choice made here wins.
        self.kind = kind;
        cx.notify();
    }

    /// Whether "In their place" is offered.
    pub fn replace_offered(&self) -> bool {
        self.replace
    }

    /// Offers or withdraws "In their place" (PDF tools offers it for some tasks only); a choice of it that is
    /// withdrawn becomes "Next to each".
    pub fn offer_replace(&mut self, offered: bool, cx: &mut Context<Self>) {
        self.replace = offered;
        if !offered && self.kind == WhereKind::Replace {
            self.kind = WhereKind::Beside;
        }
        cx.notify();
    }
}

impl Render for WhereChooser {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let bad = self.choice(cx).bad();
        let id = self.id;
        let radio = |ix: usize, kind: WhereKind, label: SharedString, cx: &mut Context<Self>| {
            let chosen = self.kind == kind;
            Button::new((id, ix))
                .ghost()
                .compact()
                .role(Role::RadioButton)
                .toggled(chosen)
                .accessibility_label(label)
                .icon(if chosen { IconName::CircleDot } else { IconName::Circle })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.kind = kind;
                    cx.notify();
                }))
        };
        let small_box = |input: &Entity<InputState>, aria: SharedString, wide: f32, on: bool| {
            div()
                .w(space(wide))
                .flex_none()
                .when(on && bad, |d| {
                    d.border_1().border_color(t.destructive).rounded(theme::size::RADIUS)
                })
                .child(Input::new(input).aria_label(aria))
        };
        let beside = t!("Next to each {noun}, its name and", noun = self.noun);
        let folder = t!("Into a folder next to them:");
        let replace = t!("In their place, the {noun}s moved to the Recycle Bin", noun = self.noun);
        col()
            .id(self.id)
            .role(Role::RadioGroup)
            .aria_label(t!("Where"))
            .test_support()
            .gap(space(1.5))
            .child(section_title(t!("Where"), &t))
            .child(
                align_row(2.)
                    .child(radio(0, WhereKind::Beside, beside.clone(), cx))
                    .child(div().flex_none().child(beside))
                    .child(small_box(
                        &self.suffix,
                        t!("Added to the name"),
                        28.,
                        self.kind == WhereKind::Beside,
                    )),
            )
            .child(
                align_row(2.)
                    .child(radio(1, WhereKind::Folder, folder.clone(), cx))
                    .child(div().flex_none().child(folder))
                    .child(small_box(
                        &self.folder,
                        t!("Folder name"),
                        36.,
                        self.kind == WhereKind::Folder,
                    )),
            )
            .when(self.replace, |this| {
                this.child(
                    align_row(2.)
                        .child(radio(2, WhereKind::Replace, replace.clone(), cx))
                        .child(div().flex_1().min_w_0().child(replace)),
                )
            })
            .child(
                div()
                    .ps(6.)
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(self.note.clone()),
            )
    }
}

// ---- Small pieces ---------------------------------------------------------------------------------------

/// A section's heading (`text-xs font-semibold text-muted-foreground`).
pub fn section_title(text: SharedString, t: &theme::Tokens) -> Div {
    div()
        .text_size(theme::size::XS)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.muted_foreground)
        .child(text)
}

/// A paragraph of help (`text-xs text-muted-foreground`).
pub fn help(text: impl Into<SharedString>, t: &theme::Tokens) -> Div {
    div()
        .text_size(theme::size::XS)
        .text_color(t.muted_foreground)
        .child(text.into())
}

/// A labelled row: a fixed-width label, then the control.
pub fn field(label: SharedString, control: impl IntoElement, t: &theme::Tokens) -> Div {
    align_row(3.)
        .child(
            div()
                .w(space(20.))
                .flex_none()
                .text_size(theme::size::XS)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.muted_foreground)
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(control))
}

/// One choice of a [`choices`] group.
pub struct Choice<T> {
    pub value: T,
    pub label: SharedString,
    pub hint: Option<SharedString>,
    pub disabled: bool,
}

impl<T> Choice<T> {
    /// A choice named `label`.
    pub fn new(value: T, label: impl Into<SharedString>) -> Self {
        Self {
            value,
            label: label.into(),
            hint: None,
            disabled: false,
        }
    }

    /// With a line of help under the name (also its tooltip and part of its accessible name).
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Can't be picked.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// What picking a choice does.
type OnPick<T> = Rc<dyn Fn(&T, &mut Window, &mut App)>;

/// A group of choices, one picked, laid out in `columns` (`Choices`): each a button with a name and a hint. The
/// buttons are `(id, index)`; `on_pick` gets the value picked (use `cx.listener` to reach the dialog).
#[derive(IntoElement)]
pub struct Choices<T: Copy + PartialEq + 'static> {
    id: &'static str,
    label: SharedString,
    value: T,
    options: Vec<Choice<T>>,
    columns: u16,
    on_pick: OnPick<T>,
}

impl<T: Copy + PartialEq + 'static> Choices<T> {
    /// The group `label`, with `id` for its buttons (`(id, index)`), `value` picked, two to a row.
    pub fn new(id: &'static str, label: impl Into<SharedString>, value: T) -> Self {
        Self {
            id,
            label: label.into(),
            value,
            options: Vec::new(),
            columns: 2,
            on_pick: Rc::new(|_, _, _| {}),
        }
    }

    /// What can be picked.
    pub fn options(mut self, options: Vec<Choice<T>>) -> Self {
        self.options = options;
        self
    }

    /// How many to a row (the TypeScript's `columns`, 2 unless said).
    pub fn columns(mut self, columns: u16) -> Self {
        self.columns = columns.max(1);
        self
    }

    /// Runs with the value picked.
    pub fn on_pick(mut self, on_pick: impl Fn(&T, &mut Window, &mut App) + 'static) -> Self {
        self.on_pick = Rc::new(on_pick);
        self
    }
}

impl<T: Copy + PartialEq + 'static> RenderOnce for Choices<T> {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let id = self.id;
        let value = self.value;
        let on_pick = self.on_pick;
        div()
            .id(id)
            .role(Role::RadioGroup)
            .aria_label(self.label)
            .test_support()
            .grid()
            .grid_cols(self.columns.max(1))
            .gap(space(1.5))
            .children(self.options.into_iter().enumerate().map(|(ix, option)| {
                let chosen = option.value == value;
                let on_pick = on_pick.clone();
                let picked = option.value;
                let label = option.label.clone();
                let hint = option.hint.clone();
                Button::new((id, ix))
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
                    .px(space(2.))
                    .py(space(1.5))
                    .role(Role::RadioButton)
                    .toggled(chosen)
                    .disabled(option.disabled)
                    .when_some(hint.clone(), |this, hint| this.tooltip(hint))
                    .accessibility_label(match &hint {
                        Some(hint) => format!("{label}. {hint}"),
                        None => label.to_string(),
                    })
                    .on_click(move |_, window, cx| on_pick(&picked, window, cx))
                    .child(
                        col()
                            .w_full()
                            .min_w_0()
                            .align_start()
                            .text_start()
                            .child(div().font_weight(FontWeight::MEDIUM).child(label))
                            .when_some(hint, |this, hint| {
                                this.child(
                                    div()
                                        .max_w_full()
                                        .truncate()
                                        .text_size(theme::size::XXS)
                                        .text_color(t.muted_foreground)
                                        .child(hint),
                                )
                            }),
                    )
            }))
    }
}

/// [`Choices`] for a dialog whose `on_pick` gets the dialog itself (and redraws it).
#[allow(clippy::too_many_arguments)]
pub fn choices<V: 'static, T: Copy + PartialEq + 'static>(
    id: &'static str,
    name: SharedString,
    value: T,
    options: Vec<Choice<T>>,
    columns: u16,
    on_pick: impl Fn(&mut V, T, &mut Window, &mut Context<V>) + Clone + 'static,
    _t: &theme::Tokens,
    cx: &mut Context<V>,
) -> impl IntoElement {
    Choices::new(id, name, value)
        .options(options)
        .columns(columns)
        .on_pick(cx.listener(move |this, picked: &T, window, cx| {
            on_pick(this, *picked, window, cx);
            cx.notify();
        }))
}

// ---- A number with minus and plus -----------------------------------------------------------------------

/// A number in a box with a button either side (the TypeScript's sliders for quality and strength).
///
/// `PORT ME(coordinator)`: the kit has no slider; this is the closest thing it has, and works from the keyboard.
pub struct NumberField {
    id: &'static str,
    label: SharedString,
    suffix: SharedString,
    input: Entity<InputState>,
    min: i64,
    max: i64,
    step: i64,
    fallback: i64,
    _subscription: Subscription,
}

impl NumberField {
    /// A field named `label` ("Quality") between `min` and `max`, starting at `start`, the buttons moving it by
    /// `step`. `suffix` follows the box ("%", "pages").
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: &'static str,
        label: SharedString,
        suffix: SharedString,
        (min, max, step): (i64, i64, i64),
        start: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(start.to_string()));
        let subscription = cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            id,
            label,
            suffix,
            input,
            min,
            max,
            step,
            fallback: start.clamp(min, max),
            _subscription: subscription,
        }
    }

    /// The box, for focusing and tests.
    pub fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    /// The number now: what is typed, kept between the least and the most; the start when it is not a number.
    pub fn value(&self, cx: &App) -> i64 {
        self.input
            .read(cx)
            .value()
            .trim()
            .parse::<i64>()
            .map_or(self.fallback, |n| n.clamp(self.min, self.max))
    }

    /// Whether what is typed is a number in range.
    pub fn valid(&self, cx: &App) -> bool {
        self.input
            .read(cx)
            .value()
            .trim()
            .parse::<i64>()
            .is_ok_and(|n| (self.min..=self.max).contains(&n))
    }

    /// Sets the number.
    pub fn set(&mut self, value: i64, window: &mut Window, cx: &mut Context<Self>) {
        let value = value.clamp(self.min, self.max);
        self.input
            .update(cx, |input, cx| input.set_value(value.to_string(), window, cx));
        cx.notify();
    }
}

impl Render for NumberField {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let value = self.value(cx);
        let valid = self.valid(cx);
        let step = self.step;
        let less = Button::new((self.id, 0_usize))
            .outline()
            .compact()
            .icon(IconName::Minus)
            .accessibility_label(t!("Less {what}", what = self.label.to_lowercase()))
            .disabled(value <= self.min)
            .on_click(cx.listener(move |this, _, window, cx| {
                let now = this.value(cx);
                this.set(now - step, window, cx);
            }));
        let more = Button::new((self.id, 1_usize))
            .outline()
            .compact()
            .icon(IconName::Plus)
            .accessibility_label(t!("More {what}", what = self.label.to_lowercase()))
            .disabled(value >= self.max)
            .on_click(cx.listener(move |this, _, window, cx| {
                let now = this.value(cx);
                this.set(now + step, window, cx);
            }));
        align_row(1.5)
            .id(self.id)
            .test_support()
            .child(less)
            .child(
                div()
                    .w(space(20.))
                    .flex_none()
                    .when(!valid, |d| {
                        d.border_1().border_color(t.destructive).rounded(theme::size::RADIUS)
                    })
                    .child(Input::new(&self.input).aria_label(self.label.clone())),
            )
            .child(more)
            .child(
                div()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(self.suffix.clone()),
            )
    }
}

// ---- The run (useActionRun) -----------------------------------------------------------------------------

/// Where a run stands.
#[derive(Clone, Debug)]
pub enum Run {
    /// Not started: the choices are on screen.
    Setup,
    /// Under way, with how far.
    Running(Progress),
    /// Finished, with the report.
    Done(Report),
    /// Failed as a whole, with why.
    Failed(SharedString),
}

impl Run {
    /// A run has been asked for (`started`).
    pub fn started(&self) -> bool {
        !matches!(self, Run::Setup)
    }

    /// It is under way (`running`).
    pub fn running(&self) -> bool {
        matches!(self, Run::Running(_))
    }

    /// It ended, well or not.
    pub fn finished(&self) -> bool {
        matches!(self, Run::Done(_) | Run::Failed(_))
    }

    /// The first thing it made (what Show reveals).
    pub fn first_made(&self) -> Option<String> {
        match self {
            Run::Done(report) => report.made.first().cloned(),
            _ => None,
        }
    }
}

/// A file action's run: its progress, then its report or error; [`ActionRun::start`] runs one and keeps Undo
/// for what it made. Observe it to redraw.
pub struct ActionRun {
    state: Run,
}

impl Default for ActionRun {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionRun {
    /// A run that hasn't started.
    pub fn new() -> Self {
        Self { state: Run::Setup }
    }

    /// Where it stands.
    pub fn state(&self) -> &Run {
        &self.state
    }

    /// Puts the run in `state` by hand (tests draw states a fake backend would have to be held for).
    pub fn set(&mut self, state: Run, cx: &mut Context<Self>) {
        self.state = state;
        cx.notify();
    }

    /// Back to the choices (`reset`).
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.set(Run::Setup, cx);
    }

    /// Starts `call` on core's runtime (`start`): `progress` brings its messages, one redraw for however many
    /// came since the last frame; when it ends the report is kept (and Undo with it, named by `undo_label`).
    pub fn start(
        &mut self,
        total: usize,
        call: Fut<Report>,
        progress: async_channel::Receiver<Progress>,
        undo_label: impl Fn(&Report) -> String + 'static,
        cx: &mut Context<Self>,
    ) {
        self.start_mapped(total, call, progress, |p| p, undo_label, cx);
    }

    /// [`ActionRun::start`] for a call whose progress is of another kind (`map` turns it into a [`Progress`]).
    pub fn start_mapped<P: Send + 'static>(
        &mut self,
        total: usize,
        call: Fut<Report>,
        progress: async_channel::Receiver<P>,
        map: impl Fn(P) -> Progress + 'static,
        undo_label: impl Fn(&Report) -> String + 'static,
        cx: &mut Context<Self>,
    ) {
        self.state = Run::Running(Progress {
            done: 0,
            total,
            current: String::new(),
            part: 0.,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            while let Ok(mut latest) = progress.recv().await {
                // Keep only the newest message each time the task wakes, so a chatty run redraws once.
                while let Ok(newer) = progress.try_recv() {
                    latest = newer;
                }
                let latest = map(latest);
                let alive = this.update(cx, |this, cx| {
                    if this.state.running() {
                        this.state = Run::Running(latest);
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(report) => {
                        remember_undo(&report, &undo_label(&report), cx);
                        this.state = Run::Done(report);
                    }
                    Err(reason) => this.state = Run::Failed(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// The undo steps a run's report left in the explorer.
struct UndoSteps {
    explorer: Entity<explorer::Explorer>,
    /// The files it replaced coming back from the Recycle Bin, if any were replaced.
    restore: Option<explorer::UndoId>,
    /// What it made going to the Recycle Bin.
    recycle: explorer::UndoId,
}

/// Records how to take back what `report` made: what it made is recycled and the files it replaced come back
/// from the Recycle Bin. `None` when it made nothing or there is no explorer.
fn record_undo(report: &Report, what: &str, cx: &mut App) -> Option<UndoSteps> {
    if report.made.is_empty() {
        return None;
    }
    let explorer = explorer::try_explorer(cx)?;
    let back: Vec<explorer::MoveItem> = report
        .recycled
        .iter()
        .map(|r| explorer::MoveItem {
            path: r.bin.clone(),
            folder: parent_path(&r.path),
            name: base_name(&r.path).to_string(),
        })
        .collect();
    let made = report.made.clone();
    let (restore, recycle) = explorer.update(cx, |explorer, cx| {
        let restore = (!back.is_empty()).then(|| explorer.remember_undo(what, explorer::UndoAction::Restore(back), cx));
        (
            restore,
            explorer.remember_undo(what, explorer::UndoAction::Recycle(made), cx),
        )
    });
    Some(UndoSteps {
        explorer,
        restore,
        recycle,
    })
}

/// Keeps how to take back what `report` made, for Ctrl+Z (`offerActionUndo`): what it made is recycled and the
/// files it replaced come back from the Recycle Bin.
///
/// NOTE(port): the TypeScript took both back in one step, and offered the step in a toast with an Undo button.
/// The explorer's undo steps hold one action each (`UndoAction`), so the replaced files' restore is its own
/// step, below the recycling of what was made: Ctrl+Z recycles what was made, the next brings the originals
/// back. [`offer_undo`]'s button does both. `PORT ME(coordinator)`: a step that holds several actions
/// (`UndoAction::Sequence` in explorer/ops_types.rs and undo.rs) would make this one Ctrl+Z.
pub fn remember_undo(report: &Report, what: &str, cx: &mut App) {
    let _ = record_undo(report, what, cx);
}

/// The Undo offer of an action's notification.
struct ActionUndoToast;

/// [`remember_undo`], and the notification that offers Undo for 8 s as the web app's toast did (`offerActionUndo`):
/// its button takes back both steps.
pub fn offer_undo(report: &Report, what: &str, window: &mut Window, cx: &mut App) {
    let Some(UndoSteps {
        explorer,
        restore,
        recycle,
    }) = record_undo(report, what, cx)
    else {
        return;
    };
    let mut label = what.to_string();
    if let Some(first) = label.get(..1) {
        label = first.to_uppercase() + &label[1..];
    }
    let key = recycle.0 as usize;
    let note = Notification::success(SharedString::from(label))
        .id1::<ActionUndoToast>(key)
        .action(move |_, _, _| {
            let explorer = explorer.clone();
            Button::new("action-undo")
                .label(t!("Undo"))
                .on_click(move |_, window, cx| {
                    let first = explorer.update(cx, |e, cx| e.undo(Some(recycle), cx));
                    let explorer = explorer.clone();
                    cx.spawn(async move |cx| {
                        first.await;
                        if let Some(restore) = restore {
                            let second = explorer.update(cx, |e, cx| e.undo(Some(restore), cx));
                            second.await;
                        }
                    })
                    .detach();
                    window.remove_notification1::<ActionUndoToast>(key, cx);
                })
        });
    window.push_notification(note, cx);
    // The web app's toast stayed 8 s.
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(std::time::Duration::from_secs(8)).await;
        let _ = handle.update(cx, |_, window, cx| {
            window.remove_notification1::<ActionUndoToast>(key, cx)
        });
    })
    .detach();
}

/// A run to start in a dialog that keeps its [`Run`] itself: how many jobs, core's call, and its progress.
pub struct Launch {
    /// How many jobs there are (the run's total until progress says).
    pub total: usize,
    /// Core's call.
    pub call: Fut<Report>,
    /// Its progress messages.
    pub progress: async_channel::Receiver<Progress>,
}

/// Starts a run in a dialog (`useActionRun().start`): sets its [`Run`] (found by `state`) to running, keeps it
/// up to date from the progress (at most once a frame), and when the call ends sets it to done or failed. A run
/// that made something offers Undo, worded `undo_label(report)` ([`offer_undo`]).
pub fn drive<V: 'static>(
    this: &mut V,
    window: &mut Window,
    cx: &mut Context<V>,
    launch: Launch,
    state: fn(&mut V) -> &mut Run,
    undo_label: impl FnOnce(&Report) -> String + 'static,
) {
    let Launch { total, call, progress } = launch;
    *state(this) = Run::Running(Progress {
        done: 0,
        total,
        current: String::new(),
        part: 0.,
    });
    cx.notify();
    // Progress: only the newest message each time the task wakes, so a chatty run redraws once.
    cx.spawn_in(window, async move |this, cx| {
        while let Ok(mut latest) = progress.recv().await {
            while let Ok(newer) = progress.try_recv() {
                latest = newer;
            }
            let alive = this.update(cx, |this, cx| {
                let run = state(this);
                if run.running() {
                    *run = Run::Running(latest);
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        }
    })
    .detach();
    cx.spawn_in(window, async move |this, cx| {
        let result = call.await;
        let _ = this.update_in(cx, |this, window, cx| {
            match result {
                Ok(report) => {
                    offer_undo(&report, &undo_label(&report), window, cx);
                    *state(this) = Run::Done(report);
                }
                Err(reason) => *state(this) = Run::Failed(reason.into()),
            }
            cx.notify();
        });
    })
    .detach();
}

/// Shows `path` in the explorer: its folder, with it selected (`revealInApp`).
pub fn reveal(path: &str, cx: &mut App) {
    let Some(explorer) = explorer::try_explorer(cx) else {
        return;
    };
    let path = path.to_string();
    explorer.update(cx, |explorer, cx| {
        let options = explorer::NavOptions::default().selecting(path.clone());
        explorer.navigate(parent_path(&path), options, cx);
    });
}

// ---- Drawing a run -----------------------------------------------------------------------------------------

/// "Converting 2 of 3 · tax.pdf · 50%" for a run under way (`RunProgress`'s line). `counted` says "2 of 3" even
/// for one file, and `done_word` adds "done" after the file's name (Convert pictures' wording).
pub fn progress_text(verb: &str, progress: &Progress, counted: bool, done_word: bool) -> String {
    let mut text = verb.to_string();
    if progress.total > 1 || (counted && progress.total > 0) {
        let at = (progress.done + 1).min(progress.total);
        text.push_str(&format!(" {} of {}", format_count(at), format_count(progress.total)));
    }
    if !progress.current.is_empty() {
        text.push_str(&format!(" · {}", progress.current));
        if done_word {
            text.push_str(&t!(" done"));
        }
    }
    if progress.part > 0. {
        text.push_str(&format!(" · {}%", (progress.part * 100.).round()));
    }
    text
}

/// How far a run is, 0 to 1: the jobs done and the part of the one under way.
pub fn whole(progress: &Progress) -> f32 {
    if progress.total == 0 {
        0.
    } else {
        ((progress.done as f32 + progress.part.min(0.999)) / progress.total as f32).clamp(0., 1.)
    }
}

/// A run under way: which of how many, and how far the one being made is.
pub fn progress_view(id: &'static str, text: String, fraction: f32, t: &theme::Tokens) -> impl IntoElement {
    let text: SharedString = text.into();
    col()
        .id(id)
        .role(Role::ProgressIndicator)
        .aria_label(text.clone())
        .test_support()
        .gap(space(2.))
        .py(space(6.))
        .child(
            align_row(2.)
                .text_color(t.muted_foreground)
                .child(Spinner::new())
                .child(div().flex_1().min_w_0().truncate().child(isolate(text))),
        )
        .child(
            row_start()
                .h(space(1.5))
                .w_full()
                .overflow_hidden()
                .rounded_full()
                .bg(t.muted)
                .child(
                    div()
                        .h_full()
                        .w(gpui_kit::relative(fraction.clamp(0., 1.)))
                        .rounded_full()
                        .bg(t.primary),
                ),
        )
}

/// [`progress_view`] for a run under way with the usual line: `verb`, which of how many, the file, how far.
pub fn run_progress(id: &'static str, verb: &str, progress: &Progress, t: &theme::Tokens) -> impl IntoElement {
    progress_view(id, progress_text(verb, progress, false, false), whole(progress), t)
}

/// The words a finished run is told in.
pub struct ResultWords {
    /// "Made 3 files" for how many were made (more than none).
    pub made: fn(usize) -> SharedString,
    /// "Nothing was made".
    pub nothing: SharedString,
    /// "2 files not done:" for how many failed.
    pub failed: fn(usize) -> SharedString,
    /// Show the sizes before and after.
    pub sizes: bool,
}

impl ResultWords {
    /// The words for songs (Edit song details): "Made 3 songs", no sizes line.
    pub fn songs() -> Self {
        Self {
            made: |n| t!("Made {what}", what = plural(n, "song")),
            ..Self::files(false)
        }
    }

    /// `RunResult`'s own words, with or without the sizes line.
    pub fn files(sizes: bool) -> Self {
        Self {
            made: |n| t!("Made {what}", what = plural(n, "file")),
            nothing: t!("Nothing was made"),
            failed: |n| t!("{what} not done:", what = plural(n, "file")),
            sizes,
        }
    }
}

/// "−42%" or "+3%" for a run that made `after` bytes out of `before` (`Math.round` rounds half up).
pub fn change_text(before: u64, after: u64) -> String {
    let change = 100.0 * (before as f64 - after as f64) / before.max(1) as f64;
    let rounded = (change + 0.5).floor().abs();
    format!("{}{rounded}%", if after <= before { "\u{2212}" } else { "+" })
}

/// "12 MB → 4.1 MB (−66%)": the sizes before and after a run, when they are known.
pub fn sizes_line(before: u64, after: u64) -> Option<String> {
    (before > 0).then(|| {
        format!(
            "{} \u{2192} {} ({})",
            format_size(before),
            format_size(after),
            change_text(before, after)
        )
    })
}

/// What a finished run did: made how many, the sizes before and after, how long it took, and what failed.
pub fn result_view(id: &'static str, report: &Report, words: &ResultWords, t: &theme::Tokens) -> impl IntoElement {
    let made = report.made.len();
    let mut headline = if made > 0 {
        (words.made)(made).to_string()
    } else {
        words.nothing.to_string()
    };
    if report.stopped {
        headline.push_str(&t!(" before it was stopped"));
    }
    let headline: SharedString = headline.into();
    let took = format_duration(report.elapsed_ms);
    let details: SharedString = match words.sizes.then(|| sizes_line(report.before, report.after)).flatten() {
        Some(sizes) => format!("{sizes} · {took}").into(),
        None => took.into(),
    };
    let failed = report.failed.clone();
    col()
        .id(id)
        .role(Role::Status)
        .aria_label(headline.clone())
        .test_support()
        .gap(space(3.))
        .child(
            align_row(2.)
                .items_start()
                .child(
                    icon(IconName::CircleCheck)
                        .size_5()
                        .flex_none()
                        .text_color(if made > 0 { t.success } else { t.muted_foreground }),
                )
                .child(
                    col()
                        .align_start()
                        .child(div().font_weight(FontWeight::MEDIUM).child(headline))
                        .when(made > 0, |this| {
                            this.child(
                                div()
                                    .id((id, 1_usize))
                                    .role(Role::Status)
                                    .aria_label(details.clone())
                                    .test_support()
                                    .text_size(theme::size::XS)
                                    .text_color(t.muted_foreground)
                                    .child(details),
                            )
                        }),
                ),
        )
        .when(!failed.is_empty(), |this| {
            let title = (words.failed)(failed.len());
            this.child(
                col()
                    .id((id, 2_usize))
                    .role(Role::List)
                    .aria_label(title.clone())
                    .test_support()
                    .gap(space(0.5))
                    .rounded(theme::size::RADIUS)
                    .bg(t.warning.opacity(0.1))
                    .px(space(3.))
                    .py(space(2.))
                    .text_size(theme::size::XS)
                    .child(div().font_weight(FontWeight::MEDIUM).child(title))
                    .children(failed.iter().take(6).enumerate().map(|(ix, (path, why))| {
                        let line: SharedString = format!("{}: {why}", base_name(path)).into();
                        div()
                            .id((id, 10 + ix))
                            .role(Role::ListItem)
                            .aria_label(line.clone())
                            .test_support()
                            .truncate()
                            .child(isolate(line))
                    }))
                    .when(failed.len() > 6, |this| {
                        this.child(t!("and {n} more", n = format_count(failed.len() - 6)))
                    }),
            )
        })
}

/// What a run that failed as a whole shows: `what` ("Couldn't convert"), and the reason under it.
pub fn error_view(id: &'static str, what: SharedString, reason: SharedString, t: &theme::Tokens) -> impl IntoElement {
    col()
        .id(id)
        .role(Role::Alert)
        .aria_label(format!("{what}. {reason}"))
        .test_support()
        .items_center()
        .gap(space(2.))
        .py(space(6.))
        .text_center()
        .child(icon(IconName::TriangleAlert).size_6().text_color(t.destructive))
        .child(div().font_weight(FontWeight::MEDIUM).child(what))
        .child(
            div()
                .max_w(space(112.))
                .text_size(theme::size::XS)
                .text_color(t.muted_foreground)
                .child(reason),
        )
}

/// What a dialog's footer needs to know.
pub struct FooterSpec {
    /// The ids' stem: "convert" gives "convert-start", "convert-stop", "convert-show", "convert-close", "convert-cancel".
    pub prefix: &'static str,
    /// The action's name ("Convert").
    pub start_label: SharedString,
    /// The action's icon.
    pub start_icon: IconName,
    /// Whether the action can be pressed now (`!disabled`).
    pub can_start: bool,
    /// Whether a run under way can be stopped (all of them can).
    pub stoppable: bool,
    /// What Show reveals once it is done, if anything.
    pub show: Option<String>,
}

/// The dialog's buttons (`RunFooter`): Stop while it runs; Show and Close after; Cancel and the action before.
pub fn footer<V: 'static>(
    spec: FooterSpec,
    run: &Run,
    on_start: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    on_stop: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    t: &theme::Tokens,
    cx: &mut Context<V>,
) -> Div {
    let running = run.running();
    let finished = run.finished();
    let id = |name: &str| SharedString::from(format!("{}-{name}", spec.prefix));
    row_start()
        .justify_end()
        .gap(space(2.))
        .pt(space(3.))
        .border_t_1()
        .border_color(t.border)
        .when(running && spec.stoppable, |this| {
            this.child(
                Button::new(id("stop"))
                    .outline()
                    .icon(IconName::Square)
                    .label(t!("Stop"))
                    .on_click(cx.listener(move |this, _, window, cx| on_stop(this, window, cx))),
            )
        })
        .when_some(spec.show.clone().filter(|_| finished), |this, made| {
            this.child(
                Button::new(id("show"))
                    .outline()
                    .icon(IconName::FolderInput)
                    .label(t!("Show"))
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        reveal(&made, cx);
                    }),
            )
        })
        .when(finished, |this| {
            this.child(
                Button::new(id("close"))
                    .primary()
                    .label(t!("Close"))
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            )
        })
        .when(!finished, |this| {
            this.child(
                Button::new(id("cancel"))
                    .outline()
                    .label(t!("Cancel"))
                    .disabled(running)
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            )
            .child(
                Button::new(id("start"))
                    .primary()
                    .icon(spec.start_icon)
                    .label(spec.start_label.clone())
                    .disabled(running || !spec.can_start)
                    .on_click(cx.listener(move |this, _, window, cx| on_start(this, window, cx))),
            )
        })
}

/// A dialog's frame: the title as a group for screen readers, the folder under it, the body (which scrolls when
/// the dialog is taller than the window), then the footer.
pub fn shell(
    id: &'static str,
    title: SharedString,
    subtitle: SharedString,
    body: impl IntoElement,
    footer: impl IntoElement,
    window: &Window,
    t: &theme::Tokens,
) -> impl IntoElement {
    // The TypeScript's `max-h-[88vh]` for the whole modal: what the title, folder and buttons take is left out.
    let room = (window.viewport_size().height * 0.88 - px(190.)).max(px(160.));
    col()
        .id(id)
        .role(Role::Group)
        .aria_label(title)
        .test_support()
        .gap(space(3.))
        .text_size(theme::size::SM)
        // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
        .text_start()
        .pt(space(1.))
        .child(
            div()
                .id((id, 900_usize))
                .role(Role::Group)
                .aria_label(subtitle.clone())
                .test_support()
                .text_size(theme::size::XS)
                .text_color(t.muted_foreground)
                .truncate()
                .child(isolate(subtitle)),
        )
        .child(
            div()
                .id((id, 901_usize))
                .max_h(room)
                .overflow_y_scroll()
                .child(col().gap(space(4.)).child(body)),
        )
        .child(footer)
}

/// Where the files are: their folder, or how many folders.
pub fn where_they_are(files: &[String]) -> SharedString {
    let folders = folders_of(files);
    match (folders.len(), files.first()) {
        (1, Some(first)) => parent_path(first).into(),
        _ => t!("In {n} folders", n = format_count(folders.len())),
    }
}

/// The names of the distinct folders among `files`, lower-cased.
pub fn folders_of(files: &[String]) -> HashSet<String> {
    files.iter().map(|f| parent_path(f).to_lowercase()).collect()
}

/// The folder line under a dialog's title: the folder they are in, or how many folders.
pub fn folder_line(id: &'static str, files: &[String], t: &theme::Tokens) -> impl IntoElement {
    let text = where_they_are(files);
    div()
        .id(id)
        .role(Role::Group)
        .aria_label(text.clone())
        .test_support()
        .text_size(theme::size::XS)
        .text_color(t.muted_foreground)
        .truncate()
        .child(isolate(text))
}

/// The height of a dialog's scrolling body: a share of the window, so a short window scrolls it.
pub fn body_height(window: &Window) -> Pixels {
    (window.viewport_size().height * 0.72).max(px(240.))
}

/// The tools backend core offers (the install cards), or none when core isn't started.
pub fn core_tools(cx: &App) -> Option<Arc<dyn SettingsBackend>> {
    let core = cx.try_global::<core_bridge::Core>()?;
    Some(Arc::new(SettingsCoreBackend::new(core.app.clone())))
}

// ---- Tools --------------------------------------------------------------------------------------------------

/// The tools model the install cards read, made with a window (`None` without core, which never happens in the
/// app). It looks for every tool as it is made.
pub fn tools_model(window: &mut Window, cx: &mut App) -> Option<Entity<Tools>> {
    let core = cx.try_global::<core_bridge::Core>()?;
    let backend: Arc<dyn SettingsBackend> = Arc::new(SettingsCoreBackend::new(core.app.clone()));
    Some(cx.new(|cx| Tools::new(backend, window, cx)))
}

/// The tools a dialog needs and the cards that install them (`useTool`, `ToolNeeded`).
pub struct Tooling {
    tools: Option<Entity<Tools>>,
    /// With no tools model at all, every tool counts as there (a dialog started without core's tools, as in the
    /// tests that don't care); [`Tooling::new`] counts none.
    assume_ready: bool,
    /// Tests say which tools are there instead (the settings' `Backend` can't be faked from outside it:
    /// `PORT ME(coordinator)`: `pub use backend::Fetch` in settings/mod.rs, then a fake can be written).
    #[cfg(test)]
    forced: Option<Vec<&'static str>>,
}

impl Tooling {
    /// Reads the tools from `tools`.
    pub fn new(tools: Option<Entity<Tools>>) -> Self {
        Self {
            tools,
            assume_ready: false,
            #[cfg(test)]
            forced: None,
        }
    }

    /// Looks for the tools through `backend`; with none (nothing to wait for) every tool counts as there.
    pub fn from_backend<V: 'static>(
        backend: Option<Arc<dyn SettingsBackend>>,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Self {
        let tools = backend.map(|backend| cx.new(|cx| Tools::new(backend, window, cx)));
        Self {
            tools,
            assume_ready: true,
            #[cfg(test)]
            forced: None,
        }
    }

    /// A tooling whose tools are never found, so [`Tooling::ready`] stays false (the state before the first look
    /// for the tools, or with one missing).
    #[cfg(test)]
    pub fn never() -> Self {
        Self::with(&[])
    }

    /// Runs `on_change` whenever the tools are looked for again or one is installed. Keep the subscription.
    pub fn observe<V: 'static>(
        &self,
        cx: &mut Context<V>,
        on_change: impl Fn(&mut V, &mut Context<V>) + 'static,
    ) -> Option<Subscription> {
        let tools = self.tools.as_ref()?;
        Some(cx.observe(tools, move |this, _, cx| on_change(this, cx)))
    }

    /// A tooling in which exactly the tools `ids` are installed (tests).
    #[cfg(test)]
    pub fn with(ids: &[&'static str]) -> Self {
        Self {
            tools: None,
            assume_ready: false,
            forced: Some(ids.to_vec()),
        }
    }

    /// The model, to redraw when it changes.
    pub fn model(&self) -> Option<&Entity<Tools>> {
        self.tools.as_ref()
    }

    /// Whether the tool `id` is there (`!!tool?.origin`); false until the tools are known.
    pub fn ready(&self, id: &str, cx: &App) -> bool {
        #[cfg(test)]
        if let Some(forced) = &self.forced {
            return forced.contains(&id);
        }
        match &self.tools {
            None => self.assume_ready,
            Some(tools) => tools.read(cx).tool(id).is_some_and(|info| info.origin.is_some()),
        }
    }

    /// The card that says a feature needs the tool `id` and installs it (`ToolNeeded`); `None` once it is there,
    /// or before the tools are known.
    pub fn card(&self, id: &str, why: SharedString, icon_name: IconName, cx: &App) -> Option<AnyElement> {
        tools::needed(self.tools.as_ref()?, id, why, icon_name, cx)
    }

    /// A tool's name as the settings know it ("pandoc"), for sentences; the id until they are known.
    pub fn name(&self, id: &str, cx: &App) -> String {
        self.tools
            .as_ref()
            .and_then(|tools| tools.read(cx).tool(id).map(|info| info.name.clone()))
            .unwrap_or_else(|| id.to_string())
    }
}

/// Redraws `view` whenever `tooling`'s tools change (an install finishing makes the action available).
pub fn watch_tools<V: 'static>(tooling: &Tooling, cx: &mut Context<V>) -> Option<Subscription> {
    tooling.model().map(|tools| cx.observe(tools, |_, _, cx| cx.notify()))
}
