//! The main window: the title bar with the tab strip, the address bar, the resizable sidebar, one or two file
//! panes, the status bar. Wave 4's views fill it; this file places them and wires what they hand out (their
//! events, and the window's actions) to the explorer model and to the parts that own the rest.
//!
//! | Area                    | View                                                                 |
//! |-------------------------|----------------------------------------------------------------------|
//! | Title bar               | gpui-component's `TitleBar` (snap layouts), holding the [`TabStrip`] |
//! | Toolbar                 | the [`AddressBar`] (back, forward, up, refresh, crumbs, search)      |
//! | Drive health button     | **temporary** (This PC's tiles and banner open it too)               |
//! | Sidebar                 | the [`Sidebar`]: places and the folder tree                          |
//! | Each pane               | [`ThisPc`], or the [`FileGrid`] or the [`FileList`] by the tab's view |
//! | Status bar              | the [`StatusBar`]                                                    |
//!
//! **Patterns here:**
//! - The root has `key_context("Workspace")` and tracks focus, so the key bindings in [`crate::actions`] are
//!   live; handlers are `.on_action(cx.listener(...))` on the root. The views handle their own contexts
//!   (FileList, AddressBar...) and let the rest bubble up to here.
//! - Each view reads the explorer itself; the workspace only decides which view a pane shows and forwards
//!   what the views can't do alone. What belongs to a later part says so with [`Workspace::not_yet`].
//! - The window's background material (Mica) follows the theme: while it is on, chrome is see-through.

use crate::components::address_bar::{AddressBar, AddressBarEvent};
use crate::components::file_grid::{FileGrid, FileGridEvent};
use crate::components::file_list::{FileList, FileListEvent};
use crate::components::sidebar::{Sidebar, SidebarEvent};
use crate::components::status_bar::StatusBar;
use crate::components::tab_strip::{PaneDropZone, TabStrip};
use crate::components::this_pc::{ThisPc, ThisPcEvent};
use crate::explorer::{self, ExplorerEvent};
use crate::kit::prelude::*;
use crate::platform::dragdrop;
use crate::{actions, core_bridge, dialogs, state, t, theme};
use glint_core::host::Manager as _;
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
}

/// Which of a pane's views is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shown {
    ThisPc,
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
    /// The first pane and, in dual-pane view, the second.
    panes: [PaneViews; 2],
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
        let panes = [0, 1].map(|pane| PaneViews {
            list: cx.new(|cx| FileList::new(pane, window, cx)),
            grid: cx.new(|cx| FileGrid::new(pane, window, cx)),
            this_pc: cx.new(|cx| ThisPc::new(pane, window, cx)),
        });

        subscriptions.push(cx.subscribe_in(&address, window, Self::on_address_event));
        subscriptions.push(cx.subscribe_in(&sidebar, window, Self::on_sidebar_event));
        for views in &panes {
            subscriptions.push(cx.subscribe_in(&views.list, window, Self::on_list_event));
            subscriptions.push(cx.subscribe_in(&views.grid, window, Self::on_grid_event));
            subscriptions.push(cx.subscribe_in(&views.this_pc, window, Self::on_this_pc_event));
        }
        // The explorer decides what each pane shows; the keyboard follows the active pane when it moves.
        let explorer = explorer::explorer(cx);
        subscriptions.push(cx.observe(&explorer, |this, _, cx| this.follow_panes(cx)));
        subscriptions.push(cx.subscribe_in(&explorer, window, |this, _, event, window, cx| {
            if matches!(event, ExplorerEvent::PaneFocused) {
                this.focus_files(window, cx);
            }
        }));
        // The file view has the keyboard from the start, so the arrows work at once.
        cx.defer_in(window, |this, window, cx| this.focus_files(window, cx));

        Self {
            focus: cx.focus_handle(),
            tabs,
            address,
            sidebar,
            status,
            panes,
            shown: [None, None],
            _drop_target: Self::take_drops(window, cx),
            _subscriptions: subscriptions,
        }
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
        } else if tab.view == ViewMode::Grid {
            Shown::Grid
        } else {
            Shown::List
        })
    }

    /// The explorer changed: a grid that went out of sight lets its thumbnails go, and the window redraws.
    fn follow_panes(&mut self, cx: &mut Context<Self>) {
        for pane in 0..2 {
            let now = Self::shown_in(pane, cx);
            if self.shown[pane] == Some(Shown::Grid) && now != Some(Shown::Grid) {
                self.panes[pane].grid.update(cx, |grid, cx| grid.pause(cx));
            }
            self.shown[pane] = now;
        }
        cx.notify();
    }

    /// Gives the keyboard to the active pane's file view (the list, the grid, or This PC's first tile).
    fn focus_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = explorer::explorer(cx).read(cx).active_pane().min(1);
        match Self::shown_in(pane, cx) {
            Some(Shown::Grid) => {
                let handle = self.panes[pane].grid.read(cx).focus_handle().clone();
                window.focus(&handle, cx);
            }
            Some(Shown::List) => self.panes[pane].list.update(cx, |list, cx| list.focus(window, cx)),
            // This PC's tiles take the keyboard when clicked or tabbed to; the window keeps it meanwhile.
            Some(Shown::ThisPc) | None => window.focus(&self.focus, cx),
        }
    }

    /// Makes the window a drop target and hands what is dragged over it to [`Self::on_drop_event`].
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
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update_in(cx, |this, window, cx| this.on_drop_event(event, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Some(target)
    }

    /// What was dragged over the window. The answers (what a drop would do where the pointer is) stay
    /// Windows' default here, copy, until the views under the pointer give their own.
    ///
    /// PORT ME(drag and drop): the file list, the grid, the folder tree, the tab strip, the crumbs and This
    /// PC's tiles answer for the place under the pointer (`DropAnswers::set`) and act on the drop; each has a
    /// PORT ME at the spot. Until then a drop only says so.
    fn on_drop_event(&mut self, event: dragdrop::DropEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let dragdrop::DropEvent::Drop { paths, effect, .. } = event {
            if effect != dragdrop::DropEffect::None && !paths.is_empty() {
                Self::not_yet(window, cx, t!("Dropping files"));
            }
        }
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
            // The bar already filters the folder as you type; searching below it is the find part's.
            AddressBarEvent::FindAsTyped { .. } | AddressBarEvent::LeaveFind | AddressBarEvent::ClearFind => {}
            AddressBarEvent::RunFind { .. } | AddressBarEvent::RerunFind { .. } => {
                Self::not_yet(window, cx, t!("Searching subfolders"))
            }
        }
    }

    fn on_sidebar_event(
        &mut self,
        _: &Entity<Sidebar>,
        event: &SidebarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let what = match event {
            SidebarEvent::RunSavedSearch(_) => t!("Saved searches"),
            SidebarEvent::AddConnection | SidebarEvent::EditRemote(_) | SidebarEvent::RemoveRemote(_) => {
                t!("Remote connections")
            }
            SidebarEvent::SearchText(_) => t!("Text search"),
        };
        Self::not_yet(window, cx, what);
    }

    fn on_list_event(
        &mut self,
        _: &Entity<FileList>,
        event: &FileListEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            FileListEvent::OpenInFileExplorer { path } => crate::platform::launch::hand_to_explorer(std::slice::from_ref(path)),
            FileListEvent::ShowProperties { path } => Self::show_properties(path.clone(), cx),
            FileListEvent::CommitRename { .. } | FileListEvent::CancelRename => {
                Self::not_yet(window, cx, t!("Renaming"))
            }
            FileListEvent::ContextMenu { .. } => Self::not_yet(window, cx, t!("The item menu")),
            FileListEvent::ConfirmOpenMany { paths } => Self::open_many(paths.clone(), window, cx),
            FileListEvent::SetColumns { .. } | FileListEvent::ResetFolderView => {
                Self::not_yet(window, cx, t!("Choosing columns"))
            }
            FileListEvent::SearchSubfolders { .. } => Self::not_yet(window, cx, t!("Searching subfolders")),
        }
    }

    fn on_grid_event(
        &mut self,
        _: &Entity<FileGrid>,
        event: &FileGridEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            FileGridEvent::ContextMenu { .. } | FileGridEvent::BackgroundContextMenu { .. } => {
                Self::not_yet(window, cx, t!("The item menu"))
            }
            FileGridEvent::DragStart { paths, from_archive } => {
                if *from_archive {
                    // They're extracted first, then dragged as copies: the archive part does that.
                    Self::not_yet(window, cx, t!("Dragging out of archives"));
                } else {
                    let drag = dragdrop::drag_out(cx, MAIN, paths.clone(), false);
                    cx.spawn(async move |_, _| {
                        if let Err(error) = drag.await {
                            eprintln!("glint: couldn't drag the items: {error}");
                        }
                    })
                    .detach();
                }
            }
            FileGridEvent::OpenMany { paths } => Self::open_many(paths.clone(), window, cx),
        }
    }

    fn on_this_pc_event(
        &mut self,
        _: &Entity<ThisPc>,
        event: &ThisPcEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let what = match event {
            ThisPcEvent::ContextMenu { .. } => t!("The item menu"),
            ThisPcEvent::RecentChanges { .. } => t!("Recent changes"),
            ThisPcEvent::FindDuplicates { .. } => t!("Finding duplicates"),
            ThisPcEvent::DiskUsage { .. } => t!("The disk usage map"),
        };
        Self::not_yet(window, cx, what);
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
            .child(div().flex_1().min_w_0().child(self.address.clone()))
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

    /// Pane `pane`: the view its tab asks for, and the zone a dragged tab can be dropped on.
    fn pane(&self, pane: usize, t: &theme::Tokens, cx: &App) -> AnyElement {
        let views = &self.panes[pane];
        let view: AnyElement = match Self::shown_in(pane, cx) {
            Some(Shown::ThisPc) => views.this_pc.clone().into_any_element(),
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
        col()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .size_full()
            .bg(fill)
            .text_color(t.foreground)
            .text_size(theme::size::SM)
            // Text starts at the reading direction's start edge (text alignment is inherited by everything below).
            .text_start()
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
            .on_action(cx.listener(|_, _: &actions::ListView, _, cx| {
                Self::explorer(cx, |e, cx| e.set_view(ViewMode::List, None, cx))
            }))
            .on_action(cx.listener(|_, _: &actions::GridView, _, cx| {
                Self::explorer(cx, |e, cx| e.set_view(ViewMode::Grid, None, cx))
            }))
            .on_action(cx.listener(|_, _: &actions::GridLarger, _, cx| Self::explorer(cx, |e, cx| e.zoom_grid(1, cx))))
            .on_action(
                cx.listener(|_, _: &actions::GridSmaller, _, cx| Self::explorer(cx, |e, cx| e.zoom_grid(-1, cx))),
            )
            .on_action(cx.listener(|_, _: &actions::GridReset, _, cx| Self::explorer(cx, |e, cx| e.zoom_grid(0, cx))))
            .on_action(cx.listener(|_, _: &actions::ToggleHidden, _, cx| {
                state::update(cx, |settings| settings.show_hidden = !settings.show_hidden)
            }))
            // Windows of their own and parts still to come.
            .on_action(
                cx.listener(|_, _: &actions::ShowDriveHealth, window, cx| {
                    dialogs::drive_health::open(window, cx, None)
                }),
            )
            .on_action(cx.listener(|_, _: &actions::ShowShortcuts, window, cx| {
                Self::not_yet(window, cx, t!("Keyboard shortcuts"))
            }))
            .on_action(
                cx.listener(|_, _: &actions::OpenSettings, window, cx| Self::not_yet(window, cx, t!("Settings"))),
            )
            .on_action(cx.listener(|_, _: &actions::CommandPalette, window, cx| {
                Self::not_yet(window, cx, t!("The command palette"))
            }))
            .on_action(cx.listener(|_, _: &actions::Rename, window, cx| Self::not_yet(window, cx, t!("Renaming"))))
            .on_action(cx.listener(|_, _: &actions::Delete, window, cx| Self::not_yet(window, cx, t!("Deleting"))))
            .on_action(cx.listener(|_, _: &actions::QuickLook, window, cx| Self::not_yet(window, cx, t!("Quick Look"))))
            .on_action(cx.listener(|_, _: &actions::TogglePreview, window, cx| {
                Self::not_yet(window, cx, t!("The preview pane"))
            }))
            .child(TitleBar::new().child(self.tabs.clone()))
            .child(self.toolbar(&t, cx))
            .child(div().flex_1().min_h_0().child(self.body(&t, cx)))
            .child(self.status.clone())
    }
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
            vec![folder("C:\\Work", "src"), file("C:\\Work", "notes.txt", 120, 0)],
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
        for icon_only in ["Back (Alt+Left)", "Forward (Alt+Right)", "Up (Alt+Up)", "New tab", "Drive health"] {
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
