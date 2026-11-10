//! The main window: the title bar with the tab strip, the address bar, the resizable sidebar, one or two file
//! panes, the status bar. Wave 4's views fill it; this file places them and wires what they hand out (their
//! events, and the window's actions) to the explorer model and to the parts that own the rest.
//!
//! | Area                    | View                                                                 |
//! |-------------------------|----------------------------------------------------------------------|
//! | Title bar               | gpui-component's `TitleBar` (snap layouts), holding the [`TabStrip`] |
//! | Toolbar                 | the mode switch (Files, Text), the [`AddressBar`], Sharing, Disk usage |
//! | Drive health button     | **temporary** (This PC's tiles and banner open it too)               |
//! | Sidebar                 | the [`Sidebar`]: places and the folder tree                          |
//! | Each pane               | [`ThisPc`], the [`DiskUsage`] map, or the [`FileGrid`] or the [`FileList`] |
//! | Status bar              | the [`StatusBar`]                                                    |
//! | Text mode (Ctrl+Shift+F) | [`TextSearch`] instead of the sidebar, the panes and the status bar |
//! | Over everything         | the [`CopyQueue`] panel and the [`MenuHost`]'s menus                 |
//!
//! **Patterns here:**
//! - The root has `key_context("Workspace")` and tracks focus, so the key bindings in [`crate::actions`] are
//!   live; handlers are `.on_action(cx.listener(...))` on the root. The views handle their own contexts
//!   (FileList, AddressBar...) and let the rest bubble up to here.
//! - Each view reads the explorer itself; the workspace only decides which view a pane shows and forwards
//!   what the views can't do alone. What belongs to a later part says so with [`Workspace::not_yet`].
//! - The window's background material (Mica) follows the theme: while it is on, chrome is see-through.

use crate::components::address_bar::{AddressBar, AddressBarEvent};
use crate::components::copy_queue::{CopyQueue, CopyQueueEvent};
use crate::components::disk_usage::{self, DiskUsage, DiskUsageEvent};
use crate::components::file_grid::{FileGrid, FileGridEvent};
use crate::components::file_list::DragFromArchive;
use crate::components::file_list::{FileList, FileListEvent};
use crate::components::menus::{self, MenuEvent, MenuHost};
use crate::components::preview::{self, PreviewPane};
use crate::components::settings::SettingsEvent;
use crate::components::sidebar::{Sidebar, SidebarEvent, SidebarRequest};
use crate::components::status_bar::StatusBar;
use crate::components::tab_strip::{PaneDropZone, TabStrip};
use crate::components::text_search::{TextSearch, TextSearchEvent};
use crate::components::this_pc::{ThisPc, ThisPcEvent};
use crate::components::{palette, settings};
use crate::explorer::{self, ExplorerEvent};
use crate::kit::prelude::*;
use crate::platform::dragdrop;
use crate::{actions, core_bridge, dialogs, notices, state, t, theme};
use glint_core::host::Manager as _;
use glint_model::entries::FileEntry;
use glint_model::format::plural;
use glint_model::paths::{base_name, same_path};
use glint_model::store::ViewMode;
use std::ops::Range;

/// The label glint-core knows the main window by.
pub const MAIN: &str = "main";

/// The sidebar's width: what it starts at, and how far it can be dragged.
const SIDEBAR: Range<Pixels> = px(160.)..px(420.);
const SIDEBAR_START: Pixels = px(240.);

/// What a pane can show: one of each, kept for the window's life so each keeps its scroll and its state.
struct PaneViews {
    list: Entity<FileList>,
    grid: Entity<FileGrid>,
    this_pc: Entity<ThisPc>,
    /// The map of what fills the tab's folder, while the tab shows it (`disk_usage::visible`).
    usage: Entity<DiskUsage>,
}

/// Which of a pane's views is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shown {
    ThisPc,
    DiskUsage,
    Grid,
    List,
}

/// The main window's content.
pub struct Workspace {
    focus: FocusHandle,
    tabs: Entity<TabStrip>,
    address: Entity<AddressBar>,
    sidebar: Entity<Sidebar>,
    status: Entity<StatusBar>,
    /// The copy queue's panel (or its pill), over the bottom end of the window.
    queue: Entity<CopyQueue>,
    /// The item, background and folder menus, drawn over everything when one is open.
    menus: Entity<MenuHost>,
    /// The open Settings view's events, while it is open.
    settings_events: Option<Subscription>,
    /// The preview pane at the end edge, while the setting shows it: dropped while hidden, since Windows'
    /// previewers are windows of their own that go only with it.
    preview: Option<Entity<PreviewPane>>,
    /// The first pane and, in dual-pane view, the second.
    panes: [PaneViews; 2],
    /// Searching inside files: made the first time it is asked for (its install cards look at every tool once),
    /// then kept for the window's life, so its results keep their place while the files are shown.
    text: Option<Entity<TextSearch>>,
    /// Text search is on screen instead of the files (the web app's `mode === "text"`).
    text_mode: bool,
    /// What is shared or being sent, for the toolbar's Sharing button (None without core, as in tests).
    sharing: Option<Entity<dialogs::share::Sharing>>,
    /// What each pane showed at the last look, so a grid going out of sight lets its thumbnails go.
    shown: [Option<Shown>; 2],
    /// Files dragged over the window from File Explorer and other apps arrive here, as long as it lives
    /// (dropping it takes the window's drop target away). None in tests, which have no Windows window.
    _drop_target: Option<dragdrop::DropTarget>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    /// Builds the window's views, their wiring, and the window's place in core's window list.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        // Windows switched between light and dark: the "System" theme follows it.
        subscriptions.push(window.observe_window_appearance(|_, cx| theme::on_appearance_changed(cx)));
        // The theme changed (the setting, Windows' accent, Mica): the window's material follows, and it redraws.
        subscriptions.push(cx.observe_global_in::<theme::GlintTheme>(window, |_, window, cx| {
            window.set_background_appearance(theme::window_background(cx));
            cx.notify();
        }));

        core_bridge::register_window(cx, MAIN, window);
        // Leaving core's window list when this view goes away with its window.
        subscriptions.push(cx.on_release(|_, cx| core_bridge::unregister_window(cx, MAIN)));

        let tabs = cx.new(|cx| TabStrip::new(window, cx));
        let address = cx.new(|cx| AddressBar::new(window, cx));
        let sidebar = cx.new(|cx| Sidebar::new(window, cx));
        let status = cx.new(|cx| StatusBar::new(window, cx));
        let queue = cx.new(|cx| CopyQueue::new(window, cx));
        let menus = cx.new(MenuHost::new);
        let panes = [0, 1].map(|pane| PaneViews {
            list: cx.new(|cx| FileList::new(pane, window, cx)),
            grid: cx.new(|cx| FileGrid::new(pane, window, cx)),
            this_pc: cx.new(|cx| ThisPc::new(pane, window, cx)),
            usage: cx.new(|cx| DiskUsage::new(pane, window, cx)),
        });

        subscriptions.push(cx.subscribe_in(&address, window, Self::on_address_event));
        subscriptions.push(cx.subscribe_in(&sidebar, window, Self::on_sidebar_event));
        subscriptions.push(cx.subscribe_in(&sidebar, window, Self::on_sidebar_request));
        subscriptions.push(cx.subscribe_in(&menus, window, Self::on_menu_event));
        subscriptions.push(cx.subscribe_in(&queue, window, Self::on_queue_event));
        for (pane, views) in panes.iter().enumerate() {
            subscriptions.push(cx.subscribe_in(&views.list, window, move |this, _, event, window, cx| {
                this.on_list_event(pane, event, window, cx)
            }));
            // Items inside an archive are extracted first, then dragged as copies.
            subscriptions.push(
                cx.subscribe_in(&views.list, window, |_, _, event: &DragFromArchive, _, cx| {
                    Self::drag_from_archive(event.paths.clone(), None, cx)
                }),
            );
            subscriptions.push(cx.subscribe_in(&views.grid, window, move |this, _, event, window, cx| {
                this.on_grid_event(pane, event, window, cx)
            }));
            subscriptions.push(
                cx.subscribe_in(&views.this_pc, window, move |this, _, event, window, cx| {
                    this.on_this_pc_event(event, window, cx)
                }),
            );
            subscriptions.push(
                cx.subscribe_in(&views.usage, window, move |this, _, event, window, cx| {
                    this.on_usage_event(pane, event, window, cx)
                }),
            );
        }
        // A tab's map turned on or off changes what its pane shows.
        let usage = disk_usage::shared(cx);
        subscriptions.push(cx.observe_in(&usage, window, |this, _, window, cx| this.follow_panes(window, cx)));
        // The Sharing button follows what is shared; back in front, what keeps other devices out is read again.
        let sharing = cx.has_global::<core_bridge::Core>().then(|| dialogs::share::shared(cx));
        if let Some(sharing) = &sharing {
            subscriptions.push(cx.observe(sharing, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    if let Some(sharing) = this.sharing.clone() {
                        sharing.update(cx, |sharing, cx| sharing.recheck(cx));
                    }
                }
            }));
        }
        // The explorer decides what each pane shows; the keyboard follows the active pane when it moves.
        let explorer = explorer::explorer(cx);
        subscriptions.push(cx.observe_in(&explorer, window, |this, _, window, cx| this.follow_panes(window, cx)));
        // The preview pane comes and goes with its setting (Alt+P) and the folder shown.
        subscriptions
            .push(cx.observe_global_in::<state::UiState>(window, |this, window, cx| this.sync_preview(window, cx)));
        subscriptions.push(
            cx.subscribe_in(&explorer, window, |this, _, event, window, cx| match event {
                ExplorerEvent::PaneFocused => this.focus_files(window, cx),
                // What the file operations tell the person, and ask first.
                ExplorerEvent::Operation(notice) => notices::show(notice, window, cx),
                ExplorerEvent::Busy(busy) => notices::busy(busy, window, cx),
                ExplorerEvent::Ask(question) => notices::ask(question, window, cx),
                ExplorerEvent::QueueOpened => this.queue.update(cx, |queue, cx| queue.show(cx)),
                ExplorerEvent::BatchRename(paths) => dialogs::batch_rename::open(window, cx, paths.clone()),
                ExplorerEvent::Find(notice) => notices::find(notice, window, cx),
                // Out of an archive to the other pane or the clipboard: extracted first, then copied.
                ExplorerEvent::FromArchive(job) => {
                    let job = job.clone();
                    Self::explorer(cx, |e, cx| e.run_archive_job(job, cx).detach())
                }
                ExplorerEvent::Archive(notice) => notices::archive(notice, window, cx),
                // Opening an item in the Recycle Bin asks for Quick Look through the explorer's flag.
                ExplorerEvent::TabChanged(_) | ExplorerEvent::SelectionChanged(_) | ExplorerEvent::TabsChanged => {
                    preview::quick_look::sync(window, cx)
                }
                _ => {}
            }),
        );
        // The file view has the keyboard from the start, so the arrows work at once.
        cx.defer_in(window, |this, window, cx| this.focus_files(window, cx));

        Self {
            focus: cx.focus_handle(),
            tabs,
            address,
            sidebar,
            status,
            queue,
            menus,
            settings_events: None,
            preview: None,
            panes,
            text: None,
            text_mode: false,
            sharing,
            shown: [None, None],
            _drop_target: Self::take_drops(window, cx),
            _subscriptions: subscriptions,
        }
    }

    /// Text search, once it has been asked for, and whether it is on screen (the self-test reads them).
    pub fn text_search(&self) -> (Option<Entity<TextSearch>>, bool) {
        (self.text.clone(), self.text_mode)
    }

    /// Whether the copy queue still has a copy or move going (quitting asks first).
    pub fn copy_queue_running(&self, cx: &App) -> bool {
        self.queue.read(cx).running()
    }

    /// What pane `pane` shows now, or None when it isn't on screen.
    fn shown_in(pane: usize, cx: &App) -> Option<Shown> {
        let explorer = explorer::explorer(cx);
        let explorer = explorer.read(cx);
        if pane >= explorer.shown_panes().len() {
            return None;
        }
        let tab = explorer.pane_tab(pane)?;
        Some(if tab.is_this_pc() {
            Shown::ThisPc
        } else if disk_usage::visible(cx, tab.id) {
            Shown::DiskUsage
        } else if tab.view == ViewMode::Grid {
            Shown::Grid
        } else {
            Shown::List
        })
    }

    /// The explorer changed: a grid that went out of sight lets its thumbnails go, and the window redraws.
    fn follow_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = explorer::explorer(cx).read(cx).active_pane().min(1);
        for pane in 0..2 {
            let now = Self::shown_in(pane, cx);
            // The view with the keyboard gave way to another (the list to its map, This PC to a folder): the new
            // one takes it, or the keys would go nowhere until a click.
            if pane == active && now != self.shown[pane] && self.view_focused(pane, self.shown[pane], window, cx) {
                cx.defer_in(window, |this, window, cx| this.focus_files(window, cx));
            }
            if self.shown[pane] == Some(Shown::Grid) && now != Some(Shown::Grid) {
                self.panes[pane].grid.update(cx, |grid, cx| grid.pause(cx));
            }
            if self.shown[pane] == Some(Shown::List) && now != Some(Shown::List) {
                self.panes[pane].list.update(cx, |list, cx| list.pause(cx));
            }
            self.shown[pane] = now;
        }
        self.sync_preview(window, cx);
        cx.notify();
    }

    /// Whether pane `pane`'s view `shown` has the keyboard (This PC's: the window keeps it, see `focus_files`).
    fn view_focused(&self, pane: usize, shown: Option<Shown>, window: &Window, cx: &App) -> bool {
        let views = &self.panes[pane];
        match shown {
            Some(Shown::List) => views.list.focus_handle(cx).contains_focused(window, cx),
            Some(Shown::Grid) => views.grid.read(cx).focus_handle().contains_focused(window, cx),
            Some(Shown::DiskUsage) => views.usage.read(cx).focus_handle().contains_focused(window, cx),
            Some(Shown::ThisPc) => self.focus.is_focused(window) || views.this_pc.read(cx).has_focus(window),
            None => false,
        }
    }

    /// Makes or drops the preview pane as its setting and the folder shown say (`preview::should_show`).
    fn sync_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match (preview::should_show(cx), self.preview.is_some()) {
            (true, false) => {
                self.preview = Some(cx.new(|cx| PreviewPane::new(window, cx)));
                cx.notify();
            }
            (false, true) => {
                self.preview = None;
                cx.notify();
            }
            _ => {}
        }
    }

    /// Gives the keyboard to the active pane's file view (the list, the grid, the map, or This PC's first tile).
    /// Text search keeps it while it is on screen.
    fn focus_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.text_mode {
            return;
        }
        let pane = explorer::explorer(cx).read(cx).active_pane().min(1);
        match Self::shown_in(pane, cx) {
            Some(Shown::DiskUsage) => {
                let handle = self.panes[pane].usage.read(cx).focus_handle().clone();
                window.focus(&handle, cx);
            }
            Some(Shown::Grid) => {
                let handle = self.panes[pane].grid.read(cx).focus_handle().clone();
                window.focus(&handle, cx);
            }
            Some(Shown::List) => self.panes[pane].list.update(cx, |list, cx| list.focus(window, cx)),
            // This PC's tiles take the keyboard when clicked or tabbed to; the window keeps it meanwhile.
            Some(Shown::ThisPc) | None => window.focus(&self.focus, cx),
        }
    }

    /// Makes the window a drop target; what is dragged over it goes to the views as GPUI's file drops (drops.rs).
    fn take_drops(window: &mut Window, cx: &mut Context<Self>) -> Option<dragdrop::DropTarget> {
        let hwnd = core_bridge::hwnd_of(window)?;
        let target = match dragdrop::register(hwnd) {
            Ok(target) => target,
            Err(error) => {
                eprintln!("glint: the window won't take dropped files: {error}");
                return None;
            }
        };
        let events = target.events.clone();
        let answers = target.answers.clone();
        // Handed to GPUI as its own file drops, outside any entity update: the views take them (drops.rs).
        cx.spawn_in(window, async move |_, cx| {
            while let Ok(event) = events.recv().await {
                if cx
                    .update(|window, cx| crate::drops::dispatch(event, &answers, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Some(target)
    }

    /// The marker for something whose part of the port isn't built yet: a notice saying so, so the wiring is
    /// visible.
    fn not_yet(window: &mut Window, cx: &mut App, what: SharedString) {
        window.push_notification(
            Notification::info(t!("{what} comes with its part of the port.", what = what)),
            cx,
        );
    }

    /// Runs `act` on the explorer model.
    fn explorer(cx: &mut App, act: impl FnOnce(&mut explorer::Explorer, &mut Context<explorer::Explorer>)) {
        explorer::explorer(cx).update(cx, act);
    }

    // ---- Text mode ------------------------------------------------------------------------------------------

    /// Text search in `folder` (the open folder when None): Ctrl+Shift+F, the mode switch, the folder menu's
    /// and the sidebar's "Search text here" (`searchText`). The files stay when the view says why it can't
    /// search there (inside an archive, a folder Windows keeps closed).
    fn enter_text(&mut self, folder: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let text = match &self.text {
            Some(text) => text.clone(),
            None => {
                let text = cx.new(|cx| TextSearch::new(window, cx));
                self._subscriptions
                    .push(cx.subscribe_in(&text, window, Self::on_text_event));
                self.text = Some(text.clone());
                text
            }
        };
        if text.update(cx, |text, cx| text.enter(folder, window, cx)) {
            self.text_mode = true;
            cx.notify();
        }
    }

    /// Back to the files (Ctrl+Shift+E, the mode switch's Files); text search keeps its query and results.
    fn leave_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.text_mode {
            return;
        }
        self.text_mode = false;
        cx.notify();
        self.focus_files(window, cx);
    }

    fn on_text_event(
        &mut self,
        _: &Entity<TextSearch>,
        event: &TextSearchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // "Show in Files": the file's folder, with the file selected (`revealInApp`).
            TextSearchEvent::Reveal { path } => {
                self.leave_text(window, cx);
                let folder = glint_model::paths::parent_path(path);
                let options = explorer::NavOptions {
                    select: Some(vec![path.clone()]),
                    ..Default::default()
                };
                Self::explorer(cx, |e, cx| e.navigate(folder, options, cx));
            }
        }
    }

    /// Files or Text, as the web app's mode switch: browsing, or searching inside the files with ripgrep.
    fn mode_switch(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.text_mode;
        row_start()
            .id("mode-switch")
            .role(Role::TabList)
            .aria_label(t!("Mode"))
            .test_support()
            .flex_none()
            .gap(px(2.))
            .p(px(2.))
            .rounded_md()
            .bg(t.foreground.opacity(0.05))
            .child(
                Button::new("mode-files")
                    .ghost()
                    .small()
                    .selected(!text)
                    .icon(IconName::FolderOpen)
                    .label(t!("Files"))
                    // The file list is "Files" to a screen reader already; the name keeps the word it shows.
                    .accessibility_label(t!("Files mode"))
                    .tooltip(t!("Browse files (Ctrl+Shift+E)"))
                    .on_click(cx.listener(|this, _, window, cx| this.leave_text(window, cx))),
            )
            .child(
                Button::new("mode-text")
                    .ghost()
                    .small()
                    .selected(text)
                    .icon(IconName::TextSearch)
                    .label(t!("Text"))
                    .accessibility_label(t!("Text mode"))
                    .tooltip(t!("Search inside files with ripgrep (Ctrl+Shift+F)"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        if !this.text_mode {
                            this.enter_text(None, window, cx)
                        }
                    })),
            )
    }

    // ---- Archives, comparing, compressing ---------------------------------------------------------------------

    /// Items inside an archive dragged out: extracted first, then dragged as copies (the same items dragged again
    /// go at once). `grid` is the grid they were dragged from, whose tiles stop looking lifted afterwards.
    fn drag_from_archive(paths: Vec<String>, grid: Option<Entity<FileGrid>>, cx: &mut App) {
        let explorer = explorer::explorer(cx);
        let extracting = explorer.update(cx, |e, cx| e.drag_from_archive(paths.clone(), cx));
        cx.spawn(async move |cx| {
            // A failure was told already (and a cancel says nothing).
            let copies = extracting.await;
            let effect = match &copies {
                Ok(copies) => Some(cx.update(|cx| dragdrop::drag_out(cx, MAIN, copies.clone(), true)).await),
                Err(_) => None,
            };
            cx.update(|cx| {
                if let Some(grid) = grid {
                    grid.update(cx, |grid, cx| grid.end_drag(cx));
                }
                match (copies, effect) {
                    (Ok(copies), Some(Ok(effect))) if effect == "released" => {
                        explorer.update(cx, |e, cx| e.drag_released(&paths, copies, cx))
                    }
                    (_, Some(Err(message))) => explorer.update(cx, |e, cx| e.drag_failed(message, cx)),
                    _ => {}
                }
            });
        })
        .detach();
    }

    /// The file the menu names, as the active tab lists it (an archive's items open from their entries).
    fn entry_of(path: &str, cx: &App) -> Option<FileEntry> {
        let explorer = explorer::explorer(cx);
        let explorer = explorer.read(cx);
        explorer
            .active_tab()?
            .rows
            .iter()
            .find(|entry| entry.path == path)
            .cloned()
    }

    /// A file inside an archive, extracted first, then opened `how`.
    fn open_archived(path: &str, how: explorer::OpenHow, cx: &mut App) {
        if let Some(entry) = Self::entry_of(path, cx) {
            Self::explorer(cx, |e, cx| e.open_archived_with(&entry, how, cx).detach());
        }
    }

    /// The two files Ctrl+M compares (`comparePair`): the two selected here, the one selected here against the one
    /// selected in the other pane, or its namesake in the other pane's folder. The first pane's is on the left.
    fn compare_pair(cx: &App) -> Option<(String, String)> {
        let explorer = explorer::explorer(cx);
        let explorer = explorer.read(cx);
        let here = explorer.active_pane().min(1);
        let files_chosen = |tab: &explorer::Tab| -> Vec<FileEntry> {
            tab.rows
                .iter()
                .filter(|e| tab.selection.contains(&e.path) && !e.dir && menus::on_disk(e))
                .cloned()
                .collect()
        };
        let tab = explorer.active_tab()?;
        let own = files_chosen(tab);
        if own.len() == 2 && tab.selection.len() == 2 {
            return Some((own[0].path.clone(), own[1].path.clone()));
        }
        let [one] = own.as_slice() else { return None };
        if !explorer.is_dual() {
            return None;
        }
        let other = explorer.pane_tab(1 - here)?;
        let ordered = |a: &FileEntry, b: &FileEntry| {
            if here == 0 {
                (a.path.clone(), b.path.clone())
            } else {
                (b.path.clone(), a.path.clone())
            }
        };
        if let [theirs] = files_chosen(other).as_slice() {
            if !same_path(&one.path, &theirs.path) {
                return Some(ordered(one, theirs));
            }
        }
        if other.archive().is_some() || same_path(&tab.path, &other.path) {
            return None;
        }
        let namesake = other
            .entries()
            .iter()
            .find(|e| !e.dir && e.lower == one.lower && menus::on_disk(e))?;
        Some(ordered(one, namesake))
    }

    /// Ctrl+M: compares the pair, or says how to choose one.
    fn compare_selected(window: &mut Window, cx: &mut App) {
        match Self::compare_pair(cx) {
            Some((left, right)) => dialogs::compare_files::open(window, cx, left, right),
            None => window.push_notification(
                Notification::info(t!("Or one in each pane, side by side (Ctrl+Shift+D)."))
                    .title(t!("Select two files to compare")),
                cx,
            ),
        }
    }

    /// Compress to: a .zip, .7z or .tar.zst beside the items, after looking for secrets in them.
    fn compress(paths: Vec<String>, format: explorer::CompressFormat, window: &mut Window, cx: &mut App) {
        dialogs::check::secrets_check_first(window, cx, paths.clone(), t!("Compress anyway"), move |_, cx| {
            let paths = paths.clone();
            Self::explorer(cx, |e, cx| e.compress(paths, format, None, cx).detach())
        });
    }

    /// Compress with a password: after looking for secrets, the password (a .7z by default; a .zip keeps its
    /// names readable), then the archive beside the items.
    fn compress_with_password(paths: Vec<String>, window: &mut Window, cx: &mut App) {
        let what = match paths.as_slice() {
            [one] => format!("“{}”", base_name(one)),
            _ => plural(paths.len(), "item"),
        };
        dialogs::check::secrets_check_first(window, cx, paths.clone(), t!("Compress anyway"), move |window, cx| {
            let paths = paths.clone();
            dialogs::password::ask_compress_password(
                window,
                cx,
                what.clone(),
                dialogs::password::Format::SevenZ,
                move |format, password, _, cx| {
                    let format = match format {
                        dialogs::password::Format::Zip => explorer::CompressFormat::Zip,
                        dialogs::password::Format::SevenZ => explorer::CompressFormat::SevenZ,
                    };
                    let password = (!password.is_empty()).then_some(password);
                    let paths = paths.clone();
                    Self::explorer(cx, |e, cx| e.compress(paths, format, password, cx).detach())
                },
            )
        });
    }

    /// The list or the grid for the active tab (`chooseView`): a map it showed goes first.
    fn choose_view(view: ViewMode, cx: &mut App) {
        let tab = explorer::explorer(cx).read(cx).active_tab_id();
        disk_usage::set(cx, tab, false);
        Self::explorer(cx, |e, cx| e.set_view(view, None, cx));
    }

    /// Disk usage, or back to the file list, for the active tab (`toggleUsage`): only in a folder, not a search.
    fn toggle_disk_usage(cx: &mut App) {
        let tab = explorer::explorer(cx).read(cx).active_tab_id();
        if Self::can_map(cx) {
            disk_usage::toggle(cx, tab);
        }
    }

    /// Whether the active tab can show a map: a folder on a drive, no search running in it.
    fn can_map(cx: &App) -> bool {
        let explorer = explorer::explorer(cx);
        let explorer = explorer.read(cx);
        explorer
            .active_tab()
            .is_some_and(|tab| tab.find.is_none() && glint_model::paths::is_folder_path(&tab.path))
    }

    // ---- The views' events ----------------------------------------------------------------------------------

    fn on_address_event(
        &mut self,
        _: &Entity<AddressBar>,
        event: &AddressBarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            AddressBarEvent::FocusFiles { first } => {
                if *first {
                    // Down out of the search box: onto the first item.
                    Self::explorer(cx, |e, cx| {
                        let first = e
                            .active_tab()
                            .and_then(|tab| tab.rows.get(0))
                            .map(|entry| entry.path.clone());
                        if let Some(first) = first {
                            e.select_only(Some(&first), cx);
                        }
                    });
                }
                self.focus_files(window, cx);
            }
            // The bar filters the folder as you type; the explorer searches below it.
            AddressBarEvent::FindAsTyped { query } => Self::explorer(cx, |e, cx| e.find_as_typed(query, cx).detach()),
            AddressBarEvent::RunFind { query } => {
                Self::explorer(cx, |e, cx| e.run_find(query, explorer::Wider::default(), cx).detach())
            }
            AddressBarEvent::LeaveFind => Self::explorer(cx, |e, cx| e.leave_find(None, cx)),
            AddressBarEvent::ClearFind => Self::explorer(cx, |e, cx| e.clear_find(cx)),
            AddressBarEvent::RerunFind { .. } => Self::explorer(cx, |e, cx| e.rerun_find(cx).detach()),
        }
    }

    fn on_sidebar_event(
        &mut self,
        _: &Entity<Sidebar>,
        event: &SidebarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SidebarEvent::RunSavedSearch(saved) => {
                let saved = saved.clone();
                Self::explorer(cx, |e, cx| e.run_saved_search(saved, cx).detach())
            }
            SidebarEvent::AddConnection => dialogs::remote::open_add(window, cx),
            SidebarEvent::EditRemote(id) => dialogs::remote::open_edit(window, cx, id.clone()),
            SidebarEvent::RemoveRemote(id) => dialogs::remote::remove_remote(window, cx, id.clone()),
            SidebarEvent::SearchText(folder) => self.enter_text(Some(folder), window, cx),
        }
    }

    /// The sidebar's places and folders open the shared folder menu.
    fn on_sidebar_request(
        &mut self,
        _: &Entity<Sidebar>,
        request: &SidebarRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match request {
            SidebarRequest::ContextMenu { path, position } => self
                .menus
                .update(cx, |menus, cx| menus.open_folder_menu(path, *position, window, cx)),
            SidebarRequest::SendToDevice { id, paths } => {
                dialogs::nearby::send_to_device(window, cx, id.clone(), paths.clone())
            }
        }
    }

    fn on_list_event(&mut self, pane: usize, event: &FileListEvent, window: &mut Window, cx: &mut Context<Self>) {
        let tab = explorer::explorer(cx).read(cx).pane_tab_id(pane);
        match event {
            FileListEvent::OpenInFileExplorer { path } => {
                crate::platform::launch::hand_to_explorer(std::slice::from_ref(path))
            }
            FileListEvent::ShowProperties { path } => Self::show_properties(path.clone(), cx),
            FileListEvent::CommitRename { name, .. } => {
                Self::explorer(cx, |e, cx| e.commit_rename_in(tab, name, cx).detach())
            }
            FileListEvent::CancelRename => Self::explorer(cx, |e, cx| e.cancel_rename_in(tab, cx)),
            FileListEvent::ContextMenu { target, at, shift } => self.menus.update(cx, |menus, cx| {
                menus.open_list_menu(pane, target.as_deref(), *at, *shift, window, cx)
            }),
            FileListEvent::ConfirmOpenMany { paths } => Self::open_many(paths.clone(), window, cx),
            FileListEvent::SetColumns { columns } => Self::explorer(cx, |e, cx| e.set_columns(columns.clone(), cx)),
            FileListEvent::ResetFolderView => Self::explorer(cx, |e, cx| {
                let path = e.tab(tab).map(|t| t.path.clone()).unwrap_or_default();
                e.reset_folder_view(&path, cx)
            }),
            FileListEvent::SearchSubfolders { query } => {
                Self::explorer(cx, |e, cx| e.run_find(query, explorer::Wider::default(), cx).detach())
            }
        }
    }

    fn on_grid_event(&mut self, pane: usize, event: &FileGridEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            FileGridEvent::ContextMenu { path, position } => self.menus.update(cx, |menus, cx| {
                menus.open_list_menu(pane, Some(path), Some(*position), false, window, cx)
            }),
            FileGridEvent::BackgroundContextMenu { position } => self.menus.update(cx, |menus, cx| {
                menus.open_background_menu(pane, Some(*position), window, cx)
            }),
            FileGridEvent::DragStart { paths, from_archive } => {
                if *from_archive {
                    // They're extracted first, then dragged as copies.
                    Self::drag_from_archive(paths.clone(), Some(self.panes[pane].grid.clone()), cx);
                } else {
                    let drag = dragdrop::drag_out(cx, MAIN, paths.clone(), false);
                    let grid = self.panes[pane].grid.clone();
                    cx.spawn(async move |_, cx| {
                        let effect = drag.await;
                        if let Err(error) = &effect {
                            eprintln!("glint: couldn't drag the items: {error}");
                        }
                        cx.update(|cx| {
                            // The dragged tiles stop looking lifted; a move took them away, so the folder is read again.
                            grid.update(cx, |grid, cx| grid.end_drag(cx));
                            if effect.as_deref() == Ok("move") {
                                Self::explorer(cx, |e, cx| e.reload(None, cx).detach());
                            }
                        });
                    })
                    .detach();
                }
            }
            FileGridEvent::OpenMany { paths } => Self::open_many(paths.clone(), window, cx),
        }
    }

    fn on_this_pc_event(&mut self, event: &ThisPcEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ThisPcEvent::ContextMenu { path, position } => self
                .menus
                .update(cx, |menus, cx| menus.open_folder_menu(path, *position, window, cx)),
            ThisPcEvent::RecentChanges { path } => dialogs::timeline::open(window, cx, path.clone()),
            ThisPcEvent::FindDuplicates { path } => dialogs::duplicates::open(window, cx, path.clone()),
            // The drive's tile: This PC already opened the drive in the tab; its map comes up (`openDriveTreemap`).
            ThisPcEvent::DiskUsage { tab, .. } => {
                if !disk_usage::shows(cx, *tab) {
                    disk_usage::toggle(cx, *tab)
                }
            }
        }
    }

    fn on_usage_event(&mut self, pane: usize, event: &DiskUsageEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            // The map's items can sit deeper than the folder's own rows: the menu takes them as they are.
            DiskUsageEvent::ItemMenu { entries, position } => self.menus.update(cx, |menus, cx| {
                menus.open_entries_menu(pane, entries.clone(), *position, window, cx)
            }),
            DiskUsageEvent::FolderMenu { folder, position } => self
                .menus
                .update(cx, |menus, cx| menus.open_folder_menu(folder, *position, window, cx)),
            DiskUsageEvent::FreeUpSpace { folder } => dialogs::cleanup::open(window, cx, folder),
            DiskUsageEvent::EnableInstantSearch => notices::turn_on_instant_search(window, cx),
        }
    }

    /// What a menu asked for that the menus don't do themselves.
    fn on_menu_event(&mut self, _: &Entity<MenuHost>, event: &MenuEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            // PORT ME(preview pane): hide the native preview while a menu covers it (preview_host::hide/reveal).
            // A menu covers the preview pane: Windows' previewer (a window of its own) steps aside meanwhile.
            MenuEvent::Shown => preview::cover(cx),
            MenuEvent::Closed => preview::uncover(cx),
            MenuEvent::Duplicates { folder } => dialogs::duplicates::open(window, cx, folder.clone()),
            MenuEvent::Timeline { folder } => dialogs::timeline::open(window, cx, folder.clone()),
            MenuEvent::Cleanup { folder } => dialogs::cleanup::open(window, cx, folder),
            MenuEvent::XRay { path } => dialogs::xray::open(window, cx, path.clone()),
            MenuEvent::Defender { paths } => {
                dialogs::check::open(window, cx, paths.clone(), dialogs::check::What::Defender)
            }
            MenuEvent::Secrets { path } => {
                dialogs::check::open(window, cx, vec![path.clone()], dialogs::check::What::Secrets)
            }
            MenuEvent::Yara { path } => {
                dialogs::check::open(window, cx, vec![path.clone()], dialogs::check::What::Yara)
            }
            MenuEvent::Rule { folder } => dialogs::rules::open(window, cx, Some(folder.clone())),
            MenuEvent::CompressInPlace { paths } => dialogs::compress_in_place::open(window, cx, paths.clone()),
            MenuEvent::Restore { paths } => Self::explorer(cx, |e, cx| {
                let entries: Vec<_> = e
                    .active_tab()
                    .map(|tab| {
                        tab.rows
                            .iter()
                            .filter(|entry| paths.contains(&entry.path))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                e.restore_recycled(&entries, cx).detach()
            }),
            MenuEvent::EmptyRecycleBin => dialogs::confirm(
                window,
                cx,
                t!("Empty the Recycle Bin"),
                t!("Delete everything in the Recycle Bin for good? It can't be restored afterwards."),
                t!("Empty"),
                true,
                |_, cx| Self::explorer(cx, |e, cx| e.empty_recycle_bin(cx).detach()),
            ),
            MenuEvent::PasteInto { folder } => Self::explorer(cx, |e, cx| e.paste(Some(folder), cx).detach()),
            MenuEvent::PasteQueued { folder } => Self::explorer(cx, |e, cx| e.paste_queued(Some(folder), cx).detach()),
            MenuEvent::CopyToOtherPane { paths, moving } => Self::explorer(cx, |e, cx| {
                e.copy_to_other_pane(*moving, Some(paths.clone()), cx).detach()
            }),
            MenuEvent::NewTextDocument => Self::new_of_kind(".txt", cx),
            MenuEvent::NewOfKind { ext } => Self::new_of_kind(ext, cx),
            MenuEvent::PreviousVersions { path } => Self::previous_versions(path.clone(), window, cx),
            MenuEvent::History { folder } => dialogs::history::open(window, cx, folder),
            MenuEvent::Locks { paths } => {
                dialogs::locks::open(window, cx, paths.clone(), |exe, _, cx| Self::reveal(&exe, cx))
            }
            MenuEvent::EditTags { paths } => dialogs::tags::open(window, cx, paths.clone()),
            MenuEvent::SetTags { paths, add, remove } => {
                dialogs::tags::set_tags(window, cx, paths.clone(), add.clone(), remove.clone())
            }
            MenuEvent::Vault { paths, decrypt } => dialogs::vault::open(window, cx, paths.clone(), *decrypt),
            MenuEvent::EditConnection { id } => dialogs::remote::open_edit(window, cx, id.clone()),
            MenuEvent::RemoveConnection { id } => dialogs::remote::remove_remote(window, cx, id.clone()),
            MenuEvent::SearchText { folder } => self.enter_text(Some(folder), window, cx),
            MenuEvent::Compare { left, right } => dialogs::compare_files::open(window, cx, left.clone(), right.clone()),
            // Archives and disk images.
            MenuEvent::ExtractHere { path } => Self::explorer(cx, |e, cx| e.extract_here(path, cx).detach()),
            MenuEvent::Mount { path } => Self::explorer(cx, |e, cx| e.mount_image(path, None, cx).detach()),
            MenuEvent::OpenWithItsApp {
                path,
                extract_first: true,
            } => Self::open_archived(path, explorer::OpenHow::Default, cx),
            MenuEvent::OpenWithEditor {
                path,
                editor,
                extract_first: true,
            } => Self::open_archived(path, explorer::OpenHow::Editor(editor.clone()), cx),
            MenuEvent::Compress { paths, format } => {
                let format = match format {
                    menus::CompressFormat::Zip => explorer::CompressFormat::Zip,
                    menus::CompressFormat::SevenZ => explorer::CompressFormat::SevenZ,
                    menus::CompressFormat::TarZst => explorer::CompressFormat::TarZst,
                };
                Self::compress(paths.clone(), format, window, cx)
            }
            MenuEvent::CompressWithPassword { paths } => Self::compress_with_password(paths.clone(), window, cx),
            // Doing things to files.
            MenuEvent::MakeSmaller { paths } => {
                dialogs::shrink::open(window, cx, paths.clone(), |window, cx, folders| {
                    Self::compress(folders, explorer::CompressFormat::SevenZ, window, cx)
                })
            }
            MenuEvent::BackToJpeg { paths } => dialogs::shrink::back_to_jpeg(window, cx, paths.clone()),
            MenuEvent::ConvertPictures { paths } => dialogs::convert::open(window, cx, paths.clone()),
            MenuEvent::Media { paths, tool } => dialogs::media::open(window, cx, paths.clone(), *tool),
            MenuEvent::EditSongs { paths } => dialogs::song::open(window, cx, paths.clone()),
            MenuEvent::ExportPdf { paths, typst } => {
                dialogs::doc_convert::export_pdf(window, cx, paths.clone(), *typst)
            }
            MenuEvent::ConvertDocument { paths } => dialogs::doc_convert::open(window, cx, paths.clone(), None),
            MenuEvent::QueryTogether { paths } => dialogs::data_tools::query_together(window, cx, paths.clone()),
            MenuEvent::ConvertData { paths } => dialogs::data_tools::convert_data(window, cx, paths.clone()),
            MenuEvent::PdfTools { paths, tool } => dialogs::pdf::open(window, cx, paths.clone(), (*tool).into()),
            MenuEvent::RemovePersonalInfo { paths } => {
                dialogs::metadata::open(window, cx, paths.clone(), dialogs::metadata::Mode::Scrub)
            }
            MenuEvent::ChangeDateTaken { paths } => {
                dialogs::metadata::open(window, cx, paths.clone(), dialogs::metadata::Mode::Date)
            }
            MenuEvent::BackUp { folder } => dialogs::backup::open(window, cx, folder.clone()),
            // Sending and sharing.
            MenuEvent::SendToDevice { device, paths } => {
                dialogs::nearby::send_to_device(window, cx, device.clone(), paths.clone())
            }
            MenuEvent::SendNearby { paths } => {
                dialogs::nearby::open(window, cx, paths.clone());
            }
            MenuEvent::SendToPc { path } => dialogs::share::offer_to_pc(window, cx, path.clone()),
            MenuEvent::ShareOnNetwork { path } => dialogs::share::offer_on_network(window, cx, path.clone()),
            MenuEvent::ShareWithWindows { paths } => dialogs::share::share_with_windows(window, cx, paths.clone()),
            MenuEvent::ReceiveHere { folder } => dialogs::share::open_receive(window, cx, folder.clone()),
            other => {
                // The parts not built yet: named by the menu's own word for them until they are.
                let name = format!("{other:?}");
                let what = name.split([' ', '{', '(']).next().unwrap_or("That").to_string();
                Self::not_yet(window, cx, what.into());
            }
        }
    }

    fn on_queue_event(
        &mut self,
        _: &Entity<CopyQueue>,
        event: &CopyQueueEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // The explorer hears the queue's events itself: it offers Undo, words what didn't go, reads folders again.
            CopyQueueEvent::Finished(_) => {}
            CopyQueueEvent::NotQueued { reason, .. } => window.push_notification(
                Notification::error(reason.clone()).title(t!("Couldn't queue the copy")),
                cx,
            ),
        }
    }

    /// A kind of the New menu by its extension (".txt"), or a plain new file when Windows doesn't list it.
    fn new_of_kind(ext: &str, cx: &mut App) {
        Self::explorer(cx, |e, cx| {
            let wanted = if ext.starts_with('.') {
                ext.to_string()
            } else {
                format!(".{ext}")
            };
            match e
                .new_kinds(cx)
                .iter()
                .find(|kind| kind.ext.eq_ignore_ascii_case(&wanted))
                .cloned()
            {
                Some(kind) => e.new_of_kind(kind, cx).detach(),
                None => e.new_item(false, cx).detach(),
            }
        });
    }

    /// Previous versions of `path`, with what the dialog hands back done here.
    fn previous_versions(path: String, window: &mut Window, cx: &mut App) {
        dialogs::versions::open(window, cx, path, |request, window, cx| match request {
            dialogs::versions::Request::OpenFolderInNewTab(folder) => {
                Self::explorer(cx, |e, cx| e.open_in_new_tab(&folder, cx))
            }
            dialogs::versions::Request::Show { folder, select } => Self::explorer(cx, |e, cx| {
                let options = explorer::NavOptions {
                    select: Some(vec![select]),
                    ..Default::default()
                };
                e.navigate(folder, options, cx)
            }),
            // The dialog closed itself first; how it was goes on the left.
            dialogs::versions::Request::CompareFiles { then, now } => {
                dialogs::compare_files::open(window, cx, then, now)
            }
            dialogs::versions::Request::CompareFolders { .. } => Self::not_yet(window, cx, t!("Comparing folders")),
        });
    }

    /// Shows `path` in the active tab: its folder, with it selected (`revealInApp`).
    fn reveal(path: &str, cx: &mut App) {
        let folder = glint_model::paths::parent_path(path);
        let target = explorer::LaunchTarget {
            path: folder,
            select: Some(path.to_string()),
        };
        Self::explorer(cx, |e, cx| {
            e.open_requested(vec![target], cx);
        });
    }

    /// The active tab's selection, in the order the rows show it.
    fn selection(cx: &App) -> Vec<String> {
        let explorer = explorer::explorer(cx);
        let explorer = explorer.read(cx);
        let Some(tab) = explorer.active_tab() else {
            return Vec::new();
        };
        tab.rows
            .iter()
            .filter(|entry| tab.selection.contains(&entry.path))
            .map(|entry| entry.path.clone())
            .collect()
    }

    /// What to select once the selection is deleted, as Explorer does: the row after it, or else before it.
    fn after_deleting(cx: &App) -> Option<String> {
        let explorer = explorer::explorer(cx);
        let explorer = explorer.read(cx);
        let tab = explorer.active_tab()?;
        let rows: Vec<&glint_model::entries::FileEntry> = tab.rows.iter().collect();
        let last = rows.iter().rposition(|entry| tab.selection.contains(&entry.path))?;
        let first = rows.iter().position(|entry| tab.selection.contains(&entry.path))?;
        rows[last + 1..]
            .iter()
            .find(|entry| !tab.selection.contains(&entry.path))
            .or_else(|| {
                rows[..first]
                    .iter()
                    .rev()
                    .find(|entry| !tab.selection.contains(&entry.path))
            })
            .map(|entry| entry.path.clone())
    }

    /// Delete (or Shift+Delete): to the Recycle Bin, or for good; in the Recycle Bin itself, for good after asking.
    fn delete_selection(permanent: bool, cx: &mut App) {
        let paths = Self::selection(cx);
        if paths.is_empty() {
            return;
        }
        let next = Self::after_deleting(cx);
        Self::explorer(cx, |e, cx| {
            if e.active_tab().is_some_and(|tab| tab.is_recycle_bin()) {
                let entries: Vec<_> = e
                    .active_tab()
                    .map(|tab| {
                        tab.rows
                            .iter()
                            .filter(|entry| paths.contains(&entry.path))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                e.purge_recycled(&entries, next.as_deref(), cx);
            } else {
                e.delete_paths(paths, permanent, next, cx).detach();
            }
        });
    }

    /// Ctrl+Shift+C: the selection's paths as text, one per line (the folder shown when nothing is selected).
    fn copy_paths(window: &mut Window, cx: &mut App) {
        let mut paths = Self::selection(cx);
        if paths.is_empty() {
            paths.push(explorer::explorer(cx).read(cx).current_path().to_string());
        }
        let count = paths.len();
        cx.write_to_clipboard(ClipboardItem::new_string(paths.join("\r\n")));
        let message = if count == 1 {
            t!("Copied the path")
        } else {
            t!("Copied {n} paths", n = count)
        };
        window.push_notification(Notification::success(message), cx);
    }

    /// Settings, with what it hands back (connecting a cloud, folder rules) answered here.
    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = settings::open(window, cx);
        self.settings_events =
            Some(
                cx.subscribe_in(&view, window, |_, _, event: &SettingsEvent, window, cx| match event {
                    SettingsEvent::ConnectCloud => dialogs::cloud_setup::open(window, cx),
                    SettingsEvent::FolderRules => dialogs::rules::open(window, cx, None),
                }),
            );
    }

    /// "Properties" on an item: Windows' own dialog, which runs beside the window.
    fn show_properties(path: String, cx: &mut App) {
        let Some(main) = cx.global::<core_bridge::Core>().app.get_webview_window(MAIN) else {
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

    /// Opening more than ten files at once asks first, as the web app did.
    ///
    /// PORT ME(dialogs): the "Open N files?" question; until its dialog exists, they open.
    fn open_many(paths: Vec<String>, _window: &mut Window, cx: &mut Context<Self>) {
        Self::explorer(cx, |e, cx| {
            for path in &paths {
                e.open_file(path, None, cx).detach();
            }
        });
    }

    // ---- The window's parts -----------------------------------------------------------------------------------

    fn toolbar(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        row_start()
            .id("toolbar")
            .role(Role::Toolbar)
            .aria_label(t!("Navigation"))
            .test_support()
            .gap(space(1.))
            .px(space(2.))
            .py(space(1.5))
            .bg(theme::chrome_fill(cx))
            .border_b_1()
            .border_color(t.border)
            .child(self.mode_switch(t, cx))
            // Text mode has its own search bar below; the files' address bar steps aside.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(!self.text_mode, |area| area.child(self.address.clone())),
            )
            .children(self.sharing_button(cx))
            .when(!self.text_mode, |bar| bar.child(self.disk_usage_button(cx)))
            // TEMPORARY: This PC's tiles and banner open it too; this button goes with the Settings part.
            .child(
                Button::new("drive-health")
                    .ghost()
                    .small()
                    .icon(IconName::HeartPulse)
                    .accessibility_label(t!("Drive health"))
                    .tooltip(t!("Drive health"))
                    .on_click(|_, window, cx| dialogs::drive_health::open(window, cx, None)),
            )
    }

    /// What is shared or being sent right now, opening its dialog (the web app's `SharingButton`); nothing when
    /// nothing is.
    fn sharing_button(&self, cx: &App) -> Option<AnyElement> {
        let (label, view) = self.sharing.as_ref()?.read(cx).button()?;
        Some(
            Button::new("sharing")
                .ghost()
                .small()
                .icon(IconName::Share2)
                .label(label)
                .on_click(move |_, window, cx| {
                    dialogs::share::open_view(window, cx, view);
                })
                .into_any_element(),
        )
    }

    /// Disk usage, or back to the file list (the web app's command bar button).
    fn disk_usage_button(&self, cx: &App) -> AnyElement {
        let tab = explorer::explorer(cx).read(cx).active_tab_id();
        let mapped = disk_usage::shows(cx, tab);
        let label = if mapped {
            t!("Back to the file list")
        } else {
            t!("Disk usage")
        };
        Button::new("disk-usage")
            .ghost()
            .small()
            .icon(IconName::LayoutDashboard)
            .selected(mapped)
            .disabled(!Self::can_map(cx))
            .accessibility_label(label.clone())
            .tooltip(if mapped {
                label
            } else {
                t!("Disk usage: a map of what fills this folder, from the instant search index when it has it, otherwise measured with pdu")
            })
            .on_click(|_, _, cx| Self::toggle_disk_usage(cx))
            .into_any_element()
    }

    /// Pane `pane`: the view its tab asks for, and the zone a dragged tab can be dropped on.
    fn pane(&self, pane: usize, t: &theme::Tokens, cx: &App) -> AnyElement {
        let views = &self.panes[pane];
        let view: AnyElement = match Self::shown_in(pane, cx) {
            Some(Shown::ThisPc) => views.this_pc.clone().into_any_element(),
            Some(Shown::DiskUsage) => views.usage.clone().into_any_element(),
            Some(Shown::Grid) => views.grid.clone().into_any_element(),
            Some(Shown::List) | None => views.list.clone().into_any_element(),
        };
        div()
            .id(("pane", pane))
            .relative()
            .size_full()
            .min_w_0()
            .bg(t.background)
            .child(view)
            .child(PaneDropZone::new(pane))
            .into_any_element()
    }

    fn body(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = resizable_panel().size(SIDEBAR_START).size_range(SIDEBAR).fixed().child(
            div()
                .id("sidebar")
                .test_support()
                .size_full()
                .bg(theme::chrome_fill(cx))
                .child(self.sidebar.clone()),
        );
        let dual = explorer::explorer(cx).read(cx).is_dual();
        let files = if dual {
            // The first pane at the start edge, the second beside it, the divider between them draggable.
            let first = resizable_panel().child(self.pane(0, t, cx));
            let second = resizable_panel().child(self.pane(1, t, cx));
            let group = h_resizable("panes");
            let group = if crate::i18n::is_rtl() {
                group.child(second).child(first)
            } else {
                group.child(first).child(second)
            };
            div().size_full().child(group).into_any_element()
        } else {
            self.pane(0, t, cx)
        };
        let files = match &self.preview {
            None => files,
            Some(pane) => {
                // Where the web app put it: at the end edge, its width kept in the settings (PORT ME(coordinator):
                // saving a dragged width back to `preview_width`).
                let rem = f32::from(theme::rem_size(cx));
                let container = (f32::from(window_width(cx)) - f32::from(SIDEBAR_START)).max(0.);
                let width = preview::pane_width(state::settings(cx).preview_width, container, rem, false);
                let shown = resizable_panel().child(files);
                let side = resizable_panel()
                    .size(px(width))
                    .child(div().id("preview-pane").size_full().child(pane.clone()));
                let group = h_resizable("files-preview");
                let group = if crate::i18n::is_rtl() {
                    group.child(side).child(shown)
                } else {
                    group.child(shown).child(side)
                };
                div().size_full().child(group).into_any_element()
            }
        };
        let content = resizable_panel().child(
            div()
                .id("content")
                .role(Role::Main)
                .test_support()
                .size_full()
                .bg(t.background)
                .child(files),
        );
        // The sidebar sits at the start edge: left in English, right in Arabic.
        let group = h_resizable("body");
        if crate::i18n::is_rtl() {
            group.child(content).child(sidebar)
        } else {
            group.child(sidebar).child(content)
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        // Over Mica the window itself is the backdrop; otherwise it is the page color.
        let fill = if theme::backdrop_on(cx) {
            hsla(0., 0., 0., 0.)
        } else {
            t.background
        };
        // Pixels in a rem follow the text-size setting; rem-based sizes (the type scale, `space`) scale with it.
        let rem = theme::rem_size(cx);
        if window.rem_size() != rem {
            window.set_rem_size(rem);
        }
        let root = col()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .bg(fill)
            .text_color(t.foreground)
            .text_size(theme::size::SM)
            // Text starts at the reading direction's start edge (text alignment is inherited by everything below).
            .text_start()
            // Either mode.
            .on_action(cx.listener(|this, _: &actions::TextSearch, window, cx| {
                // In text mode it does nothing, as in the web app.
                if !this.text_mode {
                    this.enter_text(None, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &actions::BackToFiles, window, cx| this.leave_text(window, cx)))
            .on_action(
                cx.listener(|_, _: &actions::ShowDriveHealth, window, cx| {
                    dialogs::drive_health::open(window, cx, None)
                }),
            )
            .on_action(cx.listener(|_, _: &actions::ShowShortcuts, window, cx| dialogs::shortcuts::open(window, cx)))
            .on_action(cx.listener(|_, action: &actions::ShowHistory, window, cx| {
                dialogs::history::open(window, cx, &action.query)
            }))
            .on_action(cx.listener(|this, _: &actions::OpenSettings, window, cx| this.open_settings(window, cx)))
            .on_action(cx.listener(|_, _: &actions::CommandPalette, window, cx| palette::toggle(window, cx)));
        if self.text_mode {
            // Text search instead of the files: no tabs, no sidebar, no status bar (as the web app hid them); the
            // files' keys (Delete, Ctrl+C...) aren't heard, so nothing happens to files that aren't on screen.
            let text = self.text.clone();
            return root
                .child(TitleBar::new())
                .child(self.toolbar(&t, cx))
                .child(div().flex_1().min_h_0().children(text))
                .child(self.menus.clone());
        }
        root
            // Tabs and panes.
            .on_action(cx.listener(|_, _: &actions::NewTab, _, cx| Self::explorer(cx, |e, cx| e.new_tab(None, cx))))
            .on_action(cx.listener(|_, _: &actions::CloseTab, _, cx| Self::explorer(cx, |e, cx| e.close_tab(None, cx))))
            .on_action(
                cx.listener(|_, _: &actions::ReopenClosedTab, _, cx| {
                    Self::explorer(cx, |e, cx| e.reopen_closed_tab(cx))
                }),
            )
            .on_action(cx.listener(|_, _: &actions::NextTab, _, cx| Self::explorer(cx, |e, cx| e.cycle_tab(1, cx))))
            .on_action(
                cx.listener(|_, _: &actions::PreviousTab, _, cx| Self::explorer(cx, |e, cx| e.cycle_tab(-1, cx))),
            )
            .on_action(cx.listener(|_, action: &actions::GoToTab, _, cx| {
                let index = action.index;
                Self::explorer(cx, |e, cx| e.select_tab(index + 1, cx))
            }))
            .on_action(cx.listener(|_, _: &actions::DualPane, _, cx| Self::explorer(cx, |e, cx| e.toggle_dual(cx))))
            .on_action(cx.listener(|_, _: &actions::OtherPane, _, cx| Self::explorer(cx, |e, cx| e.switch_pane(cx))))
            .on_action(cx.listener(|_, _: &actions::SwapPanes, _, cx| Self::explorer(cx, |e, cx| e.swap_panes(cx))))
            // Moving around.
            .on_action(cx.listener(|_, _: &actions::Back, _, cx| Self::explorer(cx, |e, cx| e.go_back(cx).detach())))
            .on_action(
                cx.listener(|_, _: &actions::Forward, _, cx| Self::explorer(cx, |e, cx| e.go_forward(cx).detach())),
            )
            .on_action(cx.listener(|_, _: &actions::Up, _, cx| Self::explorer(cx, |e, cx| e.go_up(cx).detach())))
            .on_action(
                cx.listener(|this, _: &actions::Refresh, _, cx| this.address.update(cx, |bar, cx| bar.refresh(cx))),
            )
            .on_action(cx.listener(|this, _: &actions::FocusAddress, window, cx| {
                this.address.update(cx, |bar, cx| bar.focus(window, cx))
            }))
            .on_action(cx.listener(|this, _: &actions::SearchHere, window, cx| {
                this.address.update(cx, |bar, cx| bar.focus_search(window, cx))
            }))
            // The view.
            .on_action(cx.listener(|_, _: &actions::ListView, _, cx| Self::choose_view(ViewMode::List, cx)))
            .on_action(cx.listener(|_, _: &actions::GridView, _, cx| Self::choose_view(ViewMode::Grid, cx)))
            .on_action(cx.listener(|_, _: &actions::GridLarger, _, cx| Self::explorer(cx, |e, cx| e.zoom_grid(1, cx))))
            .on_action(
                cx.listener(|_, _: &actions::GridSmaller, _, cx| Self::explorer(cx, |e, cx| e.zoom_grid(-1, cx))),
            )
            .on_action(cx.listener(|_, _: &actions::GridReset, _, cx| Self::explorer(cx, |e, cx| e.zoom_grid(0, cx))))
            .on_action(cx.listener(|_, _: &actions::ToggleHidden, _, cx| {
                state::update(cx, |settings| settings.show_hidden = !settings.show_hidden)
            }))
            .on_action(cx.listener(|_, _: &actions::CompareFiles, window, cx| Self::compare_selected(window, cx)))
            .on_action(cx.listener(|_, _: &actions::DiskUsage, _, cx| Self::toggle_disk_usage(cx)))
            // File operations, on the active tab's selection.
            .on_action(cx.listener(|_, _: &actions::Copy, _, cx| {
                let paths = Self::selection(cx);
                Self::explorer(cx, |e, cx| e.copy_to_clipboard(paths, false, cx).detach())
            }))
            .on_action(cx.listener(|_, _: &actions::Cut, _, cx| {
                let paths = Self::selection(cx);
                Self::explorer(cx, |e, cx| e.copy_to_clipboard(paths, true, cx).detach())
            }))
            .on_action(
                cx.listener(|_, _: &actions::Paste, _, cx| Self::explorer(cx, |e, cx| e.paste(None, cx).detach())),
            )
            .on_action(cx.listener(|_, _: &actions::CopyPath, window, cx| Self::copy_paths(window, cx)))
            .on_action(cx.listener(|_, _: &actions::Delete, _, cx| Self::delete_selection(false, cx)))
            .on_action(cx.listener(|_, _: &actions::DeletePermanently, _, cx| Self::delete_selection(true, cx)))
            .on_action(
                cx.listener(|_, _: &actions::Rename, _, cx| Self::explorer(cx, |e, cx| e.begin_rename(None, cx))),
            )
            .on_action(cx.listener(|_, _: &actions::BatchRename, window, cx| {
                // Several items: the batch rename dialog; one: its rename box (begin_rename asks for the dialog itself
                // when several are selected and archives or the Recycle Bin say no).
                let paths = Self::selection(cx);
                if paths.len() > 1 {
                    dialogs::batch_rename::open(window, cx, paths)
                } else {
                    Self::explorer(cx, |e, cx| e.begin_rename(None, cx))
                }
            }))
            .on_action(cx.listener(|_, _: &actions::Undo, _, cx| Self::explorer(cx, |e, cx| e.undo(None, cx).detach())))
            .on_action(
                cx.listener(|_, _: &actions::NewFolder, _, cx| {
                    Self::explorer(cx, |e, cx| e.new_item(true, cx).detach())
                }),
            )
            .on_action(cx.listener(|_, _: &actions::Properties, _, cx| {
                let path = Self::selection(cx)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| explorer::explorer(cx).read(cx).current_path().to_string());
                Self::show_properties(path, cx)
            }))
            .on_action(cx.listener(|_, _: &actions::QuickLook, window, cx| preview::quick_look::toggle(window, cx)))
            .on_action(cx.listener(|_, _: &actions::TogglePreview, _, cx| preview::toggle_pane(cx)))
            .child(TitleBar::new().child(self.tabs.clone()))
            .child(self.toolbar(&t, cx))
            .child(div().flex_1().min_h_0().child(self.body(&t, cx)))
            .child(self.status.clone())
            // Over everything: the copy queue, then any open menu above it.
            .child(self.queue.clone())
            .child(self.menus.clone())
    }
}

/// The main window's width, for sizing the preview pane before layout (the web app sized it from the window too).
fn window_width(cx: &App) -> Pixels {
    crate::startup::main_window(cx)
        .and_then(|main| {
            main.update(&mut cx.to_async(), |_, window, _| window.viewport_size().width)
                .ok()
        })
        .unwrap_or(px(1240.))
}

/// Opens the main window, hidden: start-up puts it where it was last time and then shows it (or leaves it in
/// the tray), so it never flashes up somewhere else first. Mica material, minimum size, the title bar the kit
/// draws. Returns the window and its workspace.
pub fn open_main_window(cx: &mut App) -> gpui_kit::Result<(AnyWindowHandle, Entity<Workspace>)> {
    let bounds = Bounds::centered(None, size(px(1240.), px(780.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Glint".into()),
            ..TitleBar::title_bar_options()
        }),
        window_background: theme::window_background(cx),
        window_min_size: Some(size(px(520.), px(360.))),
        show: false,
        ..TitleBar::window_options()
    };
    gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Workspace::new(window, cx)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::{file, folder, FakeDisk};
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{Bounds, TestAppContext, VisualTestContext};

    fn open(cx: &mut TestAppContext, rtl: bool) -> VisualTestContext {
        let disk = FakeDisk::new();
        disk.folder(
            "C:\\Work",
            vec![
                folder("C:\\Work", "src"),
                file("C:\\Work", "notes.txt", 120, 0),
                file("C:\\Work", "todo.txt", 80, 0),
            ],
        );
        cx.update(|cx| {
            testing::init(cx);
            explorer::init_with(cx, disk.source());
        });
        // After init, which starts every test in English.
        testing::set_rtl(rtl);
        let (mut visual, _) = testing::open_window(cx, |window, cx| cx.new(|cx| Workspace::new(window, cx)));
        visual.update(|_, cx| {
            explorer::explorer(cx).update(cx, |e, cx| {
                e.init_explorer(
                    vec![explorer::LaunchTarget {
                        path: "C:\\Work".into(),
                        select: None,
                    }],
                    cx,
                )
                .detach()
            })
        });
        visual.run_until_parked();
        visual
    }

    /// The bounds of an element by its id (labels change with the language), from a freshly completed frame.
    fn bounds_of(cx: &mut VisualTestContext, id: &'static str) -> Bounds<Pixels> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.find(id).bounds()
        })
    }

    fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    #[gpui_kit::test]
    fn the_window_holds_the_views_and_the_drive_health_button_opens_its_dialog(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = open(cx, false);
        assert_eq!(role_of(&mut visual, "Navigation"), Some(Role::Toolbar));
        assert_eq!(role_of(&mut visual, "Places"), Some(Role::Navigation));
        assert_eq!(role_of(&mut visual, "Files"), Some(Role::ListBox));
        assert_eq!(role_of(&mut visual, "Status bar"), Some(Role::Status));
        for icon_only in [
            "Back (Alt+Left)",
            "Forward (Alt+Right)",
            "Up (Alt+Up)",
            "New tab",
            "Drive health",
        ] {
            assert_eq!(role_of(&mut visual, icon_only), Some(Role::Button), "{icon_only}");
        }
        // The folder opened at start is in the list.
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find_by_label("notes.txt").is_some(),
                "the list shows the folder's files"
            );
        });
        // English reads left to right: the sidebar before the files.
        assert!(bounds_of(&mut visual, "sidebar").origin.x < bounds_of(&mut visual, "content").origin.x);

        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("drive-health", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn the_window_actions_reach_the_explorer(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = open(cx, false);
        let tabs = |visual: &mut VisualTestContext| {
            visual.update(|_, cx| explorer::explorer(cx).read(cx).panes()[0].tabs.len())
        };
        assert_eq!(tabs(&mut visual), 1);
        visual.dispatch_action(actions::NewTab);
        visual.run_until_parked();
        assert_eq!(tabs(&mut visual), 2, "Ctrl+T opens a tab");
        visual.dispatch_action(actions::GridView);
        visual.run_until_parked();
        let view = visual.update(|_, cx| explorer::explorer(cx).read(cx).active_tab().map(|t| t.view));
        assert_eq!(
            view,
            Some(ViewMode::Grid),
            "the grid view action switches the tab's view"
        );
    }

    #[gpui_kit::test]
    fn disk_usage_shows_the_map_in_the_pane_and_the_list_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = open(cx, false);
        let shown = |visual: &mut VisualTestContext| visual.update(|_, cx| Workspace::shown_in(0, cx));
        assert_eq!(shown(&mut visual), Some(Shown::List));
        visual.dispatch_action(actions::DiskUsage);
        visual.run_until_parked();
        assert_eq!(shown(&mut visual), Some(Shown::DiskUsage), "the pane draws the map");
        // The list view action goes back to the list, as the web app's chooseView did.
        visual.dispatch_action(actions::ListView);
        visual.run_until_parked();
        assert_eq!(shown(&mut visual), Some(Shown::List));
        let tab = visual.update(|_, cx| explorer::explorer(cx).read(cx).active_tab_id());
        assert!(!visual.update(|_, cx| disk_usage::shows(cx, tab)));
    }

    #[gpui_kit::test]
    fn ctrl_m_pairs_the_two_files_selected(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = open(cx, false);
        let pair = |visual: &mut VisualTestContext| visual.update(|_, cx| Workspace::compare_pair(cx));
        assert_eq!(pair(&mut visual), None, "nothing selected");
        visual
            .update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.select_only(Some("C:\\Work\\notes.txt"), cx)));
        assert_eq!(pair(&mut visual), None, "one file and one pane");
        visual.update(|_, cx| {
            explorer::explorer(cx).update(cx, |e, cx| {
                if let Some(rows) = e.active_tab().map(|t| t.rows.clone()) {
                    e.select_all(&rows, cx);
                }
            })
        });
        // A folder and two files: three chosen, not a pair.
        assert_eq!(pair(&mut visual), None, "three items aren't a pair");
        visual.update(|_, cx| {
            explorer::explorer(cx).update(cx, |e, cx| {
                e.select_only(Some("C:\\Work\\notes.txt"), cx);
                e.toggle_selected("C:\\Work\\todo.txt", cx);
            })
        });
        assert_eq!(
            pair(&mut visual),
            Some(("C:\\Work\\notes.txt".to_string(), "C:\\Work\\todo.txt".to_string())),
            "the two files, in the rows' order"
        );
    }

    #[gpui_kit::test]
    fn arabic_mirrors_the_window_and_still_names_everything(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = open(cx, true);
        // The sidebar goes to the other side.
        assert!(bounds_of(&mut visual, "sidebar").origin.x > bounds_of(&mut visual, "content").origin.x);
        // Named in the interface's language.
        assert_eq!(role_of(&mut visual, &t!("Files")), Some(Role::ListBox));
        assert_eq!(role_of(&mut visual, &t!("Places")), Some(Role::Navigation));
        testing::set_rtl(false);
    }
}
