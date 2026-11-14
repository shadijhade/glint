//! The main window: the title bar with the tab strip, the address bar, the resizable sidebar, one or two file
//! panes, the status bar. Wave 4's views fill it; this file places them and wires what they hand out (their
//! events, and the window's actions) to the explorer model and to the parts that own the rest.
//!
//! The window is one bar, a rail, and a sheet with a dock at its foot:
//!
//! | Area                    | View                                                                 |
//! |-------------------------|----------------------------------------------------------------------|
//! | The bar (the title bar) | gpui-component's `TitleBar` (snap layouts), holding Glint's mark (the app's menu: Settings, the palette, the shortcut sheet, Drive health), the [`AddressBar`] (Back, Forward, Up, and one field: where you are, and the search of names and of what the files hold), Sharing, Update |
//! | The rail                | the open tabs ([`TabStrip`], a list), then the [`Sidebar`]: places and the folder tree |
//! | The sheet               | each pane: [`ThisPc`], the [`DiskUsage`] map, the [`FileGrid`], the [`FileList`] or the [`ColumnsView`]; [`FrequentFolders`] above Recent, [`GalleryMonths`] beside the Gallery; the preview pane at the end edge; the terminal under the panes |
//! | The dock                | the [`StatusBar`] at its two ends and the [`CommandBar`]'s island in its middle (New, Cut, Copy, Paste, views, tools) |
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
use crate::components::columns_view::ColumnsView;
use crate::components::command_bar::{CommandBar, CommandBarEvent};
use crate::components::compare_bar::CompareBar;
use crate::components::copy_queue::{CopyQueue, CopyQueueEvent};
use crate::components::disk_usage::{self, DiskUsage, DiskUsageEvent};
use crate::components::file_grid::{FileGrid, FileGridEvent};
use crate::components::file_list::DragFromArchive;
use crate::components::file_list::{FileList, FileListEvent};
use crate::components::frequent_folders::{self, FrequentFolders, FrequentFoldersEvent};
use crate::components::gallery_months::GalleryMonths;
use crate::components::git::{self, GitEvent, ReadmeCard};
use crate::components::menus::{self, MenuEvent, MenuHost};
use crate::components::pane_bars::PaneBars;
use crate::components::pane_head::PaneHead;
use crate::components::preview::{self, PreviewPane};
use crate::components::settings::SettingsEvent;
use crate::components::sidebar::{Sidebar, SidebarEvent, SidebarRequest};
use crate::components::status_bar::StatusBar;
use crate::components::tab_strip::{PaneDropZone, TabStrip};
use crate::components::terminal::{TerminalPane, TerminalPaneEvent};
use crate::components::this_pc::{ThisPc, ThisPcEvent};
use crate::components::{palette, settings};
use crate::explorer::{self, ExplorerEvent};
use crate::kit::prelude::*;
use crate::platform::{dragdrop, pickers, secondary};
use crate::{actions, core_bridge, dialogs, notices, state, t, theme};
use glint_core::host::Manager as _;
use glint_model::entries::FileEntry;
use glint_model::format::plural;
use glint_model::paths::{base_name, same_path};
use glint_model::store::ViewMode;
use gpui_kit::component::menu::DropdownMenu as _;
use std::ops::Range;

/// The label glint-core knows the main window by.
pub const MAIN: &str = "main";

/// The sidebar's width: what it starts at, and how far it can be dragged.
const SIDEBAR: Range<Pixels> = px(160.)..px(420.);
const SIDEBAR_START: Pixels = px(240.);
/// The bar's height: room for the address field with air above and below.
const TITLE_BAR_HEIGHT: Pixels = px(48.);
/// The dock's height, at the foot of the sheet: the status bar's, with the commands' island over its middle.
const DOCK_HEIGHT: Pixels = crate::components::status_bar::HEIGHT;
/// The room, in rems, the dock keeps at each side of the commands' island for the status bar's words.
const DOCK_SIDE_ROOM: f32 = 13.;
/// The air between the sheet and the window's end and bottom edges.
const SHEET_GAP: Pixels = px(8.);
/// How long the window's light takes to turn to another place's color.
const LIGHT_TURNS_IN: std::time::Duration = std::time::Duration::from_millis(700);

/// The light behind the window (see [`crate::art::aurora`]): the hue of the place shown, and while it turns, the
/// hue it is leaving.
#[derive(Clone, Copy, Debug, Default)]
struct Light {
    /// None until the first frame.
    hue: Option<f32>,
    from: Option<f32>,
    /// Counts the turns, so each one's fade is its own animation.
    turn: usize,
}

/// What a pane can show: one of each, kept for the window's life so each keeps its scroll and its state.
struct PaneViews {
    list: Entity<FileList>,
    grid: Entity<FileGrid>,
    this_pc: Entity<ThisPc>,
    /// The map of what fills the tab's folder, while the tab shows it (`disk_usage::visible`).
    usage: Entity<DiskUsage>,
    /// The folder above and the focused folder on each side of the list, in the Columns view.
    columns: Entity<ColumnsView>,
    /// The Gallery's months, at the end edge of its list or grid.
    months: Entity<GalleryMonths>,
    /// The folders used most, above Recent.
    frequent: Entity<FrequentFolders>,
    /// The compare bar above the pane while the panes are compared (and the bar for two files selected across them).
    compare: Entity<CompareBar>,
    /// The search bar and the closed-folder bar above the pane (above the compare bar).
    bars: Entity<PaneBars>,
    /// The folder's name, large, with what is in it and a chip for each kind, at the top of the pane.
    head: Entity<PaneHead>,
}

/// Which of a pane's views is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shown {
    ThisPc,
    DiskUsage,
    Grid,
    List,
    /// The list between the folder above and the focused folder.
    Columns,
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
    /// What is shared or being sent, for the toolbar's Sharing button (None without core, as in tests).
    sharing: Option<Entity<dialogs::share::Sharing>>,
    /// The command bar above the panes.
    command_bar: Entity<CommandBar>,
    /// The sidebar's width, as dragged (within [`SIDEBAR`]); it starts at [`SIDEBAR_START`].
    sidebar_width: Pixels,
    /// While the sidebar's edge is held: where the pointer was pressed and the width then.
    sidebar_drag: Option<(Pixels, Pixels)>,
    /// The light behind the window: the hue it has, and the one it is leaving.
    light: Light,
    /// The terminal pane under the panes (Ctrl+`), drawn only while it is open.
    terminal: Entity<TerminalPane>,
    /// Whether the bar was last told folder sizes are being measured.
    sizing: bool,
    /// The extra tools' state, for installing one a menu or a script asks for (made at first use).
    tools: Option<Entity<settings::tools::Tools>>,
    /// Each pane's README card above its files, with the README's path, while the folder has one.
    readmes: [Option<(String, Entity<ReadmeCard>)>; 2],
    /// Each pane's README as last looked for, with the listing it was looked for in: the explorer changes on every
    /// key press, its listings far less often.
    readmes_found: [Option<(std::sync::Arc<[FileEntry]>, Option<FileEntry>)>; 2],
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
        let panes = [0, 1].map(|pane| {
            let list = cx.new(|cx| FileList::new(pane, window, cx));
            let columns = cx.new(|cx| ColumnsView::new(pane, window, cx));
            // The Columns view draws the pane's own list between its two side columns.
            columns.update(cx, |columns, cx| columns.set_center(list.clone().into(), cx));
            PaneViews {
                list,
                grid: cx.new(|cx| FileGrid::new(pane, window, cx)),
                this_pc: cx.new(|cx| ThisPc::new(pane, window, cx)),
                usage: cx.new(|cx| DiskUsage::new(pane, window, cx)),
                columns,
                months: cx.new(|cx| GalleryMonths::new(pane, window, cx)),
                frequent: cx.new(|cx| FrequentFolders::new(pane, window, cx)),
                compare: cx.new(|cx| CompareBar::new(pane, window, cx)),
                bars: cx.new(|cx| PaneBars::new(pane, window, cx)),
                head: cx.new(|cx| PaneHead::new(pane, window, cx)),
            }
        });
        let command_bar = cx.new(|cx| CommandBar::new(window, cx));
        // In the dock the status bar's words are at the island's two sides: it leaves them room.
        command_bar.update(cx, |bar, cx| bar.set_side_room(DOCK_SIDE_ROOM, cx));
        subscriptions.push(cx.subscribe_in(&command_bar, window, Self::on_command_bar_event));
        // The terminal pane: the window lays out again as it opens, closes or gains a tab; closed, the keyboard goes
        // back to the files.
        let terminal = cx.new(|cx| TerminalPane::new(window, cx));
        subscriptions.push(cx.observe(&terminal, |_, _, cx| cx.notify()));
        subscriptions.push(
            cx.subscribe_in(
                &terminal,
                window,
                |this, _, event: &TerminalPaneEvent, window, cx| match event {
                    TerminalPaneEvent::Hidden => this.focus_files(window, cx),
                },
            ),
        );

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
            // A right-click on a frequent folder: the shared folder menu, as on This PC's tiles.
            subscriptions.push(cx.subscribe_in(
                &views.frequent,
                window,
                |this, _, event: &FrequentFoldersEvent, window, cx| {
                    match event {
                        FrequentFoldersEvent::ContextMenu { path, position } => this
                            .menus
                            .update(cx, |menus, cx| menus.open_folder_menu(path, *position, window, cx)),
                    }
                },
            ));
        }
        // Git read a tab's folder again: its marks go to the explorer's rows (the git: filter, Group by Git).
        let git = git::shared(cx);
        subscriptions.push(cx.subscribe(&git, |_, _, event: &GitEvent, cx| match event {
            GitEvent::Changed(tab) => {
                let marks = git::marks(cx, *tab);
                Self::explorer(cx, |e, cx| e.set_git_marks(*tab, marks, cx));
            }
        }));
        // The Update button follows the updater (none in tests, which start no updater).
        if let Some(updater) = crate::updater::try_shared(cx) {
            subscriptions.push(cx.observe(&updater, |_, _, cx| cx.notify()));
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
        // Back in front: Windows' "Animation effects" switch is read again (someone who turns it off in Settings
        // comes back to a Glint that has stopped moving).
        subscriptions.push(cx.observe_window_activation(window, |_, window, cx| {
            if window.is_window_active() {
                theme::motion::sync_with_windows(cx);
            }
            // The light behind the window dims while another window is in front.
            cx.notify();
        }));
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
                // The menus' other actions and exports say how they went; a few ask the window for something.
                ExplorerEvent::Action(notice) => notices::action(notice, window, cx),
                ExplorerEvent::Export(notice) => notices::export(notice, window, cx),
                ExplorerEvent::Request(request) => this.on_request(request.clone(), window, cx),
                // File Explorer's on-demand folders changed: the menus offer Show or Hide by them.
                ExplorerEvent::OnDemandChanged => {
                    let ids: Vec<String> = explorer::explorer(cx)
                        .read(cx)
                        .on_demand()
                        .iter()
                        .map(|m| m.id.clone())
                        .collect();
                    this.menus
                        .update(cx, |menus, _| menus.update_data(|data| data.on_demand = ids));
                }
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
            sharing,
            command_bar,
            sidebar_width: SIDEBAR_START,
            sidebar_drag: None,
            light: Light::default(),
            terminal,
            sizing: false,
            tools: None,
            readmes: [None, None],
            readmes_found: [None, None],
            shown: [None, None],
            _drop_target: Self::take_drops(window, cx),
            _subscriptions: subscriptions,
        }
    }

    /// The terminal pane (the self-test opens it and types into it).
    pub fn terminal(&self) -> Entity<TerminalPane> {
        self.terminal.clone()
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
        } else if tab.view == ViewMode::Columns {
            Shown::Columns
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
            // The Columns view draws the list too: going between the two keeps it.
            let lists = |shown: Option<Shown>| matches!(shown, Some(Shown::List) | Some(Shown::Columns));
            if lists(self.shown[pane]) && !lists(now) {
                self.panes[pane].list.update(cx, |list, cx| list.pause(cx));
            }
            self.shown[pane] = now;
        }
        self.sync_readmes(window, cx);
        // The command bar's Folder sizes says Stop while they are measured.
        let sizing = explorer::explorer(cx).read(cx).is_sizing();
        if sizing != self.sizing {
            self.sizing = sizing;
            self.command_bar
                .update(cx, |bar, cx| bar.set_folder_sizes_running(sizing, cx));
        }
        self.sync_preview(window, cx);
        cx.notify();
    }

    /// Each pane's README card, as the web app showed it above the files: the setting on, a folder on a drive
    /// read from disk (not from the index), no search, no map, and a README in it.
    fn sync_readmes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let show = state::settings(cx).show_readme;
        for pane in 0..2 {
            let listing = {
                let explorer = explorer::explorer(cx);
                let explorer = explorer.read(cx);
                explorer
                    .pane_tab(pane)
                    .filter(|tab| {
                        show && pane < explorer.shown_panes().len()
                            && tab.find.is_none()
                            && glint_model::paths::is_folder_path(&tab.path)
                            && !disk_usage::shows(cx, tab.id)
                    })
                    .and_then(|tab| tab.listing.as_ref())
                    .filter(|listing| !listing.from_index)
                    .map(|listing| listing.entries.clone())
            };
            let wanted: Option<FileEntry> = listing.and_then(|entries| match &self.readmes_found[pane] {
                Some((seen, found)) if std::sync::Arc::ptr_eq(seen, &entries) => found.clone(),
                _ => {
                    let found = git::readme_of(&entries).cloned();
                    self.readmes_found[pane] = Some((entries, found.clone()));
                    found
                }
            });
            match (wanted, &self.readmes[pane]) {
                (Some(entry), Some((path, card))) => {
                    if *path != entry.path {
                        let path = entry.path.clone();
                        card.update(cx, |card, cx| card.set_entry(entry, window, cx));
                        if let Some(kept) = self.readmes[pane].as_mut() {
                            kept.0 = path;
                        }
                    }
                }
                (Some(entry), None) => {
                    let path = entry.path.clone();
                    self.readmes[pane] = Some((path, cx.new(|cx| ReadmeCard::new(entry, window, cx))));
                }
                (None, Some(_)) => self.readmes[pane] = None,
                (None, None) => {}
            }
        }
    }

    /// Whether pane `pane`'s view `shown` has the keyboard (This PC's: the window keeps it, see `focus_files`).
    fn view_focused(&self, pane: usize, shown: Option<Shown>, window: &Window, cx: &App) -> bool {
        let views = &self.panes[pane];
        match shown {
            Some(Shown::List) | Some(Shown::Columns) => views.list.focus_handle(cx).contains_focused(window, cx),
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
    fn focus_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            Some(Shown::List) | Some(Shown::Columns) => {
                self.panes[pane].list.update(cx, |list, cx| list.focus(window, cx))
            }
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

    /// Runs `act` on the explorer model.
    fn explorer(cx: &mut App, act: impl FnOnce(&mut explorer::Explorer, &mut Context<explorer::Explorer>)) {
        explorer::explorer(cx).update(cx, act);
    }

    /// The mouse's side buttons (Mouse 4 Back, Mouse 5 Forward) walk the folder history of the pane under the
    /// pointer, or the active pane's when the pointer is elsewhere.
    fn side_button(pane: Option<usize>, back: bool, cx: &mut App) {
        Self::explorer(cx, |e, cx| {
            if let Some(pane) = pane {
                e.focus_pane(pane, cx);
            }
            if back {
                e.go_back(cx).detach()
            } else {
                e.go_forward(cx).detach()
            }
        });
    }

    // ---- Searching a folder -------------------------------------------------------------------------------------

    /// "Search here" on a folder (the folder menu, the sidebar's): the folder opens in this tab and the search
    /// box takes the keyboard. What is typed there is looked for in names and inside the files.
    fn search_in(&mut self, folder: &str, window: &mut Window, cx: &mut Context<Self>) {
        let folder = folder.to_string();
        let opening = explorer::explorer(cx).update(cx, |e, cx| {
            if same_path(e.current_path(), &folder) {
                gpui_kit::Task::ready(true)
            } else {
                e.navigate_task(folder, explorer::NavOptions::default(), cx)
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            if opening.await {
                this.update_in(cx, |this, window, cx| {
                    this.address.update(cx, |bar, cx| bar.focus_search(window, cx))
                })
                .ok();
            }
        })
        .detach();
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

    // ---- The command bar, requests, tools -----------------------------------------------------------------------

    fn on_command_bar_event(
        &mut self,
        _: &Entity<CommandBar>,
        event: &CommandBarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // Its menus choose what the item and folder menus choose: the same handler does it.
            CommandBarEvent::Menu(menu) => {
                let menus = self.menus.clone();
                self.on_menu_event(&menus, menu, window, cx)
            }
            CommandBarEvent::FolderSizes { stop } => {
                Self::explorer(cx, |e, cx| if *stop { e.stop_sizes() } else { e.calc_sizes(cx) })
            }
            // A project's task typed into the terminal pane, in the project's folder.
            CommandBarEvent::RunTask { folder, command } => self
                .terminal
                .update(cx, |terminal, cx| terminal.run_task(folder, command, window, cx)),
        }
    }

    /// What the explorer asks of the window: another window, the terminal, a tool installed.
    fn on_request(&mut self, request: explorer::Request, window: &mut Window, cx: &mut Context<Self>) {
        match request {
            // Another Glint.exe in secondary mode on the folder, centred (`open_window`); the tab strip asks for
            // one at a drop point itself. A failure says so in this window.
            explorer::Request::NewWindow { path } => secondary::open_window(window, cx, &path, None, |_, _| {}),
            // A script run in a terminal tab of its own.
            explorer::Request::Terminal(job) => self
                .terminal
                .update(cx, |terminal, cx| terminal.run_job(&job, window, cx)),
            explorer::Request::InstallTool { tool } => self.install_tool(tool, window, cx),
        }
    }

    /// Installs one of the extra tools (Settings' Tools does it, with its progress and its result notices).
    fn install_tool(&mut self, tool: String, window: &mut Window, cx: &mut Context<Self>) {
        let tools = match &self.tools {
            Some(tools) => tools.clone(),
            None => {
                let Some(core) = cx.try_global::<core_bridge::Core>() else {
                    return;
                };
                let backend: std::sync::Arc<dyn settings::Backend> =
                    std::sync::Arc::new(settings::CoreBackend::new(core.app.clone()));
                let tools = cx.new(|cx| settings::tools::Tools::new(backend, window, cx));
                self.tools = Some(tools.clone());
                tools
            }
        };
        tools.update(cx, |tools, cx| tools.install(tool, window, cx));
    }

    /// The ncdu export: where to save it (Windows' save picker), then the export, with its progress.
    fn export_ncdu(folder: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = explorer::Explorer::ncdu_file_name(&folder);
        let options = pickers::Options::new()
            .title(t!("Export"))
            .filter(pickers::Filter::new(t!("ncdu export"), &["ncdu", "json"]))
            .owner(pickers::owner_of(window));
        let chosen = pickers::save_file(options, &name);
        cx.spawn(async move |_, cx| {
            if let Ok(Some(dest)) = chosen.await {
                cx.update(|cx| Self::explorer(cx, |e, cx| e.export_ncdu(&folder, &dest, cx).detach()));
            }
        })
        .detach();
    }

    /// Whether `path` is inside an archive or on a remote connection (opening it extracts a copy first).
    fn needs_a_copy(path: &str, cx: &App) -> bool {
        path.starts_with(glint_model::paths::REMOTE)
            || explorer::explorer(cx)
                .read(cx)
                .active_tab()
                .is_some_and(|tab| tab.archive().is_some())
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
            SidebarEvent::SearchText(folder) => self.search_in(folder, window, cx),
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
        self.menu_command(event, window, cx)
    }

    /// Runs one of the menus' commands as if it were chosen from a menu: the menus, and the Ctrl+K palette through
    /// [`run_menu_command`].
    pub fn menu_command(&mut self, event: &MenuEvent, window: &mut Window, cx: &mut Context<Self>) {
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
            MenuEvent::SearchText { folder } => self.search_in(folder, window, cx),
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
            MenuEvent::SendNearby { paths } => {
                dialogs::nearby::open(window, cx, paths.clone());
            }
            MenuEvent::SendToPc { path } => dialogs::share::offer_to_pc(window, cx, path.clone()),
            MenuEvent::ShareOnNetwork { path } => dialogs::share::offer_on_network(window, cx, path.clone()),
            MenuEvent::ShareWithWindows { paths } => dialogs::share::share_with_windows(window, cx, paths.clone()),
            MenuEvent::ReceiveHere { folder } => dialogs::share::open_receive(window, cx, folder.clone()),
            // Opening, showing, Recent.
            MenuEvent::OpenWithItsApp {
                path,
                extract_first: false,
            } => Self::explorer(cx, |e, cx| {
                e.open_on_disk(path, explorer::OpenHow::Default, cx).detach()
            }),
            MenuEvent::OpenWithEditor {
                path,
                editor,
                extract_first: false,
            } => Self::explorer(cx, |e, cx| {
                e.open_on_disk(path, explorer::OpenHow::Editor(editor.clone()), cx)
                    .detach()
            }),
            MenuEvent::OpenWithPicker { path } => {
                if Self::needs_a_copy(path, cx) {
                    Self::open_archived(path, explorer::OpenHow::Picker, cx)
                } else {
                    Self::explorer(cx, |e, cx| e.open_on_disk(path, explorer::OpenHow::Picker, cx).detach())
                }
            }
            MenuEvent::RunInSandbox { path } => Self::explorer(cx, |e, cx| e.run_in_sandbox(path, cx).detach()),
            MenuEvent::OpenInNewWindow { path } => Self::explorer(cx, |e, cx| e.open_in_new_window(Some(path), cx)),
            MenuEvent::OpenFileLocation { path } => Self::explorer(cx, |e, cx| e.reveal_in_app(path, cx).detach()),
            MenuEvent::ForgetRecent { paths } => {
                Self::explorer(cx, |e, cx| e.forget_recent(paths.clone(), cx).detach())
            }
            MenuEvent::RevealInFileExplorer { paths } => {
                Self::explorer(cx, |e, cx| e.reveal_in_file_explorer(paths.clone(), cx).detach())
            }
            MenuEvent::RevealArchive { archive } => Self::explorer(cx, |e, cx| {
                e.reveal_in_file_explorer(vec![archive.clone()], cx).detach()
            }),
            // Folders and pictures.
            MenuEvent::FolderSizes => Self::explorer(cx, |e, cx| {
                if e.is_sizing() {
                    e.stop_sizes()
                } else {
                    e.calc_sizes(cx)
                }
            }),
            MenuEvent::BatchRenameFolder => Self::explorer(cx, |e, cx| e.batch_rename_folder(cx)),
            MenuEvent::SetWallpaper { path } => Self::explorer(cx, |e, cx| e.set_wallpaper(path, cx).detach()),
            MenuEvent::Rotate { paths, clockwise } => {
                Self::explorer(cx, |e, cx| e.rotate_pictures(paths.clone(), *clockwise, cx).detach())
            }
            MenuEvent::CloudPin { paths, state } => {
                let state = match state {
                    menus::CloudPin::Keep => explorer::PinState::Keep,
                    menus::CloudPin::Free => explorer::PinState::Free,
                    menus::CloudPin::Auto => explorer::PinState::Auto,
                };
                Self::explorer(cx, |e, cx| e.cloud_pin(paths.clone(), state, cx).detach())
            }
            MenuEvent::ExportNcdu { folder } => Self::export_ncdu(folder.clone(), window, cx),
            // Scripts and tools.
            MenuEvent::RunScript { script, cwd, paths } => {
                Self::explorer(cx, |e, cx| e.run_script(script, cwd, paths.clone(), cx).detach())
            }
            MenuEvent::InstallTool { tool } => self.install_tool(tool.clone(), window, cx),
            MenuEvent::OpenScriptsFolder => Self::explorer(cx, |e, cx| e.open_scripts_folder(cx).detach()),
            // File Explorer's on-demand folders.
            MenuEvent::ShowOnDemand { id, name } => Self::explorer(cx, |e, cx| e.show_on_demand(id, name, cx).detach()),
            MenuEvent::HideOnDemand { id } => Self::explorer(cx, |e, cx| e.hide_on_demand(id, cx).detach()),
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
            // This folder now beside how it was, compared in the two panes (`compareWithVersion`).
            dialogs::versions::Request::CompareFolders { now, then } => {
                Self::explorer(cx, |e, cx| e.compare_with_version(&now, &then, cx).detach())
            }
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
            // With nothing selected, the folder's own path (as the address bar's button says): not This PC's, nor a
            // search's.
            let explorer = explorer::explorer(cx).read(cx);
            let in_folder = explorer
                .active_tab()
                .is_some_and(|tab| tab.find.is_none() && glint_model::paths::is_folder_path(&tab.path));
            if !in_folder {
                return;
            }
            paths.push(explorer.current_path().to_string());
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
        self.open_settings_at(None, window, cx)
    }

    /// Settings scrolled to `section` ("about"; a secondary window asked the main one for it, which is how
    /// `open-settings` reached `openSettings(section)`). A secondary window has none of its own: it asks the
    /// main window.
    pub fn open_settings_at(&mut self, section: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        if settings::handed_to_main(window, cx, section) {
            return;
        }
        let view = settings::open_at(window, cx, section);
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

    /// Opening more than ten files at once asks first, as the web app did: "Open N files?" with Open N and Cancel.
    /// The lists emit this only above ten, so it always asks.
    fn open_many(paths: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let count = paths.len();
        dialogs::confirm(
            window,
            cx,
            t!("Open files"),
            t!("Open {count} files?", count = count),
            t!("Open {count}", count = count),
            false,
            move |_, cx| {
                Self::explorer(cx, |e, cx| {
                    for path in &paths {
                        e.open_file(path, None, cx).detach();
                    }
                });
            },
        );
    }

    // ---- The window's parts -----------------------------------------------------------------------------------

    /// The bar: the kit's title bar (the window's buttons, the snap layouts, the drag area), taller for the address
    /// field and with no fill or rule of its own: it is chrome, like the rail.
    fn title_bar() -> TitleBar {
        TitleBar::new()
            .h(TITLE_BAR_HEIGHT)
            .bg(gpui_kit::transparent_black())
            .border_b_0()
    }

    /// The color of the place the active pane shows: a special place's own, the nearest known folder's it is in
    /// (Documents, and everything under it), else the accent.
    fn place_tone(cx: &App) -> Hsla {
        use glint_model::paths::{is_recycle_bin, GALLERY, RECENT};
        let explorer = explorer::explorer(cx).read(cx);
        let accent = theme::tokens(cx).primary;
        let Some(tab) = explorer.active_tab() else {
            return accent;
        };
        let path = tab.path.as_str();
        if tab.is_this_pc() {
            return accent;
        }
        if is_recycle_bin(path) {
            return theme::place_tone(theme::Place::RecycleBin, cx);
        }
        if same_path(path, RECENT) {
            return theme::place_tone(theme::Place::Recent, cx);
        }
        if same_path(path, GALLERY) {
            return theme::place_tone(theme::Place::Gallery, cx);
        }
        // The deepest known folder the path is, or is in.
        let lower = path.to_lowercase();
        explorer
            .known_folders()
            .iter()
            .filter(|known| {
                let base = known.path.to_lowercase();
                let base = base.trim_end_matches('\\');
                lower == base || lower.strip_prefix(base).is_some_and(|rest| rest.starts_with('\\'))
            })
            .max_by_key(|known| known.path.len())
            .map_or(accent, |known| theme::place_tone(theme::Place::Known(known.id), cx))
    }

    /// The light behind the window: soft color at its corners, the place's own at the start corner, under
    /// everything. When the place changes the light turns to its color; while another window is in front it dims.
    fn light(&mut self, window: &Window, cx: &App) -> impl IntoElement {
        use gpui_kit::{Animation, AnimationExt as _};
        crate::art::set_scale(window.scale_factor());
        let (dark, rtl) = (theme::tokens(cx).dark, crate::i18n::is_rtl());
        let hue = Self::place_tone(cx).h;
        match self.light.hue {
            // A few degrees is the same light (pictures are shared within 3°).
            Some(now) if (now - hue).abs() < 0.008 => {}
            Some(now) => {
                self.light = Light {
                    hue: Some(hue),
                    from: Some(now),
                    turn: self.light.turn + 1,
                };
            }
            None => self.light.hue = Some(hue),
        }
        let picture = |hue: f32| {
            img(crate::art::aurora(hue, dark, rtl))
                .size_full()
                .object_fit(gpui_kit::ObjectFit::Fill)
        };
        let layer = || div().absolute().inset_0();
        let turn = self.light.turn;
        let lights = match self.light.from.filter(|_| !cx.reduce_motion()) {
            // Turning: the old light goes as the new one comes.
            Some(from) => layer()
                .child(layer().child(picture(from)).with_animation(
                    ("light-going", turn),
                    Animation::new(LIGHT_TURNS_IN).with_easing(gpui_kit::ease_in_out),
                    |old, delta| old.opacity(1. - delta),
                ))
                .child(layer().child(picture(hue)).with_animation(
                    ("light-coming", turn),
                    Animation::new(LIGHT_TURNS_IN).with_easing(gpui_kit::ease_in_out),
                    |new, delta| new.opacity(delta),
                )),
            None => layer().child(picture(hue)),
        };
        lights.opacity(if window.is_window_active() { 1. } else { 0.5 })
    }

    /// Glint's mark (the folder with the glint on its corner) at the start of the bar: the installer's logo, drawn
    /// small, made once. It is a button: the app's own menu opens under it (Settings, the palette, the shortcut
    /// sheet, Drive health, another window), so those have a place to be found that isn't a key.
    fn mark(&self, cx: &mut Context<Self>) -> impl IntoElement {
        thread_local! {
            static MARK: std::sync::Arc<Image> = {
                // The drawing is 1024 units square; asked for at 40 px it is sharp at 20 px on any screen.
                let svg = include_str!("../../glint/installer/logo.svg")
                    .replacen("width=\"1024\" height=\"1024\"", "width=\"40\" height=\"40\"", 1);
                std::sync::Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()))
            };
        }
        let this = cx.weak_entity();
        let name = t!("Glint: settings and more");
        let button =
            Button::new("glint-mark")
                .ghost()
                .small()
                .accessibility_label(name.clone())
                .tooltip(name)
                .child(img(MARK.with(Clone::clone)).size(px(20.)).flex_none())
                .dropdown_menu(move |menu, _, _| {
                    let settings = this.clone();
                    menu.item(PopupMenuItem::new(t!("Settings")).icon(IconName::Settings).on_click(
                        move |_, window, cx| {
                            settings
                                .update(cx, |workspace, cx| workspace.open_settings(window, cx))
                                .ok();
                        },
                    ))
                    .item(
                        PopupMenuItem::new(t!("Command palette"))
                            .icon(IconName::Command)
                            .on_click(|_, window, cx| palette::toggle(window, cx)),
                    )
                    .item(
                        PopupMenuItem::new(t!("Keyboard shortcuts"))
                            .icon(IconName::Keyboard)
                            .on_click(|_, window, cx| dialogs::shortcuts::open(window, cx)),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new(t!("Drive health"))
                            .icon(IconName::HeartPulse)
                            .on_click(|_, window, cx| dialogs::drive_health::open(window, cx, None)),
                    )
                    .item(
                        PopupMenuItem::new(t!("New window"))
                            .icon(IconName::AppWindow)
                            .on_click(|_, _, cx| Self::explorer(cx, |e, cx| e.open_in_new_window(None, cx))),
                    )
                });
        // Pressing it never drags the window.
        div().id("glint-mark-place").occlude().flex_none().me(1.).child(button)
    }

    /// The end of the bar, before the window's own buttons: what is being shared, and Update while a newer Glint
    /// is ready. Empty most of the time.
    fn bar_end(&self, cx: &mut Context<Self>) -> impl IntoElement {
        row_start()
            .id("bar-end")
            // Pressing these never drags the window.
            .occlude()
            .flex_none()
            .gap(space(0.5))
            .px(space(1.))
            .children(self.sharing_button(cx))
            // 'Update' while a newer Glint is ready (a spinner while it downloads).
            .children(crate::updater::update_button(cx))
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
                .tooltip(t!("What you're sending or sharing: its ticket or link, and Stop"))
                .on_click(move |_, window, cx| {
                    dialogs::share::open_view(window, cx, view);
                })
                .into_any_element(),
        )
    }

    /// Pane `pane`: the view its tab asks for, and the zone a dragged tab can be dropped on.
    fn pane(&self, pane: usize, cx: &App) -> AnyElement {
        let views = &self.panes[pane];
        let view: AnyElement = match Self::shown_in(pane, cx) {
            Some(Shown::ThisPc) => views.this_pc.clone().into_any_element(),
            Some(Shown::DiskUsage) => views.usage.clone().into_any_element(),
            Some(Shown::Columns) => views.columns.clone().into_any_element(),
            Some(Shown::Grid) => views.grid.clone().into_any_element(),
            Some(Shown::List) | None => views.list.clone().into_any_element(),
        };
        let (recent, gallery) = {
            let explorer = explorer::explorer(cx);
            let explorer = explorer.read(cx);
            let tab = explorer.pane_tab(pane);
            (
                tab.is_some_and(frequent_folders::shown_for),
                tab.is_some_and(|tab| glint_model::paths::is_gallery(&tab.path)),
            )
        };
        let shows_rows = matches!(Self::shown_in(pane, cx), Some(Shown::List) | Some(Shown::Grid) | None);
        // The Gallery's months at the end edge (they draw nothing when narrow or with one month).
        let view = if gallery && shows_rows {
            row_start()
                .size_full()
                .child(div().flex_1().min_w_0().h_full().child(view))
                .child(div().h_full().flex_none().child(views.months.clone()))
                .into_any_element()
        } else {
            view
        };
        // The README above the folder's files (it draws nothing for an empty or binary one).
        let view = match (&self.readmes[pane], shows_rows) {
            (Some((_, card)), true) => col()
                .size_full()
                .child(div().flex_none().child(card.clone()))
                .child(div().flex_1().min_h_0().child(view))
                .into_any_element(),
            _ => view,
        };
        // The folders used most above Recent's list (nothing drawn when there are none).
        let view = if recent && shows_rows {
            col()
                .size_full()
                .child(div().flex_none().child(views.frequent.clone()))
                .child(div().flex_1().min_h_0().child(view))
                .into_any_element()
        } else {
            view
        };
        // The search and closed-folder bars, then the compare bar, on top of everything else in the pane (each draws
        // nothing when it has nothing to say).
        // Over them all, the head: the folder's name and what is in it (nothing for This PC, the map, or a search).
        let headed = shows_rows || matches!(Self::shown_in(pane, cx), Some(Shown::Columns));
        let view = col()
            .size_full()
            .when(headed, |view| view.child(div().flex_none().child(views.head.clone())))
            .child(div().flex_none().child(views.bars.clone()))
            .child(div().flex_none().child(views.compare.clone()))
            .child(div().flex_1().min_h_0().child(view));
        div()
            .id(("pane", pane))
            .relative()
            .size_full()
            .min_w_0()
            .on_mouse_up(MouseButton::Navigate(NavigationDirection::Back), move |_, _, cx| {
                cx.stop_propagation();
                Self::side_button(Some(pane), true, cx);
            })
            .on_mouse_up(MouseButton::Navigate(NavigationDirection::Forward), move |_, _, cx| {
                cx.stop_propagation();
                Self::side_button(Some(pane), false, cx);
            })
            .child(view)
            .child(PaneDropZone::new(pane))
            .into_any_element()
    }

    fn body(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let rtl = crate::i18n::is_rtl();
        let dual = explorer::explorer(cx).read(cx).is_dual();
        let files = if dual {
            // The first pane at the start edge, the second beside it, the divider between them draggable.
            let first = resizable_panel().child(self.pane(0, cx));
            let second = resizable_panel().child(self.pane(1, cx));
            let group = h_resizable("panes");
            let group = if rtl {
                group.child(second).child(first)
            } else {
                group.child(first).child(second)
            };
            div().size_full().child(group).into_any_element()
        } else {
            self.pane(0, cx)
        };
        // The terminal pane sits under the panes, in their column (it draws nothing while closed).
        let files = col()
            .size_full()
            .child(div().flex_1().min_h_0().child(files))
            .child(div().flex_none().child(self.terminal.clone()))
            .into_any_element();
        let files = match &self.preview {
            None => files,
            Some(pane) => {
                // Where the web app put it: at the end edge, its width kept in the settings: a drag of its handle
                // saves the new width back to `preview_width`.
                let rem = f32::from(theme::rem_size(cx));
                let container = (f32::from(window_width(cx)) - f32::from(self.sidebar_width)).max(0.);
                let width = preview::pane_width(state::settings(cx).preview_width, container, rem, false);
                let shown = resizable_panel().child(files);
                let side = resizable_panel()
                    .size(px(width))
                    .child(div().id("preview-pane").size_full().child(pane.clone()));
                // The preview is the last panel from the start, or the first when the window is mirrored.
                let side_index = if rtl { 0 } else { 1 };
                let group = h_resizable("files-preview").on_resize(move |sizes, _, cx| {
                    if let Some(width) = sizes.read(cx).sizes().get(side_index) {
                        let saved = dragged_preview_width(f32::from(*width));
                        state::update(cx, |settings| settings.preview_width = Some(saved));
                    }
                });
                let group = if rtl {
                    group.child(side).child(shown)
                } else {
                    group.child(shown).child(side)
                };
                div().size_full().child(group).into_any_element()
            }
        };
        // The rail on the chrome, at the start edge (left in English, right in Arabic): the open tabs, then the
        // places under them.
        let sidebar = col()
            .id("sidebar")
            .test_support()
            .flex_none()
            .h_full()
            .w(self.sidebar_width)
            .pb(SHEET_GAP)
            .child(div().flex_none().px(space(2.)).child(self.tabs.clone()))
            .child(div().flex_1().min_h_0().child(self.sidebar.clone()));
        // The dock at the foot of the sheet: the status bar's words at its two ends, and the commands' island
        // over its middle. Each end is told the room it has beside the island (as last drawn), so it says what
        // fits there and nothing of it runs under the island.
        let rem = f32::from(theme::rem_size(cx));
        let (dock_width, island) = self.command_bar.read(cx).widths(rem);
        let beside = if dock_width > 0. && island > 0. {
            ((dock_width - island) / 2. - rem * 1.5).max(1.)
        } else {
            0.
        };
        self.status.read(cx).beside().set(beside);
        let dock = div()
            .id("dock")
            .test_support()
            .relative()
            .flex_none()
            .w_full()
            .h(DOCK_HEIGHT)
            .child(self.status.clone())
            .child(div().absolute().inset_0().child(self.command_bar.clone()));
        // The sheet: the files and the dock on their own surface, a step forward of the chrome: a card with a
        // hairline, rounded all round, clear of the window's end and bottom edges.
        let sheet = div()
            .id("content")
            .role(Role::Main)
            .test_support()
            .size_full()
            // Glass: the window's light shows through it, faintly; a lit edge at its top.
            .bg(t.sheet)
            .border_1()
            .border_color(t.stroke)
            .rounded(theme::size::RADIUS_SHEET)
            .overflow_hidden()
            .child(
                col()
                    .size_full()
                    .child(div().flex_1().min_h_0().child(files))
                    .child(dock),
            );
        let sheet = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .pb(SHEET_GAP)
            .map(|around| {
                if rtl {
                    around.pl(SHEET_GAP)
                } else {
                    around.pr(SHEET_GAP)
                }
            })
            .child(sheet);
        // The sidebar's edge: a strip over the seam that drags its width (a double-click puts it back).
        let grip = px(8.);
        let at = self.sidebar_width - grip / 2.;
        let handle = div()
            .id("sidebar-resize")
            .absolute()
            .top_0()
            .bottom_0()
            .w(grip)
            .map(|handle| if rtl { handle.right(at) } else { handle.left(at) })
            .cursor_col_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    if event.click_count >= 2 {
                        this.sidebar_width = SIDEBAR_START;
                        this.sidebar_drag = None;
                    } else {
                        this.sidebar_drag = Some((event.position.x, this.sidebar_width));
                    }
                    cx.notify();
                }),
            );
        row_start()
            .relative()
            .size_full()
            .child(sidebar)
            .child(sheet)
            .child(handle)
            // While the edge is held, a layer over the window takes the pointer: the drag follows it wherever it
            // goes and ends when the button is let go.
            .when(self.sidebar_drag.is_some(), |body| {
                body.child(
                    div()
                        .id("sidebar-resizing")
                        .absolute()
                        .inset_0()
                        .cursor_col_resize()
                        .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                            if let Some((from, width)) = this.sidebar_drag {
                                let moved = event.position.x - from;
                                let wanted = if rtl { width - moved } else { width + moved };
                                let next = wanted.clamp(SIDEBAR.start, SIDEBAR.end);
                                if next != this.sidebar_width {
                                    this.sidebar_width = next;
                                    cx.notify();
                                }
                            }
                        }))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                this.sidebar_drag = None;
                                cx.notify();
                            }),
                        )
                        .on_mouse_up_out(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                this.sidebar_drag = None;
                                cx.notify();
                            }),
                        ),
                )
            })
    }
}

/// Runs one of the menus' commands in the main window, as if it were chosen from a menu (the Ctrl+K palette offers
/// them). Does nothing before the main window exists.
pub fn run_menu_command(event: MenuEvent, window: &mut Window, cx: &mut App) {
    if let Some(main) = crate::startup::main_workspace(cx) {
        main.update(cx, |workspace, cx| workspace.menu_command(&event, window, cx));
    }
}

/// Types a project's task into the main window's terminal pane, in the project's folder (the Ctrl+K palette's
/// "Run <task>", as the command bar's Run menu does). Does nothing before the main window exists.
pub fn run_task(folder: String, command: String, window: &mut Window, cx: &mut App) {
    if let Some(main) = crate::startup::main_workspace(cx) {
        main.update(cx, |workspace, cx| {
            workspace
                .terminal
                .update(cx, |terminal, cx| terminal.run_task(&folder, &command, window, cx))
        });
    }
}

/// The notice that an archived item has no Properties (the web app's toast id "archive-properties").
struct ArchivePropertiesToast;

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _span = crate::perf::span("workspace");
        let t = theme::tokens(cx).clone();
        // The window is chrome all over (over Mica, the material with a veil); the sheet lies on it.
        let fill = theme::window_fill(cx);
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
            // Under everything: the light, in the color of the place shown.
            .child(self.light(window, cx))
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
            // Ctrl+N: another window on the folder shown.
            .on_action(cx.listener(|_, _: &actions::NewWindow, _, cx| {
                Self::explorer(cx, |e, cx| e.open_in_new_window(None, cx))
            }))
            .on_action(cx.listener(|_, _: &actions::CommandPalette, window, cx| palette::toggle(window, cx)));
        root
            // The mouse's side buttons anywhere outside a pane (the panes answer for themselves).
            .on_mouse_up(MouseButton::Navigate(NavigationDirection::Back), |_, _, cx| {
                Self::side_button(None, true, cx)
            })
            .on_mouse_up(MouseButton::Navigate(NavigationDirection::Forward), |_, _, cx| {
                Self::side_button(None, false, cx)
            })
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
            .on_action(cx.listener(|_, _: &actions::ColumnsView, _, cx| Self::choose_view(ViewMode::Columns, cx)))
            .on_action(cx.listener(|_, _: &actions::GridLarger, _, cx| Self::explorer(cx, |e, cx| e.zoom_grid(1, cx))))
            .on_action(
                cx.listener(|_, _: &actions::GridSmaller, _, cx| Self::explorer(cx, |e, cx| e.zoom_grid(-1, cx))),
            )
            .on_action(cx.listener(|_, _: &actions::GridReset, _, cx| Self::explorer(cx, |e, cx| e.zoom_grid(0, cx))))
            .on_action(cx.listener(|_, _: &actions::ToggleHidden, _, cx| {
                state::update(cx, |settings| settings.show_hidden = !settings.show_hidden)
            }))
            .on_action(cx.listener(|_, _: &actions::CompareFiles, window, cx| Self::compare_selected(window, cx)))
            .on_action(
                cx.listener(|_, _: &actions::ComparePanes, _, cx| {
                    Self::explorer(cx, |e, cx| e.toggle_compare_panes(cx))
                }),
            )
            .on_action(cx.listener(|this, _: &actions::ToggleTerminal, window, cx| {
                this.terminal.update(cx, |terminal, cx| terminal.toggle(window, cx))
            }))
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
            .on_action(cx.listener(|_, _: &actions::Properties, window, cx| {
                // Properties are File Explorer's, for files on disk: an archived item's details are in the preview
                // pane, and the Recycle Bin says what its items can do.
                let (archived, in_bin) = {
                    let explorer = explorer::explorer(cx).read(cx);
                    (
                        explorer.archive_of(None).is_some(),
                        explorer.active_tab().is_some_and(|tab| tab.is_recycle_bin()),
                    )
                };
                if archived {
                    window.push_notification(
                        Notification::info(t!(
                            "For something inside an archive, the preview pane (Alt+P) shows its details."
                        ))
                        .title(t!("Properties are for files on disk"))
                        .id::<ArchivePropertiesToast>(),
                        cx,
                    );
                } else if in_bin {
                    Self::explorer(cx, |e, cx| e.recycled_notice(cx));
                } else {
                    let path = Self::selection(cx)
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| explorer::explorer(cx).read(cx).current_path().to_string());
                    Self::show_properties(path, cx)
                }
            }))
            .on_action(cx.listener(|_, _: &actions::QuickLook, window, cx| preview::quick_look::toggle(window, cx)))
            .on_action(cx.listener(|_, _: &actions::TogglePreview, _, cx| preview::toggle_pane(cx)))
            // The bar: the mark at the start edge (before the address in English, after it in Arabic), the
            // address, then what the window has to say at the end.
            // They are laid over the title bar's drag area, not in it: the kit's area is as wide as what is in it
            // asks, and a long path asked for more than the window, pushing the window's own buttons out of it.
            .child({
                let over = div().absolute().inset_0().flex().flex_row().items_center();
                Self::title_bar().child(if crate::i18n::is_rtl() {
                    over.child(self.bar_end(cx))
                        .child(self.address.clone())
                        .child(self.mark(cx))
                } else {
                    over.child(self.mark(cx))
                        .child(self.address.clone())
                        .child(self.bar_end(cx))
                })
            })
            .child(div().flex_1().min_h_0().child(self.body(&t, cx)))
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

/// What a dragged preview pane's width is saved as: the handle never leaves it narrower than the pane's least
/// (`Math.max(256, width)`).
fn dragged_preview_width(width: f32) -> f64 {
    f64::from(width.max(preview::MIN_WIDTH))
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
    use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{Bounds, TestAppContext, VisualTestContext};

    fn open(cx: &mut TestAppContext, rtl: bool) -> VisualTestContext {
        open_workspace(cx, rtl).0
    }

    /// A folder name long enough that two of them, one in the other, don't fit the address field.
    const DEEP: &str = "a folder with a name that goes on for quite a while, 2026 edition";

    fn open_workspace(cx: &mut TestAppContext, rtl: bool) -> (VisualTestContext, Entity<Workspace>) {
        open_workspace_on(cx, rtl, |_| ())
    }

    /// [`open_workspace`], with the disk prepared further before the explorer reads it.
    fn open_workspace_on(
        cx: &mut TestAppContext,
        rtl: bool,
        prepare: impl FnOnce(&FakeDisk),
    ) -> (VisualTestContext, Entity<Workspace>) {
        let disk = FakeDisk::new();
        prepare(&disk);
        disk.folder(
            "C:\\Work",
            vec![
                folder("C:\\Work", "src"),
                file("C:\\Work", "notes.txt", 120, 0),
                file("C:\\Work", "todo.txt", 80, 0),
            ],
        );
        disk.folder(
            "C:\\Work\\src",
            vec![folder("C:\\Work\\src", DEEP), file("C:\\Work\\src", "main.rs", 10, 0)],
        );
        // Folders with long names, one in the other: a path longer than the bar.
        let deep = format!("C:\\Work\\src\\{DEEP}");
        disk.folder(&deep, vec![folder(&deep, DEEP)]);
        disk.folder(&format!("{deep}\\{DEEP}"), Vec::new());
        disk.archive_folder(
            "C:\\Work\\pack.zip",
            "C:\\Work\\pack.zip",
            vec![file("C:\\Work\\pack.zip", "inner.txt", 5, 0)],
        );
        cx.update(|cx| {
            testing::init(cx);
            explorer::init_with(cx, disk.source());
        });
        // After init, which starts every test in English.
        testing::set_rtl(rtl);
        let (mut visual, workspace) = testing::open_window(cx, |window, cx| cx.new(|cx| Workspace::new(window, cx)));
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
        (visual, workspace)
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
    fn the_window_holds_the_views_and_the_marks_menu_opens_drive_health(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = open(cx, false);
        assert_eq!(role_of(&mut visual, "Address bar"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Commands"), Some(Role::Toolbar));
        assert_eq!(role_of(&mut visual, "Places"), Some(Role::Navigation));
        assert_eq!(role_of(&mut visual, "Files"), Some(Role::ListBox));
        assert_eq!(role_of(&mut visual, "Status bar"), Some(Role::Status));
        for icon_only in [
            "Back (Alt+Left)",
            "Forward (Alt+Right)",
            "Up (Alt+Up)",
            "New tab",
            "Glint: settings and more",
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

        // The rail holds the tabs over the places, and the dock is the foot of the sheet, under the files.
        let (tabs, places) = (bounds_of(&mut visual, "tablist-0"), bounds_of(&mut visual, "places"));
        assert!(tabs.origin.y + tabs.size.height <= places.origin.y + px(1.));
        let (dock, sheet) = (bounds_of(&mut visual, "dock"), bounds_of(&mut visual, "content"));
        assert!(dock.origin.y > sheet.origin.y && dock.origin.x >= sheet.origin.x);
        let island = bounds_of(&mut visual, "command-bar");
        assert!(island.origin.y >= dock.origin.y && island.origin.x > dock.origin.x);

        // The mark opens the app's menu; Drive health is in it.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("glint-mark", cx);
        });
        visual.run_until_parked();
        for item in [
            "Settings",
            "Command palette",
            "Keyboard shortcuts",
            "Drive health",
            "New window",
        ] {
            assert_eq!(
                role_of(&mut visual, item),
                Some(Role::MenuItem),
                "{item} is in the mark's menu"
            );
        }
        let item = visual.update(|window, cx| {
            window.render_frame(cx);
            window.find_by_label("Drive health").bounds().center()
        });
        visual.simulate_click(item, gpui_kit::Modifiers::default());
        visual.run_until_parked();
        visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn a_long_path_gives_way_to_the_window_and_never_pushes_its_buttons_out(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        for rtl in [false, true] {
            let (mut visual, _) = open_workspace(cx, rtl);
            let deep = format!("C:\\Work\\src\\{DEEP}\\{DEEP}");
            visual.update(|_, cx| {
                explorer::explorer(cx).update(cx, |e, cx| e.navigate(deep, explorer::NavOptions::default(), cx))
            });
            visual.simulate_resize(size(px(900.), px(640.)));
            for _ in 0..6 {
                visual.update(|window, cx| window.render_frame(cx));
                visual.run_until_parked();
            }
            let width = visual.update(|window, _| window.viewport_size().width);
            let (bar, mark) = (
                bounds_of(&mut visual, "address-bar"),
                bounds_of(&mut visual, "glint-mark"),
            );
            // The bar and the mark are inside the window, with room left for the window's own three buttons.
            assert!(
                bar.origin.x >= px(0.) && bar.right() <= width - px(100.),
                "rtl {rtl}: {bar:?} in {width:?}"
            );
            assert!(
                mark.origin.x >= px(0.) && mark.right() <= width,
                "rtl {rtl}: {mark:?} in {width:?}"
            );
            // And the field still has room to show where you are.
            let field = bounds_of(&mut visual, "address-omni");
            assert!(field.size.width > px(300.), "rtl {rtl}: {field:?}");
            testing::set_rtl(false);
        }
    }

    #[gpui_kit::test]
    fn the_windows_light_is_the_color_of_the_place_shown_and_turns_when_the_place_changes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        // C:\Work is this PC's Documents.
        let (mut visual, workspace) = open_workspace_on(cx, false, |disk| {
            disk.set_known(vec![explorer::KnownFolder {
                id: explorer::KnownId::Documents,
                name: "Documents",
                path: "C:\\Work".into(),
            }]);
        });
        let go = |visual: &mut VisualTestContext, path: &str| {
            let path = path.to_string();
            visual.update(|_, cx| {
                explorer::explorer(cx).update(cx, |e, cx| e.navigate(path, explorer::NavOptions::default(), cx))
            });
            for _ in 0..3 {
                visual.update(|window, cx| window.render_frame(cx));
                visual.run_until_parked();
            }
        };
        let (documents, accent) = visual.update(|_, cx| {
            (
                theme::place_tone(theme::Place::Known(explorer::KnownId::Documents), cx),
                theme::tokens(cx).primary,
            )
        });
        go(&mut visual, "C:\\Work");
        assert_eq!(visual.update(|_, cx| Workspace::place_tone(cx)), documents);
        let light = workspace.read_with(&visual, |workspace, _| workspace.light);
        assert_eq!(light.hue, Some(documents.h));
        let turns = light.turn;
        // A folder in Documents is lit as Documents is.
        go(&mut visual, "C:\\Work\\src");
        assert_eq!(visual.update(|_, cx| Workspace::place_tone(cx)), documents);
        assert_eq!(workspace.read_with(&visual, |workspace, _| workspace.light.turn), turns);
        // This PC has the accent: the light turns to it from Documents' blue.
        go(&mut visual, glint_model::paths::THIS_PC);
        assert_eq!(visual.update(|_, cx| Workspace::place_tone(cx)), accent);
        let light = workspace.read_with(&visual, |workspace, _| workspace.light);
        assert_eq!(
            (light.hue, light.from, light.turn),
            (Some(accent.h), Some(documents.h), turns + 1)
        );
    }

    #[gpui_kit::test]
    fn the_sheet_is_headed_by_the_folder_and_not_by_this_pc(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, _) = open_workspace(cx, false);
        let head = bounds_of(&mut visual, "pane-head-title");
        let (sheet, dock) = (bounds_of(&mut visual, "content"), bounds_of(&mut visual, "dock"));
        // The name is at the top of the sheet, over the files, clear of the dock.
        assert!(
            head.top() >= sheet.top() && head.bottom() < dock.top(),
            "{head:?} in {sheet:?}"
        );
        assert!(head.top() - sheet.top() < px(40.), "{head:?} in {sheet:?}");
        visual.update(|_, cx| {
            explorer::explorer(cx).update(cx, |e, cx| {
                e.navigate(
                    glint_model::paths::THIS_PC.to_string(),
                    explorer::NavOptions::default(),
                    cx,
                )
            })
        });
        visual.run_until_parked();
        let found = visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("pane-head-title").is_some()
        });
        assert!(!found, "This PC has its own headings");
    }

    #[test]
    fn a_dragged_preview_width_is_saved_but_never_under_the_least() {
        assert_eq!(dragged_preview_width(480.), 480.);
        assert_eq!(dragged_preview_width(100.), f64::from(preview::MIN_WIDTH));
    }

    #[gpui_kit::test]
    fn opening_many_files_asks_first_and_opens_them_on_yes(cx: &mut TestAppContext) {
        use crate::explorer::Notice;
        let _guard = testing::lock();
        let (mut visual, workspace) = open_workspace(cx, false);
        // No app is set up to open anything, so each file opened shows up as a notice naming it.
        let opened = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let log = opened.clone();
        let _watch = visual.update(|_, cx| {
            cx.subscribe(&explorer::explorer(cx), move |_, event: &ExplorerEvent, _| {
                if let ExplorerEvent::Notice(Notice::CouldNotOpenFile { path, .. }) = event {
                    log.borrow_mut().push(path.clone());
                }
            })
        });
        let paths: Vec<String> = (0..12).map(|i| format!("C:\\Work\\f{i}.txt")).collect();
        visual.update(|window, cx| workspace.update(cx, |_, cx| Workspace::open_many(paths.clone(), window, cx)));
        visual.run_until_parked();
        // Asked, nothing opened yet.
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.has_active_dialog(cx));
            assert!(window.try_find_by_label("Open 12").is_some());
            assert!(window.try_find_by_label("Open files").is_some());
        });
        assert!(opened.borrow().is_empty());
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("confirm-ok", cx);
        });
        visual.run_until_parked();
        assert_eq!(*opened.borrow(), paths);
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

    /// The keys bound to `action` where the keyboard is in the window, written as GPUI parses them ("ctrl-l").
    fn keys_for(visual: &mut VisualTestContext, workspace: &Entity<Workspace>, action: &dyn Action) -> Vec<String> {
        visual.update(|window, cx| {
            let focus = workspace.read(cx).focus.clone();
            window
                .bindings_for_action_in(action, &focus)
                .iter()
                .map(|binding| {
                    binding
                        .keystrokes()
                        .iter()
                        .map(|keystroke| keystroke.unparse(gpui_kit::OperatingSystem::Windows))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect()
        })
    }

    /// Puts the keyboard in the window, as a click on it would.
    fn focus_window(visual: &mut VisualTestContext, workspace: &Entity<Workspace>) {
        visual.update(|window, cx| {
            window.render_frame(cx);
            workspace.update(cx, |workspace, cx| window.focus(&workspace.focus, cx));
            window.render_frame(cx);
        });
    }

    #[gpui_kit::test]
    fn explorer_tsx_keys_beyond_the_shortcut_sheet_are_bound_in_the_window(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, workspace) = open_workspace(cx, false);
        focus_window(&mut visual, &workspace);
        let has = |keys: &[String], wanted: &[&str]| wanted.iter().all(|key| keys.iter().any(|k| k == key));
        // Ctrl+E and F3 find, Alt+D and F4 type a path, Ctrl+R refreshes: next to Ctrl+F, Ctrl+L and F5.
        assert!(has(
            &keys_for(&mut visual, &workspace, &actions::SearchHere),
            &["ctrl-f", "ctrl-e", "f3"]
        ));
        assert!(has(
            &keys_for(&mut visual, &workspace, &actions::FocusAddress),
            &["ctrl-l", "alt-d", "f4"]
        ));
        assert!(has(
            &keys_for(&mut visual, &workspace, &actions::Refresh),
            &["f5", "ctrl-r"]
        ));
        // Ctrl+Shift+9 for columns; the grid zooms on Ctrl++ and Ctrl+_ as well as Ctrl+= and Ctrl+-.
        assert!(has(
            &keys_for(&mut visual, &workspace, &actions::ColumnsView),
            &["ctrl-shift-9"]
        ));
        assert!(has(
            &keys_for(&mut visual, &workspace, &actions::GridLarger),
            &["ctrl-=", "ctrl-shift-=", "ctrl-+"]
        ));
        assert!(has(
            &keys_for(&mut visual, &workspace, &actions::GridSmaller),
            &["ctrl--", "ctrl-shift--"]
        ));
    }

    #[gpui_kit::test]
    fn the_keys_do_what_they_are_bound_to(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, workspace) = open_workspace(cx, false);
        focus_window(&mut visual, &workspace);
        // Ctrl+Shift+9: the columns view.
        visual.update(|window, cx| window.press("ctrl-shift-9", cx));
        visual.run_until_parked();
        let view = visual.update(|_, cx| explorer::explorer(cx).read(cx).active_tab().map(|t| t.view));
        assert_eq!(view, Some(ViewMode::Columns));
        // F4 opens the address for typing.
        let editing = |visual: &mut VisualTestContext, workspace: &Entity<Workspace>| {
            visual.update(|_, cx| workspace.read(cx).address.read(cx).is_editing())
        };
        assert!(!editing(&mut visual, &workspace));
        visual.update(|window, cx| window.press("f4", cx));
        visual.run_until_parked();
        assert!(editing(&mut visual, &workspace), "F4 types a path");
    }

    #[gpui_kit::test]
    fn the_mouses_side_buttons_walk_the_history(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, _workspace) = open_workspace(cx, false);
        let path = |visual: &mut VisualTestContext| {
            visual.update(|_, cx| explorer::explorer(cx).read(cx).current_path().to_string())
        };
        visual.update(|_, cx| {
            explorer::explorer(cx).update(cx, |e, cx| e.navigate("C:\\Work\\src", Default::default(), cx))
        });
        settle(&mut visual);
        assert_eq!(path(&mut visual), "C:\\Work\\src");
        let over_the_files = bounds_of(&mut visual, "content").center();
        // Mouse 4 goes back, Mouse 5 forward.
        visual.update(|window, cx| {
            window.pointer_up(over_the_files, MouseButton::Navigate(NavigationDirection::Back), cx)
        });
        settle(&mut visual);
        assert_eq!(path(&mut visual), "C:\\Work");
        visual.update(|window, cx| {
            window.pointer_up(over_the_files, MouseButton::Navigate(NavigationDirection::Forward), cx)
        });
        settle(&mut visual);
        assert_eq!(path(&mut visual), "C:\\Work\\src");
    }

    #[gpui_kit::test]
    fn properties_say_nothing_on_disk_and_point_at_the_preview_pane_inside_an_archive(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, _workspace) = open_workspace(cx, false);
        let said = |visual: &mut VisualTestContext| visual.update(|window, cx| !window.notifications(cx).is_empty());
        // On disk Windows' own dialog opens; nothing is said here.
        visual.dispatch_action(actions::Properties);
        settle(&mut visual);
        assert!(!said(&mut visual));
        // Inside an archive: File Explorer has none, the preview pane has the details.
        visual.update(|_, cx| {
            explorer::explorer(cx).update(cx, |e, cx| e.navigate("C:\\Work\\pack.zip", Default::default(), cx))
        });
        settle(&mut visual);
        visual.dispatch_action(actions::Properties);
        settle(&mut visual);
        assert!(said(&mut visual), "a notice points at the preview pane");
    }

    #[gpui_kit::test]
    fn copy_path_with_nothing_selected_copies_the_folders_own_path(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, _workspace) = open_workspace(cx, false);
        visual.dispatch_action(actions::CopyPath);
        settle(&mut visual);
        let copied = visual.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some("C:\\Work"));
        // With items selected, theirs.
        visual
            .update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.select_only(Some("C:\\Work\\notes.txt"), cx)));
        visual.dispatch_action(actions::CopyPath);
        settle(&mut visual);
        let copied = visual.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some("C:\\Work\\notes.txt"));
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

    #[gpui_kit::test]
    fn the_sidebars_edge_drags_its_width_within_its_limits_and_the_sheet_takes_the_rest(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = open(cx, false);
        let sidebar = bounds_of(&mut visual, "sidebar");
        assert_eq!(sidebar.size.width, SIDEBAR_START);
        // The sheet starts where the sidebar ends: nothing between them.
        assert_eq!(bounds_of(&mut visual, "content").left(), sidebar.right());
        let edge = point(sidebar.right(), sidebar.center().y);
        visual.update(|window, cx| {
            window.pointer_down(edge, MouseButton::Left, cx);
            window.pointer_move(point(edge.x + px(60.), edge.y), Some(MouseButton::Left), cx);
        });
        let dragged = bounds_of(&mut visual, "sidebar");
        assert_eq!(dragged.size.width, SIDEBAR_START + px(60.));
        assert_eq!(bounds_of(&mut visual, "content").left(), dragged.right());
        // Never past its limits, however far the pointer goes (to the far side of the files here); letting go ends
        // the drag.
        let far = point(bounds_of(&mut visual, "content").right() - px(10.), edge.y);
        assert!(
            far.x - edge.x > SIDEBAR.end - SIDEBAR_START,
            "the test window is too narrow to tell"
        );
        visual.update(|window, cx| {
            window.pointer_move(far, Some(MouseButton::Left), cx);
            window.pointer_up(far, MouseButton::Left, cx);
            window.pointer_move(point(px(20.), edge.y), None, cx);
        });
        assert_eq!(bounds_of(&mut visual, "sidebar").size.width, SIDEBAR.end);
    }

    #[gpui_kit::test]
    fn in_arabic_the_sidebars_edge_is_its_left_one(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = open(cx, true);
        let sidebar = bounds_of(&mut visual, "sidebar");
        // The sheet ends where the sidebar starts.
        assert_eq!(bounds_of(&mut visual, "content").right(), sidebar.left());
        let edge = point(sidebar.left(), sidebar.center().y);
        visual.update(|window, cx| {
            window.pointer_down(edge, MouseButton::Left, cx);
            window.pointer_move(point(edge.x - px(40.), edge.y), Some(MouseButton::Left), cx);
            window.pointer_up(point(edge.x - px(40.), edge.y), MouseButton::Left, cx);
        });
        assert_eq!(bounds_of(&mut visual, "sidebar").size.width, SIDEBAR_START + px(40.));
        testing::set_rtl(false);
    }

    /// The windows the app's spawner was asked for, parsed.
    fn record_windows(visual: &mut VisualTestContext) -> std::sync::Arc<std::sync::Mutex<Vec<secondary::Spawn>>> {
        let spawned = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = spawned.clone();
        visual.update(|_, cx| {
            secondary::use_spawner(
                cx,
                std::sync::Arc::new(move |spawn: &secondary::Spawn| {
                    seen.lock().unwrap_or_else(|e| e.into_inner()).push(spawn.clone());
                    Ok(())
                }),
            )
        });
        spawned
    }

    fn settle(visual: &mut VisualTestContext) {
        for _ in 0..40 {
            visual.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[gpui_kit::test]
    fn new_window_starts_another_glint_on_the_folder_shown_or_the_one_asked_for(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = open(cx, false);
        let spawned = record_windows(&mut visual);
        // Ctrl+N: the folder shown.
        visual.dispatch_action(actions::NewWindow);
        settle(&mut visual);
        // "Open in new window" on a folder, from the menus and the palette.
        visual
            .update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.open_in_new_window(Some("C:\\Work\\src"), cx)));
        settle(&mut visual);
        let windows: Vec<_> = spawned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|spawn| (spawn.exe.clone(), secondary::parse(spawn.args.clone())))
            .collect();
        assert_eq!(windows.len(), 2);
        for (exe, _) in &windows {
            assert_eq!(*exe, std::env::current_exe().expect("this program"));
        }
        let folders: Vec<_> = windows
            .iter()
            .filter_map(|(_, parsed)| parsed.clone().ok().flatten())
            .map(|w| w.folder)
            .collect();
        assert_eq!(folders, ["C:\\Work", "C:\\Work\\src"]);
        // The key is Ctrl+N, as Explorer.tsx handled it.
        assert!(actions::BINDINGS
            .iter()
            .any(|b| b.keys == "ctrl-n" && b.context == "Workspace" && (b.action)().name() == "glint::NewWindow"));
    }

    #[gpui_kit::test]
    fn settings_in_a_secondary_window_go_to_the_main_window(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, workspace) = open_workspace(cx, false);
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = asked.clone();
        visual.update(|_, cx| {
            secondary::use_main_ask(
                cx,
                std::sync::Arc::new(move |section: Option<String>| {
                    seen.lock().unwrap_or_else(|e| e.into_inner()).push(section);
                    true
                }),
            )
        });
        let _secondary = secondary::as_secondary(secondary::window_on("C:\\Work"));
        // Ctrl+, in the secondary window: asked of the main one, and no Settings dialog here.
        visual.dispatch_action(actions::OpenSettings);
        settle(&mut visual);
        assert_eq!(asked.lock().unwrap_or_else(|e| e.into_inner()).as_slice(), [None]);
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        // The main window opens a section it was asked for.
        drop(_secondary);
        visual.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.open_settings_at(Some("about"), window, cx)
            });
        });
        settle(&mut visual);
        visual.update(|window, cx| assert!(window.has_active_dialog(cx), "Settings opened here"));
        assert_eq!(
            asked.lock().unwrap_or_else(|e| e.into_inner()).len(),
            1,
            "and nothing was asked"
        );
    }
}
