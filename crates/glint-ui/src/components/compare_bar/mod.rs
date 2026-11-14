//! The bars over a pane while the two panes are compared: the compare bar (how the folders differ, the options,
//! Select differences, Sync, Stop) and the bar for two files selected across the panes. Wave 14 (compare-bar).
//! Ported from the two bars of `Explorer.tsx` (`crossPair`, `compareOptions`, `selectDiffs`, `syncToOther`, and the
//! JSX between the search bar and the README card).
//!
//! | TypeScript                                           | Here                                                     |
//! |------------------------------------------------------|----------------------------------------------------------|
//! | `crossPair`                                          | [`cross_pair`]                                           |
//! | which bar a pane shows, and what is in it            | [`bar_of`] -> [`Bar`] (plain data, tested without a window) |
//! | the bar's JSX                                        | [`CompareBar`], one view per pane                        |
//! | `compareOptions` (the menu under the sliders button) | the options menu, drawn by the shared [`MenuView`]       |
//! | `selectDiffs`                                        | [`Explorer::select_differences`]                         |
//! | `syncToOther`, `syncInto`                            | [`Explorer::sync_pane`], [`Explorer::sync_into`]         |
//! | `runDeepCompare`, `stopDeepCompare`, `deepKey`       | the explorer's compare part (`explorer/compare.rs`)      |
//!
//! # The two bars
//!
//! - **Selected across panes** (`dual && !comparePanes && crossPair && current`): one file selected in each pane,
//!   shown over the active pane, with **Compare files**.
//! - **The compare bar** (`dual && comparePanes && !find && isFolderPath(path)`): with a comparison, the badges
//!   (N only here, N newer, N different, N older, or All match), "N same", a spinner while the deeper comparison is
//!   worked out or "Couldn't compare deeper: ..." when it failed, then at the end: the pair's **Compare**, the
//!   options (Subfolders too, File contents, Only the differences), **Select differences (N)**, **Sync N** and
//!   Stop. Without one it says why ("Open a folder in the other pane to compare"...).
//!
//! # How the coordinator wires it
//!
//! One [`CompareBar`] per pane, made with `cx.new(|cx| CompareBar::new(pane, window, cx))` beside the pane's other
//! views, and drawn **above the file view** (between the tab strip and the README card, as in `Explorer.tsx`) as a
//! `flex-none` block. It draws nothing when its pane needs no bar, so it may be placed always. It opens the compare
//! files dialog itself and changes the settings itself; it emits no events.
//!
//! # Differences from the TypeScript
//!
//! - The Sync arrow follows the reading direction: the left pane's Sync points toward the other pane, which is on
//!   the left in the Arabic interface.
//! - The bar is built from the explorer's public state ([`bar_of`]); the explorer keeps no copy of it.

#[cfg(test)]
mod tests;

use crate::actions;
use crate::components::command_bar::keyed;
use crate::components::git::Tone;
use crate::components::menus::{on_disk, Entry, MenuView, MenuViewEvent, Node, Run, Task};
use crate::components::preview;
use crate::explorer::{explorer, Comparison, Explorer};
use crate::kit::prelude::*;
use crate::state::UiState;
use crate::{i18n, state, t, theme};
use glint_model::format::format_count;
use glint_model::paths::{is_folder_path, same_path};
use gpui_kit::{anchored, deferred, Anchor};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A press that closes the options menu is the same click as the one on its button: it must not open it again.
const SAME_PRESS: Duration = Duration::from_millis(300);

// ---- What a bar says ------------------------------------------------------------------------------------------

/// One of the two files selected across the panes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairFile {
    pub name: String,
    pub path: String,
}

/// The two files selected across the panes, the left pane's first (`crossPair`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair {
    pub left: PairFile,
    pub right: PairFile,
}

/// What the compare bar of a pane says.
#[derive(Clone, Debug)]
pub struct CompareInfo {
    /// How the pane's items differ from the other pane's; `None` while there is nothing to compare with (or the
    /// answer isn't in yet, see `pending`).
    pub comparison: Option<Arc<Comparison>>,
    /// There are two folders to compare and the answer is on its way: the bar says nothing yet.
    pub pending: bool,
    /// Both panes show this folder (`sameFolderInBothPanes`).
    pub same_folder: bool,
    /// One file selected in each pane.
    pub pair: Option<Pair>,
    /// The deeper comparison is being worked out.
    pub running: bool,
    /// Why it failed; empty if it didn't.
    pub error: String,
    /// What Sync would copy (`syncCount`).
    pub sync_count: usize,
    /// Sync copies what the deeper comparison found, subfolders too.
    pub deep_sync: bool,
    /// The other pane shows a folder Sync can copy into (`otherWritable`).
    pub other_writable: bool,
    /// Only the differences are shown.
    pub diff_only: bool,
}

/// The bar over a pane.
#[derive(Clone, Debug)]
pub enum Bar {
    /// Nothing is drawn.
    Hidden,
    /// "Selected across panes".
    Pair(Pair),
    /// The compare bar.
    Compare(CompareInfo),
}

/// The files selected in `pane` and in the other pane, one in each, when they are the pair to compare: both on
/// disk, not the same file (`crossPair`). A folder selected beside the file doesn't count. In the left pane's
/// order: the left pane's file first.
pub fn cross_pair(explorer: &Explorer, pane: usize) -> Option<Pair> {
    if !explorer.is_dual() || pane > 1 {
        return None;
    }
    let own = explorer.pane_tab(pane)?;
    let other = explorer.pane_tab(1 - pane)?;
    if own.selection.is_empty() || other.selection.is_empty() {
        return None;
    }
    let mut mine = own
        .rows
        .iter()
        .filter(|e| !e.dir && on_disk(e) && own.selection.contains(&e.path));
    let a = mine.next()?;
    if mine.next().is_some() {
        return None;
    }
    // The other pane's own items, or its search results.
    let others: &[glint_model::entries::FileEntry] = match &other.find {
        Some(find) => &find.entries,
        None => other.entries(),
    };
    let mut theirs = others
        .iter()
        .filter(|e| !e.dir && on_disk(e) && other.selection.contains(&e.path));
    let b = theirs.next()?;
    if theirs.next().is_some() || same_path(&a.path, &b.path) {
        return None;
    }
    let file = |e: &glint_model::entries::FileEntry| PairFile {
        name: e.name.clone(),
        path: e.path.clone(),
    };
    Some(if pane == 0 {
        Pair {
            left: file(a),
            right: file(b),
        }
    } else {
        Pair {
            left: file(b),
            right: file(a),
        }
    })
}

/// Which bar `pane` shows, and what is in it.
pub fn bar_of(explorer: &Explorer, pane: usize) -> Bar {
    if !explorer.is_dual() || pane > 1 {
        return Bar::Hidden;
    }
    let Some(tab) = explorer.pane_tab(pane) else {
        return Bar::Hidden;
    };
    let pair = cross_pair(explorer, pane);
    if !explorer.compare_panes {
        return match pair {
            Some(pair) if explorer.active_pane() == pane => Bar::Pair(pair),
            _ => Bar::Hidden,
        };
    }
    if tab.find.is_some() || !is_folder_path(&tab.path) {
        return Bar::Hidden;
    }
    let other = explorer.pane_tab(1 - pane);
    // `otherReady`: the other pane has opened a folder and isn't showing a search.
    let other_ready = other.filter(|o| o.ready && is_folder_path(&o.path) && o.find.is_none());
    let other_writable = other_ready.is_some_and(|o| o.archive().is_none() && !o.is_closed());
    let same_folder = other_ready.is_some_and(|o| same_path(&tab.path, &o.path));
    let comparison = explorer.comparison_of(tab.id);
    let pending = comparison.is_none()
        && !same_folder
        && tab.listing.is_some()
        && other_ready.is_some_and(|o| o.listing.is_some());
    let deep = explorer.deep_sync_items(pane);
    let sync_count = match (&deep, &comparison) {
        (Some(items), _) => items.len(),
        (None, Some(c)) => c.sync_paths.len(),
        (None, None) => 0,
    };
    Bar::Compare(CompareInfo {
        comparison,
        pending,
        same_folder,
        pair,
        running: explorer.deep_running(),
        error: explorer.deep_error().to_string(),
        sync_count,
        deep_sync: deep.is_some(),
        other_writable,
        diff_only: explorer.diff_only,
    })
}

/// Subfolders too, File contents, Only the differences: the bar's options as they are now (`settings`, `diffOnly`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub subfolders: bool,
    pub contents: bool,
    pub diff_only: bool,
    /// Names the comparison skips (Settings, Find files).
    pub exclusions: Vec<String>,
}

impl Options {
    /// The settings in force. Defaults before the settings exist.
    fn read(diff_only: bool, cx: &App) -> Options {
        match cx.try_global::<UiState>() {
            Some(ui) => Options {
                subfolders: ui.settings.compare_subfolders,
                contents: ui.settings.compare_contents,
                diff_only,
                exclusions: ui.settings.find_exclusions.clone(),
            },
            None => Options {
                diff_only,
                ..Options::default()
            },
        }
    }

    /// The options button shows a dot while anything beyond the plain comparison is on (`comparingMore`).
    pub fn comparing_more(&self) -> bool {
        self.subfolders || self.contents || self.diff_only
    }
}

// ---- The options menu -------------------------------------------------------------------------------------------

/// What a row of the options menu asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Subfolders,
    Contents,
    DiffOnly,
}

/// The prefix that marks a `Task::Navigate` as one of the bar's picks (the menu hands back only a `Run`).
const SCHEME: &str = "compare-bar:";

impl Pick {
    fn run(self) -> Run {
        let id = match self {
            Pick::Subfolders => "subfolders",
            Pick::Contents => "contents",
            Pick::DiffOnly => "diff-only",
        };
        Run::Do(Task::Navigate(format!("{SCHEME}{id}")))
    }

    /// The pick a chosen row carries.
    pub fn of(run: &Run) -> Option<Pick> {
        let Run::Do(Task::Navigate(text)) = run else {
            return None;
        };
        match text.strip_prefix(SCHEME)? {
            "subfolders" => Some(Pick::Subfolders),
            "contents" => Some(Pick::Contents),
            "diff-only" => Some(Pick::DiffOnly),
            _ => None,
        }
    }
}

/// The menu under the sliders button (`compareOptions`): the rows are check marks, so they stay readable.
pub fn options_menu(options: &Options) -> Vec<Node> {
    let skipping = if options.exclusions.is_empty() {
        t!("Subfolders too")
    } else {
        t!(
            "Subfolders too (skipping {names})",
            names = options.exclusions.join(", ")
        )
    };
    let row = |label: SharedString, pick: Pick, on: bool| {
        Node::Item(Entry::new(label, IconName::Check, pick.run()).checked(on))
    };
    vec![
        row(skipping, Pick::Subfolders, options.subfolders),
        row(
            t!("File contents, not only sizes and dates"),
            Pick::Contents,
            options.contents,
        ),
        Node::Separator,
        row(t!("Only the differences"), Pick::DiffOnly, options.diff_only),
    ]
}

// ---- The view ------------------------------------------------------------------------------------------------------

/// The options menu on screen.
struct OpenMenu {
    view: Entity<MenuView>,
    at: Point<Pixels>,
    /// What had the keyboard before it.
    focus: Option<FocusHandle>,
    _subscription: Subscription,
}

/// The bar over one pane. Draws nothing unless the pane needs one ([`bar_of`]).
pub struct CompareBar {
    pane: usize,
    explorer: Entity<Explorer>,
    /// Where the options button was painted, for the menu to open under it.
    options_at: Rc<Cell<Option<Bounds<Pixels>>>>,
    menu: Option<OpenMenu>,
    /// When a press on the options button closed its menu.
    dismissed: Option<Instant>,
    _subscriptions: Vec<Subscription>,
}

impl CompareBar {
    /// The bar over pane `pane`.
    pub fn new(pane: usize, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer(cx);
        let subscriptions = vec![
            cx.observe(&explorer, |_, _, cx| cx.notify()),
            // The options are settings: the dot and the check marks follow them.
            cx.observe_global::<UiState>(|_, cx| cx.notify()),
        ];
        CompareBar {
            pane,
            explorer,
            options_at: Rc::new(Cell::new(None)),
            menu: None,
            dismissed: None,
            _subscriptions: subscriptions,
        }
    }

    /// The pane this bar is over.
    pub fn pane(&self) -> usize {
        self.pane
    }

    /// Which bar the pane shows now.
    pub fn bar(&self, cx: &App) -> Bar {
        bar_of(self.explorer.read(cx), self.pane)
    }

    /// The options menu is open.
    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    /// The options menu's view, while it is open (tests drive it).
    #[cfg(test)]
    pub fn menu_view(&self) -> Option<&Entity<MenuView>> {
        self.menu.as_ref().map(|open| &open.view)
    }

    fn compare_files(&self, pair: &Pair, window: &mut Window, cx: &mut Context<Self>) {
        crate::dialogs::compare_files::open(window, cx, pair.left.path.clone(), pair.right.path.clone());
    }

    fn stop(&self, cx: &mut Context<Self>) {
        self.explorer
            .update(cx, |explorer, cx| explorer.toggle_compare_panes(cx));
    }

    fn select_differences(&self, cx: &mut Context<Self>) {
        let pane = self.pane;
        self.explorer
            .update(cx, |explorer, cx| explorer.select_differences(pane, cx));
    }

    fn sync(&self, cx: &mut Context<Self>) {
        let pane = self.pane;
        self.explorer
            .update(cx, |explorer, cx| explorer.sync_pane(pane, cx).detach());
    }

    /// Shows the options menu under the sliders button, taking the keyboard (`compareOptions`).
    fn open_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The press that closed this menu is this click's own: it closes the menu, not opens it again.
        if self.dismissed.take().is_some_and(|at| at.elapsed() < SAME_PRESS) {
            return;
        }
        self.close_menu(false, window, cx);
        let diff_only = self.explorer.read(cx).diff_only;
        let nodes = options_menu(&Options::read(diff_only, cx));
        let rtl = i18n::is_rtl();
        // Under the button, at its start edge: the menu opens toward the end (toward the start in Arabic).
        let at = match self.options_at.get() {
            Some(bounds) => point(
                if rtl { bounds.right() } else { bounds.left() },
                bounds.bottom() + px(4.),
            ),
            None => point(px(8.), px(48.)),
        };
        let focus = window.focused(cx);
        let view = cx.new(|cx| MenuView::new(nodes, t!("Compare options"), cx));
        view.update(cx, |view, cx| view.focus(window, cx));
        let subscription = cx.subscribe_in(&view, window, |this, _, event: &MenuViewEvent, window, cx| {
            this.on_menu_view(event.clone(), window, cx)
        });
        self.menu = Some(OpenMenu {
            view,
            at,
            focus,
            _subscription: subscription,
        });
        // A native preview draws above GPUI: it steps aside while the menu covers it.
        preview::cover(cx);
        cx.notify();
    }

    /// Closes the menu without choosing anything; `give_back` returns the keyboard to what had it.
    fn close_menu(&mut self, give_back: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.menu.take() {
            preview::uncover(cx);
            if let (true, Some(focus)) = (give_back, open.focus) {
                focus.focus(window, cx);
            }
            cx.notify();
        }
    }

    fn on_menu_view(&mut self, event: MenuViewEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.menu.take() else { return };
        preview::uncover(cx);
        cx.notify();
        match event {
            MenuViewEvent::Chose(run) => {
                if let Some(focus) = &open.focus {
                    focus.focus(window, cx);
                }
                if let Some(pick) = Pick::of(&run) {
                    self.pick(pick, cx);
                }
            }
            MenuViewEvent::Dismissed => {
                // A press on the button that opened it closed the menu: that button's click is the same press.
                let on_its_button = self
                    .options_at
                    .get()
                    .is_some_and(|bounds| bounds.contains(&window.mouse_position()));
                self.dismissed = on_its_button.then(Instant::now);
                // Escape gives the keyboard back; a press outside lets the thing pressed have it, unless that took
                // nothing.
                if let Some(focus) = open.focus.clone() {
                    let menu = open.view.read(cx).focus_handle(cx);
                    window.defer(cx, move |window, cx| {
                        if window.focused(cx).is_none_or(|now| now == menu) {
                            focus.focus(window, cx);
                        }
                    });
                }
            }
        }
    }

    /// Does what a chosen row says: flips the setting, or "only the differences".
    fn pick(&mut self, pick: Pick, cx: &mut Context<Self>) {
        match pick {
            Pick::Subfolders if cx.has_global::<UiState>() => state::update(cx, |settings| {
                settings.compare_subfolders = !settings.compare_subfolders
            }),
            Pick::Contents if cx.has_global::<UiState>() => {
                state::update(cx, |settings| settings.compare_contents = !settings.compare_contents)
            }
            Pick::DiffOnly => self.explorer.update(cx, |explorer, cx| explorer.toggle_diff_only(cx)),
            Pick::Subfolders | Pick::Contents => {}
        }
    }
}

// ---- Drawing ---------------------------------------------------------------------------------------------------

/// A pill of text with a tinted fill (`rounded-[4px] bg-*/15 px-1.5 py-0.5 font-medium`).
fn badge(id: &'static str, pane: usize, text: SharedString, fill: Hsla, ink: Hsla) -> impl IntoElement {
    div()
        .id((id, pane))
        .role(Role::Group)
        .aria_label(text.clone())
        .test_support()
        .flex_none()
        .px(space(1.5))
        .py(px(2.))
        .rounded(theme::size::RADIUS_SM)
        .bg(fill)
        .text_color(ink)
        .font_weight(FontWeight::MEDIUM)
        .child(text)
}

/// A button of the bar: a tinted one (`bg-primary/15 text-primary`) or a plain one that lights on hover.
fn pill(cx: &App, t: &theme::Tokens, id: (&'static str, usize), tinted: bool) -> Button {
    let clear = hsla(0., 0., 0., 0.);
    Button::new(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(if tinted { t.primary.opacity(0.15) } else { clear })
                .foreground(if tinted { t.primary } else { t.muted_foreground })
                .hover(if tinted { t.primary.opacity(0.25) } else { t.accent })
                .active(if tinted { t.primary.opacity(0.3) } else { t.accent }),
        )
        .xsmall()
}

/// The bar's frame: a wrapping row over a primary tint (`flex min-h-8 flex-wrap items-center gap-1.5 border-b
/// bg-primary/5 px-2.5 py-1 text-xs`).
fn frame(
    id: &'static str,
    pane: usize,
    name: SharedString,
    gap: f32,
    t: &theme::Tokens,
) -> impl ParentElement + Styled + IntoElement {
    row_start()
        .id((id, pane))
        .role(Role::Group)
        .aria_label(name)
        .test_support()
        .relative()
        .flex_none()
        .flex_wrap()
        .w_full()
        .min_h(space(8.))
        .gap(space(gap))
        .px(space(2.5))
        .py(space(1.))
        .border_b_1()
        .border_color(t.border)
        .bg(t.primary.opacity(0.05))
        .text_size(theme::size::XS)
        .text_start()
}

/// The group at the end of the bar (`ms-auto`).
fn end_group() -> Div {
    let group = row_start().flex_none().gap(space(1.));
    if i18n::is_rtl() {
        group.mr_auto()
    } else {
        group.ml_auto()
    }
}

/// The text of a pair's button: Compare “a” ↔ “b”.
fn pair_words(pair: &Pair) -> (SharedString, SharedString) {
    (
        t!("Compare “{a}” ↔ “{b}”", a = pair.left.name, b = pair.right.name),
        crate::components::command_bar::keyed_with(
            "Compare “{a}” and “{b}” ({0})",
            &[&actions::CompareFiles],
            &[("a", &pair.left.name), ("b", &pair.right.name)],
        ),
    )
}

impl CompareBar {
    /// "Selected across panes: “a” ↔ “b”" with Compare files.
    fn pair_bar(&self, pair: &Pair, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let pane = self.pane;
        let name = t!(
            "Selected across panes: “{a}” ↔ “{b}”",
            a = pair.left.name,
            b = pair.right.name
        );
        let strong = |text: &str| {
            div()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.foreground)
                .child(isolate(format!("“{text}”")))
        };
        let words = row_start()
            .min_w_0()
            .gap(space(1.))
            .text_color(t.muted_foreground)
            .child(div().flex_none().child(t!("Selected across panes:")))
            .child(strong(&pair.left.name))
            .child(div().flex_none().child("↔"))
            .child(strong(&pair.right.name));
        let chosen = pair.clone();
        frame("compare-pair-bar", pane, name, 2., &t)
            .child(
                icon(IconName::GitCompareArrows)
                    .size_3p5()
                    .flex_none()
                    .text_color(t.primary),
            )
            .child(words)
            .child(
                end_group().child(
                    pill(cx, &t, ("compare-files", pane), true)
                        .icon(IconName::GitCompareArrows)
                        .label(t!("Compare files"))
                        .tooltip(keyed(
                            "Compare the two files selected across both panes ({0})",
                            &[&actions::CompareFiles],
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| this.compare_files(&chosen, window, cx))),
                ),
            )
            .into_any_element()
    }

    /// The compare bar.
    fn compare_bar(&self, info: &CompareInfo, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let pane = self.pane;
        let options = Options::read(info.diff_only, cx);
        let dark = t.dark;
        let mut bar = frame("compare-bar", pane, t!("Compare panes"), 1.5, &t).child(
            icon(IconName::GitCompareArrows)
                .size_3p5()
                .flex_none()
                .text_color(t.primary),
        );

        // The button for the pair of files selected in each pane.
        let pair_button = info.pair.as_ref().map(|pair| {
            let (label, tip) = pair_words(pair);
            let chosen = pair.clone();
            pill(cx, &t, ("compare-pair", pane), true)
                .icon(IconName::GitCompareArrows)
                .label(label)
                .tooltip(tip)
                .on_click(cx.listener(move |this, _, window, cx| this.compare_files(&chosen, window, cx)))
        });
        let stop = pill(cx, &t, ("compare-stop", pane), false)
            .icon(IconName::X)
            .accessibility_label(t!("Stop comparing panes"))
            .tooltip(keyed("Stop comparing panes ({0})", &[&actions::ComparePanes]))
            .on_click(cx.listener(|this, _, _, cx| this.stop(cx)));

        let Some(comparison) = &info.comparison else {
            // Nothing to compare yet: say what would make something to compare.
            let words = if info.pending {
                None
            } else if info.same_folder {
                Some(match &info.pair {
                    Some(pair) => t!(
                        "Comparing “{a}” and “{b}” in this folder",
                        a = pair.left.name,
                        b = pair.right.name
                    ),
                    None => t!(
                        "Both panes show this folder — select a file in each pane to compare them, or open another folder"
                    ),
                })
            } else {
                Some(t!("Open a folder in the other pane to compare"))
            };
            let alone = info.pair.as_ref().map(|pair| {
                let chosen = pair.clone();
                pill(cx, &t, ("compare-pair", pane), true)
                    .icon(IconName::GitCompareArrows)
                    .label(t!("Compare files"))
                    .tooltip(pair_words(pair).1)
                    .on_click(cx.listener(move |this, _, window, cx| this.compare_files(&chosen, window, cx)))
            });
            return bar
                .children(words.map(|words| {
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(t.muted_foreground)
                        .child(isolate(words))
                }))
                .child(end_group().children(alone).child(stop))
                .into_any_element();
        };

        // The counts, each in its own color.
        let count = |n: usize| format_count(n);
        if comparison.unique > 0 {
            bar = bar.child(badge(
                "compare-unique",
                pane,
                t!("{n} only here", n = count(comparison.unique)),
                Tone::Emerald.tint(0.15),
                Tone::Emerald.badge_text(dark),
            ));
        }
        if comparison.newer > 0 {
            bar = bar.child(badge(
                "compare-newer",
                pane,
                t!("{n} newer", n = count(comparison.newer)),
                Tone::Sky.tint(0.15),
                Tone::Sky.badge_text(dark),
            ));
        }
        if comparison.differ > 0 {
            bar = bar.child(badge(
                "compare-differ",
                pane,
                t!("{n} different", n = count(comparison.differ)),
                Tone::Amber.tint(0.15),
                Tone::Amber.badge_text(dark),
            ));
        }
        if comparison.older > 0 {
            bar = bar.child(badge(
                "compare-older",
                pane,
                t!("{n} older", n = count(comparison.older)),
                t.muted,
                t.muted_foreground,
            ));
        }
        if comparison.diff_paths.is_empty() {
            bar = bar.child(
                div()
                    .id(("compare-all-match", pane))
                    .role(Role::Group)
                    .aria_label(t!("All match"))
                    .test_support()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(Tone::Emerald.count_text(dark))
                    .child(t!("All match")),
            );
        }
        bar = bar.child(
            div()
                .id(("compare-same", pane))
                .role(Role::Group)
                .aria_label(t!("{n} same", n = count(comparison.same)))
                .test_support()
                .flex_none()
                .text_color(t.muted_foreground)
                .child(t!("· {n} same", n = count(comparison.same))),
        );
        if info.running {
            bar = bar.child(
                div()
                    .id(("compare-deeper", pane))
                    .role(Role::Group)
                    .aria_label(t!("Comparing deeper"))
                    .test_support()
                    .flex_none()
                    .child(Spinner::new().xsmall().color(t.primary)),
            );
        }
        if !info.error.is_empty() {
            let why = SharedString::from(info.error.clone());
            let tip = why.clone();
            bar = bar.child(
                div()
                    .id(("compare-error", pane))
                    .role(Role::Group)
                    .aria_label(t!("Couldn't compare deeper: {error}", error = info.error))
                    .test_support()
                    .min_w_0()
                    .max_w_full()
                    .truncate()
                    .text_color(t.destructive)
                    .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
                    .child(isolate(t!("Couldn't compare deeper: {error}", error = why))),
            );
        }

        // The options button, with a dot while anything beyond the plain comparison is on.
        let probe = self.options_at.clone();
        let dot = options.comparing_more().then(|| {
            let dot = div()
                .absolute()
                .top(px(-2.))
                .size(rems(0.375))
                .rounded_full()
                .bg(t.primary);
            if i18n::is_rtl() {
                dot.left(px(-2.))
            } else {
                dot.right(px(-2.))
            }
        });
        let sliders = pill(cx, &t, ("compare-options", pane), false)
            .icon(IconName::SlidersHorizontal)
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(hsla(0., 0., 0., 0.))
                    .foreground(if options.comparing_more() {
                        t.primary
                    } else {
                        t.muted_foreground
                    })
                    .hover(t.accent)
                    .active(t.accent),
            )
            .accessibility_label(t!("Compare options"))
            .tooltip(t!(
                "Compare subfolders too, file contents, or show only the differences"
            ))
            .on_click(cx.listener(|this, _, window, cx| this.open_menu(window, cx)));
        let sliders = div().relative().flex_none().child(sliders).children(dot).child(
            canvas(move |bounds, _, _| probe.set(Some(bounds)), |_, _, _, _| {})
                .absolute()
                .size_full(),
        );

        let differences = comparison.diff_paths.len();
        let select = pill(cx, &t, ("compare-select", pane), false)
            .label(t!("Select differences ({n})", n = count(differences)))
            .tooltip(t!("Select all items that differ in this pane"))
            .disabled(differences == 0)
            .on_click(cx.listener(|this, _, _, cx| this.select_differences(cx)));

        // The left pane's Sync points at the other pane, wherever that is on screen.
        let toward = if (pane == 0) != i18n::is_rtl() { "→" } else { "←" };
        let sync_label: SharedString = if pane == 0 {
            t!("Sync {n} →", n = count(info.sync_count)).replace('→', toward).into()
        } else {
            t!("← Sync {n}", n = count(info.sync_count)).replace('←', toward).into()
        };
        let sync = pill(cx, &t, ("compare-sync", pane), true)
            .label(sync_label)
            .tooltip(if info.deep_sync {
                t!("Copy what's missing or newer here, in subfolders too, to the other pane")
            } else {
                t!("Copy unique and newer items from this pane to the other pane")
            })
            .disabled(!info.other_writable || info.sync_count == 0)
            .on_click(cx.listener(|this, _, _, cx| this.sync(cx)));

        let end = end_group()
            .children(pair_button)
            .child(sliders)
            .child(select)
            .child(sync)
            .child(stop);

        // The menu is outside the row's flow: it must not push the bar or be cut at its edge.
        let menu = self.menu.as_ref().map(|open| {
            let anchor = if i18n::is_rtl() {
                Anchor::TopRight
            } else {
                Anchor::TopLeft
            };
            deferred(
                anchored()
                    .position(open.at)
                    .anchor(anchor)
                    .snap_to_window_with_margin(px(4.))
                    .child(open.view.clone()),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
        });
        bar.child(end).children(menu).into_any_element()
    }
}

impl Render for CompareBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.bar(cx) {
            Bar::Hidden => {
                // The bar went away (compare panes stopped, a search began) with its menu open.
                if self.menu.is_some() {
                    self.close_menu(false, window, cx);
                }
                div().into_any_element()
            }
            Bar::Pair(pair) => self.pair_bar(&pair, cx),
            Bar::Compare(info) => self.compare_bar(&info, cx),
        }
    }
}
