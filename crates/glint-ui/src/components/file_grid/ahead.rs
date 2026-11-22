//! Scroll-ahead: the thumbnails one screenful beyond the tiles on screen, in the direction the grid is scrolling,
//! asked for as prefetches (the thumbnail service's lowest lane) so that a tile that scrolls in already has its
//! picture on its first frame instead of its icon.
//!
//! It costs nothing at rest: nothing is asked until the grid has moved, a grid that stops asks nothing more, and
//! a frame that draws the same tiles as the last one asks nothing again. The direction reversing, the folder or
//! the tile size changing drops what was asked for ahead (real asks are never touched), and a prefetch is only
//! ever made when no real thumbnail waits.

use super::thumbnail::{is_eligible, Thumbnails, Want};
use super::GridLayout;
use crate::explorer::Rows;
use crate::kit::prelude::*;
use std::hash::{Hash, Hasher};

/// The most thumbnails asked for ahead at once: a screenful of the biggest windows, not a whole folder.
const MAX_AHEAD: usize = 160;

/// A scroll smaller than this (pixels) is no movement: layout rounding, not a wheel.
const MOVED: f32 = 0.5;

/// Which way the grid scrolls, and what was asked for ahead of it.
#[derive(Default)]
pub(super) struct ScrollAhead {
    /// The scroll position of the last frame.
    top: f32,
    /// 1 scrolling down (toward the end), -1 up, 0 not yet moved since the folder or the size changed.
    dir: i8,
    /// A fingerprint of what the grid shows (folder, number of items, tile size, scale): another one starts over.
    context: u64,
    /// The lines last asked for ahead (first, end) and the direction, so the same ones are not asked again.
    asked: Option<(usize, usize, i8)>,
    /// The tab the prefetches were asked in.
    lane: String,
}

impl ScrollAhead {
    /// Called once a frame with the grid's place: asks for the next screenful in the direction of travel when the
    /// grid has moved, and drops what was asked for when the direction reverses or `context` changed.
    #[allow(clippy::too_many_arguments)]
    pub fn follow(
        &mut self,
        thumbs: &Entity<Thumbnails>,
        lane: &str,
        context: u64,
        rows: &Rows,
        layout: GridLayout,
        top: f32,
        body_height: f32,
        (css_box, scale): (f32, f32),
        cx: &mut App,
    ) {
        let moved = top - self.top;
        self.top = top;
        if context != self.context {
            self.context = context;
            self.forget(thumbs, cx);
            self.dir = 0;
            self.lane = lane.to_string();
            return;
        }
        let dir = if moved > MOVED {
            1
        } else if moved < -MOVED {
            -1
        } else {
            self.dir
        };
        if dir != self.dir {
            self.dir = dir;
            self.forget(thumbs, cx);
        }
        let len = rows.len();
        if dir == 0 || body_height <= 0.0 || len == 0 {
            return;
        }
        let cols = layout.cols.max(1);
        let lines = len.div_ceil(cols);
        let row_height = layout.row_height.max(1.0);
        let first = ((top / row_height).floor().max(0.0) as usize).min(lines);
        let end = (((top + body_height) / row_height).ceil().max(0.0) as usize).min(lines);
        let screen = end.saturating_sub(first).max(1);
        let (from, to) = if dir > 0 {
            (end, end.saturating_add(screen).min(lines))
        } else {
            (first.saturating_sub(screen), first)
        };
        if self.asked == Some((from, to, dir)) {
            return;
        }
        self.asked = Some((from, to, dir));
        if from >= to {
            return;
        }
        // Nothing on screen has a thumbnail (folders, plain documents): the folder is not one to prefetch for.
        let on_screen = (first * cols)..(end * cols).min(len);
        if !on_screen.clone().any(|i| rows.get(i).is_some_and(is_eligible)) {
            return;
        }
        let ahead = (from * cols)..(to * cols).min(len);
        let nearest_first: Box<dyn Iterator<Item = usize>> = if dir > 0 {
            Box::new(ahead)
        } else {
            Box::new(ahead.rev())
        };
        let wants: Vec<Want> = nearest_first
            .filter_map(|i| rows.get(i))
            .filter_map(|entry| Want::for_entry(entry, css_box, scale))
            .take(MAX_AHEAD)
            .collect();
        if !wants.is_empty() {
            thumbs.update(cx, |thumbs, cx| thumbs.prefetch(lane, wants, cx));
        }
    }

    /// Drops what was asked for ahead, if anything was.
    fn forget(&mut self, thumbs: &Entity<Thumbnails>, cx: &mut App) {
        if self.asked.take().is_some() {
            let lane = std::mem::take(&mut self.lane);
            thumbs.update(cx, |thumbs, _| thumbs.cancel_prefetches(&lane));
            self.lane = lane;
        }
    }

    /// The grid is swapped out: nothing asked ahead stays, and the direction is forgotten.
    pub fn release(&mut self, thumbs: &Entity<Thumbnails>, cx: &mut App) {
        self.forget(thumbs, cx);
        self.dir = 0;
        self.context = 0;
    }
}

/// What the grid shows, as a number: another one means the scroll position says nothing about direction.
pub(super) fn context_of(lane: &str, view_key: &str, items: usize, cols: usize, css_box: f32, scale: f32) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (lane, view_key, items, cols, css_box.to_bits(), scale.to_bits()).hash(&mut hasher);
    hasher.finish()
}
