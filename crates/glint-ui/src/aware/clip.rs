//! Watching the clipboard. Card D1 of docs/gpui/aware.
//!
//! Windows counts clipboard changes; asking for the count is free, reading what is on it is not, so the entity
//! looks at the count a little under twice a second and reads only when it moved.

use super::Aware;
use crate::core_bridge::{self, Core};
use crate::kit::prelude::*;
use glint_core::host::Manager as _;
use glint_model::aware::{copied_hash, Clip, CopiedHash};
use glint_model::paths::{base_name, parent_path};
use std::time::Duration;

/// How often the count is looked at.
pub(super) const EVERY: Duration = Duration::from_millis(700);
/// How many names a [`Clip::Files`] keeps.
const NAMES: usize = 5;

/// The task that runs for the life of the entity.
pub(super) fn watch(cx: &mut Context<Aware>) -> Task<()> {
    cx.spawn(async move |this, cx| loop {
        cx.background_executor().timer(EVERY).await;
        // Only while a window of the app is in front: nobody sees the answer otherwise.
        let Ok(Some(reading)) = this.update(cx, |this, cx| {
            cx.active_window()?;
            let seq = (this.sources.clip_seq)();
            if this.clip_seen == Some(seq) {
                return None;
            }
            this.clip_seen = Some(seq);
            let read = this.sources.clip.clone();
            Some(read(cx))
        }) else {
            if this_is_gone(&this) {
                return;
            }
            continue;
        };
        let clip = reading.await;
        let gone = this
            .update(cx, |this, cx| {
                // Text that is a checksum: whatever is selected is compared with it from now on.
                let copied = if clip == Clip::Text { copied_hash_of(cx) } else { None };
                let changed = this.clip != clip || this.copied != copied;
                let looks = copied.is_some() && this.copied != copied;
                this.clip = clip;
                this.copied = copied;
                if changed {
                    cx.notify();
                }
                if looks {
                    this.look_at_selection(cx);
                }
            })
            .is_err();
        if gone {
            return;
        }
    })
}

/// Whether the entity has been dropped.
fn this_is_gone(this: &WeakEntity<Aware>) -> bool {
    this.upgrade().is_none()
}

/// What is on the clipboard now, from Windows: files first, then a picture, then text.
pub(super) fn read(cx: &mut App) -> Task<Clip> {
    let main = main_window(cx);
    cx.spawn(async move |cx| {
        if let Some(main) = main {
            let files = core_bridge::run(glint_core::shell::clipboard_get_files(main)).await;
            if let Ok(Ok(Some(files))) = files {
                if let Some(clip) = files_clip(&files.paths, files.cut) {
                    return clip;
                }
            }
        }
        cx.update(|cx| from_system(cx))
    })
}

/// The checksum the clipboard's text is, if it is one.
fn copied_hash_of(cx: &App) -> Option<CopiedHash> {
    let item = cx.read_from_clipboard()?;
    item.entries.iter().find_map(|entry| match entry {
        ClipboardEntry::String(string) => copied_hash(string.text()),
        _ => None,
    })
}

/// A picture or text, if that is what the clipboard holds.
fn from_system(cx: &App) -> Clip {
    let Some(item) = cx.read_from_clipboard() else {
        return Clip::Empty;
    };
    let mut text = false;
    for entry in &item.entries {
        match entry {
            ClipboardEntry::Image(_) => return Clip::Picture,
            ClipboardEntry::String(string) if !string.text().is_empty() => text = true,
            _ => {}
        }
    }
    if text {
        Clip::Text
    } else {
        Clip::Empty
    }
}

/// Files copied or cut: how many, the first names, and the folder they all share. None when there are none.
pub(super) fn files_clip(paths: &[String], cut: bool) -> Option<Clip> {
    let first = paths.first()?;
    let parent = parent_path(first);
    let shared = paths.iter().all(|path| parent_path(path).eq_ignore_ascii_case(&parent));
    Some(Clip::Files {
        count: paths.len(),
        cut,
        from: shared.then_some(parent),
        names: paths.iter().take(NAMES).map(|path| base_name(path).to_string()).collect(),
    })
}

/// The window shell calls take as their owner.
// TODO(coordinator): from core_bridge
fn main_window(cx: &App) -> Option<glint_core::host::Window> {
    let core = cx.try_global::<Core>()?;
    core.app
        .window(crate::workspace::MAIN)
        .or_else(|| core.app.windows().into_values().next())
}
