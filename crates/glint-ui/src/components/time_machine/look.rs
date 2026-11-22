//! Space on a historical row: Quick Look over Time Machine, showing that version of the file.
//!
//! The version is staged first (`glint_core::time_machine::stage_item` copies it from the Git object, the shadow
//! copy or the backup into a read-only temporary file, off the UI thread) and then shown by the same previewer
//! Quick Look uses ([`EntryView`] in [`Place::QuickLook`]), in a dialog of its own, so Time Machine stays
//! where it is underneath. Quick Look proper follows the explorer's selection, which has nothing to do with a
//! file that only exists in the past; this host takes the same keys (context `QuickLook`): Escape and Space close it
//! and the focus goes back to the row, Page Up and Page Down scroll the preview.

use crate::actions;
use crate::components::preview::{EntryEvent, EntryView, Place};
use crate::kit::prelude::*;
use crate::{t, theme};
use glint_model::entries::{with_details, RawEntry};
use glint_model::time_machine::{RestoreSource, TimeEntry};
use std::cell::Cell;
use std::rc::Rc;

/// The dialog's size in the usual state, as Quick Look's.
const MAX_WIDTH: f32 = 1280.;
const MAX_HEIGHT: f32 = 896.;
const MARGIN: f32 = 32.;
const MAXIMIZED_MARGIN: f32 = 10.;

/// Whether a row's version can be looked at: a file, and one whose content still exists somewhere (a moved
/// file the journal only remembers has none).
pub fn can_look(entry: &TimeEntry) -> bool {
    !entry.dir && !matches!(entry.restore, RestoreSource::UsnMoved { now_path: None, .. })
}

/// The dialog's content.
struct HistoricLook {
    view: Entity<EntryView>,
    focus: FocusHandle,
    maximized: Rc<Cell<bool>>,
    _sub: Subscription,
}

impl HistoricLook {
    fn new(entry: glint_model::entries::FileEntry, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = cx.new(|cx| EntryView::new(entry, Place::QuickLook, window, cx));
        let sub = cx.subscribe_in(&view, window, |this, _, event: &EntryEvent, window, cx| match event {
            EntryEvent::Close => window.close_dialog(cx),
            EntryEvent::ToggleMaximize => {
                this.maximized.set(!this.maximized.get());
                let maximized = this.maximized.get();
                this.view.update(cx, |view, cx| view.set_maximized(maximized, cx));
                cx.notify();
            }
        });
        HistoricLook {
            view,
            focus: cx.focus_handle(),
            maximized: Rc::default(),
            _sub: sub,
        }
    }
}

impl Render for HistoricLook {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let name = self.view.read(cx).entry().name.clone();
        div()
            .id("time-machine-look")
            .role(Role::Dialog)
            .aria_label(t!("Quick Look: {name}", name = name))
            .test_support()
            .key_context("QuickLook")
            .track_focus(&self.focus)
            .size_full()
            .overflow_hidden()
            .rounded(theme::size::RADIUS_XL)
            .bg(t.card)
            .text_start()
            .on_action(cx.listener(|_, _: &actions::CloseQuickLook, window, cx| window.close_dialog(cx)))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollDown, _, cx| {
                this.view.update(cx, |view, cx| view.page(true, cx));
            }))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollUp, _, cx| {
                this.view.update(cx, |view, cx| view.page(false, cx));
            }))
            .child(self.view.clone())
    }
}

/// Shows the staged file `staged`, which is `entry`'s historical version, in Quick Look over Time Machine.
pub fn open(entry: &TimeEntry, staged: String, window: &mut Window, cx: &mut App) {
    let file = with_details(
        RawEntry {
            name: entry.name.clone(),
            path: Some(staged),
            dir: false,
            size: entry.size,
            modified: entry.modified,
            ..RawEntry::default()
        },
        "",
    );
    let look = cx.new(|cx| HistoricLook::new(file, window, cx));
    let maximized = look.read(cx).maximized.clone();
    let focus = look.read(cx).focus.clone();
    window.open_dialog(cx, move |dialog, window, _| {
        let viewport = window.viewport_size();
        let margin = if maximized.get() { MAXIMIZED_MARGIN } else { MARGIN };
        let (cap_w, cap_h) = if maximized.get() {
            (f32::MAX, f32::MAX)
        } else {
            (MAX_WIDTH, MAX_HEIGHT)
        };
        let width = (f32::from(viewport.width) - margin * 2.).min(cap_w).max(0.);
        let height = (f32::from(viewport.height) - margin * 2.).min(cap_h).max(0.);
        dialog
            .w(px(width))
            .h(px(height))
            .centered()
            .close_button(false)
            .p_0()
            .on_ok(|_, _, _| false)
            .child(look.clone())
    });
    // The dialog layer takes the focus when it opens; the box takes it from there, so its keys arrive.
    window.defer(cx, move |window, cx| window.focus(&focus, cx));
}
