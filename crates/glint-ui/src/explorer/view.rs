//! What the views ask of the explorer besides opening and selecting one item: the keyboard's focus on its own,
//! the marquee's selection, the list's columns, a folder's own view, the address bar's editing flag, and
//! ejecting a drive. Ported from the small `updateTab` calls and `setColumns`/`setFolderView` uses in
//! FileList.tsx, FileGrid.tsx, Explorer.tsx, AddressBar.tsx and the sidebar.

use super::model::{now_ms, Explorer, Prefs};
use super::tab::{PathSet, Selection};
use super::types::*;
use glint_model::columns::ColumnId;
use glint_model::paths::THIS_PC;
use glint_model::store::FolderViewPatch;
use gpui_kit::{Context, Task};
use std::sync::Arc;

impl Explorer {
    /// Moves the keyboard's focus in the active tab without touching the selection (Ctrl+arrows; a press on one
    /// item of a multi-selection, which keeps the others selected until the button comes up). `anchor`, when
    /// given, is where a Shift-selection will grow from; `None` leaves the anchor where it is. A path is
    /// not checked against the rows.
    pub fn set_focus(&mut self, path: Option<&str>, anchor: Option<&str>, cx: &mut Context<Self>) {
        let id = self.active_tab_id();
        let Some(tab) = self.tabs.get(&id) else {
            return;
        };
        let focus = path.map(str::to_string);
        let anchor = anchor.map(str::to_string).or_else(|| tab.selection.anchor.clone());
        if focus == tab.selection.focus && anchor == tab.selection.anchor {
            return;
        }
        let selection = Selection {
            paths: Arc::clone(&tab.selection.paths),
            anchor,
            focus,
        };
        self.apply_selection(id, selection, cx);
    }

    /// Selects exactly `paths`, with the anchor and the focus given (the marquee: dragging a rectangle over empty
    /// space selects what it covers). The active tab's. Nothing is raised when nothing changes, so a marquee
    /// can call it on every pointer move.
    pub fn set_selection<S: AsRef<str>>(
        &mut self,
        paths: impl IntoIterator<Item = S>,
        anchor: Option<&str>,
        focus: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let id = self.active_tab_id();
        let Some(tab) = self.tabs.get(&id) else {
            return;
        };
        let mut set = PathSet::default();
        for path in paths {
            set.insert(path.as_ref());
        }
        let (anchor, focus) = (anchor.map(str::to_string), focus.map(str::to_string));
        let current = &tab.selection;
        let same = current.anchor == anchor
            && current.focus == focus
            && current.len() == set.len()
            && set.iter().all(|p| current.contains(p));
        if same {
            return;
        }
        let selection = Selection {
            paths: Arc::new(set),
            anchor,
            focus,
        };
        self.apply_selection(id, selection, cx);
    }

    /// Chooses the columns the active tab's list shows (the header menu's Columns): a folder keeps its own
    /// (with Settings → "Each folder keeps its own view"), otherwise every folder shows these (`setColumns`).
    pub fn set_columns(&mut self, columns: Vec<ColumnId>, cx: &mut Context<Self>) {
        let id = self.active_tab_id();
        let prefs = Prefs::read(cx);
        let own = self
            .tabs
            .get(&id)
            .filter(|t| prefs.per_folder_views && t.find.is_none() && t.path != THIS_PC);
        if let Some(tab) = own {
            let path = tab.path.clone();
            self.memory.set_folder_view(
                &path,
                Some(FolderViewPatch {
                    columns: Some(columns),
                    ..Default::default()
                }),
                now_ms(),
            );
            cx.emit(ExplorerEvent::MemoryChanged);
        } else {
            crate::state::update(cx, |settings| settings.list_columns = columns);
        }
        self.refresh_views(cx);
    }

    /// "Like other folders again": forgets the columns, sort and view `path` kept for itself
    /// (`setFolderView(path, null)`).
    pub fn reset_folder_view(&mut self, path: &str, cx: &mut Context<Self>) {
        self.memory.set_folder_view(path, None, now_ms());
        cx.emit(ExplorerEvent::MemoryChanged);
        self.refresh_views(cx);
    }

    /// The address bar is being typed in, or isn't any more (`editingAddress`). Anything that changes the
    /// folder shown ends it too.
    pub fn set_editing_address(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.editing_address == on {
            return;
        }
        self.editing_address = on;
        cx.notify();
    }

    /// Ejects a drive: a USB drive made safe to remove, a mounted disc image, a disc (`ejectDrive`). Windows
    /// shows its own messages; the task says whether it worked, or why not.
    #[must_use = "dropping the task cancels waiting for it; `.detach()` it to let it run"]
    pub fn eject(&mut self, path: &str, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        let ejecting = self.source.eject(path);
        cx.spawn(async move |_, _| ejecting.await)
    }
}
