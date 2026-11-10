//! Cut, copy and paste. Ported from explorer.ts: `copyToClipboard`, `paste`, `pasteQueued`, `pasteAsFile`.
//!
//! Files go on the clipboard the way File Explorer puts them, so they paste there too, and files copied there
//! paste here. Cut items show dimmed until pasted ([`Explorer::cut_paths`]). A paste is a copy or move through
//! Windows' file engine (or the copy queue where Settings say so); with no files on the clipboard, a picture or
//! text on it becomes a new file.

use super::model::Explorer;
use super::ops::{notify_failed, run_op_busy};
use super::ops_types::*;
use super::queue::queue_in;
use super::types::*;
use glint_model::paths::{base_name, is_folder_path, is_remote_path, join_path, same_path, THIS_PC};
use gpui_kit::{AsyncApp, Context, Task, WeakEntity};
use std::collections::HashSet;
use std::sync::Arc;

impl Explorer {
    /// Paths cut to the clipboard here, shown dimmed until pasted (`cut`).
    pub fn cut_paths(&self) -> &Arc<HashSet<String>> {
        &self.ops.cut
    }

    fn set_cut(&mut self, paths: impl IntoIterator<Item = String>, cx: &mut Context<Self>) {
        let next: HashSet<String> = paths.into_iter().collect();
        if *self.ops.cut == next {
            return;
        }
        self.ops.cut = Arc::new(next);
        cx.emit(ExplorerEvent::CutChanged);
        cx.notify();
    }

    /// Ctrl+C and Ctrl+X: puts `paths` on the clipboard (`copyToClipboard`). Cut items show dimmed. From an
    /// archive or a remote connection, files are extracted first ([`ArchiveJob::ToClipboard`]); they can't be
    /// cut. The Recycle Bin and folders Windows keeps closed say so instead.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn copy_to_clipboard(&mut self, paths: Vec<String>, cut: bool, cx: &mut Context<Self>) -> Task<()> {
        if paths.is_empty() {
            return Task::ready(());
        }
        if cut && self.closed_of(None) {
            self.closed_notice(None, cx);
            return Task::ready(());
        }
        if self.recycle_bin_of(None) {
            self.recycled_notice(cx);
            return Task::ready(());
        }
        if self.archive_of(None).is_some() || self.remote_of(None) {
            if cut {
                self.read_only_notice(None, cx);
            } else {
                // Extracted first: the clipboard carries files. Pasting (here or in File Explorer) copies them.
                cx.emit(ExplorerEvent::FromArchive(ArchiveJob::ToClipboard { paths }));
            }
            return Task::ready(());
        }
        let setting = self.source.clipboard_set_files(paths.clone(), cut);
        cx.spawn(async move |this, cx| {
            let result = setting.await;
            this.update(cx, |this, cx| match result {
                Ok(()) => {
                    this.set_cut(if cut { paths.clone() } else { Vec::new() }, cx);
                    cx.emit(ExplorerEvent::Operation(OpNotice::OnClipboard {
                        count: paths.len(),
                        cut,
                    }));
                }
                Err(message) => cx.emit(ExplorerEvent::Operation(OpNotice::Failed {
                    what: Failing::Clipboard,
                    message,
                })),
            })
            .ok();
        })
    }

    /// Files extracted from an archive for the clipboard are on it: the second half of
    /// [`ArchiveJob::ToClipboard`]. They are copies, so nothing shows as cut.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn copied_staged(&mut self, staged: Vec<String>, cx: &mut Context<Self>) -> Task<()> {
        let setting = self.source.clipboard_set_files(staged.clone(), false);
        cx.spawn(async move |this, cx| {
            let result = setting.await;
            this.update(cx, |this, cx| match result {
                Ok(()) => {
                    this.set_cut(Vec::new(), cx);
                    cx.emit(ExplorerEvent::Operation(OpNotice::OnClipboard {
                        count: staged.len(),
                        cut: false,
                    }));
                }
                Err(message) => cx.emit(ExplorerEvent::Operation(OpNotice::Failed {
                    what: Failing::Clipboard,
                    message,
                })),
            })
            .ok();
        })
    }

    /// Ctrl+V: pastes files copied or cut here or in File Explorer into `dest` (the open folder by default)
    /// (`paste`). With no files on the clipboard, a picture or text on it becomes a new file named in place.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn paste(&mut self, dest: Option<&str>, cx: &mut Context<Self>) -> Task<()> {
        let dest = dest.map_or_else(|| self.current_path().to_string(), str::to_string);
        let Some(tab) = self.paste_target(&dest, cx) else {
            return Task::ready(());
        };
        let reading = self.source.clipboard_get_files();
        cx.spawn(async move |this, cx| {
            let files = match reading.await {
                Ok(files) => files,
                Err(message) => return notify_failed(&this, cx, Failing::ReadClipboard, message),
            };
            let Some(files) = files.filter(|f| !f.paths.is_empty()) else {
                return paste_as_file(&this, cx, dest, tab).await;
            };
            let count = files.paths.len();
            let (op, working) = if files.cut {
                (
                    FileOp::Move {
                        sources: files.paths.clone(),
                        dest: dest.clone(),
                    },
                    Working::Moving(count),
                )
            } else {
                (
                    FileOp::Copy {
                        sources: files.paths.clone(),
                        dest: dest.clone(),
                    },
                    Working::Copying(count),
                )
            };
            match run_op_busy(&this, cx, op, working).await {
                Err(message) => notify_failed(&this, cx, Failing::Paste, message),
                Ok(done) => {
                    if done && files.cut {
                        forget_cut(&this, cx).await;
                    }
                    let navigation = this
                        .update(cx, |this, cx| {
                            let shows = this
                                .tabs
                                .get(&tab)
                                .is_some_and(|t| same_path(&dest, &t.path) && t.find.is_none());
                            let select = files.paths.iter().map(|p| join_path(&dest, base_name(p))).collect();
                            shows.then(|| this.show_in(tab, &dest, Some(select), cx))
                        })
                        .ok()
                        .flatten();
                    if let Some(navigation) = navigation {
                        navigation.await;
                    }
                }
            }
        })
    }

    /// Checks that `dest` can be pasted into, saying why not if it can't; the active tab's id if it can.
    fn paste_target(&mut self, dest: &str, cx: &mut Context<Self>) -> Option<TabId> {
        let tab = self.active_tab_id();
        if is_remote_path(dest) {
            self.read_only_notice(Some(tab), cx);
            return None;
        }
        if !is_folder_path(dest) {
            return None;
        }
        let here = same_path(dest, self.tabs.get(&tab).map_or(THIS_PC, |t| t.path.as_str()));
        if here && self.archive_of(Some(tab)).is_some() {
            self.read_only_notice(Some(tab), cx);
            return None;
        }
        if here && self.closed_of(Some(tab)) {
            self.closed_notice(Some(tab), cx);
            return None;
        }
        Some(tab)
    }

    /// Paste with the copy queue: what's on the clipboard copied (or, cut, moved) into `dest` in turn, with Pause
    /// and Resume, and checked after copying (Settings → Files) (`pasteQueued`).
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn paste_queued(&mut self, dest: Option<&str>, cx: &mut Context<Self>) -> Task<()> {
        let dest = dest.map_or_else(|| self.current_path().to_string(), str::to_string);
        if is_remote_path(&dest) {
            let tab = self.active_tab_id();
            self.read_only_notice(Some(tab), cx);
            return Task::ready(());
        }
        if !is_folder_path(&dest) {
            return Task::ready(());
        }
        let reading = self.source.clipboard_get_files();
        cx.spawn(async move |this, cx| {
            let files = match reading.await {
                Ok(files) => files,
                Err(message) => return notify_failed(&this, cx, Failing::ReadClipboard, message),
            };
            let Some(files) = files.filter(|f| !f.paths.is_empty()) else {
                this.update(cx, |_, cx| {
                    cx.emit(ExplorerEvent::Operation(OpNotice::NoFilesOnClipboard))
                })
                .ok();
                return;
            };
            let kind = if files.cut { QueueKind::Move } else { QueueKind::Copy };
            queue_in(&this, cx, kind, files.paths, dest).await;
            if files.cut {
                forget_cut(&this, cx).await;
            }
        })
    }
}

/// What was cut has been pasted: it shows normal again, and the clipboard is emptied.
async fn forget_cut(this: &WeakEntity<Explorer>, cx: &mut AsyncApp) {
    let Ok(source) = this.update(cx, |this, cx| {
        this.set_cut(Vec::new(), cx);
        this.source.clone()
    }) else {
        return;
    };
    // A failure to empty the clipboard changes nothing (`.catch(() => {})`).
    let _ = source.clipboard_clear().await;
}

/// Paste with no files on the clipboard: its picture or its text as a new file in `dest` ("Pasted image <date>.png",
/// "Pasted text <date>.txt"), selected to be named in the folder shown, and Undo recycles it (`pasteAsFile`).
async fn paste_as_file(this: &WeakEntity<Explorer>, cx: &mut AsyncApp, dest: String, tab: TabId) {
    let Ok(source) = this.update(cx, |this, _| this.source.clone()) else {
        return;
    };
    let made = match source.clipboard_paste_file(&dest).await {
        Ok(Some(made)) => made,
        Ok(None) => {
            this.update(cx, |_, cx| cx.emit(ExplorerEvent::Operation(OpNotice::NothingToPaste)))
                .ok();
            return;
        }
        Err(message) => return notify_failed(this, cx, Failing::Paste, message),
    };
    let navigation = this
        .update(cx, |this, cx| {
            this.remember_undo(
                format!("paste of “{}”", base_name(&made)),
                UndoAction::Recycle(vec![made.clone()]),
                cx,
            );
            let shows = this
                .tabs
                .get(&tab)
                .is_some_and(|t| same_path(&dest, &t.path) && t.find.is_none());
            if shows {
                Some(this.show_in(tab, &dest, Some(vec![made.clone()]), cx))
            } else {
                cx.emit(ExplorerEvent::Operation(OpNotice::PastedInto {
                    made: made.clone(),
                    dest: dest.clone(),
                }));
                None
            }
        })
        .ok()
        .flatten();
    if let Some(navigation) = navigation {
        navigation.await;
        this.update(cx, |this, cx| this.rename_focused(tab, cx)).ok();
    }
}
