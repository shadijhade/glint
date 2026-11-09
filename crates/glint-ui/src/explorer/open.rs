//! Opening things: files, entries, typed locations, and Get access. Ported from explorer.ts: `openFile`,
//! `openEntry`, `goTo`, `getAccess`, and the notices `closedNotice`, `recycledNotice` and `readOnlyNotice`
//! (which raise a [`Notice`] for a view to show instead of calling a toast).
//!
//! What opening a file does is Windows' business: [`Source::open_item`](super::source::Source::open_item)
//! runs the shell's default action (the file's app, Windows' app picker, a shortcut's target). A shortcut to
//! a folder comes back as that folder, and a tab opens it.

use super::model::Explorer;
use super::types::*;
use glint_model::entries::{is_archive_file, is_archived, is_closed, is_recycled, is_remote, FileEntry};
use glint_model::paths::{
    is_gallery, is_recent, is_recycle_bin, is_shadow_path, parent_path, same_path, GALLERY, RECENT, RECYCLE_BIN,
    THIS_PC,
};
use gpui_kit::{Context, Task};

use super::source::Access;

/// Whether `entry` is a disc or virtual-disk image Windows can mount as a drive (.iso, .img, .vhd, .vhdx)
/// (`isDiskImage`).
pub fn is_disk_image(entry: &FileEntry) -> bool {
    !entry.dir
        && !is_archived(entry)
        && !is_remote(entry)
        && !is_recycled(entry)
        && ["iso", "img", "vhd", "vhdx"].contains(&entry.ext.as_str())
}

impl Explorer {
    /// Says Windows keeps a folder closed when something would open or change what's in it (`closedNotice`),
    /// with Get access where it can help.
    pub fn closed_notice(&mut self, tab: Option<TabId>, cx: &mut Context<Self>) {
        let id = tab.unwrap_or_else(|| self.active_tab_id());
        let Some(t) = self.tabs.get(&id) else {
            return;
        };
        let can_get_access = t.no_access().is_some_and(|n| !n.remote && !n.refused);
        cx.emit(ExplorerEvent::Notice(Notice::Closed {
            path: t.path.clone(),
            tab: id,
            from_index: t.listing.as_ref().is_some_and(|l| l.from_index),
            can_get_access,
        }));
    }

    /// Says what's in the Recycle Bin is restored, or deleted for good, when something would open, copy or
    /// change it there (`recycledNotice`).
    pub fn recycled_notice(&mut self, cx: &mut Context<Self>) {
        cx.emit(ExplorerEvent::Notice(Notice::InRecycleBin));
    }

    /// Archives and remotes open read-only: says so when something would change one (`readOnlyNotice`).
    pub fn read_only_notice(&mut self, tab: Option<TabId>, cx: &mut Context<Self>) {
        let remote = self.remote_of(tab);
        cx.emit(ExplorerEvent::Notice(Notice::ReadOnly { remote }));
    }

    /// Opens a file with its app, as a double-click in File Explorer does; a shortcut to a folder opens the
    /// folder in `tab` (the active one by default) (`openFile`). Resolves whether it opened.
    #[must_use = "dropping the task cancels opening; `.detach()` it to let it run"]
    pub fn open_file(&mut self, path: &str, tab: Option<TabId>, cx: &mut Context<Self>) -> Task<bool> {
        let id = tab.unwrap_or_else(|| self.active_tab_id());
        let opening = self.source.open_item(path);
        let path = path.to_string();
        cx.spawn(async move |this, cx| match opening.await {
            Ok(Some(folder)) => {
                match this.update(cx, |this, cx| this.navigate_task(folder, NavOptions::in_tab(id), cx)) {
                    Ok(navigation) => navigation.await,
                    Err(_) => false,
                }
            }
            Ok(None) => true,
            Err(message) => {
                this.update(cx, |_, cx| {
                    cx.emit(ExplorerEvent::Notice(Notice::CouldNotOpenFile { path, message }));
                })
                .ok();
                false
            }
        })
    }

    /// Enter or double-click on a row (`openEntry`). A folder or an archive opens as a folder; a file opens
    /// with its app. The Recycle Bin's items are looked at, not opened; items listed from the index in a
    /// closed folder can't be opened. Resolves whether something opened.
    ///
    /// A previous version of a file, a disk image and a file inside an archive are [`Handoff`]s: the parts
    /// that own those (previous versions, mounting, archive extraction) act on the event.
    #[must_use = "dropping the task cancels opening; `.detach()` it to let it run"]
    pub fn open_entry(&mut self, entry: &FileEntry, cx: &mut Context<Self>) -> Task<bool> {
        // Listed from the index: its folders open the same way, its files not at all.
        if is_closed(entry) && !entry.dir {
            self.closed_notice(None, cx);
            return Task::ready(false);
        }
        // In the Recycle Bin, an item is looked at, not opened: what's done to it there would be lost.
        if is_recycled(entry) {
            let id = self.active_tab_id();
            self.update_tab(id, |t| {
                t.selection.focus = Some(entry.path.clone());
            });
            self.quick_look = true;
            cx.emit(ExplorerEvent::SelectionChanged(id));
            cx.notify();
            return Task::ready(true);
        }
        // An archive opens like a folder.
        if entry.dir || is_archive_file(entry) {
            return self.navigate_task(entry.path.clone(), NavOptions::default(), cx);
        }
        // A previous version opens as a read-only copy: apps can't open it where it is.
        if is_shadow_path(&entry.path) {
            cx.emit(ExplorerEvent::Handoff(Handoff::VersionFile(entry.path.clone())));
            return Task::ready(true);
        }
        if is_disk_image(entry) {
            cx.emit(ExplorerEvent::Handoff(Handoff::DiskImage(entry.path.clone())));
            return Task::ready(true);
        }
        if is_archived(entry) {
            cx.emit(ExplorerEvent::Handoff(Handoff::ArchivedFile(Box::new(entry.clone()))));
            return Task::ready(true);
        }
        self.open_file(&entry.path, None, cx)
    }

    /// Opens what someone typed: "This PC", "Recycle Bin", a folder, or a file (its folder, with it selected)
    /// (`goTo`). The task resolves whether a folder opened.
    #[must_use = "dropping the task cancels the navigation; `.detach()` it to let it run"]
    pub fn go_to(&mut self, text: &str, cx: &mut Context<Self>) -> Task<bool> {
        let value = text.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("this pc") {
            return self.navigate_task(THIS_PC, NavOptions::default(), cx);
        }
        if value.eq_ignore_ascii_case("recycle bin") || is_recycle_bin(value) {
            return self.navigate_task(RECYCLE_BIN, NavOptions::default(), cx);
        }
        if value.eq_ignore_ascii_case("recent") || is_recent(value) {
            return self.navigate_task(RECENT, NavOptions::default(), cx);
        }
        if value.eq_ignore_ascii_case("gallery") || is_gallery(value) {
            return self.navigate_task(GALLERY, NavOptions::default(), cx);
        }
        let id = self.active_tab_id();
        let resolving = self.source.resolve(value);
        cx.spawn(async move |this, cx| match resolving.await {
            Ok(target) => {
                let options = NavOptions::in_tab(id);
                let navigation = if target.dir {
                    this.update(cx, |this, cx| this.navigate_task(target.path, options, cx))
                } else {
                    let folder = parent_path(&target.path);
                    let options = options.selecting(target.path);
                    this.update(cx, |this, cx| this.navigate_task(folder, options, cx))
                };
                match navigation {
                    Ok(navigation) => navigation.await,
                    Err(_) => false,
                }
            }
            Err(message) => {
                if message != "cancelled" {
                    this.update(cx, |_, cx| cx.emit(ExplorerEvent::Notice(Notice::Message(message))))
                        .ok();
                }
                false
            }
        })
    }

    /// Get access, as File Explorer's Continue button does: this account may open the tab's folder, and
    /// everything in it, from then on. Windows asks for administrator approval, unless the folder is the
    /// account's own to change (`getAccess`). The folder opens if it worked; otherwise the tab says why not.
    #[must_use = "dropping the task cancels the request; `.detach()` it to let it run"]
    pub fn get_access(&mut self, tab: Option<TabId>, cx: &mut Context<Self>) -> Task<()> {
        let id = tab.unwrap_or_else(|| self.active_tab_id());
        let Some(t) = self.tabs.get(&id).filter(|t| t.no_access().is_some()) else {
            return Task::ready(());
        };
        let path = t.path.clone();
        let asking = self.source.grant_access(&path);
        cx.spawn(async move |this, cx| {
            let result = asking.await;
            // The tab may have closed or moved on while Windows asked.
            let still_here = this
                .update(cx, |this, _| {
                    this.tabs
                        .get(&id)
                        .is_some_and(|t| t.no_access().is_some() && same_path(&t.path, &path))
                })
                .unwrap_or(false);
            if !still_here {
                return;
            }
            match result {
                Ok(Access::Granted) => {
                    let options = NavOptions {
                        no_history: true,
                        quiet: true,
                        ..NavOptions::in_tab(id)
                    };
                    if let Ok(navigation) = this.update(cx, |this, cx| this.navigate_task(path, options, cx)) {
                        navigation.await;
                    }
                }
                Ok(denied) => {
                    this.update(cx, |this, cx| {
                        this.update_tab(id, |t| {
                            if let LoadState::NoAccess(no_access) = &mut t.load {
                                no_access.refused = denied == Access::Refused;
                                no_access.still_denied = denied == Access::StillDenied;
                            }
                        });
                        this.tab_changed(id, cx);
                    })
                    .ok();
                }
                Err(message) => {
                    if message != "cancelled" {
                        this.update(cx, |_, cx| {
                            cx.emit(ExplorerEvent::Notice(Notice::CouldNotGetAccess { path, message }));
                        })
                        .ok();
                    }
                }
            }
        })
    }
}
