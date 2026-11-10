//! A file Glint can't show itself, as Windows' own previewer for its type shows it: Word, Excel and PowerPoint
//! documents, fonts… (`ShellPreview` in ShellPreview.tsx, with `platform::preview_host` behind it).
//!
//! The handler draws into an owned pop-up window kept over this box (GPUI's own window is a DirectComposition
//! window, where child windows are invisible). So this view measures its box while drawing, tells the host the
//! rectangle on every frame (a call that does nothing unless the rectangle changed), and
//!
//! - **hides** the pop-up while something covers it: a dialog (the palette, a question), or a menu (the workspace
//!   calls [`cover`] and [`uncover`] for those, from `MenuEvent::Shown` and `Closed`);
//! - **gives way** to Quick Look: the host shows one preview at a time, and Quick Look's wins over the pane's. The
//!   pane's is shown again once Quick Look has let the host go (`taken` says it was taken meanwhile);
//! - **unloads** the handler when the view goes (the item changed, the pane closed).
//!
//! Until the handler is up, and where there is none (or it is blocked: files from the internet, Glint running
//! as administrator), `fallback` stays: the file's icon, or its bytes. Underneath the pop-up it is just covered.
//! The host's reason for not previewing is shown under it unless `quiet`.

use super::sources::{sources, Native};
use crate::kit::prelude::*;
use crate::platform::preview_host::Event;
use crate::theme;
use glint_model::entries::FileEntry;
use gpui_kit::{canvas, Bounds, Task};
use std::cell::Cell;

/// The pane's claim on the preview host (`preview_host::show`'s claimant).
pub const PANE: u32 = 1;
/// Quick Look's, which wins over the pane's.
pub const QUICK_LOOK: u32 = 2;
/// A box smaller than this isn't worth a preview (`r.width < 8`).
const TOO_SMALL: f32 = 8.;

thread_local! {
    /// Raised whenever Quick Look takes the host: a pane preview shown before must be shown again.
    static TAKEN: Cell<u64> = const { Cell::new(0) };
    /// How many Quick Look hosts are alive (one at most, but one that is closing counts until it is gone).
    static QUICK_LOOKS: Cell<usize> = const { Cell::new(0) };
    /// A menu is open over the window.
    static COVERED: Cell<bool> = const { Cell::new(false) };
}

fn taken() -> u64 {
    TAKEN.with(Cell::get)
}

/// A menu (or anything else that is not a dialog) covers the window: Windows' previewers are hidden until
/// [`uncover`]. The workspace calls it for `MenuEvent::Shown`.
pub fn cover(cx: &mut App) {
    if !COVERED.with(|c| c.replace(true)) {
        cx.refresh_windows();
    }
}

/// The menu is gone: Windows' previewers are shown again (`MenuEvent::Closed`).
pub fn uncover(cx: &mut App) {
    if COVERED.with(|c| c.replace(false)) {
        cx.refresh_windows();
    }
}

fn menu_open() -> bool {
    COVERED.with(Cell::get)
}

/// How the host stands for this preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Waiting for the box's place, or for the handler.
    Starting,
    /// On screen.
    Ready,
    /// No previewer, with why (empty when the type simply has none).
    Unavailable(String),
    /// Glint let the file go for a rename, move or delete.
    Released,
}

/// Windows' previewer for one file, over a box, with `fallback` underneath.
pub struct ShellHost {
    path: String,
    claimant: u32,
    quiet: bool,
    fallback: AnyView,
    native: Native,
    state: State,
    /// `show` was called and the handler wasn't unloaded since.
    shown: bool,
    /// Hidden now for a dialog or menu.
    hidden: bool,
    /// [`taken`] when this preview was shown.
    taken: u64,
    /// The box, as drawn last.
    bounds: Cell<Option<Bounds<Pixels>>>,
    _events: Option<Task<()>>,
}

impl ShellHost {
    /// A host for `entry` for `claimant` ([`PANE`] or [`QUICK_LOOK`]). `fallback` is drawn in the box until the
    /// previewer covers it, and instead of it when there is none. `quiet` leaves the host's reason unsaid.
    pub fn new(entry: &FileEntry, claimant: u32, quiet: bool, fallback: AnyView, cx: &mut Context<Self>) -> Self {
        if claimant == QUICK_LOOK {
            QUICK_LOOKS.with(|n| n.set(n.get() + 1));
        }
        ShellHost {
            path: entry.path.clone(),
            claimant,
            quiet,
            fallback,
            native: sources(cx).native,
            state: State::Starting,
            shown: false,
            hidden: false,
            taken: taken(),
            bounds: Cell::new(None),
            _events: None,
        }
    }

    /// How the host stands.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Whether the handler was asked for and not unloaded.
    pub fn is_shown(&self) -> bool {
        self.shown
    }

    /// Whether the pop-up is hidden now.
    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// Where the box was drawn last, in the window.
    pub fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.bounds.get()
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        self.state = match event {
            Event::Ready => State::Ready,
            Event::Unavailable { reason } => State::Unavailable(reason),
            Event::Released => {
                self.shown = false;
                State::Released
            }
        };
        cx.notify();
    }

    /// Asks the host for the previewer over `bounds`.
    fn show(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let dark = theme::tokens(cx).dark;
        if self.claimant == QUICK_LOOK {
            TAKEN.with(|t| t.set(t.get().wrapping_add(1)));
        }
        self.taken = taken();
        self.shown = true;
        match (self.native.show)(window, self.claimant, Some(bounds), &self.path, dark) {
            Ok(events) => {
                self.state = State::Starting;
                self._events = Some(cx.spawn(async move |this, cx| {
                    while let Ok(event) = events.recv().await {
                        if this.update(cx, |this, cx| this.on_event(event, cx)).is_err() {
                            break;
                        }
                    }
                }));
            }
            // The window has no handle (a headless one): there is no previewer, and nothing to say.
            Err(_) => {
                self.shown = false;
                self.state = State::Unavailable(String::new());
                cx.notify();
            }
        }
    }

    /// Follows the box: shows the previewer the first time, moves it with the box, and hides it while covered.
    fn place(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.bounds.set(Some(bounds));
        if matches!(self.state, State::Unavailable(_) | State::Released) && !self.shown {
            return;
        }
        let small = f32::from(bounds.size.width) < TOO_SMALL || f32::from(bounds.size.height) < TOO_SMALL;
        // Quick Look is a dialog, so the pane's preview is covered while it is open; Quick Look's own is not.
        let covered = small || menu_open() || (self.claimant == PANE && window.has_active_dialog(cx));
        if covered {
            if self.shown && !self.hidden {
                self.hidden = true;
                (self.native.hide)(self.claimant);
            }
            return;
        }
        // Quick Look took the host while the pane's preview was covered: shown again once it has let it go.
        let stale = self.claimant == PANE && self.taken != taken();
        if stale && QUICK_LOOKS.with(Cell::get) > 0 {
            window.request_animation_frame();
            return;
        }
        if !self.shown || stale {
            self.hidden = false;
            self.show(bounds, window, cx);
            return;
        }
        (self.native.set_rect)(self.claimant, bounds);
        if self.hidden {
            self.hidden = false;
            (self.native.reveal)(self.claimant);
        }
    }
}

impl Drop for ShellHost {
    fn drop(&mut self) {
        if self.shown {
            (self.native.unload)(self.claimant);
        }
        if self.claimant == QUICK_LOOK {
            QUICK_LOOKS.with(|n| n.set(n.get().saturating_sub(1)));
        }
    }
}

impl Render for ShellHost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let this = cx.entity();
        let measure = canvas(
            move |bounds, window, cx| {
                this.update(cx, |this, cx| this.place(bounds, window, cx));
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let note = match &self.state {
            State::Unavailable(reason) if !self.quiet && !reason.is_empty() => Some(reason.clone()),
            _ => None,
        };
        col()
            .id("preview-shell")
            .relative()
            .size_full()
            .child(measure)
            .child(div().flex_1().min_h_0().child(self.fallback.clone()))
            .children(note.map(|reason| {
                div()
                    .id("preview-shell-note")
                    .role(Role::Status)
                    .aria_label(reason.clone())
                    .test_support()
                    .px(space(6.))
                    .pb(space(6.))
                    .text_center()
                    .text_size(theme::size::SM)
                    .text_color(t.muted_foreground)
                    .child(reason)
            }))
    }
}
