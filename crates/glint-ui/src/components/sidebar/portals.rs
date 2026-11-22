//! The Portals section: the live portals mounted under `%USERPROFILE%\Glint portals`, as rows between the saved
//! searches and This PC. Not in the TypeScript (portals came with 0.15).
//!
//! A row is a folder that happens to be an archive, a search, a Git commit or a backup snapshot: a click opens it
//! like any place. Its menu (right-click, the menu key or Shift+F10) has Open, Copy path, Refresh and Unmount.
//! The section is there only while something is mounted, as Searches is. The list is kept up to date by core's
//! change notice ([`glint_core::portals::on_change`]): mounting from a menu, from Time Machine or from the
//! palette, or unmounting from anywhere, changes the rows without anyone looking on a timer.
//!
//! Unmounting asks nothing, as removing a saved search or a pinned folder asks nothing: it takes the folder
//! away, never the files it shows (core only deletes a folder it made, directly under the portals folder).

use super::rows::{PortalItem, PortalKind};
use super::*;
use crate::components::menus::Node;
use crate::components::tab_strip::menu as tab_menu;
use glint_core::portals::{Portal, PortalSource};

/// The portals mounted now.
pub type PortalsSource = Arc<dyn Fn() -> Vec<PortalItem> + Send + Sync>;

/// What is told, with a `()`, whenever a portal is mounted or unmounted; `None` where nothing will tell.
pub type PortalChanges = Arc<dyn Fn() -> Option<async_channel::Receiver<()>> + Send + Sync>;

/// Something done to one portal, by its id: refreshing it or unmounting it.
pub type PortalWork = Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync>;

/// What the sidebar shows of a portal.
pub fn item_of(portal: &Portal) -> PortalItem {
    let short = |text: &str| text.chars().take(8).collect::<String>();
    let (kind, what) = match &portal.source {
        PortalSource::Archive { archive_path } => (PortalKind::Archive, archive_path.clone()),
        PortalSource::SavedSearch { query, root } => (
            PortalKind::Search,
            match (query.is_empty(), root.is_empty()) {
                (true, _) => root.clone(),
                (false, true) => query.clone(),
                (false, false) => format!("{query} · {root}"),
            },
        ),
        PortalSource::GitCommit { repo_root, rev, .. } => (PortalKind::Git, format!("{} · {repo_root}", short(rev))),
        PortalSource::ResticSnapshot { snapshot_id, .. } => (PortalKind::Backup, short(snapshot_id)),
    };
    PortalItem {
        id: portal.id.clone(),
        name: portal.name.clone(),
        path: portal.mount_path.clone(),
        kind,
        what,
        active: false,
    }
}

impl Sources {
    /// The portals glint-core has mounted, its change notice, and its refreshing and unmounting.
    pub(super) fn core_portals() -> (PortalsSource, PortalChanges, PortalWork, PortalWork) {
        let list: PortalsSource = Arc::new(|| glint_core::portals::portals_list().iter().map(item_of).collect());
        let changes: PortalChanges = Arc::new(|| {
            let (tx, rx) = async_channel::unbounded();
            // Told until the sidebar is gone (its receiver dropped), then the listener leaves core's list.
            glint_core::portals::on_change(move || tx.try_send(()).is_ok());
            Some(rx)
        });
        let refresh: PortalWork = Arc::new(|id| Box::pin(glint_core::portals::portal_refresh(id)));
        let unmount: PortalWork = Arc::new(|id| Box::pin(glint_core::portals::portal_unmount(id)));
        (list, changes, refresh, unmount)
    }
}

/// What the menu's rows do.
#[derive(Clone, Copy)]
enum Do {
    Open,
    CopyPath,
    Refresh,
    Unmount,
}

impl Sidebar {
    /// Reads the mounted portals again; the rows change only if they differ.
    pub(super) fn read_portals(&mut self, cx: &mut Context<Self>) {
        let now = (self.sources.portals)();
        if now != self.portals {
            self.portals = now;
            self.refresh(cx);
        }
    }

    /// Listens for portals coming and going (core says so), once per sidebar.
    pub(super) fn watch_portals(&mut self, cx: &mut Context<Self>) {
        let Some(changes) = (self.sources.portal_changes)() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while changes.recv().await.is_ok() {
                // Many at once (a mount and the unmount after it) are one read.
                while changes.try_recv().is_ok() {}
                if this.update(cx, |this, cx| this.read_portals(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Opens row `ix`'s menu at `at`, when it is a portal.
    pub(super) fn open_portal_menu(
        &mut self,
        ix: usize,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(Row::Portal(portal)) = self.rows.get(ix) else {
            return false;
        };
        let id = portal.id.clone();
        let nodes = vec![
            tab_menu::row(t!("Open"), IconName::FolderOpen, "open", id.clone()).node(),
            tab_menu::row(t!("Copy path"), IconName::Copy, "copy", id.clone()).node(),
            tab_menu::row(t!("Refresh"), IconName::RefreshCw, "refresh", id.clone()).node(),
            Some(Node::Separator),
            tab_menu::row(t!("Unmount"), IconName::Unplug, "unmount", id).node(),
        ];
        self.workspace_menu = None;
        self.portal_menu = tab_menu::Popup::open(
            nodes.into_iter().flatten().collect(),
            t!("Portal"),
            at,
            window,
            cx,
            |this, chose, window, cx| {
                this.portal_menu = None;
                // The keyboard comes back to the row it was on.
                window.focus(&this.focus, cx);
                if let Some(chose) = chose {
                    this.portal_chose(&chose, window, cx);
                }
                cx.notify();
            },
        );
        cx.notify();
        true
    }

    /// A row of a portal's menu was chosen: finds the portal again by its id (the list may have changed while the
    /// menu was open) and does it.
    fn portal_chose(&mut self, chose: &tab_menu::Chose, window: &mut Window, cx: &mut Context<Self>) {
        let Some(portal) = self.portals.iter().find(|p| p.id == chose.arg) else {
            return;
        };
        let (id, name, path) = (portal.id.clone(), portal.name.clone(), portal.path.clone());
        let what = match chose.tag.as_str() {
            "open" => Do::Open,
            "copy" => Do::CopyPath,
            "refresh" => Do::Refresh,
            "unmount" => Do::Unmount,
            _ => return,
        };
        self.portal_does(what, id, name, path, window, cx);
    }

    /// A choice from a portal's menu.
    fn portal_does(
        &mut self,
        what: Do,
        id: String,
        name: String,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match what {
            Do::Open => {
                self.explorer
                    .update(cx, |e, cx| e.navigate(path, NavOptions::default(), cx));
            }
            Do::CopyPath => {
                cx.write_to_clipboard(ClipboardItem::new_string(path));
                window.push_notification(Notification::success(t!("Copied the path")), cx);
            }
            Do::Refresh => self.portal_work(self.sources.portal_refresh.clone(), id, name, false, window, cx),
            Do::Unmount => self.portal_work(self.sources.portal_unmount.clone(), id, name, true, window, cx),
        }
    }

    /// Refreshes or unmounts a portal off the UI thread, and says how it went.
    fn portal_work(
        &mut self,
        work: PortalWork,
        id: String,
        name: String,
        unmounting: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let running = work(id);
        cx.spawn_in(window, async move |this, cx| {
            let result = core_bridge::run(running).await.and_then(|inner| inner);
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) if unmounting => {
                        window.push_notification(Notification::success(t!("Unmounted {name}", name = name)), cx)
                    }
                    Ok(()) => window.push_notification(Notification::success(t!("Refreshed {name}", name = name)), cx),
                    Err(reason) => window.push_notification(
                        Notification::error(reason).title(if unmounting {
                            t!("Couldn't unmount {name}", name = name)
                        } else {
                            t!("Couldn't refresh {name}", name = name)
                        }),
                        cx,
                    ),
                };
                // Unmounting changes the list; core says so too, and the second read finds nothing new.
                this.read_portals(cx);
            });
        })
        .detach();
    }

    /// A portal's row.
    pub(super) fn draw_portal(
        &self,
        ix: usize,
        p: &PortalItem,
        t: &Tokens,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (icon_name, kind) = match p.kind {
            PortalKind::Archive => (IconName::Archive, t!("Archive")),
            PortalKind::Search => (IconName::Search, t!("Search")),
            PortalKind::Git => (IconName::GitBranch, t!("Git commit")),
            PortalKind::Backup => (IconName::DatabaseBackup, t!("Backup")),
        };
        let name = SharedString::from(p.name.clone());
        let subtitle: SharedString = if p.what.is_empty() {
            kind.clone()
        } else {
            format!("{kind} · {}", p.what).into()
        };
        let tip = tooltip_of(name.clone(), format!("{subtitle}\n{}", p.path).into());
        let (accent, pressed, active) = (t.accent, t.pressed, p.active);
        let (glass, foreground, ring_color) = (t.glass, t.foreground, t.ring);
        let unmount_id = p.id.clone();
        let unmount_name = p.name.clone();
        let unmount_path = p.path.clone();
        let button = row_start()
            .id(("portal", ix))
            .role(Role::Button)
            .aria_label(name.clone())
            .aria_description(subtitle.clone())
            .aria_selected(active)
            .test_support()
            .relative()
            .w_full()
            .gap(space(2.5))
            .ps(2.)
            .pe(7.)
            .py(space(1.5))
            .rounded(theme::size::RADIUS_ROW)
            .border_1()
            .border_color(super::row_border(t, focused, active))
            .cursor_pointer()
            .text_color(t.foreground.opacity(0.9))
            .hover(move |s| s.bg(accent))
            .active(move |s| s.bg(pressed))
            .when(active, move |d| d.bg(glass).text_color(foreground))
            .on_click(cx.listener(move |this, _, window, cx| this.click(ix, window, cx)))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, _, cx| this.open_in_new_tab(ix, cx)),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                    this.right_click(ix, event.position, window, cx)
                }),
            )
            .tooltip(tip)
            .when(active, |d| d.child(Self::pill(t, space(4.))))
            .child(crate::art::chip(
                theme::rem_size(cx) * 1.2,
                theme::place_tone(theme::Place::Remote, cx),
                icon_name,
                cx,
            ))
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().child(isolate(name)))
                    .child(
                        div()
                            .truncate()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(isolate(subtitle)),
                    ),
            );
        let button = self.hop(button, ix, p.name.clone(), cx);
        let unmount = div()
            .absolute()
            .top_0()
            .bottom_0()
            .flex()
            .items_center()
            .when(crate::i18n::is_rtl(), |d| d.left(space(1.)))
            .when(!crate::i18n::is_rtl(), |d| d.right(space(1.)))
            .child(
                div()
                    .id(("portal-unmount", ix))
                    .role(Role::Button)
                    .aria_label(t!("Unmount {name}", name = p.name))
                    .test_support()
                    .tab_index(0)
                    .p(space(1.))
                    .rounded(theme::size::RADIUS_SM)
                    .border_1()
                    .border_color(gpui_kit::transparent_black())
                    .cursor_pointer()
                    .text_color(t.muted_foreground)
                    .opacity(0.)
                    .group_hover("row", |s| s.opacity(1.))
                    .focus_visible(move |s| s.opacity(1.).border_color(ring_color))
                    .hover(move |s| s.bg(accent).text_color(foreground))
                    .active(move |s| s.bg(pressed))
                    .tooltip(tooltip_of(t!("Unmount"), SharedString::default()))
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.portal_does(
                            Do::Unmount,
                            unmount_id.clone(),
                            unmount_name.clone(),
                            unmount_path.clone(),
                            window,
                            cx,
                        );
                    }))
                    .child(icon(IconName::Unplug).size_3p5()),
            );
        div()
            .group("row")
            .relative()
            .w_full()
            .pb(space(0.25))
            .child(button)
            .child(unmount)
            .into_any_element()
    }
}
