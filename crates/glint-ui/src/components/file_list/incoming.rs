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
use super::lego;
use super::pointer::layout_rows;
use super::{FileList, FileListEvent, ROW_HEIGHT};
use crate::drops;
use crate::kit::prelude::*;
use crate::platform::dragdrop::{Answer, DropEffect, Under};
use crate::t;
use glint_model::file_lego::{classify_drop, LegoRecipe};
use glint_model::paths::{droppable, is_gallery, is_recent, is_recycle_bin, is_remote_path, THIS_PC};
use std::time::{Duration, Instant};

/// How long the pointer must pause over a file row before File Lego offers to combine them,
/// so quick drags across a dense file list still drop into the folder normally.
pub(super) const LEGO_DWELL: Duration = Duration::from_millis(250);

/// Where on the list a drop would land.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum DropAt {
    /// A folder row.
    Row(String),
    /// A file row ready for a contextual File Lego recipe.
    Lego { path: String, recipe: LegoRecipe },
    /// The folder shown: the empty area, the header, a row that is not a folder.
    Folder,
}

/// Active File Lego hover dwell state for a non-folder row.
pub(super) struct LegoHover {
    pub path: String,
    pub name: String,
    pub recipe: LegoRecipe,
    pub started: Instant,
    pub ready: bool,
    /// Keys the badge's ring for this dwell.
    pub turn: u64,
    /// The name the disk gave what the recipe makes (a mux's new file), asked while the pointer dwells.
    pub output: Option<String>,
    pub _timer: gpui_kit::Task<()>,
    pub _lookup: Option<gpui_kit::Task<()>>,
}

impl LegoHover {
    /// What the pill says now.
    pub fn words(&self) -> SharedString {
        lego::pill_text(&self.recipe, self.output.as_deref())
    }
}

/// Looks up the real output name for the dwell on `path`, and keeps it on that dwell when it arrives.
fn incoming_lookup(recipe: &LegoRecipe, path: String, cx: &mut Context<FileList>) -> Option<gpui_kit::Task<()>> {
    lego::lookup_output(recipe, cx, move |list, name, cx| {
        if let Some(hover) = list.lego_hover.as_mut().filter(|h| h.path.eq_ignore_ascii_case(&path)) {
            if hover.output != name {
                hover.output = name;
                cx.notify();
            }
        }
    })
}

impl FileList {
    /// What the pill says for the recipe on `path`: the real output name where the dwell has found it.
    pub(super) fn lego_words(&self, path: &str, recipe: &LegoRecipe) -> SharedString {
        match self.lego_hover.as_ref().filter(|h| h.path.eq_ignore_ascii_case(path)) {
            Some(hover) => lego::pill_text(recipe, hover.output.as_deref()),
            None => lego::pill_text(recipe, None),
        }
    }
}

impl FileList {
    fn entry_at(&self, at: Point<Pixels>, window: &Window, cx: &App) -> Option<glint_model::entries::FileEntry> {
        let tab = self.explorer.read(cx).pane_tab(self.pane)?;
        let area = self.area.get();
        if !area.contains(&at) {
            return None;
        }
        let height = f32::from(ROW_HEIGHT.to_pixels(window.rem_size()));
        let line = (f32::from(at.y - area.origin.y) + self.scroll_top()) / height;
        let row = if line >= 0. {
            layout_rows::row_of_line(&tab.rows, line as usize)
        } else {
            None
        };
        row.and_then(|i| tab.rows.get(i)).cloned()
    }

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
        self.explorer.read(cx).tab_name(path)
    }

    /// Files are dragged over the list (every pointer move, for every list: only the one under the pointer
    /// answers). Tells Windows what a drop here would do and remembers where, to light it.
    pub(super) fn answer_drag(
        &mut self,
        event: &DragMoveEvent<ExternalPaths>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.bounds.contains(&event.event.position) {
            self.lego_hover = None;
            if self.drop_at.take().is_some() {
                cx.notify();
            }
            return;
        }

        let dragged = drops::paths(event.drag(cx));
        let hovered_file = self.entry_at(event.event.position, window, cx).filter(|e| !e.dir);

        let mut active_lego: Option<(String, String, LegoRecipe)> = None;
        if let Some(entry) = hovered_file {
            if let Some(recipe) = classify_drop(&dragged, &entry.path, false) {
                let now = cx.background_executor().now();
                let same = self
                    .lego_hover
                    .as_ref()
                    .is_some_and(|h| h.path.eq_ignore_ascii_case(&entry.path));
                if same {
                    let mut armed_now = false;
                    if let Some(hover) = self.lego_hover.as_mut() {
                        hover.recipe = recipe.clone();
                        if !hover.ready && now.saturating_duration_since(hover.started) >= LEGO_DWELL {
                            hover.ready = true;
                            armed_now = true;
                        }
                        if hover.ready {
                            active_lego = Some((hover.path.clone(), hover.name.clone(), hover.recipe.clone()));
                        }
                    }
                    if armed_now {
                        lego::snap(cx);
                    }
                } else {
                    let target_path = entry.path.clone();
                    let target_name = entry.name.clone();
                    let timer_path = target_path.clone();
                    let timer = cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(LEGO_DWELL).await;
                        let _ = this.update(cx, |this, cx| {
                            let Some(hover) = this.lego_hover.as_mut() else {
                                return;
                            };
                            if !hover.path.eq_ignore_ascii_case(&timer_path) || hover.ready {
                                return;
                            }
                            hover.ready = true;
                            let words = hover.words().to_string();
                            drops::answer(
                                cx,
                                Answer::Accept(Under {
                                    name: hover.name.clone(),
                                    copy: words.clone(),
                                    moves: words,
                                    always_move: false,
                                }),
                            );
                            this.drop_at = Some(DropAt::Lego {
                                path: hover.path.clone(),
                                recipe: hover.recipe.clone(),
                            });
                            // The ring is full as the recipe arms: the cue is felt now.
                            lego::snap(cx);
                            cx.notify();
                        });
                    });
                    let lookup_path = target_path.clone();
                    let lookup = incoming_lookup(&recipe, lookup_path, cx);
                    self.lego_hover = Some(LegoHover {
                        path: target_path,
                        name: target_name,
                        recipe,
                        started: now,
                        ready: false,
                        turn: lego::next_turn(),
                        output: None,
                        _timer: timer,
                        _lookup: lookup,
                    });
                    // The ring starts filling now.
                    cx.notify();
                }
            } else {
                self.lego_hover = None;
            }
        } else {
            self.lego_hover = None;
        }

        if let Some((path, name, recipe)) = active_lego {
            let words = self.lego_words(&path, &recipe).to_string();
            drops::answer(
                cx,
                Answer::Accept(Under {
                    name,
                    copy: words.clone(),
                    moves: words,
                    always_move: false,
                }),
            );
            let place = Some(DropAt::Lego { path, recipe });
            if self.drop_at != place {
                self.drop_at = place;
                cx.notify();
            }
            return;
        }

        let found = self.drop_dest(event.event.position, &dragged, window, cx);
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

    /// Files were dropped on the list (`dropFiles`): executes a ready File Lego recipe if hovering a
    /// matching file row, or copies/moves into the folder under the pointer.
    pub(super) fn take_drop(&mut self, dropped: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        let lego = match self.drop_at.take() {
            Some(DropAt::Lego { path, recipe }) => Some((path, recipe)),
            _ => None,
        };
        self.lego_hover = None;
        cx.notify();
        if drops::effect(cx) == DropEffect::None {
            return;
        }
        let paths = drops::paths(dropped);
        let at = window.mouse_position();
        if let Some((lego_path, _)) = lego {
            if let Some(entry) = self.entry_at(at, window, cx) {
                if entry.path.eq_ignore_ascii_case(&lego_path) {
                    if let Some(fresh) = classify_drop(&paths, &entry.path, entry.dir) {
                        cx.emit(FileListEvent::FileLego(fresh));
                        return;
                    }
                }
            }
        }
        let move_ = match drops::effect(cx) {
            DropEffect::None => return,
            DropEffect::Move => true,
            DropEffect::Copy | DropEffect::Link => false,
        };
        let Some((_, dest)) = self.drop_dest(at, &paths, window, cx) else {
            return;
        };
        self.explorer
            .update(cx, |explorer, cx| explorer.drop_files(paths, &dest, move_, cx).detach());
    }
}
