//! Scrubbing a video over its tile (`<VideoScrub>` of FileGrid.tsx's `renderTile`): one [`player::Scrub`] over the
//! video tile the pointer is on. The overlay is made when the pointer arrives and dropped when it leaves, so a
//! grid of thousands of videos holds at most one (the TypeScript mounted one per video tile).
//!
//! The grid owns a [`ScrubSlot`]; its tile hover handler calls [`ScrubSlot::enter`] and [`ScrubSlot::leave`], and
//! the tile draws [`ScrubSlot::over`] inside its thumbnail square. The `Scrub` reads its own frames (as the pointer
//! arrives), follows the pointer across the tile and lets its pictures go of the GPU's atlas when the pointer
//! leaves, and by `release_retired` when it is dropped.

use crate::components::preview::player::Scrub;
use crate::kit::prelude::*;
use glint_model::entries::{is_archived, is_list_only, is_locked, FileEntry};
use glint_model::file_types::is_video_entry;

/// Whether a tile gets a scrub overlay: a video that is on the disk and readable, not one in an archive or an
/// export (`isVideoEntry && !isLocked && !isListOnly && !isArchived`).
pub(super) fn scrubbable(entry: &FileEntry) -> bool {
    is_video_entry(entry) && !is_locked(entry) && !is_list_only(entry) && !is_archived(entry)
}

/// The one overlay, for the tile the pointer is on.
#[derive(Default)]
pub(super) struct ScrubSlot {
    current: Option<(String, Entity<Scrub>)>,
}

impl ScrubSlot {
    /// The pointer came onto the tile of the video at `path`, whose thumbnail is `thumb` logical pixels across on
    /// a screen of `scale`: an overlay is made for it (the one for another tile is dropped). Whether the grid needs
    /// drawing again is for the caller to say.
    pub fn enter<T: 'static>(&mut self, path: &str, thumb: f32, scale: f32, cx: &mut Context<T>) {
        if self.current.as_ref().is_some_and(|(p, _)| p == path) {
            return;
        }
        let width = Scrub::width_for(thumb, scale);
        let owned = path.to_string();
        self.current = Some((owned.clone(), cx.new(|cx| Scrub::new(owned, width, cx))));
    }

    /// The pointer left the tile of `path`: its overlay is dropped (not another tile's, which may have been entered
    /// already). Whether anything changed.
    pub fn leave(&mut self, path: &str) -> bool {
        if self.current.as_ref().is_some_and(|(p, _)| p == path) {
            self.current = None;
            return true;
        }
        false
    }

    /// Drops whatever overlay there is.
    pub fn clear(&mut self) {
        self.current = None;
    }

    /// The path the overlay is over.
    #[cfg(test)]
    pub fn path(&self) -> Option<&str> {
        self.current.as_ref().map(|(p, _)| p.as_str())
    }

    /// The overlay and the path it is over, for a frame.
    pub fn current(&self) -> Option<(String, Entity<Scrub>)> {
        self.current.clone()
    }
}
