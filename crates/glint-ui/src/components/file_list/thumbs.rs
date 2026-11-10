//! Small pictures in the icon cell for pictures and videos (`Thumbnail` in a row of FileList.tsx, over the
//! grid's thumbnail service in `components::file_grid::thumbnail`).
//!
//! The list tells the service which thumbnails the rows on screen need whenever those change (not every
//! frame), draws what the service has (the picture, another size's as a stand-in, or the kind's icon), frees
//! pictures the service let go of from the GPU's atlas, and lets go of its own when it goes away.

use super::pointer::layout_rows;
use super::{FileList, Frame};
use crate::components::file_grid::thumbnail::{Look, Want};
use crate::kit::prelude::*;
use glint_model::file_types::is_picture_or_video;
use std::hash::{Hash, Hasher};
use std::ops::Range;

/// Lines past the ones on screen whose thumbnails are asked for early, so scrolling doesn't show icons first.
const OVERSCAN_LINES: usize = 12;

impl FileList {
    /// Tells the thumbnail service which thumbnails the lines in `lines` (and a few past them) show, if that
    /// differs from last time, and frees the pictures it let go of.
    pub(super) fn show_thumbnails(&self, frame: &Frame, lines: &Range<usize>, window: &mut Window, cx: &mut App) {
        // The list measures one line first, with a range that isn't what will be shown: telling the service
        // about it would make it drop the real ones and ask for them again.
        if *lines == (0..1) && frame.lines > 1 {
            return;
        }
        let from = lines.start.saturating_sub(OVERSCAN_LINES);
        let to = lines.end.saturating_add(OVERSCAN_LINES).min(frame.lines);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (
            frame.thumbnails,
            &frame.lane,
            frame.box_px.to_bits(),
            frame.scale.to_bits(),
        )
            .hash(&mut hasher);
        let wanted = |line: usize| {
            layout_rows::row_of_line(&frame.rows, line)
                .and_then(|i| frame.rows.get(i))
                .filter(|entry| frame.thumbnails && is_picture_or_video(entry))
        };
        for entry in (from..to).filter_map(wanted) {
            (&entry.path, entry.modified, entry.size).hash(&mut hasher);
        }
        let fingerprint = hasher.finish();
        if self.thumbs_shown.get() != fingerprint {
            self.thumbs_shown.set(fingerprint);
            let wants: Vec<Want> = (from..to)
                .filter_map(wanted)
                .filter_map(|entry| Want::for_entry(entry, frame.box_px, frame.scale))
                .collect();
            let (viewer, lane) = (self.viewer, frame.lane.clone());
            self.thumbs
                .update(cx, |thumbs, cx| thumbs.show(viewer, &lane, wants, cx));
        }
        for freed in self.thumbs.update(cx, |thumbs, _| thumbs.take_evicted()) {
            window.drop_image(freed).ok();
        }
    }

    /// What the icon cell of `entry` draws: its thumbnail or a stand-in, or None for the kind's icon.
    pub(super) fn look_of(&self, frame: &Frame, entry: &glint_model::entries::FileEntry, cx: &App) -> Option<Look> {
        if !frame.thumbnails || !is_picture_or_video(entry) {
            return None;
        }
        match self.thumbs.read(cx).look(entry, frame.box_px, frame.scale) {
            Look::Icon => None,
            picture => Some(picture),
        }
    }

    /// Call when the list is swapped out (the tab went to the grid) but the entity is kept: its thumbnails are
    /// let go of, and asked for again when it is drawn. Dropping the entity does the same.
    pub fn pause(&mut self, cx: &mut Context<Self>) {
        let id = cx.entity_id();
        self.thumbs.update(cx, |thumbs, cx| thumbs.release(id, cx));
        self.thumbs_shown.set(0);
        self.press = None;
        self.marquee = None;
        self.tracking.set(false);
    }
}
