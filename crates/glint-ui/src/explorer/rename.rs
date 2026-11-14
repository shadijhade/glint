//! Rename. Ported from explorer.ts: `renameEntry`, `INVALID_NAME`, `INVALID_NAME_HINT`, and from Explorer.tsx's
//! `startRename` (F2).
//!
//! The file list shows the rename box while [`Tab::renaming`](super::Tab::renaming) is `Some`. F2 sets it
//! ([`Explorer::begin_rename`]); the box asks to commit what was typed ([`Explorer::commit_rename`]) or to
//! cancel ([`Explorer::cancel_rename`]), and both clear it. New items start renaming the same way.

use super::model::Explorer;
use super::ops::{notify_failed, run_op};
use super::ops_types::*;
use super::tab::Selection;
use super::types::*;
use glint_model::entries::{is_archived, is_closed, is_recycled, with_details, FileEntry, RawEntry};
use glint_model::js::js_trim;
use glint_model::paths::{join_path, parent_path};
use gpui_kit::{Context, Task};

/// The characters no Windows file name can contain (`INVALID_NAME`).
pub const INVALID_NAME_CHARS: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

/// What the person is told about them (`INVALID_NAME_HINT`).
pub const INVALID_NAME_HINT: &str = "A name can't contain any of these characters: \\ / : * ? \" < > |";

/// What a typed name comes to, or `None` when there is nothing to rename to: Windows drops trailing dots and
/// spaces itself, so they go here too (`name.trim().replace(/[. ]+$/, "")`). `Err(())` for a name with a
/// character no Windows name can hold.
pub fn clean_name(name: &str) -> Option<Result<&str, ()>> {
    let trimmed = js_trim(name).trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains(INVALID_NAME_CHARS) {
        return Some(Err(()));
    }
    Some(Ok(trimmed))
}

impl Explorer {
    /// Puts `path` in the rename box, and makes it the whole selection, the focus and the anchor.
    fn start_renaming(&mut self, tab: TabId, path: &str, cx: &mut Context<Self>) {
        self.update_tab(tab, |t| {
            t.renaming = Some(path.to_string());
            t.selection = Selection::of([path]);
            t.selection.anchor = Some(path.to_string());
            t.selection.focus = Some(path.to_string());
        });
        cx.emit(ExplorerEvent::SelectionChanged(tab));
        self.tab_changed(tab, cx);
    }

    /// Starts renaming whatever the tab's focus is on (a new item has just been selected and focused).
    pub(super) fn rename_focused(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let focus = self.tabs.get(&tab).and_then(|t| t.selection.focus.clone());
        self.update_tab(tab, |t| t.renaming = focus);
        self.tab_changed(tab, cx);
    }

    /// F2 (or Rename in a menu): puts `path` (by default the focused item, else the first selected) in the rename
    /// box and selects only it (`startRename`). Several items selected and no `path` given open the batch rename
    /// instead ([`ExplorerEvent::BatchRename`]). Archives, remote connections, closed folders and the Recycle Bin
    /// say they can't be changed.
    pub fn begin_rename(&mut self, path: Option<&str>, cx: &mut Context<Self>) {
        let id = self.active_tab_id();
        let Some(tab) = self.tabs.get(&id) else {
            return;
        };
        if tab.archive().is_some() || self.remote_of(Some(id)) {
            self.read_only_notice(Some(id), cx);
            return;
        }
        if tab.is_closed() {
            self.closed_notice(Some(id), cx);
            return;
        }
        if tab.is_recycle_bin() {
            self.recycled_notice(cx);
            return;
        }
        if path.is_none() {
            // The items chosen are those of the selection that the list shows.
            let chosen = tab
                .selection
                .iter()
                .filter(|p| tab.rows.position_of(p).is_some())
                .map(str::to_string)
                .collect::<Vec<_>>();
            if chosen.len() > 1 {
                cx.emit(ExplorerEvent::BatchRename(chosen));
                return;
            }
        }
        let target = path
            .map(str::to_string)
            .or_else(|| tab.selection.focus.clone())
            .or_else(|| tab.selection.iter().next().map(str::to_string));
        if let Some(target) = target {
            self.start_renaming(id, &target, cx);
        }
    }

    /// Escape in the rename box: closes it, renaming nothing. The active tab's.
    #[cfg(test)]
    pub fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        let id = self.active_tab_id();
        self.cancel_rename_in(id, cx);
    }

    /// [`Explorer::cancel_rename`] for a tab (the pane the box is in may not be the active one).
    pub fn cancel_rename_in(&mut self, tab: TabId, cx: &mut Context<Self>) {
        if self.tabs.get(&tab).is_none_or(|t| t.renaming.is_none()) {
            return;
        }
        self.update_tab(tab, |t| t.renaming = None);
        self.tab_changed(tab, cx);
    }

    /// Enter in the rename box: renames the item being renamed to `name` (`renameEntry`) and selects it under its
    /// new name. The active tab's. Resolves whether the item was renamed.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    #[cfg(test)]
    pub fn commit_rename(&mut self, name: &str, cx: &mut Context<Self>) -> Task<bool> {
        let id = self.active_tab_id();
        self.commit_rename_in(id, name, cx)
    }

    /// [`Explorer::commit_rename`] for a tab. The box closes whatever comes of it; nothing is renamed when the
    /// name is empty or unchanged, a name Windows can't have is told, a failure is told.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn commit_rename_in(&mut self, tab: TabId, name: &str, cx: &mut Context<Self>) -> Task<bool> {
        let Some(renaming) = self.tabs.get_mut(&tab).and_then(|t| t.renaming.take()) else {
            return Task::ready(false);
        };
        self.tab_changed(tab, cx);
        let entry = self.tabs.get(&tab).and_then(|t| {
            let entries: &[FileEntry] = match &t.find {
                Some(find) => &find.entries,
                None => t.entries(),
            };
            entries.iter().find(|e| e.path == renaming).cloned()
        });
        let Some(entry) = entry else {
            return Task::ready(false);
        };
        if is_archived(&entry) {
            self.read_only_notice(Some(tab), cx);
            return Task::ready(false);
        }
        if is_closed(&entry) {
            self.closed_notice(Some(tab), cx);
            return Task::ready(false);
        }
        if is_recycled(&entry) {
            self.recycled_notice(cx);
            return Task::ready(false);
        }
        let trimmed = match clean_name(name) {
            None => return Task::ready(false),
            Some(Ok(trimmed)) if trimmed == entry.name => return Task::ready(false),
            Some(Ok(trimmed)) => trimmed.to_string(),
            Some(Err(())) => {
                cx.emit(ExplorerEvent::Operation(OpNotice::InvalidName));
                return Task::ready(false);
            }
        };
        cx.spawn(async move |this, cx| {
            let op = FileOp::Rename {
                path: entry.path.clone(),
                name: trimmed.clone(),
            };
            match run_op(&this, cx, op).await {
                Ok(false) => false,
                Err(message) => {
                    notify_failed(
                        &this,
                        cx,
                        Failing::Rename {
                            name: entry.name.clone(),
                        },
                        message,
                    );
                    false
                }
                Ok(true) => {
                    let renamed = join_path(&parent_path(&entry.path), &trimmed);
                    let shown = this
                        .update(cx, |this, cx| this.show_renamed(tab, &entry, &trimmed, &renamed, cx))
                        .ok()
                        .flatten();
                    if let Some(navigation) = shown {
                        navigation.await;
                    }
                    true
                }
            }
        })
    }

    /// The folder (or the search's results) shows the new name, selected.
    fn show_renamed(
        &mut self,
        tab: TabId,
        entry: &FileEntry,
        name: &str,
        renamed: &str,
        cx: &mut Context<Self>,
    ) -> Option<Task<bool>> {
        let t = self.tabs.get(&tab)?;
        let Some(find) = &t.find else {
            let path = t.path.clone();
            return Some(self.show_in(tab, &path, Some(vec![renamed.to_string()]), cx));
        };
        let entries: Vec<FileEntry> = find
            .entries
            .iter()
            .map(|e| {
                if e.path == entry.path {
                    with_details(
                        RawEntry {
                            name: name.to_string(),
                            path: Some(renamed.to_string()),
                            dir: e.dir,
                            size: e.size,
                            modified: e.modified,
                            attrs: e.attrs,
                            origin: e.origin.clone(),
                        },
                        &parent_path(renamed),
                    )
                } else {
                    e.clone()
                }
            })
            .collect();
        self.set_find_entries(tab, entries, Some(renamed), cx);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_name_loses_trailing_dots_and_spaces_like_windows_does() {
        assert_eq!(clean_name("  report.txt  "), Some(Ok("report.txt")));
        assert_eq!(clean_name("notes. . "), Some(Ok("notes")));
        assert_eq!(clean_name(" . "), None);
        assert_eq!(clean_name(""), None);
        // Dots inside a name stay.
        assert_eq!(clean_name("a.b.c"), Some(Ok("a.b.c")));
    }

    #[test]
    fn a_name_with_a_character_windows_refuses_is_told_apart_from_an_empty_one() {
        for bad in ["a\\b", "a/b", "a:b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b"] {
            assert_eq!(clean_name(bad), Some(Err(())), "{bad}");
        }
        // Arabic names and spaces are fine.
        assert_eq!(clean_name("تقرير 2024.pdf"), Some(Ok("تقرير 2024.pdf")));
    }

    #[test]
    fn the_hint_lists_the_same_characters_as_the_check() {
        for c in INVALID_NAME_CHARS {
            assert!(INVALID_NAME_HINT.contains(c), "{c}");
        }
    }
}
