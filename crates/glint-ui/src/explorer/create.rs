//! New folder, New file and the New menu's kinds. Ported from explorer.ts: `newItem`, `newOfKind`, and from
//! newKinds.ts: `loadNewKinds`, `newKinds`.
//!
//! A new item is made in the open folder, named as File Explorer names it ("New folder", "New folder (2)"),
//! the folder is read again with it selected, and its name goes into the rename box. Undo recycles it. A
//! search shown in the tab ends first: the new item belongs to the folder.

use super::model::Explorer;
use super::ops::notify_failed;
use super::ops_types::*;
use super::types::*;
use glint_model::paths::{base_name, is_folder_path};
use gpui_kit::{Context, Task};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The New menu's kinds are read again after this long (an app installed since then shows up).
const KINDS_FRESH: Duration = Duration::from_secs(30);

impl Explorer {
    /// File Explorer's New menu kinds on this PC, as last read; each look starts another read in the background
    /// where the last is older than 30 s, and [`ExplorerEvent::NewKindsChanged`] says when it brings something
    /// new (`newKinds`).
    pub fn new_kinds(&mut self, cx: &mut Context<Self>) -> Arc<[NewKind]> {
        self.load_new_kinds(cx);
        self.ops.new_kinds.clone()
    }

    /// Reads the New menu's kinds, unless they were read lately or are being read (`loadNewKinds`). A failed read
    /// leaves the kinds as they were and is tried again at the next look.
    pub fn load_new_kinds(&mut self, cx: &mut Context<Self>) {
        let fresh = self.ops.kinds_asked.is_some_and(|at| at.elapsed() < KINDS_FRESH);
        if self.ops.kinds_loading || fresh {
            return;
        }
        self.ops.kinds_loading = true;
        let reading = self.source.new_kinds();
        cx.spawn(async move |this, cx| {
            let found = reading.await;
            this.update(cx, |this, cx| {
                this.ops.kinds_loading = false;
                if let Ok(kinds) = found {
                    this.ops.kinds_asked = Some(Instant::now());
                    if kinds[..] != this.ops.new_kinds[..] {
                        this.ops.new_kinds = Arc::from(kinds);
                        cx.emit(ExplorerEvent::NewKindsChanged);
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Where a new item can be made: the open folder, if it is a folder on disk that can be changed. Says why not
    /// when it can't (an archive, a folder Windows keeps closed).
    fn new_item_folder(&mut self, cx: &mut Context<Self>) -> Option<(TabId, String)> {
        let tab = self.active_tab_id();
        let path = self.tabs.get(&tab)?.path.clone();
        if !is_folder_path(&path) {
            return None;
        }
        if self.archive_of(Some(tab)).is_some() {
            self.read_only_notice(Some(tab), cx);
            return None;
        }
        if self.closed_of(Some(tab)) {
            self.closed_notice(Some(tab), cx);
            return None;
        }
        Some((tab, path))
    }

    /// A search shown in the tab ends (stopped if it runs) and its filter clears: the new item is in the folder.
    fn end_search(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let Some(t) = self.tabs.get(&tab) else {
            return;
        };
        let running = t.find.as_ref().map(|f| f.running);
        if running.is_none() && t.filter.is_empty() {
            return;
        }
        self.update_tab(tab, |t| {
            t.find = None;
            t.filter.clear();
        });
        if let Some(was_running) = running {
            self.stop_find_job(tab, was_running);
            cx.emit(ExplorerEvent::FindDropped { tab, was_running });
        }
        self.rebuild_rows(tab, cx);
        self.tab_changed(tab, cx);
    }

    /// Creates "New folder" (or a new text file) in the open folder and starts renaming it (`newItem`).
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn new_item(&mut self, dir: bool, cx: &mut Context<Self>) -> Task<()> {
        let Some((tab, path)) = self.new_item_folder(cx) else {
            return Task::ready(());
        };
        let creating = self
            .source
            .create_entry(&path, if dir { "New folder" } else { "New Text Document.txt" }, dir);
        cx.spawn(async move |this, cx| {
            let created = match creating.await {
                Ok(created) => created,
                Err(message) => return notify_failed(&this, cx, Failing::Create { dir }, message),
            };
            let navigation = this
                .update(cx, |this, cx| {
                    this.remember_undo(
                        format!("new {} “{}”", if dir { "folder" } else { "file" }, base_name(&created)),
                        UndoAction::Recycle(vec![created.clone()]),
                        cx,
                    );
                    this.end_search(tab, cx);
                    this.show_in(tab, &path, Some(vec![created]), cx)
                })
                .ok();
            if let Some(navigation) = navigation {
                navigation.await;
                this.update(cx, |this, cx| this.rename_focused(tab, cx)).ok();
            }
        })
    }

    /// Makes a file of a kind Windows' New menu has (a Word document, a zip…) in the open folder, named as File
    /// Explorer names it, and starts renaming it; one its app makes is selected once it's there (`newOfKind`).
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn new_of_kind(&mut self, kind: NewKind, cx: &mut Context<Self>) -> Task<()> {
        let Some((tab, path)) = self.new_item_folder(cx) else {
            return Task::ready(());
        };
        let making = self.source.new_file(&path, &kind.ext);
        cx.spawn(async move |this, cx| {
            let created = match making.await {
                Ok(created) => created,
                Err(message) => {
                    return notify_failed(
                        &this,
                        cx,
                        Failing::MakeNew {
                            kind: kind.name.clone(),
                        },
                        message,
                    );
                }
            };
            let navigation = this
                .update(cx, |this, cx| {
                    match &created {
                        Some(created) => {
                            this.remember_undo(
                                format!("new file “{}”", base_name(created)),
                                UndoAction::Recycle(vec![created.clone()]),
                                cx,
                            );
                        }
                        None => cx.emit(ExplorerEvent::Operation(OpNotice::AppIsMakingIt {
                            kind: kind.name.clone(),
                        })),
                    }
                    this.end_search(tab, cx);
                    this.show_in(tab, &path, created.clone().map(|c| vec![c]), cx)
                })
                .ok();
            if let Some(navigation) = navigation {
                navigation.await;
                if created.is_some() && !kind.by_app {
                    this.update(cx, |this, cx| this.rename_focused(tab, cx)).ok();
                }
            }
        })
    }
}
