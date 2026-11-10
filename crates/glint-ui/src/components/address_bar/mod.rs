//! Breadcrumbs, editing and completion (AddressBar.tsx).
//!
//! The row above the files: Back, Forward, Up and Refresh, the address field (the folder as clickable
//! breadcrumbs; a click on empty space or Ctrl+L turns it into a text box with completion), and the search
//! box. It follows the active tab of the [`Explorer`](crate::explorer::Explorer) and keeps none of its state:
//! the folder, history, filter and "typing a path" flag all live there.
//!
//! # What the TypeScript had, and where it is here
//!
//! | TypeScript | Here |
//! |---|---|
//! | `NavButton`s, `AddressBar` | [`AddressBar`] (this file) |
//! | `AddressInput`, `Suggestion` | `completion.rs` (the logic), this file (the box and its list) |
//! | `openFlyout`, the chevron menus | `flyout.rs` |
//! | `FILTERS`, `onFilterChange`, `onFindKeyDown`, the box | `search.rs` (the rules), `find.rs` (the box and its hints) |
//! | `PaneSide` | `AddressBar::pane_side` |
//! | arrows, Tab and Escape in the fields | `keys.rs` |
//!
//! # Not here yet (each is a later part)
//!
//! - **Finding:** the box filters the folder as you type (`Explorer::set_filter`) and, where the index answers at
//!   once, searches below it (`Explorer::find_as_typed`); Enter, leaving and clearing are [`AddressBarEvent`]s
//!   the workspace answers.
//! - **Git:** the branch chip after the breadcrumbs (one [`GitChip`](crate::components::git::GitChip) per tab,
//!   hidden below 26 rem of bar), and `git:` in the box's hints in a repository. The bar follows the Git model
//!   itself (it must exist, `git::init`, when the bar is made); [`AddressBar::set_in_repository`] is for tests.
//! - **Dragging and dropping:** dropping files on a breadcrumb, dragging a breadcrumb out.
//!
//! # Right to left
//!
//! Every row is built with the kit's start/end helpers, so the Arabic interface mirrors it: Back is on the
//! right, the breadcrumbs read right to left, the first crumbs fold away at the right.

mod completion;
mod find;
mod flyout;
mod keys;
mod search;

use crate::components::git::{self, GitChip};
use crate::core_bridge::Core;
use crate::explorer::{self, CoreSource, Drive, DriveKind, Explorer, KnownFolder, NavOptions, Refresh, Source, TabId};
use crate::kit::prelude::*;
use crate::{i18n, state, t, theme};
use completion::{Ask, Look, Suggestion, Typed};
use glint_model::entries::FileEntry;
use glint_model::paths::{
    crumbs, in_archive_path, is_gallery, is_recent, is_recycle_bin, is_remote_path, same_path, THIS_PC,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// How far the pointer moves with a crumb held before the folder is dragged out (`crumbDragRef`, 6 px).
const DRAG_THRESHOLD: f32 = 6.;

pub use keys::{ListAccept, ListCancel, ListComplete, ListDown, ListFirst, ListLast, ListUp};

/// A listing with more entries than this is ranked on a background thread rather than while you type.
const RANK_IN_BACKGROUND: usize = 5_000;

/// How long the "copied" check stays on the copy button (the TypeScript's 1200 ms).
const COPIED_FOR: Duration = Duration::from_millis(1200);

/// What the bar tells the rest of the window. The coordinator wires these up; the comments say what each
/// should do.
#[derive(Clone, Debug, PartialEq)]
pub enum AddressBarEvent {
    /// Give the keyboard to the file list or grid: Escape left a field, or a path was entered and opened.
    /// With `first`, move to its first item as well (Down arrow out of the search box).
    FocusFiles { first: bool },
    /// Enter in the search box: search the folder and its subfolders (`runFind(query)`).
    RunFind { query: String },
    /// The search text became empty (or holds only a `git:` filter) while search results were shown:
    /// `leaveFind()`.
    LeaveFind,
    /// The search was cleared (Escape or the clear button; the filter is already empty): `clearFind()`.
    ClearFind,
    /// Refresh (F5) while search results are shown: run that search again (`refresh` in explorer.ts).
    RerunFind { query: String, scope: String },
}

/// One breadcrumb as the bar draws it.
#[derive(Clone, Debug, PartialEq)]
struct CrumbView {
    /// What it shows: a drive by its name, a place by its own, a folder by its name.
    name: String,
    path: String,
    /// An archive on the way, opened like a folder.
    archive: bool,
    /// The top of a remote connection.
    remote_top: bool,
    /// The Recycle Bin, Recent or the Gallery: nothing to go into below.
    place: bool,
    /// The drive this root is, for its icon in the "…" menu.
    drive: Option<DriveKind>,
}

/// What the bar draws from the explorer, taken once per change so a redraw is cheap and a change that
/// doesn't concern the bar (a selection) doesn't redraw it.
#[derive(Clone, Debug, PartialEq)]
struct Seen {
    tab: TabId,
    path: String,
    back: bool,
    forward: bool,
    loading: bool,
    filter: String,
    has_find: bool,
    editing: bool,
    dual: bool,
    pane: usize,
    drives: Arc<[Drive]>,
    known: Arc<[KnownFolder]>,
    crumbs: Vec<CrumbView>,
    /// What the search box says it searches ("Search {folder}").
    folder_name: String,
    rtl: bool,
}

/// The names of the fixed places, translated; any other name as it is.
fn place_name(name: &str) -> String {
    match name {
        "This PC" => t!("This PC").to_string(),
        "Recycle Bin" => t!("Recycle Bin").to_string(),
        "Recent" => t!("Recent").to_string(),
        "Gallery" => t!("Gallery").to_string(),
        other => other.to_string(),
    }
}

impl Seen {
    fn of(explorer: &Explorer) -> Seen {
        let tab = explorer.active_tab();
        let path = tab.map_or(THIS_PC.to_string(), |t| t.path.clone());
        let archive = tab.and_then(|t| t.archive());
        let is_archive = |crumb: &str| archive.is_some_and(|a| same_path(crumb, &a.path) || same_path(crumb, &a.outer));
        let drives = explorer.drives().clone();
        let trail = crumbs(&path);
        let crumbs = trail
            .iter()
            .enumerate()
            .map(|(i, crumb)| {
                let place =
                    i == 0 && (is_recycle_bin(&crumb.path) || is_recent(&crumb.path) || is_gallery(&crumb.path));
                let drive = (i == 0)
                    .then(|| drives.iter().find(|d| same_path(&d.path, &crumb.path)))
                    .flatten();
                // A drive by its name; a place by its own; a remote by its connection's name.
                let name = if i != 0 {
                    crumb.name.clone()
                } else if place {
                    place_name(&crumb.name)
                } else if is_remote_path(&crumb.path) {
                    explorer.tab_name(&crumb.path)
                } else if let Some(drive) = drive {
                    explorer::drive_name(drive)
                } else {
                    crumb.path.clone()
                };
                CrumbView {
                    name,
                    path: crumb.path.clone(),
                    archive: is_archive(&crumb.path),
                    remote_top: i == 0 && is_remote_path(&crumb.path),
                    place,
                    drive: drive.map(|d| d.kind),
                }
            })
            .collect();
        Seen {
            tab: explorer.active_tab_id(),
            back: tab.is_some_and(|t| !t.back.is_empty()),
            forward: tab.is_some_and(|t| !t.forward.is_empty()),
            loading: tab.is_some_and(|t| t.loading),
            filter: tab.map(|t| t.filter.clone()).unwrap_or_default(),
            has_find: tab.is_some_and(|t| t.find.is_some()),
            editing: explorer.editing_address,
            dual: explorer.is_dual(),
            pane: explorer.active_pane(),
            folder_name: place_name(&explorer.tab_name(&path)),
            path,
            drives,
            known: explorer.known_folders().clone(),
            crumbs,
            rtl: i18n::is_rtl(),
        }
    }
}

/// What the last frame measured of the crumbs, which decides how many fold into the "…" menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Layout {
    /// Crumbs from the start that don't fit and are in the "…" menu.
    folded: usize,
    /// Narrower than 16 rem: "This PC" and its chevron give way to the folder you're in.
    narrow: bool,
    /// Narrower than 9 rem: the "…" goes too.
    tiny: bool,
    /// Narrower than 26 rem: the Git chip gives way (`@max-[26rem]:hidden`).
    no_chip: bool,
}

/// The address bar. See the module notes; build it with [`AddressBar::new`] and place it in the toolbar.
pub struct AddressBar {
    focus: FocusHandle,
    explorer: Entity<Explorer>,
    /// Lists the folders the completion and the menus offer: the explorer's own source in the app.
    source: Rc<dyn Source>,
    seen: Rc<Seen>,

    // Typing a path
    edit: Entity<InputState>,
    editing: bool,
    typed: Typed,
    /// Listings by folder while the box is open (the TypeScript's `cacheRef`).
    cache: HashMap<String, Arc<[FileEntry]>>,
    /// The listing of the typed folder, with its cache key, once it has arrived.
    listed: Option<(String, Arc<[FileEntry]>)>,
    listing: Option<(String, Task<()>)>,
    suggestions: Vec<Suggestion>,
    /// The highlighted suggestion (`activeIndex`, -1 for none).
    active: Option<usize>,
    rank_seq: u64,
    ranking: Option<Task<()>>,

    // The crumbs
    measure: Rc<RefCell<Measure>>,
    copied: bool,
    copied_task: Option<Task<()>>,
    flyout: Option<flyout::Flyout>,
    flyout_seq: u64,

    // The search box
    search: Entity<InputState>,
    find_focused: bool,
    plain: Option<glint_model::plain::Plain>,
    find_timer: Option<Task<()>>,
    /// A crumb pressed, and where: dragged past [`DRAG_THRESHOLD`] with the button down, it drags its folder out.
    crumb_press: Option<(String, Point<Pixels>)>,
    /// The crumb dragged files are held over, lit while they are.
    drop_over: Option<usize>,
    in_repository: bool,
    /// The branch chip of each tab, made when the tab first shows (a chip follows its tab's folder).
    chips: HashMap<TabId, Entity<GitChip>>,
    /// What the box's placeholder says now ("Search {folder}").
    placeholder: SharedString,

    _subscriptions: Vec<Subscription>,
}

/// What the paint of a frame learned: handed from the measuring callbacks to the next render.
#[derive(Default)]
struct Measure {
    layout: Layout,
    /// Where each crumb's cell was painted (window coordinates), for the crumb under dragged files.
    cells: Vec<Bounds<Pixels>>,
}

impl EventEmitter<AddressBarEvent> for AddressBar {}

impl AddressBar {
    /// The bar for the active tab of the app's explorer, listing folders through glint-core. Needs
    /// `explorer::init` and `core_bridge::init` to have run.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let source: Rc<dyn Source> = Rc::new(CoreSource::new(cx.global::<Core>().app.clone()));
        Self::with_source(source, window, cx)
    }

    /// [`AddressBar::new`] with a source of your own for the folders it lists (tests, tools).
    pub fn with_source(source: Rc<dyn Source>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        keys::ensure_bound(cx);
        let explorer = explorer::explorer(cx);
        let seen = Rc::new(Seen::of(explorer.read(cx)));
        let edit = cx.new(|cx| InputState::new(window, cx));
        let placeholder = t!("Search {folder}", folder = seen.folder_name);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.clone()));
        let mut subscriptions = vec![
            cx.observe_in(&explorer, window, |this, _, window, cx| this.sync(window, cx)),
            cx.subscribe_in(&edit, window, Self::on_edit_event),
            cx.subscribe_in(&search, window, Self::on_search_event),
            cx.observe_global::<theme::GlintTheme>(|_, cx| cx.notify()),
            cx.observe_global::<i18n::Current>(|this, cx| this.sync_view(cx)),
        ];
        // Repositories come and go as the folders open: the chip and the `git:` hint follow the Git model.
        if let Some(model) = git::try_shared(cx) {
            subscriptions.push(cx.observe(&model, |this, _, cx| this.sync_git(cx)));
        }
        subscriptions.shrink_to_fit();
        let mut bar = Self {
            focus: cx.focus_handle(),
            explorer,
            source,
            seen,
            edit,
            editing: false,
            typed: Typed::default(),
            cache: HashMap::new(),
            listed: None,
            listing: None,
            suggestions: Vec::new(),
            active: None,
            rank_seq: 0,
            ranking: None,
            measure: Rc::new(RefCell::new(Measure::default())),
            copied: false,
            copied_task: None,
            flyout: None,
            flyout_seq: 0,
            search,
            find_focused: false,
            plain: None,
            find_timer: None,
            crumb_press: None,
            drop_over: None,
            in_repository: false,
            chips: HashMap::new(),
            placeholder,
            _subscriptions: subscriptions,
        };
        bar.in_repository = git::try_shared(cx).is_some() && git::in_repository(cx, bar.seen.tab);
        // A bar built while the explorer is already asking for a path starts typing in it.
        if bar.seen.editing {
            bar.begin_editing(window, cx);
        }
        bar
    }

    // ---- What the workspace calls -----------------------------------------------------------------------

    /// Ctrl+L, Ctrl+D and F4 (the `FocusAddress` action): types a path, the whole of it selected.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing {
            self.edit.update(cx, |input, cx| {
                input.select_all(window, cx);
                input.focus(window, cx);
            });
            return;
        }
        self.explorer
            .update(cx, |explorer, cx| explorer.set_editing_address(true, cx));
        // The explorer's change reaches `sync` later; the box opens now.
        let mut seen = (*self.seen).clone();
        seen.editing = true;
        self.seen = Rc::new(seen);
        self.begin_editing(window, cx);
        cx.notify();
    }

    /// Ctrl+F (the `SearchHere` action): into the search box, its text selected.
    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |input, cx| {
            input.select_all(window, cx);
            input.focus(window, cx);
        });
    }

    /// Whether the address is being typed in.
    pub fn is_editing(&self) -> bool {
        self.editing
    }

    /// The folder shown is in a Git repository, so the search box's hints include `git:` (`useGit(...).view`).
    /// The bar asks the Git model itself ([`git::in_repository`]); this is for a bar without one.
    pub fn set_in_repository(&mut self, in_repository: bool, cx: &mut Context<Self>) {
        if self.in_repository != in_repository {
            self.in_repository = in_repository;
            cx.notify();
        }
    }

    /// Git's view of the active tab changed: the `git:` hint is offered in a repository (`git::in_repository`).
    fn sync_git(&mut self, cx: &mut Context<Self>) {
        let in_repository = git::in_repository(cx, self.seen.tab);
        self.set_in_repository(in_repository, cx);
    }

    // ---- Following the explorer -------------------------------------------------------------------------

    /// The explorer changed: take what the bar draws, notice a path being asked for, keep the search box in
    /// step with the tab's filter.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seen = Seen::of(self.explorer.read(cx));
        let tab_changed = seen.tab != self.seen.tab;
        let changed = seen != *self.seen;
        if changed {
            self.seen = Rc::new(seen);
        }
        if tab_changed {
            // Another tab has its own filter, and a menu or pending search belongs to the one before.
            self.find_timer = None;
            self.close_flyout(false, window, cx);
            self.plain = None;
        }
        if self.seen.editing && !self.editing {
            self.begin_editing(window, cx);
        } else if !self.seen.editing && self.editing {
            self.end_editing(window, cx);
        }
        self.sync_search(window, cx);
        if git::try_shared(cx).is_some() {
            self.sync_git(cx);
        }
        if changed {
            cx.notify();
        }
    }

    /// The language changed: names are rebuilt.
    fn sync_view(&mut self, cx: &mut Context<Self>) {
        let seen = Seen::of(self.explorer.read(cx));
        if seen != *self.seen {
            self.seen = Rc::new(seen);
        }
        cx.notify();
    }

    // ---- Typing a path ----------------------------------------------------------------------------------

    /// The path as the box starts out: the places by name, a folder by its whole path.
    fn typed_path(path: &str) -> String {
        if path == THIS_PC {
            "This PC".to_string()
        } else if is_recycle_bin(path) {
            "Recycle Bin".to_string()
        } else if is_recent(path) {
            "Recent".to_string()
        } else if is_gallery(path) {
            "Gallery".to_string()
        } else {
            path.to_string()
        }
    }

    /// Shows the box with the path in it, all selected, and the keyboard in it.
    fn begin_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = true;
        let text = Self::typed_path(&self.seen.path);
        self.edit.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            input.select_all(window, cx);
            input.focus(window, cx);
        });
        self.close_flyout(false, window, cx);
        self.active = None;
        self.refresh_typed(cx);
        cx.notify();
    }

    /// The explorer says the box is done (a folder opened, another tab came up).
    fn end_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = false;
        self.suggestions.clear();
        self.active = None;
        self.cache.clear();
        self.listed = None;
        self.listing = None;
        self.ranking = None;
        // The box going away must not take the keyboard with it: the window's keys work from the focused
        // element up, so it stays inside the bar until the window moves it on.
        if self.edit.read(cx).focus_handle(cx).is_focused(window) {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// Gives up typing (Escape, a click elsewhere, or a path that opened): the explorer shows the crumbs again.
    fn close_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editing {
            return;
        }
        self.explorer
            .update(cx, |explorer, cx| explorer.set_editing_address(false, cx));
        let mut seen = (*self.seen).clone();
        seen.editing = false;
        self.seen = Rc::new(seen);
        self.end_editing(window, cx);
    }

    fn on_edit_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.active = None;
                self.refresh_typed(cx);
                cx.notify();
            }
            InputEvent::PressEnter { .. } => {
                let text = self.edit.read(cx).value().to_string();
                let target = self
                    .active
                    .and_then(|i| self.suggestions.get(i))
                    .map_or(text, |s| s.path.clone());
                self.go(target, window, cx);
            }
            InputEvent::Blur => {
                if self.editing {
                    self.close_editing(window, cx);
                }
            }
            InputEvent::Focus => {}
        }
    }

    /// Opens what was typed or chosen; the box closes when a folder opened.
    fn go(&mut self, target: String, window: &mut Window, cx: &mut Context<Self>) {
        let opening = self.explorer.update(cx, |explorer, cx| explorer.go_to(&target, cx));
        cx.spawn_in(window, async move |this, cx| {
            if opening.await {
                this.update_in(cx, |this, window, cx| {
                    this.close_editing(window, cx);
                    cx.emit(AddressBarEvent::FocusFiles { first: false });
                })
                .ok();
            }
        })
        .detach();
    }

    /// The typed text changed: which folder to list, and what to offer.
    fn refresh_typed(&mut self, cx: &mut Context<Self>) {
        let text = self.edit.read(cx).value();
        self.typed = completion::split_typed(&text);
        let dir = self.typed.parent_dir.clone();
        if !completion::lists_folder(&dir) {
            self.listed = None;
            self.listing = None;
        } else {
            let key = completion::cache_key(&dir);
            if self.listed.as_ref().is_some_and(|(k, _)| *k == key) {
                // Already have it.
            } else if let Some(entries) = self.cache.get(&key) {
                self.listed = Some((key, entries.clone()));
                self.listing = None;
            } else {
                self.listed = None;
                if self.listing.as_ref().is_none_or(|(k, _)| *k != key) {
                    self.list_folder(dir, key, cx);
                }
            }
        }
        self.rank(cx);
    }

    /// Reads the typed folder without waiting for it.
    fn list_folder(&mut self, dir: String, key: String, cx: &mut Context<Self>) {
        let reading = self.source.list(&dir);
        let asked = key.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = reading.await;
            this.update(cx, |this, cx| {
                if this.listing.as_ref().is_some_and(|(k, _)| *k == asked) {
                    this.listing = None;
                }
                if let Ok(listed) = result {
                    this.cache.insert(asked.clone(), listed.entries.clone());
                    if completion::cache_key(&this.typed.parent_dir) == asked {
                        this.listed = Some((asked, listed.entries));
                        this.rank(cx);
                        cx.notify();
                    }
                }
            })
            .ok();
        });
        self.listing = Some((key, task));
    }

    /// Works out the suggestions for the typed text: at once for a small folder, on a background thread for a
    /// big one (ranking a hundred thousand names mustn't hold up a key press).
    fn rank(&mut self, cx: &mut Context<Self>) {
        self.rank_seq += 1;
        let seq = self.rank_seq;
        let show_hidden = state::settings(cx).show_hidden;
        let entries = self.listed.as_ref().map(|(_, e)| e.clone());
        let big = entries.as_ref().is_some_and(|e| e.len() > RANK_IN_BACKGROUND);
        if !big {
            self.ranking = None;
            self.suggestions = completion::suggest(&Ask {
                typed: &self.typed,
                entries: entries.as_deref(),
                drives: &self.seen.drives,
                known: &self.seen.known,
                show_hidden,
            });
            self.keep_active_in_range();
            return;
        }
        let (typed, drives, known) = (self.typed.clone(), self.seen.drives.clone(), self.seen.known.clone());
        self.ranking = Some(cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    completion::suggest(&Ask {
                        typed: &typed,
                        entries: entries.as_deref(),
                        drives: &drives,
                        known: &known,
                        show_hidden,
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                if this.rank_seq == seq {
                    this.suggestions = found;
                    this.keep_active_in_range();
                    this.ranking = None;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn keep_active_in_range(&mut self) {
        if self.active.is_some_and(|i| i >= self.suggestions.len()) {
            self.active = None;
        }
    }

    /// Down: the next suggestion, round to the first.
    fn suggestion_down(&mut self, cx: &mut Context<Self>) {
        let n = self.suggestions.len();
        if n == 0 {
            cx.propagate();
            return;
        }
        self.active = Some(self.active.map_or(0, |i| (i + 1) % n));
        cx.notify();
    }

    /// Up: the previous one, round to the last.
    fn suggestion_up(&mut self, cx: &mut Context<Self>) {
        let n = self.suggestions.len();
        if n == 0 {
            cx.propagate();
            return;
        }
        self.active = Some(match self.active {
            Some(i) if i > 0 => i - 1,
            _ => n - 1,
        });
        cx.notify();
    }

    /// Tab: the highlighted suggestion (or the first) goes into the box, a folder with a separator after it.
    fn complete_suggestion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.suggestions.get(self.active.unwrap_or(0)).cloned() else {
            cx.propagate();
            return;
        };
        let text = self.edit.read(cx).value();
        let next = completion::complete_with(&text, &item);
        self.edit.update(cx, |input, cx| {
            input.set_value(next, window, cx);
            input.focus(window, cx);
        });
        self.active = None;
        self.refresh_typed(cx);
        cx.notify();
    }

    // ---- The buttons ------------------------------------------------------------------------------------

    fn go_back(&mut self, cx: &mut Context<Self>) {
        self.explorer.update(cx, |explorer, cx| explorer.go_back(cx).detach());
    }

    fn go_forward(&mut self, cx: &mut Context<Self>) {
        self.explorer
            .update(cx, |explorer, cx| explorer.go_forward(cx).detach());
    }

    fn go_up(&mut self, cx: &mut Context<Self>) {
        self.explorer.update(cx, |explorer, cx| explorer.go_up(cx).detach());
    }

    /// Refresh (F5): the folder is read again; a search shown is run again by the find part.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let refreshing = self.explorer.update(cx, |explorer, cx| explorer.refresh(cx));
        match refreshing {
            Refresh::Folder(task) => task.detach(),
            Refresh::Search { query, scope } => cx.emit(AddressBarEvent::RerunFind { query, scope }),
        }
    }

    /// A crumb or menu item: opens the folder here; the keyboard stays in the bar.
    fn open(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.explorer
            .update(cx, |explorer, cx| explorer.navigate(path, NavOptions::default(), cx));
        window.focus(&self.focus, cx);
    }

    /// Middle click: the folder in a new tab, and you stay where you are (`middleClickOpens`).
    fn open_in_new_tab(&mut self, path: &str, cx: &mut Context<Self>) {
        self.explorer
            .update(cx, |explorer, cx| explorer.open_in_new_tab(path, cx));
    }

    /// Starts typing (a click on the empty part of the bar).
    fn start_typing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus(window, cx);
    }

    /// Copies the folder's path (the button at the end of the bar).
    fn copy_path(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.seen.path.clone()));
        self.copied = true;
        self.copied_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Dual-pane view: shows the other pane's folder (F6).
    fn switch_pane(&mut self, cx: &mut Context<Self>) {
        self.explorer.update(cx, |explorer, cx| explorer.switch_pane(cx));
    }
}

// ---- Drawing --------------------------------------------------------------------------------------------

/// The tooltip builder for `.tooltip(..)` (the kit's tooltip).
fn tip(text: SharedString) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |window, cx| crate::kit::tooltip(text.clone(), window, cx)
}

static SAMPLE_FOLDER: std::sync::LazyLock<FileEntry> = std::sync::LazyLock::new(|| sample("folder", true));
static SAMPLE_ARCHIVE: std::sync::LazyLock<FileEntry> = std::sync::LazyLock::new(|| sample("archive.zip", false));

fn sample_folder() -> &'static FileEntry {
    &SAMPLE_FOLDER
}

fn sample_archive() -> &'static FileEntry {
    &SAMPLE_ARCHIVE
}

/// The icon and colors of a [`Look`].
fn look_icon(look: Look, cx: &App) -> Icon {
    let t = theme::tokens(cx);
    let kind_color = |entry: &FileEntry| theme::kind_tone(glint_model::file_types::kind_of(entry), cx).color;
    let (name, color) = match look {
        Look::Drive(kind) => (drive_icon(kind), t.muted_foreground),
        Look::Known(id) => (known_icon(id), t.muted_foreground),
        Look::Folder => (IconName::Folder, kind_color(sample_folder())),
        Look::Archive => (IconName::FileArchive, kind_color(sample_archive())),
        Look::File => (IconName::File, t.muted_foreground),
        Look::Recent => (IconName::Clock, t.muted_foreground),
        Look::Gallery => (IconName::Images, t.muted_foreground),
        Look::RecycleBin => (IconName::Trash, t.muted_foreground),
        Look::Remote => (IconName::Cloud, t.muted_foreground),
    };
    icon(name).size_4().flex_none().text_color(color)
}

/// A name for the screen and for a person: a known folder in the interface's language.
fn suggestion_name(item: &Suggestion) -> String {
    match item.look {
        Look::Known(id) => i18n::translate(id.name()).to_string(),
        _ => item.name.clone(),
    }
}

impl Render for AddressBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let seen = self.seen.clone();
        let rem = window.rem_size();
        let viewport = window.viewport_size().width;
        let sm = viewport >= rem * 40.;
        let md = viewport >= rem * 48.;

        let button = |id: &'static str, glyph: IconName, label: SharedString| {
            Button::new(id)
                .ghost()
                .icon(glyph)
                .accessibility_label(label.clone())
                .tooltip(label)
        };
        let back = button("address-back", back_icon(), t!("Back (Alt+Left)"))
            .disabled(!seen.back)
            .on_click(cx.listener(|this, _, _, cx| this.go_back(cx)));
        // A narrow window shows Forward only when there's somewhere to go.
        let forward = (sm || seen.forward).then(|| {
            button("address-forward", forward_icon(), t!("Forward (Alt+Right)"))
                .disabled(!seen.forward)
                .on_click(cx.listener(|this, _, _, cx| this.go_forward(cx)))
        });
        let up = button("address-up", IconName::ArrowUp, t!("Up (Alt+Up)"))
            .disabled(seen.path == THIS_PC)
            .on_click(cx.listener(|this, _, _, cx| this.go_up(cx)));
        // The spinner stands in for the arrow while the folder loads (a loading button would stop answering).
        let refresh = md.then(|| {
            button(
                "address-refresh",
                if seen.loading {
                    IconName::LoaderCircle
                } else {
                    IconName::RotateCw
                },
                t!("Refresh (F5)"),
            )
            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
        });

        let field = self.address_field(&t, window, cx);
        let search = self.search_box(&t, window, cx);

        row_start()
            .id("address-bar")
            .role(Role::Group)
            .aria_label(t!("Address bar"))
            .test_support()
            .key_context("AddressBar")
            .track_focus(&self.focus)
            .min_w_0()
            .flex_1()
            .gap(space(2.))
            .child(
                row_start()
                    .flex_none()
                    .child(back)
                    .children(forward)
                    .child(up)
                    .children(refresh),
            )
            .child(field)
            .child(search)
    }
}

impl AddressBar {
    /// The field the crumbs or the typed path sit in: Fluent's, with the accent line under it while typing.
    fn address_field(&mut self, t: &theme::Tokens, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let seen = self.seen.clone();
        let editing = seen.editing && self.editing;
        let inner = if editing {
            self.edit_area(t, window, cx).into_any_element()
        } else {
            self.crumb_bar(t, cx).into_any_element()
        };
        let edge = if editing { t.primary } else { t.field_edge };
        row_start()
            .id("address-field")
            .test_support()
            .relative()
            .h(space(8.))
            .min_w_0()
            .flex_1()
            .items_center()
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.border)
            .bg(if editing { t.field_focus } else { t.field })
            .children(seen.dual.then(|| self.pane_side(t, cx)))
            .child(inner)
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left(px(1.))
                    .right(px(1.))
                    .h(if editing { px(2.) } else { px(1.) })
                    .bg(edge),
            )
    }

    /// Dual-pane view: which pane the bar shows; a click shows the other one's (F6).
    fn pane_side(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        // The first pane is on the left, or on the right in a right-to-left language.
        let left = (self.seen.pane == 0) != i18n::is_rtl();
        let label = t!(if left {
            "The left pane: F6 for the right one"
        } else {
            "The right pane: F6 for the left one"
        });
        Button::new("address-pane")
            .ghost()
            .xsmall()
            .icon(
                icon(if left {
                    IconName::PanelLeft
                } else {
                    IconName::PanelRight
                })
                .size_4()
                .text_color(t.primary),
            )
            .accessibility_label(label.clone())
            .tooltip(label)
            .on_click(cx.listener(|this, _, _, cx| this.switch_pane(cx)))
    }

    /// The path being typed, with the suggestions under it.
    fn edit_area(&mut self, t: &theme::Tokens, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        let list = (!self.suggestions.is_empty()).then(|| self.suggestion_list(t, cx));
        div()
            .id("address-edit")
            .key_context(keys::EDIT)
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .on_action(cx.listener(|this, _: &ListDown, _, cx| this.suggestion_down(cx)))
            .on_action(cx.listener(|this, _: &ListUp, _, cx| this.suggestion_up(cx)))
            .on_action(cx.listener(|this, _: &ListComplete, window, cx| this.complete_suggestion(window, cx)))
            .on_action(cx.listener(|this, _: &ListCancel, window, cx| {
                this.close_editing(window, cx);
                cx.emit(AddressBarEvent::FocusFiles { first: false });
            }))
            .child(
                Input::new(&self.edit)
                    .appearance(false)
                    .h_full()
                    .aria_label(t!("Address")),
            )
            .children(list)
    }

    /// "Path suggestions", with the keys that work in it, and one row for each.
    fn suggestion_list(&mut self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<_> = self
            .suggestions
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                let active = self.active == Some(ix);
                let path = item.path.clone();
                let name = suggestion_name(item);
                align_row(2.)
                    .id(("address-suggestion", ix))
                    .role(Role::ListBoxOption)
                    .aria_label(name.clone())
                    .aria_selected(active)
                    .test_support()
                    .w_full()
                    .px(space(2.))
                    .py(space(1.5))
                    .rounded(theme::size::RADIUS_SM)
                    .text_size(theme::size::SM)
                    .cursor_pointer()
                    .when(active, |row| row.bg(t.accent).text_color(t.foreground))
                    .when(!active, |row| row.hover(|s| s.bg(t.accent.opacity(0.6))))
                    // The box keeps the keyboard when a row is pressed.
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.active != Some(ix) {
                            this.active = Some(ix);
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| this.go(path.clone(), window, cx)))
                    .child(look_icon(item.look, cx))
                    .child(div().truncate().font_weight(FontWeight::MEDIUM).child(isolate(name)))
                    .children(item.detail.as_ref().map(|detail| {
                        div()
                            .ms(0.)
                            .ps(3.)
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_end()
                            .font_family(theme::mono_font(cx))
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(detail.clone())
                    }))
            })
            .collect();
        deferred(
            div().absolute().top_full().mt(space(1.)).left_0().right_0().child(
                col()
                    .id("address-suggestions")
                    .role(Role::ListBox)
                    .aria_label(t!("Path suggestions"))
                    .test_support()
                    .occlude()
                    .p(space(1.))
                    .rounded(theme::size::RADIUS)
                    .border_1()
                    .border_color(t.border)
                    .bg(t.popover)
                    .text_color(t.popover_foreground)
                    .shadow_lg()
                    .child(
                        row()
                            .justify_between()
                            .px(space(2.))
                            .py(space(1.))
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .child(t!("Path suggestions"))
                            .child(t!("Tab to complete · Enter to open")),
                    )
                    .children(rows),
            ),
        )
        .with_priority(10)
    }

    /// The crumbs: "This PC", its chevron, the folded "…", each folder and its chevron, the copy button.
    fn crumb_bar(&mut self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let seen = self.seen.clone();
        let layout = self.measure.borrow().layout;
        let on_this_pc = seen.path == THIS_PC;
        let n = seen.crumbs.len();
        let folded = layout.folded.min(n.saturating_sub(1));
        let entity = cx.entity().downgrade();

        // This PC, with its chevron: a narrow bar leaves them out for the folder you're in.
        let show_root = !layout.narrow || on_this_pc;
        let this_pc = show_root.then(|| {
            let label = t!("This PC");
            div()
                .id("address-this-pc")
                .role(Role::Button)
                .aria_label(label.clone())
                .test_support()
                .tab_index(0)
                .flex()
                .items_center()
                .flex_none()
                .gap(space(1.5))
                .h(space(6.))
                .px(space(1.5))
                .rounded(theme::size::RADIUS_SM)
                .cursor_pointer()
                .hover(|s| s.bg(t.accent))
                .focus_visible(|s| s.bg(t.accent))
                .tooltip(tip(label.clone()))
                .on_click(cx.listener(|this, _, window, cx| this.open(THIS_PC.to_string(), window, cx)))
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(|this, _, window, cx| {
                        window.prevent_default();
                        this.open_in_new_tab(THIS_PC, cx);
                    }),
                )
                .child(icon(IconName::Monitor).size_4().text_color(t.muted_foreground))
                .children(on_this_pc.then_some(label))
        });
        let roots_chevron = show_root.then(|| {
            self.chevron(
                "address-roots",
                t!("Drives and places"),
                flyout::At::Roots,
                THIS_PC.to_string(),
                seen.crumbs.first().map(|c| c.path.clone()),
                t,
                cx,
            )
        });

        // The folders above those that fit, in a menu; narrower still, only the folder you're in.
        let dots = (folded > 0 && !layout.tiny).then(|| {
            let given: Vec<Suggestion> = seen.crumbs.iter().take(folded).map(flyout::crumb_item).collect();
            let label = t!("Folders above");
            let menu = self.flyout_menu(flyout::At::Folded, cx);
            let button = div()
                .id("address-folded")
                .role(Role::Button)
                .aria_label(label.clone())
                .aria_expanded(self.flyout.as_ref().is_some_and(|f| f.at == flyout::At::Folded))
                .test_support()
                .tab_index(0)
                .flex()
                .items_center()
                .justify_center()
                .h(space(6.))
                .px(space(1.5))
                .rounded(theme::size::RADIUS_SM)
                .text_color(t.muted_foreground)
                .cursor_pointer()
                .hover(|s| s.bg(t.accent).text_color(t.foreground))
                .focus_visible(|s| s.bg(t.accent))
                .tooltip(tip(label))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_flyout(flyout::At::Folded, String::new(), None, Some(given.clone()), window, cx)
                }))
                .child(icon(IconName::Ellipsis).size_4());
            let last_folded = &seen.crumbs[folded - 1];
            let chevron = self.chevron(
                "address-folded-next",
                t!("Subfolders of {name}", name = last_folded.name),
                flyout::At::Crumb(folded - 1),
                last_folded.path.clone(),
                seen.crumbs.get(folded).map(|c| c.path.clone()),
                t,
                cx,
            );
            row_start()
                .flex_none()
                .child(div().relative().child(button).children(menu))
                .child(chevron)
        });

        // The crumbs, packed against the end so it is the first ones that run out of room; they are clipped,
        // and measured after the paint to know how many to fold into the "…".
        let cells: Vec<_> = seen
            .crumbs
            .iter()
            .enumerate()
            .map(|(i, crumb)| self.crumb_cell(i, crumb, i + 1 == n, i < folded, t, cx))
            .collect();
        let measure = self.measure.clone();
        let measured_entity = entity.clone();
        let count_folded = move |bounds: Vec<Bounds<Pixels>>, _: &mut Window, cx: &mut App| {
            // The last child is the probe below: the row's own box. The others are the crumbs.
            let Some((row, cells)) = bounds.split_last() else {
                return;
            };
            let rtl = i18n::is_rtl();
            // Clipped at the start edge: left in English, right in Arabic.
            let outside = |cell: &Bounds<Pixels>| {
                if rtl {
                    cell.right() > row.right() + px(0.5)
                } else {
                    cell.left() < row.left() - px(0.5)
                }
            };
            let folded = cells.iter().take_while(|cell| outside(cell)).count();
            let mut m = measure.borrow_mut();
            m.cells = cells.to_vec();
            if m.layout.folded != folded {
                m.layout.folded = folded;
                drop(m);
                let entity = measured_entity.clone();
                cx.defer(move |cx| {
                    entity.update(cx, |_, cx| cx.notify()).ok();
                });
            }
        };
        let crumbs_row = row_start()
            .on_children_prepainted(count_folded)
            .id("address-crumbs")
            .role(Role::List)
            .aria_label(t!("Folders"))
            .test_support()
            .relative()
            .h(space(6.))
            .min_w(space(12.))
            .overflow_hidden()
            .when(!i18n::is_rtl(), |r| r.justify_end())
            .when(i18n::is_rtl(), |r| r.justify_start())
            .children(cells)
            .child(canvas(|_, _, _| (), |_, _, _, _| ()).absolute().size_full())
            // Files held over a folder crumb: a drop there goes into the folder, and the drag's words say so.
            // (One listener for the row, finding the crumb under the pointer from where they were painted.)
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                let at = event.event.position;
                let under = if event.bounds.contains(&at) {
                    let cells = this.measure.borrow().cells.clone();
                    cells.iter().position(|cell| cell.contains(&at))
                } else {
                    None
                }
                .filter(|&i| this.seen.crumbs.get(i).is_some_and(crumb_takes_files));
                if let Some(i) = under {
                    let name = this.seen.crumbs[i].name.clone();
                    crate::drops::answer(
                        cx,
                        crate::platform::dragdrop::Answer::Accept(crate::components::tab_strip::drop_words(&name)),
                    );
                }
                if this.drop_over != under {
                    this.drop_over = under;
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(|this, drop: &ExternalPaths, _, cx| {
                let Some(i) = this.drop_over.take() else {
                    return;
                };
                cx.notify();
                let Some(dest) = this
                    .seen
                    .crumbs
                    .get(i)
                    .filter(|c| crumb_takes_files(c))
                    .map(|c| c.path.clone())
                else {
                    return;
                };
                let moves = crate::drops::effect(cx) == crate::platform::dragdrop::DropEffect::Move;
                let paths = crate::drops::paths(drop);
                this.explorer
                    .update(cx, |e, cx| e.drop_files(paths, &dest, moves, cx).detach());
            }));

        // Measures the bar itself, for the classes that drop "This PC" and the "…" when it is narrow.
        let measure = self.measure.clone();
        let bar_probe = canvas(
            move |bounds, window, cx| {
                let width = bounds.size.width;
                let rem = window.rem_size();
                let (narrow, tiny, no_chip) = (width < rem * 16., width < rem * 9., width < rem * 26.);
                let mut m = measure.borrow_mut();
                if m.layout.narrow != narrow || m.layout.tiny != tiny || m.layout.no_chip != no_chip {
                    m.layout.narrow = narrow;
                    m.layout.tiny = tiny;
                    m.layout.no_chip = no_chip;
                    drop(m);
                    let entity = entity.clone();
                    cx.defer(move |cx| {
                        entity.update(cx, |_, cx| cx.notify()).ok();
                    });
                }
            },
            |_, _, _, _| (),
        )
        .absolute()
        .size_full();

        // The empty part: a click types a path.
        let type_label = t!("Type a path (Ctrl+L)");
        let empty = div()
            .id("address-type")
            .role(Role::Button)
            .aria_label(type_label.clone())
            .test_support()
            .tab_index(0)
            .h_full()
            .min_w(space(4.))
            .flex_1()
            .cursor_text()
            .tooltip(tip(type_label))
            .on_click(cx.listener(|this, _, window, cx| this.start_typing(window, cx)));

        // The copy button shows when the pointer is over the bar (and for a moment after a copy).
        let copy = (!on_this_pc && !seen.crumbs.first().is_some_and(|c| c.place)).then(|| {
            let copied = self.copied;
            let label = t!("Copy folder path");
            div()
                .id("address-copy")
                .role(Role::Button)
                .aria_label(label)
                .test_support()
                .tab_index(0)
                .flex()
                .items_center()
                .justify_center()
                .flex_none()
                .size(space(6.))
                .rounded(theme::size::RADIUS_SM)
                .cursor_pointer()
                .text_color(if copied { t.success } else { t.muted_foreground })
                .opacity(if copied { 1. } else { 0. })
                .group_hover("address-bar-row", |s| s.opacity(1.))
                .focus_visible(|s| s.opacity(1.).bg(t.accent))
                .hover(|s| s.bg(t.accent).text_color(t.foreground))
                .tooltip(tip(t!("Copy folder path (Ctrl+Shift+C)")))
                .on_click(cx.listener(|this, _, _, cx| this.copy_path(cx)))
                .child(icon(if copied { IconName::Check } else { IconName::Copy }).size_3p5())
        });

        // The branch chip of this tab, after the crumbs; it draws nothing outside a repository, and leaves no gap.
        let chip = self.chip_of(seen.tab, layout.no_chip, cx);

        row_start()
            .id("address-crumb-bar")
            .test_support()
            .group("address-bar-row")
            .relative()
            .h_full()
            .min_w_0()
            .flex_1()
            .items_center()
            .overflow_hidden()
            .px(space(1.))
            .child(bar_probe)
            .children(this_pc)
            .children(roots_chevron)
            .children(dots)
            .child(crumbs_row)
            .children(chip)
            .child(empty)
            .children(copy)
    }

    /// The Git chip of `tab` (it has its own gap), or nothing: while the tab's folder isn't in a repository, or the bar is
    /// narrower than 26 rem. One chip per tab is made when the tab first shows, and dropped with the tab.
    fn chip_of(&mut self, tab: TabId, hidden: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        // Without a Git model (`git::init` not run) there is nothing for a chip to show.
        git::try_shared(cx)?;
        let explorer = self.explorer.clone();
        self.chips.retain(|id, _| explorer.read(cx).tab(*id).is_some());
        let chip = match self.chips.get(&tab) {
            Some(chip) => chip.clone(),
            None => {
                let chip = cx.new(|cx| GitChip::new(tab, cx));
                self.chips.insert(tab, chip.clone());
                chip
            }
        };
        (!hidden && chip.read(cx).is_shown(cx)).then(|| div().flex_none().child(chip).into_any_element())
    }

    /// One folder of the path: its button and the chevron after it. Folded ones stay in the row, invisible,
    /// so the room they take can be measured.
    fn crumb_cell(
        &mut self,
        i: usize,
        crumb: &CrumbView,
        last: bool,
        folded: bool,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let title: SharedString = if crumb.archive {
            t!("An archive, opened like a folder (read-only)")
        } else if is_recycle_bin(&crumb.path) {
            t!("What you deleted, to restore or delete for good")
        } else if crumb.remote_top {
            t!("A remote connection, opened read-only")
        } else {
            crumb.path.clone().into()
        };
        let (path, middle) = (crumb.path.clone(), crumb.path.clone());
        let name = crumb.name.clone();
        // A folder on disk takes files, and can be dragged out: not an archive, a remote, This PC, the Recycle
        // Bin, Recent or the Gallery (`writableCrumb`).
        let writable = crumb_takes_files(crumb);
        let (press_path, move_path) = (crumb.path.clone(), crumb.path.clone());
        let button = div()
            .id(("address-crumb", i))
            .role(Role::Button)
            .aria_label(name.clone())
            .aria_description(title.clone())
            .test_support()
            .tab_index(0)
            .flex()
            .items_center()
            .gap(space(1.))
            .h(space(6.))
            .min_w_0()
            .px(space(1.5))
            .rounded(theme::size::RADIUS_SM)
            .cursor_pointer()
            .when(last, |b| b.font_weight(FontWeight::MEDIUM))
            .hover(|s| s.bg(t.accent))
            .focus_visible(|s| s.bg(t.accent))
            .active(|s| s.bg(t.accent))
            .tooltip(tip(title))
            .when(writable && self.drop_over == Some(i) && cx.has_active_drag(), |b| {
                b.bg(t.primary.opacity(0.2))
                    .text_color(t.primary)
                    .border_1()
                    .border_color(t.primary)
            })
            .when(writable, |b| {
                // Pressed and pulled away, the crumb drags its folder out to File Explorer, the desktop…
                b.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, _| {
                        this.crumb_press = Some((press_path.clone(), event.position));
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, _| this.crumb_press = None),
                )
                .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                    let Some((path, from)) = this.crumb_press.clone() else {
                        return;
                    };
                    if event.pressed_button != Some(MouseButton::Left) {
                        this.crumb_press = None;
                        return;
                    }
                    let (dx, dy) = (event.position.x - from.x, event.position.y - from.y);
                    if (f32::from(dx).powi(2) + f32::from(dy).powi(2)).sqrt() < DRAG_THRESHOLD || path != move_path {
                        return;
                    }
                    this.crumb_press = None;
                    let drag = crate::platform::dragdrop::drag_out(cx, crate::workspace::MAIN, vec![path], false);
                    cx.spawn(async move |_, _| {
                        if let Err(error) = drag.await {
                            eprintln!("glint: couldn't drag the folder: {error}");
                        }
                    })
                    .detach();
                }))
            })
            .on_click(cx.listener(move |this, _, window, cx| this.open(path.clone(), window, cx)))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| {
                    window.prevent_default();
                    this.open_in_new_tab(&middle, cx);
                }),
            )
            .when(crumb.archive, |b| b.child(look_icon(Look::Archive, cx).size_3p5()))
            .when(crumb.remote_top, |b| {
                b.child(icon(IconName::Cloud).size_3p5().flex_none().text_color(t.primary))
            })
            .child(div().min_w_0().truncate().child(isolate(name)));
        // What's in the Recycle Bin, Recent or the Gallery isn't a folder to go into.
        let chevron = (!crumb.place).then(|| {
            self.chevron(
                ("address-chevron", i),
                t!("Subfolders of {name}", name = crumb.name),
                flyout::At::Crumb(i),
                crumb.path.clone(),
                self.seen.crumbs.get(i + 1).map(|c| c.path.clone()),
                t,
                cx,
            )
        });
        row_start()
            .items_center()
            // The folder you're in keeps its room (up to the whole row, then its name is cut); the ones above it
            // are what gives way.
            .flex_none()
            .when(last, |c| c.min_w_0().max_w_full())
            .when(folded, |c| c.invisible())
            .child(button)
            .children(chevron)
    }
}

/// Whether a folder crumb takes files dropped on it and can be dragged out: a folder on disk, not an archive, a
/// remote, This PC, the Recycle Bin, Recent or the Gallery (`writableCrumb`).
fn crumb_takes_files(crumb: &CrumbView) -> bool {
    !crumb.archive
        && !in_archive_path(&crumb.path)
        && !is_remote_path(&crumb.path)
        && !is_recycle_bin(&crumb.path)
        && !is_recent(&crumb.path)
        && !is_gallery(&crumb.path)
        && crumb.path != THIS_PC
}

/// A folder or an archive as a sample entry, which the kinds' colors are taken from.
fn sample(name: &str, dir: bool) -> FileEntry {
    use glint_model::entries::{attr, with_details, RawEntry};
    with_details(
        RawEntry {
            name: name.to_string(),
            dir,
            attrs: if dir { attr::DIRECTORY } else { 0 },
            ..Default::default()
        },
        "",
    )
}

#[cfg(test)]
mod tests;
