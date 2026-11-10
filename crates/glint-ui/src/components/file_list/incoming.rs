//! Files dropped onto the list from File Explorer, the desktop, another pane or tab, or the list itself
//! (`targetAt` and `onDrag` in Explorer.tsx, `dropTarget` in FileList.tsx).
//!
//! A folder row takes the files into that folder; the empty area, the header and any other row take them into
//! the folder shown. The list answers each pointer move with what a drop there would do, so Windows shows
//! its cursor and the words on the dragged picture ("Copy to Reports", "Move to Reports"), and lights the
//! place under the pointer. The drop itself is the explorer's `drop_files`.
//!
//! Refused, as the web refused: an archive (read-only), a folder Windows keeps closed, the Recycle Bin's items
//! (but the Recycle Bin itself takes files, to recycle them), and where the files already are or into
//! themselves. Not as the web did: a remote's folder is refused at the pointer, where the web accepted it and
//! then said it is read-only, and so are Recent and the Gallery, which are searches and not folders.

use super::cells::drop_path;
use super::pointer::layout_rows;
use super::{FileList, ROW_HEIGHT};
use crate::drops;
use crate::i18n;
use crate::kit::prelude::*;
use crate::platform::dragdrop::{Answer, DropEffect, Under};
use crate::t;
use glint_model::paths::{droppable, is_gallery, is_recent, is_recycle_bin, is_remote_path, THIS_PC};

/// Where on the list a drop would land.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum DropAt {
    /// A folder row.
    Row(String),
    /// The folder shown: the empty area, the header, a row that is not a folder.
    Folder,
}

impl FileList {
    /// Where a drop of `dragged` at `at` (window pixels) would go: the place on the list, and the folder
    /// (path) it goes into. None where nothing can be dropped.
    pub(super) fn drop_dest(
        &self,
        at: Point<Pixels>,
        dragged: &[String],
        window: &Window,
        cx: &App,
    ) -> Option<(DropAt, String)> {
        let tab = self.explorer.read(cx).pane_tab(self.pane)?;
        let area = self.area.get();
        // A folder row under the pointer takes them; any other row (a file, a folder nothing can be dropped
        // into) is the folder shown.
        let folder_row = area
            .contains(&at)
            .then(|| {
                let height = f32::from(ROW_HEIGHT.to_pixels(window.rem_size()));
                let line = (f32::from(at.y - area.origin.y) + self.scroll_top()) / height;
                // A negative line is above the first row (the scroll's overshoot).
                let row = if line >= 0. {
                    layout_rows::row_of_line(&tab.rows, line as usize)
                } else {
                    None
                };
                row.and_then(|i| tab.rows.get(i)).and_then(drop_path)
            })
            .flatten();
        let (place, dest) = match folder_row {
            Some(folder) => (DropAt::Row(folder.to_string()), folder.to_string()),
            None => {
                // The folder shown: not This PC, a search, an archive (read-only), a folder Windows keeps closed.
                let plain = tab.path != THIS_PC
                    && tab.find.is_none()
                    && tab.archive().is_none()
                    && tab.no_access().is_none()
                    && !is_gallery(&tab.path)
                    && !is_recent(&tab.path);
                if !plain {
                    return None;
                }
                (DropAt::Folder, tab.path.clone())
            }
        };
        if is_remote_path(&dest) {
            return None;
        }
        // Where they all are already, or into one of them: nothing to do there.
        if !is_recycle_bin(&dest) && droppable(dragged, &dest).is_empty() {
            return None;
        }
        Some((place, dest))
    }

    /// What a folder is called on the dragged picture: a drive's name, a known folder's, else its own, in the
    /// interface language (`dropName`).
    fn drop_name(&self, path: &str, cx: &App) -> String {
        let name = self.explorer.read(cx).tab_name(path);
        glint_model::i18n::translate(i18n::language(), &name).to_string()
    }

    /// Files are dragged over the list (every pointer move, for every list: only the one under the pointer
    /// answers). Tells Windows what a drop here would do and remembers where, to light it.
    pub(super) fn answer_drag(
        &mut self,
        event: &DragMoveEvent<ExternalPaths>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let found = if event.bounds.contains(&event.event.position) {
            let dragged = drops::paths(event.drag(cx));
            self.drop_dest(event.event.position, &dragged, window, cx)
        } else {
            None
        };
        if let Some((_, dest)) = &found {
            drops::answer(
                cx,
                Answer::Accept(Under {
                    name: self.drop_name(dest, cx),
                    copy: t!("Copy to %1").to_string(),
                    moves: t!("Move to %1").to_string(),
                    // The Recycle Bin: a drop there always deletes.
                    always_move: is_recycle_bin(dest),
                }),
            );
        }
        let place = found.map(|(place, _)| place);
        if self.drop_at != place {
            self.drop_at = place;
            cx.notify();
        }
    }

    /// Files were dropped on the list (`dropFiles`): copied into the folder under the pointer, or moved with
    /// Shift (as the drag's picture said); on the Recycle Bin, recycled. A link (Alt) copies, as the web did.
    pub(super) fn take_drop(&mut self, dropped: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_at = None;
        cx.notify();
        let move_ = match drops::effect(cx) {
            DropEffect::None => return,
            DropEffect::Move => true,
            DropEffect::Copy | DropEffect::Link => false,
        };
        let paths = drops::paths(dropped);
        let Some((_, dest)) = self.drop_dest(window.mouse_position(), &paths, window, cx) else {
            return;
        };
        self.explorer
            .update(cx, |explorer, cx| explorer.drop_files(paths, &dest, move_, cx).detach());
    }
}
