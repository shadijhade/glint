//! The head's smart buttons: Map, Colors and History appear at the end of a folder's head only when they would
//! show something, never as always-on icons that open an empty panel.
//!
//! - **Map** when photos here say where they were taken. Reading that means opening pictures, so it is asked once a
//!   listing is in, off the UI thread, of the first [`MAP_LOOKS`] pictures and videos only, through
//!   `glint_core::gallery::probe` (the map's own cache: nothing is read twice). A folder with no pictures asks nothing.
//! - **Colors** when the folder holds enough pictures to surf by color ([`COLORS_FROM`]), and always in the Gallery.
//!   Counted from the folder's make-up the head already has: nothing is asked.
//! - **History** when Time Machine would have something to scrub (`glint_core::lens::has_history`: a Git repository,
//!   a backup, a shadow copy), asked once per folder in the background and kept a minute.
//!
//! A lens whose panel is open stays, lit, so it can be closed where it was opened. Until an answer is known the
//! button isn't there; when it comes it fades in. The keys, the palette and the menus reach all three everywhere.

use super::Head;
use crate::components::status_bar::summary::Category;
use crate::t;
use glint_model::entries::FileEntry;
use gpui_kit::SharedString;
use std::sync::Arc;
use std::time::Duration;

/// A folder with this many pictures can be surfed by color.
pub const COLORS_FROM: usize = 12;
/// The most pictures and videos looked into for places.
pub const MAP_LOOKS: usize = 400;
/// How long a folder's "has a history" answer is trusted.
pub const HISTORY_KEPT: Duration = Duration::from_secs(60);

/// One of the head's smart buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lens {
    Map,
    Colors,
    History,
}

impl Lens {
    /// The order they sit in at the end of the head.
    pub const ALL: [Lens; 3] = [Lens::Map, Lens::Colors, Lens::History];

    /// The element id's number, and so each pill's own animation.
    pub fn index(self) -> usize {
        match self {
            Lens::Map => 0,
            Lens::Colors => 1,
            Lens::History => 2,
        }
    }

    /// The pill's id (pane-wide unique).
    pub fn id(self) -> &'static str {
        match self {
            Lens::Map => "pane-head-photo-map",
            Lens::Colors => "pane-head-color-wheel",
            Lens::History => "pane-head-time-machine",
        }
    }

    /// The word on the pill (the map's carries its count).
    pub fn label(self, places: usize, more: bool) -> SharedString {
        match self {
            Lens::Map if places > 0 && more => t!("Map {count}+", count = places),
            Lens::Map if places > 0 => t!("Map {count}", count = places),
            Lens::Map => t!("Map"),
            Lens::Colors => t!("Colors"),
            Lens::History => t!("History"),
        }
    }

    /// What the tooltip says, with the key where there is one.
    pub fn tip(self, places: usize, more: bool) -> SharedString {
        match self {
            Lens::Map if places > 0 && more => t!(
                "Photo map: {count} or more photos with a place (Ctrl+Shift+G)",
                count = places
            ),
            Lens::Map if places == 1 => t!("Photo map: 1 photo with a place (Ctrl+Shift+G)"),
            Lens::Map if places > 0 => t!("Photo map: {count} photos with a place (Ctrl+Shift+G)", count = places),
            Lens::Map => t!("Photo map (Ctrl+Shift+G)"),
            Lens::Colors => t!("Color wheel: find photos by color"),
            Lens::History => t!("Time Machine: scrub back through this folder's history (Ctrl+Shift+H)"),
        }
    }

    /// Roughly how wide the pill is, in rem (an icon, the word, the padding), so the chips beside it can be fitted
    /// before it is measured.
    pub fn width_rems(self, places: usize, more: bool) -> f32 {
        self.label(places, more).chars().count() as f32 * 0.5 + 3.2
    }
}

/// What the head has learned about the pictures of the folder it is over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Places {
    /// How many of the looked-at pictures say where they were taken.
    pub count: usize,
    /// There were more pictures than were looked at ("12+").
    pub more: bool,
}

/// The pictures and videos of a listing to look into for places: path, size and date, the first [`MAP_LOOKS`].
/// Also says whether there were more.
pub fn pictures_of(entries: &[FileEntry]) -> (Vec<(String, u64, i64)>, bool) {
    let mut found = Vec::new();
    let mut more = false;
    for entry in entries
        .iter()
        .filter(|e| glint_model::file_types::is_picture_or_video(e))
    {
        if found.len() == MAP_LOOKS {
            more = true;
            break;
        }
        found.push((entry.path.clone(), entry.size, entry.modified));
    }
    (found, more)
}

/// How many pictures and videos the make-up says there are (what is worth looking into for places).
pub fn media_of(kinds: &[Category]) -> usize {
    kinds
        .iter()
        .filter(|kind| matches!(kind.group.name, "Images" | "Videos"))
        .map(|kind| kind.count)
        .sum()
}

/// How many pictures the make-up says there are.
pub fn images_of(kinds: &[Category]) -> usize {
    kinds
        .iter()
        .filter(|kind| kind.group.name == "Images")
        .map(|kind| kind.count)
        .sum()
}

/// Whether the folder offers the Colors lens by itself (its panel being open is a reason of its own).
pub fn colors_fit(head: &Head) -> bool {
    head.is_gallery || (head.is_folder && head.images >= COLORS_FROM)
}

/// The listing the head last looked into for places, so the same one isn't asked about twice.
pub struct Looked {
    pub path: String,
    pub entries: Arc<[FileEntry]>,
}

impl Looked {
    pub fn is(&self, path: &str, entries: &Arc<[FileEntry]>) -> bool {
        self.path == path && Arc::ptr_eq(&self.entries, entries)
    }
}
