//! Ctrl+K: commands, folders by frecency, the calculator (CommandPalette.tsx, History.tsx).
//!
//! Ported from src/components/explorer/CommandPalette.tsx and `paletteCommands` in Explorer.tsx. (History.tsx is
//! the search history's popover; the palette never used it, so nothing of it is here.)
//!
//! Type a few letters of a folder you have been to or of a command and Enter takes you there or does it. Text that
//! looks like a path (`C:\…`, `\\server`, `~`, `%APPDATA%`, `.\…`) gets a "Go to" line first. `=` and a sum is
//! worked out by the calculator as you type (calc.rs in glint-core), its answer the one result, and Enter copies it.
//!
//! | TypeScript                         | Here                                                   |
//! |------------------------------------|--------------------------------------------------------|
//! | `CommandPalette`                   | [`Palette`], opened with [`open`] / [`toggle`]          |
//! | `Command`, `paletteCommands()`     | [`commands::Command`], [`commands::all`]                |
//! | ranking, `looksLikePath`, `=`      | [`commands::results`], [`commands::rank`]               |
//! | `FuzzyText`                        | [`commands::highlight_ranges`] drawn by the view        |
//! | `calculate`                        | a [`Sources::calc`] (core's `calc::calculate`)          |
//! | `usedScore`, `usedFolders`         | in [`commands`], with zoxide's folders from [`Sources`] |
//!
//! It is a dialog (the kit's dialog layer: the window behind is covered, Escape closes, focus returns where it
//! was), without a title, placed near the top as the web one was. The keys inside it (arrows, Enter, Escape, Tab)
//! are bound here, not in `actions::BINDINGS`: they only mean something while the palette has the focus.
//!
//! **To wire it:** the workspace's `CommandPalette` action handler calls [`toggle`] (or [`open`]).
//!
//! The commands that do things to files and the tools do what their menu items do: [`commands::Run::Menu`] hands
//! the same [`MenuEvent`](crate::components::menus::MenuEvent) to the main window
//! ([`crate::workspace::run_menu_command`]); "Run <task>" types the task into the terminal pane
//! ([`crate::workspace::run_task`]); the commands that only change the explorer or a setting (Even out the panes,
//! Invert selection, Get access, Move the tab) are [`commands::Change`]s. What is not here: "Convert the selected
//! text files to UTF-8", which waits for the workspace to offer it (nothing the palette can call runs
//! `text_to_utf8` and offers its Undo yet), and the "Ctrl+N" shown beside "New window" (no key is bound to it).

pub mod commands;
mod keys;

#[cfg(test)]
mod tests;

use self::commands::{Command, Inputs, Kind, Run};
use self::keys::{PaletteAccept, PaletteCancel, PaletteDown, PaletteKeep, PaletteUp};
use crate::components::command_bar::Facts;
use crate::explorer::{explorer, NavOptions};
use crate::kit::prelude::*;
use crate::{core_bridge, persistence, state, t, theme};
use gpui_kit::{ClipboardItem, HighlightStyle, ScrollStrategy, StyledText, Task, UniformListScrollHandle, WeakEntity};
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// How long after the last key the calculator is asked (the TypeScript's 60 ms): typing a long sum asks once.
const CALC_DEBOUNCE: Duration = Duration::from_millis(60);
/// The palette's width (`max-w-[40rem]`).
const WIDTH: f32 = 640.;
/// How far from the top it sits, of the window's height (`pt-[12vh]`).
const TOP: f32 = 0.12;
/// Rows seen at once before the list scrolls (`max-h-[min(60vh,26rem)]`).
const MOST_ROWS: usize = 11;

/// Works out an expression in the background (the calculator).
pub type CalcSource = Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>> + Send + Sync>;
/// zoxide's folders and their scores, or none when it isn't installed.
pub type ZoxideSource =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<Option<Vec<(String, f64)>>, String>> + Send>> + Send + Sync>;

/// What the tools of the folder are, read in the background: the scripts, the editors, the projects' tasks.
#[derive(Clone, Debug, Default)]
pub struct Tools {
    /// The Scripts folder's scripts.
    pub scripts: Vec<glint_core::scripts::Script>,
    /// The editors and terminals found on this PC.
    pub openers: Vec<glint_core::openers::Opener>,
    /// What the folder's projects can run.
    pub tasks: Vec<glint_core::projects::TaskGroup>,
}

/// Reads the [`Tools`] for a folder.
pub type ToolsSource = Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Tools> + Send>> + Send + Sync>;

/// Where the palette gets what takes work: glint-core in the app, fakes in tests.
#[derive(Clone)]
pub struct Sources {
    /// The calculator.
    pub calc: CalcSource,
    /// zoxide's folders, asked for when history is shared with it (Settings → History).
    pub zoxide: ZoxideSource,
    /// The scripts, editors and tasks, asked for when the palette opens.
    pub tools: ToolsSource,
    /// How long to wait after a key before asking the calculator.
    pub debounce: Duration,
}

impl Sources {
    /// glint-core's `calc::calculate` and `zoxide::zoxide_folders`.
    pub fn core() -> Self {
        Sources {
            calc: Arc::new(|expression| Box::pin(glint_core::calc::calculate(expression))),
            zoxide: Arc::new(|| Box::pin(glint_core::zoxide::zoxide_folders())),
            tools: Arc::new(|folder| {
                Box::pin(async move {
                    Tools {
                        scripts: glint_core::scripts::scripts_list()
                            .await
                            .map(|found| found.scripts)
                            .unwrap_or_default(),
                        openers: glint_core::openers::openers().await.unwrap_or_default(),
                        tasks: glint_core::projects::project_tasks(folder).await.unwrap_or_default(),
                    }
                })
            }),
            debounce: CALC_DEBOUNCE,
        }
    }
}

/// One line of the list as drawn: a heading, or a result (by its place in the results).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    Header(&'static str),
    Item(usize),
}

/// The palette: a search box over a list of results.
pub struct Palette {
    inputs: Inputs,
    sources: Sources,
    /// Every folder and command on offer.
    all: Vec<Command>,
    input: Entity<InputState>,
    /// What the box held when the results were last made.
    query: SharedString,
    results: Vec<Command>,
    rows: Vec<Row>,
    /// The highlighted result.
    active: usize,
    scroll: UniformListScrollHandle,
    answer: Option<commands::Answer>,
    calc_task: Option<Task<()>>,
    _zoxide_task: Option<Task<()>>,
    _tools_task: Option<Task<()>>,
    _subscription: Subscription,
}

impl Palette {
    /// A palette over `inputs`, its box focused. Asks for zoxide's folders when `inputs` says they're shared.
    pub fn new(inputs: Inputs, sources: Sources, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx
            .new(|cx| InputState::new(window, cx).placeholder(t!("Type a folder, a path or a command… (= for a sum)")));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.typed(cx);
            }
        });
        let all = commands::all(&inputs);
        let mut palette = Palette {
            inputs,
            sources,
            all,
            input: input.clone(),
            query: SharedString::default(),
            results: Vec::new(),
            rows: Vec::new(),
            active: 0,
            scroll: UniformListScrollHandle::new(),
            answer: None,
            calc_task: None,
            _zoxide_task: None,
            _tools_task: None,
            _subscription: subscription,
        };
        palette.remake_results();
        if state::settings(cx).zoxide {
            palette._zoxide_task = Some(palette.ask_zoxide(cx));
        }
        palette._tools_task = Some(palette.ask_tools(cx));
        // The dialog layer takes the focus when it opens; the box takes it from there.
        window.defer(cx, move |window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx));
        });
        palette
    }

    /// Reads zoxide's folders again (each time the palette opens) and adds them to the list.
    fn ask_zoxide(&self, cx: &mut Context<Self>) -> Task<()> {
        let asking = (self.sources.zoxide)();
        cx.spawn(async move |this, cx| {
            let folders = core_bridge::run(asking).await.and_then(|r| r).ok().flatten();
            if let Some(folders) = folders {
                this.update(cx, |this, cx| {
                    this.inputs.zoxide = folders;
                    this.all = commands::all(&this.inputs);
                    this.remake_results();
                    cx.notify();
                })
                .ok();
            }
        })
    }

    /// Reads the scripts, editors and tasks (each time the palette opens) and adds their commands to the list.
    fn ask_tools(&self, cx: &mut Context<Self>) -> Task<()> {
        let asking = (self.sources.tools)(self.inputs.path.clone());
        cx.spawn(async move |this, cx| {
            let Ok(tools) = core_bridge::run(asking).await else {
                return;
            };
            this.update(cx, |this, cx| {
                this.inputs.scripts = tools.scripts;
                this.inputs.openers = tools.openers;
                this.inputs.tasks = tools.tasks;
                this.all = commands::all(&this.inputs);
                this.remake_results();
                cx.notify();
            })
            .ok();
        })
    }

    /// The text in the box.
    fn text(&self, cx: &App) -> SharedString {
        self.input.read(cx).value()
    }

    /// The box changed: new results from the top, and the calculator asked if it's a sum.
    fn typed(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        if text == self.query {
            return;
        }
        self.query = text;
        self.active = 0;
        self.calc_task = None;
        if let Some(expression) = commands::expression_of(&self.query).filter(|e| !e.is_empty()) {
            let already = self.answer.as_ref().is_some_and(|a| a.expression == expression);
            if !already {
                self.calc_task = Some(self.ask_calculator(expression, cx));
            }
        }
        self.remake_results();
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Works `expression` out in the background and, if it's still the sum in the box, shows the answer.
    fn ask_calculator(&self, expression: String, cx: &mut Context<Self>) -> Task<()> {
        let (calc, debounce) = (self.sources.calc.clone(), self.sources.debounce);
        cx.spawn(async move |this, cx| {
            if !debounce.is_zero() {
                cx.background_executor().timer(debounce).await;
            }
            let outcome = core_bridge::run(calc(expression.clone())).await.and_then(|r| r);
            let (text, ok) = match outcome {
                Ok(text) => (text, true),
                Err(reason) => (reason, false),
            };
            this.update(cx, |this, cx| {
                this.answer = Some(commands::Answer { expression, text, ok });
                this.remake_results();
                cx.notify();
            })
            .ok();
        })
    }

    /// Ranks again for the box's text, keeping the highlight on the same row when it still exists.
    fn remake_results(&mut self) {
        self.results = commands::results(&self.all, &self.query, self.answer.as_ref());
        self.rows = rows_for(&self.results, self.query.trim().is_empty());
        self.active = self.active.min(self.results.len().saturating_sub(1));
    }

    /// The row an item is drawn on.
    fn row_of(&self, item: usize) -> usize {
        self.rows.iter().position(|row| *row == Row::Item(item)).unwrap_or(0)
    }

    /// Moves the highlight `step` results, round the ends.
    fn step(&mut self, step: isize, cx: &mut Context<Self>) {
        let n = self.results.len() as isize;
        if n == 0 {
            return;
        }
        self.active = (self.active as isize + step).rem_euclid(n) as usize;
        self.scroll
            .scroll_to_item(self.row_of(self.active), ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Closes the palette, then does what result `ix` says.
    fn choose(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = self.results.get(ix).cloned() else {
            return;
        };
        window.close_dialog(cx);
        carry_out(command.run, window, cx);
    }

    /// One row of the list.
    fn row(&self, row: &Row, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        match row {
            Row::Header(text) => div()
                .h(ROW)
                .flex()
                .items_end()
                .px(space(2.5))
                .pb(space(1.))
                .text_size(theme::size::XS)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.muted_foreground)
                .child(crate::i18n::translate(text))
                .into_any_element(),
            Row::Item(item) => {
                let Some(command) = self.results.get(*item) else {
                    return div().h(ROW).into_any_element();
                };
                let active = *item == self.active;
                let item = *item;
                // A match in the label is highlighted; a folder's path, where the letters must be close together.
                let typed = if commands::expression_of(&self.query).is_some() {
                    SharedString::default()
                } else {
                    self.query.clone()
                };
                let tone = command.tone.and_then(theme::tailwind).unwrap_or(t.muted_foreground);
                let highlight = |text: &SharedString, tight: bool| {
                    let ranges: Vec<(Range<usize>, HighlightStyle)> = if command.id == "go" {
                        Vec::new()
                    } else {
                        commands::highlight_ranges(text, &typed, tight)
                            .into_iter()
                            .map(|range| {
                                (
                                    range,
                                    HighlightStyle {
                                        color: Some(t.primary),
                                        font_weight: Some(FontWeight::SEMIBOLD),
                                        ..Default::default()
                                    },
                                )
                            })
                            .collect()
                    };
                    StyledText::new(isolated(text)).with_highlights(shift(ranges, text, &isolated(text)))
                };
                let mut line = row_start()
                    .id(("palette-item", item))
                    .role(Role::ListBoxOption)
                    .aria_label(command.label.clone())
                    .aria_selected(active)
                    .test_support()
                    .h(ROW)
                    .w_full()
                    .px(space(2.5))
                    .gap(space(3.))
                    .rounded(theme::size::RADIUS)
                    .text_size(theme::size::SM)
                    .cursor_pointer()
                    .when(active, |this| this.bg(t.accent))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.active != item {
                            this.active = item;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| this.choose(item, window, cx)))
                    .child(icon(command.icon).size_4().flex_none().text_color(tone))
                    // A long label is cut short too (not scrolled sideways), once its detail is gone.
                    .child(
                        div()
                            .min_w_0()
                            .flex_none()
                            .max_w_full()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(highlight(&command.label, false)),
                    );
                if let Some(detail) = &command.detail {
                    line = line.child(
                        div()
                            .min_w_0()
                            .flex_shrink(1.)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(highlight(detail, true)),
                    );
                }
                if let Some(detail) = &command.detail {
                    line = line.aria_description(detail.clone());
                }
                line.child(div().flex_1())
                    .children(command.shortcut.clone().map(|shortcut| {
                        div()
                            .flex_none()
                            .ps(4.)
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(shortcut)
                    }))
                    .into_any_element()
            }
        }
    }
}

/// A row's height: the heading rows are the same, so the list is uniform and scrolls cheaply.
const ROW: Rems = space_const(9.);

/// [`space`] as a constant.
const fn space_const(n: f32) -> Rems {
    gpui_kit::rems(n * 0.25)
}

/// `text` for drawing: an Arabic name is its own right-to-left paragraph (see [`isolate`]).
fn isolated(text: &SharedString) -> SharedString {
    isolate(text.clone())
}

/// Highlight ranges of `text` moved to where the same characters are in `drawn` (which [`isolate`] may have
/// wrapped in marks).
fn shift(
    ranges: Vec<(Range<usize>, HighlightStyle)>,
    text: &SharedString,
    drawn: &SharedString,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let by = drawn.len().saturating_sub(text.len()) / 2;
    ranges
        .into_iter()
        .map(|(range, style)| (range.start + by..range.end + by, style))
        .collect()
}

/// The list's lines: before anything's typed, where to go then what to do, each under a heading.
fn rows_for(results: &[Command], empty: bool) -> Vec<Row> {
    let mut rows = Vec::with_capacity(results.len() + 2);
    for (i, command) in results.iter().enumerate() {
        let new_kind = i == 0 || results[i - 1].kind != command.kind;
        if empty && new_kind && !is_calculator(command) {
            rows.push(Row::Header(if command.kind == Kind::Folder {
                "Go to"
            } else {
                "Commands"
            }));
        }
        rows.push(Row::Item(i));
    }
    rows
}

fn is_calculator(command: &Command) -> bool {
    command.id.starts_with("calc")
}

impl Render for Palette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let rows = self.rows.len();
        let typed = self.query.trim().to_string();

        let list = if self.results.is_empty() {
            div()
                .id("palette-empty")
                .role(Role::Status)
                .aria_label(t!("Nothing matches “{query}”", query = typed.clone()))
                .test_support()
                .px(space(3.))
                .py(space(6.))
                .text_center()
                .text_size(theme::size::SM)
                .text_color(t.muted_foreground)
                .child(t!("Nothing matches “{query}”", query = typed))
                .into_any_element()
        } else {
            uniform_list(
                "palette-results",
                rows,
                cx.processor(|this, range: Range<usize>, _, cx| {
                    let t = theme::tokens(cx).clone();
                    range
                        .filter_map(|ix| this.rows.get(ix).cloned())
                        .map(|row| this.row(&row, &t, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .h(ROW * rows.min(MOST_ROWS) as f32)
            .into_any_element()
        };
        let _ = window;

        col()
            .id("palette")
            .role(Role::Group)
            .aria_label(t!("Command palette"))
            .test_support()
            .key_context(keys::CONTEXT)
            .size_full()
            .overflow_hidden()
            .text_start()
            .on_action(cx.listener(|this, _: &PaletteDown, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &PaletteUp, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &PaletteAccept, window, cx| {
                let at = this.active;
                this.choose(at, window, cx);
            }))
            .on_action(cx.listener(|_, _: &PaletteCancel, window, cx| window.close_dialog(cx)))
            // The box keeps the keyboard; the arrows move through the list.
            .on_action(cx.listener(|_, _: &PaletteKeep, _, _| {}))
            .child(
                align_row(2.5)
                    .px(space(3.5))
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        icon(IconName::Search)
                            .size_4()
                            .flex_none()
                            .text_color(t.muted_foreground),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.input)
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .aria_label(t!("Command or folder")),
                        ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px(space(1.5))
                            .py(space(0.5))
                            .rounded(theme::size::RADIUS_SM)
                            .border_1()
                            .border_color(t.border)
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .child("Esc"),
                    ),
            )
            .child(
                div()
                    .id("palette-list")
                    .role(Role::ListBox)
                    .aria_label(t!("Results"))
                    .test_support()
                    .p(space(1.5))
                    .child(list),
            )
    }
}

// ---- Doing what was chosen ----------------------------------------------------------------------------

/// Does what a result says. The palette is already closed, so the keyboard is back where it was.
fn carry_out(run: Run, window: &mut Window, cx: &mut App) {
    match run {
        Run::Nothing => {}
        Run::Navigate(path) => {
            explorer(cx).update(cx, |e, cx| e.navigate(path, NavOptions::default(), cx));
        }
        Run::GoTo(text) => {
            explorer(cx).update(cx, |e, cx| e.go_to(&text, cx).detach());
        }
        Run::Copy(text) => {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            window.push_notification(Notification::success(t!("Copied {text}", text = text)), cx);
        }
        // Dispatched once the palette's layer is gone, so the action reaches the view that had the keyboard.
        Run::Action(make) => {
            let action = make();
            window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
        }
        Run::Change(change) => apply(change, cx),
        // What a menu item does, once the palette's layer is gone (a dialog opens over the window).
        Run::Menu(event) => {
            window.defer(cx, move |window, cx| {
                crate::workspace::run_menu_command(event, window, cx)
            });
        }
        Run::Saved(saved) => {
            explorer(cx).update(cx, |e, cx| e.run_saved_search(saved, cx).detach());
        }
        // PORT ME(coordinator): nothing the palette can call runs a task in the terminal pane yet, so the palette
        // is not given any (`Sources::core`); this is what it will do.
        Run::Task { folder, command } => {
            window.defer(cx, move |window, cx| {
                crate::workspace::run_task(folder, command, window, cx)
            });
        }
        Run::OpenIn { opener, name, path } => report(
            t!("Couldn't open it in {name}", name = name),
            glint_core::openers::open_in(opener, path),
            window,
            cx,
        ),
        Run::Terminal(folder) => report(
            t!("Couldn't open a terminal"),
            glint_core::shell::open_terminal(folder),
            window,
            cx,
        ),
        Run::FileExplorer(folder) => {
            // File Explorer's own "open" verb can take a moment: off the UI thread.
            cx.background_spawn(async move { crate::platform::launch::hand_to_explorer(&[folder]) })
                .detach();
        }
        Run::Properties(path) => show_properties(path, cx),
        Run::Share(view) => window.defer(cx, move |window, cx| {
            let view = match view {
                commands::ShareView::Send => crate::dialogs::share::View::Send,
                commands::ShareView::Network => crate::dialogs::share::View::Network,
            };
            crate::dialogs::share::open_view(window, cx, view);
        }),
        Run::AddRemote => window.defer(cx, crate::dialogs::remote::open_add),
        Run::Rules(folder) => window.defer(cx, move |window, cx| crate::dialogs::rules::open(window, cx, folder)),
    }
}

/// Runs a core call without waiting, and tells the person `what` failed if it did.
fn report(
    what: SharedString,
    work: impl Future<Output = Result<(), String>> + Send + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    window
        .spawn(cx, async move |cx| {
            if let Err(reason) = core_bridge::run(work).await.and_then(|inner| inner) {
                let _ = cx.update(|window, cx| {
                    window.push_notification(Notification::error(reason).title(what), cx);
                });
            }
        })
        .detach();
}

/// A [`commands::Change`].
fn apply(change: commands::Change, cx: &mut App) {
    use commands::Change as C;
    match change {
        C::CloseOthers(tab) => {
            explorer(cx).update(cx, |e, cx| e.close_other_tabs(tab, cx));
        }
        C::Duplicate(tab) => {
            explorer(cx).update(cx, |e, cx| e.duplicate_tab(tab, cx));
        }
        C::SideBySide(path) => {
            explorer(cx).update(cx, |e, cx| e.open_in_other_pane(&path, None, cx).detach());
        }
        C::TogglePin(path) => persistence::update_app(cx, |app| app.toggle_pin(&path)),
        C::Group(by) => {
            state::update(cx, |settings| settings.group_by = by);
        }
        C::Tiles(size) => {
            state::update(cx, |settings| settings.tile_size = size);
            explorer(cx).update(cx, |e, cx| e.set_view(glint_model::store::ViewMode::Grid, None, cx));
        }
        C::Theme(theme) => state::update(cx, |settings| settings.theme = theme),
        C::EvenPanes => state::update(cx, |settings| settings.dual_split = None),
        C::InvertSelection => {
            explorer(cx).update(cx, |e, cx| {
                if let Some(rows) = e.active_tab().map(|tab| tab.rows.clone()) {
                    e.invert_selection(&rows, cx);
                }
            });
        }
        C::GetAccess(tab) => {
            explorer(cx).update(cx, |e, cx| e.get_access(Some(tab), cx).detach());
        }
        C::MoveTab(tab) => {
            explorer(cx).update(cx, |e, cx| e.move_tab_to_other_pane(tab, cx));
        }
    }
}

/// Windows' own Properties dialog for `path`, which runs beside the window (`properties`).
fn show_properties(path: String, cx: &mut App) {
    use glint_core::host::Manager as _;
    let Some(main) = cx
        .try_global::<core_bridge::Core>()
        .and_then(|core| core.app.get_webview_window(crate::workspace::MAIN))
    else {
        return;
    };
    let shown = core_bridge::run(glint_core::shell::show_properties(main, path));
    cx.spawn(async move |_| {
        if let Ok(Err(error)) = shown.await {
            eprintln!("glint: couldn't show the properties: {error}");
        }
    })
    .detach();
}

// ---- What is on offer ---------------------------------------------------------------------------------

/// Reads what the list is built from: the active tab, the pinned folders, the folders used, the settings.
///
/// Takes the app for writing because two of the lists are asked for (File Explorer's New menu kinds start to be read
/// in the background; the Send, receive or share model is made if it wasn't): a kind that has not been read yet
/// shows from the next time the palette opens.
pub fn read_inputs(cx: &mut App) -> Inputs {
    let explorer = explorer(cx);
    let kinds = explorer.update(cx, |e, cx| e.new_kinds(cx));
    let sharing = crate::dialogs::share::shared(cx);
    let (sending, serving) = {
        let sharing = sharing.read(cx);
        (!sharing.sends().is_empty(), !sharing.serving().is_empty())
    };
    let e = explorer.read(cx);
    let tab = e.active_tab();
    let app = persistence::app(cx);
    let settings = state::settings(cx);
    Inputs {
        path: tab.map(|t| t.path.clone()).unwrap_or_default(),
        finding: tab.is_some_and(|t| t.find.is_some()),
        in_archive: tab.is_some_and(|t| t.archive().is_some()),
        shut: tab.is_some_and(|t| t.no_access().is_some()),
        grid: tab.is_some_and(|t| t.view == glint_model::store::ViewMode::Grid),
        tab: tab.map(|t| t.id),
        tabs: e.panes().get(e.active_pane()).map_or(0, |pane| pane.tabs.len()),
        dual: e.is_dual(),
        can_reopen: e.can_reopen_tab(),
        can_back: tab.is_some_and(|t| !t.back.is_empty()),
        can_forward: tab.is_some_and(|t| !t.forward.is_empty()),
        chosen: tab.is_some_and(|t| !t.selection.is_empty()),
        focused: tab.is_some_and(|t| t.selection.focus().is_some()),
        pinned: app.map(|a| a.pinned.clone()).unwrap_or_default(),
        known: e.known_folders().iter().map(|k| (k.id, k.path.clone())).collect(),
        drives: e.drives().to_vec(),
        remotes: app.map(|a| a.remotes.clone()).unwrap_or_default(),
        visits: app
            .map(|a| {
                a.visits
                    .iter()
                    .map(|(path, visit)| (path.to_string(), *visit))
                    .collect()
            })
            .unwrap_or_default(),
        zoxide: Vec::new(),
        show_hidden: settings.show_hidden,
        show_preview: settings.show_preview_pane,
        tile_size: Some(settings.tile_size),
        dark: theme::tokens(cx).dark,
        now: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
        mapped: tab.is_some_and(|t| crate::components::disk_usage::shows(cx, t.id)),
        chosen_entries: tab
            .filter(|t| !t.selection.is_empty())
            .map(|t| {
                t.rows
                    .iter()
                    .filter(|entry| t.selection.contains(&entry.path))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default(),
        selected: tab.map(|t| t.selection.to_vec()).unwrap_or_default(),
        focus_path: tab.and_then(|t| t.selection.focus().map(str::to_string)),
        other_pane: e.other_pane_folder().map(|(_, path)| path),
        undo: e.next_undo().map(|(_, label)| label.to_string()),
        pair: Facts::read(e, false, settings)
            .pair
            .map(|p| ((p.left, p.left_name), (p.right, p.right_name))),
        archive_outer: tab.and_then(|t| t.archive().map(|a| a.outer.clone())),
        saved_searches: app.map(|a| a.saved_searches.clone()).unwrap_or_default(),
        scripts: Vec::new(),
        openers: Vec::new(),
        tasks: Vec::new(),
        unlisted: tab.is_some_and(|t| t.no_access().is_some() && t.shared_entries().is_none()),
        can_get_access: tab.is_some_and(|t| t.no_access().is_some_and(|n| !n.remote && !n.refused)),
        uneven: settings.dual_split.is_some(),
        archive_top: tab.and_then(|t| {
            let archive = t.archive()?;
            let same = |a: &str, b: &str| a.to_lowercase() == b.to_lowercase();
            (same(&t.path, &archive.path) && same(&archive.path, &archive.outer)).then(|| commands::ArchiveTop {
                outer: archive.outer.clone(),
                single_file: glint_model::archives::SINGLE_FILE.contains(&archive.format),
            })
        }),
        new_kinds: kinds.iter().map(|kind| (kind.ext.clone(), kind.name.clone())).collect(),
        sending,
        serving,
    }
}

// ---- Opening it ---------------------------------------------------------------------------------------

/// The palette that is open, if one is.
struct Current(WeakEntity<Palette>);

impl Global for Current {}

/// Whether the palette is showing in `window`.
pub fn is_open(window: &mut Window, cx: &mut App) -> bool {
    cx.try_global::<Current>()
        .is_some_and(|current| current.0.upgrade().is_some())
        && window.has_active_dialog(cx)
}

/// Opens the palette over `window` (Ctrl+K), listing what applies to the active tab. Does nothing if it's
/// already open.
pub fn open(window: &mut Window, cx: &mut App) {
    open_with(window, cx, Sources::core());
}

/// Debug builds' self-test: types `text` into the open palette's box, as a person would.
pub(crate) fn type_query(text: &str, window: &mut Window, cx: &mut App) {
    let Some(palette) = cx.try_global::<Current>().and_then(|current| current.0.upgrade()) else {
        return;
    };
    let input = palette.read(cx).input.clone();
    input.update(cx, |input, cx| input.set_value(text.to_string(), window, cx));
    palette.update(cx, |palette, cx| palette.typed(cx));
}

/// Opens the palette when it's closed, closes it when it's open (the Ctrl+K action).
pub fn toggle(window: &mut Window, cx: &mut App) {
    if is_open(window, cx) {
        window.close_dialog(cx);
    } else {
        open(window, cx);
    }
}

/// [`open`] with sources of your own (tests).
pub fn open_with(window: &mut Window, cx: &mut App, sources: Sources) {
    if is_open(window, cx) {
        return;
    }
    keys::ensure_bound(cx);
    let inputs = read_inputs(cx);
    let view = cx.new(|cx| Palette::new(inputs, sources, window, cx));
    cx.set_global(Current(view.downgrade()));
    let top = window.viewport_size().height * TOP;
    window.open_dialog(cx, move |dialog, _window, _cx| {
        dialog
            .w(px(WIDTH))
            .margin_top(top)
            .close_button(false)
            .p_0()
            .child(view.clone())
    });
}
